//! Per-empties validation MSE for the NNUE evaluator, and a text exporter so
//! external evaluators can be scored on the same
//! positions. Companion to `valmse` (linear, per-stage): this one buckets by
//! empty count, which is the axis the depth-ladder experiment varies along.
//!
//! Usage:
//!   phase_mse <nnue.bin> <data-file>...              per-empties MSE (CSV)
//!   phase_mse --dump-text <out.txt> <data-file>...   "board score" lines

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::trainer::{load_examples_binary, load_examples_text};

fn main() -> ExitCode {
    let mut dump: Option<PathBuf> = None;
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dump-text" => dump = it.next().map(PathBuf::from),
            other => paths.push(PathBuf::from(other)),
        }
    }

    let nnue = if dump.is_none() {
        if paths.is_empty() {
            eprintln!("usage: phase_mse <nnue.bin> <data>... | --dump-text <out> <data>...");
            return ExitCode::FAILURE;
        }
        let p = paths.remove(0);
        let mut nn = Nnue::new(EGAROUCID_PATTERNS);
        if let Err(e) = nn.load(&p) {
            eprintln!("failed to load {}: {e}", p.display());
            return ExitCode::FAILURE;
        }
        nn.quantize();
        Some(nn)
    } else {
        None
    };

    let mut out = dump
        .as_ref()
        .map(|p| std::io::BufWriter::new(std::fs::File::create(p).expect("create dump file")));

    // 4-empties buckets: 0-3, 4-7, ..., 60-63.
    let mut sq = [0.0f64; 16];
    let mut n = [0u64; 16];
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
            let empties = board.empty_count() as usize;
            if let Some(w) = out.as_mut() {
                // Rank-major X/O text, same convention as load_examples_text.
                let mut s = String::with_capacity(64);
                for rank in 0..8 {
                    for file in 0..8 {
                        let bit = 1u64 << (file * 8 + rank);
                        s.push(if ex.black & bit != 0 {
                            'X'
                        } else if ex.white & bit != 0 {
                            'O'
                        } else {
                            '-'
                        });
                    }
                }
                writeln!(w, "{s} {}", ex.score).expect("write dump");
            } else if let Some(nn) = nnue.as_ref() {
                let err = ex.score as f64 - nn.eval(&board) as f64;
                sq[empties / 4] += err * err;
                n[empties / 4] += 1;
            }
        }
    }

    if nnue.is_some() {
        println!("empties,mse,samples");
        for b in 0..16 {
            if n[b] > 0 {
                println!(
                    "{}-{},{:.3},{}",
                    b * 4,
                    b * 4 + 3,
                    sq[b] / n[b] as f64,
                    n[b]
                );
            }
        }
    }
    ExitCode::SUCCESS
}
