//! Solve OBF problem files (FFO test format) with our solver/searcher and
//! report per-position time, nodes and NPS — directly comparable to
//! `edax -solve <file>` output.
//!
//! Endgame mode (default): Perfect-solve every position.
//! Midgame mode (--depth n): fixed-depth midgame search instead (for
//! search-speed comparison on identical positions).
//!
//! Usage: solve_obf [--depth <n>] [--weights <path>] <file.obf>...

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use kuroobi::evaluator::Evaluator;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::search::Searcher;
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::Board;

fn main() -> ExitCode {
    let mut depth: Option<u8> = None;
    let mut mpc = false;
    // Billion-node endgames oversubscribe the table by orders of magnitude;
    // measured -14..-16% nodes and -23..-31% time going from 2^22 to 2^26.
    let mut hash_bits: u32 = 26;
    let mut threads: usize = 1;
    let mut mpc_t: Option<f32> = None;
    let mut weights: Option<PathBuf> = None;
    let mut files: Vec<PathBuf> = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--depth" => depth = it.next().and_then(|v| v.parse().ok()),
            "--mpc" => mpc = true,
            "--hash-bits" => hash_bits = it.next().and_then(|v| v.parse().ok()).unwrap_or(hash_bits),
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(threads),
            "--mpc-t" => mpc_t = it.next().and_then(|v| v.parse().ok()),
            "--weights" => weights = it.next().map(PathBuf::from),
            other => files.push(PathBuf::from(other)),
        }
    }
    if files.is_empty() {
        eprintln!("usage: solve_obf [--depth <n>] [--mpc] [--hash-bits <n>] [--threads <n>] [--weights <path>] <file.obf>...");
        return ExitCode::FAILURE;
    }

    // Weights serve the midgame search directly and the endgame solver's
    // eval-based move ordering.
    let mut evaluator = Evaluator::new(EGAROUCID_PATTERNS);
    let wpath = weights.unwrap_or_else(|| PathBuf::from("weights_full.bin"));
    if let Err(e) = evaluator.load_weights(&wpath) {
        eprintln!("failed to load {}: {e}", wpath.display());
        return ExitCode::FAILURE;
    }

    let mut solver = Solver::new(hash_bits);
    solver.set_threads(threads);
    let mut searcher = Searcher::new(21);
    searcher.mpc = mpc;
    if let Some(t) = mpc_t {
        searcher.mpc_t = t;
    }

    for file in &files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("failed to read {}: {e}", file.display());
                return ExitCode::FAILURE;
            }
        };

        println!(" # | empties | value |     time |       nodes |      N/s");
        println!("---+---------+-------+----------+-------------+----------");
        let mut total_nodes = 0u64;
        let mut total_time = 0.0f64;

        for (i, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // OBF: "<64 chars> <X|O>; MOVE:+SCORE; ..."
            let Some(semi) = line.find(';') else { continue };
            let board_part = &line[..semi];
            let board = match Board::from_string(board_part) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("line {}: bad board: {e:?}", i + 1);
                    continue;
                }
            };

            let (value, nodes, secs) = match depth {
                None => {
                    let t = Instant::now();
                    let r = solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(&evaluator));
                    (r.value as f32, r.nodes, t.elapsed().as_secs_f64())
                }
                Some(d) => {
                    searcher.clear();
                    let t = Instant::now();
                    let r = searcher.search(&board, &evaluator, d);
                    (r.value, r.nodes, t.elapsed().as_secs_f64())
                }
            };

            total_nodes += nodes;
            total_time += secs;
            println!(
                "{:>2} | {:>7} | {:>5.0} | {:>7.3}s | {:>11} | {:>8.2}M",
                i + 1,
                board.empty_count(),
                value,
                secs,
                nodes,
                nodes as f64 / secs / 1e6
            );
        }
        println!("---+---------+-------+----------+-------------+----------");
        println!(
            "{}: {} nodes in {:.3}s ({:.2}M nodes/s)",
            file.display(),
            total_nodes,
            total_time,
            total_nodes as f64 / total_time / 1e6
        );
    }
    ExitCode::SUCCESS
}
