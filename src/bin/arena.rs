//! Head-to-head arena: play two weight files against each other and report
//! the win rate with a 95% confidence interval.
//!
//! Both engines pick 1-ply greedy moves with their own evaluator; the
//! endgame is played out the same way (no solver) so only the evaluators
//! differ. Opening diversity comes from playing k random plies before the
//! engines take over; each opening is played twice with colors swapped to
//! cancel first-mover bias.
//!
//! Usage:
//!   arena --a <weights-A> --b <weights-B> [OPTIONS]
//!
//! Options:
//!   --games <n>        Total games (default 1000; rounded up to even)
//!   --random-plies <n> Random opening plies (default 6)
//!   --patterns <set>   egaroucid | edax (default egaroucid)
//!   --seed <n>         RNG seed (default 7)

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::evaluator::Evaluator;
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::search::Searcher;
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::{Board, Color, Position};

struct Args {
    weights_a: PathBuf,
    weights_b: PathBuf,
    games: usize,
    random_plies: usize,
    depth: u8,
    solve_empties: u8,
    /// Per-side overrides; fall back to the shared values above.
    depth_a: Option<u8>,
    depth_b: Option<u8>,
    solve_a: Option<u8>,
    solve_b: Option<u8>,
    patterns: &'static str,
    patterns_a: Option<&'static str>,
    patterns_b: Option<&'static str>,
    seed: u64,
    mpc_a: bool,
    mpc_b: bool,
    mpc_t: f32,
}

fn parse_patterns(v: &str) -> Result<&'static str, String> {
    match v {
        "egaroucid" => Ok("egaroucid"),
        "egaroucid-plus" => Ok("egaroucid-plus"),
        "edax" => Ok("edax"),
        other => Err(format!("unknown pattern set: {other}")),
    }
}

const USAGE: &str = "\
Usage: arena --a <weights-A> --b <weights-B> [OPTIONS]

Play two weight files head-to-head (1-ply greedy both sides) and report
A's win rate with a 95% confidence interval. Each random opening is played
twice with colors swapped.

Options:
  --games <n>          Total games (default 1000, rounded up to even)
  --random-plies <n>   Random opening plies for diversity (default 6)
  --depth <n>          Search depth for both sides; 1 = greedy (default 1)
  --solve-empties <n>  Both sides play the endgame perfectly from n empties
                       (default 0 = off; matches real play conditions)
  --depth-a <n>        Search depth for A only (default: --depth)
  --depth-b <n>        Search depth for B only (default: --depth)
  --solve-a <n>        A's perfect-endgame threshold (default: --solve-empties)
  --solve-b <n>        B's perfect-endgame threshold (default: --solve-empties)
  --patterns <set>     egaroucid | edax | egaroucid-plus (default egaroucid)
  --patterns-a <set>   A's pattern library (default: --patterns)
  --patterns-b <set>   B's pattern library (default: --patterns)
  --seed <n>           RNG seed (default 7)
  -h, --help           Show this help";

fn parse_args() -> Result<Args, String> {
    let mut weights_a = None;
    let mut weights_b = None;
    let mut args = Args {
        weights_a: PathBuf::new(),
        weights_b: PathBuf::new(),
        games: 1000,
        random_plies: 6,
        depth: 1,
        solve_empties: 0,
        depth_a: None,
        depth_b: None,
        solve_a: None,
        solve_b: None,
        patterns: "egaroucid",
        patterns_a: None,
        patterns_b: None,
        seed: 7,
        mpc_a: false,
        mpc_b: false,
        mpc_t: 1.1,
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} requires a value"));
        match arg.as_str() {
            "--a" => weights_a = Some(PathBuf::from(value("--a")?)),
            "--b" => weights_b = Some(PathBuf::from(value("--b")?)),
            "--games" => {
                args.games = value("--games")?
                    .parse()
                    .map_err(|e| format!("--games: {e}"))?
            }
            "--random-plies" => {
                args.random_plies = value("--random-plies")?
                    .parse()
                    .map_err(|e| format!("--random-plies: {e}"))?
            }
            "--depth" => {
                args.depth = value("--depth")?
                    .parse()
                    .map_err(|e| format!("--depth: {e}"))?
            }
            "--solve-empties" => {
                args.solve_empties = value("--solve-empties")?
                    .parse()
                    .map_err(|e| format!("--solve-empties: {e}"))?
            }
            "--depth-a" => {
                args.depth_a = Some(
                    value("--depth-a")?
                        .parse()
                        .map_err(|e| format!("--depth-a: {e}"))?,
                )
            }
            "--depth-b" => {
                args.depth_b = Some(
                    value("--depth-b")?
                        .parse()
                        .map_err(|e| format!("--depth-b: {e}"))?,
                )
            }
            "--solve-a" => {
                args.solve_a = Some(
                    value("--solve-a")?
                        .parse()
                        .map_err(|e| format!("--solve-a: {e}"))?,
                )
            }
            "--solve-b" => {
                args.solve_b = Some(
                    value("--solve-b")?
                        .parse()
                        .map_err(|e| format!("--solve-b: {e}"))?,
                )
            }
            "--patterns" => args.patterns = parse_patterns(&value("--patterns")?)?,
            "--patterns-a" => args.patterns_a = Some(parse_patterns(&value("--patterns-a")?)?),
            "--patterns-b" => args.patterns_b = Some(parse_patterns(&value("--patterns-b")?)?),
            "--seed" => {
                args.seed = value("--seed")?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            "--mpc-a" => args.mpc_a = true,
            "--mpc-b" => args.mpc_b = true,
            "--mpc-t" => {
                args.mpc_t = value("--mpc-t")?
                    .parse()
                    .map_err(|e| format!("--mpc-t: {e}"))?
            }
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown option: {other}\n\n{USAGE}")),
        }
    }

    args.weights_a = weights_a.ok_or(format!("--a is required\n\n{USAGE}"))?;
    args.weights_b = weights_b.ok_or(format!("--b is required\n\n{USAGE}"))?;
    args.games = args.games.div_ceil(2) * 2;
    Ok(args)
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }
}

fn nth_move(mut mask: u64, mut n: u32) -> Position {
    while n > 0 {
        mask &= mask - 1;
        n -= 1;
    }
    Position::from_index(mask.trailing_zeros()).unwrap()
}

fn greedy_move(board: &Board, evaluator: &Evaluator) -> Position {
    let moves = board.movable();
    let mut best_pos = None;
    let mut best_val = f32::NEG_INFINITY;
    let mut m = moves;
    while m != 0 {
        let pos = Position::from_index(m.trailing_zeros()).unwrap();
        m &= m - 1;
        let mut child = *board;
        child.make_move_bits(pos);
        let val = -evaluator.eval(&child);
        if val > best_val {
            best_val = val;
            best_pos = Some(pos);
        }
    }
    best_pos.expect("at least one legal move")
}

/// Generate a random opening position with `plies` random moves.
fn random_opening(rng: &mut Rng, plies: usize) -> Board {
    let mut board = Board::new();
    for _ in 0..plies {
        let moves = board.movable();
        if moves == 0 {
            break;
        }
        board.make_move_bits(nth_move(moves, rng.below(moves.count_ones())));
    }
    board
}

/// One side's playing strength: evaluator, search depth (1 = greedy) and
/// the empties threshold from which it plays perfect endgame moves (0 = off).
struct EngineConfig<'a> {
    evaluator: &'a Evaluator,
    depth: u8,
    solve_empties: u8,
}

/// Play out one game from `start`; each side picks moves per its own
/// [`EngineConfig`]. Returns final score from Black's view.
///
/// Each engine uses its OWN searcher: transposition-table entries embed
/// evaluator-specific values, so sharing one table across engines would
/// let one engine consume the other's evaluations and silently blend them.
/// The solver IS shared: exact endgame values are evaluator-independent.
fn play(
    start: &Board,
    black: &EngineConfig,
    white: &EngineConfig,
    black_searcher: &mut Searcher,
    white_searcher: &mut Searcher,
    solver: &mut Solver,
) -> i32 {
    // Once BOTH sides are inside their perfect-play windows the outcome
    // is fixed; resolve it with one solver call instead of playing on.
    let both_perfect_from = black.solve_empties.min(white.solve_empties);

    let mut board = *start;
    loop {
        if board.movable() == 0 {
            let mut passed = board;
            passed.pass();
            if passed.movable() == 0 {
                let s = board.score();
                return if board.player() == Color::Black {
                    s
                } else {
                    -s
                };
            }
            board = passed;
            continue;
        }

        if both_perfect_from > 0 && board.empty_count() <= both_perfect_from {
            let result =
                solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(black.evaluator));
            let s = result.value;
            return if board.player() == Color::Black {
                s
            } else {
                -s
            };
        }

        let is_black = board.player() == Color::Black;
        let cfg = if is_black { black } else { white };
        let pos = if cfg.solve_empties > 0 && board.empty_count() <= cfg.solve_empties {
            solver
                .solve_with_eval(EndSolverMode::Perfect, &board, Some(cfg.evaluator))
                .best_move
                .expect("legal move exists")
        } else if cfg.depth <= 1 {
            greedy_move(&board, cfg.evaluator)
        } else {
            let searcher = if is_black {
                &mut *black_searcher
            } else {
                &mut *white_searcher
            };
            searcher
                .search(&board, cfg.evaluator, cfg.depth)
                .best_move
                .expect("legal move exists")
        };
        board.make_move_bits(pos);
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let lib = |name: &str| match name {
        "edax" => EDAX_PATTERNS,
        "egaroucid-plus" => EGAROUCID_PLUS_PATTERNS,
        _ => EGAROUCID_PATTERNS,
    };
    let patterns_a = lib(args.patterns_a.unwrap_or(args.patterns));
    let patterns_b = lib(args.patterns_b.unwrap_or(args.patterns));

    let mut eval_a = Evaluator::new(patterns_a);
    if let Err(e) = eval_a.load_weights(&args.weights_a) {
        eprintln!("failed to load {}: {e}", args.weights_a.display());
        return ExitCode::FAILURE;
    }
    let mut eval_b = Evaluator::new(patterns_b);
    if let Err(e) = eval_b.load_weights(&args.weights_b) {
        eprintln!("failed to load {}: {e}", args.weights_b.display());
        return ExitCode::FAILURE;
    }

    let cfg_a = EngineConfig {
        evaluator: &eval_a,
        depth: args.depth_a.unwrap_or(args.depth),
        solve_empties: args.solve_a.unwrap_or(args.solve_empties),
    };
    let cfg_b = EngineConfig {
        evaluator: &eval_b,
        depth: args.depth_b.unwrap_or(args.depth),
        solve_empties: args.solve_b.unwrap_or(args.solve_empties),
    };

    println!(
        "arena: A={} (depth {}, solve {}) vs B={} (depth {}, solve {})  ({} games, {} random plies)",
        args.weights_a.display(),
        cfg_a.depth,
        cfg_a.solve_empties,
        args.weights_b.display(),
        cfg_b.depth,
        cfg_b.solve_empties,
        args.games,
        args.random_plies,
    );

    let mut rng = Rng::new(args.seed);
    // One searcher per evaluator: TT values are evaluator-specific
    let mut searcher_a = Searcher::new(17);
    let mut searcher_b = Searcher::new(17);
    searcher_a.mpc = args.mpc_a;
    searcher_b.mpc = args.mpc_b;
    searcher_a.mpc_t = args.mpc_t;
    searcher_b.mpc_t = args.mpc_t;
    let mut solver = Solver::new(18);
    let mut a_wins = 0usize;
    let mut b_wins = 0usize;
    let mut draws = 0usize;
    let mut a_disc_sum = 0i64;

    for _pair in 0..args.games / 2 {
        let opening = random_opening(&mut rng, args.random_plies);

        // Fresh TTs per game: with warm tables, cached deeper entries from
        // earlier games change move choices, so the color-swapped replay of
        // the same opening is no longer a mirror — measured as A scoring
        // ~42% in a same-weights self-match. Cold TTs make play a pure
        // deterministic function of (opening, weights, depth): a
        // same-weights match is exactly 50% by construction.
        searcher_a.clear();
        searcher_b.clear();

        // Game 1: A plays Black
        let s1 = play(
            &opening,
            &cfg_a,
            &cfg_b,
            &mut searcher_a,
            &mut searcher_b,
            &mut solver,
        );
        a_disc_sum += s1 as i64;
        match s1.cmp(&0) {
            std::cmp::Ordering::Greater => a_wins += 1,
            std::cmp::Ordering::Less => b_wins += 1,
            std::cmp::Ordering::Equal => draws += 1,
        }

        // Game 2: colors swapped (searcher stays with its evaluator)
        searcher_a.clear();
        searcher_b.clear();
        let s2 = play(
            &opening,
            &cfg_b,
            &cfg_a,
            &mut searcher_b,
            &mut searcher_a,
            &mut solver,
        );
        a_disc_sum -= s2 as i64;
        match s2.cmp(&0) {
            std::cmp::Ordering::Greater => b_wins += 1,
            std::cmp::Ordering::Less => a_wins += 1,
            std::cmp::Ordering::Equal => draws += 1,
        }
    }

    let n = (a_wins + b_wins + draws) as f64;
    // Win rate counting draws as half
    let p = (a_wins as f64 + draws as f64 / 2.0) / n;
    // 95% CI via normal approximation
    let se = (p * (1.0 - p) / n).sqrt();
    let (lo, hi) = (p - 1.96 * se, p + 1.96 * se);

    println!("A wins {a_wins}, B wins {b_wins}, draws {draws}");
    println!(
        "A score {:.1}%  (95% CI {:.1}%..{:.1}%)  mean disc diff {:+.2}",
        p * 100.0,
        lo * 100.0,
        hi * 100.0,
        a_disc_sum as f64 / n
    );
    if lo > 0.5 {
        println!("=> A is significantly stronger");
    } else if hi < 0.5 {
        println!("=> B is significantly stronger");
    } else {
        println!("=> no significant difference");
    }
    ExitCode::SUCCESS
}
