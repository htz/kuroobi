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

// Indexed loops here iterate in an order that matters (contiguous scans,
// SIMD-style unrolling), so the iterator lints are not taken. Hot
// functions keep their long argument lists: bundling them into a struct
// would add per-call construction.
#![allow(clippy::needless_range_loop)]

use crate::board::Board;
use crate::color::Color;
use crate::evaluator::STAGE_COUNT;
use crate::pattern::Pattern;
use crate::pattern_index::{PatternIndexer, PatternIndices, MAX_MASKS};
use crate::position::Position;

/// Accumulator width (feature-transformer output dimension). Smaller H means
/// a proportionally cheaper incremental update (the search hot path); the
/// non-linearity survives well below 64.
///
/// A leaf reads the same 64 transformer rows whatever H is -- widening makes
/// each row longer, not more numerous -- so the extra bytes are sequential
/// inside a row the search was already going to touch, and the cost grows
/// slower than the width. Weight files record their own H and are not
/// interchangeable across builds; `widen_h` converts one upwards.
///
/// Selected by cargo feature so a width can be built and measured without
/// editing the source out from under a running trainer:
/// `cargo build --release --features h64 --target-dir target-h64`.
#[cfg(not(any(feature = "h16", feature = "h64", feature = "h128", feature = "h256")))]
pub const H: usize = 32;
#[cfg(feature = "h16")]
pub const H: usize = 16;
#[cfg(feature = "h64")]
pub const H: usize = 64;
#[cfg(feature = "h128")]
pub const H: usize = 128;
#[cfg(feature = "h256")]
pub const H: usize = 256;

/// How many independent copies of the feature transformer the model keeps,
/// one per slice of the game.
///
/// A leaf reads the same 64 rows of the same width whichever copy it lands
/// in -- the phase picks *which* table, not how many rows -- so buckets buy
/// parameters without buying row reads. What they might cost is footprint:
/// more table than fits near the core means more of those reads come from
/// further away.
///
/// **In search they cost nothing measurable.** band29 at depth 13 with four
/// copies replicated from one runs the same 1470455 nodes at 4.90M nodes/s
/// against 4.88M for the single copy. A search spans a narrow band of stages
/// and therefore stays inside one copy for the whole tree, so the footprint
/// it actually keeps hot is unchanged.
///
/// `evalbench --replicas` says otherwise -- 290.7 ns/eval at one copy, 360.1
/// at two, 452.8 at four, 503.8 at six -- and that number is kept here
/// because it is real and because it is wrong for this decision. It cycles
/// copies per position, which destroys exactly the locality search has, so it
/// measures the worst case rather than the case. Believing it would have
/// rejected this whole direction on a 1.56x cost that does not exist. An
/// isolated benchmark that errs pessimistic is the dangerous kind: an
/// optimistic one fails loudly the next time the whole suite is timed, while
/// a pessimistic one becomes a rejection nobody revisits.
///
/// Against this, widening H is the worse buy: H=64 is twice the parameters
/// for 1.8x the evaluation and 1.41x the search, both measured in place.
///
/// Selected by cargo feature; 1 (the default) is byte-for-byte today's model.
#[cfg(not(any(feature = "ftb2", feature = "ftb4", feature = "ftb6")))]
pub const FT_BUCKETS: usize = 1;
#[cfg(feature = "ftb2")]
pub const FT_BUCKETS: usize = 2;
#[cfg(feature = "ftb4")]
pub const FT_BUCKETS: usize = 4;
#[cfg(feature = "ftb6")]
pub const FT_BUCKETS: usize = 6;

/// Which transformer copy a stage reads. Stages are split into equal runs,
/// so the boundaries move with `FT_BUCKETS` and no bucket is ever empty.
#[inline]
pub const fn ft_bucket(stage: usize) -> usize {
    stage * FT_BUCKETS / STAGE_COUNT
}

/// Both perspectives' rows stored/updated together (Black then White), so one
/// contiguous add/sub maintains the whole accumulator per feature change.
const H2: usize = 2 * H;

/// Clamp range for feature-transformer weights during training.
///
/// Inference is int16, so a single outlier eats everyone's resolution:
/// `quantize` scales by `256 / max|ft|` (the i16 accumulator must hold a
/// 64-mask sum). An H=64 run once grew cancelling +/-200 weights in one
/// lane — f32 MSE looked fine while int16 was 13.98 discs off and lost
/// 0-12 head-to-head. Pruning after the fact makes it worse; clamp while
/// training instead. Bound chosen from healthy models (max |ft| 24.7),
/// so 32 passes them untouched and only stops runaways.
const FT_CLAMP: f32 = 32.0;

/// How much of the transformer `quantize` will let saturate int8 before it
/// halves the scale instead.
///
/// The two bounds on the scale pull opposite ways — the int16 accumulator
/// wants it fine, a byte per weight wants it coarse — and the tail is thin
/// enough that clipping wins by an order of magnitude: on the current model
/// the accumulator's scale clips 0.004% of 39M cells for 0.005 discs of
/// held-out error, where the next scale down clips nothing and costs 0.047.
/// A model whose tail outgrows this budget gets the coarser scale.
const FT_CLIP_BUDGET: f32 = 1e-4;

/// Disc-count table width (0..=64), same as the linear evaluator's.
const NUM_TABLE_SIZE: usize = 65;

/// Width of the two hidden layers in the additive head (see the module
/// docs). Kept small: the head only has to model what a single ReLU over
/// the accumulator cannot, and every lane here is dense work on the leaf
/// path.
const MLP_H1: usize = 16;
const MLP_H2: usize = 16;

/// Global gradient-norm clip.
/// Stops a single outlier batch from throwing the model off the manifold;
/// cheap insurance at large batch sizes.
const GRAD_CLIP_NORM: f32 = 1.0;

/// Mobility buckets for the tempo term. Legal-move counts clamp into this
/// range; 24 covers every count that occurs in practice.
const MOB_BUCKETS: usize = 24;

/// Product-gate readout: half the accumulator width, pairing lane `i`
/// with lane `i + H/2`.
const HALF: usize = H / 2;

/// Clamp bound for the product-gate activations, `φ(x) = clamp(x, 0, C)`.
///
/// The readout gains `pw[stage][i] · φ(acc[i]) · φ(acc[i+H/2])` terms —
/// second-order feature interactions the ReLU-linear readout cannot
/// express, at ~8 multiplies per eval. The clamp keeps the quantized
/// product inside i32 and bounds the gradient; unbounded products blow
/// up training the moment two lanes co-fire.
///
/// 16.0 was chosen from the measured activation distribution of the
/// current model (positive activations: p50 4.0 / p90 11.6 / p99 26.2),
/// so ~95% of positive mass passes unclamped while the quantized φ still
/// gets 256 levels at the current feature-transformer scale.
///
/// The term is `pw · φ(a)·φ(b) / PROD_CLAMP`: the division normalises the
/// product feature back to the linear activations' range (≤ PROD_CLAMP
/// instead of ≤ PROD_CLAMP²). Without it the pw gradients are ~16x larger
/// than every other parameter's and warm-start fine-tuning diverges
/// (val 35.5 → 66 in two epochs at lr 5e-4).
const PROD_CLAMP: f32 = 16.0;

/// Steps per disc in the int8 activations the head's first layer reads.
///
/// This sets both the resolution and where the lanes pin: the step is
/// `1 / ACT_UNITS` discs and anything past `255 / ACT_UNITS` saturates. Both
/// matter and they trade directly against each other, because their product
/// is the byte. Measured against solved values on the first epoch's model,
/// where the f32 head scores 5.751 discs, reading the lanes as 0..127:
///
/// | steps/disc | step | pin | MAE |
/// |---:|---:|---:|---:|
/// | 1 | 1.0 | 127 | 9.500 |
/// | 4 | 0.25 | 31.75 | 6.327 |
/// | 8 | 0.125 | 15.9 | 5.868 |
/// | 16 | 0.0625 | 7.94 | 7.092 |
///
/// Resolution dominates until the pin cuts into the distribution (positive
/// lanes: p50 4.0 / p90 11.6 / p99 26.2 discs), and the optimum is where the
/// two costs meet. Reading the lanes as 0..255 instead — which the ReLU
/// makes free, see [`activations_i8`] — doubles the pin at no cost in step,
/// so the meeting point moves and 8 keeps an eighth-disc step with the pin
/// out at 31.9.
const ACT_UNITS: f32 = 16.0;

/// Mover's disc count = index into the disc-count table.
#[inline]
fn num_index(board: &Board) -> usize {
    board.player_bb().count_ones() as usize
}

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
/// Rows are int8 and the accumulator int16: what a leaf costs is the number
/// of cache lines it drags in, not the arithmetic on them, so a byte per
/// weight is worth an extra widening instruction per vector. `vaddw` widens
/// and adds in one op, so the arithmetic is the same count as int16 rows
/// against half the loads.
///
/// # Safety
/// Every `mask_off[m] + raw[m]` must index a valid feature, i.e.
/// `(mask_off[m] + raw[m]) * H + H <= ft_len`.
#[inline]
unsafe fn accumulate_rows(
    acc: &mut [i16; H],
    ft: *const i8,
    mask_off: &[u32],
    raw: &[u16; MAX_MASKS],
    n: usize,
) {
    #[inline(always)]
    unsafe fn row(ft: *const i8, mask_off: &[u32], raw: &[u16; MAX_MASKS], m: usize) -> *const i8 {
        ft.add((*mask_off.get_unchecked(m) as usize + *raw.get_unchecked(m) as usize) * H)
    }

    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    {
        use std::arch::aarch64::*;
        /* One row = H lanes = `VEC` int16 accumulator vectors, loaded as
        `VEC / 2` 128-bit byte vectors. H must not be hard-coded: a fixed
        `[int16x8_t; 2]` once summed only the first 16 lanes of an H=64 net.
        The corruption is silent — the MSE path does not go through here —
        and showed up only head-to-head. Every selectable width is a whole
        number of byte vectors, which is what lets the loop below have no
        lane tail; the assertion is here so adding one that is not fails to
        compile rather than quietly dropping its last lanes. */
        const _: () = assert!(H.is_multiple_of(16), "H must be a multiple of 16");
        const VEC: usize = H.div_ceil(8);
        /* Independent partial accumulators break the dependency chain, but
        they cost registers: `PARTS * VEC` of the 32 the machine has, and the
        loop still needs room for pointers and in-flight loads. Capping the
        set at 16 keeps everything resident — four partials at 32 lanes (what
        this has always used), two at 64. Letting it grow instead spills the
        partials, and then every row load pays for a reload. */
        const PARTS: usize = if 16 / VEC == 0 { 1 } else { 16 / VEC };
        /* 16-byte loads per row. */
        const CHUNKS: usize = H / 16;
        let mut p: [[int16x8_t; VEC]; PARTS] = [[vdupq_n_s16(0); VEC]; PARTS];
        const PREFETCH_AHEAD: usize = 8;

        let mut m = 0;
        while m + PARTS <= n {
            if m + PREFETCH_AHEAD < n {
                for k in 0..PARTS {
                    let ptr = row(ft, mask_off, raw, m + PREFETCH_AHEAD + k) as *const u8;
                    // One prefetch per cache line the row spans.
                    let mut off = 0usize;
                    while off < H {
                        let q = ptr.add(off);
                        std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) q, options(nostack, readonly));
                        off += 64;
                    }
                }
            }
            for (k, part) in p.iter_mut().enumerate() {
                let r = row(ft, mask_off, raw, m + k);
                for c in 0..CHUNKS {
                    let v = vld1q_s8(r.add(c * 16));
                    part[2 * c] = vaddw_s8(part[2 * c], vget_low_s8(v));
                    part[2 * c + 1] = vaddw_high_s8(part[2 * c + 1], v);
                }
            }
            m += PARTS;
        }
        // Fold the partials, then the tail masks.
        for v in 0..VEC {
            let mut s = p[0][v];
            for part in p.iter().skip(1) {
                s = vaddq_s16(s, part[v]);
            }
            let a = vaddq_s16(vld1q_s16(acc.as_ptr().add(v * 8)), s);
            vst1q_s16(acc.as_mut_ptr().add(v * 8), a);
        }
        while m < n {
            let r = row(ft, mask_off, raw, m);
            for h in 0..H {
                *acc.get_unchecked_mut(h) = (*acc.get_unchecked(h)).wrapping_add(*r.add(h) as i16);
            }
            m += 1;
        }
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        for m in 0..n {
            let r = row(ft, mask_off, raw, m);
            for h in 0..H {
                *acc.get_unchecked_mut(h) = (*acc.get_unchecked(h)).wrapping_add(*r.add(h) as i16);
            }
        }
    }
}

/// `Σ_h relu(acc[h] + b[h]) · w[h]` (i64) over H int16 lanes, ReLU on the fly.
///
/// Bias is added here: it differs per stage (which changes every ply),
/// so it cannot be baked into the incrementally-maintained accumulator.
/// NEON widening multiply on aarch64, scalar elsewhere. Called once per leaf.
#[inline]
fn readout_dot(acc: &[i16], b: &[i16], w: &[i16]) -> i64 {
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    unsafe {
        use std::arch::aarch64::*;
        let zero = vdupq_n_s16(0);
        let mut sum = vdupq_n_s32(0);
        let mut h = 0;
        while h + 8 <= H {
            let s = vaddq_s16(vld1q_s16(acc.as_ptr().add(h)), vld1q_s16(b.as_ptr().add(h)));
            let a = vmaxq_s16(s, zero); // ReLU
            let ww = vld1q_s16(w.as_ptr().add(h));
            sum = vmlal_s16(sum, vget_low_s16(a), vget_low_s16(ww));
            sum = vmlal_high_s16(sum, a, ww);
            h += 8;
        }
        let mut acc64 = vaddvq_s32(sum) as i64;
        while h < H {
            acc64 += (*acc.get_unchecked(h))
                .wrapping_add(*b.get_unchecked(h))
                .max(0) as i64
                * *w.get_unchecked(h) as i64;
            h += 1;
        }
        acc64
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        let mut sum: i64 = 0;
        for h in 0..H {
            sum += acc[h].wrapping_add(b[h]).max(0) as i64 * w[h] as i64;
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
    num_w: *mut f32,
    pw: *mut f32,
    mob_w: *mut f32,
}
// SAFETY: the workers only ever add to disjoint-ish sparse cells; racing
// updates cost at most a lost step, never memory unsafety (same argument as
// the linear trainer's `WeightView`).
unsafe impl Send for NnueView {}
unsafe impl Sync for NnueView {}

/// Adam first/second moments. Plain SGD leaves rare cells forever
/// under-trained: the FT is a 610k x H sparse table where one position
/// touches 64 rows, and Adam's second-moment scaling gives rarely-updated
/// cells larger steps (the same reason Adagrad works on sparse
/// embeddings).
///
/// No bias correction: with 1.3B examples the warm-up shrinkage is
/// negligible, and a per-cell step table here would cost 39 MB.
///
/// [`AdamOptimizer`]: crate::evaluator::AdamOptimizer
pub struct AdamState {
    pub beta1: f32,
    pub beta2: f32,
    pub eps: f32,
    /// Decoupled weight decay (AdamW): applied to ft/out_w/pw, not biases.
    pub wd: f32,
    m_ft: Vec<f32>,
    v_ft: Vec<f32>,
    m_ft_bias: Vec<f32>,
    v_ft_bias: Vec<f32>,
    m_out_w: Vec<f32>,
    v_out_w: Vec<f32>,
    m_out_b: Vec<f32>,
    v_out_b: Vec<f32>,
    m_num_w: Vec<f32>,
    v_num_w: Vec<f32>,
    m_pw: Vec<f32>,
    v_pw: Vec<f32>,
    m_mob_w: Vec<f32>,
    v_mob_w: Vec<f32>,
    m_mlp_l1_w: Vec<f32>,
    v_mlp_l1_w: Vec<f32>,
    m_mlp_l1_b: Vec<f32>,
    v_mlp_l1_b: Vec<f32>,
    m_mlp_l2_w: Vec<f32>,
    v_mlp_l2_w: Vec<f32>,
    m_mlp_l2_b: Vec<f32>,
    v_mlp_l2_b: Vec<f32>,
    m_mlp_out_w: Vec<f32>,
    v_mlp_out_w: Vec<f32>,
    /// Scratch for the batched apply: per-row gradient accumulation with a
    /// stamp array instead of sorting (the 1M-pair sort of 132-byte elements
    /// per batch was the serial bottleneck of the whole step).
    grad_scratch: Vec<f32>,
    row_stamp: Vec<u32>,
    stamp_cur: u32,
    /// Rows each bucket touched this batch, kept between the accumulate and
    /// apply passes (gradient clipping needs the coalesced norm before any
    /// weight moves).
    touched: Vec<Vec<u32>>,
}

impl AdamState {
    pub fn new(nn: &Nnue) -> AdamState {
        AdamState {
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            wd: 0.0,
            m_ft: vec![0.0; nn.ft.len()],
            v_ft: vec![0.0; nn.ft.len()],
            m_ft_bias: vec![0.0; STAGE_COUNT * H],
            v_ft_bias: vec![0.0; STAGE_COUNT * H],
            m_out_w: vec![0.0; nn.out_w.len()],
            v_out_w: vec![0.0; nn.out_w.len()],
            m_out_b: vec![0.0; STAGE_COUNT],
            v_out_b: vec![0.0; STAGE_COUNT],
            m_num_w: vec![0.0; STAGE_COUNT * NUM_TABLE_SIZE],
            v_num_w: vec![0.0; STAGE_COUNT * NUM_TABLE_SIZE],
            m_pw: vec![0.0; STAGE_COUNT * HALF],
            v_pw: vec![0.0; STAGE_COUNT * HALF],
            m_mob_w: vec![0.0; STAGE_COUNT * MOB_BUCKETS],
            v_mob_w: vec![0.0; STAGE_COUNT * MOB_BUCKETS],
            m_mlp_l1_w: vec![0.0; MLP_H1 * H],
            v_mlp_l1_w: vec![0.0; MLP_H1 * H],
            m_mlp_l1_b: vec![0.0; MLP_H1],
            v_mlp_l1_b: vec![0.0; MLP_H1],
            m_mlp_l2_w: vec![0.0; MLP_H2 * MLP_H1],
            v_mlp_l2_w: vec![0.0; MLP_H2 * MLP_H1],
            m_mlp_l2_b: vec![0.0; MLP_H2],
            v_mlp_l2_b: vec![0.0; MLP_H2],
            m_mlp_out_w: vec![0.0; STAGE_COUNT * MLP_H2],
            v_mlp_out_w: vec![0.0; STAGE_COUNT * MLP_H2],
            grad_scratch: vec![0.0; nn.ft.len()],
            row_stamp: vec![0; nn.ft.len() / H],
            stamp_cur: 0,
            touched: Vec::new(),
        }
    }

    /// Raw pointers for Hogwild, mirroring [`Nnue::view`].
    pub fn view(&mut self) -> AdamView {
        AdamView {
            m_ft: self.m_ft.as_mut_ptr(),
            v_ft: self.v_ft.as_mut_ptr(),
            m_ft_bias: self.m_ft_bias.as_mut_ptr(),
            v_ft_bias: self.v_ft_bias.as_mut_ptr(),
            m_out_w: self.m_out_w.as_mut_ptr(),
            v_out_w: self.v_out_w.as_mut_ptr(),
            m_out_b: self.m_out_b.as_mut_ptr(),
            v_out_b: self.v_out_b.as_mut_ptr(),
            m_num_w: self.m_num_w.as_mut_ptr(),
            v_num_w: self.v_num_w.as_mut_ptr(),
            m_pw: self.m_pw.as_mut_ptr(),
            v_pw: self.v_pw.as_mut_ptr(),
            m_mob_w: self.m_mob_w.as_mut_ptr(),
            v_mob_w: self.v_mob_w.as_mut_ptr(),
            beta1: self.beta1,
            wd: self.wd,
            beta2: self.beta2,
            eps: self.eps,
        }
    }
}

/// Hogwild view of [`AdamState`].
#[derive(Clone, Copy)]
pub struct AdamView {
    m_ft: *mut f32,
    v_ft: *mut f32,
    m_ft_bias: *mut f32,
    v_ft_bias: *mut f32,
    m_out_w: *mut f32,
    v_out_w: *mut f32,
    m_out_b: *mut f32,
    v_out_b: *mut f32,
    m_num_w: *mut f32,
    v_num_w: *mut f32,
    m_pw: *mut f32,
    v_pw: *mut f32,
    m_mob_w: *mut f32,
    v_mob_w: *mut f32,
    beta1: f32,
    wd: f32,
    beta2: f32,
    eps: f32,
}
// SAFETY: same argument as `NnueView` — races lose a step, never memory safety.
unsafe impl Send for AdamView {}
unsafe impl Sync for AdamView {}

impl AdamView {
    /// One Adam step for the cell at `i`, returning the weight delta to apply.
    #[inline]
    unsafe fn step(&self, m: *mut f32, v: *mut f32, i: usize, grad: f32, lr: f32) -> f32 {
        let mp = m.add(i);
        let vp = v.add(i);
        *mp = self.beta1 * *mp + (1.0 - self.beta1) * grad;
        *vp = self.beta2 * *vp + (1.0 - self.beta2) * grad * grad;
        lr * *mp / ((*vp).sqrt() + self.eps)
    }
}

/// Post-ReLU accumulator lanes packed to int8, `ACT_UNITS` per disc, for the
/// head's first layer.
///
/// The layer is sixteen dot products of H against a matrix, sixteen times the
/// read-out's one, and it is the largest single arithmetic block on the leaf
/// path — 24% of search speed once the head carries weight. int8 on both
/// sides is what unlocks `sdot`, which lands sixteen products per instruction
/// where f32 lands four.
///
/// The lanes are non-negative after the ReLU, so a byte holds 0..255 rather
/// than 0..127 — twice the range for the same step. `sdot` needs signed
/// bytes, so they are stored biased by -128 (the saturating *unsigned*
/// narrow clamps at 255, and flipping the top bit is the subtraction). The
/// bias is a constant per row of the layer, undone there with one multiply
/// against that row's weight sum; see [`Nnue::mlp_l1_rowsum`].
///
/// Anything past `255 / ACT_UNITS` discs pins.
#[inline]
fn activations_i8(acc: &[i16], fb: &[i16], shift: i16) -> [i8; H] {
    let mut a = [0i8; H];
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    // SAFETY: `acc` and `fb` are both at least H long (callers slice them to
    // exactly H), and every load and store below stays under H.
    unsafe {
        use std::arch::aarch64::*;
        // i16 add, as in `readout_dot`: the accumulator invariant keeps the
        // biased sum inside i16, so the narrow add cannot wrap.
        let zero = vdupq_n_s16(0);
        let sh = vdupq_n_s16(-shift);
        let flip = vdupq_n_u8(0x80);
        let relu_shift = |o: usize| {
            let s = vaddq_s16(
                vld1q_s16(acc.as_ptr().add(o)),
                vld1q_s16(fb.as_ptr().add(o)),
            );
            vshlq_s16(vmaxq_s16(s, zero), sh)
        };
        let mut h = 0;
        while h + 16 <= H {
            let lo = vqmovun_s16(relu_shift(h));
            let hi = vqmovun_s16(relu_shift(h + 8));
            let u = vcombine_u8(lo, hi);
            vst1q_s8(
                a.as_mut_ptr().add(h),
                vreinterpretq_s8_u8(veorq_u8(u, flip)),
            );
            h += 16;
        }
        while h < H {
            let v = (*acc.get_unchecked(h) as i32 + *fb.get_unchecked(h) as i32).max(0);
            *a.get_unchecked_mut(h) = ((v >> shift).min(255) - 128) as i8;
            h += 1;
        }
        a
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        for (h, x) in a.iter_mut().enumerate() {
            let v = (acc[h] as i32 + fb[h] as i32).max(0);
            *x = ((v >> shift).min(255) - 128) as i8;
        }
        a
    }
}

/// `Σ row[h] · x[h]` over H int8 lanes, into i32.
///
/// `sdot` multiplies sixteen byte pairs and accumulates them into four i32
/// lanes in one instruction, so a row of H costs H/16 of them. Both sides are
/// bounded by 127 and H is at most 128, so the sum cannot leave i32.
#[inline(always)]
fn dot_i8(row: &[i8], x: &[i8; H]) -> i32 {
    debug_assert!(row.len() >= H);
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    // SAFETY: the assert above bounds every load by H, and `x` is exactly H.
    unsafe {
        use std::arch::aarch64::*;
        let mut s = vdupq_n_s32(0);
        let mut h = 0;
        while h + 16 <= H {
            s = vdotq_s32(
                s,
                vld1q_s8(x.as_ptr().add(h)),
                vld1q_s8(row.as_ptr().add(h)),
            );
            h += 16;
        }
        let mut acc = vaddvq_s32(s);
        while h < H {
            acc += *x.get_unchecked(h) as i32 * *row.get_unchecked(h) as i32;
            h += 1;
        }
        acc
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        let mut acc = 0i32;
        for h in 0..H {
            acc += x[h] as i32 * row[h] as i32;
        }
        acc
    }
}

/// `Σ row[i] · x[i]` over the first `n` lanes of both. NEON on aarch64,
/// scalar elsewhere. Two accumulators, so the multiply latency overlaps.
#[inline(always)]
fn dot_f32(row: &[f32], x: &[f32], n: usize) -> f32 {
    debug_assert!(row.len() >= n && x.len() >= n);
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    // SAFETY: the assert above bounds every load by `n`.
    unsafe {
        use std::arch::aarch64::*;
        let mut s0 = vdupq_n_f32(0.0);
        let mut s1 = vdupq_n_f32(0.0);
        let mut i = 0;
        while i + 8 <= n {
            s0 = vfmaq_f32(
                s0,
                vld1q_f32(row.as_ptr().add(i)),
                vld1q_f32(x.as_ptr().add(i)),
            );
            s1 = vfmaq_f32(
                s1,
                vld1q_f32(row.as_ptr().add(i + 4)),
                vld1q_f32(x.as_ptr().add(i + 4)),
            );
            i += 8;
        }
        while i + 4 <= n {
            s0 = vfmaq_f32(
                s0,
                vld1q_f32(row.as_ptr().add(i)),
                vld1q_f32(x.as_ptr().add(i)),
            );
            i += 4;
        }
        let mut sum = vaddvq_f32(vaddq_f32(s0, s1));
        while i < n {
            sum += *row.get_unchecked(i) * *x.get_unchecked(i);
            i += 1;
        }
        sum
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        let mut sum = 0.0f32;
        for i in 0..n {
            sum += row[i] * x[i];
        }
        sum
    }
}

/// Raw pointers to the feature-transformer cells the parallel apply writes.
#[derive(Clone, Copy)]
struct FtCells {
    scratch: *mut f32,
    stamp: *mut u32,
    m: *mut f32,
    v: *mut f32,
    w: *mut f32,
}
// SAFETY: buckets partition rows, so each thread's cells are disjoint.
unsafe impl Send for FtCells {}
unsafe impl Sync for FtCells {}

/// Thread-local gradient accumulator for synchronous minibatch training.
/// Small tables are dense; feature-transformer rows are collected sparsely
/// as (row, H values) pairs and coalesced at apply time.
pub struct GradSink {
    pub out_w: Vec<f32>,
    pub out_b: Vec<f32>,
    pub num_w: Vec<f32>,
    pub mob_w: Vec<f32>,
    pub mlp_l1_w: Vec<f32>,
    pub mlp_l1_b: Vec<f32>,
    pub mlp_l2_w: Vec<f32>,
    pub mlp_l2_b: Vec<f32>,
    pub mlp_out_w: Vec<f32>,
    pub ft_bias: Vec<f32>,
    pub pw: Vec<f32>,
    /// (feature row, err*delta per lane) bucketed by `row % parts`, so the
    /// apply phase can run one thread per bucket: rows — and therefore the
    /// scratch, moment and weight cells they touch — are disjoint across
    /// buckets by construction. Without this the apply is serial and
    /// dominates (a batch's million row-updates outweigh the threaded
    /// forward pass by ~10x).
    pub ft_rows: Vec<Vec<(u32, [f32; H])>>,
    parts: usize,
}

impl GradSink {
    pub fn new(parts: usize) -> GradSink {
        let parts = parts.max(1);
        GradSink {
            out_w: vec![0.0; STAGE_COUNT * H],
            out_b: vec![0.0; STAGE_COUNT],
            num_w: vec![0.0; STAGE_COUNT * NUM_TABLE_SIZE],
            mob_w: vec![0.0; STAGE_COUNT * MOB_BUCKETS],
            mlp_l1_w: vec![0.0; MLP_H1 * H],
            mlp_l1_b: vec![0.0; MLP_H1],
            mlp_l2_w: vec![0.0; MLP_H2 * MLP_H1],
            mlp_l2_b: vec![0.0; MLP_H2],
            mlp_out_w: vec![0.0; STAGE_COUNT * MLP_H2],
            ft_bias: vec![0.0; STAGE_COUNT * H],
            pw: vec![0.0; STAGE_COUNT * HALF],
            ft_rows: (0..parts).map(|_| Vec::new()).collect(),
            parts,
        }
    }

    #[inline]
    fn push_ft(&mut self, row: u32, vals: &[f32; H]) {
        let p = row as usize % self.parts;
        self.ft_rows[p].push((row, *vals));
    }

    pub fn clear(&mut self) {
        self.out_w.fill(0.0);
        self.out_b.fill(0.0);
        self.num_w.fill(0.0);
        self.mob_w.fill(0.0);
        self.mlp_l1_w.fill(0.0);
        self.mlp_l1_b.fill(0.0);
        self.mlp_l2_w.fill(0.0);
        self.mlp_l2_b.fill(0.0);
        self.mlp_out_w.fill(0.0);
        self.ft_bias.fill(0.0);
        self.pw.fill(0.0);
        for b in self.ft_rows.iter_mut() {
            b.clear();
        }
    }
}

/// Apply symmetry transform i (0..8) to a whole bitboard.
///
/// Used for training augmentation: averaging weights afterwards only
/// projects onto the symmetric subspace, while training on all 8 forms
/// improves the fit while staying symmetric. Zero inference cost.
pub fn sym_board(b: u64, i: u8) -> u64 {
    let mut b = b;
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
    b
}

/// Apply transform i (0..8) to one square.
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

/// Compute the index permutations that act on a pattern's weight table.
///
/// When transform s maps mask m onto another mask k of the same pattern
/// as a set, the difference in cell order becomes a digit permutation;
/// returns arrays of `perm[j] = destination of digit j`.
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
                // Where digit j (cell j of mask m) lands within mask k.
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
    // Drop identity permutations.
    out.retain(|perm| perm.iter().enumerate().any(|(j, &t)| j != t));
    out
}

/// Apply a digit permutation to an index; digit j (0 = most significant) moves to perm[j].
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
    /// Feature rows in one transformer copy; the table holds `FT_BUCKETS`
    /// of these back to back and a row id carries its copy's offset.
    n_feat_bucket: usize,
    /// Total distinct feature cells (sum of 3^size over patterns).
    n_features: usize,

    /// Feature transformer: `ft[feature * H + h]`. Shared across stages.
    ft: Vec<f32>,
    /// Accumulator bias, added once: `[H]`.
    /// Per-stage accumulator bias: `ft_bias[stage * H + h]`.
    ///
    /// Lets the ReLU threshold vary with game phase; the readout was
    /// already per-stage while the nonlinearity threshold was shared.
    /// Not baked into the accumulator — stage changes every ply, so bias
    /// is added at readout, keeping the incremental invariant intact.
    ft_bias: Vec<f32>,
    /// Per-stage read-out weights: `out_w[stage * H + h]`.
    out_w: Vec<f32>,
    /// Per-stage read-out bias: `[STAGE_COUNT]`.
    out_b: Vec<f32>,
    /// Per-disc-count correction: `num_w[stage * NUM_TABLE_SIZE + discs]`.
    ///
    /// Adds global information local patterns cannot express: within a
    /// stage the total disc count is fixed, so the mover's count encodes
    /// the disc difference exactly. The linear evaluator always had this
    /// table; NNUE lacked it. One table lookup, no search-speed cost.
    num_w: Vec<f32>,
    /// Additive head over the accumulator: two hidden layers and a per-stage
    /// read-out. `mlp_out_w` is zero-initialised, so a model that has never
    /// trained the head evaluates exactly as it did before the head existed.
    mlp_l1_w: Vec<f32>,
    mlp_l1_b: Vec<f32>,
    mlp_l2_w: Vec<f32>,
    mlp_l2_b: Vec<f32>,
    mlp_out_w: Vec<f32>,

    /// Per-(stage, mobility) tempo correction: `mob_w[stage * MOB_BUCKETS + n]`.
    ///
    /// Patterns are local, so nothing in the feature set expresses "how many
    /// moves does the side to move have" — a global quantity that carries
    /// most of the tempo advantage. Measured against exact values, the
    /// evaluation underestimated the mover by 2.04 discs on average; feeding
    /// mobility in as its own term brings that to 0.76. One table lookup, no
    /// search-speed cost.
    mob_w: Vec<f32>,
    /// Product-gate readout weights: `pw[stage * HALF + i]` scales
    /// `φ(acc[i]) · φ(acc[i + HALF])` (see [`PROD_CLAMP`]). Zero-initialised,
    /// so a model without these terms evaluates identically — old weight
    /// files load with `pw = 0` and can be fine-tuned from there.
    pw: Vec<f32>,

    // Quantized inference copies (built by `quantize`).
    /// Interleaved transformer: `ftc_i16[feature*H2 ..]` holds the Black row
    /// (H) then the pre-swapped White row (H). One contiguous 2H add/sub then
    /// maintains both perspectives, halving the loads in the hot loop.
    ftc_i16: Vec<i16>,
    /// Split (non-interleaved) copies for the leaf-rebuild path, which reads
    /// only the side to move: `ft_b_i8[feature*H..]` / `ft_w_i8[feature*H..]`.
    /// Halves the bytes touched per mask versus striding the interleaved
    /// table, and a byte per weight halves them again: at H=64 a row is then
    /// one cache line, which is what H=32 already was.
    ft_b_i8: Vec<i8>,
    ft_w_i8: Vec<i8>,
    /// Cells of the transformer that saturated int8 at the chosen scale, and
    /// the total, so a training run can see what its weights cost in the
    /// table the search reads (see [`Nnue::ft_clipped`]).
    ft_clipped: usize,
    /// Whether the optional read-out terms carry any weight at all. Set by
    /// `quantize`; see [`Nnue::extras`] for what they cost when they do.
    has_pw: bool,
    has_head: bool,
    /// The head's first layer in int8, with the right shift that packs the
    /// accumulator into the matching scale and the factor that takes the i32
    /// dot product back to disc units.
    mlp_l1_w_i8: Vec<i8>,
    /// Per-row weight sums, which undo the -128 bias the activations carry.
    mlp_l1_rowsum: Vec<i32>,
    mlp_l1_dequant: f32,
    act_shift: i16,
    ft_bias_i16: Vec<i16>,
    out_w_i16: Vec<i16>,
    pw_i16: Vec<i16>,
    /// Scale back the product-term i64 sum into disc-difference f32:
    /// `1 / (ft_scale² · pw_scale)`.
    prod_scale: f32,
    /// `PROD_CLAMP` in quantized-accumulator units (capped to i16 range).
    prod_clamp_q: i32,
    /// Scale back the i64 read-out accumulation into disc-difference f32:
    /// `1 / (ft_scale * w_scale)`.
    out_scale: f32,

    // Precision-comparison paths (i32 and interleaved f32), built by quantize.
    ftc_i32: Vec<i32>,
    ft_bias_i32: Vec<i32>,
    out_w_i32: Vec<i32>,
    out_scale_i32: f32,
    ftc_f32: Vec<f32>,
    ft_bias_f32: Vec<f32>,
    /// Accumulator scale of the int16 path, so the head can read the
    /// quantized accumulator back in disc units.
    ft_scale: f32,
    /// ft scale of the i32 comparison path (bench only; see `eval_acc_i32`).
    ft_scale32_for_bench: f32,
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
        let n_feat_bucket = off as usize;
        // `n_features` is the whole table, so every allocation and every
        // quantisation loop keyed on it covers all the copies unchanged.
        let n_features = n_feat_bucket * FT_BUCKETS;
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
            n_feat_bucket,
            n_features,
            ft: vec![0.0; n_features * H],
            ft_bias: vec![0.0; STAGE_COUNT * H],
            out_w: vec![0.0; STAGE_COUNT * H],
            out_b: vec![0.0; STAGE_COUNT],
            num_w: vec![0.0; STAGE_COUNT * NUM_TABLE_SIZE],
            mob_w: vec![0.0; STAGE_COUNT * MOB_BUCKETS],
            mlp_l1_w: vec![0.0; MLP_H1 * H],
            mlp_l1_b: vec![0.0; MLP_H1],
            mlp_l2_w: vec![0.0; MLP_H2 * MLP_H1],
            mlp_l2_b: vec![0.0; MLP_H2],
            mlp_out_w: vec![0.0; STAGE_COUNT * MLP_H2],
            pw: vec![0.0; STAGE_COUNT * HALF],
            ftc_i16: Vec::new(),
            ft_b_i8: Vec::new(),
            ft_w_i8: Vec::new(),
            ft_clipped: 0,
            has_pw: false,
            has_head: false,
            mlp_l1_w_i8: Vec::new(),
            mlp_l1_rowsum: vec![0; MLP_H1],
            mlp_l1_dequant: 0.0,
            act_shift: 0,
            ft_bias_i16: vec![0; STAGE_COUNT * H],
            out_w_i16: vec![0; STAGE_COUNT * H],
            pw_i16: vec![0; STAGE_COUNT * HALF],
            prod_scale: 0.0,
            prod_clamp_q: 0,
            out_scale: 0.0,
            ftc_i32: Vec::new(),
            ft_bias_i32: vec![0; STAGE_COUNT * H],
            out_w_i32: vec![0; STAGE_COUNT * H],
            out_scale_i32: 0.0,
            ftc_f32: Vec::new(),
            ft_bias_f32: vec![0.0; STAGE_COUNT * H],
            ft_scale: 1.0,
            ft_scale32_for_bench: 1.0,
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
    /// Average weights across the 8 symmetries, making evaluation
    /// symmetry-invariant.
    ///
    /// Mask cell-sets are closed under symmetry but their cell order is
    /// not, so identical shapes can hit different indices and differ by
    /// ~0.1 discs — which search amplifies into different move ordering
    /// and cut points. Averaging over index orbits fixes it at the root.
    /// Call before `quantize` (the quantized tables derive from f32).
    pub fn symmetrize(&mut self) {
        for (bucket, pi) in
            (0..FT_BUCKETS).flat_map(|b| (0..self.patterns.len()).map(move |i| (b, i)))
        {
            let p = &self.patterns[pi];
            let size = p.size;
            let table = 3usize.pow(size as u32);
            let base = bucket * self.n_feat_bucket + self.pattern_offset(pi);
            let perms = symmetry_index_perms(p);
            if perms.is_empty() {
                continue;
            }
            let mut seen = vec![false; table];
            for x in 0..table {
                if seen[x] {
                    continue;
                }
                // Collect x's orbit.
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
                // Average the FT rows (H dims) within the orbit.
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

    /// Offset of pattern `pi`'s weight table, in feature cells.
    fn pattern_offset(&self, pi: usize) -> usize {
        let mut off = 0usize;
        for p in self.patterns.iter().take(pi) {
            off += 3usize.pow(p.size as u32);
        }
        off
    }

    pub fn quantize(&mut self) {
        // A term whose weights are all zero contributes nothing; see
        // [`Nnue::extras`] for what skipping it is worth.
        self.has_pw = self.pw.iter().any(|&v| v != 0.0);
        self.has_head = self.mlp_out_w.iter().any(|&v| v != 0.0);

        let ft_max = self.ft.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let w_max = self.out_w.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));

        /* Power-of-two scale: fixed point should shift, and the inverse
        multiply then adds no error. The bound comes directly from the
        i16 accumulator (64-mask sum + bias must fit 32767), not from the
        old conservative per-weight-256 assumption — that alone raised
        resolution 54% on the current model without touching training. */
        let bias_max = self.ft_bias.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        let room = 32_000.0 / (self.n_masks as f32 * ft_max + bias_max);
        let mut ft_scale = (2.0f32).powi(room.log2().floor() as i32).max(1.0);

        /* The table is int8, so the same scale also decides how many weights
        saturate. Those two bounds pull opposite ways: the accumulator wants
        the finest scale it can hold, a byte per weight wants the coarsest
        the tail of the distribution needs. Clipping a few extreme cells is
        much the cheaper of the two -- on the current model, scale 16 clips
        0.004% of 39M cells and costs 0.005 discs of held-out error, where
        dropping to scale 8 to clip nothing costs 0.047 -- so keep the
        accumulator's scale and back off only if the tail is fat enough to
        matter. */
        let clip_fraction = |s: f32| {
            let n = self
                .ft
                .iter()
                .filter(|&&v| (v * s).round().abs() > 127.0)
                .count();
            n as f32 / self.ft.len().max(1) as f32
        };
        while ft_scale > 1.0 && clip_fraction(ft_scale) > FT_CLIP_BUDGET {
            ft_scale *= 0.5;
        }
        self.ft_clipped = (clip_fraction(ft_scale) * self.ft.len() as f32) as usize;

        // Saturation caps a cell at 127, so the accumulator's real bound is
        // that, not the unclipped weight's.
        let acc_max = self.n_masks as f32 * (ft_max * ft_scale).min(127.0) + bias_max * ft_scale;
        let w_limit = (i32::MAX as f32 / (acc_max * H as f32)).min(32_000.0);
        let w_scale = w_limit / w_max;
        self.out_scale = 1.0 / (ft_scale * w_scale);
        self.ft_scale = ft_scale;

        let q = |v: f32, s: f32| (v * s).round().clamp(-32768.0, 32767.0) as i16;

        for i in 0..STAGE_COUNT * H {
            self.ft_bias_i16[i] = q(self.ft_bias[i], ft_scale);
        }
        self.out_w_i16 = self.out_w.iter().map(|&v| q(v, w_scale)).collect();

        // Product-gate terms: φ ≤ PROD_CLAMP·ft_scale per factor, so with the
        // cap below a product fits i32 and the HALF-term i64 sum has orders of
        // magnitude of headroom. pw_scale mirrors w_scale's bounding style.
        let pw_max = self.pw.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let pw_scale = (16_000.0 / pw_max).min(32_000.0);
        self.pw_i16 = self.pw.iter().map(|&v| q(v, pw_scale)).collect();
        self.prod_scale = 1.0 / (ft_scale * ft_scale * pw_scale * PROD_CLAMP);
        self.prod_clamp_q = ((PROD_CLAMP * ft_scale) as i32).min(32_767);

        /* The head's first layer, int8 on both sides so `sdot` can run it.
        The activation side is a right shift of the accumulator the read-out
        already holds, so the shift has to leave `ACT_UNITS` steps per disc;
        the weight side takes whatever scale fills int8. */
        self.act_shift = (ft_scale / ACT_UNITS).max(1.0).log2().round() as i16;
        let l1_max = self.mlp_l1_w.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let l1_scale = 127.0 / l1_max;
        self.mlp_l1_w_i8 = self
            .mlp_l1_w
            .iter()
            .map(|&v| (v * l1_scale).round().clamp(-127.0, 127.0) as i8)
            .collect();
        self.mlp_l1_rowsum = (0..MLP_H1)
            .map(|i| {
                self.mlp_l1_w_i8[i * H..i * H + H]
                    .iter()
                    .map(|&w| w as i32)
                    .sum()
            })
            .collect();
        // One activation step is `2^act_shift / ft_scale` discs.
        let act_units = ft_scale / (1 << self.act_shift) as f32;
        self.mlp_l1_dequant = 1.0 / (act_units * l1_scale);

        // One stream per perspective, so a leaf touches H bytes per mask and
        // no stride.
        self.ft_b_i8 = vec![0; self.n_features * H];
        self.ft_w_i8 = vec![0; self.n_features * H];
        for (i, &v) in self.ft.iter().enumerate() {
            self.ft_b_i8[i] = (v * ft_scale).round().clamp(-127.0, 127.0) as i8;
        }
        // The White table is the Black one read through each mask's
        // digit-swapped index. Orientations of one pattern share a table
        // (rewritten identically). Every transformer copy needs this:
        // skipping the outer loop leaves the White rows of every copy but the
        // first at zero, which no Black-to-move measurement can see -- the
        // position sets used to score accuracy are all Black to move, so the
        // model looks perfect while the search scores half its leaves off an
        // empty accumulator.
        for bucket in 0..FT_BUCKETS {
            let bucket_base = bucket * self.n_feat_bucket;
            for m in 0..self.n_masks {
                let base = bucket_base + self.mask_off[m] as usize;
                let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
                for i in 0..size {
                    let src = (base + self.indexer.swapped_index(m, i)) * H;
                    let dst = (base + i) * H;
                    self.ft_w_i8[dst..dst + H].copy_from_slice(&self.ft_b_i8[src..src + H]);
                }
            }
        }
    }

    /// How many transformer cells saturated int8, and how many there are.
    /// A training run that pushes this past a fraction of a percent is
    /// spending accuracy in the table the search reads, not in the one the
    /// loss sees; [`FT_CLIP_BUDGET`] is where `quantize` starts backing the
    /// scale off instead.
    pub fn ft_clipped(&self) -> (usize, usize) {
        (self.ft_clipped, self.ft.len())
    }

    /// Build the interleaved both-perspectives table the incremental
    /// [`Accumulator`] rides on (`n_features * 2H` int16, 157 MB at H=64).
    ///
    /// Not part of [`quantize`](Self::quantize): the search rebuilds every
    /// leaf from `eval_from_indices` and never touches this, so building it
    /// unconditionally doubled the engine's resident tables for nothing.
    /// Only `nnue_bench`, which times the incremental path against the
    /// rebuild, needs it.
    pub fn build_incremental_table(&mut self) {
        // Clipped exactly as the int8 table clips, so the two paths agree
        // cell for cell and `nnue_bench`'s incremental-vs-rebuild check still
        // means what it says.
        let q = |v: f32| (v * self.ft_scale).round().clamp(-127.0, 127.0) as i16;
        self.ftc_i16 = vec![0; self.n_features * H2];
        for f in 0..self.n_features {
            for h in 0..H {
                self.ftc_i16[f * H2 + h] = q(self.ft[f * H + h]);
            }
        }
        for bucket in 0..FT_BUCKETS {
            let bucket_base = bucket * self.n_feat_bucket;
            for m in 0..self.n_masks {
                let base = bucket_base + self.mask_off[m] as usize;
                let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
                for i in 0..size {
                    let src = (base + self.indexer.swapped_index(m, i)) * H2;
                    let dst = (base + i) * H2 + H;
                    for h in 0..H {
                        self.ftc_i16[dst + h] = self.ftc_i16[src + h];
                    }
                }
            }
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
        self.ft_scale32_for_bench = ft_scale32;
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
            for st in 0..STAGE_COUNT {
                let i = st * H + h;
                self.ft_bias_i32[i] = (self.ft_bias[i] * ft_scale32).round() as i32;
                self.ft_bias_f32[i] = self.ft_bias[i];
            }
        }
        self.out_w_i32 = self
            .out_w
            .iter()
            .map(|&v| (v * w_scale32).round() as i32)
            .collect();

        // Fill the White halves from digit-swapped indices (both variants),
        // once per transformer copy.
        for bucket in 0..FT_BUCKETS {
            let bucket_base = bucket * self.n_feat_bucket;
            for m in 0..self.n_masks {
                let base = bucket_base + self.mask_off[m] as usize;
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
    }

    /// Base addresses of the int16 inference tables, for checking that rows
    /// start on cache-line boundaries (a row is `H * 2` bytes and sits at a
    /// multiple of that from the base, so the base's alignment decides
    /// whether every row straddles two lines).
    pub fn table_addrs(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("ft_b_i8", self.ft_b_i8.as_ptr() as usize),
            ("ft_w_i8", self.ft_w_i8.as_ptr() as usize),
            ("ftc_i16", self.ftc_i16.as_ptr() as usize),
        ]
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
        // Positive so ReLU units start active (same value across stages).
        self.ft_bias.fill(0.1);
        self.init_mlp_hidden();
    }

    /// Seed the head's hidden layers. The read-out is left alone (zero means
    /// "no head yet"), so this never changes what the model evaluates — it
    /// only gives the head something to differentiate from.
    fn init_mlp_hidden(&mut self) {
        let mut s: u64 = 0x5DEE_CE66_D5AB_1EE5;
        let mut next = || {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            ((s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as i64 as f32) / (1i64 << 23) as f32
        };
        for v in self.mlp_l1_w.iter_mut() {
            *v = next() * 0.5;
        }
        for v in self.mlp_l2_w.iter_mut() {
            *v = next() * 0.5;
        }
    }

    /// Active features for a Black-to-move position (absolute indices; the
    /// data convention normalizes every training example to Black to move).
    #[inline]
    fn features_black(&self, indices: &PatternIndices, stage: usize) -> [u32; MAX_MASKS] {
        let mut f = [0u32; MAX_MASKS];
        let raw = indices.raw();
        let base = self.bucket_base(stage);
        for m in 0..self.n_masks {
            f[m] = base + self.mask_off[m] + raw[m] as u32;
        }
        f
    }

    /// Row offset of the transformer copy `stage` reads.
    #[inline]
    fn bucket_base(&self, stage: usize) -> u32 {
        (ft_bucket(stage) * self.n_feat_bucket) as u32
    }

    /// Active features from the side-to-move's perspective (search path).
    #[inline]
    fn features_player(
        &self,
        indices: &PatternIndices,
        player: Color,
        stage: usize,
    ) -> [u32; MAX_MASKS] {
        if player == Color::Black {
            return self.features_black(indices, stage);
        }
        let mut f = [0u32; MAX_MASKS];
        let raw = indices.raw();
        let base = self.bucket_base(stage);
        for m in 0..self.n_masks {
            let idx = self.indexer.swapped_index(m, raw[m] as usize) as u32;
            f[m] = base + self.mask_off[m] + idx;
        }
        f
    }

    /// Evaluate from pattern indices the caller already maintains (the search
    /// keeps these incrementally). Recomputes the H accumulator from scratch
    /// rather than threading it through make/unmake — so integrating into an
    /// existing incremental-index search needs only this one call swapped in.
    /// Requires [`quantize`](Self::quantize).
    #[inline]
    pub fn eval_from_indices(&self, indices: &PatternIndices, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let ft = if board.player() == Color::Black {
            &self.ft_b_i8
        } else {
            &self.ft_w_i8
        };
        let mut acc = [0i16; H]; // bias is added at readout
                                 // The stage picks which transformer copy to read; the row offsets
                                 // inside a copy are the same, so it is a base-pointer shift and the
                                 // loop below reads exactly as many rows as with one copy.
        let base = ft_bucket(stage) * self.n_feat_bucket * H;
        // SAFETY: indices stay inside their pattern's table (the invariant the
        // scalar sum relies on), so every base + H is in bounds.
        unsafe {
            accumulate_rows(
                &mut acc,
                ft.as_ptr().add(base),
                &self.mask_off,
                indices.raw(),
                self.n_masks,
            );
        }
        let ow = &self.out_w_i16[stage * H..stage * H + H];
        let fb = &self.ft_bias_i16[stage * H..stage * H + H];
        let sum = readout_dot(&acc, fb, ow);
        self.out_b[stage]
            + sum as f32 * self.out_scale
            + self.num_term(board, stage)
            + self.mob_term(board, stage)
            + self.extras(&acc, fb, stage)
    }

    /// The two optional read-out terms — the product gate and the additive
    /// head — skipped whole when the model does not carry them.
    ///
    /// Both are dense per-leaf work on top of a read-out that is one dot
    /// product, and the head is by far the larger: sixteen dots of H against
    /// the read-out's one. Measured in place at H=64 (band29 depth 13), the
    /// head costs 25% of search speed and the gate 4%. A model whose
    /// corresponding weights are all zero was paying that for a term that
    /// evaluates to zero, which is what every model trained so far has done
    /// with the head. The flags are set once in `quantize`, so the branch is
    /// perfectly predicted and a live term pays only itself.
    #[inline]
    fn extras(&self, acc: &[i16; H], fb: &[i16], stage: usize) -> f32 {
        let mut out = 0.0;
        if self.has_pw {
            out += self.prod_sum_q(acc, fb, stage) as f32 * self.prod_scale;
        }
        if self.has_head {
            out += self.mlp_term_i8(&activations_i8(acc, fb, self.act_shift), stage);
        }
        out
    }

    /// Quantized product-gate sum over `HALF` lane pairs. `acc` carries the
    /// side-to-move accumulator *without* bias (the bias rides in `fb`,
    /// exactly as `readout_dot` consumes it).
    ///
    /// Both factors clamp into `[0, prod_clamp_q]`, which is inside i16, so
    /// the pairwise product is an i32 widening multiply and only the weighted
    /// sum needs i64. The narrow add mirrors `readout_dot` and rests on the
    /// same invariant: the biased accumulator fits i16.
    #[inline]
    fn prod_sum_q(&self, acc: &[i16], fb: &[i16], stage: usize) -> i64 {
        let pq = &self.pw_i16[stage * HALF..stage * HALF + HALF];
        let ci = self.prod_clamp_q;
        #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
        // SAFETY: `acc` and `fb` are at least `H` = `2 * HALF` long and `pq`
        // is exactly `HALF`; every load below stays inside those.
        unsafe {
            use std::arch::aarch64::*;
            let zero = vdupq_n_s16(0);
            let cap = vdupq_n_s16(ci as i16);
            let (mut s0, mut s1) = (vdupq_n_s64(0), vdupq_n_s64(0));
            let mut i = 0;
            let gate = |o: usize| {
                let s = vaddq_s16(
                    vld1q_s16(acc.as_ptr().add(o)),
                    vld1q_s16(fb.as_ptr().add(o)),
                );
                vminq_s16(vmaxq_s16(s, zero), cap)
            };
            while i + 8 <= HALF {
                let a = gate(i);
                let b = gate(i + HALF);
                let w = vld1q_s16(pq.as_ptr().add(i));
                for (p, ww) in [
                    (
                        vmull_s16(vget_low_s16(a), vget_low_s16(b)),
                        vmovl_s16(vget_low_s16(w)),
                    ),
                    (vmull_high_s16(a, b), vmovl_high_s16(w)),
                ] {
                    s0 = vaddq_s64(s0, vmull_s32(vget_low_s32(p), vget_low_s32(ww)));
                    s1 = vaddq_s64(s1, vmull_high_s32(p, ww));
                }
                i += 8;
            }
            let mut ps = vaddvq_s64(vaddq_s64(s0, s1));
            while i < HALF {
                let pa = (*acc.get_unchecked(i) as i32 + *fb.get_unchecked(i) as i32).clamp(0, ci);
                let pb = (*acc.get_unchecked(i + HALF) as i32 + *fb.get_unchecked(i + HALF) as i32)
                    .clamp(0, ci);
                ps += *pq.get_unchecked(i) as i64 * (pa * pb) as i64;
                i += 1;
            }
            ps
        }
        #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
        {
            let mut ps: i64 = 0;
            for i in 0..HALF {
                let pa = (acc[i] as i32 + fb[i] as i32).clamp(0, ci);
                let pb = (acc[i + HALF] as i32 + fb[i + HALF] as i32).clamp(0, ci);
                ps += pq[i] as i64 * (pa * pb) as i64;
            }
            ps
        }
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
        let feats = self.features_player(indices, board.player(), stage);
        self.forward(&feats, stage) + self.num_term(board, stage) + self.mob_term(board, stage)
    }

    /// Disc-count correction; within a stage the mover's count uniquely
    /// determines the disc difference.
    #[inline]
    fn num_term(&self, board: &Board, stage: usize) -> f32 {
        self.num_w[stage * NUM_TABLE_SIZE + num_index(board)]
    }

    /// Run the additive head over already-activated accumulator lanes.
    /// `a[h]` is `relu(acc[h] + bias[h])` in disc units.
    #[inline]
    fn mlp_term(&self, a: &[f32; H], stage: usize) -> f32 {
        let mut x1 = [0.0f32; MLP_H1];
        for (i, x) in x1.iter_mut().enumerate() {
            *x = (self.mlp_l1_b[i] + dot_f32(&self.mlp_l1_w[i * H..], a, H)).max(0.0);
        }
        self.mlp_tail(x1, stage)
    }

    /// The head's first layer over int8 activations, then the same tail.
    ///
    /// Only this layer is quantized. It is `MLP_H1 * H` products against the
    /// tail's `MLP_H2 * MLP_H1 + MLP_H2`, so at H=64 it is 79% of the head's
    /// arithmetic and the rest is not worth the accuracy.
    #[inline]
    fn mlp_term_i8(&self, a: &[i8; H], stage: usize) -> f32 {
        let mut x1 = [0.0f32; MLP_H1];
        for (i, x) in x1.iter_mut().enumerate() {
            // `a` carries the activations biased by -128 (see
            // `activations_i8`); the row's weight sum puts that back.
            let d = dot_i8(&self.mlp_l1_w_i8[i * H..], a) + 128 * self.mlp_l1_rowsum[i];
            *x = (self.mlp_l1_b[i] + d as f32 * self.mlp_l1_dequant).max(0.0);
        }
        self.mlp_tail(x1, stage)
    }

    /// Second layer and read-out, shared by both first-layer paths.
    #[inline]
    fn mlp_tail(&self, x1: [f32; MLP_H1], stage: usize) -> f32 {
        let mut x2 = [0.0f32; MLP_H2];
        for (j, x) in x2.iter_mut().enumerate() {
            *x = (self.mlp_l2_b[j] + dot_f32(&self.mlp_l2_w[j * MLP_H1..], &x1, MLP_H1)).max(0.0);
        }
        dot_f32(&self.mlp_out_w[stage * MLP_H2..], &x2, MLP_H2)
    }

    /// Mobility index of a position (own legal moves, clamped to a bucket).
    #[inline]
    pub fn mob_index(board: &Board) -> usize {
        (board.movable_count() as usize).min(MOB_BUCKETS - 1)
    }

    /// Tempo correction for the side to move (see [`Nnue::mob_w`]).
    #[inline]
    fn mob_term(&self, board: &Board, stage: usize) -> f32 {
        self.mob_w[stage * MOB_BUCKETS + Self::mob_index(board)]
    }

    /// Forward from explicit features + stage (without the disc-count term).
    fn forward(&self, feats: &[u32; MAX_MASKS], stage: usize) -> f32 {
        let mut acc = [0.0f32; H];
        acc.copy_from_slice(&self.ft_bias[stage * H..stage * H + H]);
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
        let pw = &self.pw[stage * HALF..stage * HALF + HALF];
        for i in 0..HALF {
            let pa = acc[i].clamp(0.0, PROD_CLAMP);
            let pb = acc[i + HALF].clamp(0.0, PROD_CLAMP);
            out += pw[i] * pa * pb * (1.0 / PROD_CLAMP);
        }
        let mut a = [0.0f32; H];
        for (h, x) in a.iter_mut().enumerate() {
            *x = acc[h].max(0.0);
        }
        out + self.mlp_term(&a, stage)
    }

    /// Build the pattern indices for a Black-to-move position (training path).
    pub fn indices(&self, black: u64, white: u64) -> PatternIndices {
        self.indexer.init(black, white)
    }

    /// Product features and residual of a Black-to-move training example,
    /// for the closed-form `pw` fit (`fit_pw`): returns
    /// `(stage, z, target - current_output)` where
    /// `z[i] = φ(acc_i)·φ(acc_{i+HALF}) / PROD_CLAMP` and the output uses
    /// the model's current `pw`.
    pub fn product_features_black(
        &self,
        black: u64,
        white: u64,
        target: f32,
    ) -> (usize, [f32; HALF], f32) {
        let ix = self.indexer.init(black, white);
        let board = Board {
            black,
            white,
            player: Color::Black,
            empty_count: 64 - (black | white).count_ones() as u8,
        };
        let stage = crate::evaluator::Evaluator::stage(&board);
        let feats = self.features_black(&ix, stage);
        let mut acc = [0.0f32; H];
        acc.copy_from_slice(&self.ft_bias[stage * H..stage * H + H]);
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += self.ft[base + h];
            }
        }
        let mut z = [0.0f32; HALF];
        for i in 0..HALF {
            let pa = acc[i].clamp(0.0, PROD_CLAMP);
            let pb = acc[i + HALF].clamp(0.0, PROD_CLAMP);
            z[i] = pa * pb * (1.0 / PROD_CLAMP);
        }
        let mut out = self.out_b[stage] + self.num_w[stage * NUM_TABLE_SIZE + num_index(&board)];
        let ow = &self.out_w[stage * H..stage * H + H];
        for h in 0..H {
            if acc[h] > 0.0 {
                out += ow[h] * acc[h];
            }
        }
        let pw = &self.pw[stage * HALF..stage * HALF + HALF];
        for i in 0..HALF {
            out += pw[i] * z[i];
        }
        (stage, z, target - out)
    }

    /// Evaluate a Black-to-move position with and without the product-gate
    /// term (f32 path), for measuring the closed-form fit's gain.
    pub fn eval_black_with_without_pw(&self, black: u64, white: u64) -> (f32, f32) {
        let ix = self.indexer.init(black, white);
        let board = Board {
            black,
            white,
            player: Color::Black,
            empty_count: 64 - (black | white).count_ones() as u8,
        };
        let stage = crate::evaluator::Evaluator::stage(&board);
        let feats = self.features_black(&ix, stage);
        let mut acc = [0.0f32; H];
        acc.copy_from_slice(&self.ft_bias[stage * H..stage * H + H]);
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += self.ft[base + h];
            }
        }
        let mut out = self.out_b[stage] + self.num_w[stage * NUM_TABLE_SIZE + num_index(&board)];
        let ow = &self.out_w[stage * H..stage * H + H];
        for h in 0..H {
            if acc[h] > 0.0 {
                out += ow[h] * acc[h];
            }
        }
        let pw = &self.pw[stage * HALF..stage * HALF + HALF];
        let mut prod = 0.0f32;
        for i in 0..HALF {
            let pa = acc[i].clamp(0.0, PROD_CLAMP);
            let pb = acc[i + HALF].clamp(0.0, PROD_CLAMP);
            prod += pw[i] * pa * pb * (1.0 / PROD_CLAMP);
        }
        (out, out + prod)
    }

    /// Clone the trainable tables (ft, ft_bias, out_w, out_b, num_w), for
    /// external trainers that seed from an engine model.
    #[allow(clippy::type_complexity)]
    pub fn export_f32(&self) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>) {
        (
            self.ft.clone(),
            self.ft_bias.clone(),
            self.out_w.clone(),
            self.out_b.clone(),
            self.num_w.clone(),
        )
    }

    /// Global feature-table rows activated by a Black-to-move position
    /// (`mask_off[m] + index[m]` for each mask), for external trainers that
    /// need the same sparse rows this model's forward pass reads.
    pub fn feature_rows_black(
        &self,
        black: u64,
        white: u64,
        stage: usize,
    ) -> ([u32; MAX_MASKS], usize) {
        let ix = self.indexer.init(black, white);
        (self.features_black(&ix, stage), self.n_masks)
    }

    /// Forward pass + per-example gradient pieces for synchronous minibatch
    /// training: returns the squared error and writes the example's
    /// contributions into `sink` (dense small tables directly, feature rows
    /// as (row, values) pairs). No weights are touched — the caller reduces
    /// sinks across threads and applies one optimizer step per batch, which
    /// is what lets Adam-family optimizers work without Hogwild races.
    #[allow(clippy::too_many_arguments)]
    pub fn grad_black_into(
        &self,
        indices: &PatternIndices,
        stage: usize,
        discs: usize,
        mob: usize,
        target: f32,
        sink: &mut GradSink,
    ) -> f32 {
        let feats = self.features_black(indices, stage);
        let mut acc = [0.0f32; H];
        acc.copy_from_slice(&self.ft_bias[stage * H..stage * H + H]);
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += self.ft[base + h];
            }
        }
        let ow_off = stage * H;
        let pw_off = stage * HALF;
        let num_off = stage * NUM_TABLE_SIZE + discs;
        let mob_off = stage * MOB_BUCKETS + mob.min(MOB_BUCKETS - 1);
        let mut out = self.out_b[stage] + self.num_w[num_off] + self.mob_w[mob_off];
        for h in 0..H {
            if acc[h] > 0.0 {
                out += self.out_w[ow_off + h] * acc[h];
            }
        }
        let mut pa = [0.0f32; HALF];
        let mut pb = [0.0f32; HALF];
        for i in 0..HALF {
            pa[i] = acc[i].clamp(0.0, PROD_CLAMP);
            pb[i] = acc[i + HALF].clamp(0.0, PROD_CLAMP);
            out += self.pw[pw_off + i] * pa[i] * pb[i] * (1.0 / PROD_CLAMP);
        }
        // Additive head. Its output is part of the score, so it has to be
        // inside `out` before the error is taken. Left out, the head chases a
        // residual it has itself already cancelled while the read-out fits
        // the same target beside it, and the two sum to roughly twice the
        // signal -- which is what a live head did to the held-out error
        // (59.13 -> 66.53 in one epoch) before this was found.
        // Pre-activations are kept for the backward pass below.
        let mut a_act = [0.0f32; H];
        for (h, x) in a_act.iter_mut().enumerate() {
            *x = acc[h].max(0.0);
        }
        let mut z1 = [0.0f32; MLP_H1];
        let mut x1 = [0.0f32; MLP_H1];
        for i in 0..MLP_H1 {
            let row = &self.mlp_l1_w[i * H..i * H + H];
            let mut v = self.mlp_l1_b[i];
            for h in 0..H {
                v += row[h] * a_act[h];
            }
            z1[i] = v;
            x1[i] = v.max(0.0);
        }
        let mut z2 = [0.0f32; MLP_H2];
        let mut x2 = [0.0f32; MLP_H2];
        for j in 0..MLP_H2 {
            let row = &self.mlp_l2_w[j * MLP_H1..j * MLP_H1 + MLP_H1];
            let mut v = self.mlp_l2_b[j];
            for i in 0..MLP_H1 {
                v += row[i] * x1[i];
            }
            z2[j] = v;
            x2[j] = v.max(0.0);
        }
        let mlp_off = stage * MLP_H2;
        for j in 0..MLP_H2 {
            out += self.mlp_out_w[mlp_off + j] * x2[j];
        }

        let err = out - target;

        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = self.out_w[ow_off + h];
                sink.out_w[ow_off + h] += err * acc[h];
            }
        }
        for i in 0..HALF {
            let w = self.pw[pw_off + i] * (1.0 / PROD_CLAMP);
            if acc[i] > 0.0 && acc[i] < PROD_CLAMP {
                delta[i] += w * pb[i];
            }
            if acc[i + HALF] > 0.0 && acc[i + HALF] < PROD_CLAMP {
                delta[i + HALF] += w * pa[i];
            }
            sink.pw[pw_off + i] += err * pa[i] * pb[i] * (1.0 / PROD_CLAMP);
        }

        let mut dz2 = [0.0f32; MLP_H2];
        for j in 0..MLP_H2 {
            sink.mlp_out_w[mlp_off + j] += err * x2[j];
            if z2[j] > 0.0 {
                dz2[j] = self.mlp_out_w[mlp_off + j];
            }
        }
        let mut dx1 = [0.0f32; MLP_H1];
        for j in 0..MLP_H2 {
            if dz2[j] == 0.0 {
                continue;
            }
            let row = &self.mlp_l2_w[j * MLP_H1..j * MLP_H1 + MLP_H1];
            sink.mlp_l2_b[j] += err * dz2[j];
            for i in 0..MLP_H1 {
                sink.mlp_l2_w[j * MLP_H1 + i] += err * dz2[j] * x1[i];
                dx1[i] += dz2[j] * row[i];
            }
        }
        for i in 0..MLP_H1 {
            if z1[i] <= 0.0 || dx1[i] == 0.0 {
                continue;
            }
            let dz1 = dx1[i];
            sink.mlp_l1_b[i] += err * dz1;
            let row = &self.mlp_l1_w[i * H..i * H + H];
            for h in 0..H {
                sink.mlp_l1_w[i * H + h] += err * dz1 * a_act[h];
                if acc[h] > 0.0 {
                    delta[h] += dz1 * row[h];
                }
            }
        }

        sink.out_b[stage] += err;
        sink.num_w[num_off] += err;
        sink.mob_w[mob_off] += err;
        for h in 0..H {
            sink.ft_bias[stage * H + h] += err * delta[h];
        }
        for &f in feats.iter().take(self.n_masks) {
            let mut row = [0.0f32; H];
            for h in 0..H {
                row[h] = err * delta[h];
            }
            sink.push_ft(f, &row);
        }
        err * err
    }

    /// One synchronous AdamW step from reduced gradient sinks (gradients are
    /// summed over the batch; pass `scale = 1/batch` to make them means).
    /// Sparse ft rows are sorted and coalesced so each touched row gets one
    /// moment update, exactly like a dense framework would do.
    pub fn apply_adamw_batch(
        &mut self,
        sinks: &mut [GradSink],
        adam: &mut AdamState,
        lr: f32,
        wd: f32,
        scale: f32,
    ) {
        let b1 = adam.beta1;
        let b2 = adam.beta2;
        let eps = adam.eps;
        let step = |m: &mut f32, v: &mut f32, g: f32, w: &mut f32, lr: f32, decay: f32| {
            *m = b1 * *m + (1.0 - b1) * g;
            *v = b2 * *v + (1.0 - b2) * g * g;
            *w -= lr * *m / (v.sqrt() + eps) + decay * lr * *w;
        };

        /* Global gradient-norm clipping.

        Coalesce the sparse rows first (pass 1), measure the norm across
        every table, then scale every gradient by the same factor before any
        weight moves (pass 2). Measuring after a partial apply would clip
        against a norm that no longer matches the step being taken.

        Lookahead is not worth it here: it rewrites every parameter every k
        steps, and sweeping a 20M-cell sparse table costs more than the batch
        that earned the step.
        It pays on a GPU, where the sweep hides under the batch. */
        let parts = sinks.first().map_or(1, |s| s.ft_rows.len());
        adam.stamp_cur = adam.stamp_cur.wrapping_add(1);
        if adam.stamp_cur == 0 {
            adam.row_stamp.fill(0);
            adam.stamp_cur = 1;
        }
        let cur = adam.stamp_cur;
        while adam.touched.len() < parts {
            adam.touched.push(Vec::new());
        }
        let cells = FtCells {
            scratch: adam.grad_scratch.as_mut_ptr(),
            stamp: adam.row_stamp.as_mut_ptr(),
            m: adam.m_ft.as_mut_ptr(),
            v: adam.v_ft.as_mut_ptr(),
            w: self.ft.as_mut_ptr(),
        };
        let mut ft_sq = 0.0f64;
        {
            let sinks_ref = &*sinks;
            let touched = &mut adam.touched[..parts];
            std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for (p, tv) in touched.iter_mut().enumerate() {
                    handles.push(scope.spawn(move || {
                        #[allow(clippy::redundant_locals)]
                        let cells = cells;
                        tv.clear();
                        let mut sq = 0.0f64;
                        // SAFETY: rows in bucket `p` satisfy `row % parts == p`,
                        // so these cells are disjoint from every other thread's,
                        // and the arrays outlive the scope.
                        unsafe {
                            for s in sinks_ref.iter() {
                                for &(row, vals) in s.ft_rows[p].iter() {
                                    let r = row as usize;
                                    let base = r * H;
                                    if *cells.stamp.add(r) != cur {
                                        *cells.stamp.add(r) = cur;
                                        tv.push(row);
                                        std::ptr::copy_nonoverlapping(
                                            vals.as_ptr(),
                                            cells.scratch.add(base),
                                            H,
                                        );
                                    } else {
                                        for h in 0..H {
                                            *cells.scratch.add(base + h) += vals[h];
                                        }
                                    }
                                }
                            }
                            for &row in tv.iter() {
                                let base = row as usize * H;
                                for h in 0..H {
                                    let g = *cells.scratch.add(base + h) * scale;
                                    sq += (g as f64) * (g as f64);
                                }
                            }
                        }
                        sq
                    }));
                }
                for h in handles {
                    ft_sq += h.join().unwrap();
                }
            });
        }
        let mut sq = ft_sq;
        for sel in 0..11usize {
            let len = match sel {
                0 => self.out_w.len(),
                1 => self.out_b.len(),
                2 => self.num_w.len(),
                3 => self.mob_w.len(),
                4 => self.ft_bias.len(),
                5 => self.pw.len(),
                6 => self.mlp_l1_w.len(),
                7 => self.mlp_l1_b.len(),
                8 => self.mlp_l2_w.len(),
                9 => self.mlp_l2_b.len(),
                _ => self.mlp_out_w.len(),
            };
            for i in 0..len {
                let g: f32 = sinks
                    .iter()
                    .map(|s| match sel {
                        0 => s.out_w[i],
                        1 => s.out_b[i],
                        2 => s.num_w[i],
                        3 => s.mob_w[i],
                        4 => s.ft_bias[i],
                        5 => s.pw[i],
                        6 => s.mlp_l1_w[i],
                        7 => s.mlp_l1_b[i],
                        8 => s.mlp_l2_w[i],
                        9 => s.mlp_l2_b[i],
                        _ => s.mlp_out_w[i],
                    })
                    .sum::<f32>()
                    * scale;
                sq += (g as f64) * (g as f64);
            }
        }
        let norm = sq.sqrt() as f32;
        let scale = if norm > GRAD_CLIP_NORM && norm.is_finite() {
            scale * (GRAD_CLIP_NORM / norm)
        } else {
            scale
        };

        // Dense small tables: sum sinks then step every cell that moved.
        for i in 0..self.out_w.len() {
            let g: f32 = sinks.iter().map(|s| s.out_w[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_out_w[i],
                    &mut adam.v_out_w[i],
                    g,
                    &mut self.out_w[i],
                    lr,
                    wd,
                );
            }
        }
        for i in 0..self.out_b.len() {
            let g: f32 = sinks.iter().map(|s| s.out_b[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_out_b[i],
                    &mut adam.v_out_b[i],
                    g,
                    &mut self.out_b[i],
                    lr,
                    0.0,
                );
            }
        }
        for i in 0..self.num_w.len() {
            let g: f32 = sinks.iter().map(|s| s.num_w[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_num_w[i],
                    &mut adam.v_num_w[i],
                    g,
                    &mut self.num_w[i],
                    lr,
                    0.0,
                );
            }
        }
        // The head's tables: weight decay applies to the 2-D ones only, as
        // it does for the transformer and read-out.
        for i in 0..self.mlp_l1_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_l1_w[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_mlp_l1_w[i],
                    &mut adam.v_mlp_l1_w[i],
                    g,
                    &mut self.mlp_l1_w[i],
                    lr,
                    wd,
                );
            }
        }
        for i in 0..self.mlp_l1_b.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_l1_b[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_mlp_l1_b[i],
                    &mut adam.v_mlp_l1_b[i],
                    g,
                    &mut self.mlp_l1_b[i],
                    lr,
                    0.0,
                );
            }
        }
        for i in 0..self.mlp_l2_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_l2_w[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_mlp_l2_w[i],
                    &mut adam.v_mlp_l2_w[i],
                    g,
                    &mut self.mlp_l2_w[i],
                    lr,
                    wd,
                );
            }
        }
        for i in 0..self.mlp_l2_b.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_l2_b[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_mlp_l2_b[i],
                    &mut adam.v_mlp_l2_b[i],
                    g,
                    &mut self.mlp_l2_b[i],
                    lr,
                    0.0,
                );
            }
        }
        for i in 0..self.mlp_out_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_out_w[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_mlp_out_w[i],
                    &mut adam.v_mlp_out_w[i],
                    g,
                    &mut self.mlp_out_w[i],
                    lr,
                    wd,
                );
            }
        }
        for i in 0..self.mob_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mob_w[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_mob_w[i],
                    &mut adam.v_mob_w[i],
                    g,
                    &mut self.mob_w[i],
                    lr,
                    0.0,
                );
            }
        }
        for i in 0..self.ft_bias.len() {
            let g: f32 = sinks.iter().map(|s| s.ft_bias[i]).sum::<f32>() * scale;
            if g != 0.0 {
                let m = &mut adam.m_ft_bias[i];
                let v = &mut adam.v_ft_bias[i];
                *m = b1 * *m + (1.0 - b1) * g;
                *v = b2 * *v + (1.0 - b2) * g * g;
                self.ft_bias[i] =
                    (self.ft_bias[i] - lr * *m / (v.sqrt() + eps)).clamp(-FT_CLAMP, FT_CLAMP);
            }
        }
        for i in 0..self.pw.len() {
            let g: f32 = sinks.iter().map(|s| s.pw[i]).sum::<f32>() * scale;
            if g != 0.0 {
                step(
                    &mut adam.m_pw[i],
                    &mut adam.v_pw[i],
                    g,
                    &mut self.pw[i],
                    lr,
                    wd,
                );
            }
        }

        // Sparse ft rows: apply the gradients coalesced in pass 1, one thread
        // per bucket (buckets partition rows, so no two threads share a cell).
        {
            let touched = &adam.touched[..parts];
            std::thread::scope(|scope| {
                for tv in touched.iter() {
                    scope.spawn(move || {
                        // Capture the whole `FtCells` (which is Send) rather
                        // than its raw-pointer fields: 2021 closures capture
                        // per-field, and a bare `*mut f32` is not Send.
                        #[allow(clippy::redundant_locals)]
                        let cells = cells;
                        // SAFETY: as in pass 1 — disjoint cells per bucket.
                        unsafe {
                            for &row in tv.iter() {
                                let base = row as usize * H;
                                for h in 0..H {
                                    let gh = *cells.scratch.add(base + h) * scale;
                                    if gh != 0.0 {
                                        let m = cells.m.add(base + h);
                                        let v = cells.v.add(base + h);
                                        *m = b1 * *m + (1.0 - b1) * gh;
                                        *v = b2 * *v + (1.0 - b2) * gh * gh;
                                        let w = cells.w.add(base + h);
                                        *w = (*w - lr * *m / ((*v).sqrt() + eps) - wd * lr * *w)
                                            .clamp(-FT_CLAMP, FT_CLAMP);
                                    }
                                }
                            }
                        }
                    });
                }
            });
        }
        for s in sinks.iter_mut() {
            for b in s.ft_rows.iter_mut() {
                b.clear();
            }
        }
    }

    /// Replace every trainable table at once (widening / conversion tools).
    /// `pw` is reset to zero — lane pairing is H-dependent.
    pub fn set_all_weights(
        &mut self,
        ft: &[f32],
        ft_bias: &[f32],
        out_w: &[f32],
        out_b: &[f32],
        num_w: &[f32],
    ) {
        assert_eq!(ft.len(), self.ft.len());
        assert_eq!(ft_bias.len(), self.ft_bias.len());
        assert_eq!(out_w.len(), self.out_w.len());
        assert_eq!(out_b.len(), self.out_b.len());
        assert_eq!(num_w.len(), self.num_w.len());
        self.ft.copy_from_slice(ft);
        self.ft_bias.copy_from_slice(ft_bias);
        self.out_w.copy_from_slice(out_w);
        self.out_b.copy_from_slice(out_b);
        self.num_w.copy_from_slice(num_w);
        self.pw.fill(0.0);
    }

    /// Replace the mobility table wholesale (conversion tools).
    pub fn set_mob_w(&mut self, v: &[f32]) {
        assert_eq!(v.len(), self.mob_w.len(), "mob_w length mismatch");
        self.mob_w.copy_from_slice(v);
    }

    /// Replace the additive head wholesale (conversion tools).
    pub fn set_mlp(&mut self, l1_w: &[f32], l1_b: &[f32], l2_w: &[f32], l2_b: &[f32], ow: &[f32]) {
        assert_eq!(l1_w.len(), self.mlp_l1_w.len(), "mlp l1_w length mismatch");
        assert_eq!(l1_b.len(), self.mlp_l1_b.len(), "mlp l1_b length mismatch");
        assert_eq!(l2_w.len(), self.mlp_l2_w.len(), "mlp l2_w length mismatch");
        assert_eq!(l2_b.len(), self.mlp_l2_b.len(), "mlp l2_b length mismatch");
        assert_eq!(ow.len(), self.mlp_out_w.len(), "mlp out_w length mismatch");
        self.mlp_l1_w.copy_from_slice(l1_w);
        self.mlp_l1_b.copy_from_slice(l1_b);
        self.mlp_l2_w.copy_from_slice(l2_w);
        self.mlp_l2_b.copy_from_slice(l2_b);
        self.mlp_out_w.copy_from_slice(ow);
    }

    /// Replace the product-gate weights wholesale (closed-form fit).
    pub fn set_pw(&mut self, v: &[f32]) {
        assert_eq!(v.len(), self.pw.len(), "pw length mismatch");
        self.pw.copy_from_slice(v);
    }

    /// Pre-ReLU accumulator (acc + per-stage bias) of `board`, for offline
    /// analysis of the activation distribution (clamp-bound selection).
    pub fn acc_pre_relu(&self, board: &Board) -> [f32; H] {
        let indices = self.indexer.init(board.black, board.white);
        let stage = crate::evaluator::Evaluator::stage(board);
        let feats = self.features_player(&indices, board.player(), stage);
        let mut acc = [0.0f32; H];
        acc.copy_from_slice(&self.ft_bias[stage * H..stage * H + H]);
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += self.ft[base + h];
            }
        }
        acc
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
    /// leaf eval is O(H). Needs [`quantize`](Self::quantize) and
    /// [`build_incremental_table`](Self::build_incremental_table).
    pub fn accumulator(&self, board: &Board) -> Accumulator {
        assert!(
            !self.ftc_i16.is_empty(),
            "the incremental accumulator needs build_incremental_table()"
        );
        let indices = self.indexer.init(board.black, board.white);
        // Per-stage bias is added at readout, not here.
        let mut acc = [0i16; H2];
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
        /* The incremental accumulator tracks the first transformer copy only.

        Which copy a position reads is a function of its stage, and a stage
        changes under the very make/unmake this accumulator exists to survive
        -- so once the game crosses a bucket boundary the running sum is over
        the wrong table. Rebuilding from the indices is the correct answer and
        costs what a leaf costs anyway; the search itself goes through
        `eval_from_indices` and never reaches this. With a single copy (the
        default) the branch is constant-false and nothing changes. */
        if FT_BUCKETS > 1 && ft_bucket(stage) != 0 {
            return self.eval_from_indices(&acc.indices, board);
        }
        let mut v = [0i16; H];
        v.copy_from_slice(if board.player() == Color::Black {
            &acc.acc[0..H]
        } else {
            &acc.acc[H..H2]
        });
        let ow = &self.out_w_i16[stage * H..stage * H + H];
        let fb = &self.ft_bias_i16[stage * H..stage * H + H];
        // out = bias + scale * sum relu(acc[h] + fb[h]) * out_w[h] + disc term
        let sum = readout_dot(&v, fb, ow);
        self.out_b[stage]
            + sum as f32 * self.out_scale
            + self.num_term(board, stage)
            + self.mob_term(board, stage)
            + self.extras(&v, fb, stage)
    }

    /// i32-precision read-out (finer quantization than i16).
    #[inline]
    pub fn eval_acc_i32(&self, acc: &Accumulator32, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black {
            &acc.acc[0..H]
        } else {
            &acc.acc[H..H2]
        };
        let ow = &self.out_w_i32[stage * H..stage * H + H];
        let fb = &self.ft_bias_i32[stage * H..stage * H + H];
        let mut sum: i64 = 0;
        for h in 0..H {
            sum += (v[h] + fb[h]).max(0) as i64 * ow[h] as i64;
        }
        // Product terms at this path's precision: dequantize the activations
        // (ft_scale32 is recoverable from the stored scales) and use the f32
        // weights — the point of this path is accumulator precision, not
        // read-out weight precision.
        let ft_scale32 = self.ft_scale32_for_bench;
        let pw = &self.pw[stage * HALF..stage * HALF + HALF];
        let mut psum = 0.0f32;
        for i in 0..HALF {
            let pa = ((v[i] + fb[i]) as f32 / ft_scale32).clamp(0.0, PROD_CLAMP);
            let pb = ((v[i + HALF] + fb[i + HALF]) as f32 / ft_scale32).clamp(0.0, PROD_CLAMP);
            psum += pw[i] * pa * pb * (1.0 / PROD_CLAMP);
        }
        self.out_b[stage]
            + sum as f32 * self.out_scale_i32
            + psum
            + self.num_term(board, stage)
            + self.mob_term(board, stage)
    }

    /// f32-precision read-out (reference, no quantization).
    #[inline]
    pub fn eval_acc_f32(&self, acc: &AccumulatorF, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black {
            &acc.acc[0..H]
        } else {
            &acc.acc[H..H2]
        };
        let ow = &self.out_w[stage * H..stage * H + H];
        let fb = &self.ft_bias_f32[stage * H..stage * H + H];
        let mut sum = 0.0f32;
        for h in 0..H {
            let a = v[h] + fb[h];
            if a > 0.0 {
                sum += a * ow[h];
            }
        }
        let pw = &self.pw[stage * HALF..stage * HALF + HALF];
        for i in 0..HALF {
            let pa = (v[i] + fb[i]).clamp(0.0, PROD_CLAMP);
            let pb = (v[i + HALF] + fb[i + HALF]).clamp(0.0, PROD_CLAMP);
            sum += pw[i] * pa * pb * (1.0 / PROD_CLAMP);
        }
        self.out_b[stage] + sum + self.num_term(board, stage) + self.mob_term(board, stage)
    }

    /// One SGD step on a Black-to-move example at `stage`. Returns squared error.
    ///
    /// Single hidden layer: `out = b[s] + Σ_h w[s][h]·relu(acc[h])`,
    /// `acc[h] = ftb[h] + Σ_f FT[f][h]`. MSE loss; the 2 in `d/dout = 2·err`
    /// is folded into `lr`. Gradients into the transformer use the *old*
    /// read-out weights (compute `delta` before mutating `out_w`).
    ///
    /// **This path does not know about the additive head**, which the
    /// evaluated score does include. Training here therefore descends on a
    /// different function than the search reads, exactly the mismatch
    /// [`Nnue::grad_black_into`] was fixed for, and any head weights present
    /// are left to drift. Use the synchronous minibatch path
    /// (`--adam --minibatch`) for any model that carries a head; this one is
    /// only sound while the head is identically zero.
    pub fn train_black(
        &mut self,
        indices: &PatternIndices,
        stage: usize,
        discs: usize,
        mob: usize,
        target: f32,
        lr: f32,
    ) -> f32 {
        let feats = self.features_black(indices, stage);

        // Forward, keeping the pre-ReLU accumulator.
        let mut acc = [0.0f32; H];
        acc.copy_from_slice(&self.ft_bias[stage * H..stage * H + H]);
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += self.ft[base + h];
            }
        }
        let ow_off = stage * H;
        let pw_off = stage * HALF;
        let num_off = stage * NUM_TABLE_SIZE + discs;
        let mob_off = stage * MOB_BUCKETS + mob.min(MOB_BUCKETS - 1);
        let mut out = self.out_b[stage] + self.num_w[num_off] + self.mob_w[mob_off];
        for h in 0..H {
            if acc[h] > 0.0 {
                out += self.out_w[ow_off + h] * acc[h];
            }
        }
        // Product-gate forward: keep the clamped activations for backward.
        let mut pa = [0.0f32; HALF];
        let mut pb = [0.0f32; HALF];
        for i in 0..HALF {
            pa[i] = acc[i].clamp(0.0, PROD_CLAMP);
            pb[i] = acc[i + HALF].clamp(0.0, PROD_CLAMP);
            out += self.pw[pw_off + i] * pa[i] * pb[i] * (1.0 / PROD_CLAMP);
        }

        let err = out - target;
        let g = lr * err;
        self.num_w[num_off] -= g;
        self.mob_w[mob_off] -= g;

        // Read-out gradients, and delta[h] = d out / d acc[h] using the OLD
        // read-out weights (ReLU-gated). Compute delta before mutating out_w.
        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = self.out_w[ow_off + h];
                self.out_w[ow_off + h] -= g * acc[h];
            }
        }
        // Product gate: chain through both factors with the OLD pw; φ' is 1
        // inside (0, PROD_CLAMP) and 0 outside.
        for i in 0..HALF {
            let w = self.pw[pw_off + i] * (1.0 / PROD_CLAMP);
            if acc[i] > 0.0 && acc[i] < PROD_CLAMP {
                delta[i] += w * pb[i];
            }
            if acc[i + HALF] > 0.0 && acc[i + HALF] < PROD_CLAMP {
                delta[i + HALF] += w * pa[i];
            }
            self.pw[pw_off + i] -= g * pa[i] * pb[i] * (1.0 / PROD_CLAMP);
        }
        self.out_b[stage] -= g;

        // Transformer gradients: d acc[h] / d ft_bias[h] = 1, likewise for each
        // active feature's row. Step = lr · err · delta[h].
        // Only the current stage's bias row moves.
        for h in 0..H {
            let i = stage * H + h;
            self.ft_bias[i] = (self.ft_bias[i] - g * delta[h]).clamp(-FT_CLAMP, FT_CLAMP);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                self.ft[base + h] = (self.ft[base + h] - g * delta[h]).clamp(-FT_CLAMP, FT_CLAMP);
            }
        }

        err * err
    }

    /// All parameters as one flat array (for SWA / weight averaging);
    /// same order as [`save`](Self::save).
    pub fn weights_flat(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(
            self.ft.len()
                + self.ft_bias.len()
                + self.out_w.len()
                + self.out_b.len()
                + self.num_w.len()
                + self.pw.len()
                + self.mob_w.len()
                + self.mlp_l1_w.len()
                + self.mlp_l1_b.len()
                + self.mlp_l2_w.len()
                + self.mlp_l2_b.len()
                + self.mlp_out_w.len(),
        );
        v.extend_from_slice(&self.ft);
        v.extend_from_slice(&self.ft_bias);
        v.extend_from_slice(&self.out_w);
        v.extend_from_slice(&self.out_b);
        v.extend_from_slice(&self.num_w);
        v.extend_from_slice(&self.pw);
        v.extend_from_slice(&self.mob_w);
        v.extend_from_slice(&self.mlp_l1_w);
        v.extend_from_slice(&self.mlp_l1_b);
        v.extend_from_slice(&self.mlp_l2_w);
        v.extend_from_slice(&self.mlp_l2_b);
        v.extend_from_slice(&self.mlp_out_w);
        v
    }

    /// Inverse of [`weights_flat`](Self::weights_flat).
    pub fn set_weights_flat(&mut self, v: &[f32]) {
        let mut o = 0;
        for dst in [
            &mut self.ft,
            &mut self.ft_bias,
            &mut self.out_w,
            &mut self.out_b,
            &mut self.num_w,
            &mut self.pw,
            &mut self.mob_w,
            &mut self.mlp_l1_w,
            &mut self.mlp_l1_b,
            &mut self.mlp_l2_w,
            &mut self.mlp_l2_b,
            &mut self.mlp_out_w,
        ] {
            let n = dst.len();
            dst.copy_from_slice(&v[o..o + n]);
            o += n;
        }
        assert_eq!(o, v.len(), "weights_flat length mismatch");
    }

    /// Replace the disc-count table wholesale, to inject the closed-form
    /// least-squares solution: with the network frozen, the optimum for
    /// `num_w[stage][discs]` is exactly the mean residual of that bucket.
    /// One counting pass beats training and also bounds the achievable
    /// gain.
    pub fn set_num_w(&mut self, v: &[f32]) {
        assert_eq!(v.len(), self.num_w.len(), "num_w length mismatch");
        self.num_w.copy_from_slice(v);
    }

    /// Disc-count table length (STAGE_COUNT x NUM_TABLE_SIZE).
    pub fn num_w_len(&self) -> usize {
        self.num_w.len()
    }

    /// Number of feature-transformer weights (features x H).
    pub fn ft_len(&self) -> usize {
        self.ft.len()
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
            num_w: self.num_w.as_mut_ptr(),
            pw: self.pw.as_mut_ptr(),
            mob_w: self.mob_w.as_mut_ptr(),
        }
    }

    /// Hogwild SGD step through a shared `view`; mirrors [`train_black`],
    /// including its blind spot: the `view` carries no head pointers, so this
    /// path is only sound while the head is identically zero.
    ///
    /// # Safety
    /// `view` must come from this model and no `&mut self` access may be live.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn train_black_shared(
        &self,
        view: &NnueView,
        indices: &PatternIndices,
        stage: usize,
        discs: usize,
        mob: usize,
        target: f32,
        lr: f32,
    ) -> f32 {
        let feats = self.features_black(indices, stage);

        let mut acc = [0.0f32; H];
        for h in 0..H {
            acc[h] = *view.ft_bias.add(stage * H + h);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += *view.ft.add(base + h);
            }
        }
        let ow_off = stage * H;
        let pw_off = stage * HALF;
        let num_off = stage * NUM_TABLE_SIZE + discs;
        let mob_off = stage * MOB_BUCKETS + mob.min(MOB_BUCKETS - 1);
        let mut out = *view.out_b.add(stage) + *view.num_w.add(num_off) + *view.mob_w.add(mob_off);
        for h in 0..H {
            if acc[h] > 0.0 {
                out += *view.out_w.add(ow_off + h) * acc[h];
            }
        }
        let mut pa = [0.0f32; HALF];
        let mut pb = [0.0f32; HALF];
        for i in 0..HALF {
            pa[i] = acc[i].clamp(0.0, PROD_CLAMP);
            pb[i] = acc[i + HALF].clamp(0.0, PROD_CLAMP);
            out += *view.pw.add(pw_off + i) * pa[i] * pb[i] * (1.0 / PROD_CLAMP);
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
        for i in 0..HALF {
            let w = *view.pw.add(pw_off + i) * (1.0 / PROD_CLAMP);
            if acc[i] > 0.0 && acc[i] < PROD_CLAMP {
                delta[i] += w * pb[i];
            }
            if acc[i + HALF] > 0.0 && acc[i + HALF] < PROD_CLAMP {
                delta[i + HALF] += w * pa[i];
            }
            *view.pw.add(pw_off + i) -= g * pa[i] * pb[i] * (1.0 / PROD_CLAMP);
        }
        *view.out_b.add(stage) -= g;
        *view.num_w.add(num_off) -= g;
        *view.mob_w.add(mob_off) -= g;
        for h in 0..H {
            let p = view.ft_bias.add(stage * H + h);
            *p = (*p - g * delta[h]).clamp(-FT_CLAMP, FT_CLAMP);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                let p = view.ft.add(base + h);
                *p = (*p - g * delta[h]).clamp(-FT_CLAMP, FT_CLAMP);
            }
        }
        err * err
    }

    /// Hogwild Adam step; same forward/backward as [`train_black_shared`],
    /// only the weight update differs.
    ///
    /// # Safety
    /// `view` / `adam` must come from this model and its [`AdamState`], and no
    /// `&mut` access to either may be live.
    // The train step takes model, moments, position, label and step all
    // at once; a bundling struct would be constructed 1.3B times.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn train_black_adam_shared(
        &self,
        view: &NnueView,
        adam: &AdamView,
        indices: &PatternIndices,
        stage: usize,
        discs: usize,
        mob: usize,
        target: f32,
        lr: f32,
    ) -> f32 {
        let feats = self.features_black(indices, stage);

        let mut acc = [0.0f32; H];
        for h in 0..H {
            acc[h] = *view.ft_bias.add(stage * H + h);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                acc[h] += *view.ft.add(base + h);
            }
        }
        let ow_off = stage * H;
        let pw_off = stage * HALF;
        let num_off = stage * NUM_TABLE_SIZE + discs;
        let mob_off = stage * MOB_BUCKETS + mob.min(MOB_BUCKETS - 1);
        let mut out = *view.out_b.add(stage) + *view.num_w.add(num_off) + *view.mob_w.add(mob_off);
        for h in 0..H {
            if acc[h] > 0.0 {
                out += *view.out_w.add(ow_off + h) * acc[h];
            }
        }
        let mut pa = [0.0f32; HALF];
        let mut pb = [0.0f32; HALF];
        for i in 0..HALF {
            pa[i] = acc[i].clamp(0.0, PROD_CLAMP);
            pb[i] = acc[i + HALF].clamp(0.0, PROD_CLAMP);
            out += *view.pw.add(pw_off + i) * pa[i] * pb[i] * (1.0 / PROD_CLAMP);
        }

        let err = out - target;

        /* Pass the raw gradient, without lr: Adam normalizes by the
        second moment, and mixing lr in here breaks that normalization
        (the SGD path's `g` is an lr-scaled step — different meaning). */
        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = *view.out_w.add(ow_off + h);
                let p = view.out_w.add(ow_off + h);
                *p -= adam.step(adam.m_out_w, adam.v_out_w, ow_off + h, err * acc[h], lr)
                    + adam.wd * lr * *p;
            }
        }
        for i in 0..HALF {
            let w = *view.pw.add(pw_off + i) * (1.0 / PROD_CLAMP);
            if acc[i] > 0.0 && acc[i] < PROD_CLAMP {
                delta[i] += w * pb[i];
            }
            if acc[i + HALF] > 0.0 && acc[i + HALF] < PROD_CLAMP {
                delta[i + HALF] += w * pa[i];
            }
            let p = view.pw.add(pw_off + i);
            *p -= adam.step(
                adam.m_pw,
                adam.v_pw,
                pw_off + i,
                err * pa[i] * pb[i] * (1.0 / PROD_CLAMP),
                lr,
            ) + adam.wd * lr * *p;
        }
        {
            let p = view.out_b.add(stage);
            *p -= adam.step(adam.m_out_b, adam.v_out_b, stage, err, lr);
            let q = view.num_w.add(num_off);
            *q -= adam.step(adam.m_num_w, adam.v_num_w, num_off, err, lr);
            let mo = view.mob_w.add(mob_off);
            *mo -= adam.step(adam.m_mob_w, adam.v_mob_w, mob_off, err, lr);
        }
        for h in 0..H {
            let i = stage * H + h;
            let p = view.ft_bias.add(i);
            let d = adam.step(adam.m_ft_bias, adam.v_ft_bias, i, err * delta[h], lr);
            *p = (*p - d).clamp(-FT_CLAMP, FT_CLAMP);
        }
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * H;
            for h in 0..H {
                let p = view.ft.add(base + h);
                let d = adam.step(adam.m_ft, adam.v_ft, base + h, err * delta[h], lr);
                *p = (*p - d - adam.wd * lr * *p).clamp(-FT_CLAMP, FT_CLAMP);
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
            /* Format 06 = 05 + the additive head. Older formats still load
            (missing tables zeroed / replicated), so their evaluations are
            unchanged -- and a zero head is exactly "no head". */
            w.write_all(b"BBRVNN06")?;
            w.write_all(&(H as u32).to_le_bytes())?;
            w.write_all(&(self.n_features as u32).to_le_bytes())?;
            w.write_all(&(STAGE_COUNT as u32).to_le_bytes())?;
            for &v in self
                .ft
                .iter()
                .chain(&self.ft_bias)
                .chain(&self.out_w)
                .chain(&self.out_b)
                .chain(&self.num_w)
                .chain(&self.pw)
                .chain(&self.mob_w)
                .chain(&self.mlp_l1_w)
                .chain(&self.mlp_l1_b)
                .chain(&self.mlp_l2_w)
                .chain(&self.mlp_l2_b)
                .chain(&self.mlp_out_w)
            {
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
        /* 01 = shared bias, no disc table; 02 = shared bias with table;
        03 = both per-stage. Legacy formats replicate the bias so their
        evaluations match exactly. */
        let (staged_bias, has_num, has_pw, has_mob, has_mlp) = match &magic {
            b"BBRVNN06" => (true, true, true, true, true),
            b"BBRVNN05" => (true, true, true, true, false),
            b"BBRVNN04" => (true, true, true, false, false),
            b"BBRVNN03" => (true, true, false, false, false),
            b"BBRVNN02" => (false, true, false, false, false),
            b"BBRVNN01" => (false, false, false, false, false),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "bad nnue magic",
                ))
            }
        };
        let mut u = [0u8; 4];
        r.read_exact(&mut u)?;
        if u32::from_le_bytes(u) as usize != H {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "H mismatch",
            ));
        }
        r.read_exact(&mut u)?;
        if u32::from_le_bytes(u) as usize != self.n_features {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "n_features mismatch",
            ));
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
        if staged_bias {
            read_into(&mut r, &mut self.ft_bias)?;
        } else {
            let mut one = vec![0.0f32; H];
            read_into(&mut r, &mut one)?;
            for st in 0..STAGE_COUNT {
                self.ft_bias[st * H..st * H + H].copy_from_slice(&one);
            }
        }
        read_into(&mut r, &mut self.out_w)?;
        read_into(&mut r, &mut self.out_b)?;
        self.num_w.fill(0.0);
        if has_num {
            read_into(&mut r, &mut self.num_w)?;
        }
        self.pw.fill(0.0);
        if has_pw {
            read_into(&mut r, &mut self.pw)?;
        }
        self.mob_w.fill(0.0);
        if has_mob {
            read_into(&mut r, &mut self.mob_w)?;
        }
        self.mlp_out_w.fill(0.0);
        if has_mlp {
            read_into(&mut r, &mut self.mlp_l1_w)?;
            read_into(&mut r, &mut self.mlp_l1_b)?;
            read_into(&mut r, &mut self.mlp_l2_w)?;
            read_into(&mut r, &mut self.mlp_l2_b)?;
            read_into(&mut r, &mut self.mlp_out_w)?;
        }
        /* A zero hidden layer is a dead subnetwork: the read-out's gradient is
        `err * activation`, that activation is zero, so the read-out never
        leaves zero and no gradient ever reaches the layers below. Seeding the
        hidden layers (leaving the read-out at zero) keeps the model's output
        identical to the byte it was saved as, and lets the head start moving.

        The test is on the weights, not on the file's format. Keying it to
        "written before the head existed" looked equivalent and was not: the
        first head run saved its dead head in the *new* format, so every run
        started from it inherited the same dead head and re-measured nothing.
        Two full training runs went that way before the numbers -- identical
        to four decimals across an epoch -- gave it away. */
        if self.mlp_l1_w.iter().all(|&v| v == 0.0) || self.mlp_l2_w.iter().all(|&v| v == 0.0) {
            self.init_mlp_hidden();
        }
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
                // Per-stage bias is added at readout (same as the i16 path).
                let mut acc = [<$T>::default(); H2];
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
                while f != 0 {
                    let s = f.trailing_zeros() as u8;
                    f &= f - 1;
                    self.$sq(acc, s, fd);
                }
            }
            pub fn $undo(&self, acc: &mut $Acc, pos: Position, flipped: u64, mover: Color) {
                let md = mover.index() as u16;
                self.$sq(acc, pos.index(), 2u16.wrapping_sub(md));
                let fd = (1 - md).wrapping_sub(md);
                let mut f = flipped;
                while f != 0 {
                    let s = f.trailing_zeros() as u8;
                    f &= f - 1;
                    self.$sq(acc, s, fd);
                }
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
quant_acc!(
    Accumulator32,
    i32,
    accumulator_i32,
    acc_apply_i32,
    acc_undo_i32,
    acc_square_i32,
    ftc_i32,
    ft_bias_i32
);
quant_acc!(
    AccumulatorF,
    f32,
    accumulator_f32,
    acc_apply_f32,
    acc_undo_f32,
    acc_square_f32,
    ftc_f32,
    ft_bias_f32
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::EGAROUCID_PATTERNS;

    /// The product-gate backward pass must actually descend: repeated steps
    /// on one example drive the error toward the target, through all three
    /// training entry points (plain, shared SGD, shared Adam). A sign error
    /// in the pw/delta chain would diverge or stall instead.
    #[test]
    fn product_gate_training_descends() {
        let make = || {
            let mut nn = Nnue::new(EGAROUCID_PATTERNS);
            nn.init_weights();
            let mut s: u64 = 0x9E37_79B9;
            for v in nn.ft.iter_mut() {
                s ^= s >> 12;
                s ^= s << 25;
                s ^= s >> 27;
                *v = ((s >> 40) as i32 as f32) / 4.0e7;
            }
            for v in nn.pw.iter_mut() {
                s ^= s >> 12;
                s ^= s << 25;
                s ^= s >> 27;
                *v = ((s >> 40) as i32 as f32) / 1.0e8;
            }
            nn
        };
        let board = Board::new();
        let target = 7.5f32;

        // Plain SGD.
        let mut nn = make();
        let ix = nn.indices(board.black, board.white);
        let stage = crate::evaluator::Evaluator::stage(&board);
        let discs = board.player_bb().count_ones() as usize;
        let first = nn.train_black(&ix, stage, discs, 0, target, 0.01);
        let mut last = first;
        for _ in 0..200 {
            last = nn.train_black(&ix, stage, discs, 0, target, 0.01);
        }
        assert!(
            last < first * 0.05,
            "plain SGD failed to descend: first {first} last {last}"
        );

        // Shared SGD and shared Adam.
        let mut nn = make();
        let view = nn.view();
        let first = unsafe { nn.train_black_shared(&view, &ix, stage, discs, 0, target, 0.01) };
        let mut last = first;
        for _ in 0..200 {
            last = unsafe { nn.train_black_shared(&view, &ix, stage, discs, 0, target, 0.01) };
        }
        assert!(
            last < first * 0.05,
            "shared SGD failed to descend: first {first} last {last}"
        );

        let mut nn = make();
        let mut adam = AdamState::new(&nn);
        let view = nn.view();
        let av = adam.view();
        let first =
            unsafe { nn.train_black_adam_shared(&view, &av, &ix, stage, discs, 0, target, 0.01) };
        let mut last = first;
        for _ in 0..400 {
            last = unsafe {
                nn.train_black_adam_shared(&view, &av, &ix, stage, discs, 0, target, 0.01)
            };
        }
        assert!(
            last < first * 0.05,
            "shared Adam failed to descend: first {first} last {last}"
        );
    }

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
        // Non-zero product-gate weights, so the quantized product path is
        // actually exercised (all-zero pw makes the terms vanish).
        for v in nn.pw.iter_mut() {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            *v = ((s >> 40) as i32 as f32) / 2.0e7;
        }
        nn.quantize();
        // The incremental path is one of the paths under test here.
        nn.build_incremental_table();

        /* Play far enough to leave the opening.

        Twelve plies stays inside the first transformer copy and inside the
        first few stages, so a table that is only correct near the start of
        the game passes. That is not hypothetical: the White rows of every
        copy but the first were once left at zero, and this test walked right
        past it because it never reached them. Play the game out, and record
        which copies were actually visited so the coverage cannot quietly
        shrink again. */
        let mut board = Board::new();
        let mut acc = nn.accumulator(&board);
        let mut buckets_seen = [false; FT_BUCKETS];
        let mut plies = 0;
        for _ in 0..60 {
            let scratch = nn.eval(&board);
            let inc = nn.eval_acc(&acc, &board);
            let ix = nn.indices(board.black, board.white);
            let from_ix = nn.eval_from_indices(&ix, &board);
            buckets_seen[ft_bucket(crate::evaluator::Evaluator::stage(&board))] = true;
            plies += 1;

            assert!(
                (inc - scratch).abs() < 1.0,
                "incremental {inc} vs scratch {scratch} (player {:?}, {} empty)",
                board.player(),
                board.empty_count()
            );
            assert!(
                (from_ix - inc).abs() < 1e-3,
                "from_indices {from_ix} vs incremental {inc} ({} empty, player {:?}): \
                 both are the same quantized computation and must match exactly",
                board.empty_count(),
                board.player()
            );

            let moves = board.movable();
            if moves == 0 {
                board.pass();
                if board.movable() == 0 {
                    break;
                }
                continue;
            }
            let pos = Position::from_index(moves.trailing_zeros()).unwrap();
            let mover = board.player();
            let flipped = board.make_move_bits(pos);
            nn.acc_apply(&mut acc, pos, flipped, mover);
        }
        assert!(
            plies > 40,
            "only reached {plies} plies; the late game is untested"
        );
        assert!(
            buckets_seen.iter().all(|&b| b),
            "transformer copies visited: {buckets_seen:?} -- every copy must be \
             exercised or a wrong one goes unnoticed"
        );
    }

    /// The score the trainer differentiates must be the score the search
    /// reads. The two forwards are written separately -- one returns a
    /// value, the other returns gradients -- so a term can be added to one
    /// and forgotten in the other, and nothing downstream complains: the
    /// run still converges, just to the wrong model.
    ///
    /// This is not hypothetical. The additive head sat outside the
    /// training error while being inside the evaluated score, so the head
    /// and the read-out both fitted the whole target and their sum roughly
    /// doubled it. One epoch took the held-out error from 59.13 to 66.53.
    #[test]
    fn training_forward_matches_eval() {
        let mut nn = Nnue::new(EGAROUCID_PATTERNS);
        nn.init_weights();
        // Every part has to be non-zero, or a term missing from one side
        // cannot show up as a difference.
        // Centred on zero: one-sided weights compound through the head and
        // push the score to five figures, where recovering it below loses
        // its low digits to cancellation.
        let mut s: u64 = 0xDEAD_BEEF_1234_5678;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 - 8.388_608e6) / 8.388_608e6
        };
        for v in nn.ft.iter_mut() {
            *v = rnd() * 0.02;
        }
        for v in nn.out_w.iter_mut() {
            *v = rnd() * 0.2;
        }
        for v in nn.pw.iter_mut() {
            *v = rnd() * 0.2;
        }
        for v in nn.num_w.iter_mut() {
            *v = rnd() * 0.2;
        }
        for v in nn.mob_w.iter_mut() {
            *v = rnd() * 0.2;
        }
        for v in nn.mlp_l1_w.iter_mut() {
            *v = rnd() * 0.2;
        }
        for v in nn.mlp_l1_b.iter_mut() {
            *v = rnd() * 0.2;
        }
        for v in nn.mlp_l2_w.iter_mut() {
            *v = rnd() * 0.2;
        }
        for v in nn.mlp_l2_b.iter_mut() {
            *v = rnd() * 0.2;
        }
        for v in nn.mlp_out_w.iter_mut() {
            *v = rnd() * 0.2;
        }

        let board = Board::new();
        let ix = nn.indices(board.black, board.white);
        let stage = crate::evaluator::Evaluator::stage(&board);
        let discs = board.player_bb().count_ones() as usize;
        let mob = 3usize;

        let evaluated = nn.forward(&nn.features_black(&ix, stage), stage)
            + nn.num_w[stage * NUM_TABLE_SIZE + discs]
            + nn.mob_w[stage * MOB_BUCKETS + mob];

        // `grad_black_into` returns (out - target)^2, which hides the sign.
        // Two targets recover `out` itself: sq(0) - sq(2) = 4·out - 4.
        let mut sink = GradSink::new(1);
        let sq0 = nn.grad_black_into(&ix, stage, discs, mob, 0.0, &mut sink);
        let sq2 = nn.grad_black_into(&ix, stage, discs, mob, 2.0, &mut sink);
        let trained = (sq0 - sq2 + 4.0) / 4.0;

        assert!(
            (trained - evaluated).abs() < 1e-3,
            "training forward {trained} vs evaluated {evaluated}: the trainer \
             is descending on a different function than the search reads"
        );
    }
}
