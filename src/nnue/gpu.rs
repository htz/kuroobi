//! The stacked-read-out model's training step on the GPU.
//!
//! Same model, same optimizer, same numbers as `apply_adamw_batch` up to
//! float summation order -- with one deliberate difference: the sparse
//! tables take a dense AdamW step every batch, which is what AdamW means
//! and what `catch_up` on the CPU only approximates. A batch
//! is one command buffer of eight kernels:
//!
//! 1. `fwd_bwd`: one workgroup per example runs the forward pass and the
//!    backward pass down to the two sparse layers, leaving a record per
//!    example (input-lane gradients, activations, layer deltas, error).
//! 2. `row_grad` (twice, transformer and phase-adaptive rows): one
//!    workgroup per row the batch touched sums that row's gradient over the
//!    examples that used it, from a CSR the CPU builds while the previous
//!    batch runs.
//! 3. `bias_part` then `dense_grad`: the read-out stacks and the two bias
//!    vectors, one thread per parameter, each summing over the examples of
//!    its stage (examples arrive sorted by stage).
//! 4. `finalize`: the global gradient norm, hence the clip factor.
//! 5. `step_rows` (twice) and `step_dense`: AdamW over every cell; every
//!    k-th step also folds the result into the Lookahead slow copy.
//!
//! The weights live on the GPU for the whole epoch and come back for the
//! held-out pass and the save. wgpu, so the same binary runs on Metal and
//! Vulkan; there is no CUDA-only path to maintain beside it.
//!
//! Checked against the CPU path with `settle_adam` forced after every step
//! (which makes it the same dense optimizer): 16 steps of 512 examples with
//! Lookahead, 305M parameters, one cell differs by more than 1e-5 (1.2e-5)
//! and the training loss agrees to four decimals. Against the CPU path as
//! it stands the two drift apart from the third step, because the lazy
//! update lets a forward pass read rows still owing momentum from a batch
//! that did not touch them; that is the CPU's approximation, not this one's.

use std::sync::mpsc;

use wgpu::util::DeviceExt;

use super::*;
use crate::trainer::{Example, SymPlan};

/// Per-example record the forward/backward kernel writes, in f32 slots.
const R_DRAW: usize = 0;
const R_DPA: usize = R_DRAW + ACC_DIMS;
/// `xin` followed by the mobility input, so L1's 257 inputs are contiguous.
const R_XIN: usize = R_DPA + PA_DIMS;
const R_A1: usize = R_XIN + SO_L1_IN;
const R_V2: usize = R_A1 + SO_L1 * 2;
const R_DL1: usize = R_V2 + SO_L2;
const R_DL2: usize = R_DL1 + SO_L1;
const R_ERR: usize = R_DL2 + SO_L2;
const R_SQ: usize = R_ERR + 1;
const REC: usize = (R_SQ + 1).div_ceil(16) * 16;

/// The dense parameter vector: the six read-out tables in
/// `apply_adamw_batch`'s order, then the transformer bias, then the
/// phase-adaptive biases.
const D_L1W: usize = 0;
const D_L1B: usize = D_L1W + SO_SIZES.0;
const D_L2W: usize = D_L1B + SO_SIZES.1;
const D_L2B: usize = D_L2W + SO_SIZES.2;
const D_OW: usize = D_L2B + SO_SIZES.3;
const D_OB: usize = D_OW + SO_SIZES.4;
const D_FTB: usize = D_OB + SO_SIZES.5;
const D_PAB: usize = D_FTB + ACC_DIMS;
const N_DENSE: usize = D_PAB + PA_BUCKETS * PA_DIMS;

/// Examples per workgroup in the bias-partial kernel, and the width of one
/// chunk's partial sums (transformer lanes, then the phase-adaptive lanes
/// per bucket).
const CHUNK: usize = 256;
const PART: usize = ACC_DIMS + PA_BUCKETS * PA_DIMS;

const WG: usize = 256;
/// Examples one `K_ROW_GRAD` workgroup sums; longer rows are split so no
/// single workgroup's serial loop bounds the kernel. 128 and 512 measured
/// within the run-to-run noise of 256 (row kernels sit 0.4 ms over their
/// bandwidth floor) while moving val in its fifth digit, so 256 stays.
const SEG: u32 = 256;
const NO_PART: u32 = u32::MAX;
/// Fields of the uniform block every kernel reads; see `Params` in the
/// shader prelude.
const N_PARAMS: usize = 24;

/// Workgroups per dispatch dimension a conforming device must accept.
const MAX_WG: u32 = 65535;

fn prelude() -> String {
    format!(
        r#"
const ACC: u32 = {acc}u;
// Per-subgroup partials for a cross-workgroup sum; sized for subgroups of
// 8 lanes or more.
const NSG_MAX: u32 = 32u;
const HH: u32 = {h}u;
const PAD: u32 = {pad}u;
const PAB: u32 = {pab}u;
const SKIP: u32 = {skip}u;
const L1: u32 = {l1}u;
const L1IN: u32 = {l1in}u;
const L2: u32 = {l2}u;
const OUTIN: u32 = {outin}u;
const NST: u32 = {nst}u;
const REC: u32 = {rec}u;
const R_DRAW: u32 = {r_draw}u;
const R_DPA: u32 = {r_dpa}u;
const R_XIN: u32 = {r_xin}u;
const R_A1: u32 = {r_a1}u;
const R_V2: u32 = {r_v2}u;
const R_DL1: u32 = {r_dl1}u;
const R_DL2: u32 = {r_dl2}u;
const R_ERR: u32 = {r_err}u;
const R_SQ: u32 = {r_sq}u;
const D_L1W: u32 = {d_l1w}u;
const D_L1B: u32 = {d_l1b}u;
const D_L2W: u32 = {d_l2w}u;
const D_L2B: u32 = {d_l2b}u;
const D_OW: u32 = {d_ow}u;
const D_OB: u32 = {d_ob}u;
const D_FTB: u32 = {d_ftb}u;
const D_PAB: u32 = {d_pab}u;
const N_DENSE: u32 = {n_dense}u;
const CHUNK: u32 = {chunk}u;
const NO_PART: u32 = 0xffffffffu;
const PART: u32 = {part}u;
const ACT_SCALE: f32 = {act_scale};
const SO_SCORE: f32 = {so_score};
const SO_MAX_W: f32 = {so_max_w};
const CLIP_NORM: f32 = {clip_norm};

struct Params {{
    n_ex: u32,
    n_masks: u32,
    nfb: u32,
    n_items: u32,
    width: u32,
    rec_off: u32,
    n_chunks: u32,
    n_a: u32,
    n_b: u32,
    n_c: u32,
    flags: u32,
    pad0: u32,
    scale: f32,
    lr: f32,
    wd: f32,
    bc1: f32,
    bc2s: f32,
    alpha: f32,
    b1: f32,
    b2: f32,
    eps: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
}}

fn linear_wg(wg: vec3<u32>, nwg: vec3<u32>) -> u32 {{
    return wg.x + wg.y * nwg.x;
}}
"#,
        acc = ACC_DIMS,
        h = H,
        pad = PA_DIMS,
        pab = PA_BUCKETS,
        skip = SO_SKIP,
        l1 = SO_L1,
        l1in = SO_L1_IN,
        l2 = SO_L2,
        outin = SO_OUT_IN,
        nst = SO_STAGES,
        rec = REC,
        r_draw = R_DRAW,
        r_dpa = R_DPA,
        r_xin = R_XIN,
        r_a1 = R_A1,
        r_v2 = R_V2,
        r_dl1 = R_DL1,
        r_dl2 = R_DL2,
        r_err = R_ERR,
        r_sq = R_SQ,
        d_l1w = D_L1W,
        d_l1b = D_L1B,
        d_l2w = D_L2W,
        d_l2b = D_L2B,
        d_ow = D_OW,
        d_ob = D_OB,
        d_ftb = D_FTB,
        d_pab = D_PAB,
        n_dense = N_DENSE,
        chunk = CHUNK,
        part = PART,
        act_scale = ACT_SCALE,
        so_score = SO_SCORE,
        so_max_w = 127.0 / 64.0,
        clip_norm = GRAD_CLIP_NORM,
    )
}

/// Forward and backward for one example per workgroup. Thread `h` owns
/// accumulator lane `h`; the read-out's first layer and the output sum are
/// products on every lane added up across subgroups, the small middle
/// layer uses the first 64 threads.
const K_FWD_BWD: &str = r#"
@group(0) @binding(0) var<storage, read> inp: array<u32>;
@group(0) @binding(1) var<storage, read> ft: array<f32>;
@group(0) @binding(2) var<storage, read> pa: array<f32>;
@group(0) @binding(3) var<storage, read> dense: array<f32>;
@group(0) @binding(4) var<storage, read_write> rec: array<f32>;
@group(0) @binding(5) var<uniform> P: Params;

var<workgroup> raw: array<f32, ACC>;
var<workgroup> xin: array<f32, SKIP>;
var<workgroup> paz: array<f32, PAD>;
var<workgroup> l1: array<f32, L1>;
var<workgroup> a1: array<f32, 32>;
var<workgroup> l2: array<f32, L2>;
var<workgroup> v2: array<f32, L2>;
var<workgroup> dl2: array<f32, L2>;
var<workgroup> da1: array<f32, 32>;
var<workgroup> dl1: array<f32, L1>;
var<workgroup> part: array<f32, L1 * NSG_MAX>;
var<workgroup> sh_err: f32;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(workgroup_id) wg: vec3<u32>,
        @builtin(num_workgroups) nwg: vec3<u32>,
        @builtin(subgroup_id) sg_id: u32,
        @builtin(subgroup_invocation_id) sg_inv: u32,
        @builtin(num_subgroups) n_sg: u32) {
    let ex = linear_wg(wg, nwg);
    if (ex >= P.n_ex) {
        return;
    }
    let h = lid.x;
    let ib = ex * (P.n_masks + 3u);
    let stage = inp[ib + P.n_masks];
    let mob = bitcast<f32>(inp[ib + P.n_masks + 1u]);
    let tgt = bitcast<f32>(inp[ib + P.n_masks + 2u]);
    let st = min(stage, NST - 1u);
    let bucket = min(stage / (NST / PAB), PAB - 1u);
    let rb = ex * REC;

    // Sparse layers: every thread one lane.
    var s = dense[D_FTB + h];
    for (var m = 0u; m < P.n_masks; m++) {
        s += ft[inp[ib + m] * ACC + h];
    }
    raw[h] = s;
    if (h < PAD) {
        var z = dense[D_PAB + bucket * PAD + h];
        let base = bucket * P.nfb;
        for (var m = 0u; m < P.n_masks; m++) {
            z += pa[(base + inp[ib + m]) * PAD + h];
        }
        paz[h] = z;
    }
    workgroupBarrier();
    if (h < HH) {
        let a = clamp(raw[h], 0.0, 1.0);
        let b = clamp(raw[h + HH], 0.0, 1.0);
        xin[h] = clamp(a * b * ACT_SCALE, 0.0, 1.0);
    }
    if (h < PAD) {
        let zc = clamp(paz[h], 0.0, 1.0);
        xin[HH + h] = zc * zc * ACT_SCALE;
    }
    workgroupBarrier();
    // L1: lane `h` multiplies its input by each output's weight (adjacent
    // threads read adjacent weights), subgroups add up, the first 16
    // threads finish.
    var xh = 0.0;
    if (h < SKIP) {
        xh = xin[h];
    }
    for (var i = 0u; i < L1; i++) {
        let p = subgroupAdd(dense[D_L1W + (st * L1 + i) * L1IN + h] * xh);
        if (sg_inv == 0u) {
            part[i * NSG_MAX + sg_id] = p;
        }
    }
    workgroupBarrier();
    if (h < L1) {
        var x = dense[D_L1B + st * L1 + h];
        for (var g = 0u; g < n_sg; g++) {
            x += part[h * NSG_MAX + g];
        }
        x += dense[D_L1W + (st * L1 + h) * L1IN + SKIP] * mob;
        l1[h] = x;
        a1[h] = clamp(x * x * ACT_SCALE, 0.0, 1.0);
        a1[L1 + h] = clamp(x, 0.0, 1.0);
    }
    workgroupBarrier();
    // L2: 64 outputs x 32 inputs; each thread takes an eighth of one
    // output's inputs, the first 64 threads add the eighths up.
    {
        let o = h % L2;
        let q = h / L2;
        let row = D_L2W + (st * L2 + o) * (L1 * 2u) + q * (L1 * 2u / 4u);
        var x = 0.0;
        for (var i = 0u; i < L1 * 2u / 4u; i++) {
            x += dense[row + i] * a1[q * (L1 * 2u / 4u) + i];
        }
        part[q * L2 + o] = x;
    }
    workgroupBarrier();
    if (h < L2) {
        var x = dense[D_L2B + st * L2 + h];
        for (var q = 0u; q < 4u; q++) {
            x += part[q * L2 + h];
        }
        l2[h] = x;
        let c = clamp(x, 0.0, 1.0);
        v2[h] = c * c * ACT_SCALE;
    }
    workgroupBarrier();
    // Output: one term per thread, added up across subgroups.
    let ow = D_OW + st * OUTIN;
    var t = 0.0;
    if (h < SKIP) {
        t = dense[ow + L2 + h] * xin[h];
    }
    if (h < L2) {
        t += dense[ow + h] * v2[h];
    }
    let ts = subgroupAdd(t);
    if (sg_inv == 0u) {
        part[sg_id] = ts;
    }
    workgroupBarrier();
    if (h == 0u) {
        var sum = 0.0;
        for (var g = 0u; g < n_sg; g++) {
            sum += part[g];
        }
        let unit = dense[D_OB + st] + sum;
        let ed = unit * SO_SCORE - tgt;
        sh_err = 2.0 * ed / SO_SCORE;
        rec[rb + R_ERR] = sh_err;
        rec[rb + R_SQ] = ed * ed;
    }
    workgroupBarrier();
    let err = sh_err;

    // Backward through the stack.
    if (h < L2) {
        let dv2 = err * dense[ow + h];
        var d = 0.0;
        if (l2[h] > 0.0 && l2[h] < 1.0) {
            d = dv2 * 2.0 * l2[h] * ACT_SCALE;
        }
        dl2[h] = d;
    }
    workgroupBarrier();
    // da1: 32 inputs x 64 outputs, eight threads per input.
    {
        let i = h % (L1 * 2u);
        let q = h / (L1 * 2u);
        var d = 0.0;
        for (var j = q * (L2 / 8u); j < (q + 1u) * (L2 / 8u); j++) {
            d += dl2[j] * dense[D_L2W + (st * L2 + j) * (L1 * 2u) + i];
        }
        part[q * (L1 * 2u) + i] = d;
    }
    workgroupBarrier();
    if (h < L1 * 2u) {
        var d = 0.0;
        for (var q = 0u; q < 8u; q++) {
            d += part[q * (L1 * 2u) + h];
        }
        da1[h] = d;
    }
    workgroupBarrier();
    if (h < L1) {
        let x = l1[h];
        let sq = x * x * ACT_SCALE;
        var d = 0.0;
        if (sq > 0.0 && sq < 1.0) {
            d += da1[h] * 2.0 * x * ACT_SCALE;
        }
        if (x > 0.0 && x < 1.0) {
            d += da1[L1 + h];
        }
        dl1[h] = d;
    }
    workgroupBarrier();
    // Gradient on the stack's input lane `h`.
    var dx = 0.0;
    if (h < SKIP) {
        dx = err * dense[ow + L2 + h];
        for (var i = 0u; i < L1; i++) {
            dx += dl1[i] * dense[D_L1W + (st * L1 + i) * L1IN + h];
        }
    }
    if (h < HH) {
        // Through the clamp, then the fold.
        let a = raw[h];
        let b = raw[h + HH];
        let ca = clamp(a, 0.0, 1.0);
        let cb = clamp(b, 0.0, 1.0);
        let acc = ca * cb * ACT_SCALE;
        var dacc = 0.0;
        if (acc > 0.0 && acc < 1.0) {
            dacc = dx;
        }
        var d0 = 0.0;
        var d1 = 0.0;
        if (a > 0.0 && a < 1.0) {
            d0 = dacc * cb * ACT_SCALE;
        }
        if (b > 0.0 && b < 1.0) {
            d1 = dacc * ca * ACT_SCALE;
        }
        rec[rb + R_DRAW + h] = d0;
        rec[rb + R_DRAW + HH + h] = d1;
    } else if (h < HH + PAD) {
        let j = h - HH;
        let z = paz[j];
        var d = 0.0;
        if (z > 0.0 && z < 1.0) {
            d = dx * 2.0 * z * ACT_SCALE;
        }
        rec[rb + R_DPA + j] = d;
    }
    if (h < SKIP) {
        rec[rb + R_XIN + h] = xin[h];
    }
    if (h == 0u) {
        rec[rb + R_XIN + SKIP] = mob;
    }
    if (h < L1 * 2u) {
        rec[rb + R_A1 + h] = a1[h];
    }
    if (h < L2) {
        rec[rb + R_V2 + h] = v2[h];
        rec[rb + R_DL2 + h] = dl2[h];
    }
    if (h < L1) {
        rec[rb + R_DL1 + h] = dl1[h];
    }
}
"#;

/// One row segment per workgroup: sum the lane gradients of up to `SEG`
/// examples that used the row (CSR order). A row that fits one segment is
/// finished here, scaled to a mean with its squared norm; a longer row's
/// segments leave raw partial sums for `K_ROW_FINISH`.
const K_ROW_GRAD: &str = r#"
@group(0) @binding(0) var<storage, read> rec: array<f32>;
@group(0) @binding(1) var<storage, read> segs: array<u32>;
@group(0) @binding(2) var<storage, read> exids: array<u32>;
@group(0) @binding(3) var<storage, read_write> grad: array<f32>;
@group(0) @binding(4) var<storage, read_write> sq: array<f32>;
@group(0) @binding(5) var<storage, read_write> part: array<f32>;
@group(0) @binding(6) var<uniform> P: Params;

var<workgroup> red: array<f32, NSG_MAX>;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(workgroup_id) wg: vec3<u32>,
        @builtin(num_workgroups) nwg: vec3<u32>,
        @builtin(subgroup_id) sg_id: u32,
        @builtin(subgroup_invocation_id) sg_inv: u32,
        @builtin(num_subgroups) n_sg: u32) {
    let t = linear_wg(wg, nwg);
    if (t >= P.n_items) {
        return;
    }
    let h = lid.x;
    let row = segs[4u * t];
    let lo = segs[4u * t + 1u];
    let hi = segs[4u * t + 2u];
    let pi = segs[4u * t + 3u];
    var s = 0.0;
    if (h < P.width) {
        for (var e = lo; e < hi; e++) {
            s += rec[exids[e] * REC + P.rec_off + h];
        }
    }
    if (pi != NO_PART) {
        if (h < P.width) {
            part[pi * P.width + h] = s;
        }
        return;
    }
    let g = s * P.scale;
    if (h < P.width) {
        grad[row * P.width + h] = g;
    }
    let gs = subgroupAdd(g * g);
    if (sg_inv == 0u) {
        red[sg_id] = gs;
    }
    workgroupBarrier();
    if (h == 0u) {
        var sum = 0.0;
        for (var i = 0u; i < n_sg; i++) {
            sum += red[i];
        }
        sq[row] = sum;
    }
}
"#;

/// One long row per workgroup: add up the partial sums its segments left.
const K_ROW_FINISH: &str = r#"
@group(0) @binding(0) var<storage, read> multi: array<u32>;
@group(0) @binding(1) var<storage, read> part: array<f32>;
@group(0) @binding(2) var<storage, read_write> grad: array<f32>;
@group(0) @binding(3) var<storage, read_write> sq: array<f32>;
@group(0) @binding(4) var<uniform> P: Params;

var<workgroup> red: array<f32, NSG_MAX>;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(workgroup_id) wg: vec3<u32>,
        @builtin(num_workgroups) nwg: vec3<u32>,
        @builtin(subgroup_id) sg_id: u32,
        @builtin(subgroup_invocation_id) sg_inv: u32,
        @builtin(num_subgroups) n_sg: u32) {
    let t = linear_wg(wg, nwg);
    if (t >= P.n_items) {
        return;
    }
    let h = lid.x;
    let row = multi[3u * t];
    let plo = multi[3u * t + 1u];
    let phi = multi[3u * t + 2u];
    var g = 0.0;
    if (h < P.width) {
        var s = 0.0;
        for (var p = plo; p < phi; p++) {
            s += part[p * P.width + h];
        }
        g = s * P.scale;
        grad[row * P.width + h] = g;
    }
    let gs = subgroupAdd(g * g);
    if (sg_inv == 0u) {
        red[sg_id] = gs;
    }
    workgroupBarrier();
    if (h == 0u) {
        var sum = 0.0;
        for (var i = 0u; i < n_sg; i++) {
            sum += red[i];
        }
        sq[row] = sum;
    }
}
"#;

/// Per chunk of `CHUNK` examples: the bias gradients' partial sums and the
/// chunk's squared error.
const K_BIAS_PART: &str = r#"
@group(0) @binding(0) var<storage, read> rec: array<f32>;
@group(0) @binding(1) var<storage, read> inp: array<u32>;
@group(0) @binding(2) var<storage, read_write> part: array<f32>;
@group(0) @binding(3) var<storage, read_write> sqe: array<f32>;
@group(0) @binding(4) var<uniform> P: Params;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(workgroup_id) wg: vec3<u32>,
        @builtin(num_workgroups) nwg: vec3<u32>) {
    let c = linear_wg(wg, nwg);
    if (c >= P.n_chunks) {
        return;
    }
    let h = lid.x;
    let lo = c * CHUNK;
    let hi = min(lo + CHUNK, P.n_ex);
    var s = 0.0;
    for (var e = lo; e < hi; e++) {
        s += rec[e * REC + R_DRAW + h];
    }
    part[c * PART + h] = s;
    if (h < PAD) {
        var sb: array<f32, PAB>;
        for (var b = 0u; b < PAB; b++) {
            sb[b] = 0.0;
        }
        for (var e = lo; e < hi; e++) {
            let stage = inp[e * (P.n_masks + 3u) + P.n_masks];
            let b = min(stage / (NST / PAB), PAB - 1u);
            sb[b] += rec[e * REC + R_DPA + h];
        }
        for (var b = 0u; b < PAB; b++) {
            part[c * PART + ACC + b * PAD + h] = sb[b];
        }
    }
    if (h == 0u) {
        var q = 0.0;
        for (var e = lo; e < hi; e++) {
            q += rec[e * REC + R_SQ];
        }
        sqe[c] = q;
    }
}
"#;

/// One thread per dense parameter: its gradient summed over the examples of
/// its stage (the read-out) or over the chunk partials (the biases).
const K_DENSE_GRAD: &str = r#"
@group(0) @binding(0) var<storage, read> rec: array<f32>;
@group(0) @binding(1) var<storage, read> stage_off: array<u32>;
@group(0) @binding(2) var<storage, read> part: array<f32>;
@group(0) @binding(3) var<storage, read_write> grad: array<f32>;
@group(0) @binding(4) var<storage, read_write> sq: array<f32>;
@group(0) @binding(5) var<uniform> P: Params;

var<workgroup> red: array<f32, 256>;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(workgroup_id) wg: vec3<u32>,
        @builtin(num_workgroups) nwg: vec3<u32>) {
    let w = linear_wg(wg, nwg);
    let e = w * 256u + lid.x;
    var g = 0.0;
    if (e < N_DENSE) {
        var s = 0.0;
        if (e < D_L1B) {
            let r = e - D_L1W;
            let st = r / (L1 * L1IN);
            let rr = r - st * (L1 * L1IN);
            let i = rr / L1IN;
            let k = rr - i * L1IN;
            for (var x = stage_off[st]; x < stage_off[st + 1u]; x++) {
                s += rec[x * REC + R_DL1 + i] * rec[x * REC + R_XIN + k];
            }
        } else if (e < D_L2W) {
            let r = e - D_L1B;
            let st = r / L1;
            let i = r - st * L1;
            for (var x = stage_off[st]; x < stage_off[st + 1u]; x++) {
                s += rec[x * REC + R_DL1 + i];
            }
        } else if (e < D_L2B) {
            let r = e - D_L2W;
            let st = r / (L2 * L1 * 2u);
            let rr = r - st * (L2 * L1 * 2u);
            let j = rr / (L1 * 2u);
            let i = rr - j * (L1 * 2u);
            for (var x = stage_off[st]; x < stage_off[st + 1u]; x++) {
                s += rec[x * REC + R_DL2 + j] * rec[x * REC + R_A1 + i];
            }
        } else if (e < D_OW) {
            let r = e - D_L2B;
            let st = r / L2;
            let j = r - st * L2;
            for (var x = stage_off[st]; x < stage_off[st + 1u]; x++) {
                s += rec[x * REC + R_DL2 + j];
            }
        } else if (e < D_OB) {
            let r = e - D_OW;
            let st = r / OUTIN;
            let q = r - st * OUTIN;
            if (q < L2) {
                for (var x = stage_off[st]; x < stage_off[st + 1u]; x++) {
                    s += rec[x * REC + R_ERR] * rec[x * REC + R_V2 + q];
                }
            } else {
                for (var x = stage_off[st]; x < stage_off[st + 1u]; x++) {
                    s += rec[x * REC + R_ERR] * rec[x * REC + R_XIN + q - L2];
                }
            }
        } else if (e < D_FTB) {
            let st = e - D_OB;
            for (var x = stage_off[st]; x < stage_off[st + 1u]; x++) {
                s += rec[x * REC + R_ERR];
            }
        } else {
            let k = e - D_FTB;
            for (var c = 0u; c < P.n_chunks; c++) {
                s += part[c * PART + k];
            }
        }
        g = s * P.scale;
        grad[e] = g;
    }
    red[lid.x] = g * g;
    workgroupBarrier();
    for (var v = 128u; v > 0u; v = v / 2u) {
        if (lid.x < v) {
            red[lid.x] += red[lid.x + v];
        }
        workgroupBarrier();
    }
    if (lid.x == 0u) {
        sq[w] = red[0];
    }
}
"#;

/// Global gradient norm from the three partial-sum arrays; the clip factor
/// every step kernel multiplies by. Also folds the batch's squared error
/// into the running shard total.
const K_FINALIZE: &str = r#"
@group(0) @binding(0) var<storage, read> sq_a: array<f32>;
@group(0) @binding(1) var<storage, read> sq_b: array<f32>;
@group(0) @binding(2) var<storage, read> sq_c: array<f32>;
@group(0) @binding(3) var<storage, read> sqe: array<f32>;
@group(0) @binding(4) var<storage, read_write> step: array<f32>;
@group(0) @binding(5) var<storage, read_write> stats: array<f32>;
@group(0) @binding(6) var<uniform> P: Params;

var<workgroup> red: array<f32, 256>;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>) {
    let h = lid.x;
    var s = 0.0;
    for (var i = h; i < P.n_a; i += 256u) {
        s += sq_a[i];
    }
    for (var i = h; i < P.n_b; i += 256u) {
        s += sq_b[i];
    }
    for (var i = h; i < P.n_c; i += 256u) {
        s += sq_c[i];
    }
    red[h] = s;
    workgroupBarrier();
    for (var w = 128u; w > 0u; w = w / 2u) {
        if (h < w) {
            red[h] += red[h + w];
        }
        workgroupBarrier();
    }
    if (h == 0u) {
        let norm = sqrt(red[0]);
        var f = 1.0;
        if (norm > CLIP_NORM && norm < 1e30) {
            f = CLIP_NORM / norm;
        }
        step[0] = f;
        step[1] = norm;
    }
    workgroupBarrier();
    var q = 0.0;
    for (var i = h; i < P.n_chunks; i += 256u) {
        q += sqe[i];
    }
    red[h] = q;
    workgroupBarrier();
    for (var w = 128u; w > 0u; w = w / 2u) {
        if (h < w) {
            red[h] += red[h + w];
        }
        workgroupBarrier();
    }
    if (h == 0u) {
        stats[0] += red[0];
        stats[1] += f32(P.n_ex);
    }
}
"#;

/// AdamW over every cell of a sparse table; a row the batch touched reads
/// its gradient through `slot`, every other row steps with a zero gradient
/// -- decay and momentum still move it, as in a dense optimizer. On a
/// Lookahead sync (`P.flags & 1`) the stepped weight is folded into the
/// slow copy in the same pass, after the step, exactly as `lookahead_sync`
/// does it -- saves reading and writing the table a second time.
const K_STEP_ROWS: &str = r#"
@group(0) @binding(0) var<storage, read_write> w: array<f32>;
@group(0) @binding(1) var<storage, read_write> m: array<f32>;
@group(0) @binding(2) var<storage, read_write> v: array<f32>;
@group(0) @binding(3) var<storage, read> slot: array<u32>;
@group(0) @binding(4) var<storage, read> grad: array<f32>;
@group(0) @binding(5) var<storage, read> step: array<f32>;
@group(0) @binding(6) var<storage, read_write> slow: array<f32>;
@group(0) @binding(7) var<uniform> P: Params;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(workgroup_id) wg: vec3<u32>,
        @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = linear_wg(wg, nwg) * 256u + lid.x;
    if (i >= P.n_items) {
        return;
    }
    let row = i / P.width;
    let h = i - row * P.width;
    let s = slot[row];
    var g = 0.0;
    if (s > 0u) {
        g = grad[(s - 1u) * P.width + h] * step[0];
    }
    let mm = P.b1 * m[i] + (1.0 - P.b1) * g;
    let vv = P.b2 * v[i] + (1.0 - P.b2) * g * g;
    m[i] = mm;
    v[i] = vv;
    var nw = w[i] * (1.0 - P.wd * P.lr) - (P.lr / P.bc1) * mm / (sqrt(vv) / P.bc2s + P.eps);
    if ((P.flags & 1u) != 0u) {
        let s = slow[i] + P.alpha * (nw - slow[i]);
        slow[i] = s;
        nw = s;
    }
    w[i] = nw;
}
"#;

/// AdamW over the dense vector. Decay on the three 2-D read-out tables,
/// the int8 clip on the two hidden ones, neither on any bias. Lookahead
/// folds in as in `K_STEP_ROWS`.
const K_STEP_DENSE: &str = r#"
@group(0) @binding(0) var<storage, read_write> w: array<f32>;
@group(0) @binding(1) var<storage, read_write> m: array<f32>;
@group(0) @binding(2) var<storage, read_write> v: array<f32>;
@group(0) @binding(3) var<storage, read> grad: array<f32>;
@group(0) @binding(4) var<storage, read> step: array<f32>;
@group(0) @binding(5) var<storage, read_write> slow: array<f32>;
@group(0) @binding(6) var<uniform> P: Params;

@compute @workgroup_size(256)
fn main(@builtin(local_invocation_id) lid: vec3<u32>,
        @builtin(workgroup_id) wg: vec3<u32>,
        @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = linear_wg(wg, nwg) * 256u + lid.x;
    if (i >= N_DENSE) {
        return;
    }
    let hidden = i < D_L1B || (i >= D_L2W && i < D_L2B);
    let decay = hidden || (i >= D_OW && i < D_OB);
    let g = grad[i] * step[0];
    let mm = P.b1 * m[i] + (1.0 - P.b1) * g;
    let vv = P.b2 * v[i] + (1.0 - P.b2) * g * g;
    m[i] = mm;
    v[i] = vv;
    var d = 0.0;
    if (decay) {
        d = P.wd;
    }
    var nw = w[i] * (1.0 - d * P.lr) - (P.lr / P.bc1) * mm / (sqrt(vv) / P.bc2s + P.eps);
    if (hidden) {
        nw = clamp(nw, -SO_MAX_W, SO_MAX_W);
    }
    if ((P.flags & 1u) != 0u) {
        let s = slow[i] + P.alpha * (nw - slow[i]);
        slow[i] = s;
        nw = s;
    }
    w[i] = nw;
}
"#;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Params {
    n_ex: u32,
    n_masks: u32,
    nfb: u32,
    n_items: u32,
    width: u32,
    rec_off: u32,
    n_chunks: u32,
    n_a: u32,
    n_b: u32,
    n_c: u32,
    flags: u32,
    pad0: u32,
    scale: f32,
    lr: f32,
    wd: f32,
    bc1: f32,
    bc2s: f32,
    alpha: f32,
    b1: f32,
    b2: f32,
    eps: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
}

impl Params {
    fn bytes(&self) -> [u8; N_PARAMS * 4] {
        let mut out = [0u8; N_PARAMS * 4];
        let words: [u32; N_PARAMS] = [
            self.n_ex,
            self.n_masks,
            self.nfb,
            self.n_items,
            self.width,
            self.rec_off,
            self.n_chunks,
            self.n_a,
            self.n_b,
            self.n_c,
            self.flags,
            self.pad0,
            self.scale.to_bits(),
            self.lr.to_bits(),
            self.wd.to_bits(),
            self.bc1.to_bits(),
            self.bc2s.to_bits(),
            self.alpha.to_bits(),
            self.b1.to_bits(),
            self.b2.to_bits(),
            self.eps.to_bits(),
            self.pad1.to_bits(),
            self.pad2.to_bits(),
            self.pad3.to_bits(),
        ];
        for (i, w) in words.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

/// A CSR over one sparse table for one batch: which rows the batch touched,
/// and which examples touched each.
struct Csr {
    counts: Vec<u32>,
    rowptr: Vec<u32>,
    exids: Vec<u32>,
    /// Row segments of at most `SEG` examples, four words each: touched
    /// row, span start and end in `exids`, and the partial-sum slot or
    /// `NO_PART` when the row is one segment.
    segs: Vec<u32>,
    /// Rows of more than one segment, three words each: touched row and
    /// its partial-sum slot range.
    multi: Vec<u32>,
    n_segs: usize,
    n_multi: usize,
    /// `slot[row]` is one past the row's index in the touched list, zero for
    /// an untouched row.
    slot: Vec<u32>,
    n_touched: usize,
}

impl Csr {
    fn new(n_rows: usize) -> Csr {
        Csr {
            counts: vec![0; n_rows],
            rowptr: Vec::new(),
            exids: Vec::new(),
            segs: Vec::new(),
            multi: Vec::new(),
            n_segs: 0,
            n_multi: 0,
            slot: vec![0; n_rows],
            n_touched: 0,
        }
    }

    /// Build from `(row, example)` pairs given as a row per `(example,
    /// mask)` cell of `rows`, `stride` per example.
    fn build(
        &mut self,
        rows: &[u32],
        n_ex: usize,
        stride: usize,
        n_masks: usize,
        row_off: impl Fn(usize) -> u32,
    ) {
        self.counts.fill(0);
        for e in 0..n_ex {
            let off = row_off(e);
            for m in 0..n_masks {
                self.counts[(rows[e * stride + m] + off) as usize] += 1;
            }
        }
        self.rowptr.clear();
        self.rowptr.push(0);
        let mut total = 0u32;
        let mut t = 0u32;
        for r in 0..self.counts.len() {
            let c = self.counts[r];
            if c == 0 {
                self.slot[r] = 0;
                continue;
            }
            t += 1;
            self.slot[r] = t;
            total += c;
            self.rowptr.push(total);
            // Reuse `counts` as the fill cursor: start of this row's span.
            self.counts[r] = total - c;
        }
        self.n_touched = t as usize;
        self.segs.clear();
        self.multi.clear();
        let mut n_part = 0u32;
        for (t, w) in self.rowptr.windows(2).enumerate() {
            let (lo, hi) = (w[0], w[1]);
            if hi - lo <= SEG {
                self.segs.extend_from_slice(&[t as u32, lo, hi, NO_PART]);
                continue;
            }
            let first = n_part;
            for a in (lo..hi).step_by(SEG as usize) {
                self.segs
                    .extend_from_slice(&[t as u32, a, hi.min(a + SEG), n_part]);
                n_part += 1;
            }
            self.multi.extend_from_slice(&[t as u32, first, n_part]);
        }
        self.n_segs = self.segs.len() / 4;
        self.n_multi = self.multi.len() / 3;
        self.exids.resize(total as usize, 0);
        for e in 0..n_ex {
            let off = row_off(e);
            for m in 0..n_masks {
                let r = (rows[e * stride + m] + off) as usize;
                let k = self.counts[r] as usize;
                self.exids[k] = e as u32;
                self.counts[r] += 1;
            }
        }
    }
}

/// One batch's inputs, prepared on the CPU.
struct Batch {
    n: usize,
    inp: Vec<u32>,
    stage_off: Vec<u32>,
    ft: Csr,
    pa: Csr,
}

struct Kernel {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

/// Per-kernel GPU time under `KUROOBI_GPU_PROF=1`: every `PROF_EVERY`th
/// batch runs each kernel in its own pass bracketed by timestamps and is
/// drained on the spot, so the sampled batches cost a little extra and
/// the rest run untouched.
struct Prof {
    qs: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    rb: wgpu::Buffer,
    period_ns: f32,
    /// Summed nanoseconds per kernel and how many samples each has.
    acc: [f64; PROF_N],
    samples: [u32; PROF_N],
}

const PROF_EVERY: u32 = 32;
const PROF_NAMES: [&str; PROF_N] = [
    "fwd_bwd",
    "row_ft",
    "row_pa",
    "fin_ft",
    "fin_pa",
    "bias_part",
    "dense_grad",
    "finalize",
    "step_ft",
    "step_pa",
    "step_dense",
    "step_ft_la",
    "step_pa_la",
    "step_dense_la",
];
const PROF_N: usize = 14;
/// Pass index of the first step kernel, and the offset to its
/// Lookahead-sync twin.
const PROF_STEP: usize = 8;
const PROF_LA: usize = 3;

pub struct GpuTrainer {
    device: wgpu::Device,
    prof: Option<Prof>,
    queue: wgpu::Queue,
    k_fwd_bwd: Kernel,
    k_row_grad: Kernel,
    k_row_finish: Kernel,
    k_bias_part: Kernel,
    k_dense_grad: Kernel,
    k_finalize: Kernel,
    k_step_rows: Kernel,
    k_step_dense: Kernel,

    n_masks: usize,
    nfb: usize,
    ft_rows: usize,
    pa_rows: usize,
    batch: usize,

    // Weights and moments.
    b_ft: wgpu::Buffer,
    b_ft_m: wgpu::Buffer,
    b_ft_v: wgpu::Buffer,
    b_pa: wgpu::Buffer,
    b_pa_m: wgpu::Buffer,
    b_pa_v: wgpu::Buffer,
    b_dense: wgpu::Buffer,
    b_dense_m: wgpu::Buffer,
    b_dense_v: wgpu::Buffer,
    la_slow: [wgpu::Buffer; 3],
    // Per-batch inputs and scratch.
    b_inp: wgpu::Buffer,
    b_rec: wgpu::Buffer,
    b_stage_off: wgpu::Buffer,
    b_ft_segs: wgpu::Buffer,
    b_ft_multi: wgpu::Buffer,
    b_ft_part: wgpu::Buffer,
    b_ft_exids: wgpu::Buffer,
    b_ft_slot: wgpu::Buffer,
    b_ft_grad: wgpu::Buffer,
    b_ft_sq: wgpu::Buffer,
    b_pa_segs: wgpu::Buffer,
    b_pa_multi: wgpu::Buffer,
    b_pa_part: wgpu::Buffer,
    b_pa_exids: wgpu::Buffer,
    b_pa_slot: wgpu::Buffer,
    b_pa_grad: wgpu::Buffer,
    b_pa_sq: wgpu::Buffer,
    b_part: wgpu::Buffer,
    b_sqe: wgpu::Buffer,
    b_dense_grad: wgpu::Buffer,
    b_dense_sq: wgpu::Buffer,
    b_step: wgpu::Buffer,
    b_stats: wgpu::Buffer,
    b_stats_read: wgpu::Buffer,
    // Uniforms, one per dispatch that needs its own values.
    u_fwd: wgpu::Buffer,
    u_row_ft: wgpu::Buffer,
    u_row_pa: wgpu::Buffer,
    u_bias: wgpu::Buffer,
    u_dense: wgpu::Buffer,
    u_fin: wgpu::Buffer,
    u_fin_ft: wgpu::Buffer,
    u_fin_pa: wgpu::Buffer,
    u_step_ft: wgpu::Buffer,
    u_step_pa: wgpu::Buffer,
    u_step_dense: wgpu::Buffer,
    // Readback staging for the epoch-end download.
    rb_ft: wgpu::Buffer,
    rb_pa: wgpu::Buffer,
    rb_dense: wgpu::Buffer,

    beta1: f32,
    beta2: f32,
    eps: f32,
    t: u32,
    la_k: u32,
    la_alpha: f32,
    la_step: u32,
    la_primed: bool,
    pending: std::collections::VecDeque<wgpu::SubmissionIndex>,
    // Double-buffered CPU preparation.
    batches: [Batch; 2],
    scratch: Vec<u32>,
}

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

fn dispatch_dims(n_wg: usize) -> (u32, u32) {
    let n = n_wg.max(1) as u32;
    let x = n.min(MAX_WG);
    (x, n.div_ceil(x))
}

impl GpuTrainer {
    /// Take the model's tables onto the GPU. `batch` is the largest batch
    /// `train_shard` will be handed; `lookahead` is `(k, alpha)`, `k = 0`
    /// for none.
    pub fn new(nn: &Nnue, batch: usize, adam: &AdamState, lookahead: (u32, f32)) -> GpuTrainer {
        assert!(
            !nn.so_grid,
            "the GPU path trains the f32 model, not the grid"
        );
        assert!(nn.base.is_none(), "the GPU path has no base evaluator");
        assert!(
            !adam.legacy_optimizer,
            "the GPU path is the corrected optimizer only"
        );
        const {
            assert!(
                SO_SKIP <= WG && ACC_DIMS == WG,
                "kernels are written for a 256-lane accumulator"
            );
            assert!(FT_BUCKETS == 1, "kernels assume one transformer copy");
            assert!(
                SO_L2 * 4 == WG && SO_L1 * 2 * 8 == WG,
                "fwd_bwd splits the middle layer four and eight ways"
            );
        }
        assert_eq!(nn.pa.len(), PA_BUCKETS * nn.n_feat_bucket * PA_DIMS);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .expect("no GPU adapter");
        let info = adapter.get_info();
        let limits = adapter.limits();
        eprintln!(
            "gpu: {} ({:?}), max buffer {} MB, max binding {} MB",
            info.name,
            info.backend,
            limits.max_buffer_size >> 20,
            limits.max_storage_buffer_binding_size >> 20
        );
        let want_prof = std::env::var_os("KUROOBI_GPU_PROF").is_some();
        let ts = wgpu::Features::TIMESTAMP_QUERY;
        let with_ts = want_prof && adapter.features().contains(ts);
        if want_prof && !with_ts {
            eprintln!("gpu: no timestamp queries on this adapter; kernel times unavailable");
        }
        assert!(
            adapter.features().contains(wgpu::Features::SUBGROUP),
            "the kernels reduce with subgroup operations, which this adapter lacks"
        );
        let mut features = wgpu::Features::SUBGROUP;
        if with_ts {
            features |= ts;
        }
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nnue_train"),
            required_features: features,
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .expect("GPU device");

        let pre = prelude();
        let make = |src: &str, label: &str| -> Kernel {
            let module = unsafe {
                device.create_shader_module_trusted(
                    wgpu::ShaderModuleDescriptor {
                        label: Some(label),
                        source: wgpu::ShaderSource::Wgsl(format!("{pre}{src}").into()),
                    },
                    wgpu::ShaderRuntimeChecks::unchecked(),
                )
            };
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
        let k_fwd_bwd = make(K_FWD_BWD, "fwd_bwd");
        let k_row_grad = make(K_ROW_GRAD, "row_grad");
        let k_row_finish = make(K_ROW_FINISH, "row_finish");
        let k_bias_part = make(K_BIAS_PART, "bias_part");
        let k_dense_grad = make(K_DENSE_GRAD, "dense_grad");
        let k_finalize = make(K_FINALIZE, "finalize");
        let k_step_rows = make(K_STEP_ROWS, "step_rows");
        let k_step_dense = make(K_STEP_DENSE, "step_dense");

        let n_masks = nn.n_masks;
        let nfb = nn.n_feat_bucket;
        let ft_rows = nn.ft.len() / ACC_DIMS;
        let pa_rows = nn.pa.len() / PA_DIMS;
        let st = wgpu::BufferUsages::STORAGE;
        let upload = |label: &str, data: &[f32], usage: wgpu::BufferUsages| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(data),
                usage,
            })
        };
        let zeros = |label: &str, n: usize, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (n.max(1) * 4) as u64,
                usage,
                mapped_at_creation: false,
            })
        };
        let mut dense = Vec::with_capacity(N_DENSE);
        dense.extend_from_slice(&nn.so_l1_w);
        dense.extend_from_slice(&nn.so_l1_b);
        dense.extend_from_slice(&nn.so_l2_w);
        dense.extend_from_slice(&nn.so_l2_b);
        dense.extend_from_slice(&nn.so_out_w);
        dense.extend_from_slice(&nn.so_out_b);
        dense.extend_from_slice(&nn.ft_bias);
        dense.extend_from_slice(&nn.pa_bias);
        assert_eq!(dense.len(), N_DENSE);

        let wu = st | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        let b_ft = upload("ft", &nn.ft, wu);
        let b_pa = upload("pa", &nn.pa, wu);
        let b_dense = upload("dense", &dense, wu);
        let b_ft_m = zeros("ft_m", nn.ft.len(), st);
        let b_ft_v = zeros("ft_v", nn.ft.len(), st);
        let b_pa_m = zeros("pa_m", nn.pa.len(), st);
        let b_pa_v = zeros("pa_v", nn.pa.len(), st);
        let b_dense_m = zeros("dense_m", N_DENSE, st);
        let b_dense_v = zeros("dense_v", N_DENSE, st);
        // Without Lookahead the step kernels still bind a slow copy; a
        // one-cell stand-in they never read.
        let la_slow = if lookahead.0 > 0 {
            [
                zeros("ft_slow", nn.ft.len(), st | wgpu::BufferUsages::COPY_DST),
                zeros("pa_slow", nn.pa.len(), st | wgpu::BufferUsages::COPY_DST),
                zeros("dense_slow", N_DENSE, st | wgpu::BufferUsages::COPY_DST),
            ]
        } else {
            [
                zeros("ft_slow", 1, st),
                zeros("pa_slow", 1, st),
                zeros("dense_slow", 1, st),
            ]
        };

        let in_stride = n_masks + 3;
        let max_occ = batch * n_masks;
        let ft_touch_max = ft_rows.min(max_occ);
        let pa_touch_max = pa_rows.min(max_occ);
        let sd = st | wgpu::BufferUsages::COPY_DST;
        let b_inp = zeros("inp", batch * in_stride, sd);
        let b_rec = zeros("rec", batch * REC, st);
        let b_stage_off = zeros("stage_off", SO_STAGES + 1, sd);
        // A row of c examples takes at most c / SEG + 1 segments; the
        // partial slots belong to rows longer than SEG, so at most twice
        // max_occ / SEG of them.
        let seg_max = |touch_max: usize| touch_max + max_occ / SEG as usize;
        let part_max = 2 * max_occ / SEG as usize;
        let b_ft_segs = zeros("ft_segs", 4 * seg_max(ft_touch_max), sd);
        let b_ft_multi = zeros("ft_multi", 3 * part_max, sd);
        let b_ft_part = zeros("ft_part", part_max * ACC_DIMS, st);
        let b_ft_exids = zeros("ft_exids", max_occ, sd);
        let b_ft_slot = zeros("ft_slot", ft_rows, sd);
        let b_ft_grad = zeros("ft_grad", ft_touch_max * ACC_DIMS, st);
        let b_ft_sq = zeros("ft_sq", ft_touch_max, st);
        let b_pa_segs = zeros("pa_segs", 4 * seg_max(pa_touch_max), sd);
        let b_pa_multi = zeros("pa_multi", 3 * part_max, sd);
        let b_pa_part = zeros("pa_part", part_max * PA_DIMS, st);
        let b_pa_exids = zeros("pa_exids", max_occ, sd);
        let b_pa_slot = zeros("pa_slot", pa_rows, sd);
        let b_pa_grad = zeros("pa_grad", pa_touch_max * PA_DIMS, st);
        let b_pa_sq = zeros("pa_sq", pa_touch_max, st);
        let n_chunks_max = batch.div_ceil(CHUNK);
        let b_part = zeros("part", n_chunks_max * PART, st);
        let b_sqe = zeros("sqe", n_chunks_max, st);
        let b_dense_grad = zeros("dense_grad", N_DENSE, st);
        let b_dense_sq = zeros("dense_sq", N_DENSE.div_ceil(WG), st);
        let b_step = zeros("step", 4, st);
        let b_stats = zeros(
            "stats",
            4,
            st | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        );
        let b_stats_read = zeros(
            "stats_read",
            4,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let uni = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("params"),
                size: (N_PARAMS * 4) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let rb = |label: &str, n: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (n * 4) as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let rb_ft = rb("rb_ft", nn.ft.len());
        let rb_pa = rb("rb_pa", nn.pa.len());
        let rb_dense = rb("rb_dense", N_DENSE);

        let mk_batch = || Batch {
            n: 0,
            inp: vec![0; batch * in_stride],
            stage_off: vec![0; SO_STAGES + 1],
            ft: Csr::new(ft_rows),
            pa: Csr::new(pa_rows),
        };
        let (u_fwd, u_row_ft, u_row_pa, u_fin_ft, u_fin_pa, u_bias, u_dense, u_fin) =
            (uni(), uni(), uni(), uni(), uni(), uni(), uni(), uni());
        let (u_step_ft, u_step_pa, u_step_dense) = (uni(), uni(), uni());
        let prof = with_ts.then(|| {
            let n = 2 * PROF_N as u32;
            Prof {
                qs: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("prof"),
                    ty: wgpu::QueryType::Timestamp,
                    count: n,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("prof_resolve"),
                    size: 8 * n as u64,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                rb: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("prof_rb"),
                    size: 8 * n as u64,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                period_ns: queue.get_timestamp_period(),
                acc: [0.0; PROF_N],
                samples: [0; PROF_N],
            }
        });
        GpuTrainer {
            device,
            prof,
            queue,
            k_fwd_bwd,
            k_row_grad,
            k_row_finish,
            k_bias_part,
            k_dense_grad,
            k_finalize,
            k_step_rows,
            k_step_dense,
            n_masks,
            nfb,
            ft_rows,
            pa_rows,
            batch,
            b_ft,
            b_ft_m,
            b_ft_v,
            b_pa,
            b_pa_m,
            b_pa_v,
            b_dense,
            b_dense_m,
            b_dense_v,
            la_slow,
            b_inp,
            b_rec,
            b_stage_off,
            b_ft_segs,
            b_ft_multi,
            b_ft_part,
            b_ft_exids,
            b_ft_slot,
            b_ft_grad,
            b_ft_sq,
            b_pa_segs,
            b_pa_multi,
            b_pa_part,
            b_pa_exids,
            b_pa_slot,
            b_pa_grad,
            b_pa_sq,
            b_part,
            b_sqe,
            b_dense_grad,
            b_dense_sq,
            b_step,
            b_stats,
            b_stats_read,
            u_fwd,
            u_row_ft,
            u_row_pa,
            u_bias,
            u_dense,
            u_fin,
            u_fin_ft,
            u_fin_pa,
            u_step_ft,
            u_step_pa,
            u_step_dense,
            rb_ft,
            rb_pa,
            rb_dense,
            beta1: adam.beta1,
            beta2: adam.beta2,
            eps: adam.eps,
            t: 0,
            la_k: lookahead.0,
            la_alpha: lookahead.1,
            la_step: 0,
            la_primed: false,
            pending: Default::default(),
            batches: [mk_batch(), mk_batch()],
            scratch: vec![0; batch * in_stride],
        }
    }

    /// Optimizer steps taken so far.
    pub fn steps(&self) -> u32 {
        self.t
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

    /// Fill `batches[slot]` from `examples`: symmetry draw, feature rows,
    /// stage sort, both CSRs.
    fn prepare(
        &mut self,
        nn: &Nnue,
        examples: &[Example],
        slot: usize,
        threads: usize,
        sym: SymPlan,
        bno: u64,
    ) {
        let n = examples.len();
        let nm = self.n_masks;
        let stride = nm + 3;
        let scratch = &mut self.scratch[..n * stride];
        let threads = threads.max(1);
        let per = n.div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (ti, (exs, out)) in examples
                .chunks(per)
                .zip(scratch.chunks_mut(per * stride))
                .enumerate()
            {
                scope.spawn(move || {
                    let mut rs = sym.stream((ti as u64 + 1) * 0x1000 + bno + 1);
                    for (ex, o) in exs.iter().zip(out.chunks_mut(stride)) {
                        let ex = sym.apply(ex, &mut rs);
                        let board = ex.board();
                        let stage = crate::evaluator::Evaluator::stage(&board);
                        let ix = nn.indices(ex.black, ex.white);
                        let f = nn.features_black(&ix, stage);
                        o[..nm].copy_from_slice(&f[..nm]);
                        o[nm] = stage as u32;
                        o[nm + 1] = mob_unit(Nnue::mob_index(&board)).to_bits();
                        o[nm + 2] = ex.score.to_bits();
                    }
                });
            }
        });
        // Counting sort by read-out stage.
        let b = &mut self.batches[slot];
        b.n = n;
        let mut cnt = [0u32; SO_STAGES + 1];
        for e in 0..n {
            cnt[so_stage(scratch[e * stride + nm] as usize) + 1] += 1;
        }
        for s in 0..SO_STAGES {
            cnt[s + 1] += cnt[s];
        }
        b.stage_off.copy_from_slice(&cnt);
        let mut cur = cnt;
        for e in 0..n {
            let st = so_stage(scratch[e * stride + nm] as usize);
            let k = cur[st] as usize;
            cur[st] += 1;
            b.inp[k * stride..k * stride + stride]
                .copy_from_slice(&scratch[e * stride..e * stride + stride]);
        }
        let inp = &b.inp[..n * stride];
        b.ft.build(inp, n, stride, nm, |_| 0);
        let nfb = self.nfb as u32;
        b.pa.build(inp, n, stride, nm, |e| {
            pa_bucket(inp[e * stride + nm] as usize) as u32 * nfb
        });
    }

    /// Upload `batches[slot]` and record one optimizer step.
    fn submit(&mut self, slot: usize, lr: f32, wd: f32) -> wgpu::SubmissionIndex {
        let b = &self.batches[slot];
        let n = b.n;
        let stride = self.n_masks + 3;
        let q = &self.queue;
        fn u32s(v: &[u32]) -> &[u8] {
            bytemuck::cast_slice(v)
        }
        q.write_buffer(&self.b_inp, 0, u32s(&b.inp[..n * stride]));
        q.write_buffer(&self.b_stage_off, 0, u32s(&b.stage_off));
        q.write_buffer(&self.b_ft_segs, 0, u32s(&b.ft.segs));
        q.write_buffer(&self.b_ft_multi, 0, u32s(&b.ft.multi));
        q.write_buffer(&self.b_ft_exids, 0, u32s(&b.ft.exids));
        q.write_buffer(&self.b_ft_slot, 0, u32s(&b.ft.slot));
        q.write_buffer(&self.b_pa_segs, 0, u32s(&b.pa.segs));
        q.write_buffer(&self.b_pa_multi, 0, u32s(&b.pa.multi));
        q.write_buffer(&self.b_pa_exids, 0, u32s(&b.pa.exids));
        q.write_buffer(&self.b_pa_slot, 0, u32s(&b.pa.slot));

        self.t += 1;
        // Lookahead, mirroring `lookahead_sync`: the first sync only takes
        // the slow copy; every later one folds into the step kernels.
        let mut la_prime = false;
        let mut la_fold = false;
        if self.la_k > 0 {
            self.la_step = self.la_step.wrapping_add(1);
            if self.la_step.is_multiple_of(self.la_k) {
                la_prime = !self.la_primed;
                la_fold = self.la_primed;
                self.la_primed = true;
            }
        }
        let bc1 = 1.0 - self.beta1.powi(self.t as i32);
        let bc2s = (1.0 - self.beta2.powi(self.t as i32)).sqrt();
        let scale = 1.0 / n as f32;
        let n_chunks = n.div_ceil(CHUNK);
        let base = Params {
            n_ex: n as u32,
            n_masks: self.n_masks as u32,
            nfb: self.nfb as u32,
            n_chunks: n_chunks as u32,
            scale,
            lr,
            wd,
            bc1,
            bc2s,
            b1: self.beta1,
            b2: self.beta2,
            eps: self.eps,
            ..Default::default()
        };
        q.write_buffer(&self.u_fwd, 0, &base.bytes());
        q.write_buffer(
            &self.u_row_ft,
            0,
            &Params {
                n_items: b.ft.n_segs as u32,
                width: ACC_DIMS as u32,
                rec_off: R_DRAW as u32,
                ..base
            }
            .bytes(),
        );
        q.write_buffer(
            &self.u_fin_ft,
            0,
            &Params {
                n_items: b.ft.n_multi as u32,
                width: ACC_DIMS as u32,
                ..base
            }
            .bytes(),
        );
        q.write_buffer(
            &self.u_row_pa,
            0,
            &Params {
                n_items: b.pa.n_segs as u32,
                width: PA_DIMS as u32,
                rec_off: R_DPA as u32,
                ..base
            }
            .bytes(),
        );
        q.write_buffer(
            &self.u_fin_pa,
            0,
            &Params {
                n_items: b.pa.n_multi as u32,
                width: PA_DIMS as u32,
                ..base
            }
            .bytes(),
        );
        q.write_buffer(&self.u_bias, 0, &base.bytes());
        q.write_buffer(&self.u_dense, 0, &base.bytes());
        let n_dense_wg = N_DENSE.div_ceil(WG);
        q.write_buffer(
            &self.u_fin,
            0,
            &Params {
                n_a: b.ft.n_touched as u32,
                n_b: b.pa.n_touched as u32,
                n_c: n_dense_wg as u32,
                ..base
            }
            .bytes(),
        );
        let step_base = Params {
            flags: la_fold as u32,
            alpha: self.la_alpha,
            ..base
        };
        q.write_buffer(
            &self.u_step_ft,
            0,
            &Params {
                n_items: (self.ft_rows * ACC_DIMS) as u32,
                width: ACC_DIMS as u32,
                ..step_base
            }
            .bytes(),
        );
        q.write_buffer(
            &self.u_step_pa,
            0,
            &Params {
                n_items: (self.pa_rows * PA_DIMS) as u32,
                width: PA_DIMS as u32,
                ..step_base
            }
            .bytes(),
        );
        q.write_buffer(&self.u_step_dense, 0, &step_base.bytes());

        let bg_fwd = self.bind(
            &self.k_fwd_bwd,
            &[
                &self.b_inp,
                &self.b_ft,
                &self.b_pa,
                &self.b_dense,
                &self.b_rec,
                &self.u_fwd,
            ],
        );
        let bg_row_ft = self.bind(
            &self.k_row_grad,
            &[
                &self.b_rec,
                &self.b_ft_segs,
                &self.b_ft_exids,
                &self.b_ft_grad,
                &self.b_ft_sq,
                &self.b_ft_part,
                &self.u_row_ft,
            ],
        );
        let bg_fin_ft = self.bind(
            &self.k_row_finish,
            &[
                &self.b_ft_multi,
                &self.b_ft_part,
                &self.b_ft_grad,
                &self.b_ft_sq,
                &self.u_fin_ft,
            ],
        );
        let bg_row_pa = self.bind(
            &self.k_row_grad,
            &[
                &self.b_rec,
                &self.b_pa_segs,
                &self.b_pa_exids,
                &self.b_pa_grad,
                &self.b_pa_sq,
                &self.b_pa_part,
                &self.u_row_pa,
            ],
        );
        let bg_fin_pa = self.bind(
            &self.k_row_finish,
            &[
                &self.b_pa_multi,
                &self.b_pa_part,
                &self.b_pa_grad,
                &self.b_pa_sq,
                &self.u_fin_pa,
            ],
        );
        let bg_bias = self.bind(
            &self.k_bias_part,
            &[
                &self.b_rec,
                &self.b_inp,
                &self.b_part,
                &self.b_sqe,
                &self.u_bias,
            ],
        );
        let bg_dense = self.bind(
            &self.k_dense_grad,
            &[
                &self.b_rec,
                &self.b_stage_off,
                &self.b_part,
                &self.b_dense_grad,
                &self.b_dense_sq,
                &self.u_dense,
            ],
        );
        let bg_fin = self.bind(
            &self.k_finalize,
            &[
                &self.b_ft_sq,
                &self.b_pa_sq,
                &self.b_dense_sq,
                &self.b_sqe,
                &self.b_step,
                &self.b_stats,
                &self.u_fin,
            ],
        );
        let bg_step_ft = self.bind(
            &self.k_step_rows,
            &[
                &self.b_ft,
                &self.b_ft_m,
                &self.b_ft_v,
                &self.b_ft_slot,
                &self.b_ft_grad,
                &self.b_step,
                &self.la_slow[0],
                &self.u_step_ft,
            ],
        );
        let bg_step_pa = self.bind(
            &self.k_step_rows,
            &[
                &self.b_pa,
                &self.b_pa_m,
                &self.b_pa_v,
                &self.b_pa_slot,
                &self.b_pa_grad,
                &self.b_step,
                &self.la_slow[1],
                &self.u_step_pa,
            ],
        );
        let bg_step_dense = self.bind(
            &self.k_step_dense,
            &[
                &self.b_dense,
                &self.b_dense_m,
                &self.b_dense_v,
                &self.b_dense_grad,
                &self.b_step,
                &self.la_slow[2],
                &self.u_step_dense,
            ],
        );

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("step"),
            });
        // A sampled batch brackets every pass with timestamps.
        let sampled = self.prof.is_some() && self.t.is_multiple_of(PROF_EVERY);
        let qs = self.prof.as_ref().map(|p| &p.qs);
        let timestamps = |i: usize| {
            qs.filter(|_| sampled)
                .map(|qs| wgpu::ComputePassTimestampWrites {
                    query_set: qs,
                    beginning_of_pass_write_index: Some(2 * i as u32),
                    end_of_pass_write_index: Some(2 * i as u32 + 1),
                })
        };
        {
            let mut run = |i: usize, k: &Kernel, bg: &wgpu::BindGroup, n_wg: usize| {
                let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some(PROF_NAMES[i]),
                    timestamp_writes: timestamps(i),
                });
                let (x, y) = dispatch_dims(n_wg);
                pass.set_pipeline(&k.pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.dispatch_workgroups(x, y, 1);
            };
            run(0, &self.k_fwd_bwd, &bg_fwd, n);
            run(1, &self.k_row_grad, &bg_row_ft, b.ft.n_segs);
            run(2, &self.k_row_grad, &bg_row_pa, b.pa.n_segs);
            if b.ft.n_multi > 0 {
                run(3, &self.k_row_finish, &bg_fin_ft, b.ft.n_multi);
            }
            if b.pa.n_multi > 0 {
                run(4, &self.k_row_finish, &bg_fin_pa, b.pa.n_multi);
            }
            run(5, &self.k_bias_part, &bg_bias, n_chunks);
            run(6, &self.k_dense_grad, &bg_dense, n_dense_wg);
            run(7, &self.k_finalize, &bg_fin, 1);
            // A sync batch's step passes are timed under their own names.
            let st = PROF_STEP + if la_fold { PROF_LA } else { 0 };
            run(
                st,
                &self.k_step_rows,
                &bg_step_ft,
                (self.ft_rows * ACC_DIMS).div_ceil(WG),
            );
            run(
                st + 1,
                &self.k_step_rows,
                &bg_step_pa,
                (self.pa_rows * PA_DIMS).div_ceil(WG),
            );
            run(st + 2, &self.k_step_dense, &bg_step_dense, n_dense_wg);
        }
        if la_prime {
            let fast = [&self.b_ft, &self.b_pa, &self.b_dense];
            for (f, s) in fast.iter().zip(self.la_slow.iter()) {
                enc.copy_buffer_to_buffer(f, 0, s, 0, None);
            }
        }
        if sampled {
            let p = self.prof.as_ref().unwrap();
            let n = 2 * PROF_N as u32;
            enc.resolve_query_set(&p.qs, 0..n, &p.resolve, 0);
            enc.copy_buffer_to_buffer(&p.resolve, 0, &p.rb, 0, None);
        }
        let idx = self.queue.submit([enc.finish()]);
        if sampled {
            self.pending.push_back(idx.clone());
            self.drain(0);
            self.take_prof_sample(la_fold);
            // Row-length spread: a row's gradient is one workgroup's serial
            // loop, so the longest row bounds `row_grad`.
            for (name, c) in [
                ("ft", &self.batches[slot].ft),
                ("pa", &self.batches[slot].pa),
            ] {
                let lens = c.rowptr.windows(2).map(|w| w[1] - w[0]);
                let (mut mx, mut over) = (0, 0usize);
                for l in lens {
                    mx = mx.max(l);
                    over += (l > 1024) as usize;
                }
                eprintln!(
                    "gpu prof: {name} rows touched {} max len {mx} rows>1024 {over}",
                    c.n_touched
                );
            }
        }
        idx
    }

    /// Read the sampled batch's timestamps into the running sums.
    fn take_prof_sample(&mut self, la_fold: bool) {
        let p = self.prof.as_mut().unwrap();
        let bytes = 8 * 2 * PROF_N as u64;
        let (tx, rx) = mpsc::channel();
        p.rb.slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("GPU wait");
        rx.recv().expect("map callback").expect("map");
        let mut ts = [0u64; 2 * PROF_N];
        {
            let view = p.rb.slice(..bytes).get_mapped_range();
            ts.copy_from_slice(bytemuck::cast_slice(&view));
        }
        p.rb.unmap();
        for i in 0..PROF_N {
            // The step passes ran under one of their two names.
            if i >= PROF_STEP && (i >= PROF_STEP + PROF_LA) != la_fold {
                continue;
            }
            p.acc[i] += ts[2 * i + 1].wrapping_sub(ts[2 * i]) as f64 * p.period_ns as f64;
            p.samples[i] += 1;
        }
    }

    /// Print and reset the per-kernel means (ms per batch).
    fn report_prof(&mut self) {
        let Some(p) = self.prof.as_mut() else { return };
        let mut line = String::from("gpu kernels ms/batch:");
        let mut total = 0.0;
        for i in 0..PROF_N {
            if p.samples[i] == 0 {
                continue;
            }
            let ms = p.acc[i] / p.samples[i] as f64 / 1e6;
            // A step pass runs in its Lookahead form one batch in k; charge
            // each form its share per batch.
            let k = self.la_k.max(1) as f64;
            let per_batch = if i >= PROF_STEP + PROF_LA {
                ms / k
            } else if i >= PROF_STEP && self.la_k > 0 {
                ms * (k - 1.0) / k
            } else {
                ms
            };
            total += per_batch;
            line += &format!(" {} {:.2}", PROF_NAMES[i], ms);
        }
        line += &format!("  (sum per batch {total:.2}, {} samples)", p.samples[0]);
        eprintln!("{line}");
        p.acc = [0.0; PROF_N];
        p.samples = [0; PROF_N];
    }

    /// Wait until at most `keep` submissions are still in flight.
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

    /// One pass over `examples` in batches, one optimizer step each. Returns
    /// the sum of squared errors. `nn` supplies the feature indexer only;
    /// its tables are stale until `download`.
    pub fn train_shard(
        &mut self,
        nn: &Nnue,
        examples: &[Example],
        threads: usize,
        lr_for_step: &mut impl FnMut() -> f32,
        wd: f32,
        sym: SymPlan,
    ) -> f64 {
        self.queue.write_buffer(&self.b_stats, 0, &[0u8; 16]);
        let mut slot = 0;
        // `KUROOBI_GPU_PROF=1` prints where the shard's time went: waiting
        // on the GPU, building the batch on the CPU, or uploading it.
        let prof = std::env::var_os("KUROOBI_GPU_PROF").is_some();
        let (mut t_wait, mut t_prep, mut t_submit) = (0.0f64, 0.0f64, 0.0f64);
        for (bno, chunk) in examples.chunks(self.batch).enumerate() {
            // The slot's previous batch must have been consumed by the
            // queue: `write_buffer` copies at submit, so two batches in
            // flight is the limit on reuse.
            let t0 = std::time::Instant::now();
            self.drain(1);
            let t1 = std::time::Instant::now();
            self.prepare(nn, chunk, slot, threads, sym, bno as u64);
            let t2 = std::time::Instant::now();
            let lr = lr_for_step();
            let idx = self.submit(slot, lr, wd);
            let t3 = std::time::Instant::now();
            t_wait += (t1 - t0).as_secs_f64();
            t_prep += (t2 - t1).as_secs_f64();
            t_submit += (t3 - t2).as_secs_f64();
            self.pending.push_back(idx);
            slot ^= 1;
        }
        let t0 = std::time::Instant::now();
        self.drain(0);
        t_wait += t0.elapsed().as_secs_f64();
        if prof {
            eprintln!("gpu prof: wait {t_wait:.2}s prep {t_prep:.2}s submit {t_submit:.2}s");
            self.report_prof();
        }
        let mut out = [0f32; 4];
        self.read_back(&self.b_stats, &self.b_stats_read, &mut out);
        out[0] as f64
    }

    fn read_back(&self, src: &wgpu::Buffer, staging: &wgpu::Buffer, out: &mut [f32]) {
        let bytes = (out.len() * 4) as u64;
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback"),
            });
        enc.copy_buffer_to_buffer(src, 0, staging, 0, bytes);
        let idx = self.queue.submit([enc.finish()]);
        let (tx, rx) = mpsc::channel();
        staging
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(idx),
                timeout: None,
            })
            .expect("GPU wait");
        rx.recv().expect("map callback").expect("map");
        {
            let view = staging.slice(..bytes).get_mapped_range();
            out.copy_from_slice(bytemuck::cast_slice(&view));
        }
        staging.unmap();
    }

    /// Bring the trained tables back into `nn` (for the held-out pass and
    /// the save). The optimizer's moments stay on the GPU.
    pub fn download(&mut self, nn: &mut Nnue) {
        self.drain(0);
        self.read_back(&self.b_ft, &self.rb_ft, &mut nn.ft);
        self.read_back(&self.b_pa, &self.rb_pa, &mut nn.pa);
        let mut dense = vec![0f32; N_DENSE];
        self.read_back(&self.b_dense, &self.rb_dense, &mut dense);
        nn.so_l1_w.copy_from_slice(&dense[D_L1W..D_L1B]);
        nn.so_l1_b.copy_from_slice(&dense[D_L1B..D_L2W]);
        nn.so_l2_w.copy_from_slice(&dense[D_L2W..D_L2B]);
        nn.so_l2_b.copy_from_slice(&dense[D_L2B..D_OW]);
        nn.so_out_w.copy_from_slice(&dense[D_OW..D_OB]);
        nn.so_out_b.copy_from_slice(&dense[D_OB..D_FTB]);
        nn.ft_bias.copy_from_slice(&dense[D_FTB..D_PAB]);
        nn.pa_bias.copy_from_slice(&dense[D_PAB..N_DENSE]);
        nn.refresh_so_grid();
    }
}
