//! Head-to-head match against an external engine.
//!
//! Three opponents are supported, each speaking its own dialect:
//!
//! * `edax`      — Edax's console: `setboard <board>` / `go`, replies "Edax plays XX"
//! * `zebra`     — the engine mode added to Zebra's sources for this driver,
//!                 which answers `setboard`/`go` with "move xx"
//! * `egaroucid` — GTP, which Egaroucid implements natively: the driver
//!                 replays the move list rather than setting a board, since
//!                 GTP has no position command
//!
//! The driver owns the authoritative game state. Our engine moves via the
//! usual Searcher/Solver stack; for Edax's turns the driver sends
//! `setboard <position>` + `go` and parses the `Edax plays XX` reply, so
//! passes and move echoes never need to be synchronized.
//!
//! Openings are random with colors swapped per pair, and results are
//! reported like `arena` (our side = A).
//!
//! Usage:
//!   lab --edax <path-to-edax-binary> [OPTIONS]
//!
//! Options:
//!   --weights <path>     Our weight file (default weights_full.bin)
//!   --patterns <set>     egaroucid | edax | egaroucid-plus (default egaroucid)
//!   --depth <n>          Our midgame depth (default 6)
//!   --solve-empties <n>  Our exact-endgame threshold (default 14)
//!   --edax-level <n>     Opponent level/depth (default 5)
//!   --protocol <p>       edax | zebra | egaroucid (default edax)
//!   --threads <n>        Threads for both sides (default 1). Ours only
//!                        parallelises the exact endgame; Edax parallelises
//!                        its whole search, so this favours Edax.
//!   --games <n>          Total games, rounded up to even (default 200)
//!   --random-plies <n>   Random opening plies (default 6)
//!   --seed <n>           RNG seed (default 7)

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio};

use kuroobi::evaluator::Evaluator;
use kuroobi::nnue::Nnue;
use kuroobi::pattern_index::PatternIndices;
use kuroobi::zobrist;
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::search::Searcher;
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::{Board, Color, Position};

struct Args {
    edax_path: PathBuf,
    weights: PathBuf,
    nnue: Option<PathBuf>,
    patterns: &'static str,
    depth: u8,
    mpc: bool,
    solve_empties: u8,
    edax_level: u32,
    protocol: &'static str,
    threads: usize,
    games: usize,
    random_plies: usize,
    seed: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        edax_path: PathBuf::new(),
        weights: PathBuf::from("weights_full.bin"),
        nnue: None,
        patterns: "egaroucid",
        depth: 6,
        mpc: false,
        solve_empties: 14,
        edax_level: 5,
        protocol: "edax",
        threads: 1,
        games: 200,
        random_plies: 6,
        seed: 7,
    };
    let mut have_edax = false;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} requires a value"));
        match arg.as_str() {
            "--edax" => {
                args.edax_path = PathBuf::from(value("--edax")?);
                have_edax = true;
            }
            "--weights" => args.weights = PathBuf::from(value("--weights")?),
            "--nnue" => args.nnue = Some(PathBuf::from(value("--nnue")?)),
            "--patterns" => {
                args.patterns = match value("--patterns")?.as_str() {
                    "egaroucid" => "egaroucid",
                    "egaroucid-plus" => "egaroucid-plus",
                    "edax" => "edax",
                    other => return Err(format!("unknown pattern set: {other}")),
                }
            }
            "--depth" => args.depth = value("--depth")?.parse().map_err(|e| format!("--depth: {e}"))?,
            "--mpc" => args.mpc = true,
            "--solve-empties" => args.solve_empties = value("--solve-empties")?.parse().map_err(|e| format!("--solve-empties: {e}"))?,
            "--edax-level" => args.edax_level = value("--edax-level")?.parse().map_err(|e| format!("--edax-level: {e}"))?,
            "--protocol" => {
                args.protocol = match value("--protocol")?.as_str() {
                    "edax" => "edax",
                    "zebra" => "zebra",
                    "egaroucid" | "gtp" => "egaroucid",
                    other => return Err(format!("--protocol: unknown {other}")),
                }
            }
            "--threads" => args.threads = value("--threads")?.parse().map_err(|e| format!("--threads: {e}"))?,
            "--games" => args.games = value("--games")?.parse().map_err(|e| format!("--games: {e}"))?,
            "--random-plies" => args.random_plies = value("--random-plies")?.parse().map_err(|e| format!("--random-plies: {e}"))?,
            "--seed" => args.seed = value("--seed")?.parse().map_err(|e| format!("--seed: {e}"))?,
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if !have_edax {
        return Err("--edax <path> is required".into());
    }
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

fn random_opening(rng: &mut Rng, plies: usize) -> (Board, Vec<String>) {
    let mut board = Board::new();
    // The move list is only needed by GTP, which cannot be handed a position.
    let mut moves_played = Vec::new();
    for _ in 0..plies {
        let moves = board.movable();
        if moves == 0 {
            break;
        }
        let pos = nth_move(moves, rng.below(moves.count_ones()));
        let f = (b'a' + pos.index() / 8) as char;
        let r = (b'1' + pos.index() % 8) as char;
        moves_played.push(format!("{f}{r}"));
        board.make_move_bits(pos);
    }
    (board, moves_played)
}

/// A running Edax console process.
struct Edax {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    protocol: &'static str,
    /// GTP has no "set this position" command, so the move list is replayed
    /// from the opening each time the driver asks for a move.
    history: Vec<String>,
}

impl Edax {
    fn spawn(
        path: &PathBuf,
        level: u32,
        threads: usize,
        protocol: &'static str,
    ) -> std::io::Result<Edax> {
        // Run from the binary's directory so data/eval.dat resolves.
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let mut child = Command::new(path)
            .args(match protocol {
                // Zebra's engine mode takes its strength on argv: midgame
                // depth, then the empty counts for exact and win/loss search.
                "zebra" => vec![
                    level.to_string(),
                    level.saturating_sub(2).to_string(),
                    level.to_string(),
                    "22".to_string(),
                ],
                "egaroucid" => vec![
                    "-gtp".to_string(),
                    "-l".to_string(),
                    level.to_string(),
                    "-t".to_string(),
                    threads.to_string(),
                    "-nobook".to_string(),
                    "-q".to_string(),
                ],
                _ => vec![
                    "-book-usage".to_string(),
                    "off".to_string(),
                    "-l".to_string(),
                    level.to_string(),
                    "-n".to_string(),
                    threads.to_string(),
                ],
            })
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Edax { child, stdin, stdout, protocol, history: Vec::new() })
    }

    fn send(&mut self, cmd: &str) -> std::io::Result<()> {
        self.stdin.write_all(cmd.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()
    }

    /// Ask Edax to move on `board`. Returns its move, or None for a pass
    /// (callers never ask when the side to move has no legal move, so a
    /// pass reply is treated as a protocol error by the caller).
    /// GTP has no "set this position" command, so the driver replays the
    /// move list from the start of the game each time it asks for a move.
    fn gtp_move(&mut self, board: &Board) -> std::io::Result<Option<Position>> {
        self.send("clear_board")?;
        self.drain_gtp()?;
        let hist = std::mem::take(&mut self.history);
        for (i, mv) in hist.iter().enumerate() {
            let color = if i % 2 == 0 { "B" } else { "W" };
            self.send(&format!("play {color} {mv}"))?;
            self.drain_gtp()?;
        }
        self.history = hist;
        let color = if board.player() == Color::Black { "B" } else { "W" };
        self.send(&format!("genmove {color}"))?;
        let reply = self.read_gtp()?;
        let mv = reply.trim();
        if mv.eq_ignore_ascii_case("pass") {
            return Ok(None);
        }
        let b = mv.as_bytes();
        if b.len() >= 2 {
            let file = b[0].to_ascii_uppercase().wrapping_sub(b'A');
            let rank = b[1].wrapping_sub(b'1');
            if file < 8 && rank < 8 {
                return Ok(Position::from_file_rank(file, rank));
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unparsable gtp move: {reply}"),
        ))
    }

    /// A GTP reply is `= <text>` (or `? <text>` on error) then a blank line.
    fn read_gtp(&mut self) -> std::io::Result<String> {
        let mut out = String::new();
        let mut saw = false;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "engine exited",
                ));
            }
            let t = line.trim_end();
            if let Some(rest) = t.strip_prefix('=') {
                out = rest.trim().to_string();
                saw = true;
            } else if t.starts_with('?') {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("gtp error: {t}"),
                ));
            } else if t.is_empty() && saw {
                return Ok(out);
            }
        }
    }

    fn drain_gtp(&mut self) -> std::io::Result<()> {
        self.read_gtp().map(|_| ())
    }

    fn best_move(&mut self, board: &Board) -> std::io::Result<Option<Position>> {
        if self.protocol == "egaroucid" {
            return self.gtp_move(board);
        }
        self.send(&format!("setboard {}", board))?;
        self.send("go")?;
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.stdout.read_line(&mut line)?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "edax exited",
                ));
            }
            let found = if self.protocol == "zebra" {
                line.strip_prefix("move ").map(|m| m.trim().to_string())
            } else {
                line.find("plays").map(|i| line[i + 5..].trim().to_string())
            };
            if let Some(mv) = found {
                let mv = mv.as_str();
                if mv.eq_ignore_ascii_case("PS") || mv.eq_ignore_ascii_case("pass") {
                    return Ok(None);
                }
                let bytes = mv.as_bytes();
                if bytes.len() >= 2 {
                    let file = bytes[0].to_ascii_uppercase().wrapping_sub(b'A');
                    let rank = bytes[1].wrapping_sub(b'1');
                    if file < 8 && rank < 8 {
                        return Ok(Position::from_file_rank(file, rank));
                    }
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unparsable edax move: {line}"),
                ));
            }
        }
    }
}

impl Drop for Edax {
    fn drop(&mut self) {
        let _ = self.send("quit");
        let _ = self.child.wait();
    }
}

/// Play one game; returns final score from OUR side's perspective
/// Wall-clock spent thinking, so that a match can be reported at the
/// time each side actually used rather than at nominal settings.
#[derive(Default)]
struct Clock {
    ours: f64,
    edax: f64,
    our_moves: u64,
    edax_moves: u64,
}

/// (empties to the winner).
/// Null-window width for PVS. The evaluator returns continuous disc
/// differences, so a zero-width window can't separate "equal" from "better";
/// a hundredth of a disc is far below any meaningful difference.
const PVS_EPS: f32 = 0.01;

/// From this remaining depth upward, order children by a full NNUE eval;
/// shallower nodes use cheap mobility ordering (see `ordered`).
const EVAL_ORDER_DEPTH: u32 = 3;

/// Static square preference for cheap ordering (file-major): corners best,
/// X-squares worst. Scaled to sit between mobility steps.
const SQUARE_BIAS: [i8; 64] = [
    30, -12, 0, -1, -1, 0, -12, 30,
    -12, -15, -3, -3, -3, -3, -15, -12,
    0, -3, 0, -1, -1, 0, -3, 0,
    -1, -3, -1, -1, -1, -1, -3, -1,
    -1, -3, -1, -1, -1, -1, -3, -1,
    0, -3, 0, -1, -1, 0, -3, 0,
    -12, -15, -3, -3, -3, -3, -15, -12,
    30, -12, 0, -1, -1, 0, -12, 30,
];

/// One transposition-table slot for the NNUE search. `flag`: 0 empty,
/// 1 exact, 2 lower bound, 3 upper bound.
#[derive(Clone, Copy)]
struct TtEntry {
    key: u64,
    value: f32,
    best: u8,
    depth: u8,
    flag: u8,
}

/// Fixed-depth NNUE alpha-beta with a transposition table and NNUE-eval move
/// ordering — the pieces a real engine has, and what keeps the wall-clock
/// competitive (a naive search without them explodes).
struct NnueSearch<'a> {
    nn: &'a Nnue,
    tt: Vec<TtEntry>,
    mask: u64,
    /// Nodes visited, to diagnose ordering quality (effective branching).
    pub nodes: u64,
}

impl<'a> NnueSearch<'a> {
    fn new(nn: &'a Nnue, bits: u32) -> Self {
        NnueSearch {
            nn,
            tt: vec![
                TtEntry { key: 0, value: 0.0, best: 64, depth: 0, flag: 0 };
                1usize << bits
            ],
            mask: (1u64 << bits) - 1,
            nodes: 0,
        }
    }

    fn clear(&mut self) {
        for e in &mut self.tt {
            e.flag = 0;
        }
    }

    /// Best move at `depth`, via iterative deepening: each pass seeds the TT
    /// so the next pass orders by the prior best move — this is what lets the
    /// deep pass skip the expensive per-node eval ordering (only new nodes pay
    /// for it), the way real engines avoid a full 1-ply scan everywhere.
    fn best_move(&mut self, b: &Board, depth: u32) -> Option<Position> {
        if b.movable() == 0 {
            return None;
        }
        let mut acc = self.nn.indices(b.black, b.white);
        for d in 1..=depth {
            self.negamax(b, &mut acc, d, f32::NEG_INFINITY, f32::INFINITY);
        }
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let e = self.tt[(h & self.mask) as usize];
        if e.key == h && e.best < 64 {
            Position::from_index(e.best as u32)
        } else {
            self.ordered(b, &mut acc, 1).first().map(|k| k.0)
        }
    }

    /// Children with their flip masks, best-first.
    ///
    /// A full 1-ply eval per child is by far the most expensive thing this
    /// search does — it is *not* counted as a node, yet it dominates the per
    /// node cost. Keep it for the nodes where ordering quality
    /// actually pays and use cheap mobility elsewhere:
    ///
    /// - deep nodes (`depth >= EVAL_ORDER_DEPTH`): NNUE eval, the strong signal
    /// - shallow nodes: opponent mobility (fewer replies is better) plus a
    ///   static square bias — no eval, no accumulator work at all
    fn ordered(
        &self,
        b: &Board,
        acc: &mut PatternIndices,
        depth: u32,
    ) -> Vec<(Position, u64, f32)> {
        let mover = b.player();
        let mut kids = Vec::with_capacity(b.movable_count() as usize);
        let eval_order = depth >= EVAL_ORDER_DEPTH;
        let mut m = b.movable();
        while m != 0 {
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            m &= m - 1;
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            let key = if eval_order {
                self.nn.ix_apply(acc, pos, flipped, mover);
                let k = -self.nn.eval_from_indices(acc, &nb);
                self.nn.ix_undo(acc, pos, flipped, mover);
                k
            } else {
                // Restricting the opponent is the classic cheap proxy; the
                // square bias breaks ties toward corners and away from X/C.
                -(nb.movable_count() as f32) * 4.0 + SQUARE_BIAS[pos.index() as usize] as f32
            };
            kids.push((pos, flipped, key));
        }
        kids.sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
        kids
    }

    fn negamax(&mut self, b: &Board, acc: &mut PatternIndices, depth: u32, mut alpha: f32, beta: f32) -> f32 {
        self.nodes += 1;
        if b.is_game_over() {
            let p = b.player_bb().count_ones() as i32;
            let o = b.opponent_bb().count_ones() as i32;
            let e = 64 - p - o;
            let diff = if p > o { p - o + e } else if o > p { p - o - e } else { 0 };
            return diff as f32 * 1000.0;
        }
        if depth == 0 {
            return self.nn.eval_from_indices(acc, b);
        }
        if b.movable() == 0 {
            let mut nb = *b;
            nb.pass();
            return -self.negamax(&nb, acc, depth, -beta, -alpha);
        }

        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let slot = (h & self.mask) as usize;
        let mut tt_move = 64u8;
        {
            let e = self.tt[slot];
            if e.key == h && e.flag != 0 {
                tt_move = e.best;
                if e.depth as u32 >= depth {
                    match e.flag {
                        1 => return e.value,
                        2 if e.value >= beta => return e.value,
                        3 if e.value <= alpha => return e.value,
                        _ => {}
                    }
                }
            }
        }

        let orig_alpha = alpha;
        let mover = b.player();
        let mut best = f32::NEG_INFINITY;
        let mut best_move = 64u8;

        // With a TT move (typical once iterative deepening has seeded this
        // node) put it first over a cheap natural order — skip the expensive
        // per-child eval scan. Only genuinely new deep nodes pay for it.
        let mut kids = if tt_move >= 64 && depth >= 2 {
            self.ordered(b, acc, depth)
        } else {
            let mut v = Vec::with_capacity(b.movable_count() as usize);
            let mut m = b.movable();
            while m != 0 {
                let pos = Position::from_index(m.trailing_zeros()).unwrap();
                m &= m - 1;
                let mut nb = *b;
                let flipped = nb.make_move_bits(pos);
                v.push((pos, flipped, 0.0));
            }
            v
        };
        if tt_move < 64 {
            if let Some(i) = kids.iter().position(|k| k.0.index() == tt_move) {
                kids.swap(0, i);
            }
        }

        // PVS / NegaScout: full window for the best-ordered child, a null
        // window for later siblings, re-searched only when one beats alpha.
        // Same value as plain alpha-beta. (Measured: no node reduction here —
        // the eval-based ordering already gets the effective branching to ~3,
        // near the sqrt(b) ideal, so there is nothing left for PVS to prune.
        // Kept because it costs nothing and helps when ordering degrades.)
        let mut first = true;
        for (pos, flipped, _) in kids {
            let mut nb = *b;
            nb.make_move_bits(pos);
            self.nn.ix_apply(acc, pos, flipped, mover);
            let v = if first {
                -self.negamax(&nb, acc, depth - 1, -beta, -alpha)
            } else {
                let probe = -self.negamax(&nb, acc, depth - 1, -(alpha + PVS_EPS), -alpha);
                if probe > alpha && probe < beta {
                    -self.negamax(&nb, acc, depth - 1, -beta, -probe)
                } else {
                    probe
                }
            };
            self.nn.ix_undo(acc, pos, flipped, mover);
            first = false;
            if v > best {
                best = v;
                best_move = pos.index() as u8;
            }
            if best > alpha {
                alpha = best;
            }
            if alpha >= beta {
                break;
            }
        }

        let flag = if best <= orig_alpha { 3 } else if best >= beta { 2 } else { 1 };
        self.tt[slot] = TtEntry { key: h, value: best, best: best_move, depth: depth as u8, flag };
        best
    }
}

#[allow(clippy::too_many_arguments)]
fn play(
    start: &Board,
    we_are_black: bool,
    evaluator: &Evaluator,
    mut nnue: Option<&mut NnueSearch>,
    searcher: &mut Searcher,
    solver: &mut Solver,
    edax: &mut Edax,
    depth: u8,
    solve_empties: u8,
    clock: &mut Clock,
    opening_moves: &[String],
) -> Result<i32, String> {
    let mut board = *start;
    searcher.clear();
    edax.history.clear();
    // The opening was played before this function saw the board, so GTP
    // needs it replayed from the initial position.
    if edax.protocol == "egaroucid" {
        edax.history = opening_moves.to_vec();
    }

    loop {
        if board.movable() == 0 {
            let mut passed = board;
            passed.pass();
            if passed.movable() == 0 {
                break; // game over
            }
            if edax.protocol == "egaroucid" {
                edax.history.push("pass".to_string());
            }
            board = passed;
            continue;
        }

        let our_turn = (board.player() == Color::Black) == we_are_black;
        let t0 = std::time::Instant::now();
        let pos = if our_turn {
            if solve_empties > 0 && board.empty_count() <= solve_empties {
                solver
                    .solve_with_eval(EndSolverMode::Perfect, &board, Some(evaluator))
                    .best_move
                    .ok_or("solver returned no move")?
            } else if let Some(nn) = nnue.as_deref_mut() {
                nn.best_move(&board, depth as u32).ok_or("nnue returned no move")?
            } else {
                searcher
                    .search(&board, evaluator, depth)
                    .best_move
                    .ok_or("searcher returned no move")?
            }
        } else {
            match edax.best_move(&board) {
                Ok(Some(p)) => p,
                Ok(None) => return Err("edax passed although moves exist".into()),
                Err(e) => return Err(format!("edax io error: {e}")),
            }
        };

        let dt = t0.elapsed().as_secs_f64();
        if our_turn {
            clock.ours += dt;
            clock.our_moves += 1;
        } else {
            clock.edax += dt;
            clock.edax_moves += 1;
        }

        if board.movable() & pos.to_bit() == 0 {
            return Err(format!("illegal move {pos:?} (our_turn={our_turn})"));
        }
        // GTP is replayed from the move list, so every move must be recorded
        // — including passes, which GTP spells out.
        if edax.protocol == "egaroucid" {
            let f = (b'a' + pos.index() / 8) as char;
            let r = (b'1' + pos.index() % 8) as char;
            edax.history.push(format!("{f}{r}"));
        }
        board.make_move_bits(pos);
    }

    let diff = board.black_count() as i32 - board.white_count() as i32;
    let empties = board.empty_count() as i32;
    let score_black = match diff.cmp(&0) {
        std::cmp::Ordering::Greater => diff + empties,
        std::cmp::Ordering::Less => diff - empties,
        std::cmp::Ordering::Equal => 0,
    };
    Ok(if we_are_black { score_black } else { -score_black })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let patterns = match args.patterns {
        "edax" => EDAX_PATTERNS,
        "egaroucid-plus" => EGAROUCID_PLUS_PATTERNS,
        _ => EGAROUCID_PATTERNS,
    };
    let mut evaluator = Evaluator::new(patterns);
    if let Err(e) = evaluator.load_weights(&args.weights) {
        eprintln!("failed to load {}: {e}", args.weights.display());
        return ExitCode::FAILURE;
    }

    // Optional NNUE evaluator for our midgame moves (endgame stays exact).
    let nnue = match &args.nnue {
        Some(p) => {
            let mut nn = Nnue::new(patterns);
            if let Err(e) = nn.load(p) {
                eprintln!("failed to load nnue {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
            nn.quantize();
            Some(nn)
        }
        None => None,
    };
    // The search borrows the model; TT sized 2^20 (~24 MB), cleared per game.
    let mut nnue_search = nnue.as_ref().map(|nn| NnueSearch::new(nn, 20));

    let mut edax = match Edax::spawn(&args.edax_path, args.edax_level, args.threads, args.protocol) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to start edax: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "lab: us={} (depth {}, solve {}) vs Edax level {}  ({} games, {} random plies, {} threads)",
        args.weights.display(),
        args.depth,
        args.solve_empties,
        args.edax_level,
        args.games,
        args.random_plies,
        args.threads
    );

    let mut rng = Rng::new(args.seed);
    let mut searcher = Searcher::new(17);
    searcher.mpc = args.mpc;
    searcher.threads = args.threads;
    // Deep endgame thresholds (20+ empties) need a much larger table.
    let mut solver = Solver::new(if args.solve_empties >= 18 { 22 } else { 18 });
    solver.set_threads(args.threads);
    let mut clock = Clock::default();
    let mut wins = 0usize;
    let mut losses = 0usize;
    let mut draws = 0usize;
    let mut disc_sum = 0i64;

    for pair in 0..args.games / 2 {
        let (opening, opening_moves) = random_opening(&mut rng, args.random_plies);
        for we_are_black in [true, false] {
            if let Some(s) = &mut nnue_search {
                s.clear();
            }
            match play(
                &opening, we_are_black, &evaluator, nnue_search.as_mut(), &mut searcher, &mut solver,
                &mut edax, args.depth, args.solve_empties, &mut clock, &opening_moves,
            ) {
                Ok(s) => {
                    disc_sum += s as i64;
                    match s.cmp(&0) {
                        std::cmp::Ordering::Greater => wins += 1,
                        std::cmp::Ordering::Less => losses += 1,
                        std::cmp::Ordering::Equal => draws += 1,
                    }
                }
                Err(e) => {
                    eprintln!("game error (pair {pair}): {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    let n = (wins + losses + draws) as f64;
    let p = (wins as f64 + draws as f64 / 2.0) / n;
    let se = (p * (1.0 - p) / n).sqrt();
    println!("we win {wins}, edax wins {losses}, draws {draws}");
    println!(
        "our score {:.1}%  (95% CI {:.1}%..{:.1}%)  mean disc diff {:+.2}",
        p * 100.0,
        (p - 1.96 * se) * 100.0,
        (p + 1.96 * se) * 100.0,
        disc_sum as f64 / n
    );
    println!(
        "thinking time: ours {:.1}s / {} moves = {:.3}s per move; \
         edax {:.1}s / {} moves = {:.3}s per move  (ratio {:.2}x)",
        clock.ours,
        clock.our_moves,
        clock.ours / clock.our_moves.max(1) as f64,
        clock.edax,
        clock.edax_moves,
        clock.edax / clock.edax_moves.max(1) as f64,
        (clock.ours / clock.our_moves.max(1) as f64)
            / (clock.edax / clock.edax_moves.max(1) as f64).max(1e-9)
    );
    if let Some(s) = &nnue_search {
        println!(
            "nnue search: {} nodes over {} of our moves = {:.0} nodes/move",
            s.nodes,
            clock.our_moves,
            s.nodes as f64 / clock.our_moves.max(1) as f64
        );
    }
    ExitCode::SUCCESS
}
