//! Self-play reinforcement training CLI.
//!
//! Plays games where both sides pick moves with the current evaluator
//! (1-ply greedy with ε-random exploration), resolves the endgame exactly
//! with the solver, then updates the evaluator from each game with TD(λ).
//!
//! Usage:
//!   selfplay [OPTIONS]
//!
//! Options:
//!   --games <n>       Number of self-play games (default 10000)
//!   --weights <path>  Weight file to load and update (default weights.bin)
//!   --lr <f>          SGD learning rate (default 0.0005)
//!   --decay <f>       lr decay applied every --save-every games (default 1.0)
//!   --lambda <f>      TD(λ) mixing: 1.0 = Monte-Carlo, 0.0 = TD(0)
//!                     (default 0.7)
//!   --epsilon <f>     Exploration: probability of a uniformly random move
//!                     (default 0.10)
//!   --solve-empties <n>  Switch to exact endgame solving at n empties
//!                     (default 12; 0 disables)
//!   --patterns <set>  egaroucid | edax (default egaroucid)
//!   --save-every <n>  Save weights every n games (default 500)
//!   --seed <n>        RNG seed (default 42)
//!   --log <path>      Append per-window stats as CSV

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use kuroobi::evaluator::{Evaluator, SgdOptimizer};
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::search::Searcher;
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::{Board, Color, Position};

struct Args {
    games: usize,
    weights_path: PathBuf,
    learning_rate: f32,
    decay: f32,
    lambda: f32,
    epsilon: f32,
    depth: u8,
    solve_empties: u8,
    patterns: &'static str,
    save_every: usize,
    seed: u64,
    log_path: Option<PathBuf>,
    opponents: Vec<PathBuf>,
}

const USAGE: &str = "\
Usage: selfplay [OPTIONS]

Reinforce the evaluator through self-play: ε-greedy move selection with
midgame alpha-beta search, exact endgame resolution, TD(λ) updates per
game. With --opponents, the learner plays a league against frozen past
weights (colors alternate) instead of pure self-play.

Options:
  --games <n>          Self-play games to run (default 10000)
  --weights <path>     Weight file to load and update (default weights.bin)
  --depth <n>          Midgame search depth in plies; 1 = greedy (default 4)
  --opponents <paths>  Comma-separated frozen opponent weight files
  --lr <f>             SGD learning rate (default 0.0005)
  --decay <f>          lr decay per save window (default 1.0 = none)
  --lambda <f>         TD(λ): 1.0=Monte-Carlo .. 0.0=TD(0) (default 0.7)
  --epsilon <f>        Random-move exploration rate (default 0.10)
  --solve-empties <n>  Exact endgame solve threshold (default 12, 0=off)
  --patterns <set>     egaroucid | edax | egaroucid-plus (default egaroucid)
  --save-every <n>     Save weights every n games (default 500)
  --seed <n>           RNG seed (default 42)
  --log <path>         Append per-window stats as CSV
  -h, --help           Show this help";

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        games: 10_000,
        weights_path: PathBuf::from("weights/weights.bin"),
        learning_rate: 0.0005,
        decay: 1.0,
        lambda: 0.7,
        epsilon: 0.10,
        depth: 4,
        solve_empties: 12,
        patterns: "egaroucid",
        save_every: 500,
        seed: 42,
        log_path: None,
        opponents: Vec::new(),
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next().ok_or_else(|| format!("{name} requires a value"))
        };
        match arg.as_str() {
            "--games" => args.games = value("--games")?.parse().map_err(|e| format!("--games: {e}"))?,
            "--weights" => args.weights_path = PathBuf::from(value("--weights")?),
            "--lr" => args.learning_rate = value("--lr")?.parse().map_err(|e| format!("--lr: {e}"))?,
            "--decay" => args.decay = value("--decay")?.parse().map_err(|e| format!("--decay: {e}"))?,
            "--lambda" => args.lambda = value("--lambda")?.parse().map_err(|e| format!("--lambda: {e}"))?,
            "--epsilon" => args.epsilon = value("--epsilon")?.parse().map_err(|e| format!("--epsilon: {e}"))?,
            "--depth" => args.depth = value("--depth")?.parse().map_err(|e| format!("--depth: {e}"))?,
            "--opponents" => {
                args.opponents = value("--opponents")?
                    .split(',')
                    .map(PathBuf::from)
                    .collect();
            }
            "--solve-empties" => args.solve_empties = value("--solve-empties")?.parse().map_err(|e| format!("--solve-empties: {e}"))?,
            "--patterns" => {
                let v = value("--patterns")?;
                match v.as_str() {
                    "egaroucid" => args.patterns = "egaroucid",
                    "egaroucid-plus" => args.patterns = "egaroucid-plus",
                    "edax" => args.patterns = "edax",
                    other => return Err(format!("unknown pattern set: {other}")),
                }
            }
            "--save-every" => args.save_every = value("--save-every")?.parse().map_err(|e| format!("--save-every: {e}"))?,
            "--seed" => args.seed = value("--seed")?.parse().map_err(|e| format!("--seed: {e}"))?,
            "--log" => args.log_path = Some(PathBuf::from(value("--log")?)),
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown option: {other}\n\n{USAGE}")),
        }
    }
    Ok(args)
}

/// xorshift64* PRNG — deterministic, dependency-free.
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

    /// Uniform float in [0, 1).
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform integer in [0, n).
    fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }
}

/// Pick the nth set bit of a mask.
fn nth_move(mut mask: u64, mut n: u32) -> Position {
    while n > 0 {
        mask &= mask - 1;
        n -= 1;
    }
    Position::from_index(mask.trailing_zeros()).unwrap()
}

/// Greedy 1-ply move: maximize our value = minimize the opponent's eval of
/// the child position (child eval is from the opponent's perspective).
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
    best_pos.expect("greedy_move called with at least one legal move")
}

/// Pick a move for the side to move: ε-random, else depth-limited search
/// (depth <= 1 falls back to the cheap greedy path).
#[allow(clippy::too_many_arguments)]
fn choose_move(
    board: &Board,
    evaluator: &Evaluator,
    searcher: &mut Searcher,
    rng: &mut Rng,
    epsilon: f32,
    depth: u8,
) -> Position {
    let moves = board.movable();
    if rng.next_f32() < epsilon {
        nth_move(moves, rng.below(moves.count_ones()))
    } else if depth <= 1 {
        greedy_move(board, evaluator)
    } else {
        searcher
            .search(board, evaluator, depth)
            .best_move
            .expect("side to move has legal moves")
    }
}

struct GameOutcome {
    history: Vec<Board>,
    /// Final disc difference from Black's perspective.
    score: f32,
}

/// Play one training game. The learner plays `learner_color` with the
/// learning evaluator; the opponent evaluator plays the other side (for
/// pure self-play both are the same evaluator). Only the learner's
/// positions... actually ALL positions are recorded: TD targets are
/// perspective-corrected, so both sides' positions are valid samples.
/// When the board reaches `solve_empties`, the endgame is resolved exactly
/// by the solver and the exact score becomes the game outcome.
///
/// Each side gets its own searcher because TT entries embed
/// evaluator-specific values; the learner's TT must additionally be
/// cleared by the caller after each weight update.
#[allow(clippy::too_many_arguments)]
fn play_game(
    learner: &Evaluator,
    opponent: &Evaluator,
    learner_is_black: bool,
    learner_searcher: &mut Searcher,
    opponent_searcher: &mut Searcher,
    solver: &mut Solver,
    rng: &mut Rng,
    epsilon: f32,
    depth: u8,
    solve_empties: u8,
) -> GameOutcome {
    let mut board = Board::new();
    let mut history = Vec::with_capacity(60);

    loop {
        // Exact endgame resolution (skipped when the side to move has no
        // move: fall through to the pass/game-over logic below)
        if solve_empties > 0 && board.empty_count() <= solve_empties && board.check_all() {
            let result = solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(learner));
            // result.value is from the current player's view
            let score_black = if board.player() == Color::Black {
                result.value as f32
            } else {
                -(result.value as f32)
            };
            return GameOutcome { history, score: score_black };
        }

        let moves = board.movable();
        if moves == 0 {
            let mut passed = board;
            passed.pass();
            if passed.movable() == 0 {
                // Game over: score with empty-square bonus, Black's view
                let score = board.score() as f32;
                let score_black = if board.player() == Color::Black {
                    score
                } else {
                    -score
                };
                return GameOutcome { history, score: score_black };
            }
            board = passed;
            continue;
        }

        history.push(board);

        let mover_is_learner = (board.player() == Color::Black) == learner_is_black;
        let (eval, searcher) = if mover_is_learner {
            (learner, &mut *learner_searcher)
        } else {
            (opponent, &mut *opponent_searcher)
        };
        let pos = choose_move(&board, eval, searcher, rng, epsilon, depth);
        board.make_move_bits(pos);
    }
}

fn append_log(
    path: &Path,
    games_done: usize,
    window_err: f32,
    black_wins: usize,
    white_wins: usize,
    draws: usize,
    lr: f32,
) -> std::io::Result<()> {
    use std::io::Write;
    let new_file = !path.exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if new_file {
        writeln!(f, "games,mean_abs_err,black_wins,white_wins,draws,lr")?;
    }
    writeln!(f, "{games_done},{window_err:.4},{black_wins},{white_wins},{draws},{lr:.6}")
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let patterns = match args.patterns {
        "edax" => EDAX_PATTERNS,
        "egaroucid-plus" => EGAROUCID_PLUS_PATTERNS,
        _ => EGAROUCID_PATTERNS,
    };

    let mut evaluator = Evaluator::new(patterns);
    if args.weights_path.exists() {
        match evaluator.load_weights(&args.weights_path) {
            Ok(()) => println!("loaded weights from {}", args.weights_path.display()),
            Err(e) => {
                eprintln!("failed to load {}: {e}", args.weights_path.display());
                return ExitCode::FAILURE;
            }
        }
    } else {
        println!("warning: {} not found, starting from zero weights", args.weights_path.display());
    }

    // Frozen league opponents (empty = pure self-play)
    let mut opponents: Vec<Evaluator> = Vec::new();
    for path in &args.opponents {
        let mut opp = Evaluator::new(patterns);
        match opp.load_weights(path) {
            Ok(()) => {
                println!("league opponent: {}", path.display());
                opponents.push(opp);
            }
            Err(e) => {
                eprintln!("failed to load opponent {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }

    println!(
        "selfplay: {} games, depth {}, lr {}, λ {}, ε {}, solve at {} empties, save every {}{}",
        args.games,
        args.depth,
        args.learning_rate,
        args.lambda,
        args.epsilon,
        args.solve_empties,
        args.save_every,
        if opponents.is_empty() {
            String::new()
        } else {
            format!(", league of {}", opponents.len())
        }
    );

    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let flag = interrupted.clone();
        if let Err(e) = ctrlc_handler(move || {
            if flag.swap(true, Ordering::SeqCst) {
                eprintln!("\nsecond interrupt: exiting immediately");
                std::process::exit(130);
            }
            eprintln!("\ninterrupt received: finishing window, then saving...");
        }) {
            eprintln!("warning: could not install Ctrl-C handler: {e}");
        }
    }

    let mut opt = SgdOptimizer::new(args.learning_rate, 1.0);
    let mut solver = Solver::new(18);
    // Separate searchers: the learner's evaluator changes every game, so
    // its TT is stale after each update and must be cleared; frozen
    // opponents keep a valid TT for their (never-changing) weights.
    let mut learner_searcher = Searcher::new(17);
    let mut opponent_searcher = Searcher::new(17);
    let mut rng = Rng::new(args.seed);

    let started = Instant::now();
    let mut window_err = 0.0f32;
    let mut window_games = 0usize;
    let mut black_wins = 0usize;
    let mut white_wins = 0usize;
    let mut draws = 0usize;

    for game_no in 1..=args.games {
        // League mode: rotate opponents; learner alternates colors.
        // Pure self-play when no opponents are given.
        let learner_is_black = game_no % 2 == 1;
        let outcome = if opponents.is_empty() {
            play_game(
                &evaluator, &evaluator, learner_is_black,
                &mut learner_searcher, &mut opponent_searcher, &mut solver, &mut rng,
                args.epsilon, args.depth, args.solve_empties,
            )
        } else {
            let opp = &opponents[(game_no / 2) % opponents.len()];
            play_game(
                &evaluator, opp, learner_is_black,
                &mut learner_searcher, &mut opponent_searcher, &mut solver, &mut rng,
                args.epsilon, args.depth, args.solve_empties,
            )
        };

        match outcome.score.partial_cmp(&0.0) {
            Some(std::cmp::Ordering::Greater) => black_wins += 1,
            Some(std::cmp::Ordering::Less) => white_wins += 1,
            _ => draws += 1,
        }

        window_err += evaluator.train_game(&outcome.history, outcome.score, args.lambda, &mut opt);
        window_games += 1;

        // Weights changed: the learner's cached search values are stale.
        // (In pure self-play the "opponent" shares the learner's evaluator,
        // so its TT is stale too.)
        learner_searcher.clear();
        if opponents.is_empty() {
            opponent_searcher.clear();
        }

        if game_no % args.save_every == 0 || game_no == args.games || interrupted.load(Ordering::SeqCst) {
            let mean_err = window_err / window_games.max(1) as f32;
            let elapsed = started.elapsed().as_secs_f32();
            let gps = game_no as f32 / elapsed;
            println!(
                "game {:>7}/{}: mse {:.3}  B/W/D {}/{}/{}  lr {:.6}  ({:.1} games/s)",
                game_no, args.games, mean_err, black_wins, white_wins, draws, opt.learning_rate, gps
            );

            if let Some(log) = &args.log_path {
                if let Err(e) = append_log(log, game_no, mean_err, black_wins, white_wins, draws, opt.learning_rate) {
                    eprintln!("failed to write log {}: {e}", log.display());
                    return ExitCode::FAILURE;
                }
            }

            if let Err(e) = evaluator.save_weights(&args.weights_path) {
                eprintln!("failed to save {}: {e}", args.weights_path.display());
                return ExitCode::FAILURE;
            }

            // Window rollover + optional lr decay per window
            window_err = 0.0;
            window_games = 0;
            opt.learning_rate *= args.decay;

            if interrupted.load(Ordering::SeqCst) {
                println!(
                    "stopped after {game_no} games; weights saved to {}",
                    args.weights_path.display()
                );
                return ExitCode::SUCCESS;
            }
        }
    }

    println!("done: weights saved to {}", args.weights_path.display());
    ExitCode::SUCCESS
}

/// Minimal SIGINT handler installation without external crates.
fn ctrlc_handler<F: FnMut() + Send + 'static>(handler: F) -> std::io::Result<()> {
    use std::sync::Mutex;
    static HANDLER: Mutex<Option<Box<dyn FnMut() + Send>>> = Mutex::new(None);

    extern "C" fn trampoline(_: libc::c_int) {
        if let Ok(mut guard) = HANDLER.try_lock() {
            if let Some(h) = guard.as_mut() {
                h();
            }
        }
    }

    *HANDLER.lock().unwrap() = Some(Box::new(handler));
    // SAFETY: installing a signal handler with a valid extern "C" fn.
    let prev = unsafe { libc::signal(libc::SIGINT, trampoline as libc::sighandler_t) };
    if prev == libc::SIG_ERR {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
