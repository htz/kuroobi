//! Measures the selective-search sigma under NNUE.
//!
//! `mpc_sigma` was measured with the linear evaluator and reused for
//! NNUE on a "safer anyway" argument — the endgame sigma did the same
//! and turned out 2x too large, so the midgame gets measured too.
//! An oversized sigma prunes less and loses depth per unit time.
//!
//! Each position is searched at several depths with cleared tables,
//! emitted as CSV; sigma fitting happens downstream. Positions run in
//! parallel, searches stay sequential (Lazy SMP changes the tree; we
//! measure values, not speed).
//!
//! Usage:
//!   mpccalib_nnue [--threads N] [--stride N] [--max N] [--depths a,b,c]
//!                 <nnue.bin> <linear.bin> <data-file>...

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use kuroobi::evaluator::Evaluator;
use kuroobi::midgame::{NnueSearch, SharedTt};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::trainer::load_examples_binary;

fn main() -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut stride = 997usize; // prime stride decorrelates from file order
    let mut max_positions = 2000usize;
    let mut threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let mut depths: Vec<u32> = vec![0, 2, 4, 6, 8, 10];

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(threads),
            "--stride" => stride = it.next().and_then(|v| v.parse().ok()).unwrap_or(stride),
            "--max" => {
                max_positions = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(max_positions)
            }
            "--depths" => {
                if let Some(v) = it.next() {
                    depths = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                }
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if paths.len() < 3 {
        eprintln!(
            "usage: mpccalib_nnue [--threads N] [--stride N] [--max N] [--depths a,b,c] \
             <nnue.bin> <linear.bin> <data-file>..."
        );
        return ExitCode::FAILURE;
    }
    let nnue_path = paths.remove(0);
    let linear_path = paths.remove(0);

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    if let Err(err) = nn.load(&nnue_path) {
        eprintln!("failed to load {}: {err}", nnue_path.display());
        return ExitCode::FAILURE;
    }
    // Skipping this makes the SIMD path read uninitialized memory.
    nn.quantize();
    let nn: &'static Nnue = Box::leak(Box::new(nn));

    // Depth-0 values come from the linear evaluator (same disc units).
    let mut evaluator = Evaluator::new(EGAROUCID_PATTERNS);
    if let Err(err) = evaluator.load_weights(&linear_path) {
        eprintln!("failed to load {}: {err}", linear_path.display());
        return ExitCode::FAILURE;
    }

    // Collect positions first for parallel dispatch.
    let mut boards = Vec::new();
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
            boards.push(board);
            if boards.len() >= max_positions {
                break 'outer;
            }
        }
    }
    eprintln!("measuring {} positions on {threads} threads", boards.len());

    print!("empties");
    for d in &depths {
        print!(",d{d}");
    }
    println!();

    let next = AtomicUsize::new(0);
    let rows: Vec<Vec<String>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (next, boards, depths, evaluator) = (&next, &boards, &depths, &evaluator);
                s.spawn(move || {
                    /* Per-thread tables: sharing lets one position's
                    results help another and breaks independence. 18 bits
                    is ~4 MB per thread. */
                    let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(18)));
                    let mut search = NnueSearch::new(nn, tt);
                    search.threads = 1; // sequential search keeps the tree fixed
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(board) = boards.get(i) else { break };
                        let empties = 64 - (board.black | board.white).count_ones();
                        let mut row = format!("{empties}");
                        for &d in depths.iter() {
                            let v = if d == 0 {
                                evaluator.eval(board)
                            } else {
                                tt.clear();
                                let (_, v, _) = search.best_move_deadline(board, d, None);
                                v
                            };
                            row.push_str(&format!(",{v:.3}"));
                        }
                        out.push(row);
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let mut n = 0usize;
    for chunk in rows {
        for r in chunk {
            println!("{r}");
            n += 1;
        }
    }
    eprintln!("{n} positions calibrated");
    ExitCode::SUCCESS
}
