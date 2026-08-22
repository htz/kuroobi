//! Measures how often the ponder prediction hits.
//!
//! The single-predicted-move scheme's value is mostly this hit rate:
//! a hit reuses the whole tree, a miss keeps only table residue. The
//! prediction comes from the table (`Engine::tt_best`), never a fresh
//! search — live pondering has no better source, so measuring a better
//! predictor would be meaningless.
//!
//! Self-play caveat: the opponent is the same engine. Hence the
//! adjustable opponent — equal settings give a near-upper-bound,
//! a shallower opponent approximates humans/other engines.
//!
//! Usage:
//!   ponderhit [OPTIONS]
//!
//! Options:
//!   --games <n>          number of games (default 20)
//!   --depth <n>          our midgame depth (default 8)
//!   --solve-empties <n>  our solve entry (default 14)
//!   --opp-depth <n>      opponent midgame depth (default = --depth)
//!   --opp-solve <n>      opponent solve entry (default = --solve-empties)
//!   --threads <n>        threads (default 1)
//!   --random-plies <n>   random opening plies (default 8)
//!   --seed <n>           RNG seed (default 7)
//!   --nnue <path>        NNUE weights (default weights/nnue-h16.bin)
//!   --weights <path>     linear weights (default weights/linear.bin)

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::board::Board;
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::position::Position;

struct Args {
    games: usize,
    depth: u32,
    solve_empties: u8,
    opp_depth: Option<u32>,
    opp_solve: Option<u8>,
    threads: usize,
    random_plies: usize,
    seed: u64,
    nnue: PathBuf,
    weights: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            games: 20,
            depth: 8,
            solve_empties: 14,
            opp_depth: None,
            opp_solve: None,
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
            "--depth" => a.depth = val()?.parse().map_err(|_| "bad --depth")?,
            "--solve-empties" => {
                a.solve_empties = val()?.parse().map_err(|_| "bad --solve-empties")?
            }
            "--opp-depth" => a.opp_depth = Some(val()?.parse().map_err(|_| "bad --opp-depth")?),
            "--opp-solve" => a.opp_solve = Some(val()?.parse().map_err(|_| "bad --opp-solve")?),
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

/// RNG; only diversifies openings, xorshift suffices.
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

/// Empty count as game progress (60 -> 0).
fn empties(b: &Board) -> u32 {
    64 - (b.player_bb() | b.opponent_bb()).count_ones()
}

/// Report in three phases: opening-only hits are cheap — the midgame
/// after book exit is what matters.
fn phase(e: u32) -> usize {
    match e {
        45..=64 => 0, // opening
        21..=44 => 1, // midgame
        _ => 2,       // endgame
    }
}
const PHASE_NAME: [&str; 3] = ["opening (45+ empty)", "midgame (21-44)", "endgame (20-)"];

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let cfg = |depth: u32, solve: u8| EngineConfig {
        depth,
        solve_empties: solve,
        threads: args.threads,
        nnue: args.nnue.clone(),
        weights: args.weights.clone(),
        // Book off: book moves leave nothing in the table, making
        // no-prediction trivial; we measure the search's predictive power.
        use_book: false,
        ..EngineConfig::default()
    };

    let mut me = match Engine::new(cfg(args.depth, args.solve_empties)) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut opp = match Engine::new(cfg(
        args.opp_depth.unwrap_or(args.depth),
        args.opp_solve.unwrap_or(args.solve_empties),
    )) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut rng = Rng(args.seed | 1);
    // [predicted, hit] per phase.
    let mut got = [0u64; 3];
    let mut hit = [0u64; 3];
    // Times no prediction existed (evicted from the table).
    let mut miss_pred = [0u64; 3];
    /* Opponent's legal-move count: scheme 2's 1/N. Othello should be
    narrow, but measure rather than assume. */
    let mut br_sum = [0u64; 3];
    let mut br_n = [0u64; 3];

    for g in 0..args.games {
        let mut b = Board::new();
        // Diversify openings; repeating one line yields one game's info.
        for _ in 0..args.random_plies {
            if b.is_game_over() {
                break;
            }
            let ms: Vec<Position> = b.movable_iter().collect();
            if ms.is_empty() {
                b.pass();
                continue;
            }
            let p = ms[rng.below(ms.len())];
            b.make_move_unchecked(p);
        }

        // We move first on even games; without the swap only one side's
        // turns get measured.
        let mut my_turn = g % 2 == 0;
        while !b.is_game_over() {
            if b.movable_count() == 0 {
                b.pass();
                my_turn = !my_turn;
                continue;
            }
            if my_turn {
                let ev = me.choose(&b);
                let Some(p) = ev.pos else { break };
                let e = empties(&b);
                b.make_move_unchecked(p);
                // The post-move position's best = the predicted reply.
                let pred = me.tt_best(&b);
                my_turn = false;

                // Opponent's turn; a pass changes who the prediction is
                // about, so skip those.
                if b.is_game_over() || b.movable_count() == 0 {
                    continue;
                }
                let actual = opp.choose(&b).pos;
                let Some(actual) = actual else { break };
                let ph = phase(e);
                br_sum[ph] += b.movable_count() as u64;
                br_n[ph] += 1;
                match pred {
                    Some(pred) if pred == actual => {
                        got[ph] += 1;
                        hit[ph] += 1;
                    }
                    Some(_) => got[ph] += 1,
                    None => miss_pred[ph] += 1,
                }
                b.make_move_unchecked(actual);
                my_turn = true;
            } else {
                let Some(p) = opp.choose(&b).pos else { break };
                b.make_move_unchecked(p);
                my_turn = true;
            }
        }
        eprint!("\r{}/{} games", g + 1, args.games);
    }
    eprintln!();

    println!(
        "ponderhit: self depth {} solve {} / opponent depth {} solve {} ({} games, {} random opening plies, no book)",
        args.depth,
        args.solve_empties,
        args.opp_depth.unwrap_or(args.depth),
        args.opp_solve.unwrap_or(args.solve_empties),
        args.games,
        args.random_plies,
    );
    let mut t_got = 0u64;
    let mut t_hit = 0u64;
    let mut t_none = 0u64;
    for i in 0..3 {
        let n = got[i] + miss_pred[i];
        if n == 0 {
            continue;
        }
        println!(
            "  {:<18} predicted {:>5}/{:<5} ({:>4.1}%)   hit {:>5}/{:<5} ({:>4.1}%)",
            PHASE_NAME[i],
            got[i],
            n,
            100.0 * got[i] as f64 / n as f64,
            hit[i],
            got[i],
            if got[i] == 0 {
                0.0
            } else {
                100.0 * hit[i] as f64 / got[i] as f64
            },
        );
        if br_n[i] > 0 {
            println!(
                "  {:<18} opponent legal moves avg {:.1} (the 1/N of scheme 2)",
                "",
                br_sum[i] as f64 / br_n[i] as f64
            );
        }
        t_got += got[i];
        t_hit += hit[i];
        t_none += miss_pred[i];
    }
    let n = t_got + t_none;
    if n > 0 {
        println!(
            "  {:<18} predicted {:>5}/{:<5} ({:>4.1}%)   hit {:>5}/{:<5} ({:>4.1}%)",
            "total",
            t_got,
            n,
            100.0 * t_got as f64 / n as f64,
            t_hit,
            t_got,
            if t_got == 0 {
                0.0
            } else {
                100.0 * t_hit as f64 / t_got as f64
            },
        );
        // Rate counting no-prediction as a miss; scheme 1's true expectation.
        println!(
            "  effective hit rate (no prediction counted as a miss): {:.1}%",
            100.0 * t_hit as f64 / n as f64
        );
    }
    ExitCode::SUCCESS
}
