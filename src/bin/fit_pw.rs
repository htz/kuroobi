//! Closed-form fit of the product-gate weights `pw` with everything else
//! frozen.
//!
//! With the feature transformer fixed, the product features
//! `z_i = φ(acc_i)·φ(acc_{i+H/2}) / PROD_CLAMP` are just 8 fixed regressors
//! per stage, so the optimal `pw` is per-stage ridge least squares on the
//! current model's residuals — the exact upper bound of what the gate can
//! add without retraining the transformer (same idea as `set_num_w`'s
//! closed-form disc-table fit).
//!
//! Usage: fit_pw --init <model.bin> [--ridge f] [--limit n] [--out path]
//!               [--val file]... <data-file>...
// Index-based loops mirror the textbook Gaussian-elimination form; iterator
// rewrites obscure the row/column structure.
#![allow(clippy::needless_range_loop)]
use kuroobi::nnue::{Nnue, H};
use kuroobi::pattern::EGAROUCID_PATTERNS;
use std::path::PathBuf;

const HALF: usize = H / 2;

/// 17-byte packed example: black u64, white u64, score i8 (LE, rank-major).
fn read_examples(path: &PathBuf, cap: usize) -> Vec<(u64, u64, f32)> {
    let bytes = std::fs::read(path).expect("read data");
    let n = (bytes.len() / 17).min(cap);
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let b = &bytes[i * 17..i * 17 + 17];
        let black = u64::from_le_bytes(b[0..8].try_into().unwrap());
        let white = u64::from_le_bytes(b[8..16].try_into().unwrap());
        let score = b[16] as i8 as f32;
        v.push((black, white, score));
    }
    v
}

/// Solve A x = b for a symmetric positive-definite HALF x HALF system.
fn solve(a: &mut [[f64; HALF]; HALF], b: &mut [f64; HALF]) -> [f64; HALF] {
    for i in 0..HALF {
        let piv = a[i][i];
        if piv.abs() < 1e-12 {
            continue;
        }
        for j in i + 1..HALF {
            let f = a[j][i] / piv;
            for k in i..HALF {
                a[j][k] -= f * a[i][k];
            }
            b[j] -= f * b[i];
        }
    }
    let mut x = [0.0f64; HALF];
    for i in (0..HALF).rev() {
        let mut s = b[i];
        for k in i + 1..HALF {
            s -= a[i][k] * x[k];
        }
        x[i] = if a[i][i].abs() < 1e-12 {
            0.0
        } else {
            s / a[i][i]
        };
    }
    x
}

fn main() {
    let mut init: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut val_files: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut ridge = 1e3f64;
    let mut limit = usize::MAX;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--init" => init = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            "--val" => val_files.push(PathBuf::from(it.next().unwrap())),
            "--ridge" => ridge = it.next().unwrap().parse().unwrap(),
            "--limit" => limit = it.next().unwrap().parse().unwrap(),
            other => files.push(PathBuf::from(other)),
        }
    }
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(&init.expect("--init required")).expect("load");

    // Per-stage normal equations over the product features.
    let mut mats = vec![[[0.0f64; HALF]; HALF]; kuroobi::evaluator::STAGE_COUNT];
    let mut rhss = vec![[0.0f64; HALF]; kuroobi::evaluator::STAGE_COUNT];
    let mut seen = 0usize;
    let per_file = limit / files.len().max(1);
    for f in &files {
        for (black, white, target) in read_examples(f, per_file) {
            let (stage, z, resid) = nn.product_features_black(black, white, target);
            let m = &mut mats[stage];
            let r = &mut rhss[stage];
            for i in 0..HALF {
                for j in 0..HALF {
                    m[i][j] += (z[i] * z[j]) as f64;
                }
                r[i] += (z[i] * resid) as f64;
            }
            seen += 1;
        }
    }
    eprintln!("accumulated {seen} examples");

    let mut pw = vec![0.0f32; kuroobi::evaluator::STAGE_COUNT * HALF];
    for st in 0..kuroobi::evaluator::STAGE_COUNT {
        let mut a = mats[st];
        let mut b = rhss[st];
        for i in 0..HALF {
            a[i][i] += ridge;
        }
        let x = solve(&mut a, &mut b);
        for i in 0..HALF {
            pw[st * HALF + i] = x[i] as f32;
        }
    }
    nn.set_pw(&pw);
    let mx = pw.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    eprintln!("fit done, max |pw| = {mx:.4}");

    // Validation MSE before/after (before = pw zeroed).
    for vf in &val_files {
        let examples = read_examples(vf, 2_000_000);
        let mut se0 = 0.0f64;
        let mut se1 = 0.0f64;
        for &(black, white, target) in &examples {
            let (e0, e1) = nn.eval_black_with_without_pw(black, white);
            se0 += ((e0 - target) as f64).powi(2);
            se1 += ((e1 - target) as f64).powi(2);
        }
        let n = examples.len() as f64;
        eprintln!(
            "{}: val without pw {:.4}  with pw {:.4}  (n={})",
            vf.display(),
            se0 / n,
            se1 / n,
            examples.len()
        );
    }
    if let Some(o) = out {
        nn.save(&o).expect("save");
        eprintln!("saved {}", o.display());
    }
}
