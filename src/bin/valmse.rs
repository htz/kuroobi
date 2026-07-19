//! Validation MSE: evaluate a weight file against labeled positions
//! without touching the weights. Companion to `train` for early-stopping
//! decisions on a held-out set.
//!
//! Usage: valmse [--patterns <set>] <weights.bin> <data-file>...

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::evaluator::Evaluator;
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::trainer::{load_examples_binary, load_examples_text};

fn main() -> ExitCode {
    let mut patterns = EGAROUCID_PATTERNS;
    let mut paths: Vec<PathBuf> = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--patterns" => match it.next().as_deref() {
                Some("egaroucid") => patterns = EGAROUCID_PATTERNS,
                Some("egaroucid-plus") => patterns = EGAROUCID_PLUS_PATTERNS,
                Some("edax") => patterns = EDAX_PATTERNS,
                other => {
                    eprintln!("unknown pattern set: {other:?}");
                    return ExitCode::FAILURE;
                }
            },
            other => paths.push(PathBuf::from(other)),
        }
    }
    if paths.len() < 2 {
        eprintln!("usage: valmse [--patterns <set>] <weights.bin> <data-file>...");
        return ExitCode::FAILURE;
    }
    let weights_path = paths.remove(0);

    let mut e = Evaluator::new(patterns);
    if let Err(err) = e.load_weights(&weights_path) {
        eprintln!("failed to load {}: {err}", weights_path.display());
        return ExitCode::FAILURE;
    }

    use kuroobi::evaluator::STAGE_COUNT;
    let mut stage_sq = [0.0f64; STAGE_COUNT];
    let mut stage_n = [0u64; STAGE_COUNT];
    let mut sum_sq = 0.0f64;
    let mut n = 0u64;
    for path in &paths {
        let examples = if path.extension().is_some_and(|x| x == "txt") {
            load_examples_text(path)
        } else {
            load_examples_binary(path)
        };
        let examples = match examples {
            Ok(ex) => ex,
            Err(err) => {
                eprintln!("failed to load {}: {err}", path.display());
                return ExitCode::FAILURE;
            }
        };
        for ex in &examples {
            let board = ex.board();
            let err = ex.score as f64 - e.eval(&board) as f64;
            sum_sq += err * err;
            n += 1;
            let stage = Evaluator::stage(&board);
            stage_sq[stage] += err * err;
            stage_n[stage] += 1;
        }
    }

    println!("stage,mse,samples");
    for stage in 0..STAGE_COUNT {
        if stage_n[stage] > 0 {
            println!(
                "{},{:.3},{}",
                stage,
                stage_sq[stage] / stage_n[stage] as f64,
                stage_n[stage]
            );
        }
    }

    println!(
        "{}: val mse {:.4} over {} positions",
        weights_path.display(),
        sum_sq / n.max(1) as f64,
        n
    );
    ExitCode::SUCCESS
}
