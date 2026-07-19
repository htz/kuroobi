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
    let mut weights: Option<PathBuf> = None;
    let mut files: Vec<PathBuf> = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--depth" => depth = it.next().and_then(|v| v.parse().ok()),
            "--weights" => weights = it.next().map(PathBuf::from),
            other => files.push(PathBuf::from(other)),
        }
    }
    if files.is_empty() {
        eprintln!("usage: solve_obf [--depth <n>] [--weights <path>] <file.obf>...");
        return ExitCode::FAILURE;
    }

    let mut evaluator = Evaluator::new(EGAROUCID_PATTERNS);
    if depth.is_some() {
        let wpath = weights.unwrap_or_else(|| PathBuf::from("weights_full.bin"));
        if let Err(e) = evaluator.load_weights(&wpath) {
            eprintln!("failed to load {}: {e}", wpath.display());
            return ExitCode::FAILURE;
        }
    }

    let mut solver = Solver::new(22);
    let mut searcher = Searcher::new(21);

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
                    let r = solver.solve(EndSolverMode::Perfect, &board);
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
