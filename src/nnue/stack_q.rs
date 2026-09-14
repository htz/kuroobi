//! The stacked read-out in integers, the form the search runs.
//!
//! Every scale here is fixed rather than chosen from the model, so that the
//! arithmetic is defined by the format alone and a model trained against
//! these numbers evaluates the same wherever the file is loaded:
//!
//! | quantity              | type | one unit is        |
//! |-----------------------|------|--------------------|
//! | transformer, PA rows  | i16  | 1/512              |
//! | activations           | u8   | 1/256              |
//! | L1, L2 weights        | i8   | 1/64               |
//! | L1, L2 biases         | i32  | 1/2^14 (= 256·64)  |
//! | output weights        | i16  | 1/4096             |
//! | output bias, sum      | i32  | 1/2^20             |
//!
//! Activations are stored shifted by -128 as i8, not as u8: the dot-product
//! instruction this machine has multiplies signed by signed, and the
//! usual way around that -- splitting the input into a low seven bits
//! and a sign bit and running the dot twice -- costs twice the multiplies.
//! Shifting instead is exact once each row's weight sum, times 128, is
//! folded into its bias.
//!
//! The dense layers are 257→16, 32→64 and 320→1 per stage: a few kilobytes
//! against the 32 random rows of 512 bytes the transformer reads, so the
//! row reads still set the pace and this file's job is to not add to it.

use super::{
    ft_bucket, pa_bucket, so_stage, Nnue, ACC_DIMS, H, PA_BUCKETS, PA_DIMS, SO_L1, SO_L2,
    SO_OUT_IN, SO_SKIP, SO_STAGES,
};
use crate::color::Color;
use crate::pattern_index::{PatternIndices, MAX_MASKS};

/// Rows are scaled so that `1.0` is this many steps.
const ROW_ONE: i32 = 512;
/// A row lane is clamped at this before the product, one step short of
/// `ROW_ONE` so the product of two clamped lanes fits a byte.
const ROW_CLAMP: i16 = 510;
/// `L1`'s input, padded to whole 16-byte groups plus one extra chunk of
/// four so the mobility lane (index 256) has a home.
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

/// Weights packed for `vdotq_laneq_s32`: for input chunk `c` (four inputs)
/// and output `o`, the four bytes `w[o][4c..4c+4]` sit at
/// `(c * OUT + o) * 4`. One 16-byte load then holds four outputs by four
/// inputs, which is exactly what one `sdot` consumes.
#[inline]
fn packed_index(out: usize, inp: usize, n_out: usize) -> usize {
    (inp / 4 * n_out + out) * 4 + inp % 4
}

/// The quantized tables of the stacked read-out. Built by [`Nnue::quantize`].
#[derive(Default)]
pub(super) struct StackQ {
    /// Transformer rows per perspective, `[feature][ACC_DIMS]`, the White
    /// copy pre-swapped like `ft_w_i8`.
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
    /// Weights that did not fit their integer type, and how many there were.
    pub clipped: usize,
    pub total: usize,
}

impl StackQ {
    pub(super) fn build(net: &Nnue) -> StackQ {
        use std::cell::Cell;
        let clipped = Cell::new(0usize);
        let total = Cell::new(0usize);
        let qi16 = |v: f32, scale: f32| -> i16 {
            let r = (v * scale).round();
            total.set(total.get() + 1);
            if r > 32767.0 || r < -32768.0 {
                clipped.set(clipped.get() + 1);
            }
            r.clamp(-32768.0, 32767.0) as i16
        };

        let n_feat = net.n_features;
        let mut ft_b = vec![0i16; n_feat * ACC_DIMS];
        for (d, &v) in ft_b.iter_mut().zip(net.ft.iter()) {
            *d = qi16(v, ROW_ONE as f32);
        }
        let ft_bias: Vec<i16> = net
            .ft_bias
            .iter()
            .map(|&v| qi16(v, ROW_ONE as f32))
            .collect();
        let mut pa_b = vec![0i16; net.pa.len()];
        for (d, &v) in pa_b.iter_mut().zip(net.pa.iter()) {
            *d = qi16(v, ROW_ONE as f32);
        }
        let pa_bias: Vec<i16> = net
            .pa_bias
            .iter()
            .map(|&v| qi16(v, ROW_ONE as f32))
            .collect();

        // The White copies: each mask's rows read through the digit-swapped
        // index, in every transformer copy (see `quantize` for why every).
        let mut ft_w = vec![0i16; ft_b.len()];
        let mut pa_w = vec![0i16; pa_b.len()];
        let n_bucket = net.n_feat_bucket;
        for m in 0..net.n_masks {
            let off = net.mask_off[m] as usize;
            let size = net.patterns[net.indexer.mask_patterns()[m] as usize].table_size();
            for i in 0..size {
                let j = net.indexer.swapped_index(m, i);
                for b in 0..super::FT_BUCKETS {
                    let src = (b * n_bucket + off + j) * ACC_DIMS;
                    let dst = (b * n_bucket + off + i) * ACC_DIMS;
                    ft_w[dst..dst + ACC_DIMS].copy_from_slice(&ft_b[src..src + ACC_DIMS]);
                }
                if !pa_b.is_empty() {
                    for b in 0..PA_BUCKETS {
                        let src = (b * n_bucket + off + j) * PA_DIMS;
                        let dst = (b * n_bucket + off + i) * PA_DIMS;
                        pa_w[dst..dst + PA_DIMS].copy_from_slice(&pa_b[src..src + PA_DIMS]);
                    }
                }
            }
        }

        let qi8 = |v: f32| -> i8 {
            let r = (v * 64.0).round();
            total.set(total.get() + 1);
            if r.abs() > 127.0 {
                clipped.set(clipped.get() + 1);
            }
            r.clamp(-127.0, 127.0) as i8
        };
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
            clipped: clipped.get(),
            total: total.get(),
        }
    }

    /// Row ids inside one transformer copy, one per mask.
    #[inline]
    fn rows(net: &Nnue, indices: &PatternIndices) -> [u32; MAX_MASKS] {
        let mut f = [0u32; MAX_MASKS];
        let raw = indices.raw();
        for m in 0..net.n_masks {
            f[m] = net.mask_off[m] + raw[m] as u32;
        }
        f
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
        let rows = Self::rows(net, indices);
        let (ft, pa) = if player == Color::Black {
            (&self.ft_b, &self.pa_b)
        } else {
            (&self.ft_w, &self.pa_w)
        };
        let ft_base = ft_bucket(stage) * net.n_feat_bucket;
        let pa_base = pa_bucket(stage) * net.n_feat_bucket;
        let st = so_stage(stage);
        #[cfg(all(target_arch = "aarch64", not(feature = "nnue-scalar")))]
        // SAFETY: row ids stay inside their pattern's table, so every row
        // read is in bounds, and the buffers below are sized for the layers.
        let sum = unsafe {
            self.eval_neon(
                ft.as_ptr().add(ft_base * ACC_DIMS),
                pa.as_ptr().add(pa_base * PA_DIMS),
                &rows,
                net.n_masks,
                st,
                stage,
                mob,
            )
        };
        #[cfg(not(all(target_arch = "aarch64", not(feature = "nnue-scalar"))))]
        let sum = self.eval_scalar(
            &ft[ft_base * ACC_DIMS..],
            &pa[pa_base * PA_DIMS..],
            &rows,
            net.n_masks,
            st,
            stage,
            mob,
        );
        sum as f32 / OUT_PER_DISC
    }

    /// The same arithmetic, one integer at a time. What the vector kernel is
    /// checked against, and the path of machines without one.
    #[cfg_attr(
        all(target_arch = "aarch64", not(feature = "nnue-scalar")),
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
        // Shifted input to the stack: x - 128, as the kernel stores it.
        let mut xin = [0i8; L1_IN];
        for i in 0..H {
            // The vector form: `lo` clamped and shifted up by 5, `hi` capped,
            // a doubling multiply-high (>> 15), then narrowed to a byte.
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
        xin[SO_SKIP] = ((mob as i32 * 7).min(255) - 128) as i8;

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

        #[repr(align(64))]
        struct Buf<const N: usize>([i8; N]);

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

        /* Ask for every line of the rows the second and third passes will
        read before the first pass starts. The three passes walk the same
        32 rows, and the first is already waiting on memory; getting the
        other two rows' lines moving in the same shadow is worth a quarter
        of the cold-row cost (1980 -> 1500 ns over val22's 1500 positions).
        One line per row
        recovers only half of that: the hardware prefetcher does not carry
        on from a single touch. */
        for m in 0..n {
            let r = *rows.get_unchecked(m) as usize;
            let hp = ft.add(r * ACC_DIMS + H) as *const u8;
            let pp = pa.add(r * PA_DIMS) as *const u8;
            for l in 0..(H * 2 / 64) {
                let q = hp.add(l * 64);
                std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) q, options(nostack, readonly));
                let q = pp.add(l * 64);
                std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) q, options(nostack, readonly));
            }
        }
        let mut xin = Buf::<L1_IN>([0; L1_IN]);
        let x = xin.0.as_mut_ptr();
        let clamp = vdupq_n_s16(ROW_CLAMP);
        let shift = vdupq_n_s8(-128);

        // The base layer, low then high half, folded to a byte per pair.
        let lo = sum128(ft, ACC_DIMS, self.ft_bias.as_ptr(), rows, n);
        let hi = sum128(ft.add(H), ACC_DIMS, self.ft_bias.as_ptr().add(H), rows, n);
        for r in 0..16 {
            let a = vreinterpretq_s16_u16(vqshluq_n_s16::<5>(vminq_s16(lo[r], clamp)));
            let b = vminq_s16(hi[r], clamp);
            let p = vqmovun_s16(vqdmulhq_s16(a, b));
            vst1_s8(
                x.add(r * 8),
                vadd_s8(vreinterpret_s8_u8(p), vget_low_s8(shift)),
            );
        }
        // The phase-adaptive layer, squared.
        let bucket = pa_bucket(stage);
        let pz = sum128(
            pa,
            PA_DIMS,
            self.pa_bias.as_ptr().add(bucket * PA_DIMS),
            rows,
            n,
        );
        for r in 0..16 {
            let v = vminq_s16(pz[r], clamp);
            let p = vqmovun_s16(vqdmulhq_s16(
                vreinterpretq_s16_u16(vqshluq_n_s16::<5>(v)),
                v,
            ));
            vst1_s8(
                x.add(H + r * 8),
                vadd_s8(vreinterpret_s8_u8(p), vget_low_s8(shift)),
            );
        }
        *x.add(SO_SKIP) = ((mob as i32 * 7).min(255) - 128) as i8;

        // L1: 257 -> 16, four accumulators of four outputs each.
        let l1w = self.l1_w.as_ptr().add(st * L1_CHUNKS * 4 * SO_L1);
        let mut acc1: [int32x4_t; 4] = [vdupq_n_s32(0); 4];
        for j in 0..4 {
            acc1[j] = vld1q_s32(self.l1_b.as_ptr().add(st * SO_L1 + j * 4));
        }
        for g in 0..SO_SKIP / 16 {
            let xv = vld1q_s8(x.add(g * 16));
            let wp = l1w.add(g * 4 * SO_L1 * 4);
            for j in 0..4 {
                acc1[j] = vdotq_laneq_s32::<0>(acc1[j], vld1q_s8(wp.add(j * 16)), xv);
                acc1[j] = vdotq_laneq_s32::<1>(acc1[j], vld1q_s8(wp.add(64 + j * 16)), xv);
                acc1[j] = vdotq_laneq_s32::<2>(acc1[j], vld1q_s8(wp.add(128 + j * 16)), xv);
                acc1[j] = vdotq_laneq_s32::<3>(acc1[j], vld1q_s8(wp.add(192 + j * 16)), xv);
            }
        }
        // The chunk holding mobility (the rest of the padding is zero weight).
        {
            let c = SO_SKIP / 4;
            let xb =
                vreinterpretq_s8_s32(vdupq_n_s32((x.add(c * 4) as *const i32).read_unaligned()));
            let wp = l1w.add(c * SO_L1 * 4);
            for j in 0..4 {
                acc1[j] = vdotq_s32(acc1[j], vld1q_s8(wp.add(j * 16)), xb);
            }
        }
        // Squared and plain, each to a byte, shifted.
        let mut a1 = Buf::<L2_IN>([0; L2_IN]);
        let s0 = vcombine_s16(vqmovn_s32(acc1[0]), vqmovn_s32(acc1[1]));
        let s1 = vcombine_s16(vqmovn_s32(acc1[2]), vqmovn_s32(acc1[3]));
        let sq = vcombine_u8(
            vqshrun_n_s16::<5>(vqdmulhq_s16(s0, s0)),
            vqshrun_n_s16::<5>(vqdmulhq_s16(s1, s1)),
        );
        let re = vcombine_u8(vqshrun_n_s16::<6>(s0), vqshrun_n_s16::<6>(s1));
        vst1q_s8(a1.0.as_mut_ptr(), vaddq_s8(vreinterpretq_s8_u8(sq), shift));
        vst1q_s8(
            a1.0.as_mut_ptr().add(16),
            vaddq_s8(vreinterpretq_s8_u8(re), shift),
        );

        // L2: 32 -> 64, sixteen accumulators.
        let l2w = self.l2_w.as_ptr().add(st * L2_CHUNKS * 4 * SO_L2);
        let mut acc2: [int32x4_t; 16] = [vdupq_n_s32(0); 16];
        for j in 0..16 {
            acc2[j] = vld1q_s32(self.l2_b.as_ptr().add(st * SO_L2 + j * 4));
        }
        for g in 0..L2_IN / 16 {
            let xv = vld1q_s8(a1.0.as_ptr().add(g * 16));
            let wp = l2w.add(g * 4 * SO_L2 * 4);
            for j in 0..16 {
                acc2[j] = vdotq_laneq_s32::<0>(acc2[j], vld1q_s8(wp.add(j * 16)), xv);
                acc2[j] = vdotq_laneq_s32::<1>(acc2[j], vld1q_s8(wp.add(256 + j * 16)), xv);
                acc2[j] = vdotq_laneq_s32::<2>(acc2[j], vld1q_s8(wp.add(512 + j * 16)), xv);
                acc2[j] = vdotq_laneq_s32::<3>(acc2[j], vld1q_s8(wp.add(768 + j * 16)), xv);
            }
        }
        // Squared clipped, to a byte, shifted; then the stack's inputs follow.
        let mut xo = Buf::<OUT_IN>([0; OUT_IN]);
        let top = vdupq_n_s32(255 << 6);
        let zero = vdupq_n_s32(0);
        for j in 0..8 {
            let v0 = vqmovn_s32(vmaxq_s32(vminq_s32(acc2[2 * j], top), zero));
            let v1 = vqmovn_s32(vmaxq_s32(vminq_s32(acc2[2 * j + 1], top), zero));
            let v = vcombine_s16(v0, v1);
            let p = vqshrun_n_s16::<5>(vqdmulhq_s16(v, v));
            vst1_s8(
                xo.0.as_mut_ptr().add(j * 8),
                vadd_s8(vreinterpret_s8_u8(p), vget_low_s8(shift)),
            );
        }
        std::ptr::copy_nonoverlapping(x, xo.0.as_mut_ptr().add(SO_L2), SO_SKIP);

        // Output: 320 -> 1, in eight widening accumulators.
        let ow = self.out_w.as_ptr().add(st * OUT_IN);
        let xp = xo.0.as_ptr();
        let mut o: [int32x4_t; 8] = [vdupq_n_s32(0); 8];
        for g in 0..OUT_IN / 32 {
            let xv = vld1q_s8_x2(xp.add(g * 32));
            let wv = vld1q_s16_x4(ow.add(g * 32));
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
        let s = vaddq_s32(
            vaddq_s32(vaddq_s32(o[0], o[1]), vaddq_s32(o[2], o[3])),
            vaddq_s32(vaddq_s32(o[4], o[5]), vaddq_s32(o[6], o[7])),
        );
        vaddvq_s32(s) + self.out_b[st]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;
    use crate::pattern::NNUE_PATTERNS;
    use crate::position::Position;

    /// Plays a game and hands every position to `f`, alternating colours.
    fn walk_game(nn: &Nnue, mut f: impl FnMut(&Board, usize)) {
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
        let _ = nn;
    }

    /// The scalar kernel's result for the position, in discs.
    fn scalar_discs(nn: &Nnue, board: &Board) -> f32 {
        let ix = nn.indices(board.black, board.white);
        let stage = crate::linear::Linear::stage(board);
        let rows = StackQ::rows(nn, &ix);
        let (ft, pa) = if board.player() == Color::Black {
            (&nn.sq.ft_b, &nn.sq.pa_b)
        } else {
            (&nn.sq.ft_w, &nn.sq.pa_w)
        };
        nn.sq.eval_scalar(
            &ft[ft_bucket(stage) * nn.n_feat_bucket * ACC_DIMS..],
            &pa[pa_bucket(stage) * nn.n_feat_bucket * PA_DIMS..],
            &rows,
            nn.n_masks,
            so_stage(stage),
            stage,
            Nnue::mob_index(board),
        ) as f32
            / OUT_PER_DISC
    }

    /// The vector kernel against the scalar one, exactly, for both colours
    /// along a game.
    ///
    /// Only the two integer kernels are compared: on the synthetic model the
    /// f32 read-out sits fully saturated (its output does not move when the
    /// transformer is replaced by noise), and a saturated f32 network and a
    /// saturated byte network agree on nothing. The comparison against f32
    /// needs trained weights, see below.
    #[test]
    fn vector_kernel_matches_scalar() {
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.init_weights();
        nn.quantize();
        walk_game(&nn, |board, ply| {
            let ix = nn.indices(board.black, board.white);
            let q = nn.eval_from_indices(&ix, board);
            let s = scalar_discs(&nn, board);
            assert!(
                (q - s).abs() < 1e-6,
                "vector {q} vs scalar {s} at ply {ply}"
            );
        });
    }

    /// The integer stack against the f32 network it was converted from, on
    /// trained weights (`KUROOBI_STACK_WEIGHTS` names the file).
    ///
    /// The 255/256 factors and the byte rounding, compounded over the fold
    /// and three activations, come to 0.4% of the score. What the bound
    /// really guards is the training: a stage whose L1 weights are all
    /// smaller than 1/64 rounds to an all-zero layer here, and the f32 net
    /// it was converted from does not. The first stacked model trained
    /// without that constraint fails
    /// exactly so -- stage 34's largest L1 weight is 0.0033, and the game
    /// drifts by 15..19 discs from 24 to 29 empties while agreeing to 0.5%
    /// elsewhere. A model this kernel can run has to be trained on the
    /// grid, and this is the test that says whether it was.
    #[test]
    #[ignore]
    fn integer_stack_matches_f32_on_trained_weights() {
        let path = std::env::var("KUROOBI_STACK_WEIGHTS")
            .expect("KUROOBI_STACK_WEIGHTS must name a stacked-read-out weight file");
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.load(std::path::Path::new(&path)).expect("weights load");
        nn.quantize();
        let mut worst = 0.0f32;
        walk_game(&nn, |board, _| {
            let ix = nn.indices(board.black, board.white);
            let f = nn.eval_indices(board, &ix);
            let q = nn.eval_from_indices(&ix, board);
            eprintln!("{} empty: f32 {f:.3} int {q:.3}", board.empty_count());
            worst = worst.max((q - f).abs() / f.abs().max(4.0));
        });
        assert!(
            worst < 5e-2,
            "integer stack drifts {worst} (relative) from f32"
        );
    }
}

#[cfg(test)]
mod probe {
    use super::*;
    use crate::board::Board;
    use crate::nnue::SO_SCORE;
    use crate::pattern::NNUE_PATTERNS;
    use crate::position::Position;

    /// Per-stage range of every layer of a trained model, and of the
    /// activations it produces on random games, against the integer grid.
    #[test]
    #[ignore]
    fn stage_ranges() {
        let path = std::env::var("KUROOBI_STACK_WEIGHTS").expect("KUROOBI_STACK_WEIGHTS");
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.load(std::path::Path::new(&path)).expect("weights load");
        nn.quantize();
        const S: usize = crate::nnue::SO_STAGES;
        // [stage][what]: max |l1 pre|, max |l2 pre|, n
        let mut l1max = vec![0f32; S];
        let mut l1pos = vec![0f32; S];
        let mut l2max = vec![0f32; S];
        // Mean |contribution| to the output, in discs: hidden path, skip path.
        let mut hid = vec![0f64; S];
        let mut skip = vec![0f64; S];
        let mut n = vec![0usize; S];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let mut board = Board::new();
            loop {
                let stage = crate::linear::Linear::stage(&board);
                let st = so_stage(stage);
                let ix = nn.indices(board.black, board.white);
                let feats = nn.features_player(&ix, board.player(), stage);
                let mut raw = [0.0f32; ACC_DIMS];
                for &f in feats.iter().take(nn.n_masks) {
                    let row = &nn.ft[f as usize * ACC_DIMS..(f as usize + 1) * ACC_DIMS];
                    for h in 0..ACC_DIMS {
                        raw[h] += row[h];
                    }
                }
                for h in 0..ACC_DIMS {
                    raw[h] += nn.ft_bias[h];
                }
                let acc = crate::nnue::fold_pairs_f32(&raw);
                let (pa, _) = nn.pa_forward(&feats, stage);
                let xin = crate::nnue::stack_inputs(&acc, &pa);
                let mob = crate::nnue::mob_unit(Nnue::mob_index(&board));
                let mut a1 = [0f32; SO_L1 * 2];
                for i in 0..SO_L1 {
                    let row = &nn.so_l1_w[(st * SO_L1 + i) * crate::nnue::SO_L1_IN..];
                    let mut x = nn.so_l1_b[st * SO_L1 + i];
                    for (k, &v) in xin.iter().enumerate() {
                        x += row[k] * v;
                    }
                    x += row[SO_SKIP] * mob;
                    l1max[st] = l1max[st].max(x.abs());
                    l1pos[st] = l1pos[st].max(x);
                    a1[i] = (x * x * crate::nnue::ACT_SCALE).clamp(0.0, 1.0);
                    a1[SO_L1 + i] = x.clamp(0.0, 1.0);
                }
                let ow = &nn.so_out_w[st * OUT_IN..];
                let mut h = 0f32;
                for j in 0..SO_L2 {
                    let row = &nn.so_l2_w[(st * SO_L2 + j) * (SO_L1 * 2)..];
                    let mut x = nn.so_l2_b[st * SO_L2 + j];
                    for (i, a) in a1.iter().enumerate() {
                        x += row[i] * a;
                    }
                    l2max[st] = l2max[st].max(x);
                    let c = x.clamp(0.0, 1.0);
                    h += ow[j] * c * c * crate::nnue::ACT_SCALE;
                }
                let mut s = 0f32;
                for (k, &v) in xin.iter().enumerate() {
                    s += ow[SO_L2 + k] * v;
                }
                hid[st] += (h * SO_SCORE).abs() as f64;
                skip[st] += (s * SO_SCORE).abs() as f64;
                n[st] += 1;
                let moves = board.movable();
                if moves == 0 {
                    board.pass();
                    if board.movable() == 0 {
                        break;
                    }
                    continue;
                }
                let cnt = moves.count_ones() as u64;
                let pick = (rnd() % cnt) as u32;
                let mut m = moves;
                for _ in 0..pick {
                    m &= m - 1;
                }
                board.make_move_bits(Position::from_index(m.trailing_zeros()).unwrap());
            }
        }
        eprintln!("st   n   |l1w|max  |l1b|max  |l2w|max  |l2b|max  |ow|max  l1pre_max l1pre_pos l2pre_max  |hid|  |skip|  ob");
        for st in 0..S {
            let amax = |v: &[f32]| v.iter().fold(0f32, |m, x| m.max(x.abs()));
            let l1w = amax(
                &nn.so_l1_w
                    [st * SO_L1 * crate::nnue::SO_L1_IN..(st + 1) * SO_L1 * crate::nnue::SO_L1_IN],
            );
            let l1b = amax(&nn.so_l1_b[st * SO_L1..(st + 1) * SO_L1]);
            let l2w = amax(&nn.so_l2_w[st * SO_L2 * SO_L1 * 2..(st + 1) * SO_L2 * SO_L1 * 2]);
            let l2b = amax(&nn.so_l2_b[st * SO_L2..(st + 1) * SO_L2]);
            let ow = amax(&nn.so_out_w[st * OUT_IN..(st + 1) * OUT_IN]);
            eprintln!(
                "{st:2} {:4} {l1w:9.5} {l1b:9.5} {l2w:9.5} {l2b:9.5} {ow:8.5} {:9.4} {:9.4} {:9.4} {:6.2} {:6.2} {:6.2}",
                n[st],
                l1max[st],
                l1pos[st],
                l2max[st],
                hid[st] / n[st].max(1) as f64,
                skip[st] / n[st].max(1) as f64,
                nn.so_out_b[st] * SO_SCORE,
            );
        }
    }
}
