//! Weight-file statistics: per-pattern contribution by stage band.
//!
//! For each stage band and pattern, prints the fraction of nonzero cells
//! (SGD/Adam only ever touch visited cells, so nonzero = visited) and the
//! RMS over those cells scaled by the pattern's orientation count — a
//! rough per-position contribution scale in evaluator points.
//!
//! Usage: wstats [--patterns egaroucid|edax] <weights.bin>

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::evaluator::{Evaluator, STAGE_COUNT};
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};

fn main() -> ExitCode {
    let mut patterns = EGAROUCID_PATTERNS;
    let mut path: Option<PathBuf> = None;

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
            other => path = Some(PathBuf::from(other)),
        }
    }
    let Some(path) = path else {
        eprintln!("usage: wstats [--patterns egaroucid|edax|egaroucid-plus] <weights.bin>");
        return ExitCode::FAILURE;
    };

    let mut e = Evaluator::new(patterns);
    if let Err(err) = e.load_weights(&path) {
        eprintln!("failed to load {}: {err}", path.display());
        return ExitCode::FAILURE;
    }

    // Stage bands: opening / early-mid / late-mid / endgame
    let bands: &[(usize, usize, &str)] = &[
        (0, 14, "open  0-14"),
        (15, 29, "mid  15-29"),
        (30, 44, "late 30-44"),
        (45, 60, "end  45-60"),
    ];

    println!("{}: per-pattern RMS(nonzero) x orientations, by stage band", path.display());
    println!("{:<18} {:>4} {}", "pattern", "ori",
        bands.iter().map(|b| format!("{:>16}", b.2)).collect::<String>());

    for (pi, p) in patterns.iter().enumerate() {
        let mut cols = String::new();
        for &(lo, hi, _) in bands {
            let mut sum_sq = 0.0f64;
            let mut nonzero = 0u64;
            let mut total = 0u64;
            for stage in lo..=hi.min(STAGE_COUNT - 1) {
                for idx in 0..p.table_size() {
                    let w = e.weight(stage, pi, idx) as f64;
                    total += 1;
                    if w != 0.0 {
                        nonzero += 1;
                        sum_sq += w * w;
                    }
                }
            }
            let rms = if nonzero > 0 { (sum_sq / nonzero as f64).sqrt() } else { 0.0 };
            let contrib = rms * p.masks.len() as f64;
            let vis = 100.0 * nonzero as f64 / total.max(1) as f64;
            cols.push_str(&format!("  {contrib:>7.2} ({vis:>3.0}%)"));
        }
        println!("{:<18} {:>4}{}", p.name, p.masks.len(), cols);
    }

    // Disc-count feature (v2 files; zero for v1)
    let mut cols = String::new();
    for &(lo, hi, _) in bands {
        let mut sum_sq = 0.0f64;
        let mut nonzero = 0u64;
        for stage in lo..=hi.min(STAGE_COUNT - 1) {
            for count in 0..=64 {
                let w = e.num_weight(stage, count) as f64;
                if w != 0.0 {
                    nonzero += 1;
                    sum_sq += w * w;
                }
            }
        }
        let rms = if nonzero > 0 { (sum_sq / nonzero as f64).sqrt() } else { 0.0 };
        cols.push_str(&format!("  {rms:>7.2} (---%)"));
    }
    println!("{:<18} {:>4}{}", "disc-count", 1, cols);

    ExitCode::SUCCESS
}
