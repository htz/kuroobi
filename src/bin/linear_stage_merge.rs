//! Assemble one linear model out of several, taking each stage from whichever
//! input scores best on a held-out set.
//!
//! The stages are independent tables -- a position only ever reads and only
//! ever updates `weights[stage]` -- so picking stage 20 from one file and
//! stage 40 from another produces a model that is exactly as good as the
//! better input at every stage. A training run that improved half the board
//! and lost the other half is therefore not a wash: the half it improved is
//! keepable on its own.
//!
//! `--select-by` picks which number decides. The default is MAE; `spread`
//! is the error with the stage's constant offset removed, which is what
//! move ordering actually sees -- the moves compared at a node are all one
//! ply deeper, hence all in the same stage, so a per-stage offset cancels
//! in the argmax. Measured across five evaluators, ranking by spread
//! reproduced their head-to-head order exactly while ranking by MAE put
//! the strongest of them fourth. Use it when the inputs come from
//! different model families; between linear models the offsets sit inside
//! ±0.3 and the two agree.
//!
//! Scored on MAE, not MSE. Squared error is dominated by the thinly-sampled
//! opening stages, where it runs 70-90 against 8-40 in the endgame, so an
//! outlier there outweighs a real loss elsewhere.
//!
//! Usage: linear_stage_merge --val <file.data> --out <path>
//!                    [--select-by mae|mse|spread] <weights.bin>...
use kuroobi::linear::{Linear, STAGE_COUNT};
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::record::{Filter, TeacherPolicy};
use kuroobi::trainer::load_examples_filtered_into;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Per-stage error split into its constant part and the rest.
///
/// `spread` is `sqrt(mse - bias^2)`: the standard deviation of the error,
/// which is what survives once the stage's constant offset is taken out.
fn stats_of(a: &[f64; 4]) -> (f64, f64, f64, f64) {
    let n = a[0];
    let (mse, mae, bias) = (a[2] / n, a[1] / n, a[3] / n);
    (mse, mae, bias, (mse - bias * bias).max(0.0).sqrt())
}

fn score_of(which: &str, a: &[f64; 4]) -> f64 {
    let (mse, mae, _, sd) = stats_of(a);
    match which {
        "mse" => mse,
        "spread" => sd,
        _ => mae,
    }
}

fn main() -> ExitCode {
    let mut val_paths: Vec<PathBuf> = Vec::new();
    let mut search_value_to_ply: Option<u8> = None;
    let mut drop_random = false;
    let mut keep_above_ply: Option<u8> = None;
    let mut min_ply = 0u8;
    let mut out: Option<PathBuf> = None;
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut select_by = String::from("mae");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--val" => val_paths.extend(it.next().map(PathBuf::from)),
            /* The same teacher the inputs were fitted to. Without these
            every record reads as the game's result, and stages trained
            against a depth-4 search value would be picked on a target
            nothing was aiming at. */
            "--search-value-to-ply" => search_value_to_ply = it.next().and_then(|v| v.parse().ok()),
            "--drop-random" => drop_random = true,
            "--keep-above-ply" => keep_above_ply = it.next().and_then(|v| v.parse().ok()),
            "--min-ply" => min_ply = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--out" => out = it.next().map(PathBuf::from),
            "--select-by" => select_by = it.next().unwrap_or_default(),
            other if other.starts_with("--") => {
                eprintln!("unknown flag {other}");
                return ExitCode::FAILURE;
            }
            file => inputs.push(PathBuf::from(file)),
        }
    }
    let Some(out) = out else {
        eprintln!(
            "usage: linear_stage_merge --val <file.data> [--val ...] --out <path>\n\
             [--select-by mae|mse|spread] [--search-value-to-ply <n>]\n\
             [--drop-random] [--keep-above-ply <n>] [--min-ply <n>]\n\
             <weights.bin>..."
        );
        return ExitCode::FAILURE;
    };
    if val_paths.is_empty() {
        eprintln!("no --val given");
        return ExitCode::FAILURE;
    }
    if inputs.is_empty() {
        eprintln!("no input weights given");
        return ExitCode::FAILURE;
    }

    let filter = Filter {
        min_ply,
        max_score_diff: None,
        drop_random,
        keep_above_ply,
    };
    let policy = TeacherPolicy {
        search_value_to_ply,
    };
    let mut val = Vec::new();
    for path in &val_paths {
        if let Err(e) = load_examples_filtered_into(path, &mut val, None, &filter, &policy) {
            eprintln!("failed to read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    println!(
        "val: {} positions from {} file(s), teacher: {}",
        val.len(),
        val_paths.len(),
        policy.describe()
    );

    // [count, sum_abs, sum_sq, sum_err] per stage, per input.
    let mut scores: Vec<Vec<[f64; 4]>> = Vec::with_capacity(inputs.len());
    let mut evs: Vec<Linear> = Vec::with_capacity(inputs.len());
    for p in &inputs {
        let mut ev = Linear::new(EGAROUCID_PATTERNS);
        if let Err(e) = ev.load_weights(Path::new(p)) {
            eprintln!("failed to load {}: {e}", p.display());
            return ExitCode::FAILURE;
        }
        let mut acc = vec![[0.0f64; 4]; STAGE_COUNT];
        for ex in &val {
            let board = ex.board();
            // Prediction minus truth, so a positive mean reads as "this
            // model scores positions high".
            let e = ev.eval(&board) as f64 - ex.score as f64;
            let a = &mut acc[Linear::stage(&board)];
            a[0] += 1.0;
            a[1] += e.abs();
            a[2] += e * e;
            a[3] += e;
        }
        scores.push(acc);
        evs.push(ev);
    }

    // Build into a copy of the first input so untouched stages keep something
    // valid rather than whatever an empty evaluator would hold.
    let mut merged = Linear::new(EGAROUCID_PATTERNS);
    if let Err(e) = merged.load_weights(Path::new(&inputs[0])) {
        eprintln!("failed to load {}: {e}", inputs[0].display());
        return ExitCode::FAILURE;
    }

    let mut taken = vec![0usize; inputs.len()];
    println!(
        "  stage  empties       n      MSE     MAE     bias   spread   from  (selecting on {select_by})"
    );
    for (st, first) in scores[0].iter().enumerate().take(STAGE_COUNT) {
        let n = first[0];
        if n == 0.0 {
            continue;
        }
        let (best, _) = (0..inputs.len())
            .map(|i| (i, score_of(&select_by, &scores[i][st])))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        let (mse, mae, bias, sd) = stats_of(&scores[best][st]);
        let (w, num) = evs[best].stage_weights(st);
        merged.set_stage_weights(st, &w, &num);
        taken[best] += 1;
        println!(
            "  {:>5}  {:>7}  {:>6}  {:>7.3}  {:>6.3}  {:>+7.3}  {:>7.3}   {}",
            st,
            60 - st,
            n as u64,
            mse,
            mae,
            bias,
            sd,
            inputs[best].display()
        );
    }
    for (i, p) in inputs.iter().enumerate() {
        println!("{:>3} stages from {}", taken[i], p.display());
    }
    if let Err(e) = merged.save_weights(&out) {
        eprintln!("failed to save {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    println!("merged model saved to {}", out.display());
    ExitCode::SUCCESS
}
