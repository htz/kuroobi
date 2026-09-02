//! Smooth a linear model across neighbouring stages.
//!
//! Adjacent stages differ by one ply: the positions, the pattern statistics
//! and the amount of data behind them all vary continuously, so their weight
//! tables should too. Training them independently does not enforce that --
//! each stage sees its own sample of a noisy target and lands wherever that
//! sample points, so a stage can end up worse than the average of the two
//! either side of it.
//!
//! For each stage this tries the stage's own table, each neighbour's table,
//! and the pairwise and three-way means, scores every candidate on that
//! stage's own held-out positions, and keeps the best. A stage that is
//! already the best of the six is left exactly as it was, so this can only
//! help on the set it is measured against.
//!
//! Candidates are built from a snapshot taken before anything is written, so
//! a stage never smooths against a neighbour this run already changed.
//!
//! Usage: stage_smooth --val <file.data> --in <weights.bin> --out <path>
use kuroobi::evaluator::{Evaluator, STAGE_COUNT};
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::trainer::{load_examples_binary_into, Example};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type Snapshot = (Vec<Vec<f32>>, Vec<f32>);

/// Element-wise mean of several stages' tables.
fn mean_of(parts: &[&Snapshot]) -> Snapshot {
    let n = parts.len() as f32;
    let mut w = parts[0].0.clone();
    for (pi, table) in w.iter_mut().enumerate() {
        for (ci, cell) in table.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for p in parts {
                acc += p.0[pi][ci];
            }
            *cell = acc / n;
        }
    }
    let mut num = parts[0].1.clone();
    for (i, cell) in num.iter_mut().enumerate() {
        let mut acc = 0.0f32;
        for p in parts {
            acc += p.1[i];
        }
        *cell = acc / n;
    }
    (w, num)
}

fn mae_on(ev: &Evaluator, rows: &[Example]) -> f64 {
    if rows.is_empty() {
        return f64::INFINITY;
    }
    let mut acc = 0.0f64;
    for ex in rows {
        acc += (ex.score as f64 - ev.eval(&ex.board()) as f64).abs();
    }
    acc / rows.len() as f64
}

fn main() -> ExitCode {
    let mut val_path: Option<PathBuf> = None;
    let mut in_path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--val" => val_path = it.next().map(PathBuf::from),
            "--in" => in_path = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let (Some(val_path), Some(in_path), Some(out)) = (val_path, in_path, out) else {
        eprintln!("usage: stage_smooth --val <file.data> --in <weights.bin> --out <path>");
        return ExitCode::FAILURE;
    };

    let mut val = Vec::new();
    if let Err(e) = load_examples_binary_into(&val_path, &mut val, None) {
        eprintln!("failed to read {}: {e}", val_path.display());
        return ExitCode::FAILURE;
    }
    let mut by_stage: Vec<Vec<Example>> = vec![Vec::new(); STAGE_COUNT];
    for ex in &val {
        by_stage[Evaluator::stage(&ex.board())].push(*ex);
    }

    let mut ev = Evaluator::new(EGAROUCID_PATTERNS);
    if let Err(e) = ev.load_weights(Path::new(&in_path)) {
        eprintln!("failed to load {}: {e}", in_path.display());
        return ExitCode::FAILURE;
    }
    let snap: Vec<Snapshot> = (0..STAGE_COUNT).map(|st| ev.stage_weights(st)).collect();

    let mut changed = 0usize;
    let mut total_gain = 0.0f64;
    println!("| 空き | n | 現状 | 採用 | 採用後 | 改善 |");
    println!("|---:|---:|---:|---|---:|---:|");
    for st in 0..STAGE_COUNT {
        let rows = &by_stage[st];
        if rows.is_empty() {
            continue;
        }
        // Candidates: own, each neighbour, and the means.
        let mut cands: Vec<(String, Snapshot)> = vec![("自分".into(), snap[st].clone())];
        let lo = st.checked_sub(1).filter(|_| st > 0);
        let hi = if st + 1 < STAGE_COUNT {
            Some(st + 1)
        } else {
            None
        };
        if let Some(l) = lo {
            cands.push(("隣-1".into(), snap[l].clone()));
            cands.push(("平均(-1,0)".into(), mean_of(&[&snap[l], &snap[st]])));
        }
        if let Some(h) = hi {
            cands.push(("隣+1".into(), snap[h].clone()));
            cands.push(("平均(0,+1)".into(), mean_of(&[&snap[st], &snap[h]])));
        }
        if let (Some(l), Some(h)) = (lo, hi) {
            cands.push((
                "平均(-1,0,+1)".into(),
                mean_of(&[&snap[l], &snap[st], &snap[h]]),
            ));
        }

        let mut best = 0usize;
        let mut best_mae = f64::INFINITY;
        let mut own_mae = f64::INFINITY;
        for (i, (_, c)) in cands.iter().enumerate() {
            ev.set_stage_weights(st, &c.0, &c.1);
            let m = mae_on(&ev, rows);
            if i == 0 {
                own_mae = m;
            }
            if m < best_mae {
                best_mae = m;
                best = i;
            }
        }
        ev.set_stage_weights(st, &cands[best].1 .0, &cands[best].1 .1);
        if best != 0 {
            changed += 1;
            total_gain += own_mae - best_mae;
        }
        println!(
            "| {} | {} | {:.3} | {} | {:.3} | {:+.3} |",
            60 - st,
            rows.len(),
            own_mae,
            cands[best].0,
            best_mae,
            best_mae - own_mae
        );
    }
    println!("\n{changed} ステージを差し替え、合計改善 {total_gain:.3}");
    if let Err(e) = ev.save_weights(&out) {
        eprintln!("failed to save {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    println!("saved to {}", out.display());
    ExitCode::SUCCESS
}
