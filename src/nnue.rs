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

/// Lanes the transformer actually sums into, before the read-out sees them.
///
/// With `pairmul` the input layer is twice as wide as the model's working
/// width: it accumulates `2 * H` lanes and folds them to `H` by multiplying
/// lane `i` with lane `i + H`. Second-order interaction enters at the input
/// rather than as a correction bolted onto the read-out, which is where a
/// a wider input layer puts it -- 256 lanes clamped and multiplied
/// pairwise down to 128.
///
/// Everything downstream is unchanged: the read-out, the head and the
/// product gate all still see `H` values. Only the table doubles.
#[cfg(feature = "pairmul")]
pub const ACC_DIMS: usize = 2 * H;
#[cfg(not(feature = "pairmul"))]
pub const ACC_DIMS: usize = H;

/// Both perspectives' rows stored and updated together (Black then White),
/// so one contiguous add/sub maintains the whole accumulator per feature
/// change.
///
/// Twice `ACC_DIMS`
/// rather than twice `H`: under `pairmul` a single perspective is already
/// `2H` wide before the fold, and sizing this on `H` left the White half
/// overlapping the Black one -- the incremental path then disagreed with a
/// rebuild from scratch, which is exactly what `test_eval_paths_agree` saw.
const H2: usize = 2 * ACC_DIMS;

/// Length of the accumulator bias table.
///
/// The shared read-out biases the *folded* lanes and does it per stage, so
/// there are `STAGE_COUNT * H` of them. The stack biases the sparse
/// layer, which is before the fold and has one set for the whole game, so
/// with the stacked read-out there are `ACC_DIMS`.
#[cfg(feature = "stackedout")]
const FT_BIAS_LEN: usize = ACC_DIMS;
#[cfg(not(feature = "stackedout"))]
const FT_BIAS_LEN: usize = STAGE_COUNT * H;

/// Clamp bound for each factor of the pairwise product, in accumulator
/// units before scaling. A quantized form clamps at 510 with an 8-bit-ish
/// shift after; this keeps the same shape, expressed in disc units so it
/// travels with `ft_scale`.
#[cfg(all(feature = "pairmul", not(feature = "stackedout")))]
const PAIR_CLAMP: f32 = 32.0;
/* With the stacked read-out the whole network works on [0,1], as the
reference does: each factor of the product is clamped at 1 and the product
is scaled by 255/256. The 32-disc clamp belongs to the shared read-out,
where the accumulator carries disc units all the way to the score. */
#[cfg(all(feature = "pairmul", feature = "stackedout"))]
const PAIR_CLAMP: f32 = 1.0;

/// What a squared or paired activation is multiplied by.
///
/// This shape uses `255/256` throughout: its quantized form divides by
/// 256 with a shift where the algebra calls for 255, and training carries
/// the same factor so the two agree. Without the stack there is no such
/// division to compensate for, and the factor is one.
#[cfg(feature = "stackedout")]
const ACT_SCALE: f32 = 255.0 / 256.0;
#[cfg_attr(not(feature = "stackedout"), allow(dead_code))]
#[cfg(not(feature = "stackedout"))]
const ACT_SCALE: f32 = 1.0;

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

/// The stacked read-out: widths of its two hidden layers, and the factor
/// that turns its output back into discs.
///
/// Shape: `L1` takes the accumulator plus
/// mobility, its output is paired with its own square to double the width,
/// `L2` widens again, and the final layer sees `L2` *and* the accumulator
/// directly -- a short path from the input layer to the score alongside the
/// deep one. Every layer has its own weights per stage, where the current
/// read-out has a single weight vector per stage and one small head shared
/// across all of them.
///
/// Everything inside runs on `[0, 1]`: the
/// accumulator is divided by its clamp on the way in and the score is
/// multiplied by `SO_SCORE` on the way out. Clamping at 1.0 only means
/// anything if the values reaching it are on that scale.
/// Stacks in the read-out, one per ply.
///
/// Sixty, not `STAGE_COUNT`: a ply runs 0..59, while a stage here runs
/// 0..60 -- the extra one is the finished board, which no
/// training example reaches. Stage 60 shares the last stack.
#[cfg(feature = "stackedout")]
const SO_STAGES: usize = 60;

/// Which stack a stage reads.
#[cfg(feature = "stackedout")]
#[inline]
fn so_stage(stage: usize) -> usize {
    stage.min(SO_STAGES - 1)
}

#[cfg(feature = "stackedout")]
const SO_L1: usize = 16;
#[cfg(feature = "stackedout")]
const SO_L2: usize = 64;
/// Discs per unit of the stacked read-out's output. Training against
/// targets divided by 64 puts the output weights on the same
/// order as the rest of the network; this does the division here instead so
/// the trainer keeps working in discs.
#[cfg(feature = "stackedout")]
const SO_SCORE: f32 = 64.0;

/// Element counts for the stacked read-out's tables, all zero when the
/// feature is off so the vectors cost nothing and every loop over them is
/// empty.
/// The phase-adaptive input: a second sparse layer whose output is
/// concatenated with the base one, with its own set of weights per phase.
///
/// It is not the same thing as `FT_BUCKETS`, which makes phase copies of the
/// base layer and *replaces* it. Here the base stays one shared layer and
/// this is added beside it.
#[cfg(feature = "pa128")]
const PA_DIMS: usize = 128;
#[cfg(not(feature = "pa128"))]
const PA_DIMS: usize = 0;
#[cfg(feature = "pa128")]
const PA_BUCKETS: usize = 6;
#[cfg(not(feature = "pa128"))]
const PA_BUCKETS: usize = 1;

/// Which phase copy of the adaptive input a stage reads.
#[cfg(feature = "pa128")]
#[inline]
fn pa_bucket(stage: usize) -> usize {
    // `ply / (60 / buckets)`, counted on plies -- not
    // `stage * buckets / STAGE_COUNT`, which moves every boundary because
    // there are 61 stages and 60 plies.
    (stage / (SO_STAGES / PA_BUCKETS)).min(PA_BUCKETS - 1)
}

/// Width of what the stack sees directly: the folded accumulator and the
/// phase-adaptive output, side by side. It feeds L1 (with mobility appended)
/// and the output layer (after L2) -- two skip paths.
#[cfg(feature = "stackedout")]
const SO_SKIP: usize = H + PA_DIMS;
#[cfg(feature = "stackedout")]
const SO_L1_IN: usize = SO_SKIP + 1;
#[cfg(feature = "stackedout")]
const SO_OUT_IN: usize = SO_L2 + SO_SKIP;

#[cfg(feature = "stackedout")]
const SO_SIZES: (usize, usize, usize, usize, usize, usize) = (
    SO_STAGES * SO_L1 * SO_L1_IN,
    SO_STAGES * SO_L1,
    SO_STAGES * SO_L2 * (SO_L1 * 2),
    SO_STAGES * SO_L2,
    SO_STAGES * SO_OUT_IN,
    SO_STAGES,
);
#[cfg(not(feature = "stackedout"))]
const SO_SIZES: (usize, usize, usize, usize, usize, usize) = (0, 0, 0, 0, 0, 0);

/// The fold's clamp as an f32, for paths that normalise by it. Equal to
/// `PAIR_CLAMP` when that exists and to the activation clamp otherwise, so
/// the stacked read-out has a sane scale either way.
#[cfg(all(feature = "stackedout", feature = "pairmul"))]
const PAIR_CLAMP_F32: f32 = PAIR_CLAMP;
#[cfg(all(feature = "stackedout", not(feature = "pairmul")))]
const PAIR_CLAMP_F32: f32 = ACT_CLAMP;

/// Mobility on `[0, 1]`, the scale the stacked read-out works in. The
/// reference multiplies the raw count by 7/255 and caps it at one.
#[cfg(feature = "stackedout")]
#[inline]
fn mob_unit(mob: usize) -> f32 {
    (mob as f32 * (7.0 / 255.0)).min(1.0)
}

/// One moment vector per table of the stacked read-out, in the order
/// `apply_adamw_batch` walks them.
fn so_moment_shapes() -> Vec<Vec<f32>> {
    vec![
        vec![0.0; SO_SIZES.0],
        vec![0.0; SO_SIZES.1],
        vec![0.0; SO_SIZES.2],
        vec![0.0; SO_SIZES.3],
        vec![0.0; SO_SIZES.4],
        vec![0.0; SO_SIZES.5],
    ]
}

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

/// Clamp bound for the read-out's squared activation, in discs.
///
/// The read-out applies `φ(x) = clamp(x, 0, C)² / C` rather than
/// `max(0, x)`, which puts a second-order term in the model for free: the
/// row reads are unchanged and only the arithmetic on lanes already in
/// registers differs. That is the same interaction the product gate buys
/// with `pw`, except the gate can only pair lane `i` with lane `i + H/2`
/// at ~8 multiplies, while this squares every lane.
///
/// Dividing by `C` keeps the activation in the range the linear one
/// occupied (≤ C instead of ≤ C²). Without it the read-out weights would
/// need a scale of their own and every learning rate tuned against the
/// linear model would be wrong by a factor of C.
///
/// `C = 16` matches [`PROD_CLAMP`] and is chosen the same way: from the
/// measured activation spread (positive lanes: p50 4.0 / p90 11.6 / p99
/// 26.2). Saturation above it is deliberate -- a squared activation with
/// no ceiling lets one loud lane dominate the sum.
const ACT_CLAMP: f32 = 16.0;

/// The side to move's legal-move count as the head reads it.
///
/// Scaled to roughly the range the accumulator's activations occupy, so
/// one input does not arrive an order of magnitude louder than the H it
/// sits beside and swamp the first layer before training can balance it.
/// The count itself is already clamped to [`MOB_BUCKETS`] by
/// [`Nnue::mob_index`].
#[inline]
/* The stacked read-out uses neither: it clamps inline against its own
[0,1] range and scales mobility by 7/255 rather than 0.5. Kept for the
shared read-out, which is still the shipped shape. */
#[cfg_attr(feature = "stackedout", allow(dead_code))]
fn mob_input(mob: usize) -> f32 {
    mob as f32 * 0.5
}

/// `clamp(x, 0, C)² / C` -- the read-out's activation, in disc units.
#[inline]
#[cfg_attr(feature = "stackedout", allow(dead_code))]
fn screlu(x: f32) -> f32 {
    let c = x.clamp(0.0, ACT_CLAMP);
    c * c * (1.0 / ACT_CLAMP)
}

/// `dφ/dx` for [`screlu`]. Zero outside the clamp, where the activation is
/// flat and no gradient should flow.
#[inline]
#[cfg_attr(feature = "stackedout", allow(dead_code))]
fn screlu_grad(x: f32) -> f32 {
    if x > 0.0 && x < ACT_CLAMP {
        2.0 * x * (1.0 / ACT_CLAMP)
    } else {
        0.0
    }
}

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
pub const ACT_UNITS: f32 = 16.0;

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
/// `(mask_off[m] + raw[m]) * ACC_DIMS + ACC_DIMS <= ft_len`.
#[inline]
unsafe fn accumulate_rows(
    acc: &mut [i16; ACC_DIMS],
    ft: *const i8,
    mask_off: &[u32],
    raw: &[u16; MAX_MASKS],
    n: usize,
) {
    #[inline(always)]
    unsafe fn row(ft: *const i8, mask_off: &[u32], raw: &[u16; MAX_MASKS], m: usize) -> *const i8 {
        ft.add((*mask_off.get_unchecked(m) as usize + *raw.get_unchecked(m) as usize) * ACC_DIMS)
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
        const _: () = assert!(
            ACC_DIMS.is_multiple_of(16),
            "ACC_DIMS must be a multiple of 16"
        );
        const VEC: usize = ACC_DIMS.div_ceil(8);
        /* Independent partial accumulators break the dependency chain, but
        they cost registers: `PARTS * VEC` of the 32 the machine has, and the
        loop still needs room for pointers and in-flight loads. Capping the
        set at 16 keeps everything resident — four partials at 32 lanes (what
        this has always used), two at 64. Letting it grow instead spills the
        partials, and then every row load pays for a reload. */
        const PARTS: usize = if 16 / VEC == 0 { 1 } else { 16 / VEC };
        /* 16-byte loads per row. */
        const CHUNKS: usize = ACC_DIMS / 16;
        let mut p: [[int16x8_t; VEC]; PARTS] = [[vdupq_n_s16(0); VEC]; PARTS];
        const PREFETCH_AHEAD: usize = 8;

        let mut m = 0;
        while m + PARTS <= n {
            if m + PREFETCH_AHEAD < n {
                for k in 0..PARTS {
                    let ptr = row(ft, mask_off, raw, m + PREFETCH_AHEAD + k) as *const u8;
                    // One prefetch per cache line the row spans.
                    let mut off = 0usize;
                    while off < ACC_DIMS {
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
            for h in 0..ACC_DIMS {
                *acc.get_unchecked_mut(h) = (*acc.get_unchecked(h)).wrapping_add(*r.add(h) as i16);
            }
            m += 1;
        }
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        for m in 0..n {
            let r = row(ft, mask_off, raw, m);
            for h in 0..ACC_DIMS {
                *acc.get_unchecked_mut(h) = (*acc.get_unchecked(h)).wrapping_add(*r.add(h) as i16);
            }
        }
    }
}

/// Fold the accumulator to `H` lanes.
///
/// Without `pairmul` there is nothing to fold. With it, lane `i` and lane
/// `i + H` are clamped and multiplied, which is this
/// input layer: interaction between two learned lanes before the read-out
/// ever sees them, rather than a correction added afterwards.
///
/// The clamp bounds each factor so the product stays inside i16 after the
/// shift, and bounds the gradient -- an unbounded product blows up the
/// moment two lanes co-fire, which is the same reason the product gate
/// clamps.
#[inline]
#[cfg_attr(feature = "stackedout", allow(dead_code))]
fn fold_pairs(acc: &[i16; ACC_DIMS], cap: i16, shift: i16) -> [i16; H] {
    #[cfg(not(feature = "pairmul"))]
    {
        let _ = (cap, shift);
        *acc
    }
    #[cfg(feature = "pairmul")]
    {
        let mut out = [0i16; H];
        for i in 0..H {
            let a = acc[i].clamp(0, cap) as i32;
            let b = acc[i + H].clamp(0, cap) as i32;
            out[i] = ((a * b) >> shift) as i16;
        }
        out
    }
}

/// `Σ_h φ(acc[h] + b[h]) · w[h]` (i64) over H int16 lanes, where `φ` is the
/// squared clipped activation of [`screlu`], applied on the fly.
///
/// Bias is added here: it differs per stage (which changes every ply),
/// so it cannot be baked into the incrementally-maintained accumulator.
///
/// The quantized form of `clamp(x,0,C)²/C` is a multiply and a shift.
/// Clamping at `cap = C · ft_scale` bounds the square by `cap²`, and
/// dividing by `cap` -- the shift, since both `C` and `ft_scale` are powers
/// of two -- lands the result back in `[0, cap]`, exactly the range the
/// linear activation occupied. So `out_scale` is unchanged and the
/// read-out weights keep their meaning.
///
/// **That range is why this is not much dearer than the ReLU it replaced.**
/// The square needs int32 to be computed, but the shifted result is bounded
/// by `cap` and so fits back in int16 with nothing lost -- which puts the
/// weighted sum back on `vmlal_s16`, four lanes per instruction into int32,
/// instead of widening everything to int64. Squaring in int32 and
/// accumulating in int64 cost 8.64M nodes/s against 7.30M when first
/// written; narrowing costs two instructions per eight lanes and buys the
/// rest back.
///
/// NEON on aarch64, scalar elsewhere. Called once per leaf.
#[inline]
#[cfg_attr(feature = "stackedout", allow(dead_code))]
fn readout_dot(acc: &[i16], b: &[i16], w: &[i16], cap: i16, shift: i16) -> i64 {
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    unsafe {
        use std::arch::aarch64::*;
        let zero = vdupq_n_s16(0);
        let capv = vdupq_n_s16(cap);
        let sh = vdupq_n_s32(-(shift as i32));
        let mut sum = vdupq_n_s32(0);
        let mut h = 0;
        while h + 8 <= H {
            let s = vaddq_s16(vld1q_s16(acc.as_ptr().add(h)), vld1q_s16(b.as_ptr().add(h)));
            let a = vminq_s16(vmaxq_s16(s, zero), capv);
            // a² needs int32 to compute; a²>>shift is bounded by `cap` and
            // so returns to int16 exactly, with no saturation possible.
            let lo = vshlq_s32(vmull_s16(vget_low_s16(a), vget_low_s16(a)), sh);
            let hi = vshlq_s32(vmull_high_s16(a, a), sh);
            let phi = vcombine_s16(vmovn_s32(lo), vmovn_s32(hi));
            let ww = vld1q_s16(w.as_ptr().add(h));
            sum = vmlal_s16(sum, vget_low_s16(phi), vget_low_s16(ww));
            sum = vmlal_high_s16(sum, phi, ww);
            h += 8;
        }
        let mut acc64 = vaddvq_s32(sum) as i64;
        while h < H {
            let a = (*acc.get_unchecked(h))
                .wrapping_add(*b.get_unchecked(h))
                .clamp(0, cap) as i32;
            acc64 += ((a * a) >> shift) as i64 * *w.get_unchecked(h) as i64;
            h += 1;
        }
        acc64
    }
    #[cfg(any(not(target_arch = "aarch64"), feature = "nnue-scalar"))]
    {
        let mut sum: i64 = 0;
        for h in 0..H {
            let a = acc[h].wrapping_add(b[h]).clamp(0, cap) as i32;
            sum += ((a * a) >> shift) as i64 * w[h] as i64;
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
    m_mlp_mob_w: Vec<f32>,
    v_mlp_mob_w: Vec<f32>,
    /// Adam moments for the stacked read-out, one entry per table. Empty
    /// when the build has no stack, which is why they are read only under
    /// the feature.
    #[cfg_attr(not(feature = "stackedout"), allow(dead_code))]
    m_so: Vec<Vec<f32>>,
    #[cfg_attr(not(feature = "stackedout"), allow(dead_code))]
    v_so: Vec<Vec<f32>>,
    /// Scratch for the batched apply: per-row gradient accumulation with a
    /// stamp array instead of sorting (the 1M-pair sort of 132-byte elements
    /// per batch was the serial bottleneck of the whole step).
    grad_scratch: Vec<f32>,
    row_stamp: Vec<u32>,
    /// The same three for the phase-adaptive layer, whose rows are a
    /// separate table with a separate width.
    #[cfg_attr(not(feature = "pa128"), allow(dead_code))]
    /// Last step each row of the transformer / phase-adaptive layer moved.
    ft_last: Vec<u32>,
    pa_last: Vec<u32>,
    #[cfg_attr(not(feature = "pa128"), allow(dead_code))]
    m_pa: Vec<f32>,
    #[cfg_attr(not(feature = "pa128"), allow(dead_code))]
    v_pa: Vec<f32>,
    #[cfg_attr(not(feature = "pa128"), allow(dead_code))]
    m_pa_bias: Vec<f32>,
    #[cfg_attr(not(feature = "pa128"), allow(dead_code))]
    v_pa_bias: Vec<f32>,
    #[cfg_attr(not(feature = "pa128"), allow(dead_code))]
    pa_scratch: Vec<f32>,
    #[cfg_attr(not(feature = "pa128"), allow(dead_code))]
    pa_stamp: Vec<u32>,
    #[cfg_attr(not(feature = "pa128"), allow(dead_code))]
    pa_touched: Vec<Vec<u32>>,
    /* Lookahead, the other stabiliser in the recipe: keep a slow
    copy of every weight, and every `k` steps pull both toward each other by
    `alpha`. It smooths the trajectory the fast weights take.

    Off unless asked for, and worth knowing why. It rewrites *every*
    parameter every k steps, and here that is a 76M-cell base table plus a
    228M-cell phase-adaptive one -- work that does not shrink with the batch.
    On a GPU that sweep hides under the batch; on a CPU it is the batch. The
    cost is measured rather than assumed: see `--lookahead`. */
    /// Optimizer steps taken, for Adam's bias correction.
    t: u32,
    /// Reproduce the optimizer as it was before a numeric comparison
    /// found four things wrong with it: no bias correction, and a zero
    /// gradient skipping a cell's decay in three places. Exists so the
    /// fixes can be measured against what they replaced on identical data;
    /// nothing but the comparison should set it.
    pub legacy_optimizer: bool,
    la_k: u32,
    la_alpha: f32,
    la_step: u32,
    la_slow: Vec<Vec<f32>>,
    stamp_cur: u32,
    /// Rows each bucket touched this batch, kept between the accumulate and
    /// apply passes (gradient clipping needs the coalesced norm before any
    /// weight moves).
    touched: Vec<Vec<u32>>,
}

impl AdamState {
    /// Turn Lookahead on with the trained settings (`k = 6`,
    /// `alpha = 0.5`). `k = 0` leaves it off.
    pub fn set_lookahead(&mut self, k: u32, alpha: f32) {
        self.la_k = k;
        self.la_alpha = alpha;
        self.la_step = 0;
        self.la_slow.clear();
    }

    pub fn new(nn: &Nnue) -> AdamState {
        AdamState {
            beta1: 0.9,
            beta2: 0.999,
            eps: 1e-8,
            wd: 0.0,
            m_ft: vec![0.0; nn.ft.len()],
            v_ft: vec![0.0; nn.ft.len()],
            m_ft_bias: vec![0.0; FT_BIAS_LEN],
            v_ft_bias: vec![0.0; FT_BIAS_LEN],
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
            m_mlp_mob_w: vec![0.0; MLP_H1],
            v_mlp_mob_w: vec![0.0; MLP_H1],
            m_so: so_moment_shapes(),
            v_so: so_moment_shapes(),
            grad_scratch: vec![0.0; nn.ft.len()],
            row_stamp: vec![0; nn.ft.len() / ACC_DIMS],
            ft_last: vec![0; nn.ft.len() / ACC_DIMS],
            pa_last: vec![0; nn.pa.len().checked_div(PA_DIMS).unwrap_or(0)],
            m_pa: vec![0.0; nn.pa.len()],
            v_pa: vec![0.0; nn.pa.len()],
            m_pa_bias: vec![0.0; PA_DIMS * PA_BUCKETS],
            v_pa_bias: vec![0.0; PA_DIMS * PA_BUCKETS],
            pa_scratch: vec![0.0; nn.pa.len()],
            // `PA_DIMS` is zero without the layer, and so is `nn.pa`.
            pa_stamp: vec![0; nn.pa.len().checked_div(PA_DIMS).unwrap_or(0)],
            pa_touched: Vec::new(),
            t: 0,
            legacy_optimizer: false,
            la_k: 0,
            la_alpha: 0.5,
            la_step: 0,
            la_slow: Vec::new(),
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
#[cfg_attr(feature = "stackedout", allow(dead_code))]
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
#[cfg_attr(feature = "stackedout", allow(dead_code))]
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

/// The f32 twin of [`fold_pairs`], for the training paths.
///
/// Returns the folded lanes. `raw` carries the accumulated transformer
/// output, `ACC_DIMS` wide; the result is `H` wide and is what the read-out,
/* The stack's input activation. The layer stack is fed
`clamp(a,0,1)*clamp(b,0,1)`, so every input it sees is inside [0,1]; ours
arrives in disc units, and scaling alone let a raw +50 accumulator through.
Left unclamped the starting validation error was 1.03e6.

`PAIR_CLAMP_F32` is the range the accumulator activation is bounded to, so
dividing by it puts a saturated accumulator exactly at 1. */
/// The stack's direct inputs: the folded accumulator, activated, followed by
/// the phase-adaptive output, which arrives already activated.
#[cfg(feature = "stackedout")]
#[inline]
fn stack_inputs(acc: &[f32; H], pa: &[f32; PA_DIMS]) -> [f32; SO_SKIP] {
    let mut xin = [0.0f32; SO_SKIP];
    for h in 0..H {
        xin[h] = so_act(acc[h]);
    }
    xin[H..].copy_from_slice(pa);
    xin
}

/// Copy a per-stage table's first stage over all the others.
#[cfg(feature = "stackedout")]
fn replicate_stage0(t: &mut [f32], per_stage: usize) {
    for st in 1..SO_STAGES {
        let (head, tail) = t.split_at_mut(st * per_stage);
        tail[..per_stage].copy_from_slice(&head[..per_stage]);
    }
}

#[cfg(feature = "stackedout")]
#[inline]
fn so_act(a: f32) -> f32 {
    (a * (1.0 / PAIR_CLAMP_F32)).clamp(0.0, 1.0)
}

/// Derivative of [`so_act`]: flat inside the clamp, dead outside it.
#[cfg(feature = "stackedout")]
#[inline]
fn so_act_grad(a: f32) -> f32 {
    let x = a * (1.0 / PAIR_CLAMP_F32);
    if x > 0.0 && x < 1.0 {
        1.0 / PAIR_CLAMP_F32
    } else {
        0.0
    }
}

/// the head and the product gate all consume.
#[inline]
fn fold_pairs_f32(raw: &[f32; ACC_DIMS]) -> [f32; H] {
    #[cfg(not(feature = "pairmul"))]
    {
        *raw
    }
    #[cfg(feature = "pairmul")]
    {
        let mut out = [0.0f32; H];
        for i in 0..H {
            let a = raw[i].clamp(0.0, PAIR_CLAMP);
            let b = raw[i + H].clamp(0.0, PAIR_CLAMP);
            out[i] = a * b * (ACT_SCALE / PAIR_CLAMP);
        }
        out
    }
}

/// Push a gradient on the folded lanes back to the raw ones.
///
/// Without `pairmul` the fold is the identity and the gradient passes
/// straight through. With it, `d(a*b/C)/da = b/C` and symmetrically -- zero
/// outside the clamp, where the fold is flat.
#[inline]
fn fold_pairs_back(raw: &[f32; ACC_DIMS], dfolded: &[f32; H]) -> [f32; ACC_DIMS] {
    #[cfg(not(feature = "pairmul"))]
    {
        let _ = raw;
        *dfolded
    }
    #[cfg(feature = "pairmul")]
    {
        let mut d = [0.0f32; ACC_DIMS];
        for i in 0..H {
            let a = raw[i];
            let b = raw[i + H];
            let ca = a.clamp(0.0, PAIR_CLAMP);
            let cb = b.clamp(0.0, PAIR_CLAMP);
            if a > 0.0 && a < PAIR_CLAMP {
                d[i] = dfolded[i] * cb * (ACT_SCALE / PAIR_CLAMP);
            }
            if b > 0.0 && b < PAIR_CLAMP {
                d[i + H] = dfolded[i] * ca * (ACT_SCALE / PAIR_CLAMP);
            }
        }
        d
    }
}

/// `Σ row[i] · x[i]` over the first `n` lanes of both. NEON on aarch64,
/// scalar elsewhere. Two accumulators, so the multiply latency overlaps.
#[inline(always)]
#[cfg_attr(feature = "stackedout", allow(dead_code))]
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
    /// Step at which each row last took an update, so a row can catch up on
    /// the steps it sat out. See `catch_up`.
    last: *mut u32,
}
// SAFETY: buckets partition rows, so each thread's cells are disjoint.
unsafe impl Send for FtCells {}
unsafe impl Sync for FtCells {}

/// Bring one row up to date on the steps it sat out.
///
/// A dense optimizer touches every parameter every step: the decay shrinks
/// it and the momentum left from earlier steps keeps moving it. A sparse
/// one only touches the rows this batch lit up, so a row that goes quiet
/// stops decaying -- and over the millions of steps a full run takes, that
/// is the difference between a row a dense optimizer drives to nothing and
/// a row this keeps alive.
///
/// So replay the gap. The momentum tail dies off as `beta1^k`, so the loop
/// exits as soon as it stops mattering and the remaining decay closes in
/// one power. The one approximation is the learning rate: the schedule's
/// value during the skipped steps is not kept, so the current one stands in
/// for it. It only scales a term that is already near zero by the time the
/// gap is long enough for the difference to show.
///
/// # Safety
/// `m`, `v` and `w` must each point to `n` writable floats.
#[allow(clippy::too_many_arguments)]
#[inline]
unsafe fn catch_up(
    m: *mut f32,
    v: *mut f32,
    w: *mut f32,
    n: usize,
    last: u32,
    now: u32,
    lr: f32,
    wd: f32,
    b1: f32,
    b2: f32,
    eps: f32,
) {
    let miss = now.saturating_sub(last).saturating_sub(1);
    if miss == 0 {
        return;
    }
    let mut k = 0u32;
    let mut alive = true;
    while k < miss && alive {
        k += 1;
        let st = (last + k) as i32;
        let bc1 = 1.0 - b1.powi(st);
        let bc2s = (1.0 - b2.powi(st)).sqrt();
        alive = false;
        for i in 0..n {
            unsafe {
                let mi = m.add(i);
                let vi = v.add(i);
                *mi *= b1;
                *vi *= b2;
                let wi = w.add(i);
                *wi = *wi * (1.0 - wd * lr) - (lr / bc1) * *mi / ((*vi).sqrt() / bc2s + eps);
                if mi.read().abs() > 1e-12 {
                    alive = true;
                }
            }
        }
    }
    if k < miss && wd != 0.0 {
        let f = (1.0 - wd * lr).powi((miss - k) as i32);
        for i in 0..n {
            unsafe {
                *w.add(i) *= f;
            }
        }
    }
}

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
    /// Gradient for the head's mobility input (see `Nnue::mlp_mob_w`).
    pub mlp_mob_w: Vec<f32>,
    /// Gradients for the stacked read-out (see `Nnue::stacked_readout`).
    pub so_l1_w: Vec<f32>,
    pub so_l1_b: Vec<f32>,
    pub so_l2_w: Vec<f32>,
    pub so_l2_b: Vec<f32>,
    pub so_out_w: Vec<f32>,
    pub so_out_b: Vec<f32>,
    pub ft_bias: Vec<f32>,
    pub pa_bias: Vec<f32>,
    pub pw: Vec<f32>,
    /// (feature row, err*delta per lane) bucketed by `row % parts`, so the
    /// apply phase can run one thread per bucket: rows — and therefore the
    /// scratch, moment and weight cells they touch — are disjoint across
    /// buckets by construction. Without this the apply is serial and
    /// dominates (a batch's million row-updates outweigh the threaded
    /// forward pass by ~10x).
    pub ft_rows: Vec<Vec<(u32, [f32; ACC_DIMS])>>,
    /// The same sparse collection for the phase-adaptive layer's rows.
    pub pa_rows: Vec<Vec<(u32, [f32; PA_DIMS])>>,
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
            mlp_mob_w: vec![0.0; MLP_H1],
            so_l1_w: vec![0.0; SO_SIZES.0],
            so_l1_b: vec![0.0; SO_SIZES.1],
            so_l2_w: vec![0.0; SO_SIZES.2],
            so_l2_b: vec![0.0; SO_SIZES.3],
            so_out_w: vec![0.0; SO_SIZES.4],
            so_out_b: vec![0.0; SO_SIZES.5],
            ft_bias: vec![0.0; FT_BIAS_LEN],
            pa_bias: vec![0.0; PA_DIMS * PA_BUCKETS],
            pw: vec![0.0; STAGE_COUNT * HALF],
            ft_rows: (0..parts).map(|_| Vec::new()).collect(),
            pa_rows: (0..parts).map(|_| Vec::new()).collect(),
            parts,
        }
    }

    #[inline]
    fn push_ft(&mut self, row: u32, vals: &[f32; ACC_DIMS]) {
        let p = row as usize % self.parts;
        self.ft_rows[p].push((row, *vals));
    }

    #[cfg(feature = "pa128")]
    #[inline]
    fn push_pa(&mut self, row: u32, vals: &[f32; PA_DIMS]) {
        let p = row as usize % self.parts;
        self.pa_rows[p].push((row, *vals));
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
        self.mlp_mob_w.fill(0.0);
        self.so_l1_w.fill(0.0);
        self.so_l1_b.fill(0.0);
        self.so_l2_w.fill(0.0);
        self.so_l2_b.fill(0.0);
        self.so_out_w.fill(0.0);
        self.so_out_b.fill(0.0);
        self.ft_bias.fill(0.0);
        self.pa_bias.fill(0.0);
        self.pw.fill(0.0);
        for b in self.ft_rows.iter_mut() {
            b.clear();
        }
        for b in self.pa_rows.iter_mut() {
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
    /// Phase-adaptive input layer: `pa[(bucket * n_feat_bucket + row) *
    /// PA_DIMS + j]`, with its own bias per bucket. Empty without `pa128`.
    pub pa: Vec<f32>,
    pub pa_bias: Vec<f32>,
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
    /// The head's first layer takes the side to move's legal-move count as
    /// one more input, so mobility can interact with the pattern lanes
    /// instead of only being added at the end (`mob_w`, which stays: a
    /// per-bucket table says things a single weight cannot).
    mlp_mob_w: Vec<f32>,
    /// The stacked read-out's weights, one set per stage. Unused (and
    /// empty) unless the `stackedout` feature is on.
    so_l1_w: Vec<f32>,
    so_l1_b: Vec<f32>,
    so_l2_w: Vec<f32>,
    so_l2_b: Vec<f32>,
    so_out_w: Vec<f32>,
    so_out_b: Vec<f32>,

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
    /// `ACT_CLAMP` in quantized-accumulator units, and the shift that undoes
    /// the squaring's scale (see `readout_dot`).
    act_clamp_q: i16,
    act_shift_q: i16,
    /// Clamp and shift for the pairwise fold (see `fold_pairs`). Both are
    /// derived from `ft_scale`, so they travel with the quantisation.
    /// Run the head's first layer in f32 instead of int8. See `extras`.
    pub head_f32: bool,
    /// Steps per disc on the head's int8 activation, chosen before
    /// `quantize`. Finer steps resolve small activations but saturate
    /// sooner: int8 tops out at `127 / act_units` discs.
    pub act_units: f32,
    #[cfg_attr(feature = "stackedout", allow(dead_code))]
    pair_clamp_q: i16,
    #[cfg_attr(feature = "stackedout", allow(dead_code))]
    pair_shift_q: i16,
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
            ft: vec![0.0; n_features * ACC_DIMS],
            ft_bias: vec![0.0; FT_BIAS_LEN],
            pa: vec![0.0; n_feat_bucket * PA_BUCKETS * PA_DIMS],
            pa_bias: vec![0.0; PA_DIMS * PA_BUCKETS],
            out_w: vec![0.0; STAGE_COUNT * H],
            out_b: vec![0.0; STAGE_COUNT],
            num_w: vec![0.0; STAGE_COUNT * NUM_TABLE_SIZE],
            mob_w: vec![0.0; STAGE_COUNT * MOB_BUCKETS],
            mlp_l1_w: vec![0.0; MLP_H1 * H],
            mlp_l1_b: vec![0.0; MLP_H1],
            mlp_l2_w: vec![0.0; MLP_H2 * MLP_H1],
            mlp_l2_b: vec![0.0; MLP_H2],
            mlp_out_w: vec![0.0; STAGE_COUNT * MLP_H2],
            mlp_mob_w: vec![0.0; MLP_H1],
            so_l1_w: vec![0.0; SO_SIZES.0],
            so_l1_b: vec![0.0; SO_SIZES.1],
            so_l2_w: vec![0.0; SO_SIZES.2],
            so_l2_b: vec![0.0; SO_SIZES.3],
            so_out_w: vec![0.0; SO_SIZES.4],
            so_out_b: vec![0.0; SO_SIZES.5],
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
            ft_bias_i16: vec![0; FT_BIAS_LEN],
            out_w_i16: vec![0; STAGE_COUNT * H],
            pw_i16: vec![0; STAGE_COUNT * HALF],
            prod_scale: 0.0,
            prod_clamp_q: 0,
            act_clamp_q: 0,
            act_shift_q: 0,
            head_f32: false,
            act_units: ACT_UNITS,
            pair_clamp_q: 0,
            pair_shift_q: 0,
            out_scale: 0.0,
            ftc_i32: Vec::new(),
            ft_bias_i32: vec![0; FT_BIAS_LEN],
            out_w_i32: vec![0; STAGE_COUNT * H],
            out_scale_i32: 0.0,
            ftc_f32: Vec::new(),
            ft_bias_f32: vec![0.0; FT_BIAS_LEN],
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
    /// accumulates in on NEON (`vmlal_s16`): what it multiplies is not the
    /// accumulator but `φ(acc)`, which the squared clipped activation caps
    /// at `ACT_CLAMP · ft_scale`. So the budget is `φ_max · w_max · H ≤
    /// 2^31`. Filling the full i16 range regardless silently overflows those
    /// lanes and flips the sign of the score.
    ///
    /// Budgeting against the accumulator's own range instead — which is what
    /// this did while the activation was a plain ReLU — costs the read-out
    /// some five bits of scale for nothing, since `φ` is thirty times
    /// smaller than the accumulator can be.
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
        // Overridable so the budget's cost can be measured rather than
        // assumed; the constant is what ships.
        let budget: f32 = std::env::var("KUROOBI_FT_CLIP_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(FT_CLIP_BUDGET);
        while ft_scale > 1.0 && clip_fraction(ft_scale) > budget {
            ft_scale *= 0.5;
        }
        self.ft_clipped = (clip_fraction(ft_scale) * self.ft.len() as f32) as usize;

        // Saturation caps a cell at 127, so the accumulator's real bound is
        // that, not the unclipped weight's.
        /* The read-out sums `phi(acc) * w`, and the squared clipped
        activation bounds `phi` by the clamp -- not by the accumulator's own
        range, which is what this used to budget against. The accumulator
        can reach 64 masks times 127; `phi` cannot exceed `ACT_CLAMP *
        ft_scale`, some thirty times smaller. Budgeting against the real
        bound leaves the read-out weights a far finer scale for the same
        int32 headroom. */
        let phi_max = (ACT_CLAMP * ft_scale).min(32_767.0);
        let w_limit = (i32::MAX as f32 / (phi_max * H as f32)).min(32_000.0);
        let w_scale = w_limit / w_max;
        self.out_scale = 1.0 / (ft_scale * w_scale);
        self.ft_scale = ft_scale;

        let q = |v: f32, s: f32| (v * s).round().clamp(-32768.0, 32767.0) as i16;

        for i in 0..FT_BIAS_LEN {
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
        /* The read-out's squared activation. Both `ACT_CLAMP` and `ft_scale`
        are powers of two, so dividing the square by the cap is a shift and
        the result lands back in `[0, cap]` -- the range the linear
        activation had, which is why `out_scale` needs no adjustment. */
        let cap = (ACT_CLAMP * ft_scale).min(32_767.0);
        self.act_clamp_q = cap as i16;
        self.act_shift_q = cap.log2().round() as i16;
        /* The pairwise fold, when the input layer is doubled. Same shape as
        the squared activation: clamp both factors, multiply, shift by the
        clamp so the result lands back in the range a single lane occupied
        and every downstream scale keeps its meaning. */
        #[cfg(feature = "pairmul")]
        {
            let pc = (PAIR_CLAMP * ft_scale).min(32_767.0);
            self.pair_clamp_q = pc as i16;
            self.pair_shift_q = pc.log2().round() as i16;
        }

        /* The head's first layer, int8 on both sides so `sdot` can run it.
        The activation side is a right shift of the accumulator the read-out
        already holds, so the shift has to leave `ACT_UNITS` steps per disc;
        the weight side takes whatever scale fills int8. */
        /* Steps per disc on the head's int8 activation. Overridable so the
        trade can be measured: finer steps resolve small activations but
        saturate sooner (int8 tops out at `127 / units` discs), and the
        shipped 16 tops out at 7.9 -- which a mid-game accumulator passes
        routinely. */
        self.act_shift = (ft_scale / self.act_units).max(1.0).log2().round() as i16;
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
        self.ft_b_i8 = vec![0; self.n_features * ACC_DIMS];
        self.ft_w_i8 = vec![0; self.n_features * ACC_DIMS];
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
                    let src = (base + self.indexer.swapped_index(m, i)) * ACC_DIMS;
                    let dst = (base + i) * ACC_DIMS;
                    self.ft_w_i8[dst..dst + ACC_DIMS]
                        .copy_from_slice(&self.ft_b_i8[src..src + ACC_DIMS]);
                }
            }
        }
    }

    /// How many transformer cells saturated int8, and how many there are.
    /// A training run that pushes this past a fraction of a percent is
    /// spending accuracy in the table the search reads, not in the one the
    /// loss sees; [`FT_CLIP_BUDGET`] is where `quantize` starts backing the
    /// scale off instead.
    /// The scales `quantize` chose, for reporting how coarse the conversion
    /// the search reads actually is: transformer scale, read-out scale, and
    /// the largest transformer weight it had to fit.
    /// Drop the product-gate term, to see what its quantization costs.
    pub fn zero_pw(&mut self) {
        self.pw.fill(0.0);
        self.pw_i16.fill(0);
        self.has_pw = false;
    }

    pub fn quant_scales(&self) -> (f32, f32, f32) {
        let ft_max = self.ft.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        (
            self.ft_scale,
            1.0 / (self.out_scale * self.ft_scale),
            ft_max,
        )
    }

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
            for h in 0..ACC_DIMS {
                self.ftc_i16[f * H2 + h] = q(self.ft[f * ACC_DIMS + h]);
            }
        }
        for bucket in 0..FT_BUCKETS {
            let bucket_base = bucket * self.n_feat_bucket;
            for m in 0..self.n_masks {
                let base = bucket_base + self.mask_off[m] as usize;
                let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
                for i in 0..size {
                    let src = (base + self.indexer.swapped_index(m, i)) * H2;
                    let dst = (base + i) * H2 + ACC_DIMS;
                    for h in 0..ACC_DIMS {
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
            for h in 0..ACC_DIMS {
                self.ftc_i32[f * H2 + h] = (self.ft[f * ACC_DIMS + h] * ft_scale32).round() as i32;
            }
        }
        // f32 path: interleaved, no quantization (reference precision).
        self.ftc_f32 = vec![0.0; self.n_features * H2];
        for f in 0..self.n_features {
            for h in 0..ACC_DIMS {
                self.ftc_f32[f * H2 + h] = self.ft[f * ACC_DIMS + h];
            }
        }
        for i in 0..FT_BIAS_LEN {
            self.ft_bias_i32[i] = (self.ft_bias[i] * ft_scale32).round() as i32;
            self.ft_bias_f32[i] = self.ft_bias[i];
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
                    let dst = (base + i) * H2 + ACC_DIMS;
                    for h in 0..ACC_DIMS {
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
        /* Positive so ReLU units start active (same value across stages).
        It starts at zero instead, and with the stacked
        read-out there is no ReLU here to keep active -- the accumulator
        feeds a product whose factors are clamped at zero either way. */
        #[cfg(feature = "stackedout")]
        self.ft_bias.fill(0.0);
        #[cfg(not(feature = "stackedout"))]
        self.ft_bias.fill(0.1);
        self.init_mlp_hidden();
        /* A product layer cannot start from zero.
        With `pairmul` a lane's output is `a * b`, so its gradients are
        `d/da = b` and `d/db = a`: an all-zero table produces zero output
        *and* zero gradient, and the layer never leaves the origin. That is
        exactly what a first attempt did -- training error stuck at 374
        where the summing layer reached 26, because nothing moved. A
        summing layer has no such problem, which is why the table has always
        started at zero, and why this is the one shape that needs seeding.

        He initialisation scaled down, which is what a product layer
        wants (uniform on the fan-in, then
        multiplied by 0.25): the bound is `sqrt(6 / fan_in) * 0.25`.

        Fan-in is `ACC_DIMS`, the accumulator width. That reads backwards --
        a lane sums one row per mask, so the masks look like the fan-in --
        but the table is laid out `[features, width]` and the rule
        takes `size(1)` as fan-in, so the width is what its bound is
        computed from. Matching the number rather than the reasoning is the
        point here.

        Signed, not positive-only: a lane pair should be able to learn
        either direction, and the clamp at zero prunes the half it does not
        want. */
        #[cfg(feature = "pairmul")]
        {
            let bound = (6.0 / ACC_DIMS as f32).sqrt() * 0.25;
            for w in &mut self.ft {
                *w = next() * bound;
            }
        }
        /* The phase-adaptive layer, initialised the same way -- and, like
        every phase copy starts from the first one. Six
        independent draws would be six different sub-models each seeing a
        sixth of the data. */
        #[cfg(feature = "pa128")]
        {
            let bound = (6.0 / (PA_DIMS * PA_BUCKETS) as f32).sqrt() * 0.25;
            let rows = self.n_feat_bucket;
            for r in 0..rows {
                for j in 0..PA_DIMS {
                    self.pa[r * PA_DIMS + j] = next() * bound;
                }
            }
            for b in 1..PA_BUCKETS {
                let (head, tail) = self.pa.split_at_mut(b * rows * PA_DIMS);
                tail[..rows * PA_DIMS].copy_from_slice(&head[..rows * PA_DIMS]);
            }
            self.pa_bias.fill(0.0);
        }
        /* The stacked read-out's layers, He-initialised on their own fan-in.
        A dense layer of zeros is as dead as a product layer of zeros once
        anything downstream multiplies: L1's output is squared before L2
        sees it, so an all-zero L1 gives L2 nothing to differentiate. The
        final layer's bias starts at zero. */
        #[cfg(feature = "stackedout")]
        {
            /* The usual dense-layer default, which is what these
            stacks get: weight and bias both uniform on +/-1/sqrt(fan_in),
            and a zero bias on the output layer.

            Drawn once and copied across every stage, which is why it
            copies its first stack over all the others. Sixty-one
            independent draws are sixty-one different models, each trained
            on the 1/61 of the data that lands in its stage. */
            let b1 = 1.0 / ((H + 1) as f32).sqrt();
            for i in 0..SO_L1 * (H + 1) {
                self.so_l1_w[i] = next() * b1;
            }
            for i in 0..SO_L1 {
                self.so_l1_b[i] = next() * b1;
            }
            let b2 = 1.0 / ((SO_L1 * 2) as f32).sqrt();
            for i in 0..SO_L2 * (SO_L1 * 2) {
                self.so_l2_w[i] = next() * b2;
            }
            for i in 0..SO_L2 {
                self.so_l2_b[i] = next() * b2;
            }
            let bo = 1.0 / ((SO_L2 + H) as f32).sqrt();
            for i in 0..SO_L2 + H {
                self.so_out_w[i] = next() * bo;
            }
            self.so_out_b[0] = 0.0;
            replicate_stage0(&mut self.so_l1_w, SO_L1 * (H + 1));
            replicate_stage0(&mut self.so_l1_b, SO_L1);
            replicate_stage0(&mut self.so_l2_w, SO_L2 * (SO_L1 * 2));
            replicate_stage0(&mut self.so_l2_b, SO_L2);
            replicate_stage0(&mut self.so_out_w, SO_L2 + H);
            replicate_stage0(&mut self.so_out_b, 1);
        }
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

    #[cfg_attr(feature = "stackedout", allow(clippy::needless_return))]
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
        let mut raw_acc = [0i16; ACC_DIMS]; // bias is added at readout
                                            // The stage picks which transformer copy to read; the row offsets
                                            // inside a copy are the same, so it is a base-pointer shift and the
                                            // loop below reads exactly as many rows as with one copy.
        let base = ft_bucket(stage) * self.n_feat_bucket * ACC_DIMS;
        // SAFETY: indices stay inside their pattern's table (the invariant the
        // scalar sum relies on), so every base + ACC_DIMS is in bounds.
        unsafe {
            accumulate_rows(
                &mut raw_acc,
                ft.as_ptr().add(base),
                &self.mask_off,
                indices.raw(),
                self.n_masks,
            );
        }
        #[cfg(feature = "stackedout")]
        {
            /* The stacked read-out runs in f32, as the head already does.
            Its layers are dense and small next to the 32 random row reads
            that dominate a leaf, and quantising them is an optimisation to
            make once the shape has earned its place.

            The fold happens here rather than on the int16 lanes, so that
            the activation scale and the clamp are the same arithmetic the
            trainer used. Only the accumulator is quantized. */
            let inv = 1.0 / self.ft_scale;
            let mut raw = [0.0f32; ACC_DIMS];
            for i in 0..ACC_DIMS {
                raw[i] = raw_acc[i] as f32 * inv + self.ft_bias[i];
            }
            let acc = fold_pairs_f32(&raw);
            let ix = self.indexer.init(board.black, board.white);
            let feats = self.features_player(&ix, board.player(), stage);
            let (pa, _) = self.pa_forward(&feats, stage);
            return self.stacked_readout(&acc, &pa, Self::mob_index(board), stage);
        }
        #[cfg(not(feature = "stackedout"))]
        {
            let acc = fold_pairs(&raw_acc, self.pair_clamp_q, self.pair_shift_q);
            let fb = &self.ft_bias_i16[stage * H..stage * H + H];
            let ow = &self.out_w_i16[stage * H..stage * H + H];
            let sum = readout_dot(&acc, fb, ow, self.act_clamp_q, self.act_shift_q);
            self.out_b[stage]
                + sum as f32 * self.out_scale
                + self.num_term(board, stage)
                + self.mob_term(board, stage)
                + self.extras(&acc, fb, Self::mob_index(board), stage)
        }
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
    #[cfg_attr(feature = "stackedout", allow(dead_code))]
    fn extras(&self, acc: &[i16; H], fb: &[i16], mob: usize, stage: usize) -> f32 {
        let mut out = 0.0;
        if self.has_pw {
            out += self.prod_sum_q(acc, fb, stage) as f32 * self.prod_scale;
        }
        if self.has_head {
            /* The head's first layer in f32 rather than int8, when asked.
            The int8 form was measured against a model whose head was a
            correction on top of a read-out that already carried the score;
            on a model where the head *is* most of the score, the same
            conversion costs 1.3 discs of held-out error -- the whole gap
            between this model's trained accuracy and what the search
            reads. Off by default until the speed side is measured. */
            if self.head_f32 {
                // Plain ReLU with no ceiling, which is what `forward` feeds
                // the head. The int8 form saturates at 127 after its shift;
                // that ceiling is the thing under test here.
                let mut a = [0.0f32; H];
                let inv = 1.0 / self.ft_scale;
                for h in 0..H {
                    a[h] = (acc[h].wrapping_add(fb[h])).max(0) as f32 * inv;
                }
                out += self.mlp_term(&a, mob, stage);
            } else {
                out += self.mlp_term_i8(&activations_i8(acc, fb, self.act_shift), mob, stage);
            }
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
    #[cfg_attr(feature = "stackedout", allow(dead_code))]
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
        let base = self.forward(&feats, Self::mob_index(board), stage);
        #[cfg(feature = "stackedout")]
        {
            base
        }
        #[cfg(not(feature = "stackedout"))]
        {
            base + self.num_term(board, stage) + self.mob_term(board, stage)
        }
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
    #[cfg_attr(feature = "stackedout", allow(dead_code))]
    fn mlp_term(&self, a: &[f32; H], mob: usize, stage: usize) -> f32 {
        let m = mob_input(mob);
        let mut x1 = [0.0f32; MLP_H1];
        for (i, x) in x1.iter_mut().enumerate() {
            let v =
                self.mlp_l1_b[i] + self.mlp_mob_w[i] * m + dot_f32(&self.mlp_l1_w[i * H..], a, H);
            *x = v.max(0.0);
        }
        self.mlp_tail(x1, stage)
    }

    /// The head's first layer over int8 activations, then the same tail.
    ///
    /// Only this layer is quantized. It is `MLP_H1 * H` products against the
    /// tail's `MLP_H2 * MLP_H1 + MLP_H2`, so at H=64 it is 79% of the head's
    /// arithmetic and the rest is not worth the accuracy.
    #[inline]
    #[cfg_attr(feature = "stackedout", allow(dead_code))]
    fn mlp_term_i8(&self, a: &[i8; H], mob: usize, stage: usize) -> f32 {
        let m = mob_input(mob);
        let mut x1 = [0.0f32; MLP_H1];
        for (i, x) in x1.iter_mut().enumerate() {
            // `a` carries the activations biased by -128 (see
            // `activations_i8`); the row's weight sum puts that back.
            let d = dot_i8(&self.mlp_l1_w_i8[i * H..], a) + 128 * self.mlp_l1_rowsum[i];
            let v = self.mlp_l1_b[i] + self.mlp_mob_w[i] * m + d as f32 * self.mlp_l1_dequant;
            *x = v.max(0.0);
        }
        self.mlp_tail(x1, stage)
    }

    /// Second layer and read-out, shared by both first-layer paths.
    #[inline]
    #[cfg_attr(feature = "stackedout", allow(dead_code))]
    fn mlp_tail(&self, x1: [f32; MLP_H1], stage: usize) -> f32 {
        let mut x2 = [0.0f32; MLP_H2];
        for (j, x) in x2.iter_mut().enumerate() {
            *x = (self.mlp_l2_b[j] + dot_f32(&self.mlp_l2_w[j * MLP_H1..], &x1, MLP_H1)).max(0.0);
        }
        dot_f32(&self.mlp_out_w[stage * MLP_H2..], &x2, MLP_H2)
    }

    /// The phase-adaptive layer for one position: a sparse sum over the same
    /// feature rows the base layer reads, from this phase's copy, through a
    /// squared clipped activation.
    ///
    /// Returns the activation and its input; the backward pass needs the
    /// latter and recomputing it there would mean summing the rows twice.
    #[cfg(feature = "stackedout")]
    fn pa_forward(
        &self,
        feats: &[u32; MAX_MASKS],
        stage: usize,
    ) -> ([f32; PA_DIMS], [f32; PA_DIMS]) {
        #[allow(unused_mut)]
        let mut z = [0.0f32; PA_DIMS];
        #[cfg(feature = "pa128")]
        {
            let bucket = pa_bucket(stage);
            z.copy_from_slice(&self.pa_bias[bucket * PA_DIMS..bucket * PA_DIMS + PA_DIMS]);
            let base_row = bucket * self.n_feat_bucket;
            for &f in feats.iter().take(self.n_masks) {
                let off = (base_row + f as usize) * PA_DIMS;
                let row = &self.pa[off..off + PA_DIMS];
                for (j, zz) in z.iter_mut().enumerate() {
                    *zz += row[j];
                }
            }
        }
        #[cfg(not(feature = "pa128"))]
        let _ = (feats, stage);
        let mut a = [0.0f32; PA_DIMS];
        for (j, av) in a.iter_mut().enumerate() {
            let c = z[j].clamp(0.0, 1.0);
            *av = c * c * ACT_SCALE;
        }
        (a, z)
    }

    /// The stacked read-out: the whole score from the stack's inputs and
    /// mobility.
    ///
    /// `acc` is in disc units; everything inside runs on `[0, 1]`, so it is
    /// divided by the fold's clamp on the way in and the result multiplied
    /// by `SO_SCORE` on the way out. `pa` arrives already activated.
    ///
    /// The paired activation is the part worth naming: `L1`'s output is
    /// concatenated with its own square before `L2` sees it, so one layer
    /// hands the next both a linear and a quadratic view of the same
    /// sixteen values. The final layer takes `L2` *and* the stack's inputs,
    /// a short path from the input alongside the deep one.
    #[cfg(feature = "stackedout")]
    fn stacked_readout(
        &self,
        acc: &[f32; H],
        pa: &[f32; PA_DIMS],
        mob: usize,
        stage: usize,
    ) -> f32 {
        let st = so_stage(stage);
        let xin = stack_inputs(acc, pa);
        let mut l1 = [0.0f32; SO_L1];
        for (i, v) in l1.iter_mut().enumerate() {
            let row = &self.so_l1_w[(st * SO_L1 + i) * SO_L1_IN..];
            let mut x = self.so_l1_b[st * SO_L1 + i];
            for (k, &xv) in xin.iter().enumerate() {
                x += row[k] * xv;
            }
            x += row[SO_SKIP] * mob_unit(mob);
            *v = x;
        }
        // Squared and plain, side by side, then clamped.
        let mut a1 = [0.0f32; SO_L1 * 2];
        for i in 0..SO_L1 {
            a1[i] = (l1[i] * l1[i] * ACT_SCALE).clamp(0.0, 1.0);
            a1[SO_L1 + i] = l1[i].clamp(0.0, 1.0);
        }
        let mut l2 = [0.0f32; SO_L2];
        for (j, v) in l2.iter_mut().enumerate() {
            let row = &self.so_l2_w[(st * SO_L2 + j) * (SO_L1 * 2)..];
            let mut x = self.so_l2_b[st * SO_L2 + j];
            for (i, a) in a1.iter().enumerate() {
                x += row[i] * a;
            }
            // Squared clipped between L2 and output.
            *v = x.clamp(0.0, 1.0) * x.clamp(0.0, 1.0) * ACT_SCALE;
        }
        let ow = &self.so_out_w[st * SO_OUT_IN..];
        let mut out = self.so_out_b[st];
        for (j, v) in l2.iter().enumerate() {
            out += ow[j] * v;
        }
        for (k, &xv) in xin.iter().enumerate() {
            out += ow[SO_L2 + k] * xv;
        }
        out * SO_SCORE
    }

    /// Mobility index of a position (own legal moves, clamped to a bucket).
    #[inline]
    pub fn mob_index(board: &Board) -> usize {
        /* The stacked read-out takes the raw count, then scales by
        7/255 and clamps the *result* at one, so everything up to 36 stays
        distinguishable. Rounding into a 24-wide bucket first threw away the
        12 counts above it. The bucketed form is what `mob_w` indexes, and
        that table only exists in the other shape. */
        #[cfg(feature = "stackedout")]
        {
            board.movable_count() as usize
        }
        #[cfg(not(feature = "stackedout"))]
        {
            (board.movable_count() as usize).min(MOB_BUCKETS - 1)
        }
    }

    /// Tempo correction for the side to move (see [`Nnue::mob_w`]).
    #[inline]
    fn mob_term(&self, board: &Board, stage: usize) -> f32 {
        self.mob_w[stage * MOB_BUCKETS + Self::mob_index(board)]
    }

    #[cfg_attr(feature = "stackedout", allow(clippy::needless_return))]
    /// Forward from explicit features + stage (without the disc-count term).
    fn forward(&self, feats: &[u32; MAX_MASKS], mob: usize, stage: usize) -> f32 {
        let mut raw = [0.0f32; ACC_DIMS];
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * ACC_DIMS;
            let row = &self.ft[base..base + ACC_DIMS];
            for h in 0..ACC_DIMS {
                raw[h] += row[h];
            }
        }
        // Before the fold with the stacked read-out, after it otherwise --
        // see `FT_BIAS_LEN`.
        #[cfg(feature = "stackedout")]
        for h in 0..ACC_DIMS {
            raw[h] += self.ft_bias[h];
        }
        #[allow(unused_mut)]
        let mut acc = fold_pairs_f32(&raw);
        #[cfg(not(feature = "stackedout"))]
        for (h, b) in self.ft_bias[stage * H..stage * H + H].iter().enumerate() {
            acc[h] += b;
        }
        #[cfg(feature = "stackedout")]
        {
            // The stacked read-out replaces the linear one, the product gate
            // and the head all at once -- it is the whole score from the
            // folded lanes, so none of the terms below apply.
            let (pa, _) = self.pa_forward(feats, stage);
            return self.stacked_readout(&acc, &pa, mob, stage);
        }
        #[cfg(not(feature = "stackedout"))]
        let ow = &self.out_w[stage * H..stage * H + H];
        #[cfg(not(feature = "stackedout"))]
        let mut out = self.out_b[stage];
        #[cfg(not(feature = "stackedout"))]
        for h in 0..H {
            out += ow[h] * screlu(acc[h]);
        }
        #[cfg(not(feature = "stackedout"))]
        {
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
            out + self.mlp_term(&a, mob, stage)
        }
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
        /* Diagnostic paths, and the bias they want does not exist with the
        stacked read-out: there it sits on the raw lanes and is not indexed
        by stage. Leaving it out beats indexing past the end of the table. */
        #[cfg(not(feature = "stackedout"))]
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
        /* Diagnostic paths, and the bias they want does not exist with the
        stacked read-out: there it sits on the raw lanes and is not indexed
        by stage. Leaving it out beats indexing past the end of the table. */
        #[cfg(not(feature = "stackedout"))]
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

    #[cfg_attr(feature = "stackedout", allow(clippy::needless_return))]
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
        let mut raw = [0.0f32; ACC_DIMS];
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * ACC_DIMS;
            for h in 0..ACC_DIMS {
                raw[h] += self.ft[base + h];
            }
        }
        #[cfg(feature = "stackedout")]
        for h in 0..ACC_DIMS {
            raw[h] += self.ft_bias[h];
        }
        #[allow(unused_mut)]
        let mut acc = fold_pairs_f32(&raw);
        #[cfg(not(feature = "stackedout"))]
        for (h, b) in self.ft_bias[stage * H..stage * H + H].iter().enumerate() {
            acc[h] += b;
        }
        #[cfg(feature = "stackedout")]
        {
            let (pa, pa_z) = self.pa_forward(&feats, stage);
            let (sq, draw, dpa) =
                self.grad_stacked(&raw, &acc, &pa, &pa_z, discs, mob, target, stage, sink);
            for &f in feats.iter().take(self.n_masks) {
                sink.push_ft(f, &draw);
            }
            #[cfg(feature = "pa128")]
            {
                let base_row = (pa_bucket(stage) * self.n_feat_bucket) as u32;
                for &f in feats.iter().take(self.n_masks) {
                    sink.push_pa(base_row + f, &dpa);
                }
            }
            #[cfg(not(feature = "pa128"))]
            let _ = dpa;
            return sq;
        }
        #[cfg(not(feature = "stackedout"))]
        {
            let num_off = stage * NUM_TABLE_SIZE + discs;
            let mob_off = stage * MOB_BUCKETS + mob.min(MOB_BUCKETS - 1);
            let ow_off = stage * H;
            let pw_off = stage * HALF;
            let mut out = self.out_b[stage] + self.num_w[num_off] + self.mob_w[mob_off];
            for h in 0..H {
                out += self.out_w[ow_off + h] * screlu(acc[h]);
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
            let mob_in = mob_input(mob);
            let mut z1 = [0.0f32; MLP_H1];
            let mut x1 = [0.0f32; MLP_H1];
            for i in 0..MLP_H1 {
                let row = &self.mlp_l1_w[i * H..i * H + H];
                let mut v = self.mlp_l1_b[i] + self.mlp_mob_w[i] * mob_in;
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
                // d(out)/d(acc) through the squared clipped activation, and the
                // read-out weight's own gradient against the activation itself.
                delta[h] = self.out_w[ow_off + h] * screlu_grad(acc[h]);
                sink.out_w[ow_off + h] += err * screlu(acc[h]);
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
                sink.mlp_mob_w[i] += err * dz1 * mob_in;
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
            // `delta` is on the folded lanes; the table lives on the raw ones.
            // Without `pairmul` the fold is the identity and this is a copy.
            let draw = fold_pairs_back(&raw, &delta);
            for &f in feats.iter().take(self.n_masks) {
                let mut row = [0.0f32; ACC_DIMS];
                for h in 0..ACC_DIMS {
                    row[h] = err * draw[h];
                }
                sink.push_ft(f, &row);
            }
            err * err
        }
    }

    /// Forward and backward through the stacked read-out, in one pass.
    ///
    /// Returns the squared error in the same units the rest of the trainer
    /// reports, so the two shapes' `train` numbers stay on one scale even
    /// though the network inside works on `[0, 1]`.
    ///
    /// **The error fed backwards is divided by `SO_SCORE`.** This shape
    /// trains against targets divided by 64, which puts its gradients -- and
    /// therefore its learning rate -- on the normalised scale. Keeping the
    /// loss in discs while the network is normalised would multiply every
    /// gradient by 64², and no rate tuned on the normalised scale would
    /// transfer.
    #[cfg(feature = "stackedout")]
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn grad_stacked(
        &self,
        raw: &[f32; ACC_DIMS],
        acc: &[f32; H],
        pa: &[f32; PA_DIMS],
        pa_z: &[f32; PA_DIMS],
        discs: usize,
        mob: usize,
        target: f32,
        stage: usize,
        sink: &mut GradSink,
    ) -> (f32, [f32; ACC_DIMS], [f32; PA_DIMS]) {
        let _ = discs;
        let st = so_stage(stage);
        let m = mob_unit(mob);
        let xin = stack_inputs(acc, pa);

        // ---- forward, keeping every pre-activation for the way back ----
        let mut l1 = [0.0f32; SO_L1];
        for (i, v) in l1.iter_mut().enumerate() {
            let row = &self.so_l1_w[(st * SO_L1 + i) * SO_L1_IN..];
            let mut x = self.so_l1_b[st * SO_L1 + i];
            for (k, &xv) in xin.iter().enumerate() {
                x += row[k] * xv;
            }
            *v = x + row[SO_SKIP] * m;
        }
        let mut a1 = [0.0f32; SO_L1 * 2];
        for i in 0..SO_L1 {
            a1[i] = (l1[i] * l1[i] * ACT_SCALE).clamp(0.0, 1.0);
            a1[SO_L1 + i] = l1[i].clamp(0.0, 1.0);
        }
        let mut l2 = [0.0f32; SO_L2];
        let mut v2 = [0.0f32; SO_L2];
        for j in 0..SO_L2 {
            let row = &self.so_l2_w[(st * SO_L2 + j) * (SO_L1 * 2)..];
            let mut x = self.so_l2_b[st * SO_L2 + j];
            for (i, a) in a1.iter().enumerate() {
                x += row[i] * a;
            }
            l2[j] = x;
            let c = x.clamp(0.0, 1.0);
            v2[j] = c * c * ACT_SCALE;
        }
        let ow = &self.so_out_w[st * SO_OUT_IN..];
        let mut unit = self.so_out_b[st];
        for (j, v) in v2.iter().enumerate() {
            unit += ow[j] * v;
        }
        for (k, &xv) in xin.iter().enumerate() {
            unit += ow[SO_L2 + k] * xv;
        }
        /* No disc-count or mobility table in this shape. Its
        only side input is the mobility that goes into L1, and the disc
        count is what the patterns already cover. Keeping ours would have
        made this a different model with a head start. */
        let out = unit * SO_SCORE;
        let err_discs = out - target;
        /* The gradient of `mse_loss`, which is `2*(pred - target)` on the
        reference's /64 scale. Adam is invariant to a constant on the
        gradient, but weight decay is not -- it is applied outside the
        moment normalisation, so dropping the 2 halves decay's weight
        relative to the loss. */
        let err = 2.0 * err_discs / SO_SCORE;

        // ---- backward ----
        sink.so_out_b[st] += err;
        let mut dxin = [0.0f32; SO_SKIP];
        let so_off = st * SO_OUT_IN;
        let mut dv2 = [0.0f32; SO_L2];
        for j in 0..SO_L2 {
            sink.so_out_w[so_off + j] += err * v2[j];
            dv2[j] = err * ow[j];
        }
        for (k, &xv) in xin.iter().enumerate() {
            sink.so_out_w[so_off + SO_L2 + k] += err * xv;
            dxin[k] += err * ow[SO_L2 + k];
        }
        let mut da1 = [0.0f32; SO_L1 * 2];
        for j in 0..SO_L2 {
            // d(clamp(x,0,1)²)/dx = 2·clamp(x,0,1), zero outside the clamp.
            let dl2 = if l2[j] > 0.0 && l2[j] < 1.0 {
                dv2[j] * 2.0 * l2[j] * ACT_SCALE
            } else {
                0.0
            };
            if dl2 == 0.0 {
                continue;
            }
            sink.so_l2_b[st * SO_L2 + j] += dl2;
            let woff = (st * SO_L2 + j) * (SO_L1 * 2);
            let row = &self.so_l2_w[woff..woff + SO_L1 * 2];
            for i in 0..SO_L1 * 2 {
                sink.so_l2_w[woff + i] += dl2 * a1[i];
                da1[i] += dl2 * row[i];
            }
        }
        for i in 0..SO_L1 {
            let sq = l1[i] * l1[i] * ACT_SCALE;
            let mut dl1 = 0.0;
            if sq > 0.0 && sq < 1.0 {
                dl1 += da1[i] * 2.0 * l1[i] * ACT_SCALE;
            }
            if l1[i] > 0.0 && l1[i] < 1.0 {
                dl1 += da1[SO_L1 + i];
            }
            if dl1 == 0.0 {
                continue;
            }
            sink.so_l1_b[st * SO_L1 + i] += dl1;
            let woff = (st * SO_L1 + i) * SO_L1_IN;
            let row = &self.so_l1_w[woff..woff + SO_L1_IN];
            for (k, &xv) in xin.iter().enumerate() {
                sink.so_l1_w[woff + k] += dl1 * xv;
                dxin[k] += dl1 * row[k];
            }
            sink.so_l1_w[woff + SO_SKIP] += dl1 * m;
        }

        /* Split the stack's input gradient back into its two sources. The
        accumulator half goes through the clamp; the phase-adaptive half
        goes through that layer's squared clipped activation, whose input
        `pa_z` the caller kept for exactly this. */
        let mut dacc = [0.0f32; H];
        for (h, d) in dacc.iter_mut().enumerate() {
            *d = dxin[h] * so_act_grad(acc[h]);
        }
        let mut dpa = [0.0f32; PA_DIMS];
        for (j, d) in dpa.iter_mut().enumerate() {
            let z = pa_z[j];
            if z > 0.0 && z < 1.0 {
                *d = dxin[H + j] * 2.0 * z * ACT_SCALE;
            }
        }

        // The bias is on the raw lanes now, so it takes the same gradient
        // the rows do, and there is one set of it rather than one per stage.
        let draw = fold_pairs_back(raw, &dacc);
        for h in 0..ACC_DIMS {
            sink.ft_bias[h] += draw[h];
        }
        #[cfg(feature = "pa128")]
        {
            let off = pa_bucket(stage) * PA_DIMS;
            for j in 0..PA_DIMS {
                sink.pa_bias[off + j] += dpa[j];
            }
        }
        // The caller owns the feature list, so it pushes the rows; hand the
        // raw-lane gradients back for it to use.
        (err_discs * err_discs, draw, dpa)
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
        /* Bias correction, which AdamW calls for and this did not apply.
        Without it the first steps are damped by roughly `1 - beta^t` -- a
        factor of ten on step one and still 3% at step 100 -- so a run that
        looks like it needs a smaller learning rate is really running a
        different optimizer. */
        adam.t = adam.t.saturating_add(1);
        let t_now = adam.t;
        let bc1 = 1.0 - b1.powi(adam.t as i32);
        let bc2s = (1.0 - b2.powi(adam.t as i32)).sqrt();
        let legacy = adam.legacy_optimizer;
        let step = |m: &mut f32, v: &mut f32, g: f32, w: &mut f32, lr: f32, decay: f32| {
            if legacy && g == 0.0 {
                return;
            }
            *m = b1 * *m + (1.0 - b1) * g;
            *v = b2 * *v + (1.0 - b2) * g * g;
            if legacy {
                *w -= lr * *m / ((*v).sqrt() + eps) + decay * lr * *w;
                return;
            }
            // Decoupled decay first, then the corrected step -- the order
            // AdamW specifies.
            *w = *w * (1.0 - decay * lr) - (lr / bc1) * *m / ((*v).sqrt() / bc2s + eps);
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
            last: adam.ft_last.as_mut_ptr(),
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
                                    let base = r * ACC_DIMS;
                                    if *cells.stamp.add(r) != cur {
                                        *cells.stamp.add(r) = cur;
                                        tv.push(row);
                                        std::ptr::copy_nonoverlapping(
                                            vals.as_ptr(),
                                            cells.scratch.add(base),
                                            ACC_DIMS,
                                        );
                                    } else {
                                        for h in 0..ACC_DIMS {
                                            *cells.scratch.add(base + h) += vals[h];
                                        }
                                    }
                                }
                            }
                            for &row in tv.iter() {
                                let base = row as usize * ACC_DIMS;
                                for h in 0..ACC_DIMS {
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
        /* The phase-adaptive rows, coalesced the same way. Its own stamp
        array and scratch: the row numbers index a different table, so they
        would collide with the base layer's. */
        #[allow(unused_mut)]
        let mut pa_sq = 0.0f64;
        #[cfg(feature = "pa128")]
        {
            while adam.pa_touched.len() < parts {
                adam.pa_touched.push(Vec::new());
            }
            let cells = FtCells {
                scratch: adam.pa_scratch.as_mut_ptr(),
                stamp: adam.pa_stamp.as_mut_ptr(),
                m: adam.m_pa.as_mut_ptr(),
                v: adam.v_pa.as_mut_ptr(),
                w: self.pa.as_mut_ptr(),
                last: adam.pa_last.as_mut_ptr(),
            };
            let sinks_ref = &*sinks;
            let touched = &mut adam.pa_touched[..parts];
            std::thread::scope(|scope| {
                let mut handles = Vec::new();
                for (p, tv) in touched.iter_mut().enumerate() {
                    handles.push(scope.spawn(move || {
                        #[allow(clippy::redundant_locals)]
                        let cells = cells;
                        tv.clear();
                        let mut sq = 0.0f64;
                        // SAFETY: as for the base layer -- rows in bucket `p`
                        // satisfy `row % parts == p`, so the cells one thread
                        // touches are disjoint from every other thread's.
                        unsafe {
                            for s in sinks_ref.iter() {
                                for &(row, vals) in s.pa_rows[p].iter() {
                                    let r = row as usize;
                                    let base = r * PA_DIMS;
                                    if *cells.stamp.add(r) != cur {
                                        *cells.stamp.add(r) = cur;
                                        tv.push(row);
                                        std::ptr::copy_nonoverlapping(
                                            vals.as_ptr(),
                                            cells.scratch.add(base),
                                            PA_DIMS,
                                        );
                                    } else {
                                        for j in 0..PA_DIMS {
                                            *cells.scratch.add(base + j) += vals[j];
                                        }
                                    }
                                }
                            }
                            for &row in tv.iter() {
                                let base = row as usize * PA_DIMS;
                                for j in 0..PA_DIMS {
                                    let g = *cells.scratch.add(base + j) * scale;
                                    sq += (g as f64) * (g as f64);
                                }
                            }
                        }
                        sq
                    }));
                }
                for h in handles {
                    pa_sq += h.join().unwrap();
                }
            });
        }
        let mut sq = ft_sq + pa_sq;
        for sel in 0..13usize {
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
                10 => self.mlp_out_w.len(),
                11 => self.mlp_mob_w.len(),
                _ => self.pa_bias.len(),
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
                        10 => s.mlp_out_w[i],
                        11 => s.mlp_mob_w[i],
                        _ => s.pa_bias[i],
                    })
                    .sum::<f32>()
                    * scale;
                sq += (g as f64) * (g as f64);
            }
        }
        /* The stacked read-out's tables belong in the norm too. Left out,
        the clip fires at the wrong threshold -- the
        first step matched and the second did not, because that was the step
        where the norm first crossed one. */
        #[cfg(feature = "stackedout")]
        for sel in 0..6usize {
            let len = match sel {
                0 => self.so_l1_w.len(),
                1 => self.so_l1_b.len(),
                2 => self.so_l2_w.len(),
                3 => self.so_l2_b.len(),
                4 => self.so_out_w.len(),
                _ => self.so_out_b.len(),
            };
            for i in 0..len {
                let g: f32 = sinks
                    .iter()
                    .map(|s| match sel {
                        0 => s.so_l1_w[i],
                        1 => s.so_l1_b[i],
                        2 => s.so_l2_w[i],
                        3 => s.so_l2_b[i],
                        4 => s.so_out_w[i],
                        _ => s.so_out_b[i],
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

        /* Dense small tables: every cell steps, not only the ones with a
        gradient this batch. A dense optimizer moves a zero-gradient cell too
        -- decay shrinks it and whatever momentum it carries keeps pushing --
        and skipping that left the rarely-touched tables (the disc-count and
        mobility ones especially) never decaying at all. */
        for i in 0..self.out_w.len() {
            let g: f32 = sinks.iter().map(|s| s.out_w[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_out_w[i],
                &mut adam.v_out_w[i],
                g,
                &mut self.out_w[i],
                lr,
                wd,
            );
        }
        for i in 0..self.out_b.len() {
            let g: f32 = sinks.iter().map(|s| s.out_b[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_out_b[i],
                &mut adam.v_out_b[i],
                g,
                &mut self.out_b[i],
                lr,
                0.0,
            );
        }
        for i in 0..self.num_w.len() {
            let g: f32 = sinks.iter().map(|s| s.num_w[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_num_w[i],
                &mut adam.v_num_w[i],
                g,
                &mut self.num_w[i],
                lr,
                0.0,
            );
        }
        // The head's tables: weight decay applies to the 2-D ones only, as
        // it does for the transformer and read-out.
        for i in 0..self.mlp_l1_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_l1_w[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_mlp_l1_w[i],
                &mut adam.v_mlp_l1_w[i],
                g,
                &mut self.mlp_l1_w[i],
                lr,
                wd,
            );
        }
        for i in 0..self.mlp_l1_b.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_l1_b[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_mlp_l1_b[i],
                &mut adam.v_mlp_l1_b[i],
                g,
                &mut self.mlp_l1_b[i],
                lr,
                0.0,
            );
        }
        for i in 0..self.mlp_l2_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_l2_w[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_mlp_l2_w[i],
                &mut adam.v_mlp_l2_w[i],
                g,
                &mut self.mlp_l2_w[i],
                lr,
                wd,
            );
        }
        for i in 0..self.mlp_l2_b.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_l2_b[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_mlp_l2_b[i],
                &mut adam.v_mlp_l2_b[i],
                g,
                &mut self.mlp_l2_b[i],
                lr,
                0.0,
            );
        }
        for i in 0..self.mlp_out_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_out_w[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_mlp_out_w[i],
                &mut adam.v_mlp_out_w[i],
                g,
                &mut self.mlp_out_w[i],
                lr,
                wd,
            );
        }
        /* The stacked read-out. Weights on the two hidden layers are clipped
        to the range quantisation can represent, which this shape
        does (127/64) -- without it a weight can drift somewhere int8 cannot
        follow and the quantised model diverges from the trained one. */
        #[cfg(feature = "stackedout")]
        {
            const SO_MAX_W: f32 = 127.0 / 64.0;
            /* Two separate flags. Decay follows the usual rule --
            every 2-D parameter, biases and 1-D ones exempt -- while the
            clip is only on the two layers it quantises to int8. Sharing one
            flag left the output layer's weights undecayed. */
            let mut tables: [(&mut Vec<f32>, bool, bool); 6] = [
                (&mut self.so_l1_w, true, true),
                (&mut self.so_l1_b, false, false),
                (&mut self.so_l2_w, true, true),
                (&mut self.so_l2_b, false, false),
                (&mut self.so_out_w, false, true),
                (&mut self.so_out_b, false, false),
            ];
            for (t, (w, clip, decay)) in tables.iter_mut().enumerate() {
                for i in 0..w.len() {
                    let g: f32 = sinks
                        .iter()
                        .map(|s| match t {
                            0 => s.so_l1_w[i],
                            1 => s.so_l1_b[i],
                            2 => s.so_l2_w[i],
                            3 => s.so_l2_b[i],
                            4 => s.so_out_w[i],
                            _ => s.so_out_b[i],
                        })
                        .sum::<f32>()
                        * scale;
                    /* No `if g == 0` skip. A dense optimizer moves a cell
                    with zero gradient too -- the decay shrinks it and any
                    momentum it still carries keeps pushing -- and these
                    tables are small enough to walk in full. */
                    step(
                        &mut adam.m_so[t][i],
                        &mut adam.v_so[t][i],
                        g,
                        &mut w[i],
                        lr,
                        if *decay { wd } else { 0.0 },
                    );
                    if *clip {
                        w[i] = w[i].clamp(-SO_MAX_W, SO_MAX_W);
                    }
                }
            }
        }

        for i in 0..self.mlp_mob_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mlp_mob_w[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_mlp_mob_w[i],
                &mut adam.v_mlp_mob_w[i],
                g,
                &mut self.mlp_mob_w[i],
                lr,
                wd,
            );
        }
        for i in 0..self.mob_w.len() {
            let g: f32 = sinks.iter().map(|s| s.mob_w[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_mob_w[i],
                &mut adam.v_mob_w[i],
                g,
                &mut self.mob_w[i],
                lr,
                0.0,
            );
        }
        for i in 0..self.ft_bias.len() {
            let g: f32 = sinks.iter().map(|s| s.ft_bias[i]).sum::<f32>() * scale;
            // Dense on both sides, so no zero-gradient skip -- see the note
            // on the stacked read-out's tables.
            let m = &mut adam.m_ft_bias[i];
            let v = &mut adam.v_ft_bias[i];
            *m = b1 * *m + (1.0 - b1) * g;
            *v = b2 * *v + (1.0 - b2) * g * g;
            let nb = if legacy {
                self.ft_bias[i] - lr * *m / ((*v).sqrt() + eps)
            } else {
                self.ft_bias[i] - (lr / bc1) * *m / ((*v).sqrt() / bc2s + eps)
            };
            #[cfg(feature = "stackedout")]
            {
                self.ft_bias[i] = nb;
            }
            #[cfg(not(feature = "stackedout"))]
            {
                self.ft_bias[i] = nb.clamp(-FT_CLAMP, FT_CLAMP);
            }
        }
        for i in 0..self.pw.len() {
            let g: f32 = sinks.iter().map(|s| s.pw[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_pw[i],
                &mut adam.v_pw[i],
                g,
                &mut self.pw[i],
                lr,
                wd,
            );
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
                                let base = row as usize * ACC_DIMS;
                                let lp = cells.last.add(row as usize);
                                if !legacy {
                                    catch_up(
                                        cells.m.add(base),
                                        cells.v.add(base),
                                        cells.w.add(base),
                                        ACC_DIMS,
                                        *lp,
                                        t_now,
                                        lr,
                                        wd,
                                        b1,
                                        b2,
                                        eps,
                                    );
                                }
                                *lp = t_now;
                                for h in 0..ACC_DIMS {
                                    /* No zero-gradient skip. A lane whose
                                    activation sat outside its clamp gets a
                                    zero gradient, and skipping it there
                                    means it never decays -- exactly one
                                    step's worth of decay short, which is
                                    what a step-by-step check found. The
                                    row is already being walked; the branch
                                    saved nothing. */
                                    let gh = *cells.scratch.add(base + h) * scale;
                                    if !(legacy && gh == 0.0) {
                                        let m = cells.m.add(base + h);
                                        let v = cells.v.add(base + h);
                                        *m = b1 * *m + (1.0 - b1) * gh;
                                        *v = b2 * *v + (1.0 - b2) * gh * gh;
                                        let w = cells.w.add(base + h);
                                        let nw = if legacy {
                                            *w - lr * *m / ((*v).sqrt() + eps) - wd * lr * *w
                                        } else {
                                            *w * (1.0 - wd * lr)
                                                - (lr / bc1) * *m / ((*v).sqrt() / bc2s + eps)
                                        };
                                        // Nothing bounds this
                                        // layer; the bound exists to protect
                                        // an int16 accumulator's resolution,
                                        // and here it is read in f32.
                                        #[cfg(feature = "stackedout")]
                                        {
                                            *w = nw;
                                        }
                                        #[cfg(not(feature = "stackedout"))]
                                        {
                                            *w = nw.clamp(-FT_CLAMP, FT_CLAMP);
                                        }
                                    }
                                }
                            }
                        }
                    });
                }
            });
        }
        // The phase-adaptive rows take the same step. No `FT_CLAMP`: that
        // bound exists so the base layer's int16 accumulator keeps its
        // resolution, and this layer is read in f32.
        #[cfg(feature = "pa128")]
        {
            let cells = FtCells {
                scratch: adam.pa_scratch.as_mut_ptr(),
                stamp: adam.pa_stamp.as_mut_ptr(),
                m: adam.m_pa.as_mut_ptr(),
                v: adam.v_pa.as_mut_ptr(),
                w: self.pa.as_mut_ptr(),
                last: adam.pa_last.as_mut_ptr(),
            };
            let touched = &adam.pa_touched[..parts];
            std::thread::scope(|scope| {
                for tv in touched.iter() {
                    scope.spawn(move || {
                        #[allow(clippy::redundant_locals)]
                        let cells = cells;
                        // SAFETY: as in pass 1 -- disjoint cells per bucket.
                        unsafe {
                            for &row in tv.iter() {
                                let base = row as usize * PA_DIMS;
                                let lp = cells.last.add(row as usize);
                                if !legacy {
                                    catch_up(
                                        cells.m.add(base),
                                        cells.v.add(base),
                                        cells.w.add(base),
                                        PA_DIMS,
                                        *lp,
                                        t_now,
                                        lr,
                                        wd,
                                        b1,
                                        b2,
                                        eps,
                                    );
                                }
                                *lp = t_now;
                                for j in 0..PA_DIMS {
                                    // As above: no skip, or the cell misses
                                    // its decay.
                                    let gj = *cells.scratch.add(base + j) * scale;
                                    if !(legacy && gj == 0.0) {
                                        let m = cells.m.add(base + j);
                                        let v = cells.v.add(base + j);
                                        *m = b1 * *m + (1.0 - b1) * gj;
                                        *v = b2 * *v + (1.0 - b2) * gj * gj;
                                        let w = cells.w.add(base + j);
                                        *w = if legacy {
                                            *w - lr * *m / ((*v).sqrt() + eps) - wd * lr * *w
                                        } else {
                                            *w * (1.0 - wd * lr)
                                                - (lr / bc1) * *m / ((*v).sqrt() / bc2s + eps)
                                        };
                                    }
                                }
                            }
                        }
                    });
                }
            });
        }
        for i in 0..self.pa_bias.len() {
            let g: f32 = sinks.iter().map(|s| s.pa_bias[i]).sum::<f32>() * scale;
            step(
                &mut adam.m_pa_bias[i],
                &mut adam.v_pa_bias[i],
                g,
                &mut self.pa_bias[i],
                lr,
                0.0,
            );
        }
        for s in sinks.iter_mut() {
            for b in s.ft_rows.iter_mut() {
                b.clear();
            }
            for b in s.pa_rows.iter_mut() {
                b.clear();
            }
        }
        self.lookahead_sync(adam);
    }

    /// Load every table from a flat f32 dump, in the order
    /// `tmp/table_dump_check.py` writes them.
    ///
    /// Exists so the model can be checked against independent numbers
    /// rather than against a reading of its source. Not a save format --
    /// there is no header and no versioning, on purpose: it is only ever
    /// written and read by that one script and this one function.
    #[cfg(feature = "stackedout")]
    pub fn load_reference_tables(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Read;
        let mut r = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut read = |dst: &mut [f32]| -> std::io::Result<()> {
            let mut b = [0u8; 4];
            for x in dst.iter_mut() {
                r.read_exact(&mut b)?;
                *x = f32::from_le_bytes(b);
            }
            Ok(())
        };
        read(&mut self.ft)?;
        read(&mut self.ft_bias)?;
        read(&mut self.pa)?;
        read(&mut self.pa_bias)?;
        read(&mut self.so_l1_w)?;
        read(&mut self.so_l1_b)?;
        read(&mut self.so_l2_w)?;
        read(&mut self.so_l2_b)?;
        read(&mut self.so_out_w)?;
        read(&mut self.so_out_b)?;
        let mut extra = [0u8; 1];
        if r.read(&mut extra)? != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "reference dump is longer than this build's tables",
            ));
        }
        Ok(())
    }

    /// Bring every row up to date before the weights are read.
    ///
    /// The sparse update defers a row's decay until the row is next touched
    /// (see `catch_up`), which is invisible during training and wrong the
    /// moment the weights are saved or evaluated: rows that went quiet
    /// early would keep a value a dense optimizer would have shrunk. Call
    /// this once at the end of training, with the last learning rate used.
    pub fn settle_adam(&mut self, adam: &mut AdamState, lr: f32, wd: f32) {
        if adam.legacy_optimizer {
            return;
        }
        let (b1, b2, eps, t) = (adam.beta1, adam.beta2, adam.eps, adam.t);
        for r in 0..adam.ft_last.len() {
            let base = r * ACC_DIMS;
            // SAFETY: `base + ACC_DIMS` is in bounds for all three tables,
            // which are sized from the same row count.
            unsafe {
                catch_up(
                    adam.m_ft.as_mut_ptr().add(base),
                    adam.v_ft.as_mut_ptr().add(base),
                    self.ft.as_mut_ptr().add(base),
                    ACC_DIMS,
                    adam.ft_last[r],
                    t + 1,
                    lr,
                    wd,
                    b1,
                    b2,
                    eps,
                );
            }
            adam.ft_last[r] = t;
        }
        for r in 0..adam.pa_last.len() {
            let base = r * PA_DIMS;
            // SAFETY: as above, for the phase-adaptive tables.
            unsafe {
                catch_up(
                    adam.m_pa.as_mut_ptr().add(base),
                    adam.v_pa.as_mut_ptr().add(base),
                    self.pa.as_mut_ptr().add(base),
                    PA_DIMS,
                    adam.pa_last[r],
                    t + 1,
                    lr,
                    wd,
                    b1,
                    b2,
                    eps,
                );
            }
            adam.pa_last[r] = t;
        }
    }

    /// Every table, in the order the check script writes
    /// them. Read-only counterpart to `load_reference_tables`.
    #[cfg(feature = "stackedout")]
    pub fn reference_tables(&self) -> Vec<&[f32]> {
        vec![
            &self.ft,
            &self.ft_bias,
            &self.pa,
            &self.pa_bias,
            &self.so_l1_w,
            &self.so_l1_b,
            &self.so_l2_w,
            &self.so_l2_b,
            &self.so_out_w,
            &self.so_out_b,
        ]
    }

    /// Every trainable table, in one list.
    ///
    /// Lookahead has to touch all of them or it smooths some parameters and
    /// not others; enumerating them here means adding a table cannot quietly
    /// leave it out.
    fn trainable_tables(&mut self) -> Vec<&mut [f32]> {
        vec![
            &mut self.ft,
            &mut self.ft_bias,
            &mut self.pa,
            &mut self.pa_bias,
            &mut self.out_w,
            &mut self.out_b,
            &mut self.num_w,
            &mut self.mob_w,
            &mut self.pw,
            &mut self.mlp_l1_w,
            &mut self.mlp_l1_b,
            &mut self.mlp_l2_w,
            &mut self.mlp_l2_b,
            &mut self.mlp_out_w,
            &mut self.mlp_mob_w,
            &mut self.so_l1_w,
            &mut self.so_l1_b,
            &mut self.so_l2_w,
            &mut self.so_l2_b,
            &mut self.so_out_w,
            &mut self.so_out_b,
        ]
    }

    /// One Lookahead sync, if this step is one: `slow += alpha*(fast - slow)`
    /// then `fast = slow`.
    fn lookahead_sync(&mut self, adam: &mut AdamState) {
        if adam.la_k == 0 {
            return;
        }
        adam.la_step = adam.la_step.wrapping_add(1);
        if !adam.la_step.is_multiple_of(adam.la_k) {
            return;
        }
        let alpha = adam.la_alpha;
        let mut tables = self.trainable_tables();
        if adam.la_slow.len() != tables.len() {
            adam.la_slow = tables.iter().map(|t| t.to_vec()).collect();
            return;
        }
        for (t, slow) in tables.iter_mut().zip(adam.la_slow.iter_mut()) {
            for (w, sw) in t.iter_mut().zip(slow.iter_mut()) {
                *sw += alpha * (*w - *sw);
                *w = *sw;
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
        /* Diagnostic paths, and the bias they want does not exist with the
        stacked read-out: there it sits on the raw lanes and is not indexed
        by stage. Leaving it out beats indexing past the end of the table. */
        #[cfg(not(feature = "stackedout"))]
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

    #[cfg_attr(feature = "stackedout", allow(clippy::needless_return))]
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
        let mut v = [0i16; ACC_DIMS];
        v.copy_from_slice(if board.player() == Color::Black {
            &acc.acc[0..ACC_DIMS]
        } else {
            &acc.acc[ACC_DIMS..H2]
        });
        #[cfg(feature = "stackedout")]
        {
            let inv = 1.0 / self.ft_scale;
            let mut raw = [0.0f32; ACC_DIMS];
            for i in 0..ACC_DIMS {
                raw[i] = v[i] as f32 * inv + self.ft_bias[i];
            }
            let feats = self.features_player(&acc.indices, board.player(), stage);
            let (pa, _) = self.pa_forward(&feats, stage);
            let folded = fold_pairs_f32(&raw);
            return self.stacked_readout(&folded, &pa, Self::mob_index(board), stage);
        }
        #[cfg(not(feature = "stackedout"))]
        {
            // The pairwise fold, which `eval_from_indices` also applies.
            // Missing here, this path read the first half of the accumulator
            // raw and called it the activation.
            let v = fold_pairs(&v, self.pair_clamp_q, self.pair_shift_q);
            let fb = &self.ft_bias_i16[stage * H..stage * H + H];
            let ow = &self.out_w_i16[stage * H..stage * H + H];
            // out = bias + scale * sum phi(acc+fb) * out_w + disc term
            let sum = readout_dot(&v, fb, ow, self.act_clamp_q, self.act_shift_q);
            self.out_b[stage]
                + sum as f32 * self.out_scale
                + self.num_term(board, stage)
                + self.mob_term(board, stage)
                + self.extras(&v, fb, Self::mob_index(board), stage)
        }
    }

    /// i32-precision read-out (finer quantization than i16).
    #[inline]
    /* Bench-only precision comparison, and it indexes the bias per
    stage -- which the stacked read-out does not have. */
    #[cfg(not(feature = "stackedout"))]
    pub fn eval_acc_i32(&self, acc: &Accumulator32, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black {
            &acc.acc[0..ACC_DIMS]
        } else {
            &acc.acc[ACC_DIMS..H2]
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
    /* Bench-only precision comparison, and it indexes the bias per
    stage -- which the stacked read-out does not have. */
    #[cfg(not(feature = "stackedout"))]
    pub fn eval_acc_f32(&self, acc: &AccumulatorF, board: &Board) -> f32 {
        let stage = crate::evaluator::Evaluator::stage(board);
        let v = if board.player() == Color::Black {
            &acc.acc[0..ACC_DIMS]
        } else {
            &acc.acc[ACC_DIMS..H2]
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
        /* Diagnostic paths, and the bias they want does not exist with the
        stacked read-out: there it sits on the raw lanes and is not indexed
        by stage. Leaving it out beats indexing past the end of the table. */
        #[cfg(not(feature = "stackedout"))]
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
            /* Format 07 = 06 + the head's mobility input. Older formats
            still load (missing tables zeroed / replicated), so their
            evaluations are unchanged -- a zero head is exactly "no head",
            and a zero mobility weight is "mobility only reaches the score
            through `mob_w`", which is what 06 did.

            **A 06 file's read-out means something different now.** The
            activation changed from `max(0,x)` to `clamp(x,0,C)²/C`, so an
            old model loads without error and evaluates wrongly. Retrain
            rather than convert; the weights were fitted to the other
            curve. */
            w.write_all(b"BBRVNN09")?;
            // The accumulator width, which is what sizes `ft` -- twice the
            // model's working width under `pairmul`. A file written by one
            // build is rejected by the other on this field.
            w.write_all(&(ACC_DIMS as u32).to_le_bytes())?;
            w.write_all(&(self.n_features as u32).to_le_bytes())?;
            w.write_all(&(STAGE_COUNT as u32).to_le_bytes())?;
            /* Length of the stacked read-out's tables, zero when the build
            has no stack. It is written rather than derived so that a build
            without the stack refuses a file with one, instead of reading
            the shared tables and silently ignoring the layers that carry
            most of the model. */
            // Same idea as `so_len` below: written, not derived, so a build
            // without the layer refuses a file that has one.
            w.write_all(&(self.pa.len() as u32).to_le_bytes())?;
            let so_len = self.so_l1_w.len()
                + self.so_l1_b.len()
                + self.so_l2_w.len()
                + self.so_l2_b.len()
                + self.so_out_w.len()
                + self.so_out_b.len();
            w.write_all(&(so_len as u32).to_le_bytes())?;
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
                .chain(&self.mlp_mob_w)
                .chain(&self.so_l1_w)
                .chain(&self.so_l1_b)
                .chain(&self.so_l2_w)
                .chain(&self.so_l2_b)
                .chain(&self.so_out_w)
                .chain(&self.so_out_b)
                .chain(&self.pa)
                .chain(&self.pa_bias)
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
        let (staged_bias, has_num, has_pw, has_mob, has_mlp, has_mlp_mob) = match &magic {
            b"BBRVNN09" | b"BBRVNN08" | b"BBRVNN07" => (true, true, true, true, true, true),
            b"BBRVNN06" => (true, true, true, true, true, false),
            b"BBRVNN05" => (true, true, true, true, false, false),
            b"BBRVNN04" => (true, true, true, false, false, false),
            b"BBRVNN03" => (true, true, false, false, false, false),
            b"BBRVNN02" => (false, true, false, false, false, false),
            b"BBRVNN01" => (false, false, false, false, false, false),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "bad nnue magic",
                ))
            }
        };
        let mut u = [0u8; 4];
        r.read_exact(&mut u)?;
        if u32::from_le_bytes(u) as usize != ACC_DIMS {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "accumulator width mismatch",
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
        let pa_len = if &magic == b"BBRVNN09" {
            r.read_exact(&mut u)?;
            u32::from_le_bytes(u) as usize
        } else {
            0
        };
        if pa_len != self.pa.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "phase-adaptive layer mismatch: this build and the file disagree on whether \
                 the accumulator has a phase-adaptive half",
            ));
        }
        let so_len = if &magic == b"BBRVNN09" || &magic == b"BBRVNN08" {
            r.read_exact(&mut u)?;
            u32::from_le_bytes(u) as usize
        } else {
            0
        };
        let want_so = self.so_l1_w.len()
            + self.so_l1_b.len()
            + self.so_l2_w.len()
            + self.so_l2_b.len()
            + self.so_out_w.len()
            + self.so_out_b.len();
        if so_len != want_so {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stacked read-out mismatch: this build and the file disagree on whether the \
                 score comes from the layer stack or from the shared read-out",
            ));
        }
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
        self.mlp_mob_w.fill(0.0);
        if has_mlp_mob {
            read_into(&mut r, &mut self.mlp_mob_w)?;
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
        if so_len > 0 {
            read_into(&mut r, &mut self.so_l1_w)?;
            read_into(&mut r, &mut self.so_l1_b)?;
            read_into(&mut r, &mut self.so_l2_w)?;
            read_into(&mut r, &mut self.so_l2_b)?;
            read_into(&mut r, &mut self.so_out_w)?;
            read_into(&mut r, &mut self.so_out_b)?;
        }
        if pa_len > 0 {
            read_into(&mut r, &mut self.pa)?;
            read_into(&mut r, &mut self.pa_bias)?;
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
        /* Zero without the stacked read-out: there the read-out is linear
        in the accumulator, so quantization error is bounded by the disc
        floor alone and any drift is a real disagreement. */
        #[cfg(feature = "stackedout")]
        const QUANT_TOL_REL: f32 = 1e-3;
        #[cfg(not(feature = "stackedout"))]
        const QUANT_TOL_REL: f32 = 0.0;

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

            /* The gap here is quantization, not a path disagreement: the
            two accumulators are bit-identical integers, and only the
            read-out differs (f32 reference against i16). One disc is the
            right floor, but the stacked read-out ends in a x64 scale and
            two squarings, so its quantization error grows with the score
            instead of staying flat -- hence the relative term. */
            let tol = 1.0 + inc.abs().max(scratch.abs()) * QUANT_TOL_REL;
            assert!(
                (inc - scratch).abs() < tol,
                "incremental {inc} vs scratch {scratch} (player {:?}, {} empty)",
                board.player(),
                board.empty_count()
            );
            assert!(
                (from_ix - inc).abs() < 1e-3 * from_ix.abs().max(1.0),
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

        let evaluated = {
            let f = nn.forward(&nn.features_black(&ix, stage), mob, stage);
            #[cfg(feature = "stackedout")]
            {
                f
            }
            #[cfg(not(feature = "stackedout"))]
            {
                f + nn.num_w[stage * NUM_TABLE_SIZE + discs] + nn.mob_w[stage * MOB_BUCKETS + mob]
            }
        };

        // `grad_black_into` returns (out - target)^2, which hides the sign.
        // Two targets recover `out` itself: sq(0) - sq(2) = 4·out - 4.
        let mut sink = GradSink::new(1);
        let sq0 = nn.grad_black_into(&ix, stage, discs, mob, 0.0, &mut sink);
        let sq2 = nn.grad_black_into(&ix, stage, discs, mob, 2.0, &mut sink);
        let trained = (sq0 - sq2 + 4.0) / 4.0;

        // Relative, not absolute: the stacked read-out works on [0,1] and
        // multiplies by 64 at the end, so a score is two orders of magnitude
        // larger there than in the linear shape and f32 rounding scales with
        // it. A function actually differing shows up far above this.
        assert!(
            (trained - evaluated).abs() < 1e-3 * evaluated.abs().max(1.0),
            "training forward {trained} vs evaluated {evaluated}: the trainer \
             is descending on a different function than the search reads"
        );
    }

    /* The hand-written backward pass against finite differences.

    `training_forward_matches_eval` shows the two forward paths agree; it
    says nothing about the gradient, and a wrong gradient does not crash --
    it descends on something else and looks like a model that trains badly.
    An autograd gradient offers nothing to
    read across; the check has to be numeric.

    The loss is `((out - target)/SO_SCORE)^2`, which is what `grad_stacked`
    accumulates into the sink -- the squared error on the /64
    scale. */
    #[cfg(feature = "stackedout")]
    #[test]
    fn stacked_gradient_matches_finite_differences() {
        let mut nn = Nnue::new(EGAROUCID_PATTERNS);
        nn.init_weights();
        let mut s: u64 = 0x0BAD_C0DE_1234_5678;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 - 8.388_608e6) / 8.388_608e6
        };
        // Random, and large enough that the clamps are exercised on both
        // sides: a gradient that is right only where nothing saturates is
        // not right.
        for v in nn.ft.iter_mut() {
            *v = rnd() * 0.3;
        }
        for v in nn.ft_bias.iter_mut() {
            *v = rnd() * 0.3;
        }
        for v in nn.pa.iter_mut() {
            *v = rnd() * 0.3;
        }
        for v in nn.pa_bias.iter_mut() {
            *v = rnd() * 0.3;
        }
        for v in nn.so_l1_w.iter_mut() {
            *v = rnd() * 0.5;
        }
        for v in nn.so_l1_b.iter_mut() {
            *v = rnd() * 0.5;
        }
        for v in nn.so_l2_w.iter_mut() {
            *v = rnd() * 0.5;
        }
        for v in nn.so_l2_b.iter_mut() {
            *v = rnd() * 0.5;
        }
        for v in nn.so_out_w.iter_mut() {
            *v = rnd() * 0.5;
        }
        for v in nn.so_out_b.iter_mut() {
            *v = rnd() * 0.5;
        }

        let mut board = Board::new();
        for _ in 0..17 {
            let m = board.movable();
            if m == 0 {
                board.pass();
                continue;
            }
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            board.make_move(pos).unwrap();
        }
        let ix = nn.indices(board.black, board.white);
        let stage = crate::evaluator::Evaluator::stage(&board);
        let discs = board.black.count_ones() as usize;
        let mob = Nnue::mob_index(&board);
        let target = 7.0f32;

        let mut sink = GradSink::new(1);
        nn.grad_black_into(&ix, stage, discs, mob, target, &mut sink);

        let feats = nn.features_black(&ix, stage);
        let loss = |nn: &Nnue| -> f32 {
            let out = nn.forward(&feats, mob, stage);
            let e = (out - target) / SO_SCORE;
            e * e
        };

        // (table selector, index, gradient the sink holds)
        let row = feats[0] as usize;
        #[cfg(feature = "pa128")]
        let pa_row = pa_bucket(stage) * nn.n_feat_bucket + row;
        let mut cases: Vec<(&str, usize, f32)> = Vec::new();
        for i in [0usize, 1, 7] {
            cases.push((
                "so_l1_w",
                i,
                sink.so_l1_w[so_stage(stage) * SO_L1 * SO_L1_IN + i],
            ));
            cases.push((
                "so_l2_w",
                i,
                sink.so_l2_w[so_stage(stage) * SO_L2 * SO_L1 * 2 + i],
            ));
            cases.push((
                "so_out_w",
                i,
                sink.so_out_w[so_stage(stage) * SO_OUT_IN + i],
            ));
        }
        cases.push(("so_out_b", so_stage(stage), sink.so_out_b[so_stage(stage)]));
        cases.push((
            "so_l1_b",
            so_stage(stage) * SO_L1,
            sink.so_l1_b[so_stage(stage) * SO_L1],
        ));
        cases.push((
            "so_l2_b",
            so_stage(stage) * SO_L2,
            sink.so_l2_b[so_stage(stage) * SO_L2],
        ));
        for i in [0usize, 3] {
            cases.push(("ft_bias", i, sink.ft_bias[i]));
        }
        let ft_grad: Vec<(usize, f32)> = // One part, so every row lands in bucket zero.
            sink.ft_rows[0]
            .iter()
            .find(|(r, _)| *r as usize == row)
            .map(|(_, v)| vec![(0usize, v[0]), (3usize, v[3])])
            .unwrap_or_default();
        assert!(!ft_grad.is_empty(), "no gradient was pushed for row {row}");
        #[cfg(feature = "pa128")]
        let pa_grad: Vec<(usize, f32)> = sink.pa_rows[0]
            .iter()
            .find(|(r, _)| *r as usize == pa_row)
            .map(|(_, v)| vec![(0usize, v[0]), (5usize, v[5])])
            .unwrap_or_default();

        let mut worst = 0.0f32;
        let mut worst_name = String::new();
        let mut check = |nn: &mut Nnue, name: &str, sel: u8, i: usize, want: f32| {
            let h = 1e-3f32;
            let cell = |nn: &mut Nnue, sel: u8, i: usize| -> *mut f32 {
                match sel {
                    0 => &mut nn.so_l1_w[i],
                    1 => &mut nn.so_l2_w[i],
                    2 => &mut nn.so_out_w[i],
                    3 => &mut nn.so_out_b[i],
                    4 => &mut nn.so_l1_b[i],
                    5 => &mut nn.so_l2_b[i],
                    6 => &mut nn.ft_bias[i],
                    7 => &mut nn.ft[i],
                    _ => &mut nn.pa[i],
                }
            };
            // SAFETY: the pointer is used before any other borrow of `nn`.
            let orig = unsafe { *cell(nn, sel, i) };
            unsafe { *cell(nn, sel, i) = orig + h };
            let up = loss(nn);
            unsafe { *cell(nn, sel, i) = orig - h };
            let down = loss(nn);
            unsafe { *cell(nn, sel, i) = orig };
            let numeric = (up - down) / (2.0 * h);
            /* Absolute floor before the relative comparison. A central
            difference on f32 with h=1e-3 carries a few times 1e-5 of noise,
            so a gradient of 3e-4 can be 3% "wrong" while being exactly
            right -- and a cell that small moves no weight anyway. The
            floor is well under the disagreements this is meant to catch:
            dropping the factor of two in the loss shows up as 0.5. */
            let d = if (numeric - want).abs() < 1e-4 {
                0.0
            } else {
                (numeric - want).abs() / numeric.abs().max(want.abs()).max(1e-4)
            };
            if d > worst {
                worst = d;
                worst_name = format!("{name}[{i}] numeric {numeric:.6} sink {want:.6}");
            }
        };

        for (name, i, g) in cases {
            let sel = match name {
                "so_l1_w" => 0,
                "so_l2_w" => 1,
                "so_out_w" => 2,
                "so_out_b" => 3,
                "so_l1_b" => 4,
                "so_l2_b" => 5,
                _ => 6,
            };
            let idx = match name {
                "so_l1_w" => so_stage(stage) * SO_L1 * SO_L1_IN + i,
                "so_l2_w" => so_stage(stage) * SO_L2 * SO_L1 * 2 + i,
                "so_out_w" => so_stage(stage) * SO_OUT_IN + i,
                _ => i,
            };
            check(&mut nn, name, sel, idx, g);
        }
        for (j, g) in ft_grad {
            check(&mut nn, "ft", 7, row * ACC_DIMS + j, g);
        }
        #[cfg(feature = "pa128")]
        for (j, g) in pa_grad {
            check(&mut nn, "pa", 8, pa_row * PA_DIMS + j, g);
        }

        assert!(
            worst < 0.02,
            "backward disagrees with finite differences by {worst:.4} relative: {worst_name}"
        );
    }
}
