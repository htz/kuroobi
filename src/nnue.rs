//! NNUE-style non-linear evaluator built on the existing pattern features.

#![allow(clippy::needless_range_loop)]

use crate::board::Board;
use crate::color::Color;
use crate::linear::STAGE_COUNT;
use crate::pattern::Pattern;
use crate::pattern_index::{PatternIndexer, PatternIndices, MAX_MASKS};
use crate::position::Position;

#[cfg(feature = "gpu")]
pub mod gpu;
mod stack_q;

pub const H: usize = 128;

pub const FT_BUCKETS: usize = 1;

#[inline]
pub const fn ft_bucket(stage: usize) -> usize {
    stage * FT_BUCKETS / STAGE_COUNT
}

pub const ACC_DIMS: usize = 2 * H;

const H2: usize = 2 * ACC_DIMS;

const FT_BIAS_LEN: usize = ACC_DIMS;

const PAIR_CLAMP: f32 = 1.0;

const ACT_SCALE: f32 = 255.0 / 256.0;

const FT_CLAMP: f32 = 32.0;

const FT_CLIP_BUDGET: f32 = 1e-4;

const NUM_TABLE_SIZE: usize = 65;

const MLP_H1: usize = 16;
const MLP_H2: usize = 16;

const SO_STAGES: usize = 60;

#[inline]
fn so_stage(stage: usize) -> usize {
    stage.min(SO_STAGES - 1)
}

const SO_L1: usize = 16;
const SO_L2: usize = 64;
const SO_SCORE: f32 = 64.0;

const PA_DIMS: usize = 128;
const PA_BUCKETS: usize = 6;

#[inline]
fn pa_bucket(stage: usize) -> usize {
    (stage / (SO_STAGES / PA_BUCKETS)).min(PA_BUCKETS - 1)
}

const SO_SKIP: usize = H + PA_DIMS;
const SO_L1_IN: usize = SO_SKIP + 1;
const SO_OUT_IN: usize = SO_L2 + SO_SKIP;

const SO_SIZES: (usize, usize, usize, usize, usize, usize) = (
    SO_STAGES * SO_L1 * SO_L1_IN,
    SO_STAGES * SO_L1,
    SO_STAGES * SO_L2 * (SO_L1 * 2),
    SO_STAGES * SO_L2,
    SO_STAGES * SO_OUT_IN,
    SO_STAGES,
);

const PAIR_CLAMP_F32: f32 = PAIR_CLAMP;

#[inline]
fn mob_unit(mob: usize) -> f32 {
    (mob as f32 * (7.0 / 255.0)).min(1.0)
}

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

const GRAD_CLIP_NORM: f32 = 1.0;

const MOB_BUCKETS: usize = 24;

const HALF: usize = H / 2;

const PROD_CLAMP: f32 = 16.0;

const ACT_CLAMP: f32 = 16.0;

pub const ACT_UNITS: f32 = 16.0;

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
        const _: () = assert!(
            ACC_DIMS.is_multiple_of(16),
            "ACC_DIMS must be a multiple of 16"
        );
        const VEC: usize = ACC_DIMS.div_ceil(8);
        const PARTS: usize = if 16 / VEC == 0 { 1 } else { 16 / VEC };
        const CHUNKS: usize = ACC_DIMS / 16;
        let mut p: [[int16x8_t; VEC]; PARTS] = [[vdupq_n_s16(0); VEC]; PARTS];
        const PREFETCH_AHEAD: usize = 8;

        let mut m = 0;
        while m + PARTS <= n {
            if m + PREFETCH_AHEAD < n {
                for k in 0..PARTS {
                    let ptr = row(ft, mask_off, raw, m + PREFETCH_AHEAD + k) as *const u8;
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

#[derive(Clone)]
pub struct Accumulator {
    indices: PatternIndices,
    acc: [i16; H2],
}

pub struct NnueView {
    ft: *mut f32,
    ft_bias: *mut f32,
    out_w: *mut f32,
    out_b: *mut f32,
    num_w: *mut f32,
    pw: *mut f32,
    mob_w: *mut f32,
}
unsafe impl Send for NnueView {}
unsafe impl Sync for NnueView {}

pub struct AdamState {
    pub beta1: f32,
    pub beta2: f32,
    pub eps: f32,
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
    m_so: Vec<Vec<f32>>,
    v_so: Vec<Vec<f32>>,
    grad_scratch: Vec<f32>,
    row_stamp: Vec<u32>,
    ft_last: Vec<u32>,
    pa_last: Vec<u32>,
    m_pa: Vec<f32>,
    v_pa: Vec<f32>,
    m_pa_bias: Vec<f32>,
    v_pa_bias: Vec<f32>,
    pa_scratch: Vec<f32>,
    pa_stamp: Vec<u32>,
    pa_touched: Vec<Vec<u32>>,
    t: u32,
    pub legacy_optimizer: bool,
    la_k: u32,
    la_alpha: f32,
    la_step: u32,
    la_slow: Vec<Vec<f32>>,
    stamp_cur: u32,
    touched: Vec<Vec<u32>>,
}

macro_rules! adam_flat_tables {
    ($s:ident, $f:ident) => {
        $f!($s.m_ft);
        $f!($s.v_ft);
        $f!($s.m_ft_bias);
        $f!($s.v_ft_bias);
        $f!($s.m_out_w);
        $f!($s.v_out_w);
        $f!($s.m_out_b);
        $f!($s.v_out_b);
        $f!($s.m_num_w);
        $f!($s.v_num_w);
        $f!($s.m_pw);
        $f!($s.v_pw);
        $f!($s.m_mob_w);
        $f!($s.v_mob_w);
        $f!($s.m_mlp_l1_w);
        $f!($s.v_mlp_l1_w);
        $f!($s.m_mlp_l1_b);
        $f!($s.v_mlp_l1_b);
        $f!($s.m_mlp_l2_w);
        $f!($s.v_mlp_l2_w);
        $f!($s.m_mlp_l2_b);
        $f!($s.v_mlp_l2_b);
        $f!($s.m_mlp_out_w);
        $f!($s.v_mlp_out_w);
        $f!($s.m_mlp_mob_w);
        $f!($s.v_mlp_mob_w);
        $f!($s.m_pa);
        $f!($s.v_pa);
        $f!($s.m_pa_bias);
        $f!($s.v_pa_bias);
    };
}

macro_rules! adam_nested_tables {
    ($s:ident, $f:ident) => {
        $f!($s.m_so);
        $f!($s.v_so);
        $f!($s.la_slow);
    };
}

impl AdamState {
    pub fn write_state(&self, w: &mut impl std::io::Write) -> std::io::Result<()> {
        for v in [self.beta1, self.beta2, self.eps, self.wd, self.la_alpha] {
            w.write_all(&v.to_le_bytes())?;
        }
        for v in [self.t, self.la_k, self.la_step, self.stamp_cur] {
            w.write_all(&v.to_le_bytes())?;
        }
        w.write_all(&[u8::from(self.legacy_optimizer)])?;
        let me = self;
        macro_rules! put {
            ($t:expr) => {{
                w.write_all(&($t.len() as u64).to_le_bytes())?;
                for x in $t.iter() {
                    w.write_all(&x.to_le_bytes())?;
                }
            }};
        }
        adam_flat_tables!(me, put);
        put!(me.ft_last);
        put!(me.pa_last);
        macro_rules! put_nested {
            ($t:expr) => {{
                w.write_all(&($t.len() as u64).to_le_bytes())?;
                for inner in $t.iter() {
                    put!(inner);
                }
            }};
        }
        adam_nested_tables!(me, put_nested);
        Ok(())
    }

    pub fn read_state(&mut self, r: &mut impl std::io::Read) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind, Read};
        let r = &mut *r;
        let mut f4 = [0u8; 4];
        let mut f32_of = |r: &mut dyn Read| -> std::io::Result<f32> {
            r.read_exact(&mut f4)?;
            Ok(f32::from_le_bytes(f4))
        };
        self.beta1 = f32_of(r)?;
        self.beta2 = f32_of(r)?;
        self.eps = f32_of(r)?;
        self.wd = f32_of(r)?;
        self.la_alpha = f32_of(r)?;
        let mut u4 = [0u8; 4];
        let mut u32_of = |r: &mut dyn Read| -> std::io::Result<u32> {
            r.read_exact(&mut u4)?;
            Ok(u32::from_le_bytes(u4))
        };
        self.t = u32_of(r)?;
        self.la_k = u32_of(r)?;
        self.la_step = u32_of(r)?;
        self.stamp_cur = u32_of(r)?;
        let mut b1 = [0u8; 1];
        r.read_exact(&mut b1)?;
        self.legacy_optimizer = b1[0] != 0;

        fn take_len(r: &mut dyn Read) -> std::io::Result<usize> {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            Ok(u64::from_le_bytes(b) as usize)
        }
        fn fill_f32(r: &mut dyn Read, dst: &mut [f32]) -> std::io::Result<()> {
            let n = take_len(r)?;
            if n != dst.len() {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "checkpoint table is {n} cells, this model wants {}",
                        dst.len()
                    ),
                ));
            }
            let mut b = [0u8; 4];
            for x in dst.iter_mut() {
                r.read_exact(&mut b)?;
                *x = f32::from_le_bytes(b);
            }
            Ok(())
        }
        fn fill_u32(r: &mut dyn Read, dst: &mut [u32]) -> std::io::Result<()> {
            let n = take_len(r)?;
            if n != dst.len() {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "checkpoint table is {n} cells, this model wants {}",
                        dst.len()
                    ),
                ));
            }
            let mut b = [0u8; 4];
            for x in dst.iter_mut() {
                r.read_exact(&mut b)?;
                *x = u32::from_le_bytes(b);
            }
            Ok(())
        }

        let me = self;
        macro_rules! get {
            ($t:expr) => {
                fill_f32(r, &mut $t)?
            };
        }
        adam_flat_tables!(me, get);
        fill_u32(r, &mut me.ft_last)?;
        fill_u32(r, &mut me.pa_last)?;
        macro_rules! get_nested {
            ($t:expr) => {{
                let outer = take_len(r)?;
                if $t.len() != outer {
                    $t.resize(outer, Vec::new());
                }
                for inner in $t.iter_mut() {
                    let n = take_len(r)?;
                    if inner.len() != n {
                        inner.resize(n, 0.0);
                    }
                    let mut b = [0u8; 4];
                    for x in inner.iter_mut() {
                        r.read_exact(&mut b)?;
                        *x = f32::from_le_bytes(b);
                    }
                }
            }};
        }
        adam_nested_tables!(me, get_nested);
        Ok(())
    }

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
unsafe impl Send for AdamView {}
unsafe impl Sync for AdamView {}

impl AdamView {
    #[inline]
    unsafe fn step(&self, m: *mut f32, v: *mut f32, i: usize, grad: f32, lr: f32) -> f32 {
        let mp = m.add(i);
        let vp = v.add(i);
        *mp = self.beta1 * *mp + (1.0 - self.beta1) * grad;
        *vp = self.beta2 * *vp + (1.0 - self.beta2) * grad * grad;
        lr * *mp / ((*vp).sqrt() + self.eps)
    }
}

#[inline]
fn stack_inputs(acc: &[f32; H], pa: &[f32; PA_DIMS]) -> [f32; SO_SKIP] {
    let mut xin = [0.0f32; SO_SKIP];
    for h in 0..H {
        xin[h] = so_act(acc[h]);
    }
    xin[H..].copy_from_slice(pa);
    xin
}

fn replicate_stage0(t: &mut [f32], per_stage: usize) {
    for st in 1..SO_STAGES {
        let (head, tail) = t.split_at_mut(st * per_stage);
        tail[..per_stage].copy_from_slice(&head[..per_stage]);
    }
}

#[inline]
fn so_act(a: f32) -> f32 {
    (a * (1.0 / PAIR_CLAMP_F32)).clamp(0.0, 1.0)
}

#[inline]
fn so_act_grad(a: f32) -> f32 {
    let x = a * (1.0 / PAIR_CLAMP_F32);
    if x > 0.0 && x < 1.0 {
        1.0 / PAIR_CLAMP_F32
    } else {
        0.0
    }
}

#[inline]
fn fold_pairs_f32(raw: &[f32; ACC_DIMS]) -> [f32; H] {
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

#[inline]
fn fold_pairs_back(raw: &[f32; ACC_DIMS], dfolded: &[f32; H]) -> [f32; ACC_DIMS] {
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

#[derive(Clone, Copy)]
struct FtCells {
    scratch: *mut f32,
    stamp: *mut u32,
    m: *mut f32,
    v: *mut f32,
    w: *mut f32,
    last: *mut u32,
}
unsafe impl Send for FtCells {}
unsafe impl Sync for FtCells {}

type Grad = fn(&GradSink) -> &[f32];
type RowBlock<const D: usize> = Vec<(u32, [f32; D])>;
type DenseParam<'a> = (
    &'a mut Vec<f32>,
    &'a mut Vec<f32>,
    &'a mut Vec<f32>,
    Grad,
    f32,
);

/// Same order as [`Nnue::dense_lens`].
const DENSE_GRADS: [Grad; 19] = [
    |s| s.out_w.as_slice(),
    |s| s.out_b.as_slice(),
    |s| s.num_w.as_slice(),
    |s| s.mob_w.as_slice(),
    |s| s.ft_bias.as_slice(),
    |s| s.pw.as_slice(),
    |s| s.mlp_l1_w.as_slice(),
    |s| s.mlp_l1_b.as_slice(),
    |s| s.mlp_l2_w.as_slice(),
    |s| s.mlp_l2_b.as_slice(),
    |s| s.mlp_out_w.as_slice(),
    |s| s.mlp_mob_w.as_slice(),
    |s| s.pa_bias.as_slice(),
    |s| s.so_l1_w.as_slice(),
    |s| s.so_l1_b.as_slice(),
    |s| s.so_l2_w.as_slice(),
    |s| s.so_l2_b.as_slice(),
    |s| s.so_out_w.as_slice(),
    |s| s.so_out_b.as_slice(),
];

/// (gradient, clip to int8 range, weight decay)
const SO_GRADS: [(Grad, bool, bool); 6] = [
    (|s| s.so_l1_w.as_slice(), true, true),
    (|s| s.so_l1_b.as_slice(), false, false),
    (|s| s.so_l2_w.as_slice(), true, true),
    (|s| s.so_l2_b.as_slice(), false, false),
    (|s| s.so_out_w.as_slice(), false, true),
    (|s| s.so_out_b.as_slice(), false, false),
];

struct Adw {
    b1: f32,
    b2: f32,
    eps: f32,
    bc1: f32,
    bc2s: f32,
    legacy: bool,
    lr: f32,
    wd: f32,
    scale: f32,
}

impl Adw {
    fn grad(&self, sinks: &[GradSink], get: Grad, i: usize) -> f32 {
        sinks.iter().map(|s| get(s)[i]).sum::<f32>() * self.scale
    }

    fn step(&self, m: &mut f32, v: &mut f32, g: f32, w: &mut f32, decay: f32) {
        if self.legacy && g == 0.0 {
            return;
        }
        *m = self.b1 * *m + (1.0 - self.b1) * g;
        *v = self.b2 * *v + (1.0 - self.b2) * g * g;
        if self.legacy {
            *w -= self.lr * *m / ((*v).sqrt() + self.eps) + decay * self.lr * *w;
            return;
        }
        *w = *w * (1.0 - decay * self.lr)
            - (self.lr / self.bc1) * *m / ((*v).sqrt() / self.bc2s + self.eps);
    }

    fn dense(
        &self,
        w: &mut [f32],
        m: &mut [f32],
        v: &mut [f32],
        sinks: &[GradSink],
        get: Grad,
        decay: f32,
    ) {
        for i in 0..w.len() {
            let g = self.grad(sinks, get, i);
            self.step(&mut m[i], &mut v[i], g, &mut w[i], decay);
        }
    }
}

/// Returns the squared norm of the summed gradient.
fn reduce_rows<const D: usize>(
    cells: FtCells,
    cur: u32,
    touched: &mut [Vec<u32>],
    sinks: &[GradSink],
    rows: fn(&GradSink) -> &[RowBlock<D>],
    scale: f32,
) -> f64 {
    let mut sq = 0.0f64;
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (p, tv) in touched.iter_mut().enumerate() {
            handles.push(scope.spawn(move || {
                #[allow(clippy::redundant_locals)]
                let cells = cells;
                tv.clear();
                let mut acc = 0.0f64;
                unsafe {
                    for s in sinks.iter() {
                        for &(row, vals) in rows(s)[p].iter() {
                            let r = row as usize;
                            let base = r * D;
                            if *cells.stamp.add(r) != cur {
                                *cells.stamp.add(r) = cur;
                                tv.push(row);
                                std::ptr::copy_nonoverlapping(
                                    vals.as_ptr(),
                                    cells.scratch.add(base),
                                    D,
                                );
                            } else {
                                for h in 0..D {
                                    *cells.scratch.add(base + h) += vals[h];
                                }
                            }
                        }
                    }
                    for &row in tv.iter() {
                        let base = row as usize * D;
                        for h in 0..D {
                            let g = *cells.scratch.add(base + h) * scale;
                            acc += (g as f64) * (g as f64);
                        }
                    }
                }
                acc
            }));
        }
        for h in handles {
            sq += h.join().unwrap();
        }
    });
    sq
}

fn apply_rows<const D: usize>(cells: FtCells, touched: &[Vec<u32>], adw: &Adw, t_now: u32) {
    std::thread::scope(|scope| {
        for tv in touched.iter() {
            scope.spawn(move || {
                #[allow(clippy::redundant_locals)]
                let cells = cells;
                unsafe {
                    for &row in tv.iter() {
                        let base = row as usize * D;
                        let lp = cells.last.add(row as usize);
                        if !adw.legacy {
                            catch_up(
                                cells.m.add(base),
                                cells.v.add(base),
                                cells.w.add(base),
                                D,
                                *lp,
                                t_now,
                                adw.lr,
                                adw.wd,
                                adw.b1,
                                adw.b2,
                                adw.eps,
                            );
                        }
                        *lp = t_now;
                        for h in 0..D {
                            let g = *cells.scratch.add(base + h) * adw.scale;
                            adw.step(
                                &mut *cells.m.add(base + h),
                                &mut *cells.v.add(base + h),
                                g,
                                &mut *cells.w.add(base + h),
                                adw.wd,
                            );
                        }
                    }
                }
            });
        }
    });
}
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
    pub mlp_mob_w: Vec<f32>,
    pub so_l1_w: Vec<f32>,
    pub so_l1_b: Vec<f32>,
    pub so_l2_w: Vec<f32>,
    pub so_l2_b: Vec<f32>,
    pub so_out_w: Vec<f32>,
    pub so_out_b: Vec<f32>,
    pub ft_bias: Vec<f32>,
    pub pa_bias: Vec<f32>,
    pub pw: Vec<f32>,
    pub ft_rows: Vec<Vec<(u32, [f32; ACC_DIMS])>>,
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
    out.retain(|perm| perm.iter().enumerate().any(|(j, &t)| j != t));
    out
}

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MpcSigma {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub qa: f32,
    pub qb: f32,
    pub qc: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MpcAlpha {
    pub depths: Vec<u32>,
    pub empties: Vec<u32>,
    /// `[depth][empties][own bucket][opp bucket]`, row-major.
    pub cells: Vec<f32>,
}

impl MpcAlpha {
    pub const BUCKETS: usize = 4;
    const MAGIC: &'static [u8; 8] = b"MPCALPHA";

    pub fn bucket(mobility: u32) -> usize {
        match mobility {
            0..=5 => 0,
            6..=8 => 1,
            9..=11 => 2,
            _ => 3,
        }
    }

    fn cell(&self, d: usize, e: usize, own: usize, opp: usize) -> f32 {
        let b = Self::BUCKETS;
        self.cells[((d * self.empties.len() + e) * b + own) * b + opp]
    }

    fn along(points: &[u32], x: u32) -> (usize, usize, f32) {
        if x <= points[0] {
            return (0, 0, 0.0);
        }
        for i in 1..points.len() {
            if x <= points[i] {
                let f = (x - points[i - 1]) as f32 / (points[i] - points[i - 1]) as f32;
                return (i - 1, i, f);
            }
        }
        let last = points.len() - 1;
        (last, last, 0.0)
    }

    pub fn value(&self, depth: u32, empties: u32, own_mobility: u32, opp_mobility: u32) -> f32 {
        let (own, opp) = (Self::bucket(own_mobility), Self::bucket(opp_mobility));
        let (d0, d1, fd) = Self::along(&self.depths, depth);
        let (e0, e1, fe) = Self::along(&self.empties, empties);
        let at = |d| self.cell(d, e0, own, opp) * (1.0 - fe) + self.cell(d, e1, own, opp) * fe;
        at(d0) * (1.0 - fd) + at(d1) * fd
    }

    fn write_to(&self, w: &mut impl std::io::Write) -> std::io::Result<()> {
        w.write_all(Self::MAGIC)?;
        w.write_all(&(self.depths.len() as u32).to_le_bytes())?;
        w.write_all(&(self.empties.len() as u32).to_le_bytes())?;
        for &v in self.depths.iter().chain(&self.empties) {
            w.write_all(&v.to_le_bytes())?;
        }
        for &v in &self.cells {
            w.write_all(&v.to_le_bytes())?;
        }
        Ok(())
    }

    /// `None` at the end of the file: weights written before the table existed.
    fn read_from(r: &mut impl std::io::Read) -> std::io::Result<Option<MpcAlpha>> {
        let mut magic = [0u8; 8];
        match r.read_exact(&mut magic) {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
            Ok(()) => {}
        }
        if &magic != Self::MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unknown section after the weights",
            ));
        }
        let mut u = [0u8; 4];
        let mut next = |r: &mut dyn std::io::Read| -> std::io::Result<[u8; 4]> {
            r.read_exact(&mut u)?;
            Ok(u)
        };
        let nd = u32::from_le_bytes(next(r)?) as usize;
        let ne = u32::from_le_bytes(next(r)?) as usize;
        let mut depths = Vec::with_capacity(nd);
        for _ in 0..nd {
            depths.push(u32::from_le_bytes(next(r)?));
        }
        let mut empties = Vec::with_capacity(ne);
        for _ in 0..ne {
            empties.push(u32::from_le_bytes(next(r)?));
        }
        let n = nd * ne * Self::BUCKETS * Self::BUCKETS;
        let mut cells = Vec::with_capacity(n);
        for _ in 0..n {
            cells.push(f32::from_le_bytes(next(r)?));
        }
        if nd == 0 || ne == 0 {
            return Ok(None);
        }
        Ok(Some(MpcAlpha {
            depths,
            empties,
            cells,
        }))
    }
}

impl MpcSigma {
    pub const LEN: usize = 6;

    pub fn from_array(v: [f32; MpcSigma::LEN]) -> MpcSigma {
        MpcSigma {
            a: v[0],
            b: v[1],
            c: v[2],
            qa: v[3],
            qb: v[4],
            qc: v[5],
        }
    }

    pub fn to_array(self) -> [f32; MpcSigma::LEN] {
        [self.a, self.b, self.c, self.qa, self.qb, self.qc]
    }

    #[inline]
    pub fn value(&self, empties: u32, depth: u32, pc_depth: u32) -> f32 {
        let s = self.a * empties as f32 + self.b * depth as f32 + self.c * pc_depth as f32;
        (self.qa * s * s + self.qb * s + self.qc).max(1.0)
    }
}

pub struct Nnue {
    base: Option<Box<crate::linear::Linear>>,
    patterns: &'static [Pattern],
    indexer: PatternIndexer,
    n_masks: usize,
    mask_off: Vec<u32>,
    n_feat_bucket: usize,
    n_features: usize,

    ft: Vec<f32>,
    ft_bias: Vec<f32>,
    pub pa: Vec<f32>,
    pub pa_bias: Vec<f32>,
    out_w: Vec<f32>,
    out_b: Vec<f32>,
    num_w: Vec<f32>,
    mlp_l1_w: Vec<f32>,
    mlp_l1_b: Vec<f32>,
    mlp_l2_w: Vec<f32>,
    mlp_l2_b: Vec<f32>,
    mlp_out_w: Vec<f32>,
    mlp_mob_w: Vec<f32>,
    so_l1_w: Vec<f32>,
    so_l1_b: Vec<f32>,
    so_l2_w: Vec<f32>,
    so_l2_b: Vec<f32>,
    so_out_w: Vec<f32>,
    so_out_b: Vec<f32>,
    so_l1_wq: Vec<f32>,
    so_l2_wq: Vec<f32>,
    so_grid: bool,

    mob_w: Vec<f32>,
    pw: Vec<f32>,

    ftc_i16: Vec<i16>,
    ft_b_i8: Vec<i8>,
    ft_w_i8: Vec<i8>,
    ft_clipped: usize,
    has_pw: bool,
    has_head: bool,
    mlp_l1_w_i8: Vec<i8>,
    mlp_l1_rowsum: Vec<i32>,
    mlp_l1_dequant: f32,
    act_shift: i16,
    ft_bias_i16: Vec<i16>,
    out_w_i16: Vec<i16>,
    pw_i16: Vec<i16>,
    prod_scale: f32,
    prod_clamp_q: i32,
    act_clamp_q: i16,
    act_shift_q: i16,
    pub head_f32: bool,
    pub act_units: f32,
    out_scale: f32,

    ft_scale: f32,
    sq: stack_q::StackQ,

    mpc_sigma: Option<MpcSigma>,
    mpc_alpha: Option<MpcAlpha>,
}

impl Nnue {
    pub fn new(patterns: &'static [Pattern]) -> Nnue {
        let indexer = PatternIndexer::new(patterns);
        let n_masks = indexer.n_masks();

        let mut pattern_off = Vec::with_capacity(patterns.len());
        let mut off = 0u32;
        for p in patterns {
            pattern_off.push(off);
            off += p.table_size() as u32;
        }
        let n_feat_bucket = off as usize;
        let n_features = n_feat_bucket * FT_BUCKETS;
        let mask_off: Vec<u32> = indexer
            .mask_patterns()
            .iter()
            .map(|&pi| pattern_off[pi as usize])
            .collect();

        Nnue {
            base: None,
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
            so_l1_wq: vec![0.0; SO_SIZES.0],
            so_l2_wq: vec![0.0; SO_SIZES.2],
            so_grid: false,
            pw: vec![0.0; STAGE_COUNT * HALF],
            ftc_i16: Vec::new(),
            ft_b_i8: Vec::new(),
            ft_w_i8: Vec::new(),
            sq: stack_q::StackQ::default(),
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
            out_scale: 0.0,
            ft_scale: 1.0,
            mpc_sigma: None,
            mpc_alpha: None,
        }
    }

    pub fn mpc_sigma(&self) -> Option<MpcSigma> {
        self.mpc_sigma
    }

    pub fn mpc_alpha(&self) -> Option<&MpcAlpha> {
        self.mpc_alpha.as_ref()
    }

    pub fn set_mpc_alpha(&mut self, alpha: Option<MpcAlpha>) {
        self.mpc_alpha = alpha;
    }

    pub fn set_mpc_sigma(&mut self, sigma: Option<MpcSigma>) {
        self.mpc_sigma = sigma;
    }

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

    fn pattern_offset(&self, pi: usize) -> usize {
        let mut off = 0usize;
        for p in self.patterns.iter().take(pi) {
            off += 3usize.pow(p.size as u32);
        }
        off
    }

    pub fn quantize(&mut self) {
        self.has_pw = self.pw.iter().any(|&v| v != 0.0);
        self.has_head = self.mlp_out_w.iter().any(|&v| v != 0.0);

        let ft_max = self.ft.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let w_max = self.out_w.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));

        let bias_max = self.ft_bias.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        let room = 32_000.0 / (self.n_masks as f32 * ft_max + bias_max);
        let mut ft_scale = (2.0f32).powi(room.log2().floor() as i32).max(1.0);

        let clip_fraction = |s: f32| {
            let n = self
                .ft
                .iter()
                .filter(|&&v| (v * s).round().abs() > 127.0)
                .count();
            n as f32 / self.ft.len().max(1) as f32
        };
        let budget: f32 = std::env::var("KUROOBI_FT_CLIP_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(FT_CLIP_BUDGET);
        while ft_scale > 1.0 && clip_fraction(ft_scale) > budget {
            ft_scale *= 0.5;
        }
        self.ft_clipped = (clip_fraction(ft_scale) * self.ft.len() as f32) as usize;

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

        let pw_max = self.pw.iter().fold(1e-6f32, |m, &v| m.max(v.abs()));
        let pw_scale = (16_000.0 / pw_max).min(32_000.0);
        self.pw_i16 = self.pw.iter().map(|&v| q(v, pw_scale)).collect();
        self.prod_scale = 1.0 / (ft_scale * ft_scale * pw_scale * PROD_CLAMP);
        self.prod_clamp_q = ((PROD_CLAMP * ft_scale) as i32).min(32_767);
        let cap = (ACT_CLAMP * ft_scale).min(32_767.0);
        self.act_clamp_q = cap as i16;
        self.act_shift_q = cap.log2().round() as i16;
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
        let act_units = ft_scale / (1 << self.act_shift) as f32;
        self.mlp_l1_dequant = 1.0 / (act_units * l1_scale);

        self.ft_b_i8 = vec![0; self.n_features * ACC_DIMS];
        self.ft_w_i8 = vec![0; self.n_features * ACC_DIMS];
        for (i, &v) in self.ft.iter().enumerate() {
            self.ft_b_i8[i] = (v * ft_scale).round().clamp(-127.0, 127.0) as i8;
        }
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
        {
            self.sq = stack_q::StackQ::build(self);
        }
    }

    pub fn ft_clipped(&self) -> (usize, usize) {
        (self.ft_clipped, self.ft.len())
    }

    pub fn build_incremental_table(&mut self) {
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

    pub fn n_features(&self) -> usize {
        self.n_features
    }

    pub fn init_weights(&mut self) {
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
        self.ft_bias.fill(0.0);
        self.init_mlp_hidden();
        {
            let bound = (6.0 / ACC_DIMS as f32).sqrt() * 0.25;
            for w in &mut self.ft {
                *w = next() * bound;
            }
        }
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
        {
            let b1 = 1.0 / (SO_L1_IN as f32).sqrt();
            for i in 0..SO_L1 * SO_L1_IN {
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
            let bo = 1.0 / (SO_OUT_IN as f32).sqrt();
            for i in 0..SO_OUT_IN {
                self.so_out_w[i] = next() * bo;
            }
            self.so_out_b[0] = 0.0;
            replicate_stage0(&mut self.so_l1_w, SO_L1 * SO_L1_IN);
            replicate_stage0(&mut self.so_l1_b, SO_L1);
            replicate_stage0(&mut self.so_l2_w, SO_L2 * (SO_L1 * 2));
            replicate_stage0(&mut self.so_l2_b, SO_L2);
            replicate_stage0(&mut self.so_out_w, SO_OUT_IN);
            replicate_stage0(&mut self.so_out_b, 1);
            self.refresh_so_grid();
        }
    }

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

    #[inline]
    fn bucket_base(&self, stage: usize) -> u32 {
        (ft_bucket(stage) * self.n_feat_bucket) as u32
    }

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

    #[allow(clippy::needless_return)]
    #[inline]
    pub fn eval_from_indices(&self, indices: &PatternIndices, board: &Board) -> f32 {
        self.net_from_indices(indices, board) + self.base_score(board, indices)
    }

    #[inline]
    fn base_score(&self, board: &Board, indices: &PatternIndices) -> f32 {
        match &self.base {
            Some(e) => e.eval_indices(board, indices),
            None => 0.0,
        }
    }

    pub fn set_base(&mut self, e: crate::linear::Linear) {
        self.base = Some(Box::new(e));
    }

    #[inline]
    pub fn net_from_indices(&self, indices: &PatternIndices, board: &Board) -> f32 {
        let stage = crate::linear::Linear::stage(board);
        let ft = if board.player() == Color::Black {
            &self.ft_b_i8
        } else {
            &self.ft_w_i8
        };
        let mut raw_acc = [0i16; ACC_DIMS]; // bias is added at readout
        let base = ft_bucket(stage) * self.n_feat_bucket * ACC_DIMS;
        unsafe {
            accumulate_rows(
                &mut raw_acc,
                ft.as_ptr().add(base),
                &self.mask_off,
                indices.raw(),
                self.n_masks,
            );
        }
        {
            let _ = raw_acc;
            self.sq
                .eval(self, indices, board.player(), stage, Self::mob_index(board))
        }
    }

    pub fn eval(&self, board: &Board) -> f32 {
        let ix = self.indexer.init(board.black, board.white);
        self.eval_indices(board, &ix)
    }

    pub fn eval_indices(&self, board: &Board, indices: &PatternIndices) -> f32 {
        self.net_indices(board, indices) + self.base_score(board, indices)
    }

    pub fn net_indices(&self, board: &Board, indices: &PatternIndices) -> f32 {
        let stage = crate::linear::Linear::stage(board);
        let feats = self.features_player(indices, board.player(), stage);
        let base = self.forward(&feats, Self::mob_index(board), stage);
        {
            base
        }
    }

    fn pa_forward(
        &self,
        feats: &[u32; MAX_MASKS],
        stage: usize,
    ) -> ([f32; PA_DIMS], [f32; PA_DIMS]) {
        let mut z = [0.0f32; PA_DIMS];
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
        let mut a = [0.0f32; PA_DIMS];
        for (j, av) in a.iter_mut().enumerate() {
            let c = z[j].clamp(0.0, 1.0);
            *av = c * c * ACT_SCALE;
        }
        (a, z)
    }

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

    #[inline]
    pub fn mob_index(board: &Board) -> usize {
        {
            board.movable_count() as usize
        }
    }

    #[allow(clippy::needless_return)]
    fn forward(&self, feats: &[u32; MAX_MASKS], mob: usize, stage: usize) -> f32 {
        let mut raw = [0.0f32; ACC_DIMS];
        for &f in feats.iter().take(self.n_masks) {
            let base = f as usize * ACC_DIMS;
            let row = &self.ft[base..base + ACC_DIMS];
            for h in 0..ACC_DIMS {
                raw[h] += row[h];
            }
        }
        for h in 0..ACC_DIMS {
            raw[h] += self.ft_bias[h];
        }
        let acc = fold_pairs_f32(&raw);
        {
            let (pa, _) = self.pa_forward(feats, stage);
            return self.stacked_readout(&acc, &pa, mob, stage);
        }
    }

    pub fn indexer(&self) -> &PatternIndexer {
        &self.indexer
    }

    pub fn indices(&self, black: u64, white: u64) -> PatternIndices {
        self.indexer.init(black, white)
    }

    #[allow(clippy::needless_return)]
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
        for h in 0..ACC_DIMS {
            raw[h] += self.ft_bias[h];
        }
        let acc = fold_pairs_f32(&raw);
        {
            let (pa, pa_z) = self.pa_forward(&feats, stage);
            let (sq, draw, dpa) =
                self.grad_stacked(&raw, &acc, &pa, &pa_z, discs, mob, target, stage, sink);
            for &f in feats.iter().take(self.n_masks) {
                sink.push_ft(f, &draw);
            }
            {
                let base_row = (pa_bucket(stage) * self.n_feat_bucket) as u32;
                for &f in feats.iter().take(self.n_masks) {
                    sink.push_pa(base_row + f, &dpa);
                }
            }
            return sq;
        }
    }

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
        let mut xin = stack_inputs(acc, pa);
        let grid = |a: f32| (a * 256.0).floor().min(255.0) / 256.0;
        let act = |a: f32| if self.so_grid { grid(a) } else { a };
        if self.so_grid {
            for x in xin.iter_mut() {
                *x = grid(*x);
            }
        }

        debug_assert_eq!(self.so_l1_wq.len(), self.so_l1_w.len());
        let mut l1 = [0.0f32; SO_L1];
        for (i, v) in l1.iter_mut().enumerate() {
            let row = &self.so_l1_wq[(st * SO_L1 + i) * SO_L1_IN..];
            let mut x = self.so_l1_b[st * SO_L1 + i];
            for (k, &xv) in xin.iter().enumerate() {
                x += row[k] * xv;
            }
            *v = x + row[SO_SKIP] * m;
        }
        let mut a1 = [0.0f32; SO_L1 * 2];
        for i in 0..SO_L1 {
            a1[i] = act((l1[i] * l1[i] * ACT_SCALE).clamp(0.0, 1.0));
            a1[SO_L1 + i] = act(l1[i].clamp(0.0, 1.0));
        }
        let mut l2 = [0.0f32; SO_L2];
        let mut v2 = [0.0f32; SO_L2];
        for j in 0..SO_L2 {
            let row = &self.so_l2_wq[(st * SO_L2 + j) * (SO_L1 * 2)..];
            let mut x = self.so_l2_b[st * SO_L2 + j];
            for (i, a) in a1.iter().enumerate() {
                x += row[i] * a;
            }
            l2[j] = x;
            let c = x.clamp(0.0, 1.0);
            v2[j] = act(c * c * ACT_SCALE);
        }
        let ow = &self.so_out_w[st * SO_OUT_IN..];
        let mut unit = self.so_out_b[st];
        for (j, v) in v2.iter().enumerate() {
            unit += ow[j] * v;
        }
        for (k, &xv) in xin.iter().enumerate() {
            unit += ow[SO_L2 + k] * xv;
        }
        let out = unit * SO_SCORE;
        let err_discs = out - target;
        let err = 2.0 * err_discs / SO_SCORE;

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
            let row = &self.so_l2_wq[woff..woff + SO_L1 * 2];
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
            let row = &self.so_l1_wq[woff..woff + SO_L1_IN];
            for (k, &xv) in xin.iter().enumerate() {
                sink.so_l1_w[woff + k] += dl1 * xv;
                dxin[k] += dl1 * row[k];
            }
            sink.so_l1_w[woff + SO_SKIP] += dl1 * m;
        }

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

        let draw = fold_pairs_back(raw, &dacc);
        for h in 0..ACC_DIMS {
            sink.ft_bias[h] += draw[h];
        }
        {
            let off = pa_bucket(stage) * PA_DIMS;
            for j in 0..PA_DIMS {
                sink.pa_bias[off + j] += dpa[j];
            }
        }
        (err_discs * err_discs, draw, dpa)
    }

    pub fn apply_adamw_batch(
        &mut self,
        sinks: &mut [GradSink],
        adam: &mut AdamState,
        lr: f32,
        wd: f32,
        scale: f32,
    ) {
        adam.t = adam.t.saturating_add(1);
        let t_now = adam.t;
        let mut adw = Adw {
            b1: adam.beta1,
            b2: adam.beta2,
            eps: adam.eps,
            bc1: 1.0 - adam.beta1.powi(t_now as i32),
            bc2s: (1.0 - adam.beta2.powi(t_now as i32)).sqrt(),
            legacy: adam.legacy_optimizer,
            lr,
            wd,
            scale,
        };

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
        while adam.pa_touched.len() < parts {
            adam.pa_touched.push(Vec::new());
        }
        let ft_cells = FtCells {
            scratch: adam.grad_scratch.as_mut_ptr(),
            stamp: adam.row_stamp.as_mut_ptr(),
            m: adam.m_ft.as_mut_ptr(),
            v: adam.v_ft.as_mut_ptr(),
            w: self.ft.as_mut_ptr(),
            last: adam.ft_last.as_mut_ptr(),
        };
        let pa_cells = FtCells {
            scratch: adam.pa_scratch.as_mut_ptr(),
            stamp: adam.pa_stamp.as_mut_ptr(),
            m: adam.m_pa.as_mut_ptr(),
            v: adam.v_pa.as_mut_ptr(),
            w: self.pa.as_mut_ptr(),
            last: adam.pa_last.as_mut_ptr(),
        };

        let mut sq = reduce_rows(
            ft_cells,
            cur,
            &mut adam.touched[..parts],
            sinks,
            |s| s.ft_rows.as_slice(),
            scale,
        ) + reduce_rows(
            pa_cells,
            cur,
            &mut adam.pa_touched[..parts],
            sinks,
            |s| s.pa_rows.as_slice(),
            scale,
        );
        for (get, len) in DENSE_GRADS.iter().zip(self.dense_lens()) {
            for i in 0..len {
                let g = adw.grad(sinks, *get, i);
                sq += (g as f64) * (g as f64);
            }
        }
        let norm = sq.sqrt() as f32;
        if norm > GRAD_CLIP_NORM && norm.is_finite() {
            adw.scale = scale * (GRAD_CLIP_NORM / norm);
        }

        let mut dense: [DenseParam; 8] = [
            (
                &mut self.out_w,
                &mut adam.m_out_w,
                &mut adam.v_out_w,
                |s| s.out_w.as_slice(),
                wd,
            ),
            (
                &mut self.out_b,
                &mut adam.m_out_b,
                &mut adam.v_out_b,
                |s| s.out_b.as_slice(),
                0.0,
            ),
            (
                &mut self.num_w,
                &mut adam.m_num_w,
                &mut adam.v_num_w,
                |s| s.num_w.as_slice(),
                0.0,
            ),
            (
                &mut self.mlp_l1_w,
                &mut adam.m_mlp_l1_w,
                &mut adam.v_mlp_l1_w,
                |s| s.mlp_l1_w.as_slice(),
                wd,
            ),
            (
                &mut self.mlp_l1_b,
                &mut adam.m_mlp_l1_b,
                &mut adam.v_mlp_l1_b,
                |s| s.mlp_l1_b.as_slice(),
                0.0,
            ),
            (
                &mut self.mlp_l2_w,
                &mut adam.m_mlp_l2_w,
                &mut adam.v_mlp_l2_w,
                |s| s.mlp_l2_w.as_slice(),
                wd,
            ),
            (
                &mut self.mlp_l2_b,
                &mut adam.m_mlp_l2_b,
                &mut adam.v_mlp_l2_b,
                |s| s.mlp_l2_b.as_slice(),
                0.0,
            ),
            (
                &mut self.mlp_out_w,
                &mut adam.m_mlp_out_w,
                &mut adam.v_mlp_out_w,
                |s| s.mlp_out_w.as_slice(),
                wd,
            ),
        ];
        for (w, m, v, get, decay) in dense.iter_mut() {
            adw.dense(w, m, v, sinks, *get, *decay);
        }

        // int8 quantization caps the second-order head's weights
        const SO_MAX_W: f32 = 127.0 / 64.0;
        let mut so: [&mut Vec<f32>; 6] = [
            &mut self.so_l1_w,
            &mut self.so_l1_b,
            &mut self.so_l2_w,
            &mut self.so_l2_b,
            &mut self.so_out_w,
            &mut self.so_out_b,
        ];
        for (t, w) in so.iter_mut().enumerate() {
            let (get, clip, decay) = SO_GRADS[t];
            adw.dense(
                w,
                &mut adam.m_so[t],
                &mut adam.v_so[t],
                sinks,
                get,
                if decay { wd } else { 0.0 },
            );
            if clip {
                for x in w.iter_mut() {
                    *x = x.clamp(-SO_MAX_W, SO_MAX_W);
                }
            }
        }
        self.refresh_so_grid();

        let mut dense: [DenseParam; 2] = [
            (
                &mut self.mlp_mob_w,
                &mut adam.m_mlp_mob_w,
                &mut adam.v_mlp_mob_w,
                |s| s.mlp_mob_w.as_slice(),
                wd,
            ),
            (
                &mut self.mob_w,
                &mut adam.m_mob_w,
                &mut adam.v_mob_w,
                |s| s.mob_w.as_slice(),
                0.0,
            ),
        ];
        for (w, m, v, get, decay) in dense.iter_mut() {
            adw.dense(w, m, v, sinks, *get, *decay);
        }
        for i in 0..self.ft_bias.len() {
            let g = adw.grad(sinks, |s| s.ft_bias.as_slice(), i);
            let m = &mut adam.m_ft_bias[i];
            let v = &mut adam.v_ft_bias[i];
            *m = adw.b1 * *m + (1.0 - adw.b1) * g;
            *v = adw.b2 * *v + (1.0 - adw.b2) * g * g;
            self.ft_bias[i] -= if adw.legacy {
                lr * *m / ((*v).sqrt() + adw.eps)
            } else {
                (lr / adw.bc1) * *m / ((*v).sqrt() / adw.bc2s + adw.eps)
            };
        }
        adw.dense(
            &mut self.pw,
            &mut adam.m_pw,
            &mut adam.v_pw,
            sinks,
            |s| s.pw.as_slice(),
            wd,
        );

        apply_rows::<ACC_DIMS>(ft_cells, &adam.touched[..parts], &adw, t_now);
        apply_rows::<PA_DIMS>(pa_cells, &adam.pa_touched[..parts], &adw, t_now);
        adw.dense(
            &mut self.pa_bias,
            &mut adam.m_pa_bias,
            &mut adam.v_pa_bias,
            sinks,
            |s| s.pa_bias.as_slice(),
            0.0,
        );
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

    fn dense_lens(&self) -> [usize; 19] {
        [
            self.out_w.len(),
            self.out_b.len(),
            self.num_w.len(),
            self.mob_w.len(),
            self.ft_bias.len(),
            self.pw.len(),
            self.mlp_l1_w.len(),
            self.mlp_l1_b.len(),
            self.mlp_l2_w.len(),
            self.mlp_l2_b.len(),
            self.mlp_out_w.len(),
            self.mlp_mob_w.len(),
            self.pa_bias.len(),
            self.so_l1_w.len(),
            self.so_l1_b.len(),
            self.so_l2_w.len(),
            self.so_l2_b.len(),
            self.so_out_w.len(),
            self.so_out_b.len(),
        ]
    }

    pub fn set_so_grid(&mut self, on: bool) {
        self.so_grid = on;
        self.refresh_so_grid();
    }

    fn refresh_so_grid(&mut self) {
        let round = |w: f32| {
            if self.so_grid {
                (w * 64.0).round() / 64.0
            } else {
                w
            }
        };
        self.so_l1_wq.clear();
        self.so_l1_wq.extend(self.so_l1_w.iter().map(|&w| round(w)));
        self.so_l2_wq.clear();
        self.so_l2_wq.extend(self.so_l2_w.iter().map(|&w| round(w)));
    }

    pub fn settle_adam(&mut self, adam: &mut AdamState, lr: f32, wd: f32) {
        if adam.legacy_optimizer {
            return;
        }
        let (b1, b2, eps, t) = (adam.beta1, adam.beta2, adam.eps, adam.t);
        for r in 0..adam.ft_last.len() {
            let base = r * ACC_DIMS;
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
        self.refresh_so_grid();
    }

    #[inline]
    pub fn ix_apply(&self, ix: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        self.indexer.apply(ix, pos, flipped, mover);
    }

    #[inline]
    pub fn ix_undo(&self, ix: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        self.indexer.undo(ix, pos, flipped, mover);
    }

    pub fn accumulator(&self, board: &Board) -> Accumulator {
        assert!(
            !self.ftc_i16.is_empty(),
            "the incremental accumulator needs build_incremental_table()"
        );
        let indices = self.indexer.init(board.black, board.white);
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

    #[inline]
    fn acc_square(&self, acc: &mut Accumulator, sq: u8, digit_diff: u16) {
        let ftc = self.ftc_i16.as_ptr();
        let raw = acc.indices.raw_mut();
        let vec = &mut acc.acc;
        for e in self.indexer.square_entries(sq) {
            let mask = e.mask as usize;
            let delta = digit_diff.wrapping_mul(e.pow3);
            let old = raw[mask] as usize;
            let new = raw[mask].wrapping_add(delta) as usize;
            raw[mask] = new as u16;
            let base = self.mask_off[mask] as usize;
            unsafe {
                acc_row_addsub(vec, ftc.add((base + new) * H2), ftc.add((base + old) * H2));
            }
        }
    }

    #[allow(clippy::needless_return)]
    #[inline]
    pub fn eval_acc(&self, acc: &Accumulator, board: &Board) -> f32 {
        let stage = crate::linear::Linear::stage(board);
        if FT_BUCKETS > 1 && ft_bucket(stage) != 0 {
            return self.eval_from_indices(&acc.indices, board);
        }
        let mut v = [0i16; ACC_DIMS];
        v.copy_from_slice(if board.player() == Color::Black {
            &acc.acc[0..ACC_DIMS]
        } else {
            &acc.acc[ACC_DIMS..H2]
        });
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
    }

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

        let mut acc = [0.0f32; H];
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

        let err = out - target;
        let g = lr * err;
        self.num_w[num_off] -= g;
        self.mob_w[mob_off] -= g;

        let mut delta = [0.0f32; H];
        for h in 0..H {
            if acc[h] > 0.0 {
                delta[h] = self.out_w[ow_off + h];
                self.out_w[ow_off + h] -= g * acc[h];
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
            self.pw[pw_off + i] -= g * pa[i] * pb[i] * (1.0 / PROD_CLAMP);
        }
        self.out_b[stage] -= g;

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

    pub fn set_num_w(&mut self, v: &[f32]) {
        assert_eq!(v.len(), self.num_w.len(), "num_w length mismatch");
        self.num_w.copy_from_slice(v);
    }

    pub fn num_w_len(&self) -> usize {
        self.num_w.len()
    }

    pub fn ft_len(&self) -> usize {
        self.ft.len()
    }

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

    /// # Safety
    ///
    /// Callers must not write the same weight cell concurrently.
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

    /// # Safety
    ///
    /// Callers must not write the same weight cell concurrently.
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

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        {
            let mut w = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            self.write_to(&mut w)?;
            std::io::Write::flush(&mut w)?;
        }
        std::fs::rename(&tmp, path)
    }

    pub fn write_to(&self, w: &mut impl std::io::Write) -> std::io::Result<()> {
        {
            w.write_all(b"BBRVNN10")?;
            w.write_all(&(ACC_DIMS as u32).to_le_bytes())?;
            w.write_all(&(self.n_features as u32).to_le_bytes())?;
            w.write_all(&(STAGE_COUNT as u32).to_le_bytes())?;
            w.write_all(&(self.pa.len() as u32).to_le_bytes())?;
            let so_len = self.so_l1_w.len()
                + self.so_l1_b.len()
                + self.so_l2_w.len()
                + self.so_l2_b.len()
                + self.so_out_w.len()
                + self.so_out_b.len();
            w.write_all(&(so_len as u32).to_le_bytes())?;
            w.write_all(&u32::from(self.mpc_sigma.is_some()).to_le_bytes())?;
            for &v in &self
                .mpc_sigma
                .unwrap_or(MpcSigma::from_array([0.0; 6]))
                .to_array()
            {
                w.write_all(&v.to_le_bytes())?;
            }
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
        }
        if let Some(a) = &self.mpc_alpha {
            a.write_to(w)?;
        }
        Ok(())
    }

    pub fn import_packed(&mut self, raw: &[u8]) -> std::io::Result<()> {
        use std::io::{Error, ErrorKind};
        self.mpc_sigma = None;
        self.mpc_alpha = None;
        const L1_PAD_IN: usize = 288;
        const L2_PAD_IN: usize = SO_L1 * 2;
        const OUT_PAD_IN: usize = 320;
        let mut at = 0usize;
        let mut take = |n: usize| -> std::io::Result<&[u8]> {
            if at + n > raw.len() {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "packed weights truncated",
                ));
            }
            let s = &raw[at..at + n];
            at += n;
            Ok(s)
        };
        let i16s = |b: &[u8]| -> Vec<i16> {
            b.as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c))
                .collect()
        };
        let i32s = |b: &[u8]| -> Vec<i32> {
            b.as_chunks::<4>()
                .0
                .iter()
                .map(|c| i32::from_le_bytes(*c))
                .collect()
        };
        let nf = self.n_features;
        if self.n_masks != 32 || nf != 297_432 || self.pa.len() != PA_BUCKETS * nf * PA_DIMS {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "import_packed wants the unshared pattern set (--patterns nnue)",
            ));
        }
        for (dst, v) in self.ft_bias.iter_mut().zip(i16s(take(ACC_DIMS * 2)?)) {
            *dst = v as f32 / 512.0;
        }
        for (dst, v) in self.ft.iter_mut().zip(i16s(take(nf * ACC_DIMS * 2)?)) {
            *dst = v as f32 / 512.0;
        }
        for b in 0..PA_BUCKETS {
            let bias = &mut self.pa_bias[b * PA_DIMS..(b + 1) * PA_DIMS];
            for (dst, v) in bias.iter_mut().zip(i16s(take(PA_DIMS * 2)?)) {
                *dst = v as f32 / 512.0;
            }
            let rows = &mut self.pa[b * nf * PA_DIMS..(b + 1) * nf * PA_DIMS];
            for (dst, v) in rows.iter_mut().zip(i16s(take(nf * PA_DIMS * 2)?)) {
                *dst = v as f32 / 512.0;
            }
        }
        for st in 0..SO_STAGES {
            for (o, v) in i32s(take(SO_L1 * 4)?).into_iter().enumerate() {
                self.so_l1_b[st * SO_L1 + o] = v as f32 / (1u32 << 14) as f32;
            }
            let w = take(L1_PAD_IN * SO_L1)?;
            for o in 0..SO_L1 {
                for k in 0..SO_L1_IN {
                    self.so_l1_w[(st * SO_L1 + o) * SO_L1_IN + k] =
                        w[o * L1_PAD_IN + k] as i8 as f32 / 64.0;
                }
            }
            for (o, v) in i32s(take(SO_L2 * 4)?).into_iter().enumerate() {
                self.so_l2_b[st * SO_L2 + o] = v as f32 / (1u32 << 14) as f32;
            }
            let w = take(L2_PAD_IN * SO_L2)?;
            for o in 0..SO_L2 {
                for k in 0..SO_L1 * 2 {
                    self.so_l2_w[(st * SO_L2 + o) * (SO_L1 * 2) + k] =
                        w[o * L2_PAD_IN + k] as i8 as f32 / 64.0;
                }
            }
            let b = i32s(take(4)?)[0];
            self.so_out_b[st] = b as f32 / (1u32 << 20) as f32;
            let w = i16s(take(OUT_PAD_IN * 2)?);
            for k in 0..SO_OUT_IN {
                self.so_out_w[st * SO_OUT_IN + k] = w[k] as f32 / 4096.0;
            }
        }
        if at != raw.len() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("packed weights: {} bytes left over", raw.len() - at),
            ));
        }
        Ok(())
    }

    pub fn load(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let mut r = std::io::BufReader::new(std::fs::File::open(path)?);
        self.read_from(&mut r)
    }

    pub fn read_from(&mut self, r: &mut impl std::io::Read) -> std::io::Result<()> {
        use std::io::Read;
        let mut r = &mut *r;
        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        let (staged_bias, has_num, has_pw, has_mob, has_mlp, has_mlp_mob) = match &magic {
            b"BBRVNN10" | b"BBRVNN09" | b"BBRVNN08" | b"BBRVNN07" => {
                (true, true, true, true, true, true)
            }
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
        let pa_len = if &magic == b"BBRVNN10" || &magic == b"BBRVNN09" {
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
        let so_len = if &magic == b"BBRVNN10" || &magic == b"BBRVNN09" || &magic == b"BBRVNN08" {
            r.read_exact(&mut u)?;
            u32::from_le_bytes(u) as usize
        } else {
            0
        };
        self.mpc_sigma = None;
        if &magic == b"BBRVNN10" {
            r.read_exact(&mut u)?;
            let present = u32::from_le_bytes(u) != 0;
            let mut coef = [0.0f32; MpcSigma::LEN];
            for c in coef.iter_mut() {
                r.read_exact(&mut u)?;
                *c = f32::from_le_bytes(u);
            }
            if present {
                self.mpc_sigma = Some(MpcSigma::from_array(coef));
            }
        }
        let want_so = self.so_l1_w.len()
            + self.so_l1_b.len()
            + self.so_l2_w.len()
            + self.so_l2_b.len()
            + self.so_out_w.len()
            + self.so_out_b.len();
        let extra_stage = so_len == want_so + want_so / SO_STAGES;
        if so_len != want_so && !extra_stage {
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
        read_into(r, &mut self.ft)?;
        if staged_bias {
            read_into(r, &mut self.ft_bias)?;
        } else {
            let mut one = vec![0.0f32; H];
            read_into(r, &mut one)?;
            for st in 0..STAGE_COUNT {
                self.ft_bias[st * H..st * H + H].copy_from_slice(&one);
            }
        }
        read_into(r, &mut self.out_w)?;
        read_into(r, &mut self.out_b)?;
        self.num_w.fill(0.0);
        if has_num {
            read_into(r, &mut self.num_w)?;
        }
        self.pw.fill(0.0);
        if has_pw {
            read_into(r, &mut self.pw)?;
        }
        self.mob_w.fill(0.0);
        if has_mob {
            read_into(r, &mut self.mob_w)?;
        }
        self.mlp_out_w.fill(0.0);
        if has_mlp {
            read_into(r, &mut self.mlp_l1_w)?;
            read_into(r, &mut self.mlp_l1_b)?;
            read_into(r, &mut self.mlp_l2_w)?;
            read_into(r, &mut self.mlp_l2_b)?;
            read_into(r, &mut self.mlp_out_w)?;
        }
        self.mlp_mob_w.fill(0.0);
        if has_mlp_mob {
            read_into(r, &mut self.mlp_mob_w)?;
        }
        if self.mlp_l1_w.iter().all(|&v| v == 0.0) || self.mlp_l2_w.iter().all(|&v| v == 0.0) {
            self.init_mlp_hidden();
        }
        if so_len > 0 {
            let read_stages = |r: &mut dyn Read, dst: &mut [f32]| -> std::io::Result<()> {
                read_into(r, dst)?;
                if extra_stage {
                    let mut tail = vec![0.0f32; dst.len() * (so_len - want_so) / want_so];
                    read_into(r, &mut tail)?;
                }
                Ok(())
            };
            read_stages(&mut r, &mut self.so_l1_w)?;
            read_stages(&mut r, &mut self.so_l1_b)?;
            read_stages(&mut r, &mut self.so_l2_w)?;
            read_stages(&mut r, &mut self.so_l2_b)?;
            read_stages(&mut r, &mut self.so_out_w)?;
            read_stages(&mut r, &mut self.so_out_b)?;
        }
        if pa_len > 0 {
            read_into(r, &mut self.pa)?;
            read_into(r, &mut self.pa_bias)?;
        }
        self.mpc_alpha = if &magic == b"BBRVNN10" {
            MpcAlpha::read_from(&mut r)?
        } else {
            None
        };
        self.refresh_so_grid();
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_patterns() -> &'static [crate::pattern::Pattern] {
    static SET: std::sync::OnceLock<&'static [crate::pattern::Pattern]> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| {
        crate::pattern::from_spec(
            "Edge2X: A1 B1 C1 D1 E1 F1 G1 H1 B2 G2\nCorner3x3: A1 B1 C1 A2 B2 C2 A3 B3 C3\n",
            true,
        )
        .expect("test spec parses")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_carries_the_sigma_it_was_measured_with_or_says_it_has_none() {
        let dir = std::env::temp_dir().join(format!("kuroobi-sigma-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.bin");

        let mut nn = Nnue::new(test_patterns());
        nn.init_weights();
        assert_eq!(nn.mpc_sigma(), None, "a fresh model has no measurement");
        nn.save(&path).unwrap();
        let mut back = Nnue::new(test_patterns());
        back.load(&path).unwrap();
        assert_eq!(back.mpc_sigma(), None);

        let s = MpcSigma::from_array([-0.05, 0.3, -0.6, 0.011, 0.5, 3.25]);
        nn.set_mpc_sigma(Some(s));
        nn.save(&path).unwrap();
        let mut back = Nnue::new(test_patterns());
        back.load(&path).unwrap();
        assert_eq!(back.mpc_sigma(), Some(s));
        assert_eq!(back.mpc_sigma().unwrap().value(30, 8, 4), s.value(30, 8, 4));

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    fn alpha_for_test() -> MpcAlpha {
        let (depths, empties) = (vec![10, 18], vec![20, 40]);
        let n = depths.len() * empties.len() * MpcAlpha::BUCKETS * MpcAlpha::BUCKETS;
        MpcAlpha {
            depths,
            empties,
            cells: (0..n).map(|i| 0.5 + i as f32 / 64.0).collect(),
        }
    }

    #[test]
    fn a_file_carries_its_alpha_and_one_written_without_it_still_loads() {
        let dir = std::env::temp_dir().join(format!("kuroobi-alpha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.bin");

        let mut nn = Nnue::new(test_patterns());
        nn.init_weights();
        nn.save(&path).unwrap();
        let mut back = Nnue::new(test_patterns());
        back.load(&path).unwrap();
        assert_eq!(back.mpc_alpha(), None);

        nn.set_mpc_alpha(Some(alpha_for_test()));
        nn.save(&path).unwrap();
        let mut back = Nnue::new(test_patterns());
        back.load(&path).unwrap();
        assert_eq!(back.mpc_alpha(), Some(&alpha_for_test()));
        assert_eq!(back.ft, nn.ft, "the weights before the table are untouched");

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    #[test]
    fn alpha_is_the_cell_at_a_measured_point_and_the_mean_halfway() {
        let a = alpha_for_test();
        let (own, opp) = (10, 4); // buckets 2 and 0
        let cell = |d: usize, e: usize| a.cell(d, e, 2, 0);
        assert_eq!(a.value(10, 20, own, opp), cell(0, 0));
        assert_eq!(a.value(18, 40, own, opp), cell(1, 1));
        let mid = (cell(0, 0) + cell(0, 1) + cell(1, 0) + cell(1, 1)) / 4.0;
        assert!((a.value(14, 30, own, opp) - mid).abs() < 1e-6);
        assert_eq!(
            a.value(30, 60, own, opp),
            cell(1, 1),
            "beyond the last points it holds"
        );
    }

    #[test]
    fn adam_moments_survive_a_round_trip_and_a_mismatch_does_not() {
        let mut nn = Nnue::new(test_patterns());
        nn.init_weights();
        let mut a = AdamState::new(&nn);
        a.wd = 0.0125;
        a.t = 4321;
        a.stamp_cur = 77;
        for (i, x) in a.m_ft.iter_mut().enumerate().take(64) {
            *x = i as f32 * 0.5;
        }
        for (i, x) in a.v_out_w.iter_mut().enumerate().take(16) {
            *x = 1.0 / (i as f32 + 2.0);
        }
        a.ft_last[3] = 99;
        let mut buf: Vec<u8> = Vec::new();
        a.write_state(&mut buf).unwrap();

        let mut b = AdamState::new(&nn);
        b.read_state(&mut &buf[..]).unwrap();
        assert_eq!(b.t, 4321);
        assert_eq!(b.stamp_cur, 77);
        assert_eq!(b.wd, 0.0125);
        assert_eq!(b.ft_last[3], 99);
        assert_eq!(&b.m_ft[..64], &a.m_ft[..64]);
        assert_eq!(&b.v_out_w[..16], &a.v_out_w[..16]);

        let corner_only = crate::pattern::from_spec("Corner3x3: A1 B1 C1 A2 B2 C2 A3 B3 C3", true)
            .expect("test spec parses");
        let mut other = Nnue::new(corner_only);
        other.init_weights();
        let mut c = AdamState::new(&other);
        assert!(
            c.read_state(&mut &buf[..]).is_err(),
            "moments from another model shape must be refused"
        );
    }

    #[test]
    fn the_margin_has_a_floor() {
        let s = MpcSigma::from_array([-1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(s.value(60, 8, 4), 1.0);
    }

    #[test]
    fn product_gate_training_descends() {
        let make = || {
            let mut nn = Nnue::new(test_patterns());
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

        let mut nn = make();
        let ix = nn.indices(board.black, board.white);
        let stage = crate::linear::Linear::stage(&board);
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

    #[test]
    fn test_eval_paths_agree() {
        let mut nn = Nnue::new(test_patterns());
        nn.init_weights();
        let mut s: u64 = 0x1234_5678;
        for v in nn.ft.iter_mut() {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            *v = ((s >> 40) as i32 as f32) / 8.0e6;
        }
        for v in nn.pw.iter_mut() {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            *v = ((s >> 40) as i32 as f32) / 2.0e7;
        }
        nn.quantize();
        nn.build_incremental_table();

        const QUANT_TOL_REL: f32 = 1e-3;

        let mut board = Board::new();
        let mut acc = nn.accumulator(&board);
        let mut buckets_seen = [false; FT_BUCKETS];
        let mut plies = 0;
        for _ in 0..60 {
            let scratch = nn.eval(&board);
            let inc = nn.eval_acc(&acc, &board);
            let ix = nn.indices(board.black, board.white);
            let from_ix = nn.eval_from_indices(&ix, &board);
            buckets_seen[ft_bucket(crate::linear::Linear::stage(&board))] = true;
            plies += 1;

            let tol = 1.0 + inc.abs().max(scratch.abs()) * QUANT_TOL_REL;
            assert!(
                (inc - scratch).abs() < tol,
                "incremental {inc} vs scratch {scratch} (player {:?}, {} empty)",
                board.player(),
                board.empty_count()
            );
            let _ = from_ix;

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

    #[test]
    fn training_forward_matches_eval() {
        let mut nn = Nnue::new(test_patterns());
        nn.init_weights();
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
        let stage = crate::linear::Linear::stage(&board);
        let discs = board.player_bb().count_ones() as usize;
        let mob = 3usize;

        let evaluated = {
            let f = nn.forward(&nn.features_black(&ix, stage), mob, stage);
            {
                f
            }
        };

        let mut sink = GradSink::new(1);
        let sq0 = nn.grad_black_into(&ix, stage, discs, mob, 0.0, &mut sink);
        let sq2 = nn.grad_black_into(&ix, stage, discs, mob, 2.0, &mut sink);
        let trained = (sq0 - sq2 + 4.0) / 4.0;

        assert!(
            (trained - evaluated).abs() < 1e-3 * evaluated.abs().max(1.0),
            "training forward {trained} vs evaluated {evaluated}: the trainer \
             is descending on a different function than the search reads"
        );
    }

    #[test]
    fn stacked_grid_rounds_the_forward_pass() {
        let mut nn = Nnue::new(crate::pattern::NNUE_PATTERNS);
        nn.init_weights();
        let mut board = Board::new();
        for _ in 0..12 {
            let m = board.movable();
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            board.make_move(pos).unwrap();
        }
        let ix = nn.indices(board.black, board.white);
        let stage = crate::linear::Linear::stage(&board);
        let discs = board.black.count_ones() as usize;
        let mob = Nnue::mob_index(&board);
        let loss_of = |nn: &mut Nnue, on: bool| {
            nn.set_so_grid(on);
            let mut sink = GradSink::new(1);
            nn.grad_black_into(&ix, stage, discs, mob, 7.0, &mut sink)
        };
        let off = loss_of(&mut nn, false);
        let on = loss_of(&mut nn, true);
        assert!(
            (on - off).abs() > 1e-6,
            "grid on and off gave the same loss {on}; the forward pass is not rounding"
        );
        let on_grid = |w: f32| ((w * 64.0).round() / 64.0 - w).abs() < 1e-7;
        assert!(
            nn.so_l1_wq.iter().all(|&w| on_grid(w)),
            "L1 read off the grid"
        );
        assert!(
            nn.so_l2_wq.iter().all(|&w| on_grid(w)),
            "L2 read off the grid"
        );
        assert!(
            nn.so_l1_w.iter().any(|&w| !on_grid(w)),
            "the originals were rounded; the optimizer would have nothing to move"
        );
        nn.set_so_grid(false);
        assert!(
            nn.so_l1_wq == nn.so_l1_w,
            "grid off must read the originals"
        );
    }

    #[test]
    fn stacked_gradient_matches_finite_differences() {
        let mut nn = Nnue::new(test_patterns());
        nn.init_weights();
        let mut s: u64 = 0x0BAD_C0DE_1234_5678;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 - 8.388_608e6) / 8.388_608e6
        };
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
        nn.refresh_so_grid();

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
        let stage = crate::linear::Linear::stage(&board);
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

        let row = feats[0] as usize;
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
            let orig = unsafe { *cell(nn, sel, i) };
            unsafe { *cell(nn, sel, i) = orig + h };
            nn.refresh_so_grid();
            let up = loss(nn);
            unsafe { *cell(nn, sel, i) = orig - h };
            nn.refresh_so_grid();
            let down = loss(nn);
            unsafe { *cell(nn, sel, i) = orig };
            nn.refresh_so_grid();
            let numeric = (up - down) / (2.0 * h);
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
        for (j, g) in pa_grad {
            check(&mut nn, "pa", 8, pa_row * PA_DIMS + j, g);
        }

        assert!(
            worst < 0.02,
            "backward disagrees with finite differences by {worst:.4} relative: {worst_name}"
        );
    }
}
