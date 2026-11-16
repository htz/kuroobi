//! The stacked read-out in integers, the form the search runs.

use super::{
    pa_bucket, so_stage, Nnue, ACC_DIMS, H, PA_DIMS, SO_L1, SO_L2, SO_OUT_IN, SO_SKIP, SO_STAGES,
};
use crate::color::Color;
use crate::pattern_index::{PatternIndices, MAX_MASKS};
#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
use std::arch::aarch64::{int16x8_t, int16x8x4_t, int32x4_t, int8x16_t, int8x16x2_t, int8x8_t};

/// Rows are scaled so that `1.0` is this many steps.
const ROW_ONE: i32 = 512;
/// A row lane is clamped at this before the product, one step short of `ROW_ONE` so the product of two clamped lanes fits a byte.
const ROW_CLAMP: i16 = 510;
/// `L1`'s input, padded to whole 16-byte groups plus one extra chunk of four so the mobility lane (index 256) has a home.
const L1_IN: usize = SO_SKIP + 8;
const L1_CHUNKS: usize = L1_IN / 4;
/// `L2`'s input is `L1`'s output twice: squared and plain.
const L2_IN: usize = SO_L1 * 2;
const L2_CHUNKS: usize = L2_IN / 4;
/// The stacked output's input: `L2` then the stack's own inputs.
const OUT_IN: usize = SO_OUT_IN;
/// Steps per disc in the output sum: `2^20 / SO_SCORE`.
const OUT_PER_DISC: f32 = (1u32 << 20) as f32 / super::SO_SCORE;

const _: () = assert!(SO_SKIP == H + PA_DIMS && SO_SKIP.is_multiple_of(16));
const _: () = assert!(
    H == 128 && PA_DIMS == 128,
    "the kernels are written for 128 lanes"
);
const _: () = assert!(SO_L1 == 16 && SO_L2 == 64);
const _: () = assert!(OUT_IN == SO_L2 + SO_SKIP && OUT_IN.is_multiple_of(16));

/// Starts loading every line of row `r` in both tables; Apple's line is 128 B, so a 64 B stride would ask twice.
#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
#[inline(always)]
unsafe fn prefetch_row(ft: *const i16, pa: *const i16, r: u32) {
    let (hp, pp) = (
        ft.add(r as usize * ACC_DIMS) as *const u8,
        pa.add(r as usize * PA_DIMS) as *const u8,
    );
    for l in 0..ACC_DIMS * 2 / 128 {
        crate::prefetch::l1(hp.add(l * 128));
    }
    for l in 0..PA_DIMS * 2 / 128 {
        crate::prefetch::l1(pp.add(l * 128));
    }
}

/// Starts loading every 64 B line of row `r` in both tables.
#[cfg(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    not(feature = "nnue-scalar")
))]
#[inline(always)]
unsafe fn prefetch_row_x86(ft: *const i16, pa: *const i16, r: u32) {
    let (hp, pp) = (
        ft.add(r as usize * ACC_DIMS) as *const u8,
        pa.add(r as usize * PA_DIMS) as *const u8,
    );
    for l in 0..ACC_DIMS * 2 / 64 {
        crate::prefetch::l1(hp.add(l * 64));
    }
    for l in 0..PA_DIMS * 2 / 64 {
        crate::prefetch::l1(pp.add(l * 64));
    }
}

/// Inputs of one layer as 16-bit lanes, aligned for 32 B loads.
#[cfg(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    not(feature = "nnue-scalar")
))]
#[repr(align(32))]
struct Lanes<const N: usize>([i16; N]);

/// Weights packed for `_mm256_madd_epi16`: for input pair `p` and output `o`, `w[o][2p]` and `w[o][2p + 1]` sit at `(p * n_out + o) * 2`.
#[cfg(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    not(feature = "nnue-scalar")
))]
fn pair_pack(packed: &[i8], n_out: usize, n_in: usize) -> Vec<i16> {
    let mut out = vec![0i16; n_out * n_in];
    for o in 0..n_out {
        for k in 0..n_in {
            out[(k / 2 * n_out + o) * 2 + k % 2] = packed[packed_index(o, k, n_out)] as i16;
        }
    }
    out
}

/// The mobility input lane, shifted like the rest of `L1`'s input.
#[inline(always)]
fn mob_byte(mob: usize) -> i8 {
    ((mob as i32 * 7).min(255) - 128) as i8
}

#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
#[repr(align(64))]
struct Buf<const N: usize>([i8; N]);

/// Eight transformer inputs: the clamped low and high sums multiplied, as `x - 128`.
#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
#[inline(always)]
unsafe fn ft_bytes(lo: int16x8_t, hi: int16x8_t) -> int8x8_t {
    use std::arch::aarch64::*;
    let clamp = vdupq_n_s16(ROW_CLAMP);
    let a = vreinterpretq_s16_u16(vqshluq_n_s16::<5>(vminq_s16(lo, clamp)));
    let p = vqmovun_s16(vqdmulhq_s16(a, vminq_s16(hi, clamp)));
    vadd_s8(vreinterpret_s8_u8(p), vdup_n_s8(-128))
}

/// Eight phase inputs: the clamped sum squared, as `x - 128`.
#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
#[inline(always)]
unsafe fn pa_bytes(z: int16x8_t) -> int8x8_t {
    use std::arch::aarch64::*;
    let v = vminq_s16(z, vdupq_n_s16(ROW_CLAMP));
    let p = vqmovun_s16(vqdmulhq_s16(
        vreinterpretq_s16_u16(vqshluq_n_s16::<5>(v)),
        v,
    ));
    vadd_s8(vreinterpret_s8_u8(p), vdup_n_s8(-128))
}

/// `L1`'s sixteen outputs as `L2`'s input: squared, then plain, each as `x - 128`.
#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
#[inline(always)]
unsafe fn l1_bytes(acc: &[int32x4_t; 4]) -> [int8x16_t; 2] {
    use std::arch::aarch64::*;
    let shift = vdupq_n_s8(-128);
    let s0 = vcombine_s16(vqmovn_s32(acc[0]), vqmovn_s32(acc[1]));
    let s1 = vcombine_s16(vqmovn_s32(acc[2]), vqmovn_s32(acc[3]));
    let sq = vcombine_u8(
        vqshrun_n_s16::<5>(vqdmulhq_s16(s0, s0)),
        vqshrun_n_s16::<5>(vqdmulhq_s16(s1, s1)),
    );
    let re = vcombine_u8(vqshrun_n_s16::<6>(s0), vqshrun_n_s16::<6>(s1));
    [
        vaddq_s8(vreinterpretq_s8_u8(sq), shift),
        vaddq_s8(vreinterpretq_s8_u8(re), shift),
    ]
}

/// Thirty-two of the output layer's products, added onto its eight running sums.
#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
#[inline(always)]
unsafe fn out_mla(o: &mut [int32x4_t; 8], wv: int16x8x4_t, xv: int8x16x2_t) {
    use std::arch::aarch64::*;
    let x0 = vmovl_s8(vget_low_s8(xv.0));
    let x1 = vmovl_s8(vget_high_s8(xv.0));
    let x2 = vmovl_s8(vget_low_s8(xv.1));
    let x3 = vmovl_s8(vget_high_s8(xv.1));
    o[0] = vmlal_s16(o[0], vget_low_s16(wv.0), vget_low_s16(x0));
    o[1] = vmlal_s16(o[1], vget_high_s16(wv.0), vget_high_s16(x0));
    o[2] = vmlal_s16(o[2], vget_low_s16(wv.1), vget_low_s16(x1));
    o[3] = vmlal_s16(o[3], vget_high_s16(wv.1), vget_high_s16(x1));
    o[4] = vmlal_s16(o[4], vget_low_s16(wv.2), vget_low_s16(x2));
    o[5] = vmlal_s16(o[5], vget_high_s16(wv.2), vget_high_s16(x2));
    o[6] = vmlal_s16(o[6], vget_low_s16(wv.3), vget_low_s16(x3));
    o[7] = vmlal_s16(o[7], vget_high_s16(wv.3), vget_high_s16(x3));
}

#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
#[inline(always)]
unsafe fn out_total(o: &[int32x4_t; 8]) -> i32 {
    use std::arch::aarch64::*;
    vaddvq_s32(vaddq_s32(
        vaddq_s32(vaddq_s32(o[0], o[1]), vaddq_s32(o[2], o[3])),
        vaddq_s32(vaddq_s32(o[4], o[5]), vaddq_s32(o[6], o[7])),
    ))
}

/// Eight of `L2`'s outputs, clamped and squared, as `x - 128`.
#[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
#[inline(always)]
unsafe fn l2_bytes(a: int32x4_t, b: int32x4_t) -> int8x8_t {
    use std::arch::aarch64::*;
    let (top, zero) = (vdupq_n_s32(255 << 6), vdupq_n_s32(0));
    let v = vcombine_s16(
        vqmovn_s32(vmaxq_s32(vminq_s32(a, top), zero)),
        vqmovn_s32(vmaxq_s32(vminq_s32(b, top), zero)),
    );
    let p = vqshrun_n_s16::<5>(vqdmulhq_s16(v, v));
    vadd_s8(vreinterpret_s8_u8(p), vdup_n_s8(-128))
}

/// Weights packed for `vdotq_laneq_s32`: for input chunk `c` (four inputs) and output `o`, the four bytes `w[o][4c..4c+4]` sit at `(c * OUT + o) * 4`.
#[inline]
fn packed_index(out: usize, inp: usize, n_out: usize) -> usize {
    (inp / 4 * n_out + out) * 4 + inp % 4
}

#[derive(Default)]
pub(super) struct StackQ {
    /// Transformer rows per perspective, `[feature][ACC_DIMS]`, the White copy pre-swapped.
    ft_b: Vec<i16>,
    ft_w: Vec<i16>,
    ft_bias: Vec<i16>,
    /// Phase-adaptive rows per perspective, `[bucket][feature][PA_DIMS]`.
    pa_b: Vec<i16>,
    pa_w: Vec<i16>,
    pa_bias: Vec<i16>,
    /// Per stage, packed (see [`packed_index`]).
    l1_w: Vec<i8>,
    /// Per stage, with `128 * rowsum` folded in for the shifted input.
    l1_b: Vec<i32>,
    l2_w: Vec<i8>,
    l2_b: Vec<i32>,
    out_w: Vec<i16>,
    out_b: Vec<i32>,
    /// `l1_w` and `l2_w` per stage again, as [`pair_pack`] lays them out.
    #[cfg(all(
        target_arch = "x86_64",
        target_feature = "avx2",
        not(feature = "nnue-scalar")
    ))]
    l1_pairs: Vec<i16>,
    #[cfg(all(
        target_arch = "x86_64",
        target_feature = "avx2",
        not(feature = "nnue-scalar")
    ))]
    l2_pairs: Vec<i16>,
    /// Evaluations by board, shared by every thread; rebuilt with the weights.
    pub(super) cache: Vec<super::EvalSlot>,
}

impl StackQ {
    pub(super) fn build(net: &Nnue) -> StackQ {
        let qi16 =
            |v: f32, scale: f32| -> i16 { (v * scale).round().clamp(-32768.0, 32767.0) as i16 };

        let rows = |t: &[f32]| -> Vec<i16> { t.iter().map(|&v| qi16(v, ROW_ONE as f32)).collect() };
        let ft_b = rows(&net.ft);
        let ft_bias = rows(&net.ft_bias);
        let pa_b = rows(&net.pa);
        let pa_bias = rows(&net.pa_bias);

        let ft_w = net.indexer.swap_colors(&ft_b, &net.mask_off, ACC_DIMS);
        let pa_w = pa_b
            .chunks(net.n_features * PA_DIMS)
            .flat_map(|bucket| net.indexer.swap_colors(bucket, &net.mask_off, PA_DIMS))
            .collect();

        let qi8 = |v: f32| -> i8 { (v * 64.0).round().clamp(-127.0, 127.0) as i8 };
        let mut l1_w = vec![0i8; SO_STAGES * L1_CHUNKS * 4 * SO_L1];
        let mut l1_b = vec![0i32; SO_STAGES * SO_L1];
        let mut l2_w = vec![0i8; SO_STAGES * L2_CHUNKS * 4 * SO_L2];
        let mut l2_b = vec![0i32; SO_STAGES * SO_L2];
        for st in 0..SO_STAGES {
            let l1 = &mut l1_w[st * L1_CHUNKS * 4 * SO_L1..(st + 1) * L1_CHUNKS * 4 * SO_L1];
            for o in 0..SO_L1 {
                let row = &net.so_l1_w[(st * SO_L1 + o) * super::SO_L1_IN..];
                let mut sum = 0i32;
                for k in 0..super::SO_L1_IN {
                    let w = qi8(row[k]);
                    sum += w as i32;
                    l1[packed_index(o, k, SO_L1)] = w;
                }
                let b = (net.so_l1_b[st * SO_L1 + o] * (1 << 14) as f32).round() as i32;
                l1_b[st * SO_L1 + o] = b + 128 * sum;
            }
            let l2 = &mut l2_w[st * L2_CHUNKS * 4 * SO_L2..(st + 1) * L2_CHUNKS * 4 * SO_L2];
            for o in 0..SO_L2 {
                let row = &net.so_l2_w[(st * SO_L2 + o) * L2_IN..];
                let mut sum = 0i32;
                for k in 0..L2_IN {
                    let w = qi8(row[k]);
                    sum += w as i32;
                    l2[packed_index(o, k, SO_L2)] = w;
                }
                let b = (net.so_l2_b[st * SO_L2 + o] * (1 << 14) as f32).round() as i32;
                l2_b[st * SO_L2 + o] = b + 128 * sum;
            }
        }
        let mut out_w = vec![0i16; SO_STAGES * OUT_IN];
        let mut out_b = vec![0i32; SO_STAGES];
        for st in 0..SO_STAGES {
            let mut sum = 0i32;
            for k in 0..OUT_IN {
                let w = qi16(net.so_out_w[st * OUT_IN + k], 4096.0);
                sum += w as i32;
                out_w[st * OUT_IN + k] = w;
            }
            let b = (net.so_out_b[st] * (1u32 << 20) as f32).round() as i32;
            out_b[st] = b + 128 * sum;
        }

        #[cfg(all(
            target_arch = "x86_64",
            target_feature = "avx2",
            not(feature = "nnue-scalar")
        ))]
        let (l1_pairs, l2_pairs) = {
            let (n1, n2) = (L1_CHUNKS * 4 * SO_L1, L2_CHUNKS * 4 * SO_L2);
            let mut a = Vec::with_capacity(SO_STAGES * n1);
            let mut b = Vec::with_capacity(SO_STAGES * n2);
            for st in 0..SO_STAGES {
                a.extend(pair_pack(&l1_w[st * n1..(st + 1) * n1], SO_L1, L1_IN));
                b.extend(pair_pack(&l2_w[st * n2..(st + 1) * n2], SO_L2, L2_IN));
            }
            (a, b)
        };

        StackQ {
            ft_b,
            ft_w,
            ft_bias,
            pa_b,
            pa_w,
            pa_bias,
            l1_w,
            l1_b,
            l2_w,
            l2_b,
            out_w,
            out_b,
            #[cfg(all(
                target_arch = "x86_64",
                target_feature = "avx2",
                not(feature = "nnue-scalar")
            ))]
            l1_pairs,
            #[cfg(all(
                target_arch = "x86_64",
                target_feature = "avx2",
                not(feature = "nnue-scalar")
            ))]
            l2_pairs,
            cache: (0..1usize << super::EVAL_CACHE_BITS)
                .map(|_| super::EvalSlot::default())
                .collect(),
        }
    }

    /// The rows for `player` to move: the transformer's, and the phase rows of `stage`'s bucket.
    #[inline(always)]
    fn tables(&self, net: &Nnue, player: Color, stage: usize) -> (&[i16], &[i16]) {
        let (ft, pa) = if player == Color::Black {
            (&self.ft_b, &self.pa_b)
        } else {
            (&self.ft_w, &self.pa_w)
        };
        let base = pa_bucket(stage) * net.n_features * PA_DIMS;
        // `pa` holds every bucket's rows, so the start is inside it.
        (ft, unsafe { pa.get_unchecked(base..) })
    }

    /// The score in discs for the side to move.
    #[inline]
    pub(super) fn eval(
        &self,
        net: &Nnue,
        indices: &PatternIndices,
        player: Color,
        stage: usize,
        mob: usize,
    ) -> f32 {
        let rows = net.features_black(indices);
        let (ft, pa) = self.tables(net, player, stage);
        let st = so_stage(stage);
        #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
        let sum =
            unsafe { self.eval_neon(ft.as_ptr(), pa.as_ptr(), &rows, net.n_masks, st, stage, mob) };
        #[cfg(all(
            target_arch = "x86_64",
            target_feature = "avx2",
            not(feature = "nnue-scalar")
        ))]
        let sum =
            unsafe { self.eval_avx2(ft.as_ptr(), pa.as_ptr(), &rows, net.n_masks, st, stage, mob) };
        #[cfg(not(any(
            all(target_arch = "aarch64", not(feature = "nnue-scalar")),
            all(
                target_arch = "x86_64",
                target_feature = "avx2",
                not(feature = "nnue-scalar")
            )
        )))]
        let sum = self.eval_scalar(ft, pa, &rows, net.n_masks, st, stage, mob);
        sum as f32 / OUT_PER_DISC
    }

    /// Two siblings' scores: the same values as two [`StackQ::eval`] calls.
    pub(super) fn eval_pair(
        &self,
        net: &Nnue,
        indices: (&PatternIndices, &PatternIndices),
        player: Color,
        stage: usize,
        mob: (usize, usize),
    ) -> (f32, f32) {
        #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
        {
            let ra = net.features_black(indices.0);
            let rb = net.features_black(indices.1);
            let (ft, pa) = self.tables(net, player, stage);
            let (a, b) = unsafe {
                self.eval_pair_neon(
                    ft.as_ptr(),
                    pa.as_ptr(),
                    &ra,
                    &rb,
                    net.n_masks,
                    so_stage(stage),
                    stage,
                    mob,
                )
            };
            (a as f32 / OUT_PER_DISC, b as f32 / OUT_PER_DISC)
        }
        #[cfg(not(all(target_arch = "aarch64", not(feature = "nnue-scalar"))))]
        (
            self.eval(net, indices.0, player, stage, mob.0),
            self.eval(net, indices.1, player, stage, mob.1),
        )
    }

    #[cfg_attr(
        any(
            all(target_arch = "aarch64", not(feature = "nnue-scalar")),
            all(
                target_arch = "x86_64",
                target_feature = "avx2",
                not(feature = "nnue-scalar")
            )
        ),
        allow(dead_code)
    )]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn eval_scalar(
        &self,
        ft: &[i16],
        pa: &[i16],
        rows: &[u32; MAX_MASKS],
        n: usize,
        st: usize,
        stage: usize,
        mob: usize,
    ) -> i32 {
        let mut acc = [0i32; ACC_DIMS];
        for (a, &b) in acc.iter_mut().zip(self.ft_bias.iter()) {
            *a = b as i32;
        }
        for &r in rows.iter().take(n) {
            let row = &ft[r as usize * ACC_DIMS..(r as usize + 1) * ACC_DIMS];
            for (a, &v) in acc.iter_mut().zip(row) {
                *a += v as i32;
            }
        }
        let mut xin = [0i8; L1_IN];
        for i in 0..H {
            let lo = (acc[i] as i16).clamp(0, ROW_CLAMP) as i32;
            let hi = (acc[i + H] as i16).min(ROW_CLAMP) as i32;
            let p = ((lo << 5) * hi) >> 15;
            xin[i] = (p.clamp(0, 255) - 128) as i8;
        }
        let bucket = pa_bucket(stage);
        let mut pacc = [0i32; PA_DIMS];
        for (a, &b) in pacc.iter_mut().zip(self.pa_bias[bucket * PA_DIMS..].iter()) {
            *a = b as i32;
        }
        for &r in rows.iter().take(n) {
            let row = &pa[r as usize * PA_DIMS..(r as usize + 1) * PA_DIMS];
            for (a, &v) in pacc.iter_mut().zip(row) {
                *a += v as i32;
            }
        }
        for j in 0..PA_DIMS {
            let v = (pacc[j] as i16).min(ROW_CLAMP) as i32;
            let p = ((v.max(0) << 5) * v) >> 15;
            xin[H + j] = (p.clamp(0, 255) - 128) as i8;
        }
        xin[SO_SKIP] = mob_byte(mob);

        let l1w = &self.l1_w[st * L1_CHUNKS * 4 * SO_L1..];
        let mut a1 = [0i8; L2_IN];
        for o in 0..SO_L1 {
            let mut s = self.l1_b[st * SO_L1 + o];
            for k in 0..L1_IN {
                s += l1w[packed_index(o, k, SO_L1)] as i32 * xin[k] as i32;
            }
            let s = s.clamp(-32768, 32767);
            let sq = ((s * s) >> 15).min(32767) >> 5;
            a1[o] = (sq.min(255) - 128) as i8;
            a1[SO_L1 + o] = ((s >> 6).clamp(0, 255) - 128) as i8;
        }
        let l2w = &self.l2_w[st * L2_CHUNKS * 4 * SO_L2..];
        let mut xout = [0i8; OUT_IN];
        for o in 0..SO_L2 {
            let mut s = self.l2_b[st * SO_L2 + o];
            for k in 0..L2_IN {
                s += l2w[packed_index(o, k, SO_L2)] as i32 * a1[k] as i32;
            }
            let v = s.clamp(0, 255 << 6);
            let sq = ((v * v) >> 15) >> 5;
            xout[o] = (sq.min(255) - 128) as i8;
        }
        xout[SO_L2..].copy_from_slice(&xin[..SO_SKIP]);
        let ow = &self.out_w[st * OUT_IN..(st + 1) * OUT_IN];
        let mut sum = self.out_b[st];
        for k in 0..OUT_IN {
            sum += ow[k] as i32 * xout[k] as i32;
        }
        sum
    }

    /// [`StackQ::eval_scalar`] in AVX2: the same integers, sixteen lanes at a time.
    #[cfg(all(
        target_arch = "x86_64",
        target_feature = "avx2",
        not(feature = "nnue-scalar")
    ))]
    #[allow(clippy::too_many_arguments)]
    #[inline]
    unsafe fn eval_avx2(
        &self,
        ft: *const i16,
        pa: *const i16,
        rows: &[u32; MAX_MASKS],
        n: usize,
        st: usize,
        stage: usize,
        mob: usize,
    ) -> i32 {
        use std::arch::x86_64::*;

        /// Sum 128 lanes of `n` rows onto `bias`, wrapping like the 16-bit lanes of the scalar sum.
        #[inline(always)]
        unsafe fn sum128(
            table: *const i16,
            stride: usize,
            bias: *const i16,
            rows: &[u32; MAX_MASKS],
            n: usize,
        ) -> [__m256i; 8] {
            let mut acc = [_mm256_setzero_si256(); 8];
            for (r, a) in acc.iter_mut().enumerate() {
                *a = _mm256_loadu_si256(bias.add(r * 16) as *const __m256i);
            }
            for m in 0..n {
                let row = table.add(*rows.get_unchecked(m) as usize * stride);
                for (r, a) in acc.iter_mut().enumerate() {
                    *a =
                        _mm256_add_epi16(*a, _mm256_loadu_si256(row.add(r * 16) as *const __m256i));
                }
            }
            acc
        }

        for m in 0..n {
            prefetch_row_x86(ft, pa, *rows.get_unchecked(m));
        }
        let clamp = _mm256_set1_epi16(ROW_CLAMP);
        let (zero, top, shift) = (
            _mm256_setzero_si256(),
            _mm256_set1_epi16(255),
            _mm256_set1_epi16(128),
        );
        // `((a << 5) * b) >> 15` as the high half of `(a << 6) * b`, clamped to a byte and shifted by 128.
        let byte = |a: __m256i, b: __m256i| {
            let p = _mm256_mulhi_epi16(_mm256_slli_epi16::<6>(a), b);
            _mm256_sub_epi16(_mm256_min_epi16(_mm256_max_epi16(p, zero), top), shift)
        };

        let mut x = Lanes::<L1_IN>([0; L1_IN]);
        let xp = x.0.as_mut_ptr() as *mut __m256i;
        let lo = sum128(ft, ACC_DIMS, self.ft_bias.as_ptr(), rows, n);
        let hi = sum128(ft.add(H), ACC_DIMS, self.ft_bias.as_ptr().add(H), rows, n);
        for r in 0..8 {
            let a = _mm256_min_epi16(_mm256_max_epi16(lo[r], zero), clamp);
            _mm256_store_si256(xp.add(r), byte(a, _mm256_min_epi16(hi[r], clamp)));
        }
        let pz = sum128(
            pa,
            PA_DIMS,
            self.pa_bias.as_ptr().add(pa_bucket(stage) * PA_DIMS),
            rows,
            n,
        );
        for (r, &z) in pz.iter().enumerate() {
            let v = _mm256_min_epi16(z, clamp);
            _mm256_store_si256(xp.add(H / 16 + r), byte(_mm256_max_epi16(v, zero), v));
        }
        x.0[SO_SKIP] = mob_byte(mob) as i16;

        // `L1`: one input pair against eight outputs per `madd`; two chains per output half.
        let w1 = self.l1_pairs.as_ptr().add(st * L1_IN * SO_L1) as *const __m256i;
        let mut acc1 = [
            _mm256_loadu_si256(self.l1_b.as_ptr().add(st * SO_L1) as *const __m256i),
            _mm256_loadu_si256(self.l1_b.as_ptr().add(st * SO_L1 + 8) as *const __m256i),
            zero,
            zero,
        ];
        let xi = x.0.as_ptr() as *const i32;
        for p in 0..L1_IN / 2 {
            let xv = _mm256_set1_epi32(xi.add(p).read());
            let c = (p & 1) * 2;
            acc1[c] = _mm256_add_epi32(
                acc1[c],
                _mm256_madd_epi16(_mm256_loadu_si256(w1.add(p * 2)), xv),
            );
            acc1[c + 1] = _mm256_add_epi32(
                acc1[c + 1],
                _mm256_madd_epi16(_mm256_loadu_si256(w1.add(p * 2 + 1)), xv),
            );
        }
        let mut s1 = [0i32; SO_L1];
        _mm256_storeu_si256(
            s1.as_mut_ptr() as *mut __m256i,
            _mm256_add_epi32(acc1[0], acc1[2]),
        );
        _mm256_storeu_si256(
            s1.as_mut_ptr().add(8) as *mut __m256i,
            _mm256_add_epi32(acc1[1], acc1[3]),
        );
        let mut a1 = Lanes::<L2_IN>([0; L2_IN]);
        for (o, &s) in s1.iter().enumerate() {
            let s = s.clamp(-32768, 32767);
            let sq = ((s * s) >> 15).min(32767) >> 5;
            a1.0[o] = (sq.min(255) - 128) as i16;
            a1.0[SO_L1 + o] = ((s >> 6).clamp(0, 255) - 128) as i16;
        }

        // `L2`: eight registers of eight outputs each.
        let w2 = self.l2_pairs.as_ptr().add(st * L2_IN * SO_L2) as *const __m256i;
        let b2 = self.l2_b.as_ptr().add(st * SO_L2) as *const __m256i;
        let mut acc2 = [zero; SO_L2 / 8];
        for (j, a) in acc2.iter_mut().enumerate() {
            *a = _mm256_loadu_si256(b2.add(j));
        }
        let ai = a1.0.as_ptr() as *const i32;
        for p in 0..L2_IN / 2 {
            let xv = _mm256_set1_epi32(ai.add(p).read());
            for (j, a) in acc2.iter_mut().enumerate() {
                *a = _mm256_add_epi32(
                    *a,
                    _mm256_madd_epi16(_mm256_loadu_si256(w2.add(p * 8 + j)), xv),
                );
            }
        }
        let mut s2 = [0i32; SO_L2];
        for (j, a) in acc2.iter().enumerate() {
            _mm256_storeu_si256(s2.as_mut_ptr().add(j * 8) as *mut __m256i, *a);
        }
        let mut xo = Lanes::<OUT_IN>([0; OUT_IN]);
        for (o, &s) in s2.iter().enumerate() {
            let v = s.clamp(0, 255 << 6);
            xo.0[o] = ((((v * v) >> 15) >> 5).min(255) - 128) as i16;
        }
        xo.0[SO_L2..].copy_from_slice(&x.0[..SO_SKIP]);

        let ow = self.out_w.as_ptr().add(st * OUT_IN) as *const __m256i;
        let xq = xo.0.as_ptr() as *const __m256i;
        let mut o = [zero; 2];
        for g in 0..OUT_IN / 16 {
            let m = _mm256_madd_epi16(_mm256_loadu_si256(ow.add(g)), _mm256_load_si256(xq.add(g)));
            o[g & 1] = _mm256_add_epi32(o[g & 1], m);
        }
        let v = _mm256_add_epi32(o[0], o[1]);
        let h = _mm_add_epi32(_mm256_castsi256_si128(v), _mm256_extracti128_si256::<1>(v));
        let h = _mm_add_epi32(h, _mm_shuffle_epi32::<0b01_00_11_10>(h));
        let h = _mm_add_epi32(h, _mm_shuffle_epi32::<0b10_11_00_01>(h));
        _mm_cvtsi128_si32(h) + self.out_b[st]
    }

    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    #[allow(clippy::too_many_arguments)]
    #[inline]
    unsafe fn eval_neon(
        &self,
        ft: *const i16,
        pa: *const i16,
        rows: &[u32; MAX_MASKS],
        n: usize,
        st: usize,
        stage: usize,
        mob: usize,
    ) -> i32 {
        use std::arch::aarch64::*;

        /// Sum 128 lanes of `n` rows onto `bias`, in sixteen registers.
        #[inline(always)]
        unsafe fn sum128(
            table: *const i16,
            stride: usize,
            bias: *const i16,
            rows: &[u32; MAX_MASKS],
            n: usize,
        ) -> [int16x8_t; 16] {
            let mut acc: [int16x8_t; 16] = [vdupq_n_s16(0); 16];
            for (r, a) in acc.iter_mut().enumerate() {
                *a = vld1q_s16(bias.add(r * 8));
            }
            for m in 0..n {
                let row = table.add(*rows.get_unchecked(m) as usize * stride);
                for (r, a) in acc.iter_mut().enumerate() {
                    *a = vaddq_s16(*a, vld1q_s16(row.add(r * 8)));
                }
            }
            acc
        }

        for m in 0..n {
            prefetch_row(ft, pa, *rows.get_unchecked(m));
        }
        let mut xin = Buf::<L1_IN>([0; L1_IN]);
        let x = xin.0.as_mut_ptr();

        let lo = sum128(ft, ACC_DIMS, self.ft_bias.as_ptr(), rows, n);
        let hi = sum128(ft.add(H), ACC_DIMS, self.ft_bias.as_ptr().add(H), rows, n);
        for r in 0..16 {
            vst1_s8(x.add(r * 8), ft_bytes(lo[r], hi[r]));
        }
        let bucket = pa_bucket(stage);
        let pz = sum128(
            pa,
            PA_DIMS,
            self.pa_bias.as_ptr().add(bucket * PA_DIMS),
            rows,
            n,
        );
        for (r, &z) in pz.iter().enumerate() {
            vst1_s8(x.add(H + r * 8), pa_bytes(z));
        }
        *x.add(SO_SKIP) = mob_byte(mob);

        self.layers_neon(x, st)
    }

    /// The stacked layers on one position's input bytes (`x - 128`, `L1_IN` of them).
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    #[inline(always)]
    unsafe fn layers_neon(&self, x: *const i8, st: usize) -> i32 {
        use std::arch::aarch64::*;
        let l1w = self.l1_w.as_ptr().add(st * L1_CHUNKS * 4 * SO_L1);
        let mut acc1: [int32x4_t; 4] = [vdupq_n_s32(0); 4];
        for (j, a) in acc1.iter_mut().enumerate() {
            *a = vld1q_s32(self.l1_b.as_ptr().add(st * SO_L1 + j * 4));
        }
        // Four partial sums per output: one chain of 64 `sdot`s would wait on its own latency.
        let mut part: [[int32x4_t; 4]; 3] = [[vdupq_n_s32(0); 4]; 3];
        for g in 0..SO_SKIP / 16 {
            let xv = vld1q_s8(x.add(g * 16));
            let wp = l1w.add(g * 4 * SO_L1 * 4);
            for j in 0..4 {
                acc1[j] = vdotq_laneq_s32::<0>(acc1[j], vld1q_s8(wp.add(j * 16)), xv);
                part[0][j] = vdotq_laneq_s32::<1>(part[0][j], vld1q_s8(wp.add(64 + j * 16)), xv);
                part[1][j] = vdotq_laneq_s32::<2>(part[1][j], vld1q_s8(wp.add(128 + j * 16)), xv);
                part[2][j] = vdotq_laneq_s32::<3>(part[2][j], vld1q_s8(wp.add(192 + j * 16)), xv);
            }
        }
        for j in 0..4 {
            acc1[j] = vaddq_s32(
                vaddq_s32(acc1[j], part[0][j]),
                vaddq_s32(part[1][j], part[2][j]),
            );
        }
        {
            let c = SO_SKIP / 4;
            let xb =
                vreinterpretq_s8_s32(vdupq_n_s32((x.add(c * 4) as *const i32).read_unaligned()));
            let wp = l1w.add(c * SO_L1 * 4);
            for (j, a) in acc1.iter_mut().enumerate() {
                *a = vdotq_s32(*a, vld1q_s8(wp.add(j * 16)), xb);
            }
        }
        let mut a1 = Buf::<L2_IN>([0; L2_IN]);
        let [sq, re] = l1_bytes(&acc1);
        vst1q_s8(a1.0.as_mut_ptr(), sq);
        vst1q_s8(a1.0.as_mut_ptr().add(16), re);

        let l2w = self.l2_w.as_ptr().add(st * L2_CHUNKS * 4 * SO_L2);
        let mut acc2: [int32x4_t; 16] = [vdupq_n_s32(0); 16];
        for (j, a) in acc2.iter_mut().enumerate() {
            *a = vld1q_s32(self.l2_b.as_ptr().add(st * SO_L2 + j * 4));
        }
        for g in 0..L2_IN / 16 {
            let xv = vld1q_s8(a1.0.as_ptr().add(g * 16));
            let wp = l2w.add(g * 4 * SO_L2 * 4);
            for (j, a) in acc2.iter_mut().enumerate() {
                *a = vdotq_laneq_s32::<0>(*a, vld1q_s8(wp.add(j * 16)), xv);
                *a = vdotq_laneq_s32::<1>(*a, vld1q_s8(wp.add(256 + j * 16)), xv);
                *a = vdotq_laneq_s32::<2>(*a, vld1q_s8(wp.add(512 + j * 16)), xv);
                *a = vdotq_laneq_s32::<3>(*a, vld1q_s8(wp.add(768 + j * 16)), xv);
            }
        }
        let mut xo = Buf::<OUT_IN>([0; OUT_IN]);
        for j in 0..8 {
            vst1_s8(
                xo.0.as_mut_ptr().add(j * 8),
                l2_bytes(acc2[2 * j], acc2[2 * j + 1]),
            );
        }
        std::ptr::copy_nonoverlapping(x, xo.0.as_mut_ptr().add(SO_L2), SO_SKIP);

        let ow = self.out_w.as_ptr().add(st * OUT_IN);
        let xp = xo.0.as_ptr();
        let mut o: [int32x4_t; 8] = [vdupq_n_s32(0); 8];
        for g in 0..OUT_IN / 32 {
            out_mla(
                &mut o,
                vld1q_s16_x4(ow.add(g * 32)),
                vld1q_s8_x2(xp.add(g * 32)),
            );
        }
        out_total(&o) + self.out_b[st]
    }

    /// [`StackQ::layers_neon`] for two inputs of one stage: each weight is read once, and the two chains interleave.
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    #[inline(always)]
    unsafe fn layers_pair_neon(&self, x: [*const i8; 2], st: usize) -> (i32, i32) {
        use std::arch::aarch64::*;
        let l1w = self.l1_w.as_ptr().add(st * L1_CHUNKS * 4 * SO_L1);
        let mut acc1: [[int32x4_t; 4]; 2] = [[vdupq_n_s32(0); 4]; 2];
        // Two chains per output, not four: two positions' four would not fit the registers.
        let mut part: [[int32x4_t; 4]; 2] = [[vdupq_n_s32(0); 4]; 2];
        for acc in acc1.iter_mut() {
            for (j, a) in acc.iter_mut().enumerate() {
                *a = vld1q_s32(self.l1_b.as_ptr().add(st * SO_L1 + j * 4));
            }
        }
        for g in 0..SO_SKIP / 16 {
            let xv = [vld1q_s8(x[0].add(g * 16)), vld1q_s8(x[1].add(g * 16))];
            let wp = l1w.add(g * 4 * SO_L1 * 4);
            for j in 0..4 {
                let w = [
                    vld1q_s8(wp.add(j * 16)),
                    vld1q_s8(wp.add(64 + j * 16)),
                    vld1q_s8(wp.add(128 + j * 16)),
                    vld1q_s8(wp.add(192 + j * 16)),
                ];
                for e in 0..2 {
                    acc1[e][j] = vdotq_laneq_s32::<0>(acc1[e][j], w[0], xv[e]);
                    part[e][j] = vdotq_laneq_s32::<1>(part[e][j], w[1], xv[e]);
                    acc1[e][j] = vdotq_laneq_s32::<2>(acc1[e][j], w[2], xv[e]);
                    part[e][j] = vdotq_laneq_s32::<3>(part[e][j], w[3], xv[e]);
                }
            }
        }
        let mut a1: [[int8x16_t; 2]; 2] = [[vdupq_n_s8(0); 2]; 2];
        {
            let c = SO_SKIP / 4;
            let wp = l1w.add(c * SO_L1 * 4);
            for e in 0..2 {
                let xb = vreinterpretq_s8_s32(vdupq_n_s32(
                    (x[e].add(c * 4) as *const i32).read_unaligned(),
                ));
                for j in 0..4 {
                    acc1[e][j] = vaddq_s32(acc1[e][j], part[e][j]);
                    acc1[e][j] = vdotq_s32(acc1[e][j], vld1q_s8(wp.add(j * 16)), xb);
                }
                a1[e] = l1_bytes(&acc1[e]);
            }
        }

        let l2w = self.l2_w.as_ptr().add(st * L2_CHUNKS * 4 * SO_L2);
        let mut xo = [Buf::<OUT_IN>([0; OUT_IN]), Buf::<OUT_IN>([0; OUT_IN])];
        for half in 0..2 {
            let mut acc2: [[int32x4_t; 8]; 2] = [[vdupq_n_s32(0); 8]; 2];
            for acc in acc2.iter_mut() {
                for (j, a) in acc.iter_mut().enumerate() {
                    *a = vld1q_s32(self.l2_b.as_ptr().add(st * SO_L2 + (half * 8 + j) * 4));
                }
            }
            for g in 0..L2_IN / 16 {
                let wp = l2w.add(g * 4 * SO_L2 * 4);
                for j in 0..8 {
                    let jj = half * 8 + j;
                    let w = [
                        vld1q_s8(wp.add(jj * 16)),
                        vld1q_s8(wp.add(256 + jj * 16)),
                        vld1q_s8(wp.add(512 + jj * 16)),
                        vld1q_s8(wp.add(768 + jj * 16)),
                    ];
                    for (acc, xs) in acc2.iter_mut().zip(&a1) {
                        let xv = xs[g];
                        acc[j] = vdotq_laneq_s32::<0>(acc[j], w[0], xv);
                        acc[j] = vdotq_laneq_s32::<1>(acc[j], w[1], xv);
                        acc[j] = vdotq_laneq_s32::<2>(acc[j], w[2], xv);
                        acc[j] = vdotq_laneq_s32::<3>(acc[j], w[3], xv);
                    }
                }
            }
            for e in 0..2 {
                for k in 0..4 {
                    vst1_s8(
                        xo[e].0.as_mut_ptr().add((half * 4 + k) * 8),
                        l2_bytes(acc2[e][2 * k], acc2[e][2 * k + 1]),
                    );
                }
            }
        }
        for e in 0..2 {
            std::ptr::copy_nonoverlapping(x[e], xo[e].0.as_mut_ptr().add(SO_L2), SO_SKIP);
        }

        let ow = self.out_w.as_ptr().add(st * OUT_IN);
        let mut o: [[int32x4_t; 8]; 2] = [[vdupq_n_s32(0); 8]; 2];
        for g in 0..OUT_IN / 32 {
            let wv = vld1q_s16_x4(ow.add(g * 32));
            for (oe, x) in o.iter_mut().zip(&xo) {
                out_mla(oe, wv, vld1q_s8_x2(x.0.as_ptr().add(g * 32)));
            }
        }
        (
            out_total(&o[0]) + self.out_b[st],
            out_total(&o[1]) + self.out_b[st],
        )
    }

    /// Two siblings at once: same side to move and stage, so a row both use is read once.
    #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
    #[allow(clippy::too_many_arguments)]
    unsafe fn eval_pair_neon(
        &self,
        ft: *const i16,
        pa: *const i16,
        ra: &[u32; MAX_MASKS],
        rb: &[u32; MAX_MASKS],
        n: usize,
        st: usize,
        stage: usize,
        mob: (usize, usize),
    ) -> (i32, i32) {
        use std::arch::aarch64::*;
        let mut both = [0u32; MAX_MASKS];
        let mut only_a = [0u32; MAX_MASKS];
        let mut only_b = [0u32; MAX_MASKS];
        let (mut n_both, mut n_diff) = (0, 0);
        for m in 0..n {
            let (x, y) = (*ra.get_unchecked(m), *rb.get_unchecked(m));
            if x == y {
                both[n_both] = x;
                n_both += 1;
            } else {
                only_a[n_diff] = x;
                only_b[n_diff] = y;
                n_diff += 1;
            }
        }
        for &r in &both[..n_both] {
            prefetch_row(ft, pa, r);
        }
        for i in 0..n_diff {
            prefetch_row(ft, pa, only_a[i]);
            prefetch_row(ft, pa, only_b[i]);
        }

        let mut xa = Buf::<L1_IN>([0; L1_IN]);
        let mut xb = Buf::<L1_IN>([0; L1_IN]);

        for c in 0..H / 32 {
            let mut lo_a: [int16x8_t; 4] = [vdupq_n_s16(0); 4];
            let mut hi_a: [int16x8_t; 4] = [vdupq_n_s16(0); 4];
            for r in 0..4 {
                lo_a[r] = vld1q_s16(self.ft_bias.as_ptr().add(c * 32 + r * 8));
                hi_a[r] = vld1q_s16(self.ft_bias.as_ptr().add(H + c * 32 + r * 8));
            }
            for &row in &both[..n_both] {
                let p = ft.add(row as usize * ACC_DIMS + c * 32);
                for r in 0..4 {
                    lo_a[r] = vaddq_s16(lo_a[r], vld1q_s16(p.add(r * 8)));
                    hi_a[r] = vaddq_s16(hi_a[r], vld1q_s16(p.add(H + r * 8)));
                }
            }
            let (mut lo_b, mut hi_b) = (lo_a, hi_a);
            for i in 0..n_diff {
                let p = ft.add(only_a[i] as usize * ACC_DIMS + c * 32);
                let q = ft.add(only_b[i] as usize * ACC_DIMS + c * 32);
                for r in 0..4 {
                    lo_a[r] = vaddq_s16(lo_a[r], vld1q_s16(p.add(r * 8)));
                    hi_a[r] = vaddq_s16(hi_a[r], vld1q_s16(p.add(H + r * 8)));
                    lo_b[r] = vaddq_s16(lo_b[r], vld1q_s16(q.add(r * 8)));
                    hi_b[r] = vaddq_s16(hi_b[r], vld1q_s16(q.add(H + r * 8)));
                }
            }
            for r in 0..4 {
                for (lo, hi, x) in [(lo_a[r], hi_a[r], &mut xa), (lo_b[r], hi_b[r], &mut xb)] {
                    vst1_s8(x.0.as_mut_ptr().add(c * 32 + r * 8), ft_bytes(lo, hi));
                }
            }
        }
        let pbias = self.pa_bias.as_ptr().add(pa_bucket(stage) * PA_DIMS);
        for c in 0..PA_DIMS / 64 {
            let mut za: [int16x8_t; 8] = [vdupq_n_s16(0); 8];
            for (r, z) in za.iter_mut().enumerate() {
                *z = vld1q_s16(pbias.add(c * 64 + r * 8));
            }
            for &row in &both[..n_both] {
                let p = pa.add(row as usize * PA_DIMS + c * 64);
                for (r, z) in za.iter_mut().enumerate() {
                    *z = vaddq_s16(*z, vld1q_s16(p.add(r * 8)));
                }
            }
            let mut zb = za;
            for i in 0..n_diff {
                let p = pa.add(only_a[i] as usize * PA_DIMS + c * 64);
                let q = pa.add(only_b[i] as usize * PA_DIMS + c * 64);
                for r in 0..8 {
                    za[r] = vaddq_s16(za[r], vld1q_s16(p.add(r * 8)));
                    zb[r] = vaddq_s16(zb[r], vld1q_s16(q.add(r * 8)));
                }
            }
            for r in 0..8 {
                for (z, x) in [(za[r], &mut xa), (zb[r], &mut xb)] {
                    vst1_s8(x.0.as_mut_ptr().add(H + c * 64 + r * 8), pa_bytes(z));
                }
            }
        }
        xa.0[SO_SKIP] = mob_byte(mob.0);
        xb.0[SO_SKIP] = mob_byte(mob.1);
        self.layers_pair_neon([xa.0.as_ptr(), xb.0.as_ptr()], st)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;
    use crate::pattern::NNUE_PATTERNS;
    use crate::position::Position;

    fn walk_game(mut f: impl FnMut(&Board, usize)) {
        let mut board = Board::new();
        for ply in 0..60 {
            f(&board, ply);
            let moves = board.movable();
            if moves == 0 {
                board.pass();
                if board.movable() == 0 {
                    break;
                }
                continue;
            }
            let pos = Position::from_index(moves.trailing_zeros()).unwrap();
            board.make_move_bits(pos);
        }
    }

    fn scalar_discs(nn: &Nnue, board: &Board) -> f32 {
        let ix = nn.indices(board.black, board.white);
        let stage = crate::linear::Linear::stage(board);
        let rows = nn.features_black(&ix);
        let (ft, pa) = nn.sq.tables(nn, board.player(), stage);
        nn.sq.eval_scalar(
            ft,
            pa,
            &rows,
            nn.n_masks,
            so_stage(stage),
            stage,
            Nnue::mob_index(board),
        ) as f32
            / OUT_PER_DISC
    }

    #[test]
    fn vector_kernel_matches_scalar() {
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.init_weights();
        nn.quantize();
        walk_game(|board, ply| {
            let ix = nn.indices(board.black, board.white);
            let q = nn.eval_from_indices(&ix, board);
            let s = scalar_discs(&nn, board);
            assert!(
                (q - s).abs() < 1e-6,
                "vector {q} vs scalar {s} at ply {ply}"
            );
        });
    }

    #[test]
    fn pair_kernel_matches_two_singles() {
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.init_weights();
        nn.quantize();
        walk_game(|board, _| {
            let ix = nn.indices(board.black, board.white);
            let kids: Vec<(Board, PatternIndices)> = (0..64)
                .filter(|&i| board.movable() >> i & 1 == 1)
                .map(|i| {
                    let pos = Position::from_index(i).unwrap();
                    let mut b = *board;
                    let flipped = b.make_move_bits(pos);
                    let mut cix = ix;
                    nn.ix_apply(&mut cix, pos, flipped, board.player());
                    (b, cix)
                })
                .collect();
            for w in kids.windows(2) {
                let ((ba, ia), (bb, ib)) = (&w[0], &w[1]);
                let stage = crate::linear::Linear::stage(ba);
                let mobs = (Nnue::mob_index(ba), Nnue::mob_index(bb));
                let (x, y) = nn.sq.eval_pair(&nn, (ia, ib), ba.player(), stage, mobs);
                assert_eq!(
                    x.to_bits(),
                    nn.sq.eval(&nn, ia, ba.player(), stage, mobs.0).to_bits()
                );
                assert_eq!(
                    y.to_bits(),
                    nn.sq.eval(&nn, ib, bb.player(), stage, mobs.1).to_bits()
                );
            }
        });
    }

    /// `KUROOBI_STACK_WEIGHTS` names the trained weight file; every position of a game and its children.
    #[test]
    #[ignore]
    fn vector_kernel_matches_scalar_on_trained_weights() {
        let path = std::env::var("KUROOBI_STACK_WEIGHTS")
            .expect("KUROOBI_STACK_WEIGHTS must name a stacked-read-out weight file");
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.load(std::path::Path::new(&path)).expect("weights load");
        nn.quantize();
        let mut checked = 0;
        walk_game(|board, _| {
            for i in (0..64).filter(|&i| board.movable() >> i & 1 == 1) {
                let mut b = *board;
                b.make_move_bits(Position::from_index(i).unwrap());
                let ix = nn.indices(b.black, b.white);
                let q = nn.eval_from_indices(&ix, &b);
                assert_eq!(q.to_bits(), scalar_discs(&nn, &b).to_bits(), "{b:?}");
                checked += 1;
            }
        });
        assert!(checked > 200, "only {checked} positions");
    }

    /// `KUROOBI_STACK_WEIGHTS` names the trained weight file.
    #[test]
    #[ignore]
    fn integer_stack_matches_f32_on_trained_weights() {
        let path = std::env::var("KUROOBI_STACK_WEIGHTS")
            .expect("KUROOBI_STACK_WEIGHTS must name a stacked-read-out weight file");
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.load(std::path::Path::new(&path)).expect("weights load");
        nn.quantize();
        let mut worst = 0.0f32;
        walk_game(|board, _| {
            let ix = nn.indices(board.black, board.white);
            let f = nn.eval_indices(board, &ix);
            let q = nn.eval_from_indices(&ix, board);
            worst = worst.max((q - f).abs() / f.abs().max(4.0));
        });
        // 0.088 on the 2026-10-05 weights; no file of this architecture measured under 0.059.
        assert!(
            worst < 0.1,
            "integer stack drifts {worst} (relative) from f32"
        );
    }
}
