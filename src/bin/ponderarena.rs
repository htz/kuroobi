//! Measures the effect of pondering.
//!
//! A and B play with equal per-move time; only A ponders during the
//! opponent's turn. Comparing the two within the same games gives less
//! variance than separate runs.
//!
//! Only the single-predicted-move scheme exists; spreading over all
//! legal replies was measured and rejected (`notes/pondering.md`).
//!
//! Two outputs:
//! * reached depth — the direct signal that the mechanism works
//! * win rate — the only valid strength verdict, but needs many games,
//!   so check depth first
//!
//! Fixed depth shows no difference (the search runs to the end anyway);
//! always measure with `--ms`. Tables are cleared per game.
//!
//! Usage:
//!   ponderarena [OPTIONS]
//!
//! Options:
//!   --games <n>          number of games (default 10)
//!   --ms <n>             time per move in ms (default 200)
//!   --ponder <on|off>    whether A ponders (default on; off = control)
//!   --fixed-depth        fixed-depth mode (measure time, not depth)
//!   --ponder-ms <n>      ponder time in fixed-depth mode (default 300)
//!   --depth <n>          midgame depth cap (default 20)
//!   --solve-empties <n>  solve entry (default 14)
//!   --threads <n>        threads (default 1)
//!   --random-plies <n>   random opening plies (default 8)
//!   --seed <n>           RNG seed (default 7)
//!   --nnue <path>        NNUE weights (default weights/nnue-h16.bin)
//!   --weights <path>     linear weights (default weights/linear.bin)

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use kuroobi::board::Board;
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::position::Position;

struct Args {
    games: usize,
    ms: u64,
    ponder: bool,
    /// Disable ProbCut: it prunes probabilistically, so table contents
    /// can change results; used to verify fixed-depth moves are stable.
    no_mpc: bool,
    /// Fixed-depth mode: the benefit becomes speed, so measure per-move
    /// time instead of reached depth.
    fixed: bool,
    /// Ponder time in fixed-depth mode (a stand-in for opponent think time).
    ponder_ms: u64,
    depth: u32,
    solve_empties: u8,
    threads: usize,
    random_plies: usize,
    seed: u64,
    nnue: PathBuf,
    weights: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            games: 10,
            ms: 200,
            ponder: true,
            no_mpc: false,
            fixed: false,
            ponder_ms: 300,
            depth: 20,
            solve_empties: 14,
            threads: 1,
            random_plies: 8,
            seed: 7,
            nnue: PathBuf::from("weights/nnue-h16.bin"),
            weights: PathBuf::from("weights/linear.bin"),
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{k} needs a value"));
        match k.as_str() {
            "--games" => a.games = val()?.parse().map_err(|_| "bad --games")?,
            "--ms" => a.ms = val()?.parse().map_err(|_| "bad --ms")?,
            "--ponder" => {
                a.ponder = match val()?.as_str() {
                    "on" => true,
                    "off" => false,
                    m => return Err(format!("--ponder must be on or off ({m})")),
                }
            }
            "--no-mpc" => a.no_mpc = true,
            "--fixed-depth" => a.fixed = true,
            "--ponder-ms" => a.ponder_ms = val()?.parse().map_err(|_| "bad --ponder-ms")?,
            "--depth" => a.depth = val()?.parse().map_err(|_| "bad --depth")?,
            "--solve-empties" => {
                a.solve_empties = val()?.parse().map_err(|_| "bad --solve-empties")?
            }
            "--threads" => a.threads = val()?.parse().map_err(|_| "bad --threads")?,
            "--random-plies" => {
                a.random_plies = val()?.parse().map_err(|_| "bad --random-plies")?
            }
            "--seed" => a.seed = val()?.parse().map_err(|_| "bad --seed")?,
            "--nnue" => a.nnue = PathBuf::from(val()?),
            "--weights" => a.weights = PathBuf::from(val()?),
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(a)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Reached-depth stats; solved moves (depth 0) are excluded — depth is
/// meaningless there and would only drag the mean down.
#[derive(Default)]
struct Depths {
    sum: u64,
    n: u64,
}
impl Depths {
    fn add(&mut self, d: u32) {
        if d > 0 {
            self.sum += d as u64;
            self.n += 1;
        }
    }
    fn avg(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum as f64 / self.n as f64
        }
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let cfg = || EngineConfig {
        depth: args.depth,
        solve_empties: args.solve_empties,
        threads: args.threads,
        nnue: args.nnue.clone(),
        weights: args.weights.clone(),
        // Book off: book moves bypass the search, hiding both the
        // ponder effect and the time usage.
        use_book: false,
        mpc: !args.no_mpc,
        ..EngineConfig::default()
    };

    let mut a = match Engine::new(cfg()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine A: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut b = match Engine::new(cfg()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine B: {e}");
            return ExitCode::FAILURE;
        }
    };

    let budget = Duration::from_millis(args.ms);
    let mut rng = Rng(args.seed | 1);
    let (mut da, mut db) = (Depths::default(), Depths::default());
    // Fixed-depth metric: milliseconds to the same depth.
    let (mut ta, mut tb) = (0u128, 0u128);
    let (mut na, mut nb) = (0u64, 0u64);
    let (mut wins, mut losses, mut draws) = (0u32, 0u32, 0u32);
    let mut ponder_nodes = 0u64;
    // How often/long pondering ran in the solve region.
    let (mut end_n, mut end_ms) = (0u64, 0u128);

    for g in 0..args.games {
        // Clear per game; a warm table skews even same-settings play.
        a.clear_tables();
        b.clear_tables();

        let mut board = Board::new();
        for _ in 0..args.random_plies {
            if board.is_game_over() {
                break;
            }
            let ms: Vec<Position> = board.movable_iter().collect();
            if ms.is_empty() {
                board.pass();
                continue;
            }
            let p = ms[rng.below(ms.len())];
            board.make_move_unchecked(p);
        }
        // Openings come in color-swapped pairs (cancels the first-move edge).
        let mut a_turn = g % 2 == 0;
        /* Per-game record: without a book and with equal openings the
        move sequence should match across runs; a mismatch means ponder
        changed the moves, invalidating the time comparison. */
        let mut line = String::new();
        let g_start = (ta, tb);

        while !board.is_game_over() {
            if board.movable_count() == 0 {
                board.pass();
                a_turn = !a_turn;
                continue;
            }
            let deadline = Instant::now() + budget;
            if a_turn {
                let t0 = Instant::now();
                let ev = if args.fixed {
                    a.choose(&board)
                } else {
                    a.choose_within(&board, Some(deadline))
                };
                ta += t0.elapsed().as_micros();
                na += 1;
                da.add(ev.depth);
                let Some(p) = ev.pos else { break };
                line.push_str(&format!("{},", p.index()));
                board.make_move_unchecked(p);
                /* The ponder itself. Live it runs on the opponent's
                time, so A's clock is unaffected; here it runs for B's
                allotment. */
                if args.ponder && !board.is_game_over() && board.movable_count() > 0 {
                    let slice = if args.fixed {
                        Duration::from_millis(args.ponder_ms)
                    } else {
                        budget
                    };
                    let t = Instant::now();
                    ponder_nodes += a.ponder(&board, Instant::now() + slice);
                    if board.empty_count() <= args.solve_empties {
                        end_n += 1;
                        end_ms += t.elapsed().as_millis();
                    }
                }
            } else {
                let t0 = Instant::now();
                let ev = if args.fixed {
                    b.choose(&board)
                } else {
                    b.choose_within(&board, Some(deadline))
                };
                tb += t0.elapsed().as_micros();
                nb += 1;
                db.add(ev.depth);
                let Some(p) = ev.pos else { break };
                line.push_str(&format!("{},", p.index()));
                board.make_move_unchecked(p);
            }
            a_turn = !a_turn;
        }

        // Disc difference recounted from A's color (not mover view).
        let (bl, wh) = (
            board.player_bb().count_ones() as i32,
            board.opponent_bb().count_ones() as i32,
        );
        // `player_bb` is the mover's discs; orient by whether A moved last.
        let diff = if a_turn { bl - wh } else { wh - bl };
        match diff.cmp(&0) {
            std::cmp::Ordering::Greater => wins += 1,
            std::cmp::Ordering::Less => losses += 1,
            std::cmp::Ordering::Equal => draws += 1,
        }
        // Per-game totals plus a record fingerprint for cross-run checks.
        let mut h: u64 = 1469598103934665603;
        for b in line.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        println!(
            "  game {:>2}  A {:>8.3} s  B {:>8.3} s  record {:016x}",
            g + 1,
            (ta - g_start.0) as f64 / 1e6,
            (tb - g_start.1) as f64 / 1e6,
            h,
        );
    }
    eprintln!();

    let m = if args.ponder { "on" } else { "off (control)" };
    println!(
        "ponderarena: {} ms per move / depth cap {} / solve {} / {} threads / {} games (no book)",
        args.ms, args.depth, args.solve_empties, args.threads, args.games,
    );
    println!("  A = pondering {m} / B = off");
    if args.fixed {
        let (aa, bb) = (
            ta as f64 / na.max(1) as f64 / 1000.0,
            tb as f64 / nb.max(1) as f64 / 1000.0,
        );
        /* Fixed-depth metric is total search time; win rate is
        meaningless (same depth = same moves). Compare A-with-ponder vs
        A-without across seeds, not A vs B — A and B hold different
        colors and positions. B's totals should match across runs; that
        is the control. */
        println!(
            "  search total  A {:.2} s ({} moves / {:.1} ms per move)   B {:.2} s ({} moves / {:.1} ms per move)",
            ta as f64 / 1e6,
            na,
            aa,
            tb as f64 / 1e6,
            nb,
            bb,
        );
        println!(
            "  Note: run --ponder off with the same seed and compare **A total against A total**.\n     If B totals differ between the two runs, the comparison is unusable"
        );
    }
    println!(
        "  depth reached  A {:.2}  B {:.2}   diff {:+.2} plies",
        da.avg(),
        db.avg(),
        da.avg() - db.avg()
    );
    println!(
        "  nodes visited while pondering {} ({:.0} / move)",
        ponder_nodes,
        if da.n == 0 {
            0.0
        } else {
            ponder_nodes as f64 / da.n as f64
        }
    );
    println!(
        "  ponders in the solve range {} / {} ms total",
        end_n, end_ms
    );
    let total = (wins + losses + draws) as f64;
    println!(
        "  A record  {}W {}L {}D ({:.1}%)",
        wins,
        losses,
        draws,
        100.0 * (wins as f64 + 0.5 * draws as f64) / total.max(1.0)
    );
    println!("  Note: the win rate is underpowered at this game count; judge the effect by the depth difference");
    ExitCode::SUCCESS
}
