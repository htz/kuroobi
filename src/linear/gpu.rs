//! GPU training for the linear pattern evaluator.
//!
//! The linear model is the NNUE's feature transformer with one output and no
//! hidden layer: a prediction is the sum of 64 pattern cells plus one
//! disc-count cell, and every cell's gradient is the same error. That makes
//! the whole model a sparse gather-sum forward and a scatter-add backward,
//! which is what a GPU is for. On one core the CPU trainer manages 32k
//! positions a second, because each position applies 512 sequential Adam
//! steps (eight symmetric forms times sixty-four cells).
//!
//! What changes against the CPU path, and why:
//!
//! - **Minibatch, not per-example.** The CPU applies one Adam step per cell
//!   touch, in order; a batch sums the touches of a cell and applies one
//!   step. This is the same change `nnue_train --minibatch` made, and it is
//!   what makes the work parallel at all.
//! - **The eight symmetric forms stay.** They are not redundant here: the
//!   mask lists carry four of each shape's eight ordered images, so the
//!   eight forms read partly different rows and predict slightly different
//!   values. Each form is its own row of the batch, exactly as the CPU
//!   trains it.
//!
//! The flat cell numbering is stage-major: stage `s`, pattern `p`, index
//! `i` is cell `s * stage_stride + pat_off[p] + i`, and the disc-count table
//! follows the patterns as `pat_off[n_patterns]`.

use std::sync::mpsc;

use wgpu::util::DeviceExt;

use crate::board::Board;
use crate::linear::{Linear, NUM_TABLE_SIZE, STAGE_COUNT};
use crate::trainer::Example;

/// Threads per workgroup, for both kernels.
const WG: u32 = 256;

/// Adam, matching `AdamOptimizer`'s defaults.
const BETA1: f32 = 0.9;
const BETA2: f32 = 0.999;
const EPSILON: f32 = 1e-8;

/// The eight symmetric forms of every position are trained, so a batch of
/// `n` examples is `8 * n` rows.
pub const FORMS: usize = 8;

/// Uniform block shared by the kernels.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    n_rows: u32,
    n_cells: u32,
    cells_per_row: u32,
    lr: f32,
}

/// `back[f][s]` is the square of the original board that symmetric form
/// `f` shows at square `s`.
///
/// Read out of `Board::symmetries` itself, one single-disc board per
/// square, rather than restated here: a second copy of the transform
/// would be a second thing to keep in step.
fn square_maps() -> [[u8; 64]; FORMS] {
    let mut back = [[0u8; 64]; FORMS];
    for s in 0..64u8 {
        let b = Board {
            black: 1u64 << s,
            white: 0,
            player: crate::color::Color::Black,
            empty_count: 63,
        };
        for (f, sym) in b.symmetries().into_iter().enumerate() {
            back[f][sym.black.trailing_zeros() as usize] = s;
        }
    }
    back
}

/// The kernels, as one shader module each.
struct Kernel {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

pub struct LinearGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    index: Kernel,
    fwd: Kernel,
    scatter: Kernel,
    bwd: Kernel,
    reduce: Kernel,
    b_w: wgpu::Buffer,
    b_m: wgpu::Buffer,
    b_v: wgpu::Buffer,
    b_t: wgpu::Buffer,
    /// Two of each per-batch buffer: the device is still reading one while
    /// the host builds the next, which is what keeps both busy.
    b_ex: [wgpu::Buffer; 2],
    b_msq: wgpu::Buffer,
    b_mptr: wgpu::Buffer,
    b_mbase: wgpu::Buffer,
    b_cells: [wgpu::Buffer; 2],
    b_tgt: [wgpu::Buffer; 2],
    b_err: [wgpu::Buffer; 2],
    b_grad: wgpu::Buffer,
    pending: std::collections::VecDeque<wgpu::SubmissionIndex>,
    b_stats: wgpu::Buffer,
    b_stats_read: wgpu::Buffer,
    b_par: wgpu::Buffer,
    n_cells: usize,
    stages: (usize, usize),
    cells_per_row: usize,
    max_rows: usize,
    /// Scratch reused between batches so a multi-gigabyte alloc/free pair
    /// does not happen 20,000 times an epoch.
    /// Five words a position: the two bitboards and the label.
    ex_words: Vec<u32>,
}

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    pollster_lite(fut)
}

/// Minimal executor: the GPU futures here complete on poll.
fn pollster_lite<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// Fixed-point scale for the accumulated gradient. The worst case is a
/// whole batch's rows on one cell at the largest disc difference -- the
/// disc-count cell of a stage is touched once by every row -- so
/// 524,288 x 64 x 32 is 1.07e9, inside i32's 2.1e9.
const SCALE: f32 = 32.0;

fn shader_prelude(
    cells_per_row: usize,
    stride: usize,
    numoff: u32,
    stlo: usize,
    nst: usize,
) -> String {
    let scale_f = SCALE;
    let forms = FORMS;
    format!(
        "const WG: u32 = {WG}u;
const CELLS: u32 = {cells_per_row}u;
const B1: f32 = {BETA1};
const B2: f32 = {BETA2};
const EPS: f32 = {EPSILON};
const SCALE: f32 = {scale_f};
const FORMS: u32 = {forms}u;
const NST: u32 = {nst}u;
const STLO: u32 = {stlo}u;
const STRIDE: u32 = {stride}u;
const NUMOFF: u32 = {numoff}u;

struct Params {{
  n_rows: u32,
  n_cells: u32,
  cells_per_row: u32,
  lr: f32,
}};
"
    )
}

const INDEX_SRC: &str = r#"
@group(0) @binding(0) var<storage, read> ex: array<u32>;      // 5 words each
@group(0) @binding(1) var<storage, read> msq: array<u32>;     // mask squares
@group(0) @binding(2) var<storage, read> mptr: array<u32>;    // offset<<8 | len
@group(0) @binding(3) var<storage, read> mbase: array<u32>;   // pattern offset
@group(0) @binding(4) var<storage, read_write> cells: array<u32>;
@group(0) @binding(5) var<storage, read_write> tgt: array<f32>;
@group(0) @binding(6) var<uniform> par: Params;

// One thread per row, building that row's cell numbers from the board.
//
// The symmetry is folded into the mask tables instead of the board: form
// `f` reading mask `m` is the same as the original board reading `m`
// mapped back through `f`, so the tables carry the eight mapped copies and
// the kernel never rotates anything. That keeps the host's upload to the
// twenty bytes of a position, against the 136 MB of pre-built indices a
// batch used to carry.
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  let r = gid.x;
  if (r >= par.n_rows) { return; }
  let e = r / FORMS;
  let f = r % FORMS;
  let blo = ex[e * 5u];
  let bhi = ex[e * 5u + 1u];
  let wlo = ex[e * 5u + 2u];
  let whi = ex[e * 5u + 3u];
  tgt[r] = bitcast<f32>(ex[e * 5u + 4u]);

  let discs = countOneBits(blo) + countOneBits(bhi) + countOneBits(wlo) + countOneBits(whi);
  let empties = 64u - discs;
  // Linear::stage: 60 - empties, floored at zero, capped at the last.
  var stage = 0u;
  if (empties < 60u) { stage = 60u - empties; }
  stage = min(stage, STLO + NST - 1u);
  // A run aimed at part of the board carries only those stages, so the
  // cell space starts at its first one. The host drops anything outside
  // the window before the batch, so the subtraction cannot wrap.
  let base = (stage - STLO) * STRIDE;

  let out = r * CELLS;
  for (var k = 0u; k < CELLS - 1u; k = k + 1u) {
    let ptr = mptr[f * (CELLS - 1u) + k];
    let off = ptr >> 8u;
    let len = ptr & 255u;
    var idx = 0u;
    for (var j = 0u; j < len; j = j + 1u) {
      let sq = msq[off + j];
      var digit = 2u;
      if (sq < 32u) {
        let bit = 1u << sq;
        if ((blo & bit) != 0u) { digit = 0u; } else if ((wlo & bit) != 0u) { digit = 1u; }
      } else {
        let bit = 1u << (sq - 32u);
        if ((bhi & bit) != 0u) { digit = 0u; } else if ((whi & bit) != 0u) { digit = 1u; }
      }
      idx = idx * 3u + digit;
    }
    cells[out + k] = base + mbase[k] + idx;
  }
  // The disc-count cell: the side to move is Black in every example.
  cells[out + CELLS - 1u] = base + NUMOFF + countOneBits(blo) + countOneBits(bhi);
}
"#;

const FWD_SRC: &str = r#"
@group(0) @binding(0) var<storage, read> cells: array<u32>;
@group(0) @binding(1) var<storage, read> tgt: array<f32>;
@group(0) @binding(2) var<storage, read> w: array<f32>;
@group(0) @binding(3) var<storage, read_write> err: array<f32>;
@group(0) @binding(4) var<uniform> par: Params;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  let r = gid.x;
  if (r >= par.n_rows) { return; }
  let base = r * CELLS;
  var s = 0.0;
  for (var i = 0u; i < CELLS; i = i + 1u) {
    s = s + w[cells[base + i]];
  }
  // The trainer's error is target - prediction, and every cell's gradient
  // is that error unchanged: the model is a plain sum of its cells.
  err[r] = tgt[r] - s;
}
"#;

const SCATTER_SRC: &str = r#"
@group(0) @binding(0) var<storage, read> cells: array<u32>;
@group(0) @binding(1) var<storage, read> err: array<f32>;
@group(0) @binding(2) var<storage, read_write> grad: array<atomic<i32>>;
@group(0) @binding(3) var<uniform> par: Params;

// One thread per row, adding its error into each cell it touched.
//
// Fixed point, because WGSL has no float atomic and a compare-exchange
// loop would serialise the hot cells (the disc-count cell of a stage is
// touched by every row of that stage). SCALE is chosen so the worst case
// -- every row of a batch on one cell, at the largest error a disc
// difference can hold -- stays inside i32.
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  let r = gid.x;
  if (r >= par.n_rows) { return; }
  // Rounded, not truncated: WGSL's f32-to-i32 conversion goes toward
  // zero, which shrinks every term of the sum rather than scattering it,
  // and near convergence the true sum is small enough for that to matter.
  let q = i32(round(err[r] * SCALE));
  let base = r * CELLS;
  for (var i = 0u; i < CELLS; i = i + 1u) {
    atomicAdd(&grad[cells[base + i]], q);
  }
}
"#;

const BWD_SRC: &str = r#"
@group(0) @binding(0) var<storage, read_write> grad: array<atomic<i32>>;
@group(0) @binding(1) var<storage, read_write> w: array<f32>;
@group(0) @binding(2) var<storage, read_write> m: array<f32>;
@group(0) @binding(3) var<storage, read_write> v: array<f32>;
@group(0) @binding(4) var<storage, read_write> tc: array<u32>;
@group(0) @binding(5) var<uniform> par: Params;

// Sweeps every cell and steps the ones the batch touched, then clears the
// accumulator for the next batch. Reading 37 million counters is 150 MB of
// device bandwidth, which at 400 GB/s costs less than building a sparse
// map on the host and shipping it across.
//
// A cell whose errors cancel to exactly zero is skipped rather than
// stepped with a zero gradient. That leaves its moments and step count
// alone, which is the same thing an untouched cell gets.
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>,
        @builtin(num_workgroups) nwg: vec3<u32>) {
  let stride = nwg.x * 256u;
  var c = gid.x;
  loop {
    if (c >= par.n_cells) { break; }
    let qi = atomicLoad(&grad[c]);
    if (qi != 0) {
      atomicStore(&grad[c], 0);
      let g = f32(qi) / SCALE;
      let step = tc[c] + 1u;
      tc[c] = step;
      let mm = B1 * m[c] + (1.0 - B1) * g;
      let vv = B2 * v[c] + (1.0 - B2) * g * g;
      m[c] = mm;
      v[c] = vv;
      let mh = mm / (1.0 - pow(B1, f32(step)));
      let vh = vv / (1.0 - pow(B2, f32(step)));
      // Plus, not minus: g is the positive-direction error.
      w[c] = w[c] + par.lr * mh / (sqrt(vh) + EPS);
    }
    c = c + stride;
  }
}
"#;

const REDUCE_SRC: &str = r#"
@group(0) @binding(0) var<storage, read> err: array<f32>;
@group(0) @binding(1) var<storage, read_write> stats: array<f32>;
@group(0) @binding(2) var<uniform> par: Params;

var<workgroup> red: array<f32, 256>;

// One workgroup, grid-striding the batch: the sum of squares is a scalar
// the host reads once a shard, not a hot path.
@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>) {
  var s = 0.0;
  for (var i = lid.x; i < par.n_rows; i = i + WG) {
    s = s + err[i] * err[i];
  }
  red[lid.x] = s;
  workgroupBarrier();
  var w = WG / 2u;
  loop {
    if (w == 0u) { break; }
    if (lid.x < w) { red[lid.x] = red[lid.x] + red[lid.x + w]; }
    workgroupBarrier();
    w = w / 2u;
  }
  if (lid.x == 0u) { stats[0] = stats[0] + red[0]; }
}
"#;

impl LinearGpu {
    /// `batch` is examples per step; a step trains `FORMS * batch` rows.
    pub fn new(ev: &Linear, batch: usize, stages: (usize, usize)) -> LinearGpu {
        let (st_lo, st_hi) = stages;
        assert!(st_lo <= st_hi && st_hi < STAGE_COUNT, "stage window");
        let n_stages = st_hi + 1 - st_lo;
        let patterns = ev.patterns();
        let mut pat_off = Vec::with_capacity(patterns.len() + 2);
        let mut off = 0u32;
        for p in patterns {
            pat_off.push(off);
            off += p.table_size() as u32;
        }
        pat_off.push(off); // the disc-count table
        off += NUM_TABLE_SIZE as u32;
        let stage_stride = off as usize;
        let n_cells = stage_stride * n_stages;
        let max_rows = batch * FORMS;
        let cells_per_row = patterns.iter().map(|p| p.masks.len()).sum::<usize>() + 1;
        // Mask tables, one set per symmetric form, with the transform
        // folded in. `mbase` carries each slot's pattern offset so the
        // kernel needs no pattern structure of its own.
        let back = square_maps();
        let n_slots = cells_per_row - 1;
        let mut msq: Vec<u32> = Vec::new();
        let mut mptr: Vec<u32> = vec![0; FORMS * n_slots];
        let mut mbase: Vec<u32> = vec![0; n_slots];
        for f in 0..FORMS {
            let mut k = 0usize;
            for (pi, p) in patterns.iter().enumerate() {
                for m in p.masks {
                    let off = msq.len() as u32;
                    assert!(m.len() <= 255, "a mask longer than the length field");
                    for &sq in *m {
                        msq.push(back[f][sq as usize] as u32);
                    }
                    mptr[f * n_slots + k] = (off << 8) | m.len() as u32;
                    mbase[k] = pat_off[pi];
                    k += 1;
                }
            }
            assert_eq!(k, n_slots);
        }
        /* 384 of the 512 (form, mask) pairs repeat an earlier one -- a
        pattern's mask list carries four of its shape's eight ordered
        images -- so only 128 of them need the base-3 walk. Computing the
        128 and copying the rest was tried and made no difference (2.15M
        against 2.13M positions a second): the index walk is not what this
        kernel waits on. What it waits on is the random access into the
        149 MB weight table, 520 reads and 520 atomic adds a position,
        which only training fewer forms would cut. */
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .expect("no GPU adapter");
        let info = adapter.get_info();
        let limits = adapter.limits();
        eprintln!(
            "gpu: {} ({:?}), {} cells x 20 B = {} MB of weights, moments and gradient",
            info.name,
            info.backend,
            n_cells,
            (n_cells * 20) >> 20
        );
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("linear_train"),
            required_limits: limits,
            ..Default::default()
        }))
        .expect("GPU device");

        let pre = shader_prelude(
            cells_per_row,
            stage_stride,
            pat_off[patterns.len()],
            st_lo,
            n_stages,
        );
        let make = |src: &str, label: &str| -> Kernel {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(format!("{pre}{src}").into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let layout = pipeline.get_bind_group_layout(0);
            Kernel { pipeline, layout }
        };
        let index = make(INDEX_SRC, "index");
        let fwd = make(FWD_SRC, "forward");
        let scatter = make(SCATTER_SRC, "scatter");
        let bwd = make(BWD_SRC, "adam");
        let reduce = make(REDUCE_SRC, "reduce");

        let st = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let zeros = |n: usize, label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (n * 4) as u64,
                usage: st | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let b_w = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("weights"),
            contents: bytemuck::cast_slice(&ev.flat_stages(st_lo, st_hi)),
            usage: st | wgpu::BufferUsages::COPY_SRC,
        });
        let b_m = zeros(n_cells, "adam m");
        let b_v = zeros(n_cells, "adam v");
        let b_t = zeros(n_cells, "adam t");
        let init = |v: &[u32], label: &str| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(v),
                usage: st,
            })
        };
        let b_msq = init(&msq, "mask squares");
        let b_mptr = init(&mptr, "mask pointers");
        let b_mbase = init(&mbase, "mask bases");
        let b_ex = [
            zeros(max_rows / FORMS * 5, "ex 0"),
            zeros(max_rows / FORMS * 5, "ex 1"),
        ];
        let b_cells = [
            zeros(max_rows * cells_per_row, "cells 0"),
            zeros(max_rows * cells_per_row, "cells 1"),
        ];
        let b_tgt = [zeros(max_rows, "targets 0"), zeros(max_rows, "targets 1")];
        let b_err = [zeros(max_rows, "errors 0"), zeros(max_rows, "errors 1")];
        let b_grad = zeros(n_cells, "grad");
        let b_stats = zeros(4, "stats");
        let b_stats_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stats readback"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let b_par = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        LinearGpu {
            stages,
            device,
            queue,
            index,
            fwd,
            scatter,
            bwd,
            reduce,
            b_w,
            b_m,
            b_v,
            b_t,
            b_ex,
            b_msq,
            b_mptr,
            b_mbase,
            b_cells,
            b_tgt,
            b_err,
            b_grad,
            pending: std::collections::VecDeque::new(),
            b_stats,
            b_stats_read,
            b_par,
            n_cells,
            cells_per_row,
            max_rows,
            ex_words: Vec::new(),
        }
    }

    /// Pack the batch as five words a position. The kernel does the rest.
    fn prepare(&mut self, batch: &[Example]) -> usize {
        self.ex_words.clear();
        self.ex_words.reserve(batch.len() * 5);
        let (lo, hi) = self.stages;
        let mut n = 0usize;
        for ex in batch {
            // The kernel indexes into this run's stages only; a position
            // from another stage would land on the wrong table.
            let st = Linear::stage(&ex.board());
            if st < lo || st > hi {
                continue;
            }
            n += 1;
            self.ex_words.push(ex.black as u32);
            self.ex_words.push((ex.black >> 32) as u32);
            self.ex_words.push(ex.white as u32);
            self.ex_words.push((ex.white >> 32) as u32);
            self.ex_words.push(ex.score.to_bits());
        }
        n
    }

    fn bind(&self, k: &Kernel, bufs: &[&wgpu::Buffer]) -> wgpu::BindGroup {
        let entries: Vec<wgpu::BindGroupEntry> = bufs
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect();
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &k.layout,
            entries: &entries,
        })
    }

    /// One pass over `examples`, one optimizer step per batch. Returns the
    /// sum of squared errors over all rows (examples times forms).
    pub fn train_shard(
        &mut self,
        examples: &[Example],
        lr_for_step: &mut impl FnMut() -> f32,
    ) -> f64 {
        self.queue.write_buffer(&self.b_stats, 0, &[0u8; 16]);
        // `KUROOBI_GPU_PROF=1` splits the shard's time two ways: building
        // the rows on the host, and uploading plus running them. Which one
        // leads decides what is worth optimising next.
        let prof = std::env::var_os("KUROOBI_GPU_PROF").is_some();
        let (mut t_prep, mut t_dev) = (0.0f64, 0.0f64);
        let per_batch = self.max_rows / FORMS;
        let mut slot = 0usize;
        for batch in examples.chunks(per_batch) {
            // Two batches may be in flight; a third would overwrite a slot
            // the device is still reading.
            self.drain(1);
            let t0 = std::time::Instant::now();
            let kept = self.prepare(batch);
            let t1 = std::time::Instant::now();
            if kept == 0 {
                continue;
            }
            let n_rows = kept * FORMS;
            let t2 = t1;
            t_prep += (t1 - t0).as_secs_f64();
            let par = Params {
                n_rows: n_rows as u32,
                n_cells: self.n_cells as u32,
                cells_per_row: self.cells_per_row as u32,
                lr: lr_for_step(),
            };
            let q = &self.queue;
            q.write_buffer(&self.b_par, 0, bytemuck::bytes_of(&par));
            q.write_buffer(&self.b_ex[slot], 0, bytemuck::cast_slice(&self.ex_words));

            let g_idx = self.bind(
                &self.index,
                &[
                    &self.b_ex[slot],
                    &self.b_msq,
                    &self.b_mptr,
                    &self.b_mbase,
                    &self.b_cells[slot],
                    &self.b_tgt[slot],
                    &self.b_par,
                ],
            );
            let g_fwd = self.bind(
                &self.fwd,
                &[
                    &self.b_cells[slot],
                    &self.b_tgt[slot],
                    &self.b_w,
                    &self.b_err[slot],
                    &self.b_par,
                ],
            );
            let g_sca = self.bind(
                &self.scatter,
                &[
                    &self.b_cells[slot],
                    &self.b_err[slot],
                    &self.b_grad,
                    &self.b_par,
                ],
            );
            let g_bwd = self.bind(
                &self.bwd,
                &[
                    &self.b_grad,
                    &self.b_w,
                    &self.b_m,
                    &self.b_v,
                    &self.b_t,
                    &self.b_par,
                ],
            );
            let g_red = self.bind(
                &self.reduce,
                &[&self.b_err[slot], &self.b_stats, &self.b_par],
            );

            let mut enc = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut p = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: None,
                    timestamp_writes: None,
                });
                p.set_pipeline(&self.index.pipeline);
                p.set_bind_group(0, &g_idx, &[]);
                p.dispatch_workgroups((n_rows as u32).div_ceil(WG), 1, 1);
                p.set_pipeline(&self.fwd.pipeline);
                p.set_bind_group(0, &g_fwd, &[]);
                p.dispatch_workgroups((n_rows as u32).div_ceil(WG), 1, 1);
                p.set_pipeline(&self.reduce.pipeline);
                p.set_bind_group(0, &g_red, &[]);
                p.dispatch_workgroups(1, 1, 1);
                p.set_pipeline(&self.scatter.pipeline);
                p.set_bind_group(0, &g_sca, &[]);
                p.dispatch_workgroups((n_rows as u32).div_ceil(WG), 1, 1);
                p.set_pipeline(&self.bwd.pipeline);
                p.set_bind_group(0, &g_bwd, &[]);
                // A grid-stride sweep: 37 million cells is past the
                // per-dimension workgroup limit, and a fixed grid keeps
                // the dispatch one-dimensional.
                p.dispatch_workgroups(2048, 1, 1);
            }
            self.pending.push_back(self.queue.submit([enc.finish()]));
            t_dev += t2.elapsed().as_secs_f64();
            slot ^= 1;
        }
        let t = std::time::Instant::now();
        self.drain(0);
        t_dev += t.elapsed().as_secs_f64();
        if prof {
            eprintln!("gpu prof: rows {t_prep:.2}s  device {t_dev:.2}s");
        }
        let mut out = [0f32; 4];
        self.read_back(&mut out);
        out[0] as f64
    }

    /// Block until at most `keep` submissions are still in flight.
    fn drain(&mut self, keep: usize) {
        while self.pending.len() > keep {
            let idx = self.pending.pop_front().unwrap();
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(idx),
                    timeout: None,
                })
                .expect("GPU wait");
        }
    }

    fn read_back(&self, out: &mut [f32; 4]) {
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_buffer_to_buffer(&self.b_stats, 0, &self.b_stats_read, 0, 16);
        self.queue.submit([enc.finish()]);
        let (tx, rx) = mpsc::channel();
        self.b_stats_read
            .slice(..16)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().unwrap().unwrap();
        out.copy_from_slice(bytemuck::cast_slice(
            &self.b_stats_read.slice(..16).get_mapped_range(),
        ));
        self.b_stats_read.unmap();
    }

    /// Copy the trained weights back into `ev`.
    pub fn download(&self, ev: &mut Linear) {
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("weight readback"),
            size: (self.n_cells * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_buffer_to_buffer(&self.b_w, 0, &staging, 0, (self.n_cells * 4) as u64);
        self.queue.submit([enc.finish()]);
        let (tx, rx) = mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().unwrap().unwrap();
        let view = staging.slice(..).get_mapped_range();
        ev.set_flat_stages(self.stages.0, self.stages.1, bytemuck::cast_slice(&view));
        drop(view);
        staging.unmap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flat numbering has to round-trip, or the GPU trains cells the
    /// evaluator reads somewhere else entirely.
    #[test]
    fn flat_layout_round_trips() {
        use crate::pattern::LINEAR_PATTERNS;
        let mut ev = Linear::new(LINEAR_PATTERNS);
        let mut flat = ev.flat_all();
        for (i, w) in flat.iter_mut().enumerate() {
            *w = (i % 997) as f32;
        }
        ev.set_flat_all(&flat);
        assert_eq!(ev.flat_all(), flat);
    }

    /// A board's cells under the flat numbering must sum to what `eval`
    /// returns; the GPU forward is exactly that sum.
    #[test]
    fn flat_cells_sum_to_the_evaluation() {
        use crate::pattern::LINEAR_PATTERNS;
        let mut ev = Linear::new(LINEAR_PATTERNS);
        let mut flat = ev.flat_all();
        let mut k = 0u32;
        for w in flat.iter_mut() {
            k = k.wrapping_mul(1664525).wrapping_add(1013904223);
            *w = (k >> 20) as f32 / 4096.0 - 0.5;
        }
        ev.set_flat_all(&flat);

        let mut b = Board::new();
        for mv in [19u32, 18, 17, 26] {
            if let Some(p) = crate::position::Position::from_index(mv) {
                b.make_move_bits(p);
            }
        }
        let patterns = ev.patterns();
        let mut off = 0u32;
        let mut pat_off = Vec::new();
        for p in patterns {
            pat_off.push(off);
            off += p.table_size() as u32;
        }
        pat_off.push(off);
        let stage_stride = off + NUM_TABLE_SIZE as u32;
        let base = Linear::stage(&b) as u32 * stage_stride;
        let mut sum = 0.0f32;
        for (pi, p) in patterns.iter().enumerate() {
            for idx in p.indices(b.black, b.white, b.player()) {
                sum += flat[(base + pat_off[pi] + idx as u32) as usize];
            }
        }
        sum += flat[(base + pat_off[patterns.len()] + b.player_bb().count_ones()) as usize];
        assert!(
            (sum - ev.eval(&b)).abs() < 1e-3,
            "flat sum {sum} against eval {}",
            ev.eval(&b)
        );
    }
}
