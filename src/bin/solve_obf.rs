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
    let mut nnue_path: Option<PathBuf> = Some(PathBuf::from("weights/nnue-h16.bin"));
    let mut files: Vec<PathBuf> = Vec::new();

    let mut grand_time = 0.0f64;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--depth" => depth = it.next().and_then(|v| v.parse().ok()),
            "--mpc" => mpc = true,
            "--hash-bits" => {
                hash_bits = it.next().and_then(|v| v.parse().ok()).unwrap_or(hash_bits)
            }
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(threads),
            "--mpc-t" => mpc_t = it.next().and_then(|v| v.parse().ok()),
            "--weights" => weights = it.next().map(PathBuf::from),
            "--nnue" => nnue_path = it.next().map(PathBuf::from),
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
    let wpath = weights.unwrap_or_else(|| PathBuf::from("weights/linear.bin"));
    if let Err(e) = evaluator.load_weights(&wpath) {
        eprintln!("failed to load {}: {e}", wpath.display());
        return ExitCode::FAILURE;
    }

    let mut solver = Solver::new(hash_bits);
    solver.set_threads(threads);
    // The selective probes prefer the NNUE (see solver::sel_nnue_probe);
    // load it so the benchmark matches the match configuration.
    if let Some(p) = &nnue_path {
        let mut nn = kuroobi::nnue::Nnue::new(kuroobi::pattern::EGAROUCID_PATTERNS);
        if nn.load(p).is_ok() {
            nn.quantize();
            let nn: &'static kuroobi::nnue::Nnue = Box::leak(Box::new(nn));
            let mtt: &'static kuroobi::midgame::SharedTt =
                Box::leak(Box::new(kuroobi::midgame::SharedTt::new(22)));
            solver.set_nnue(nn, mtt);
        } else {
            eprintln!("note: nnue {} not found, linear probes", p.display());
        }
    }
    let mut searcher = Searcher::new(21);
    searcher.mpc = mpc;
    if let Some(t) = mpc_t {
        searcher.mpc_t = t;
    }

    // Sampling thread for --features layer-profile: reads the search's
    // current (phase, empties) marker at a fixed rate, off the search thread.
    let counts: std::sync::Arc<Vec<Vec<std::sync::atomic::AtomicU64>>> = std::sync::Arc::new(
        (0..kuroobi::solver::layer_profile::PHASES)
            .map(|_| {
                (0..64)
                    .map(|_| std::sync::atomic::AtomicU64::new(0))
                    .collect()
            })
            .collect(),
    );
    if kuroobi::solver::layer_profile::ENABLED {
        let counts = std::sync::Arc::clone(&counts);
        std::thread::spawn(move || loop {
            let (phase, empties) = kuroobi::solver::layer_profile::sample();
            counts[phase][empties].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_micros(20));
        });
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
        let mut solved = 0usize;

        for (i, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // OBF: "<64 chars> <X|O>; MOVE:+SCORE; ..."
            //
            // A line without `;` is still a position: `bench/band*.obf` holds
            // just "<64 chars> <X|O>" with no move list. Skipping those made
            // the whole file vanish and the run print "0 nodes" with exit code
            // 0 — a success that measured nothing. Take the line as-is and let
            // `Board::from_string` reject it if it really is garbage.
            let board_part = match line.find(';') {
                Some(semi) => &line[..semi],
                None => line,
            };
            let board = match Board::from_string(board_part) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("line {}: bad board: {e:?}", i + 1);
                    continue;
                }
            };

            let (value, nodes, secs) = match depth {
                None => {
                    // Emptying the table is what makes the positions
                    // independent; it is harness scaffolding, not search, and
                    // it runs strictly before the thread pool starts. Leaving
                    // it in understates the speedup (a fixed serial cost is
                    // 1.9% of a one-thread solve but 8.1% of a ten-thread one)
                    // and it is not what `edax -solve` or Egaroucid's `-solve`
                    // report either — both call their table init outside the
                    // timed region. Subtract it instead of timing around the
                    // solve, so the engine keeps one entry point.
                    let c0 = kuroobi::solver::CLEAR_NS.load(std::sync::atomic::Ordering::Relaxed);
                    let t = Instant::now();
                    let r =
                        solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(&evaluator));
                    let wall = t.elapsed().as_secs_f64();
                    let clear = (kuroobi::solver::CLEAR_NS
                        .load(std::sync::atomic::Ordering::Relaxed)
                        - c0) as f64
                        / 1e9;
                    (r.value as f32, r.nodes, wall - clear)
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
            solved += 1;
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
        // Nothing parsed means the file is in a format we cannot read. Saying
        // "0 nodes in 0.000s (NaNM nodes/s)" and exiting 0 reads as success.
        if solved == 0 {
            eprintln!(
                "{}: no readable problems (0 / {} lines)",
                file.display(),
                content.lines().filter(|l| !l.trim().is_empty()).count()
            );
            return ExitCode::FAILURE;
        }
        println!("---+---------+-------+----------+-------------+----------");
        println!(
            "{}: {} nodes in {:.3}s ({:.2}M nodes/s)",
            file.display(),
            total_nodes,
            total_time,
            total_nodes as f64 / total_time / 1e6
        );
        {
            use kuroobi::solver as s;
            use std::sync::atomic::Ordering::Relaxed;
            let phase = |ns: &std::sync::atomic::AtomicU64, n: &std::sync::atomic::AtomicU64| {
                let (secs, nodes) = (ns.load(Relaxed) as f64 / 1e9, n.load(Relaxed));
                format!(
                    "{secs:.3}s {nodes} nodes {:.1}M/s",
                    nodes as f64 / secs / 1e6
                )
            };
            println!(
                "  table clear {:.3}s (excluded from the times above)\n  warm-up {}\n  exact   {}",
                s::CLEAR_NS.load(Relaxed) as f64 / 1e9,
                phase(&s::WARMUP_NS, &s::WARMUP_NODES),
                phase(&s::EXACT_NS, &s::EXACT_NODES)
            );
            let sp = kuroobi::solver::SPLITS.load(Relaxed);
            if sp > 0 {
                let live = s::TASK_NS.load(Relaxed) as f64 / 1e9;
                let wait = s::WAIT_NS.load(Relaxed) as f64 / 1e9;
                // Thread-seconds available vs actually working. Time inside
                // tasks plus the main thread's wall is what we occupy;
                // subtracting the waits leaves the work.
                let cap = threads as f64 * total_time;
                let busy = live + total_time - wait;
                println!(
                    "  aborts fired {} / tasks discarded {}\n  splits {} handing off {} siblings (refused {})",
                    kuroobi::solver::ABORT_FIRED.load(std::sync::atomic::Ordering::Relaxed),
                    kuroobi::solver::TASK_ABORTED.load(std::sync::atomic::Ordering::Relaxed),
                    sp,
                    s::HANDED.load(Relaxed),
                    s::REFUSED.load(Relaxed)
                );
                println!(
                    "  task time {live:.2}s  split wait {wait:.2}s  \
                     -> occupancy {:.0}% busy {:.0}% of {cap:.1} thread-s",
                    (live + total_time) / cap * 100.0,
                    busy / cap * 100.0
                );
            }
        }
        grand_time += total_time;
        if kuroobi::solver::node_accounting::ENABLED {
            // Edax counts ordering and ETC probes as nodes; add them so the
            // totals are comparable with `edax -solve`.
            let (sorted, etc, look) = kuroobi::solver::node_accounting::totals();
            let ordering = sorted + etc + look;
            println!(
                "{}: + ordering {ordering} (sorted {sorted} / etc {etc} / lookahead {look}) \
                 = {} nodes on Edax's terms ({:.2}M nodes/s)",
                file.display(),
                total_nodes + ordering,
                (total_nodes + ordering) as f64 / total_time / 1e6
            );
        }
    }
    if kuroobi::solver::layer_profile::ENABLED {
        use kuroobi::solver::layer_profile::{NODES, PHASES, PHASE_NAMES};
        use std::sync::atomic::Ordering::Relaxed;
        let total: u64 = counts
            .iter()
            .flat_map(|p| p.iter())
            .map(|c| c.load(Relaxed))
            .sum();
        println!("\nsamples {total}  (share of wall-clock)");
        print!("empties |");
        for name in PHASE_NAMES {
            print!(" {name:>7} |");
        }
        println!("   total |      nodes |  ns/node");
        let mut sums = [0u64; PHASES];
        for e in 0..64usize {
            let row: Vec<u64> = (0..PHASES).map(|p| counts[p][e].load(Relaxed)).collect();
            let rt: u64 = row.iter().sum();
            if rt == 0 {
                continue;
            }
            for (sum, v) in sums.iter_mut().zip(row.iter()) {
                *sum += v;
            }
            let pct = |v: u64| 100.0 * v as f64 / total as f64;
            print!("{e:7} |");
            for v in row.iter() {
                print!(" {:6.2}% |", pct(*v));
            }
            let nodes = NODES[e].load(Relaxed);
            let ns = if nodes > 0 {
                rt as f64 / total as f64 * grand_time * 1e9 / nodes as f64
            } else {
                0.0
            };
            println!(" {:6.2}% | {nodes:>10} | {ns:8.1}", pct(rt));
        }
        println!(
            "  total | {}",
            (0..PHASES)
                .map(|p| format!(
                    "{} {:.2}%",
                    PHASE_NAMES[p],
                    100.0 * sums[p] as f64 / total as f64
                ))
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    ExitCode::SUCCESS
}
