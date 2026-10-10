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

pub const ACC_DIMS: usize = 2 * H;

const FT_BIAS_LEN: usize = ACC_DIMS;

const PAIR_CLAMP: f32 = 1.0;

const ACT_SCALE: f32 = 255.0 / 256.0;

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

/// True when the stages own every pattern-add bucket they touch, so training them alone moves no other stage.
pub fn owns_whole_buckets(lo: usize, hi: usize) -> bool {
    let w = SO_STAGES / PA_BUCKETS;
    lo <= hi && hi < SO_STAGES && lo.is_multiple_of(w) && (hi + 1).is_multiple_of(w)
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

#[inline]
fn mob_unit(mob: usize) -> f32 {
    (mob as f32 * (7.0 / 255.0)).min(1.0)
}

fn xorshift_unit(s: &mut u64) -> f32 {
    *s ^= *s >> 12;
    *s ^= *s << 25;
    *s ^= *s >> 27;
    ((s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as i64 as f32) / (1i64 << 23) as f32
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

pub struct AdamState {
    pub beta1: f32,
    pub beta2: f32,
    pub eps: f32,
    pub wd: f32,
    m_ft: Vec<f32>,
    v_ft: Vec<f32>,
    m_ft_bias: Vec<f32>,
    v_ft_bias: Vec<f32>,
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

        fn take_len(r: &mut dyn Read) -> std::io::Result<usize> {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            Ok(u64::from_le_bytes(b) as usize)
        }
        fn cells<T>(
            r: &mut dyn Read,
            dst: &mut [T],
            from: fn([u8; 4]) -> T,
        ) -> std::io::Result<()> {
            let mut b = [0u8; 4];
            for x in dst.iter_mut() {
                r.read_exact(&mut b)?;
                *x = from(b);
            }
            Ok(())
        }
        fn fill<T>(r: &mut dyn Read, dst: &mut [T], from: fn([u8; 4]) -> T) -> std::io::Result<()> {
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
            cells(r, dst, from)
        }

        let me = self;
        macro_rules! get {
            ($t:expr) => {
                fill(r, &mut $t, f32::from_le_bytes)?
            };
        }
        adam_flat_tables!(me, get);
        fill(r, &mut me.ft_last, u32::from_le_bytes)?;
        fill(r, &mut me.pa_last, u32::from_le_bytes)?;
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
                    cells(r, inner, f32::from_le_bytes)?;
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
            la_k: 0,
            la_alpha: 0.5,
            la_step: 0,
            la_slow: Vec::new(),
            stamp_cur: 0,
            touched: Vec::new(),
        }
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
    (a * (1.0 / PAIR_CLAMP)).clamp(0.0, 1.0)
}

#[inline]
fn so_act_grad(a: f32) -> f32 {
    let x = a * (1.0 / PAIR_CLAMP);
    if x > 0.0 && x < 1.0 {
        1.0 / PAIR_CLAMP
    } else {
        0.0
    }
}

#[inline]
fn fold_pairs_f32(raw: &[f32; ACC_DIMS]) -> [f32; H] {
    let mut out = [0.0f32; H];
    for i in 0..H {
        let a = raw[i].clamp(0.0, PAIR_CLAMP);
        let b = raw[i + H].clamp(0.0, PAIR_CLAMP);
        out[i] = a * b * (ACT_SCALE / PAIR_CLAMP);
    }
    out
}

#[inline]
fn fold_pairs_back(raw: &[f32; ACC_DIMS], dfolded: &[f32; H]) -> [f32; ACC_DIMS] {
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

/// Same order as [`Nnue::dense_lens`].
const DENSE_GRADS: [Grad; 8] = [
    |s| s.ft_bias.as_slice(),
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
    lr: f32,
    wd: f32,
    scale: f32,
}

impl Adw {
    fn grad(&self, sinks: &[GradSink], get: Grad, i: usize) -> f32 {
        sinks.iter().map(|s| get(s)[i]).sum::<f32>() * self.scale
    }

    fn step(&self, m: &mut f32, v: &mut f32, g: f32, w: &mut f32, decay: f32) {
        *m = self.b1 * *m + (1.0 - self.b1) * g;
        *v = self.b2 * *v + (1.0 - self.b2) * g * g;
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
    pub so_l1_w: Vec<f32>,
    pub so_l1_b: Vec<f32>,
    pub so_l2_w: Vec<f32>,
    pub so_l2_b: Vec<f32>,
    pub so_out_w: Vec<f32>,
    pub so_out_b: Vec<f32>,
    pub ft_bias: Vec<f32>,
    pub pa_bias: Vec<f32>,
    pub ft_rows: Vec<Vec<(u32, [f32; ACC_DIMS])>>,
    pub pa_rows: Vec<Vec<(u32, [f32; PA_DIMS])>>,
    parts: usize,
}

impl GradSink {
    pub fn new(parts: usize) -> GradSink {
        let parts = parts.max(1);
        GradSink {
            so_l1_w: vec![0.0; SO_SIZES.0],
            so_l1_b: vec![0.0; SO_SIZES.1],
            so_l2_w: vec![0.0; SO_SIZES.2],
            so_l2_b: vec![0.0; SO_SIZES.3],
            so_out_w: vec![0.0; SO_SIZES.4],
            so_out_b: vec![0.0; SO_SIZES.5],
            ft_bias: vec![0.0; FT_BIAS_LEN],
            pa_bias: vec![0.0; PA_DIMS * PA_BUCKETS],
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

    /// Leaves only the per-stage head and the stage-bucketed pattern add, so the shared layers stay put.
    pub fn keep_stage_heads_only(&mut self) {
        self.ft_bias.fill(0.0);
        for b in self.ft_rows.iter_mut() {
            b.clear();
        }
    }

    pub fn clear(&mut self) {
        self.so_l1_w.fill(0.0);
        self.so_l1_b.fill(0.0);
        self.so_l2_w.fill(0.0);
        self.so_l2_b.fill(0.0);
        self.so_out_w.fill(0.0);
        self.so_out_b.fill(0.0);
        self.ft_bias.fill(0.0);
        self.pa_bias.fill(0.0);
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
    patterns: &'static [Pattern],
    indexer: PatternIndexer,
    n_masks: usize,
    mask_off: Vec<u32>,
    n_features: usize,

    ft: Vec<f32>,
    ft_bias: Vec<f32>,
    pub pa: Vec<f32>,
    pub pa_bias: Vec<f32>,
    so_l1_w: Vec<f32>,
    so_l1_b: Vec<f32>,
    so_l2_w: Vec<f32>,
    so_l2_b: Vec<f32>,
    so_out_w: Vec<f32>,
    so_out_b: Vec<f32>,

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
            ft: vec![0.0; n_features * ACC_DIMS],
            ft_bias: vec![0.0; FT_BIAS_LEN],
            pa: vec![0.0; n_features * PA_BUCKETS * PA_DIMS],
            pa_bias: vec![0.0; PA_DIMS * PA_BUCKETS],
            so_l1_w: vec![0.0; SO_SIZES.0],
            so_l1_b: vec![0.0; SO_SIZES.1],
            so_l2_w: vec![0.0; SO_SIZES.2],
            so_l2_b: vec![0.0; SO_SIZES.3],
            so_out_w: vec![0.0; SO_SIZES.4],
            so_out_b: vec![0.0; SO_SIZES.5],
            sq: stack_q::StackQ::default(),
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

    pub fn quantize(&mut self) {
        self.sq = stack_q::StackQ::build(self);
    }

    pub fn n_features(&self) -> usize {
        self.n_features
    }

    pub fn init_weights(&mut self) {
        let mut s: u64 = 0x1234_5678_9abc_def0;
        let mut next = || xorshift_unit(&mut s);
        // the removed read-out drew first; skipping its draws keeps a fresh network bit-identical
        for _ in 0..STAGE_COUNT * H {
            next();
        }
        self.ft_bias.fill(0.0);
        let bound = (6.0 / ACC_DIMS as f32).sqrt() * 0.25;
        for w in &mut self.ft {
            *w = next() * bound;
        }
        let bound = (6.0 / (PA_DIMS * PA_BUCKETS) as f32).sqrt() * 0.25;
        let rows = self.n_features;
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
    }

    #[inline]
    fn features_black(&self, indices: &PatternIndices) -> [u32; MAX_MASKS] {
        let mut f = [0u32; MAX_MASKS];
        let raw = indices.raw();
        for m in 0..self.n_masks {
            f[m] = self.mask_off[m] + raw[m] as u32;
        }
        f
    }

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

    #[inline]
    pub fn eval_from_indices(&self, indices: &PatternIndices, board: &Board) -> f32 {
        self.net_from_indices(indices, board)
    }

    #[inline]
    pub fn net_from_indices(&self, indices: &PatternIndices, board: &Board) -> f32 {
        let stage = crate::linear::Linear::stage(board);
        self.sq
            .eval(self, indices, board.player(), stage, Self::mob_index(board))
    }

    pub fn eval(&self, board: &Board) -> f32 {
        let ix = self.indexer.init(board.black, board.white);
        self.eval_indices(board, &ix)
    }

    pub fn eval_indices(&self, board: &Board, indices: &PatternIndices) -> f32 {
        self.net_indices(board, indices)
    }

    pub fn net_indices(&self, board: &Board, indices: &PatternIndices) -> f32 {
        let stage = crate::linear::Linear::stage(board);
        let feats = self.features_player(indices, board.player());
        self.forward(&feats, Self::mob_index(board), stage)
    }

    fn pa_forward(
        &self,
        feats: &[u32; MAX_MASKS],
        stage: usize,
    ) -> ([f32; PA_DIMS], [f32; PA_DIMS]) {
        let mut z = [0.0f32; PA_DIMS];
        let bucket = pa_bucket(stage);
        z.copy_from_slice(&self.pa_bias[bucket * PA_DIMS..bucket * PA_DIMS + PA_DIMS]);
        let base_row = bucket * self.n_features;
        for &f in feats.iter().take(self.n_masks) {
            let off = (base_row + f as usize) * PA_DIMS;
            let row = &self.pa[off..off + PA_DIMS];
            for (j, zz) in z.iter_mut().enumerate() {
                *zz += row[j];
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
        board.movable_count() as usize
    }

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
        let (pa, _) = self.pa_forward(feats, stage);
        self.stacked_readout(&acc, &pa, mob, stage)
    }

    pub fn indexer(&self) -> &PatternIndexer {
        &self.indexer
    }

    pub fn indices(&self, black: u64, white: u64) -> PatternIndices {
        self.indexer.init(black, white)
    }

    pub fn grad_black_into(
        &self,
        indices: &PatternIndices,
        stage: usize,
        mob: usize,
        target: f32,
        sink: &mut GradSink,
    ) -> f32 {
        let feats = self.features_black(indices);
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
        let (pa, pa_z) = self.pa_forward(&feats, stage);
        let (sq, draw, dpa) = self.grad_stacked(&raw, &acc, &pa, &pa_z, mob, target, stage, sink);
        for &f in feats.iter().take(self.n_masks) {
            sink.push_ft(f, &draw);
        }
        let base_row = (pa_bucket(stage) * self.n_features) as u32;
        for &f in feats.iter().take(self.n_masks) {
            sink.push_pa(base_row + f, &dpa);
        }
        sq
    }

    #[allow(clippy::too_many_arguments)]
    fn grad_stacked(
        &self,
        raw: &[f32; ACC_DIMS],
        acc: &[f32; H],
        pa: &[f32; PA_DIMS],
        pa_z: &[f32; PA_DIMS],
        mob: usize,
        target: f32,
        stage: usize,
        sink: &mut GradSink,
    ) -> (f32, [f32; ACC_DIMS], [f32; PA_DIMS]) {
        let st = so_stage(stage);
        let m = mob_unit(mob);
        let xin = stack_inputs(acc, pa);
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
        let off = pa_bucket(stage) * PA_DIMS;
        for j in 0..PA_DIMS {
            sink.pa_bias[off + j] += dpa[j];
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

        adw.dense(
            &mut self.ft_bias,
            &mut adam.m_ft_bias,
            &mut adam.v_ft_bias,
            sinks,
            |s| s.ft_bias.as_slice(),
            0.0,
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

    fn dense_lens(&self) -> [usize; 8] {
        [
            self.ft_bias.len(),
            self.pa_bias.len(),
            self.so_l1_w.len(),
            self.so_l1_b.len(),
            self.so_l2_w.len(),
            self.so_l2_b.len(),
            self.so_out_w.len(),
            self.so_out_b.len(),
        ]
    }

    pub fn settle_adam(&mut self, adam: &mut AdamState, lr: f32, wd: f32) {
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
    }

    #[inline]
    pub fn ix_apply(&self, ix: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        self.indexer.apply(ix, pos, flipped, mover);
    }

    pub fn ft_len(&self) -> usize {
        self.ft.len()
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
        w.write_all(b"BBRVNN11")?;
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
        if let Some(a) = &self.mpc_alpha {
            a.write_to(w)?;
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
        if &magic != b"BBRVNN11" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad nnue magic",
            ));
        }
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
        r.read_exact(&mut u)?;
        let pa_len = u32::from_le_bytes(u) as usize;
        if pa_len != self.pa.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "phase-adaptive layer mismatch: this build and the file disagree on whether \
                 the accumulator has a phase-adaptive half",
            ));
        }
        r.read_exact(&mut u)?;
        let so_len = u32::from_le_bytes(u) as usize;
        r.read_exact(&mut u)?;
        let present = u32::from_le_bytes(u) != 0;
        let mut coef = [0.0f32; MpcSigma::LEN];
        for c in coef.iter_mut() {
            r.read_exact(&mut u)?;
            *c = f32::from_le_bytes(u);
        }
        self.mpc_sigma = present.then(|| MpcSigma::from_array(coef));
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
        read_into(r, &mut self.ft)?;
        read_into(r, &mut self.ft_bias)?;
        read_into(r, &mut self.so_l1_w)?;
        read_into(r, &mut self.so_l1_b)?;
        read_into(r, &mut self.so_l2_w)?;
        read_into(r, &mut self.so_l2_b)?;
        read_into(r, &mut self.so_out_w)?;
        read_into(r, &mut self.so_out_b)?;
        read_into(r, &mut self.pa)?;
        read_into(r, &mut self.pa_bias)?;
        self.mpc_alpha = MpcAlpha::read_from(&mut r)?;
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
    fn head_only_ranges_must_cover_whole_pattern_add_buckets() {
        assert!(owns_whole_buckets(50, 59));
        assert!(owns_whole_buckets(40, 59));
        assert!(!owns_whole_buckets(48, 59));
        assert!(!owns_whole_buckets(50, 55));
        assert!(!owns_whole_buckets(50, 60));
    }

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
    fn a_saved_network_reads_back() {
        let mut a = Nnue::new(test_patterns());
        a.init_weights();
        let mut bytes = Vec::new();
        a.write_to(&mut bytes).unwrap();
        let mut b = Nnue::new(test_patterns());
        b.read_from(&mut bytes.as_slice()).unwrap();
        assert_eq!(a.ft, b.ft);
        assert_eq!(a.pa, b.pa);
        assert_eq!(a.so_l1_w, b.so_l1_w);
        assert_eq!(a.so_out_b, b.so_out_b);
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
        for (i, x) in a.v_ft_bias.iter_mut().enumerate().take(16) {
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
        assert_eq!(&b.v_ft_bias[..16], &a.v_ft_bias[..16]);

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

        let board = Board::new();
        let ix = nn.indices(board.black, board.white);
        let stage = crate::linear::Linear::stage(&board);
        let mob = 3usize;

        let evaluated = nn.forward(&nn.features_black(&ix), mob, stage);

        let mut sink = GradSink::new(1);
        let sq0 = nn.grad_black_into(&ix, stage, mob, 0.0, &mut sink);
        let sq2 = nn.grad_black_into(&ix, stage, mob, 2.0, &mut sink);
        let trained = (sq0 - sq2 + 4.0) / 4.0;

        assert!(
            (trained - evaluated).abs() < 1e-3 * evaluated.abs().max(1.0),
            "training forward {trained} vs evaluated {evaluated}: the trainer \
             is descending on a different function than the search reads"
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
        let mob = Nnue::mob_index(&board);
        let target = 7.0f32;

        let mut sink = GradSink::new(1);
        nn.grad_black_into(&ix, stage, mob, target, &mut sink);

        let feats = nn.features_black(&ix);
        let loss = |nn: &Nnue| -> f32 {
            let out = nn.forward(&feats, mob, stage);
            let e = (out - target) / SO_SCORE;
            e * e
        };

        let row = feats[0] as usize;
        let pa_row = pa_bucket(stage) * nn.n_features + row;
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
        // One part, so every row lands in bucket zero.
        let ft_grad: Vec<(usize, f32)> = sink.ft_rows[0]
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
            let up = loss(nn);
            unsafe { *cell(nn, sel, i) = orig - h };
            let down = loss(nn);
            unsafe { *cell(nn, sel, i) = orig };
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
