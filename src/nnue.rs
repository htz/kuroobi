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
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
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
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        for i in 0..H2 {
            acc[i] = acc[i].wrapping_add((*new.add(i)).wrapping_sub(*old.add(i)));
        }
    }
}

/// Sum the `n` masks' transformer rows into `acc` (leaf rebuild).
///
/// The naive loop adds every row into one accumulator, so 64 dependent adds
/// serialise behind each other and the random row loads cannot overlap. Four
/// independent partial accumulators break that chain — integer addition is
/// associative, so the result is bit-identical — and the rows for later masks
/// are prefetched while the current ones are being added.
///
/// # Safety
/// Every `mask_off[m] + raw[m]` must index a valid feature, i.e.
/// `(mask_off[m] + raw[m]) * H + H <= ft_len`.
#[inline]
unsafe fn accumulate_rows(
    acc: &mut [i16; H],
    ft: *const i16,
    mask_off: &[u32],
    raw: &[u16; MAX_MASKS],
    n: usize,
) {
    #[inline(always)]
    unsafe fn row(ft: *const i16, mask_off: &[u32], raw: &[u16; MAX_MASKS], m: usize) -> *const i16 {
        ft.add((*mask_off.get_unchecked(m) as usize + *raw.get_unchecked(m) as usize) * H)
    }

    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    {
        use std::arch::aarch64::*;
        // H=16 i16 = two 128-bit vectors; keep four partial sums of each half.
        let mut p: [[int16x8_t; 2]; 4] = [[vdupq_n_s16(0); 2]; 4];
        const PREFETCH_AHEAD: usize = 8;

        let mut m = 0;
        while m + 4 <= n {
            if m + PREFETCH_AHEAD < n {
                for k in 0..4 {
                    let ptr = row(ft, mask_off, raw, m + PREFETCH_AHEAD + k);
                    std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) ptr, options(nostack, readonly));
                }
            }
            for (k, part) in p.iter_mut().enumerate() {
                let r = row(ft, mask_off, raw, m + k);
                part[0] = vaddq_s16(part[0], vld1q_s16(r));
                if H > 8 {
                    part[1] = vaddq_s16(part[1], vld1q_s16(r.add(8)));
                }
            }
            m += 4;
        }
        // Fold the partials, then the tail masks.
        let lo = vaddq_s16(vaddq_s16(p[0][0], p[1][0]), vaddq_s16(p[2][0], p[3][0]));
        let hi = vaddq_s16(vaddq_s16(p[0][1], p[1][1]), vaddq_s16(p[2][1], p[3][1]));
        let a0 = vaddq_s16(vld1q_s16(acc.as_ptr()), lo);
        vst1q_s16(acc.as_mut_ptr(), a0);
        if H > 8 {
            let a1 = vaddq_s16(vld1q_s16(acc.as_ptr().add(8)), hi);
            vst1q_s16(acc.as_mut_ptr().add(8), a1);
        }
        while m < n {
            let r = row(ft, mask_off, raw, m);
            for h in 0..H {
                *acc.get_unchecked_mut(h) = (*acc.get_unchecked(h)).wrapping_add(*r.add(h));
            }
            m += 1;
        }
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        for m in 0..n {
            let r = row(ft, mask_off, raw, m);
            for h in 0..H {
                *acc.get_unchecked_mut(h) = (*acc.get_unchecked(h)).wrapping_add(*r.add(h));
            }
        }
    }
}

/// `Σ_h relu(acc[h]) · w[h]` (i64) over H int16 lanes, ReLU on the fly.
/// NEON widening multiply on aarch64, scalar elsewhere. Called once per leaf.
#[inline]
fn readout_dot(acc: &[i16], w: &[i16]) -> i64 {
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
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
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
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

/// 変換 i (0..8) を 1 マスに適用する。
fn sym_square(sq: u8, i: u8) -> u8 {
    let mut b = 1u64 << sq;
    if i >= 4 {
        b = crate::bitboard::mirror_horizontal(b);
        for _ in 0..(i - 4) {
            b = crate::bitboard::rotate_90(b);
        }
    } else {
        for _ in 0..i {
            b = crate::bitboard::rotate_90(b);
        }
    }
    b.trailing_zeros() as u8
}

/// パターンの重み表に作用するインデックス置換の集合を求める。
///
/// マスク m に対称変換 s を適用したセル列が、同じパターンの別マスク k と
/// **集合として**一致するとき、両者の並びの差が「桁の置換」になる。
/// 返すのは `perm[j] = j 桁目の移動先` の配列。
fn symmetry_index_perms(p: &Pattern) -> Vec<Vec<usize>> {
    let masks: Vec<&[u8]> = p.masks.to_vec();
    let mut out: Vec<Vec<usize>> = Vec::new();
    for m in &masks {
        for s in 0..8u8 {
            let mapped: Vec<u8> = m.iter().map(|&c| sym_square(c, s)).collect();
            for k in &masks {
                if k.len() != mapped.len() {
                    continue;
                }
                let mut sorted_k: Vec<u8> = k.to_vec();
                sorted_k.sort_unstable();
                let mut sorted_m = mapped.clone();
                sorted_m.sort_unstable();
                if sorted_k != sorted_m {
                    continue;
                }
                // j 桁目 (mask m の j 番目のセル) が mask k の何番目に来るか
                let mut perm = vec![usize::MAX; mapped.len()];
                let mut ok = true;
                for (j, c) in mapped.iter().enumerate() {
                    match k.iter().position(|x| x == c) {
                        Some(pos) => perm[j] = pos,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && perm.iter().any(|&x| x != usize::MAX) && !out.contains(&perm) {
                    out.push(perm);
                }
                break;
            }
        }
    }
    // 恒等置換は意味がないので落とす
    out.retain(|perm| perm.iter().enumerate().any(|(j, &t)| j != t));
    out
}

/// インデックスに桁の置換を適用する。桁 j (最上位が 0) の値が perm[j] 桁目へ移る。
fn apply_index_perm(index: usize, size: usize, perm: &[usize]) -> usize {
    let mut digits = vec![0usize; size];
    let mut x = index;
    for j in (0..size).rev() {
        digits[j] = x % 3;
        x /= 3;
    }
    let mut out_digits = vec![0usize; size];
    for j in 0..size {
        out_digits[perm[j]] = digits[j];
    }
    let mut y = 0usize;
    for j in 0..size {
        y = y * 3 + out_digits[j];
    }
    y
}

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
    /// Split (non-interleaved) copies for the leaf-rebuild path, which reads
    /// only the side to move: `ft_b_i16[feature*H..]` / `ft_w_i16[feature*H..]`.
    /// Halves the bytes touched per mask versus striding the interleaved table.
    ft_b_i16: Vec<i16>,
    ft_w_i16: Vec<i16>,
    ft_bias_i16: [i16; H2],
    out_w_i16: Vec<i16>,
    /// Scale back the i64 read-out accumulation into disc-difference f32:
    /// `1 / (ft_scale * w_scale)`.
    out_scale: f32,

    // Precision-comparison paths (i32 and interleaved f32), built by quantize.
    ftc_i32: Vec<i32>,
    ft_bias_i32: [i32; H2],
    out_w_i32: Vec<i32>,
    out_scale_i32: f32,
    ftc_f32: Vec<f32>,
    ft_bias_f32: [f32; H2],
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
            ft_b_i16: Vec::new(),
            ft_w_i16: Vec::new(),
            ft_bias_i16: [0; H2],
            out_w_i16: vec![0; STAGE_COUNT * H],
            out_scale: 0.0,
            ftc_i32: Vec::new(),
            ft_bias_i32: [0; H2],
            out_w_i32: vec![0; STAGE_COUNT * H],
            out_scale_i32: 0.0,
            ftc_f32: Vec::new(),
            ft_bias_f32: [0.0; H2],
        }
    }

    /// Build the int16 inference copies from the trained f32 weights. Call
    /// after loading/training and before any accumulator use in search.
    ///
    /// A feature entry maps to at most ~256, so the ~64-mask accumulator (plus
    /// bias) stays under ~16.6k — well inside i16.
    ///
    /// The read-out scale is then bounded by the **i32** lanes `readout_dot`
    /// accumulates in on NEON (`vmlal_s16`): `acc_max · w_max · H` must fit in
    /// i32, i.e. `w_max ≤ 2^31 / (16640 · H)`. Filling the full i16 range here
    /// silently overflows those lanes and flips the sign of the score, so keep
    /// the margin — the lost precision is immaterial (see the f32/i16 MSE
    /// comparison: 0.05 disc² at 8x this resolution).
    /// 重みを 8 対称で平均し、評価を対称不変にする。
    ///
    /// パターンのマスクはセル集合としては 8 対称で閉じているが、**並び順が
    /// 変わる**組み合わせがあるため、同一配置でも別インデックスを引いて評価が
    /// わずかにずれる (実測 ~0.1 石)。探索はその差を並べ替え順とカット位置の
    /// 違いに増幅するので、対称局面で答えが変わる。ここでインデックスの軌道
    /// (対称変換で移り合う集合) ごとに重みを平均して根治する。
    ///
    /// 平均なので表現力は落ちず、モデルが本来持つべき対称性を課すだけ。
    /// `quantize` の前に呼ぶこと (量子化表は f32 から作られるため)。
    pub fn symmetrize(&mut self) {
        for (pi, p) in self.patterns.iter().enumerate() {
            let size = p.size;
            let table = 3usize.pow(size as u32);
            let base = self.pattern_offset(pi);
            let perms = symmetry_index_perms(p);
            if perms.is_empty() {
                continue;
            }
            let mut seen = vec![false; table];
            for x in 0..table {
                if seen[x] {
                    continue;
                }
                // x の軌道を集める
                let mut orbit = vec![x];
                seen[x] = true;
                let mut i = 0;
                while i < orbit.len() {
                    let cur = orbit[i];
                    for perm in &perms {
                        let y = apply_index_perm(cur, size, perm);
                        if !seen[y] {
                            seen[y] = true;
                            orbit.push(y);
                        }
                    }
                    i += 1;
                }
                if orbit.len() < 2 {
                    continue;
                }
                // 軌道内の FT 行 (H 次元) を平均
                let inv = 1.0 / orbit.len() as f32;
                for h in 0..H {
                    let mut sum = 0.0f32;
                    for &y in &orbit {
                        sum += self.ft[(base + y) * H + h];
                    }
                    let avg = sum * inv;
                    for &y in &orbit {
                        self.ft[(base + y) * H + h] = avg;
                    }
                }
            }
        }
    }

    /// パターン `pi` の重み表の先頭オフセット (特徴セル単位)。
    fn pattern_offset(&self, pi: usize) -> usize {
        let mut off = 0usize;
        for p in self.patterns.iter().take(pi) {
            off += 3usize.pow(p.size as u32);
        }
        off
    }

    pub fn quantize(&mut self) {
        const ACC_MAX: f32 = 16_640.0; // 64 masks x 256 + bias
        let ft_max = self.ft.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let w_max = self.out_w.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let ft_scale = 256.0 / ft_max;
        let w_limit = (i32::MAX as f32 / (ACC_MAX * H as f32)).min(32_000.0);
        let w_scale = w_limit / w_max;
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

        // Fill the White half of each i16 feature from its digit-swapped index.
        // Orientations of one pattern share a table (rewritten identically).
        for m in 0..self.n_masks {
            let base = self.mask_off[m] as usize;
            let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
            for i in 0..size {
                let src = (base + self.indexer.swapped_index(m, i)) * H2;
                let dst = (base + i) * H2 + H;
                for h in 0..H {
                    self.ftc_i16[dst + h] = self.ftc_i16[src + h];
                }
            }
        }

        // Split copies for the leaf-rebuild path: one stream per perspective,
        // so a leaf touches H (not 2H) bytes per mask.
        self.ft_b_i16 = vec![0; self.n_features * H];
        self.ft_w_i16 = vec![0; self.n_features * H];
        for f in 0..self.n_features {
            let src = f * H2;
            let dst = f * H;
            self.ft_b_i16[dst..dst + H].copy_from_slice(&self.ftc_i16[src..src + H]);
            self.ft_w_i16[dst..dst + H].copy_from_slice(&self.ftc_i16[src + H..src + H2]);
        }
    }

    /// Build the i32/f32 comparison tables (78 MB each). Bench only — the
    /// search uses just the i16 path built by [`quantize`](Self::quantize).
    pub fn build_precision_variants(&mut self) {
        let ft_max = self.ft.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let w_max = self.out_w.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        // i32 path: 8x finer scale than i16 (near-f32 precision).
        let ft_scale32 = 2048.0 / ft_max;
        let w_scale32 = 262144.0 / w_max;
        self.out_scale_i32 = 1.0 / (ft_scale32 * w_scale32);
        self.ftc_i32 = vec![0; self.n_features * H2];
        for f in 0..self.n_features {
            for h in 0..H {
                self.ftc_i32[f * H2 + h] = (self.ft[f * H + h] * ft_scale32).round() as i32;
            }
        }
        // f32 path: interleaved, no quantization (reference precision).
        self.ftc_f32 = vec![0.0; self.n_features * H2];
        for f in 0..self.n_features {
            for h in 0..H {
                self.ftc_f32[f * H2 + h] = self.ft[f * H + h];
            }
        }
        for h in 0..H {
            self.ft_bias_i32[h] = (self.ft_bias[h] * ft_scale32).round() as i32;
            self.ft_bias_i32[H + h] = self.ft_bias_i32[h];
            self.ft_bias_f32[h] = self.ft_bias[h];
            self.ft_bias_f32[H + h] = self.ft_bias[h];
        }
        self.out_w_i32 = self.out_w.iter().map(|&v| (v * w_scale32).round() as i32).collect();

        // Fill the White halves from digit-swapped indices (both variants).
        for m in 0..self.n_masks {
            let base = self.mask_off[m] as usize;
            let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
            for i in 0..size {
                let src = (base + self.indexer.swapped_index(m, i)) * H2;
                let dst = (base + i) * H2 + H;
                for h in 0..H {
                    self.ftc_i32[dst + h] = self.ftc_i32[src + h];
                    self.ftc_f32[dst + h] = self.ftc_f32[src + h];
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

    /// Evaluate from pattern indices the caller already maintains (the search
    /// keeps these incrementally). Recomputes the H accumulator from scratch —
    /// like neural-reversi, which rebuilds its accumulator per eval rather than
    /// threading it through make/unmake — so integrating into an existing
    /// incremental-index search needs only this one call swapped in.
    /// Requires [`quantize`](Self::quantize).
    #[inline]
    pub fn eval_from_indices(&self, indices: &PatternIndices, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let ft = if board.player() == Color::Black {
            &self.ft_b_i16
        } else {
            &self.ft_w_i16
        };
        let mut acc = [0i16; H];
        acc.copy_from_slice(&self.ft_bias_i16[..H]); // halves are equal
        // SAFETY: indices stay inside their pattern's table (the invariant the
        // scalar sum relies on), so every base + H is in bounds.
        unsafe {
            accumulate_rows(&mut acc, ft.as_ptr(), &self.mask_off, indices.raw(), self.n_masks);
        }
        let ow = &self.out_w_i16[stage * H..stage * H + H];
        let sum = readout_dot(&acc, ow);
        self.out_b[stage] + sum as f32 * self.out_scale
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

    /// Maintain the caller's pattern indices across a move — the cheap 2-byte
    /// updates the leaf-rebuild path rides on (see
    /// [`eval_from_indices`](Self::eval_from_indices)).
    #[inline]
    pub fn ix_apply(&self, ix: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        self.indexer.apply(ix, pos, flipped, mover);
    }

    /// Exact inverse of [`ix_apply`](Self::ix_apply).
    #[inline]
    pub fn ix_undo(&self, ix: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        self.indexer.undo(ix, pos, flipped, mover);
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

    /// i32-precision read-out (finer quantization than i16).
    #[inline]
    pub fn eval_acc_i32(&self, acc: &Accumulator32, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black { &acc.acc[0..H] } else { &acc.acc[H..H2] };
        let ow = &self.out_w_i32[stage * H..stage * H + H];
        let mut sum: i64 = 0;
        for h in 0..H {
            sum += v[h].max(0) as i64 * ow[h] as i64;
        }
        self.out_b[stage] + sum as f32 * self.out_scale_i32
    }

    /// f32-precision read-out (reference, no quantization).
    #[inline]
    pub fn eval_acc_f32(&self, acc: &AccumulatorF, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black { &acc.acc[0..H] } else { &acc.acc[H..H2] };
        let ow = &self.out_w[stage * H..stage * H + H];
        let mut sum = 0.0f32;
        for h in 0..H {
            if v[h] > 0.0 {
                sum += v[h] * ow[h];
            }
        }
        self.out_b[stage] + sum
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

/// Generate an incremental accumulator + update methods for a non-wrapping
/// element type (i32 or f32), mirroring the i16 path. Used only to compare
/// precision/speed; the i16 path stays the production one.
macro_rules! quant_acc {
    ($Acc:ident, $T:ty, $build:ident, $apply:ident, $undo:ident, $sq:ident, $ftc:ident, $bias:ident) => {
        #[derive(Clone)]
        pub struct $Acc {
            indices: PatternIndices,
            acc: [$T; H2],
        }
        impl Nnue {
            pub fn $build(&self, board: &Board) -> $Acc {
                let indices = self.indexer.init(board.black, board.white);
                let mut acc = self.$bias;
                for m in 0..self.n_masks {
                    let f = (self.mask_off[m] as usize + indices.raw()[m] as usize) * H2;
                    for i in 0..H2 {
                        acc[i] += self.$ftc[f + i];
                    }
                }
                $Acc { indices, acc }
            }
            pub fn $apply(&self, acc: &mut $Acc, pos: Position, flipped: u64, mover: Color) {
                let md = mover.index() as u16;
                self.$sq(acc, pos.index(), md.wrapping_sub(2));
                let fd = md.wrapping_sub(1 - md);
                let mut f = flipped;
                while f != 0 { let s = f.trailing_zeros() as u8; f &= f - 1; self.$sq(acc, s, fd); }
            }
            pub fn $undo(&self, acc: &mut $Acc, pos: Position, flipped: u64, mover: Color) {
                let md = mover.index() as u16;
                self.$sq(acc, pos.index(), 2u16.wrapping_sub(md));
                let fd = (1 - md).wrapping_sub(md);
                let mut f = flipped;
                while f != 0 { let s = f.trailing_zeros() as u8; f &= f - 1; self.$sq(acc, s, fd); }
            }
            #[inline]
            fn $sq(&self, acc: &mut $Acc, sq: u8, digit_diff: u16) {
                let ftc = &self.$ftc;
                let raw = acc.indices.raw_mut();
                let vec = &mut acc.acc;
                for e in self.indexer.square_entries(sq) {
                    let mask = e.mask as usize;
                    let delta = digit_diff.wrapping_mul(e.pow3);
                    let old = raw[mask] as usize;
                    let new = raw[mask].wrapping_add(delta) as usize;
                    raw[mask] = new as u16;
                    let base = self.mask_off[mask] as usize;
                    let no = (base + new) * H2;
                    let oo = (base + old) * H2;
                    for i in 0..H2 {
                        vec[i] += ftc[no + i] - ftc[oo + i];
                    }
                }
            }
        }
    };
}
quant_acc!(Accumulator32, i32, accumulator_i32, acc_apply_i32, acc_undo_i32, acc_square_i32, ftc_i32, ft_bias_i32);
quant_acc!(AccumulatorF, f32, accumulator_f32, acc_apply_f32, acc_undo_f32, acc_square_f32, ftc_f32, ft_bias_f32);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::EGAROUCID_PATTERNS;

    /// A small deterministic model: every quantized path must agree with the
    /// f32 forward pass (within quantization error) on a played-out sequence,
    /// for both colours to move.
    #[test]
    fn test_eval_paths_agree() {
        let mut nn = Nnue::new(EGAROUCID_PATTERNS);
        nn.init_weights();
        // Give the transformer some structure so the paths can disagree if the
        // layouts (interleaving, digit swap) are wrong.
        let mut s: u64 = 0x1234_5678;
        for v in nn.ft.iter_mut() {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            *v = ((s >> 40) as i32 as f32) / 8.0e6;
        }
        nn.quantize();

        let mut board = Board::new();
        let mut acc = nn.accumulator(&board);
        for _ in 0..12 {
            let scratch = nn.eval(&board);
            let inc = nn.eval_acc(&acc, &board);
            let ix = nn.indices(board.black, board.white);
            let from_ix = nn.eval_from_indices(&ix, &board);

            assert!(
                (inc - scratch).abs() < 1.0,
                "incremental {inc} vs scratch {scratch} (player {:?})",
                board.player()
            );
            assert!(
                (from_ix - inc).abs() < 1e-3,
                "from_indices {from_ix} vs incremental {inc}: both are the same \
                 quantized computation and must match exactly"
            );

            let moves = board.movable();
            if moves == 0 {
                break;
            }
            let pos = Position::from_index(moves.trailing_zeros()).unwrap();
            let mover = board.player();
            let flipped = board.make_move_bits(pos);
            nn.acc_apply(&mut acc, pos, flipped, mover);
        }
    }
}
