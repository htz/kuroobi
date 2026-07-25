//! Fixed-depth head-to-head: NNUE evaluator vs the linear champion.
//!
//! Both sides run the *same* plain alpha-beta at a fixed depth, so search
//! effort is equal and only the evaluator differs — a pure eval-strength test
//! (speed is irrelevant here; the NNUE forward is recomputed each node). Each
//! random opening is played twice with colours swapped to cancel first-mover
//! bias. A's win rate is reported with a 95% CI, like `arena`.

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::evaluator::Evaluator;
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::{Board, Color, Position};

/// An evaluator: score a board from the side-to-move's perspective.
enum Eval {
    Linear(Evaluator),
    Nn(Nnue),
}
impl Eval {
    fn score(&self, b: &Board) -> f32 {
        match self {
            Eval::Linear(e) => e.eval(b),
            Eval::Nn(n) => n.eval(b),
        }
    }
}

/// Terminal disc difference from the side-to-move's perspective.
fn terminal_value(b: &Board) -> f32 {
    let p = (b.player_bb()).count_ones() as i32;
    let o = (b.opponent_bb()).count_ones() as i32;
    let empties = 64 - p - o;
    // Empties awarded to the leader (game-theoretic convention).
    let diff = if p > o {
        p - o + empties
    } else if o > p {
        p - o - empties
    } else {
        0
    };
    diff as f32
}

/// Negamax + alpha-beta at fixed `depth`, eval at the horizon.
fn negamax(b: &Board, e: &Eval, depth: u32, mut alpha: f32, beta: f32) -> f32 {
    if b.is_game_over() {
        return terminal_value(b) * 1000.0; // dominate eval scores
    }
    if depth == 0 {
        return e.score(b);
    }
    let moves = b.movable();
    if moves == 0 {
        let mut nb = *b;
        nb.pass();
        return -negamax(&nb, e, depth, -beta, -alpha);
    }
    let mut best = f32::NEG_INFINITY;
    let mut m = moves;
    while m != 0 {
        let sq = m.trailing_zeros();
        m &= m - 1;
        let pos = Position::from_index(sq).unwrap();
        let mut nb = *b;
        nb.make_move_unchecked(pos);
        let v = -negamax(&nb, e, depth - 1, -beta, -alpha);
        if v > best {
            best = v;
        }
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            break;
        }
    }
    best
}

/// Best root move for `e` at `depth` (None if no legal move).
fn best_move(b: &Board, e: &Eval, depth: u32) -> Option<Position> {
    let moves = b.movable();
    if moves == 0 {
        return None;
    }
    let mut best = f32::NEG_INFINITY;
    let mut best_pos = None;
    let mut m = moves;
    while m != 0 {
        let sq = m.trailing_zeros();
        m &= m - 1;
        let pos = Position::from_index(sq).unwrap();
        let mut nb = *b;
        nb.make_move_unchecked(pos);
        let v = -negamax(&nb, e, depth - 1, f32::NEG_INFINITY, f32::INFINITY);
        if v > best {
            best = v;
            best_pos = Some(pos);
        }
    }
    best_pos
}

/// Play one game from `start`; A plays Black if `a_is_black`. Returns the
/// final disc difference from A's perspective.
fn play(start: &Board, a: &Eval, b_side: &Eval, depth: u32, a_is_black: bool) -> i32 {
    let mut board = *start;
    loop {
        if board.is_game_over() {
            break;
        }
        let a_to_move = (board.player() == Color::Black) == a_is_black;
        let e = if a_to_move { a } else { b_side };
        match best_move(&board, e, depth) {
            Some(pos) => board.make_move_unchecked(pos),
            None => board.pass(),
        }
    }
    let black = board.black.count_ones() as i32;
    let white = board.white.count_ones() as i32;
    let black_diff = black - white;
    if a_is_black {
        black_diff
    } else {
        -black_diff
    }
}

/// Deterministic xorshift for reproducible random openings.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn random_opening(rng: &mut Rng, plies: usize) -> Board {
    let mut b = Board::new();
    for _ in 0..plies {
        if b.is_game_over() {
            break;
        }
        let moves = b.movable();
        if moves == 0 {
            b.pass();
            continue;
        }
        let n = moves.count_ones();
        let pick = (rng.next() % n as u64) as u32;
        let mut m = moves;
        for _ in 0..pick {
            m &= m - 1;
        }
        let pos = Position::from_index(m.trailing_zeros()).unwrap();
        b.make_move_unchecked(pos);
    }
    b
}

fn main() -> ExitCode {
    let mut depth = 4u32;
    let mut games = 400usize;
    let mut plies = 6usize;
    let mut seed = 7u64;
    let mut a_path = PathBuf::from("nnue.bin"); // A = NNUE
    let mut b_path = PathBuf::from("weights_full.bin"); // B = linear
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--depth" => depth = it.next().unwrap().parse().unwrap(),
            "--games" => games = it.next().unwrap().parse().unwrap(),
            "--random-plies" => plies = it.next().unwrap().parse().unwrap(),
            "--seed" => seed = it.next().unwrap().parse().unwrap(),
            "--nnue" => a_path = PathBuf::from(it.next().unwrap()),
            "--linear" => b_path = PathBuf::from(it.next().unwrap()),
            other => {
                eprintln!("unknown option {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    if let Err(e) = nn.load(&a_path) {
        eprintln!("failed to load nnue {}: {e}", a_path.display());
        return ExitCode::FAILURE;
    }
    let mut lin = Evaluator::new(EGAROUCID_PATTERNS);
    if let Err(e) = lin.load_weights(&b_path) {
        eprintln!("failed to load linear {}: {e}", b_path.display());
        return ExitCode::FAILURE;
    }
    let a = Eval::Nn(nn);
    let b = Eval::Linear(lin);

    println!(
        "nnue_arena: A={} (NNUE) vs B={} (linear), depth {depth}, {games} games, {plies} random plies",
        a_path.display(),
        b_path.display()
    );

    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    let pairs = games / 2;
    let (mut a_wins, mut b_wins, mut draws) = (0i32, 0i32, 0i32);
    let mut a_disc_sum = 0i64;
    for _ in 0..pairs {
        let start = random_opening(&mut rng, plies);
        for &a_is_black in &[true, false] {
            let d = play(&start, &a, &b, depth, a_is_black);
            a_disc_sum += d as i64;
            match d.cmp(&0) {
                std::cmp::Ordering::Greater => a_wins += 1,
                std::cmp::Ordering::Less => b_wins += 1,
                std::cmp::Ordering::Equal => draws += 1,
            }
        }
    }
    let n = (a_wins + b_wins + draws) as f64;
    let score = (a_wins as f64 + 0.5 * draws as f64) / n;
    let se = (score * (1.0 - score) / n).sqrt();
    let (lo, hi) = (score - 1.96 * se, score + 1.96 * se);
    println!(
        "A wins {a_wins}, B wins {b_wins}, draws {draws}",
    );
    println!(
        "A score {:.1}%  (95% CI {:.1}%..{:.1}%)  mean disc diff {:+.2}",
        score * 100.0,
        lo * 100.0,
        hi * 100.0,
        a_disc_sum as f64 / n
    );
    println!(
        "=> {}",
        if lo > 0.5 {
            "A (NNUE) is significantly stronger"
        } else if hi < 0.5 {
            "B (linear) is significantly stronger"
        } else {
            "no significant difference"
        }
    );
    ExitCode::SUCCESS
}
