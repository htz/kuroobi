//! ProbCut calibration: measure how well shallow searches predict deep
//! searches on real positions. For each sampled position, run independent
//! searches (transposition table cleared in between) at several depths and
//! emit one CSV row per position with all depth values. The error model
//! sigma(empties, depth, shallow_depth) is fitted offline from this data.
//!
//! Usage: mpccalib [--patterns <set>] [--stride N] [--max N] <weights.bin> <data-file>...

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::evaluator::Evaluator;
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::search::Searcher;
use kuroobi::trainer::load_examples_binary;

const DEPTHS: [u8; 6] = [0, 2, 4, 6, 8, 10];

fn main() -> ExitCode {
    let mut patterns = EGAROUCID_PATTERNS;
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut stride = 997usize; // prime stride decorrelates from file ordering
    let mut max_positions = 2000usize;

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
            "--stride" => stride = it.next().and_then(|v| v.parse().ok()).unwrap_or(stride),
            "--max" => {
                max_positions = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(max_positions)
            }
            other => paths.push(PathBuf::from(other)),
        }
    }
    if paths.len() < 2 {
        eprintln!("usage: mpccalib [--patterns <set>] [--stride N] [--max N] <weights.bin> <data-file>...");
        return ExitCode::FAILURE;
    }
    let weights_path = paths.remove(0);

    let mut evaluator = Evaluator::new(patterns);
    if let Err(err) = evaluator.load_weights(&weights_path) {
        eprintln!("failed to load {}: {err}", weights_path.display());
        return ExitCode::FAILURE;
    }

    let mut searcher = Searcher::new(18);
    let mut count = 0usize;

    print!("empties");
    for d in DEPTHS {
        print!(",d{d}");
    }
    println!();

    'outer: for path in &paths {
        let examples = match load_examples_binary(path) {
            Ok(ex) => ex,
            Err(err) => {
                eprintln!("failed to load {}: {err}", path.display());
                return ExitCode::FAILURE;
            }
        };
        for ex in examples.iter().step_by(stride) {
            let board = ex.board();
            let empties = 64 - (board.black | board.white).count_ones();
            if !(8..=45).contains(&empties) || board.movable() == 0 {
                continue;
            }
            let mut row = format!("{empties}");
            for d in DEPTHS {
                let v = if d == 0 {
                    evaluator.eval(&board)
                } else {
                    searcher.clear();
                    searcher.search(&board, &evaluator, d).value
                };
                row.push_str(&format!(",{v:.3}"));
            }
            println!("{row}");
            count += 1;
            if count >= max_positions {
                break 'outer;
            }
        }
    }
    eprintln!("{count} positions calibrated");
    ExitCode::SUCCESS
}
