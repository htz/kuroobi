//! NNUE-style non-linear evaluator built on the existing pattern features.
//!
//! The linear evaluator ([`crate::evaluator`]) sums one scalar weight per
//! active pattern cell. Its held-out MSE floors around 39 disc² because a
//! linear model cannot represent feature interactions. This module reuses the
//! *same* incrementally-maintained pattern indices but routes them through a
//! small network:
//!
//! ```text
//!   active features ── feature transformer (shared) ──▶ accumulator (H)
//!                                                          │ ReLU
//!                                                          ▼
//!                                    per-stage linear read-out ──▶ score
//! ```
//!
//! The feature transformer is a learned H-vector per pattern cell, summed like
//! `eval_sum` but vector-valued — so the accumulator can be maintained
//! incrementally during search exactly as the scalar sum is. The single ReLU
//! is the non-linearity a linear model lacks; the read-out is bucketed by
//! stage (disc count) to keep phase specificity.
//!
//! Weights are f32 here for training/validation; a quantized int16/int8 path
//! for search follows once this beats the linear floor.

use crate::board::Board;
use crate::color::Color;
use crate::evaluator::STAGE_COUNT;
use crate::pattern::Pattern;
use crate::pattern_index::{PatternIndexer, PatternIndices, MAX_MASKS};
use crate::position::Position;

/// Accumulator width (feature-transformer output dimension). Smaller H means
/// a proportionally cheaper incremental update (the search hot path); the
/// non-linearity survives well below 64.
pub const H: usize = 16;

/// Both perspectives' rows stored/updated together (Black then White), so one
/// contiguous add/sub maintains the whole accumulator per feature change.
const H2: usize = 2 * H;

/// `acc[i] += new[i] - old[i]` over `H2` int16 lanes (both perspectives).
/// NEON on aarch64 (int16x8, so H2=32 is four vector ops), scalar elsewhere.
#[inline]
unsafe fn acc_row_addsub(acc: &mut [i16; H2], new: *const i16, old: *const i16) {
    #[cfg(target_arch = "aarch64")]
    {
        use std::arch::aarch64::*;
        let mut i = 0;
        while i + 8 <= H2 {
            let a = vld1q_s16(acc.as_ptr().add(i));
            let n = vld1q_s16(new.add(i));
            let o = vld1q_s16(old.add(i));
            vst1q_s16(acc.as_mut_ptr().add(i), vaddq_s16(a, vsubq_s16(n, o)));
            i += 8;
        }
        while i < H2 {
            let v = (*acc.get_unchecked(i)).wrapping_add((*new.add(i)).wrapping_sub(*old.add(i)));
            *acc.get_unchecked_mut(i) = v;
            i += 1;
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        for i in 0..H2 {
            acc[i] = acc[i].wrapping_add((*new.add(i)).wrapping_sub(*old.add(i)));
        }
    }
}

/// `Σ_h relu(acc[h]) · w[h]` (i64) over H int16 lanes, ReLU on the fly.
/// NEON widening multiply on aarch64, scalar elsewhere. Called once per leaf.
#[inline]
fn readout_dot(acc: &[i16], w: &[i16]) -> i64 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        use std::arch::aarch64::*;
        let zero = vdupq_n_s16(0);
        let mut sum = vdupq_n_s32(0);
        let mut h = 0;
        while h + 8 <= H {
            let a = vmaxq_s16(vld1q_s16(acc.as_ptr().add(h)), zero); // ReLU
            let ww = vld1q_s16(w.as_ptr().add(h));
            sum = vmlal_s16(sum, vget_low_s16(a), vget_low_s16(ww));
            sum = vmlal_high_s16(sum, a, ww);
            h += 8;
        }
        let mut acc64 = vaddvq_s32(sum) as i64;
        while h < H {
            acc64 += (*acc.get_unchecked(h)).max(0) as i64 * *w.get_unchecked(h) as i64;
            h += 1;
        }
        acc64
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut sum: i64 = 0;
        for h in 0..H {
            sum += acc[h].max(0) as i64 * w[h] as i64;
        }
        sum
    }
}

/// Incrementally-maintained network input for search: the pattern indices
/// plus both perspectives' H-dim accumulators. Updated on make/unmake so a
/// leaf eval is O(H) instead of an O(features·H) rebuild.
///
/// The accumulators are int16 (quantized) so the update is a NEON add: each
/// feature contributes ≤ ~256 and there are 64 masks, so the sum stays inside
/// i16. Requires [`Nnue::quantize`] to have been called.
#[derive(Clone)]
pub struct Accumulator {
    indices: PatternIndices,
    /// `[Black perspective (H) | White perspective (H)]`, maintained together.
    acc: [i16; H2],
}

/// Raw pointers into an [`Nnue`]'s trainable arrays for Hogwild SGD.
pub struct NnueView {
    ft: *mut f32,
    ft_bias: *mut f32,
    out_w: *mut f32,
    out_b: *mut f32,
}
// SAFETY: the workers only ever add to disjoint-ish sparse cells; racing
// updates cost at most a lost step, never memory unsafety (same argument as
// the linear trainer's `WeightView`).
unsafe impl Send for NnueView {}
unsafe impl Sync for NnueView {}

/// One NNUE model over a fixed pattern library.
pub struct Nnue {
    patterns: &'static [Pattern],
    indexer: PatternIndexer,
    n_masks: usize,
    /// Flat start offset of each mask's pattern table (mask -> feature base).
    mask_off: Vec<u32>,
    /// Total distinct feature cells (sum of 3^size over patterns).
    n_features: usize,

    /// Feature transformer: `ft[feature * H + h]`. Shared across stages.
    ft: Vec<f32>,
    /// Accumulator bias, added once: `[H]`.
    ft_bias: Vec<f32>,
    /// Per-stage read-out weights: `out_w[stage * H + h]`.
    out_w: Vec<f32>,
    /// Per-stage read-out bias: `[STAGE_COUNT]`.
    out_b: Vec<f32>,

    // Quantized inference copies (built by `quantize`).
    /// Interleaved transformer: `ftc_i16[feature*H2 ..]` holds the Black row
    /// (H) then the pre-swapped White row (H). One contiguous 2H add/sub then
    /// maintains both perspectives, halving the loads in the hot loop.
    ftc_i16: Vec<i16>,
    ft_bias_i16: [i16; H2],
    out_w_i16: Vec<i16>,
    /// Scale back the i64 read-out accumulation into disc-difference f32:
    /// `1 / (ft_scale * w_scale)`.
    out_scale: f32,
}

impl Nnue {
    pub fn new(patterns: &'static [Pattern]) -> Nnue {
        let indexer = PatternIndexer::new(patterns);
        let n_masks = indexer.n_masks();

        // Flat feature layout: concatenate each pattern's 3^size table; a
        // mask maps to its owning pattern's base (orientations share a table),
        // exactly as `Evaluator::rebuild_flat` builds `mask_off`.
        let mut pattern_off = Vec::with_capacity(patterns.len());
        let mut off = 0u32;
        for p in patterns {
            pattern_off.push(off);
            off += p.table_size() as u32;
        }
        let n_features = off as usize;
        let mask_off: Vec<u32> = indexer
            .mask_patterns()
            .iter()
            .map(|&pi| pattern_off[pi as usize])
            .collect();

        Nnue {
            patterns,
            indexer,
            n_masks,
            mask_off,
            n_features,
            ft: vec![0.0; n_features * H],
            ft_bias: vec![0.0; H],
            out_w: vec![0.0; STAGE_COUNT * H],
            out_b: vec![0.0; STAGE_COUNT],
            ftc_i16: Vec::new(),
            ft_bias_i16: [0; H2],
            out_w_i16: vec![0; STAGE_COUNT * H],
            out_scale: 0.0,
        }
    }

    /// Build the int16 inference copies from the trained f32 weights. Call
    /// after loading/training and before any accumulator use in search.
    ///
    /// Scales are chosen so a single feature entry maps to at most ~256 (the
    /// 64-mask accumulator then stays well inside i16) and the read-out
    /// weights fill the i16 range.
    pub fn quantize(&mut self) {
        let ft_max = self.ft.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let w_max = self.out_w.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let ft_scale = 256.0 / ft_max;
        let w_scale = 32000.0 / w_max;
        self.out_scale = 1.0 / (ft_scale * w_scale);

        let q = |v: f32, s: f32| (v * s).round().clamp(-32768.0, 32767.0) as i16;

        // Black rows first (interleaved White filled in below).
        self.ftc_i16 = vec![0; self.n_features * H2];
        for f in 0..self.n_features {
            for h in 0..H {
                self.ftc_i16[f * H2 + h] = q(self.ft[f * H + h], ft_scale);
            }
        }
        for h in 0..H {
            let b = q(self.ft_bias[h], ft_scale);
            self.ft_bias_i16[h] = b;
            self.ft_bias_i16[H + h] = b;
        }
        self.out_w_i16 = self.out_w.iter().map(|&v| q(v, w_scale)).collect();

        // Fill the White half of each feature from its digit-swapped index.
        // Orientations of one pattern share a table (rewritten identically).
        for m in 0..self.n_masks {
            let base = self.mask_off[m] as usize;
            let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
            for i in 0..size {
                let src = (base + self.indexer.swapped_index(m, i)) * H2; // Black row of swapped idx
                let dst = (base + i) * H2 + H; // White half of this idx
                for h in 0..H {
                    self.ftc_i16[dst + h] = self.ftc_i16[src + h];
                }
            }
        }
    }

    pub fn n_features(&self) -> usize {
        self.n_features
    }

    /// Small deterministic weight init: break symmetry in the read-out and the
    /// bias so ReLU units don't all start dead, keep the transformer at zero
    /// (features start neutral and grow from data).
    pub fn init_weights(&mut self) {
        // A tiny fixed pattern is enough; SGD does the rest. Vary the read-out
        // per (stage, h) so the H units differentiate from step one.
        let mut s: u64 = 0x1234_5678_9abc_def0;
        let mut next = || {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            ((s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as i64 as f32) / (1i64 << 23) as f32
        };
        for w in &mut self.out_w {
            *w = next() * 0.1;
        }
        for b in &mut self.ft_bias {
            *b = 0.1; // positive so ReLU units start active
        }
    }

    /// Active features for a Black-to-move position (absolute indices; the
    /// data convention normalizes every training example to Black to move).
    #[inline]
    fn features_black(&self, indices: &PatternIndices) -> [u32; MAX_MASKS] {
        let mut f = [0u32; MAX_MASKS];
        let raw = indices.raw();
        for m in 0..self.n_masks {
            f[m] = self.mask_off[m] + raw[m] as u32;
        }
        f
    }

    /// Active features from the side-to-move's perspective (search path).
    #[inline]
    fn features_player(&self, indices: &PatternIndices, player: Color) -> [u32; MAX_MASKS] {
        if player == Color::Black {
            return self.features_black(indices);
        }
        let mut f = [0u32; MAX_MASKS];
        let raw = indices.raw();
        for m in 0..self.n_masks {
            let idx = self.indexer.swapped_index(m, raw[m] as usize) as u32;
            f[m] = self.mask_off[m] + idx;
        }
        f
    }

    /// Evaluate a board from scratch (rebuilds indices). Convenience for
    /// non-incremental callers (arena / validation).
    pub fn eval(&self, board: &Board) -> f32 {
        let ix = self.indexer.init(board.black, board.white);
        self.eval_indices(board, &ix)
    }

    /// Forward pass to a scalar score (disc-difference units).
    pub fn eval_indices(&self, board: &Board, indices: &PatternIndices) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let feats = self.features_player(indices, board.player());
        self.forward(&feats, stage)
    }

    /// Forward from explicit features + stage.
    fn forward(&self, feats: &[u32; MAX_MASKS], stage: usize) -> f32 {
        let mut acc = self.ft_bias.clone();
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            let row = &self.ft[base..base + H];
            for h in 0..H {
                acc[h] += row[h];
            }
        }
        let ow = &self.out_w[stage * H..stage * H + H];
        let mut out = self.out_b[stage];
        for h in 0..H {
            if acc[h] > 0.0 {
                out += ow[h] * acc[h];
            }
        }
        out
    }

    /// Build the pattern indices for a Black-to-move position (training path).
    pub fn indices(&self, black: u64, white: u64) -> PatternIndices {
        self.indexer.init(black, white)
    }

    /// Build a dual-perspective incremental accumulator for `board` (i16).
    ///
    /// The network is trained on Black-to-move absolute features, so a
    /// White-to-move position is scored from the colour-swapped view. Two
    /// accumulators are kept — `black` over absolute features, `white` over
    /// swap-indexed features — both updated incrementally so either side's
    /// leaf eval is O(H). Needs [`quantize`](Self::quantize).
    pub fn accumulator(&self, board: &Board) -> Accumulator {
        let indices = self.indexer.init(board.black, board.white);
        let mut acc = self.ft_bias_i16;
        for m in 0..self.n_masks {
            let raw = indices.raw()[m] as usize;
            let f = (self.mask_off[m] as usize + raw) * H2;
            for i in 0..H2 {
                acc[i] = acc[i].wrapping_add(self.ftc_i16[f + i]);
            }
        }
        Accumulator { indices, acc }
    }

    /// Update `acc` for `mover` playing `pos` and flipping `flipped`. Mirrors
    /// `PatternIndexer::apply` (placed empty→mover, flips opponent→mover).
    #[inline]
    pub fn acc_apply(&self, acc: &mut Accumulator, pos: Position, flipped: u64, mover: Color) {
        let md = mover.index() as u16;
        self.acc_square(acc, pos.index(), md.wrapping_sub(2));
        let flip_diff = md.wrapping_sub(1 - md);
        let mut f = flipped;
        while f != 0 {
            let sq = f.trailing_zeros() as u8;
            f &= f - 1;
            self.acc_square(acc, sq, flip_diff);
        }
    }

    /// Exact inverse of [`acc_apply`](Self::acc_apply).
    #[inline]
    pub fn acc_undo(&self, acc: &mut Accumulator, pos: Position, flipped: u64, mover: Color) {
        let md = mover.index() as u16;
        self.acc_square(acc, pos.index(), 2u16.wrapping_sub(md));
        let flip_diff = (1 - md).wrapping_sub(md);
        let mut f = flipped;
        while f != 0 {
            let sq = f.trailing_zeros() as u8;
            f &= f - 1;
            self.acc_square(acc, sq, flip_diff);
        }
    }

    /// One square's colour change: shift every affected mask's index and swap
    /// its feature vector into both accumulators (add new row, subtract old).
    #[inline]
    fn acc_square(&self, acc: &mut Accumulator, sq: u8, digit_diff: u16) {
        let ftc = self.ftc_i16.as_ptr();
        let raw = acc.indices.raw_mut();
        let vec = &mut acc.acc;
        // Direct loop over the affected masks (no closure), so the compiler can
        // keep the accumulator in registers across the whole square update.
        for e in self.indexer.square_entries(sq) {
            let mask = e.mask as usize;
            let delta = digit_diff.wrapping_mul(e.pow3);
            let old = raw[mask] as usize;
            let new = raw[mask].wrapping_add(delta) as usize;
            raw[mask] = new as u16;
            let base = self.mask_off[mask] as usize;
            // SAFETY: offsets stay within their pattern tables.
            unsafe {
                acc_row_addsub(vec, ftc.add((base + new) * H2), ftc.add((base + old) * H2));
            }
        }
    }

    /// Evaluate from the incremental accumulator (side-to-move perspective).
    #[inline]
    pub fn eval_acc(&self, acc: &Accumulator, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black {
            &acc.acc[0..H]
        } else {
            &acc.acc[H..H2]
        };
        let ow = &self.out_w_i16[stage * H..stage * H + H];
        // out = bias + scale · Σ relu(acc[h]) · out_w[h]
        let sum = readout_dot(v, ow);
        self.out_b[stage] + sum as f32 * self.out_scale
    }

    /// One SGD step on a Black-to-move example at `stage`. Returns squared error.
    ///
    /// Single hidden layer: `out = b[s] + Σ_h w[s][h]·relu(acc[h])`,
    /// `acc[h] = ftb[h] + Σ_f FT[f][h]`. MSE loss; the 2 in `d/dout = 2·err`
    /// is folded into `lr`. Gradients into the transformer use the *old*
    /// read-out weights (compute `delta` before mutating `out_w`).
    pub fn train_black(&mut self, indices: &PatternIndices, stage: usize, target: f32, lr: f32) -> f32 {
        let feats = self.features_black(indices);

        // Forward, keeping the pre-ReLU accumulator.
        let mut acc = self.ft_bias.clone();
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += self.ft[base + h];
            }
        }
        let ow_off = stage * H;
        let mut out = self.out_b[stage];
        for h in 0..H {
            if acc[h] > 0.0 {
                out += self.out_w[ow_off + h] * acc[h];
            }
        }

        let err = out - target;
        let g = lr * err;

        // Read-out gradients, and delta[h] = d out / d acc[h] using the OLD
        // read-out weights (ReLU-gated). Compute delta before mutating out_w.
        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = self.out_w[ow_off + h];
                self.out_w[ow_off + h] -= g * acc[h];
            }
        }
        self.out_b[stage] -= g;

        // Transformer gradients: d acc[h] / d ft_bias[h] = 1, likewise for each
        // active feature's row. Step = lr · err · delta[h].
        for h in 0..H {
            self.ft_bias[h] -= g * delta[h];
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                self.ft[base + h] -= g * delta[h];
            }
        }

        err * err
    }

    /// Raw mutable pointers to the trainable arrays, for lock-free (Hogwild)
    /// parallel SGD. Sound while the workers are the only access to the model
    /// and updates stay sparse (a handful of feature rows + one read-out row).
    pub fn view(&mut self) -> NnueView {
        NnueView {
            ft: self.ft.as_mut_ptr(),
            ft_bias: self.ft_bias.as_mut_ptr(),
            out_w: self.out_w.as_mut_ptr(),
            out_b: self.out_b.as_mut_ptr(),
        }
    }

    /// Hogwild SGD step through a shared `view`; mirrors [`train_black`].
    ///
    /// # Safety
    /// `view` must come from this model and no `&mut self` access may be live.
    pub unsafe fn train_black_shared(
        &self,
        view: &NnueView,
        indices: &PatternIndices,
        stage: usize,
        target: f32,
        lr: f32,
    ) -> f32 {
        let feats = self.features_black(indices);

        let mut acc = [0.0f32; H];
        for h in 0..H {
            acc[h] = *view.ft_bias.add(h);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += *view.ft.add(base + h);
            }
        }
        let ow_off = stage * H;
        let mut out = *view.out_b.add(stage);
        for h in 0..H {
            if acc[h] > 0.0 {
                out += *view.out_w.add(ow_off + h) * acc[h];
            }
        }

        let err = out - target;
        let g = lr * err;

        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = *view.out_w.add(ow_off + h);
                *view.out_w.add(ow_off + h) -= g * acc[h];
            }
        }
        *view.out_b.add(stage) -= g;
        for h in 0..H {
            *view.ft_bias.add(h) -= g * delta[h];
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                *view.ft.add(base + h) -= g * delta[h];
            }
        }
        err * err
    }

    /// Serialize weights to a simple little-endian file.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        let tmp = path.with_extension("tmp");
        {
            let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            w.write_all(b"BBRVNN01")?;
            w.write_all(&(H as u32).to_le_bytes())?;
            w.write_all(&(self.n_features as u32).to_le_bytes())?;
            w.write_all(&(STAGE_COUNT as u32).to_le_bytes())?;
            for &v in self.ft.iter().chain(&self.ft_bias).chain(&self.out_w).chain(&self.out_b) {
                w.write_all(&v.to_le_bytes())?;
            }
            w.flush()?;
        }
        std::fs::rename(&tmp, path)
    }

    /// Load weights previously written by [`save`](Self::save).
    pub fn load(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Read;
        let mut r = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        if &magic != b"BBRVNN01" {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad nnue magic"));
        }
        let mut u = [0u8; 4];
        r.read_exact(&mut u)?;
        if u32::from_le_bytes(u) as usize != H {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "H mismatch"));
        }
        r.read_exact(&mut u)?;
        if u32::from_le_bytes(u) as usize != self.n_features {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "n_features mismatch"));
        }
        r.read_exact(&mut u)?;
        let read_into = |r: &mut dyn Read, dst: &mut [f32]| -> std::io::Result<()> {
            let mut b = [0u8; 4];
            for x in dst.iter_mut() {
                r.read_exact(&mut b)?;
                *x = f32::from_le_bytes(b);
            }
            Ok(())
        };
        read_into(&mut r, &mut self.ft)?;
        read_into(&mut r, &mut self.ft_bias)?;
        read_into(&mut r, &mut self.out_w)?;
        read_into(&mut r, &mut self.out_b)?;
        Ok(())
    }
}
