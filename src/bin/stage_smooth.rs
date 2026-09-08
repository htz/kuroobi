//! Smooth a linear model across neighbouring stages, at the best blend found.
//!
//! Adjacent stages differ by one ply: the positions, the pattern statistics
//! and the amount of data behind them all vary continuously, so their weight
//! tables should too. Training them independently does not enforce that --
//! each stage sees its own sample of a noisy target and lands wherever that
//! sample points, so a stage can end up worse than the average of the two
//! either side of it.
//!
//! An earlier version of this tool tried six fixed candidates (the stage
//! itself, each neighbour, the pairwise means and the three-way mean). That
//! can only express 0, 1/2 and 1/3, and the blend a stage actually wants is
//! rarely one of those -- most of the gain here is at ratios like 0.9/0.1.
//! This searches the ratio instead.
//!
//! **The search is exact, not a scan.** Evaluation is a sum of table
//! look-ups, so it is linear in the weights: blending two tables at ratio
//! `t` blends their scores at the same `t`. Each position's absolute error
//! is therefore a V in `t`, their mean is convex piecewise-linear, and the
//! minimiser is the weighted median of the per-position break points. No
//! step size to choose and nothing to miss between samples. Linearity also
//! means the tables never have to be blended during the search: three score
//! vectors (own, previous, next) are computed once per stage and every ratio
//! after that is arithmetic on those.
//!
//! Both neighbours are used: the ratio against one is solved exactly, then
//! the result is blended with the other and solved again, a few rounds of
//! coordinate descent over the three-way mix. A stage that cannot be
//! improved keeps its own table byte for byte.
//!
//! Candidates are built from a snapshot taken before anything is written, so
//! a stage never smooths against a neighbour this run already changed.
//!
//! What this optimises is MAE on the given file, and the same file chooses
//! the ratio, so the number it reports is not held out from the choice. One
//! parameter fitted on thousands of positions moves that number very little,
//! but strength is still decided head to head.
//!
//! `--split` answers the question the reported gain cannot: it fits the
//! ratio on half the stage's positions and scores it on the other half, so
//! the number is held out from the choice that produced it. Nothing is
//! written in that mode -- it is a measurement, not a model.
//!
//! Usage: stage_smooth --val <file.data> --in <weights.bin> --out <path>
//!                     [--rounds N] [--min-gain X] [--split]
use kuroobi::evaluator::{Evaluator, STAGE_COUNT};
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::trainer::{load_examples_binary_into, Example};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type Snapshot = (Vec<Vec<f32>>, Vec<f32>);

/// Mix of stage tables: `coef[k]` is the share of stage `stages[k]`.
struct Mix {
    stages: Vec<usize>,
    coef: Vec<f64>,
}

/// Blend the snapshots named by a mix into one table.
fn blend(snap: &[Snapshot], mix: &Mix) -> Snapshot {
    let base = &snap[mix.stages[0]];
    let mut w = base.0.clone();
    for (pi, table) in w.iter_mut().enumerate() {
        for (ci, cell) in table.iter_mut().enumerate() {
            let mut acc = 0.0f64;
            for (k, &st) in mix.stages.iter().enumerate() {
                acc += mix.coef[k] * snap[st].0[pi][ci] as f64;
            }
            *cell = acc as f32;
        }
    }
    let mut num = base.1.clone();
    for (i, cell) in num.iter_mut().enumerate() {
        let mut acc = 0.0f64;
        for (k, &st) in mix.stages.iter().enumerate() {
            acc += mix.coef[k] * snap[st].1[i] as f64;
        }
        *cell = acc as f32;
    }
    (w, num)
}

/// Scores the evaluator gives each position, with whatever stage table is
/// currently loaded.
fn scores(ev: &Evaluator, rows: &[Example]) -> Vec<f64> {
    rows.iter().map(|ex| ev.eval(&ex.board()) as f64).collect()
}

fn mae_of(labels: &[f64], pred: &[f64]) -> f64 {
    if labels.is_empty() {
        return f64::INFINITY;
    }
    let acc: f64 = labels
        .iter()
        .zip(pred)
        .map(|(y, p)| (y - p).abs())
        .sum::<f64>();
    acc / labels.len() as f64
}

fn mse_of(labels: &[f64], pred: &[f64]) -> f64 {
    if labels.is_empty() {
        return f64::INFINITY;
    }
    let acc: f64 = labels
        .iter()
        .zip(pred)
        .map(|(y, p)| (y - p) * (y - p))
        .sum::<f64>();
    acc / labels.len() as f64
}

/// The `t` in `[0, 1]` minimising `mean |y - (t*a + (1-t)*b)|`.
///
/// Writing `c = y - b` and `d = a - b`, the objective is `mean |c - t*d|`: a
/// sum of V shapes, each with its corner at `t = c/d`. A convex piecewise-
/// linear sum is minimised where the slope changes sign, which is the corner
/// at which the cumulative weight `|d|` first reaches half the total -- the
/// weighted median. Terms with `d == 0` do not depend on `t` and drop out.
/// Convexity also means clamping the unconstrained minimiser into `[0, 1]`
/// gives the constrained one.
fn best_ratio(labels: &[f64], a: &[f64], b: &[f64]) -> f64 {
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(labels.len());
    let mut total = 0.0f64;
    for i in 0..labels.len() {
        let d = a[i] - b[i];
        if d == 0.0 {
            continue;
        }
        let c = labels[i] - b[i];
        pts.push((c / d, d.abs()));
        total += d.abs();
    }
    if pts.is_empty() {
        return 1.0; // Nothing separates the two tables here; keep the stage's own.
    }
    pts.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let half = total / 2.0;
    let mut run = 0.0f64;
    let mut t = pts[pts.len() - 1].0;
    for (p, w) in &pts {
        run += w;
        if run >= half {
            t = *p;
            break;
        }
    }
    t.clamp(0.0, 1.0)
}

/// Where a stage's blend should come from: `[previous, own, next]` shares
/// summing to one.
///
/// Coordinate descent over the three-way mix, done in score space: `cur` is
/// the candidate's score on every position, and each step solves one ratio
/// exactly, so the whole search never touches a weight table.
fn fit_share(
    labels: &[f64],
    own: &[f64],
    prev: Option<&Vec<f64>>,
    next: Option<&Vec<f64>>,
    rounds: usize,
) -> [f64; 3] {
    let mut cur = own.to_vec();
    let mut share = [0.0f64, 1.0, 0.0];
    for _ in 0..rounds.max(1) {
        let before = mae_of(labels, &cur);
        for (slot, nb) in [(0usize, prev), (2usize, next)] {
            let Some(nb) = nb else { continue };
            let t = best_ratio(labels, &cur, nb);
            if t >= 1.0 {
                continue;
            }
            for (c, n) in cur.iter_mut().zip(nb) {
                *c = t * *c + (1.0 - t) * *n;
            }
            for s in share.iter_mut() {
                *s *= t;
            }
            share[slot] += 1.0 - t;
        }
        if before - mae_of(labels, &cur) < 1e-9 {
            break;
        }
    }
    share
}

/// The blended score of every position, given where the blend comes from.
fn apply_share(
    own: &[f64],
    prev: Option<&Vec<f64>>,
    next: Option<&Vec<f64>>,
    share: [f64; 3],
) -> Vec<f64> {
    (0..own.len())
        .map(|i| {
            share[1] * own[i]
                + prev.map_or(0.0, |v| share[0] * v[i])
                + next.map_or(0.0, |v| share[2] * v[i])
        })
        .collect()
}

fn main() -> ExitCode {
    let mut val_path: Option<PathBuf> = None;
    let mut in_path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut rounds = 3usize;
    let mut min_gain = 0.0f64;
    let mut split = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--val" => val_path = it.next().map(PathBuf::from),
            "--in" => in_path = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            "--rounds" => match it.next().map(|v| v.parse()) {
                Some(Ok(n)) => rounds = n,
                _ => {
                    eprintln!("--rounds needs a number");
                    return ExitCode::FAILURE;
                }
            },
            "--min-gain" => match it.next().map(|v| v.parse()) {
                Some(Ok(v)) => min_gain = v,
                _ => {
                    eprintln!("--min-gain needs a number");
                    return ExitCode::FAILURE;
                }
            },
            "--split" => split = true,
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let (Some(val_path), Some(in_path), Some(out)) = (val_path, in_path, out) else {
        eprintln!(
            "usage: stage_smooth --val <file.data> --in <weights.bin> --out <path> \
             [--rounds N] [--min-gain X]"
        );
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
    println!("| stage | 空き | n | 現状MSE | 現状MAE | 後MSE | 後MAE | 改善 | 自分 | 前 | 後 |");
    println!("|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for st in 0..STAGE_COUNT {
        let rows = &by_stage[st];
        if rows.is_empty() {
            continue;
        }
        let labels: Vec<f64> = rows.iter().map(|ex| ex.score as f64).collect();

        // Three full evaluations, then every ratio is arithmetic on these.
        ev.set_stage_weights(st, &snap[st].0, &snap[st].1);
        let own = scores(&ev, rows);
        let prev = st.checked_sub(1).map(|l| {
            ev.set_stage_weights(st, &snap[l].0, &snap[l].1);
            scores(&ev, rows)
        });
        let next = if st + 1 < STAGE_COUNT {
            ev.set_stage_weights(st, &snap[st + 1].0, &snap[st + 1].1);
            Some(scores(&ev, rows))
        } else {
            None
        };
        ev.set_stage_weights(st, &snap[st].0, &snap[st].1);

        // In split mode the ratio is fitted on the even-indexed positions and
        // scored on the odd ones, so the reported gain is held out from the
        // choice. `val61` was sampled by an even stride through each stage's
        // pool, so alternating rows splits games, not phases.
        let pick = |v: &Vec<f64>, even: bool| -> Vec<f64> {
            v.iter()
                .enumerate()
                .filter(|(i, _)| (i % 2 == 0) == even)
                .map(|(_, x)| *x)
                .collect()
        };
        let (share, labels, own, prev, next) = if split {
            let share = fit_share(
                &pick(&labels, true),
                &pick(&own, true),
                prev.as_ref().map(|v| pick(v, true)).as_ref(),
                next.as_ref().map(|v| pick(v, true)).as_ref(),
                rounds,
            );
            (
                share,
                pick(&labels, false),
                pick(&own, false),
                prev.as_ref().map(|v| pick(v, false)),
                next.as_ref().map(|v| pick(v, false)),
            )
        } else {
            let share = fit_share(&labels, &own, prev.as_ref(), next.as_ref(), rounds);
            (share, labels, own, prev, next)
        };

        let own_mae = mae_of(&labels, &own);
        let own_mse = mse_of(&labels, &own);
        let cur = apply_share(&own, prev.as_ref(), next.as_ref(), share);
        let new_mae = mae_of(&labels, &cur);
        let new_mse = mse_of(&labels, &cur);
        let gain = own_mae - new_mae;
        let keep = gain > min_gain;
        if keep && !split {
            let mut stages = vec![st];
            let mut coef = vec![share[1]];
            if let Some(l) = st.checked_sub(1) {
                stages.push(l);
                coef.push(share[0]);
            }
            if st + 1 < STAGE_COUNT {
                stages.push(st + 1);
                coef.push(share[2]);
            }
            let mixed = blend(&snap, &Mix { stages, coef });
            ev.set_stage_weights(st, &mixed.0, &mixed.1);
        }
        if keep {
            changed += 1;
            total_gain += gain;
        }
        println!(
            "| {} | {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:+.3} | {:.3} | {:.3} | {:.3} |",
            st,
            60 - st,
            labels.len(),
            own_mse,
            own_mae,
            if keep { new_mse } else { own_mse },
            if keep { new_mae } else { own_mae },
            if keep { -gain } else { 0.0 },
            if keep { share[1] } else { 1.0 },
            if keep { share[0] } else { 0.0 },
            if keep { share[2] } else { 0.0 },
        );
    }
    println!("\n{changed} ステージを差し替え、MAE 合計 {total_gain:.3} 改善");
    if split {
        println!("--split: 比率は偶数番で決め、奇数番で測った。書き出しはしない");
        return ExitCode::SUCCESS;
    }
    if let Err(e) = ev.save_weights(&out) {
        eprintln!("failed to save {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    println!("saved to {}", out.display());
    ExitCode::SUCCESS
}
