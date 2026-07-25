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
    /// Check that the parallel search returns the same move as the sequential
    /// one, instead of inferring it from win rates (far too noisy to see a
    /// small regression).
    verify_parallel: bool,
    /// Net for side B in `--self-vs`. Different net, identical search: the
    /// direct answer to "is the new model stronger", which validation MSE only
    /// predicts.
    nnue_b: Option<PathBuf>,
    /// Play our own engine against itself with a different thread count. The
    /// only sensitive way to ask "does parallelism cost strength?" once exact
    /// equivalence has been given up: measuring it through a third engine
    /// wastes most of the signal, and at 20 games the band is +-20%.
    self_vs: Option<usize>,
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
        verify_parallel: false,
        self_vs: None,
        nnue_b: None,
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
            "--verify-parallel" => args.verify_parallel = true,
            "--self-vs" => args.self_vs = Some(value("--self-vs")?.parse().map_err(|e| format!("--self-vs: {e}"))?),
            "--nnue-b" => args.nnue_b = Some(PathBuf::from(value("--nnue-b")?)),
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
    /// Nodes our midgame search visited, so parallel *throughput* can be told
    /// apart from parallel *speedup*: if nodes per second scale with the core
    /// count but wall-clock does not, the threads are being fed and the extra
    /// work is simply wasted; if nps saturates too, the cores are starved and
    /// no scheduling change can help.
    our_nodes: u64,
}

/// (empties to the winner).
/// Null-window width for PVS. The evaluator returns continuous disc
/// differences, so a zero-width window can't separate "equal" from "better";
/// a hundredth of a disc is far below any meaningful difference.
const PVS_EPS: f32 = 0.01;

/// One ordered child: move, its flip mask (reused so the search never
/// recomputes the flips), and the ordering key.
type Kid = (Position, u64, f32);

/// Upper bound on legal moves in a Reversi position.
const MAX_KIDS: usize = 34;

/// From this remaining depth upward, order children by a full NNUE eval;
/// shallower nodes use cheap mobility ordering (see `ordered`).
const EVAL_ORDER_DEPTH: u32 = 3;

/// A worker whose subtree became irrelevant returns this instead of a score,
/// so the caller knows to discard it rather than treat it as an evaluation.
const ABORTED: f32 = f32::NEG_INFINITY;

/// Nodes between abort checks: the flag is shared, so reading it every node
/// would put a contended load in the hot path.
/// Below `ybwc_min_depth` a subtree is cheaper to
/// search than to hand off.
/// Give each pool worker its own transposition table instead of sharing one.
/// Diagnostic: independent processes reach 5.4x aggregate throughput where six
/// threads reach 2.2x, and the shared table is the largest thing threads have
/// that processes do not.
fn use_private_tt() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("PRIVATE_TT").is_ok_and(|v| v != "0"))
}

thread_local! {
    static WORKER_TT: std::cell::Cell<Option<&'static SharedTt>> =
        const { std::cell::Cell::new(None) };
}

/// This thread's private table, built on first use.
fn private_tt() -> Option<&'static SharedTt> {
    if !use_private_tt() {
        return None;
    }
    WORKER_TT.with(|c| {
        if c.get().is_none() {
            c.set(Some(Box::leak(Box::new(SharedTt::new(22)))));
        }
        c.get()
    })
}

/// How many children are searched sequentially before the rest are handed out.
/// One (the eldest brother) is the classic choice. Searching a second one
/// first costs latency but avoids giving away a whole fan of subtrees that
/// the second child's cutoff was about to make pointless.
fn ybwc_elder() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        // Measured (6 threads, 60 self-play games): 1 -> 1.30x, 2 -> 1.66x,
        // 3 -> 1.63x, all at 50% score. A cheaper per-node evaluation would
        // favour one; our expensive leaves make two the better trade.
        std::env::var("YBWC_ELDER").ok().and_then(|v| v.parse().ok()).unwrap_or(2)
    })
}

/// Queue depth per worker.
fn pool_capacity_factor() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("POOL_CAP").ok().and_then(|v| v.parse().ok()).unwrap_or(4)
    })
}

/// How much each step of `mpc_relax` widens the ProbCut margin.
const MPC_RELAX_STEP: f32 = 1.18;

/// Minimum remaining depth for a YBWC split. Overridable so the split can be
/// switched off (set it above the search depth) when isolating which half of
/// the parallel scheme costs strength.
fn ybwc_min_depth() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("YBWC_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(6)
    })
}
const ABORT_CHECK_INTERVAL: u64 = 512;

/// Multi-ProbCut: from this remaining depth up, a shallow search decides
/// whether the node lies far enough outside the window to skip entirely.
/// Strong engines prune this way at deep settings, so a fair deep comparison
/// needs it on both sides.
const MPC_MIN_DEPTH: u32 = 4;

/// Confidence in standard deviations of the shallow search's prediction error.
/// The same knob is often expressed as a selectivity percentage (74% ~ 1.13σ).
const MPC_T: f32 = 1.1;

/// How deeply ProbCut may nest. A single level leaves the deep tree barely
/// pruned; letting the probe itself prune is what makes MPC "multi".
const MPC_MAX_LEVEL: u32 = 3;

/// Probe depth for a node at `depth`, mirroring the linear searcher's rule
/// (keeps the parity of `depth`, which matters because odd and even plies
/// evaluate from opposite sides).
fn mpc_reduced_depth(depth: u32) -> u32 {
    2 * (depth / 4) + (depth & 1)
}

/// Standard deviation of the error between a `pc_depth` search and a `depth`
/// search at `empties` empties. Fitted from measurements for this pattern
/// evaluator (see `search::mpc_sigma`); the NNUE output is in the same
/// disc-difference units, and it is *more* accurate, so this is a safe
/// (slightly conservative) model to prune against.
fn mpc_sigma(empties: u32, depth: u32, pc_depth: u32) -> f32 {
    const A: f32 = -0.068941;
    const B: f32 = 0.368775;
    const C: f32 = -0.713476;
    const QA: f32 = 0.010223;
    const QB: f32 = 0.647219;
    const QC: f32 = 4.050545;
    let s = A * empties as f32 + B * depth as f32 + C * pc_depth as f32;
    QA * s * s + QB * s + QC
}

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

/// A transposition table shared by the root-split workers.
///
/// Writes race, but every entry carries the position key and is compared
/// exactly, so a torn read can only produce a *mismatch* (recomputed), never a
/// wrong value for another position. This is the same argument the linear
/// searcher's shared table relies on.
struct SharedTt {
    entries: std::cell::UnsafeCell<Vec<TtEntry>>,
    mask: u64,
}
// SAFETY: see the note above — entries are self-validating.
unsafe impl Sync for SharedTt {}

impl SharedTt {
    fn new(bits: u32) -> SharedTt {
        SharedTt {
            entries: std::cell::UnsafeCell::new(vec![
                TtEntry { key: 0, value: 0.0, best: 64, depth: 0, flag: 0, relax: 0 };
                1usize << bits
            ]),
            mask: (1u64 << bits) - 1,
        }
    }

    #[inline]
    fn get(&self, hash: u64) -> TtEntry {
        // SAFETY: index is masked into range; a racing write can only make the
        // key mismatch, which the caller treats as a miss.
        unsafe {
            let v = &*self.entries.get();
            *v.get_unchecked((hash & self.mask) as usize)
        }
    }

    #[inline]
    fn put(&self, hash: u64, e: TtEntry) {
        // SAFETY: as above.
        unsafe {
            let v = &mut *self.entries.get();
            *v.get_unchecked_mut((hash & self.mask) as usize) = e;
        }
    }

    fn clear(&self) {
        // SAFETY: called between games, with no workers running.
        unsafe {
            for e in (*self.entries.get()).iter_mut() {
                e.flag = 0;
            }
        }
    }
}

/// One transposition-table slot for the NNUE search. `flag`: 0 empty,
/// 1 exact, 2 lower bound, 3 upper bound.
#[derive(Clone, Copy)]
struct TtEntry {
    key: u64,
    value: f32,
    best: u8,
    depth: u8,
    flag: u8,
    /// How *accurately* this entry was searched: 0 is the main thread's
    /// selectivity, higher means a wider ProbCut margin and therefore fewer
    /// cuts. The table refuses
    /// to hand out bounds unless `(depth, relax)` both meet what the asking
    /// node needs. Without that gate a helper's aggressive cut silently becomes
    /// the main search's answer — which reads as a speedup (fewer nodes) but is
    /// really a strength loss.
    relax: u8,
}

/// How often a split was attempted and how often the pool accepted it. A large
/// gap means the workers are saturated; a small `SPLIT_TRIED` means the search
/// is not offering them work in the first place.
static SPLIT_TRIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nodes searched inside pool tasks. Against the total this is the share of
/// the tree the workers actually carried: if it is small they are idle, not
/// slow, and no amount of queue tuning will help.
static TASK_NODES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nanoseconds pool workers spent inside tasks. Against (workers x our search
/// time) this is worker utilisation: it separates "the workers are idle" from
/// "the workers are busy but contending".
static WORKER_BUSY_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static POOL_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SPLIT_DONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Where a split-off null-window search leaves its answer.
struct Slot {
    done: std::sync::atomic::AtomicBool,
    bits: std::sync::atomic::AtomicU32,
    nodes: std::sync::atomic::AtomicU64,
}

impl Slot {
    fn new() -> Slot {
        use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
        Slot { done: AtomicBool::new(false), bits: AtomicU32::new(0), nodes: AtomicU64::new(0) }
    }
    fn set(&self, v: f32, nodes: u64) {
        use std::sync::atomic::Ordering;
        self.bits.store(v.to_bits(), Ordering::Relaxed);
        self.nodes.store(nodes, Ordering::Relaxed);
        self.done.store(true, Ordering::Release);
    }
}

/// Task pool shared by both forms of parallelism — one pool serves both the
/// Lazy SMP helpers and the YBWC splits.
///
/// The lifetime is the scope the workers were spawned in, so tasks may borrow
/// the net and the table instead of forcing everything to be `'static`.
struct Pool {
    q: std::sync::Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send + 'static>>>,
    cv: std::sync::Condvar,
    /// Tasks queued or running. Never allowed past `workers`, so a thread
    /// waiting on a task can always be sure someone is able to run it.
    outstanding: std::sync::atomic::AtomicUsize,
    /// Queue length, readable without taking the lock. A thread waiting on a
    /// split polls for work; if that poll locks the mutex every iteration it
    /// serialises every worker against the waiters and throughput collapses.
    queued: std::sync::atomic::AtomicUsize,
    workers: usize,
    /// Cap on queued-or-running tasks. Above `workers` so nodes deeper than the
    /// first few can still split once the top of the tree is busy.
    capacity: usize,
    stop: std::sync::atomic::AtomicBool,
}

impl Pool {
    fn new(workers: usize) -> Pool {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        Pool {
            q: std::sync::Mutex::new(std::collections::VecDeque::new()),
            cv: std::sync::Condvar::new(),
            outstanding: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            workers,
            capacity: workers * pool_capacity_factor(),
            stop: AtomicBool::new(false),
        }
    }

    /// Queue a task if a worker can take it. Returns false when the pool is
    /// saturated — the caller then does the work itself, which is what keeps
    /// the split decision cheap.
    fn try_push(&self, f: impl FnOnce() + Send + 'static) -> bool {
        use std::sync::atomic::Ordering;
        if self.workers == 0 {
            return false;
        }
        SPLIT_TRIED.fetch_add(1, Ordering::Relaxed);
        // Claim a slot first; a lost race just means one fewer split.
        if self
            .outstanding
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < self.capacity).then_some(n + 1)
            })
            .is_err()
        {
            return false;
        }
        SPLIT_DONE.fetch_add(1, Ordering::Relaxed);
        self.q.lock().unwrap().push_back(Box::new(f));
        self.queued.fetch_add(1, Ordering::Relaxed);
        self.cv.notify_one();
        true
    }

    /// Run one queued task if there is one. Used both by the workers and by a
    /// thread waiting on a split, so waiting never wastes a core.
    fn run_one(&self) -> bool {
        use std::sync::atomic::Ordering;
        // Cheap reject first: taking the lock just to find the queue empty is
        // what makes polling expensive for everyone else.
        if self.queued.load(Ordering::Relaxed) == 0 {
            return false;
        }
        let task = {
            let mut q = self.q.lock().unwrap();
            let t = q.pop_front();
            if t.is_some() {
                self.queued.fetch_sub(1, Ordering::Relaxed);
            }
            t
        };
        match task {
            Some(t) => {
                t();
                self.outstanding.fetch_sub(1, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Block until `slot` is filled, running other queued tasks in the
    /// meantime rather than idling.
    fn wait(&self, slot: &Slot) {
        use std::sync::atomic::Ordering;
        let mut idle = 0u32;
        while !slot.done.load(Ordering::Acquire) {
            if self.run_one() {
                idle = 0;
                continue;
            }
            // Nothing to steal: spin briefly (the task is usually about to
            // finish), then hand the core back rather than burn it.
            idle += 1;
            if idle < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
    }

    fn worker_loop(&self) {
        use std::sync::atomic::Ordering;
        loop {
            let task = {
                let mut q = self.q.lock().unwrap();
                loop {
                    if let Some(t) = q.pop_front() {
                        self.queued.fetch_sub(1, Ordering::Relaxed);
                        break Some(t);
                    }
                    if self.stop.load(Ordering::Relaxed) {
                        break None;
                    }
                    let (g, _) = self
                        .cv
                        .wait_timeout(q, std::time::Duration::from_micros(200))
                        .unwrap();
                    q = g;
                }
            };
            match task {
                Some(t) => {
                    let t0 = std::time::Instant::now();
                    t();
                    WORKER_BUSY_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    self.outstanding.fetch_sub(1, Ordering::Relaxed);
                }
                None => return,
            }
        }
    }

    fn shutdown(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.cv.notify_all();
    }
}

/// Fixed-depth NNUE alpha-beta with a transposition table and NNUE-eval move
/// ordering — the pieces a real engine has, and what keeps the wall-clock
/// competitive (a naive search without them explodes).
struct NnueSearch {
    nn: &'static Nnue,
    tt: &'static SharedTt,
    /// Workers for the root split (1 = sequential).
    pub threads: usize,
    /// Nodes visited, to diagnose ordering quality (effective branching).
    pub nodes: u64,
    /// Enable ProbCut (selective pruning).
    pub mpc: bool,
    /// How many ProbCut probes are nested above this node.
    probcut_level: u32,
    /// Set by the root when a sibling's result makes this worker's subtree
    /// irrelevant; checked periodically so the worker can stop immediately
    /// instead of finishing work nobody will use.
    abort: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    abort_countdown: u64,
    /// Set once the main Lazy SMP thread has reached the target depth; helpers
    /// stop as soon as they notice, since their results are no longer needed.
    /// Shared iteration counter, and the iteration this worker is serving.
    /// Helpers are spawned once per move and loop over iterations themselves,
    /// so the main thread never has to join them mid-search; it just bumps the
    /// counter and a helper notices its pass is stale on the next check.
    done: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    my_gen: u32,
    /// Pool used to split null-window child searches (YBWC). Workers split
    /// their own subtrees too — without that a worker handed a large subtree
    /// becomes the critical path and the speedup collapses to the size of the
    /// largest single task. It is deadlock-free because a thread waiting on a
    /// split runs queued tasks while it waits, so no task can be stranded
    /// behind the thread that needs it.
    pool: Option<&'static Pool>,
    /// Extra ProbCut aggressiveness for Lazy SMP helpers: shrinks the margin so
    /// two helpers at the same depth explore differently instead of re-deriving
    /// the same entries.
    mpc_relax: u32,
}

impl NnueSearch {
    fn new(nn: &'static Nnue, tt: &'static SharedTt) -> Self {
        NnueSearch {
            nn,
            tt,
            threads: 1,
            nodes: 0,
            mpc: false,
            probcut_level: 0,
            abort: None,
            abort_countdown: ABORT_CHECK_INTERVAL,
            done: None,
            my_gen: 0,
            pool: None,
            mpc_relax: 0,
        }
    }

    /// A worker sharing this search's model and table, with its own counters.
    /// The process-wide task pool, built on first use. A
    /// single global pool is the point: workers that live
    /// across moves cost nothing per search, and both Lazy SMP and YBWC draw
    /// from the same set of cores instead of oversubscribing them.
    fn shared_pool(&self, workers: usize) -> Option<&'static Pool> {
        if workers == 0 {
            // Checked before the cell, not inside it: a sequential searcher
            // must not inherit a pool that a parallel one happened to build.
            return None;
        }
        static POOL: std::sync::OnceLock<Option<&'static Pool>> = std::sync::OnceLock::new();
        *POOL.get_or_init(|| {
            POOL_WORKERS.store(workers, std::sync::atomic::Ordering::Relaxed);
            let pool: &'static Pool = Box::leak(Box::new(Pool::new(workers)));
            for _ in 0..workers {
                std::thread::spawn(move || pool.worker_loop());
            }
            Some(pool)
        })
    }

    fn worker(&self) -> NnueSearch {
        NnueSearch {
            nn: self.nn,
            tt: self.tt,
            threads: 1,
            nodes: 0,
            mpc: self.mpc,
            probcut_level: 0,
            abort: self.abort.clone(),
            abort_countdown: ABORT_CHECK_INTERVAL,
            done: self.done.clone(),
            my_gen: self.my_gen,
            pool: self.pool,
            mpc_relax: self.mpc_relax,
        }
    }

    /// Whether a Lazy SMP helper should stop looping.
    #[inline]
    fn stopped(&self) -> bool {
        self.done
            .as_ref()
            .is_some_and(|g| g.load(std::sync::atomic::Ordering::Relaxed) != self.my_gen)
    }

    /// Whether this worker has been told to stop. Checked once every
    /// `ABORT_CHECK_INTERVAL` nodes.
    #[inline]
    fn should_stop(&mut self) -> bool {
        let Some(flag) = &self.abort else { return false };
        self.abort_countdown -= 1;
        if self.abort_countdown != 0 {
            return false;
        }
        self.abort_countdown = ABORT_CHECK_INTERVAL;
        flag.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Periodic check that also honours the Lazy SMP done flag, so helpers
    /// unwind promptly instead of finishing a deep pass nobody will read.
    #[inline]
    fn should_stop_or_done(&mut self) -> bool {
        if self.should_stop() {
            return true;
        }
        self.done.is_some() && self.stopped()
    }

    fn clear(&mut self) {
        self.tt.clear();
    }

    /// Best move at `depth`, via iterative deepening: each pass seeds the TT
    /// so the next pass orders by the prior best move — this is what lets the
    /// deep pass skip the expensive per-node eval ordering (only new nodes pay
    /// for it), the way real engines avoid a full 1-ply scan everywhere.
    fn best_move(&mut self, b: &Board, depth: u32) -> Option<Position> {
        self.best_move_valued(b, depth).0
    }

    /// Best move *and* the root value, so a caller can check that a parallel
    /// search reproduces the sequential one exactly — the move alone can
    /// coincide while the value diverges.
    fn best_move_valued(&mut self, b: &Board, depth: u32) -> (Option<Position>, f32) {
        if b.movable() == 0 {
            return (None, f32::NAN);
        }
        if self.threads > 1 && depth >= 2 {
            // Lazy SMP runs its own iterative deepening on every worker; doing
            // the shallow passes here as well would just duplicate them.
            return self.lazy_smp(b, depth);
        }
        let mut acc = self.nn.indices(b.black, b.white);
        // Iterative deepening: each pass seeds the table so the next orders by
        // the prior best move.
        let mut value = f32::NAN;
        for d in 1..=depth {
            value = self.negamax(b, &mut acc, d, f32::NEG_INFINITY, f32::INFINITY);
        }
        (self.root_best(b, &mut acc), value)
    }

    /// The move the table records for `b`, falling back to a 1-ply ordering.
    fn root_best(&self, b: &Board, acc: &mut PatternIndices) -> Option<Position> {
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let e = self.tt.get(h);
        if e.key == h && e.best < 64 {
            return Position::from_index(e.best as u32);
        }
        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0.0); MAX_KIDS];
        let n = self.ordered_into(b, acc, 1, b.movable(), &mut kids);
        (n > 0).then(|| kids[0].0)
    }

    /// Lazy SMP.
    ///
    /// The main thread owns the iterative deepening. **Helpers are launched and
    /// joined inside each iteration**, not once for the whole search: their job
    /// is to fill the table for the iteration the main thread is about to run,
    /// and a helper still running a stale iteration is wasted work.
    ///
    /// Helpers diverge along two axes:
    ///
    /// - **depth** `main_depth + ctz(idx + 1)`: helper 0 shares the main depth,
    ///   1 goes one deeper, 2 shares again, 3 goes two deeper... so the search
    ///   effort stays concentrated near the current iteration while a few
    ///   threads scout ahead. A flat 0..2 spread (what this used to do) puts
    ///   too much work on plies the main thread will not reach this iteration.
    /// - **selectivity**: when several helpers land on the same depth, each
    ///   subsequent one prunes harder (`sub_mpc_level` increments). Two threads
    ///   searching the same depth with the same selectivity just re-derive the
    ///   same entries.
    ///
    /// Helpers are also only worth launching while the iteration is cheap
    /// (see `smp_max_depth`).
    fn lazy_smp(&mut self, b: &Board, depth: u32) -> (Option<Position>, f32) {
        use std::sync::atomic::{AtomicU64, Ordering};
        // Tuning knobs read once from the environment, so a sweep does not need
        // one build per point (thermal drift makes serial rebuild-and-measure
        // unreliable — see the measurement protocol).
        fn env_u32(key: &'static str, default: u32) -> u32 {
            std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        fn smp_min_depth() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_MIN_DEPTH", 1))
        }
        fn smp_spread() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_SPREAD", 2))
        }
        fn smp_sharpen_max() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_SHARPEN_MAX", 2))
        }

        /// Above this iteration Lazy SMP is switched off and the whole pool
        /// goes into YBWC instead.
        ///
        /// The two are not interchangeable. Lazy SMP buys nothing but table
        /// entries: every helper searches the same tree, so its value decays
        /// as soon as the table is warm, and on a machine with c cores it can
        /// never exceed a small constant. YBWC divides the actual work. Cheap
        /// early iterations are where redundant searches are affordable and
        /// where a split would cost more than the subtree; deep iterations are
        /// the opposite.
        fn smp_max_depth() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_MAX_DEPTH", 10))
        }

        let nodes = AtomicU64::new(0);
        let mut acc = self.nn.indices(b.black, b.white);
        let mut value = f32::NAN;

        // One pool for both kinds of parallel work, so a core
        // freed by one is immediately usable by the other. It is built once and
        // outlives every search, which is also what lets tasks be `'static`.
        let workers = self.threads - 1;
        let pool = self.shared_pool(workers);

        {
            for main_depth in 1..=depth {
                let lazy = main_depth >= smp_min_depth() && main_depth <= smp_max_depth();
                // Helpers are retired at the end of the iteration by this flag;
                // the generation counter doubles as their stop signal.
                let gen = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(main_depth));
                let mut slots = Vec::new();
                if lazy && pool.is_some() {
                    let pool = pool.unwrap();
                    for idx in 0..workers {
                        let slot = std::sync::Arc::new(Slot::new());
                        let mut w = self.worker();
                        w.done = Some(gen.clone());
                        w.my_gen = main_depth;
                        // ctz(idx+1): half the helpers share the main depth,
                        // the rest scout progressively further ahead.
                        let ahead = (idx as u32 + 1).trailing_zeros() * smp_spread();
                        // Same depth => prune harder, so they do not duplicate.
                        w.mpc_relax = (idx as u32 / 2).min(smp_sharpen_max());
                        let d = (main_depth + ahead).min(depth);
                        let task_slot = slot.clone();
                        let root = *b;
                        if pool.try_push(move || {
                            let mut wacc = w.nn.indices(root.black, root.white);
                            w.negamax(&root, &mut wacc, d, f32::NEG_INFINITY, f32::INFINITY);
                            task_slot.set(0.0, w.nodes);
                        }) {
                            slots.push(slot);
                        }
                    }
                } else {
                    // No redundant helpers to compete with: let the main search
                    // hand its younger brothers to the pool.
                    self.pool = pool;
                }

                value = self.negamax(b, &mut acc, main_depth, f32::NEG_INFINITY, f32::INFINITY);
                self.pool = None;

                // Retire this iteration's helpers before starting the next, so
                // stale passes do not keep cores away from the deeper search.
                gen.store(u32::MAX, Ordering::Relaxed);
                for slot in slots {
                    pool.unwrap().wait(&slot);
                    nodes.fetch_add(slot.nodes.load(Ordering::Relaxed), Ordering::Relaxed);
                }
            }
        }
        self.nodes += nodes.load(Ordering::Relaxed);
        (self.root_best(b, &mut acc), value)
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
    /// Fills `out` and returns how many children there are. Writing into a
    /// caller-owned stack array keeps this off the heap — returning a `Vec`
    /// meant one allocation per interior node, which at ~60k nodes a move is
    /// pure overhead.
    fn ordered_into(
        &self,
        b: &Board,
        acc: &mut PatternIndices,
        depth: u32,
        moves: u64,
        out: &mut [Kid; MAX_KIDS],
    ) -> usize {
        let mover = b.player();
        let eval_order = depth >= EVAL_ORDER_DEPTH;
        let mut n = 0usize;
        let mut m = moves;
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
            out[n] = (pos, flipped, key);
            n += 1;
        }
        out[..n].sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
        n
    }

    fn negamax(&mut self, b: &Board, acc: &mut PatternIndices, depth: u32, mut alpha: f32, beta: f32) -> f32 {
        self.nodes += 1;
        if self.should_stop_or_done() {
            return ABORTED;
        }
        // One move generation per node. `is_game_over()` generates moves for
        // both sides internally, and the pass check and child loop each
        // generated them again — three or four generations where one suffices.
        let moves = b.movable();
        if moves == 0 {
            let mut nb = *b;
            nb.pass();
            if nb.movable() == 0 {
                let p = b.player_bb().count_ones() as i32;
                let o = b.opponent_bb().count_ones() as i32;
                let e = 64 - p - o;
                let diff = if p > o { p - o + e } else if o > p { p - o - e } else { 0 };
                return diff as f32 * 1000.0;
            }
            if depth == 0 {
                return self.nn.eval_from_indices(acc, b);
            }
            let raw = self.negamax(&nb, acc, depth, -beta, -alpha);
            return if raw == ABORTED { ABORTED } else { -raw };
        }
        if depth == 0 {
            return self.nn.eval_from_indices(acc, b);
        }

        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let mut tt_move = 64u8;
        {
            let e = self.tt.get(h);
            if e.key == h && e.flag != 0 {
                // The stored move is always worth having for ordering; the
                // stored *value* only if it was searched at least as deep and
                // at least as accurately as this node needs.
                tt_move = e.best;
                if e.depth as u32 >= depth && e.relax >= self.mpc_relax as u8 {
                    match e.flag {
                        1 => return e.value,
                        2 if e.value >= beta => return e.value,
                        3 if e.value <= alpha => return e.value,
                        _ => {}
                    }
                }
            }
        }

        // Multi-ProbCut: a reduced-depth null-window probe decides whether this
        // node lies so far outside [alpha, beta] that a full search cannot
        // change the parent's decision. The margin is T sigma of the shallow
        // search's error, so it tightens as the probe gets closer to full
        // depth — a single fixed margin under-prunes exactly where the tree is
        // largest. Only at null-window nodes (never on the PV), and the probe
        // may itself prune up to MPC_MAX_LEVEL deep.
        if self.mpc
            && depth >= MPC_MIN_DEPTH
            && self.probcut_level < MPC_MAX_LEVEL
            && alpha.is_finite()
            && beta.is_finite()
            && beta - alpha <= PVS_EPS * 1.5
        {
            let pd = mpc_reduced_depth(depth);
            if pd >= 1 && pd < depth {
                // Helpers widen the margin (prune less) so same-depth workers
                // diverge; the main thread always uses MPC_T. Widening, not
                // tightening, is what makes a helper's entry safe for the main
                // thread to reuse.
                let t = MPC_T * MPC_RELAX_STEP.powi(self.mpc_relax as i32);
                let margin = t * mpc_sigma(b.empty_count() as u32, depth, pd);
                self.probcut_level += 1;
                let hi = beta + margin;
                let high = self.negamax(b, acc, pd, hi - PVS_EPS, hi);
                let cut = if high == ABORTED {
                    // Aborted probe: no information. Bail out of the node
                    // entirely rather than mistake -inf for "far below alpha".
                    self.probcut_level -= 1;
                    return ABORTED;
                } else if high >= hi {
                    Some(beta)
                } else {
                    let lo = alpha - margin;
                    let low = self.negamax(b, acc, pd, lo, lo + PVS_EPS);
                    if low == ABORTED {
                        self.probcut_level -= 1;
                        return ABORTED;
                    }
                    if low <= lo {
                        Some(alpha)
                    } else {
                        None
                    }
                };
                self.probcut_level -= 1;
                if let Some(v) = cut {
                    return v;
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
        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0.0); MAX_KIDS];
        let n_kids = if tt_move >= 64 && depth >= 2 {
            self.ordered_into(b, acc, depth, moves, &mut kids)
        } else {
            let mut n = 0usize;
            let mut m = moves;
            while m != 0 {
                let pos = Position::from_index(m.trailing_zeros()).unwrap();
                m &= m - 1;
                let mut nb = *b;
                kids[n] = (pos, nb.make_move_bits(pos), 0.0);
                n += 1;
            }
            n
        };
        if tt_move < 64 {
            if let Some(i) = kids[..n_kids].iter().position(|k| k.0.index() == tt_move) {
                kids.swap(0, i);
            }
        }

        // PVS / NegaScout: full window for the best-ordered child, a null
        // window for later siblings, re-searched only when one beats alpha.
        // Same value as plain alpha-beta. (Measured: no node reduction here —
        // the eval-based ordering already gets the effective branching to ~3,
        // near the sqrt(b) ideal, so there is nothing left for PVS to prune.
        // Kept because it costs nothing and helps when ordering degrades.)
        // YBWC (Young Brothers Wait Concept): the eldest child is searched
        // sequentially to establish the window, and only then are the younger
        // siblings' null-window searches handed to idle workers. Splitting
        // before the window is known would parallelise work that a cutoff was
        // about to make unnecessary — which is exactly what Lazy SMP does, and
        // why it stops scaling once every core already holds a copy of the
        // same tree.
        //
        // Splits start at `ybwc_min_depth`; below that a
        // subtree is cheaper than the handoff.
        //
        // Only null-window nodes are split, which is what `ybwc_*_nws` means:
        // there a child that beats alpha is an immediate cutoff, so the moment
        // one task fails high every sibling can be dropped. At a PV node the
        // same result would instead demand a full-window re-search while the
        // siblings stayed relevant — parallel work that mostly gets thrown
        // away. (Measured: splitting PV nodes too made 10 threads *slower*
        // than the old redundant-helper scheme, 0.066 vs 0.024 s per move.)
        //
        // The test is on the window handed to the *child*, not the one this
        // node was given. Every non-eldest child is searched with a null window
        // whatever kind of node its parent is, so PV nodes have young brothers
        // to give away too — and being at the top of the tree, theirs are the
        // largest subtrees there are. Testing the parent's window instead
        // excluded the whole PV spine and left the workers idle 68% of the
        // time.
        let is_nws = beta - alpha <= PVS_EPS * 1.5;
        let split_ok = self.pool.is_some() && depth >= ybwc_min_depth();
        // (child, slot, alpha it was split against)
        let mut pending: Vec<(Position, u64, std::sync::Arc<Slot>, f32)> = Vec::new();
        // Allocated on the first actual split, not on every eligible node.
        let mut node_abort: Option<std::sync::Arc<std::sync::atomic::AtomicBool>> = None;

        let mut first = true;
        let mut aborted = false;
        for i in 0..n_kids {
            // Stop as soon as a split-off sibling has already cut this node.
            // Without this check the loop keeps handing
            // out subtrees that the cutoff has just made irrelevant, and the
            // wasted nodes eat the whole parallel gain — 82% extra nodes at 6
            // threads, against 2.2x the throughput.
            if let Some(stop) = &node_abort {
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
            }
            let (pos, flipped, _) = kids[i];
            // The flips are already known from the ordering pass; applying them
            // directly avoids recomputing a full flip per child.
            let mut nb = *b;
            nb.apply_flips(pos, flipped);

            if i >= ybwc_elder() && !first && split_ok {
                let pool = self.pool.unwrap();
                let slot = std::sync::Arc::new(Slot::new());
                let stop = node_abort
                    .get_or_insert_with(|| {
                        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
                    })
                    .clone();
                let (nn, tt, mpc, sharpen) = (self.nn, self.tt, self.mpc, self.mpc_relax);
                let (gen, my_gen) = (self.done.clone(), self.my_gen);
                let (task_slot, child, a, d) = (slot.clone(), nb, alpha, depth - 1);
                let pushed = pool.try_push(move || {
                    let mut w = NnueSearch::new(nn, private_tt().unwrap_or(tt));
                    w.mpc = mpc;
                    w.mpc_relax = sharpen;
                    w.pool = Some(pool);
                    w.abort = Some(stop.clone());
                    w.done = gen;
                    w.my_gen = my_gen;
                    let mut cacc = w.nn.indices(child.black, child.white);
                    let v = w.negamax(&child, &mut cacc, d, -(a + PVS_EPS), -a);
                    // Beating alpha cuts the parent only at a null-window
                    // node; there, retire the siblings at once rather than let
                    // them run to completion. At a PV node the same result only means this
                    // child needs a full-window re-search, and the siblings
                    // still matter.
                    if is_nws && v != ABORTED && -v > a {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    TASK_NODES.fetch_add(w.nodes, std::sync::atomic::Ordering::Relaxed);
                    task_slot.set(v, w.nodes);
                });
                if pushed {
                    pending.push((pos, flipped, slot, alpha));
                    continue;
                }
            }

            self.nn.ix_apply(acc, pos, flipped, mover);
            // Inspect the child's own return value: negating it would turn the
            // ABORTED sentinel into +inf and hide the abort.
            let raw = if first {
                self.negamax(&nb, acc, depth - 1, -beta, -alpha)
            } else {
                let probe = self.negamax(&nb, acc, depth - 1, -(alpha + PVS_EPS), -alpha);
                if probe != ABORTED && -probe > alpha && -probe < beta {
                    self.negamax(&nb, acc, depth - 1, -beta, probe)
                } else {
                    probe
                }
            };
            self.nn.ix_undo(acc, pos, flipped, mover);
            if raw == ABORTED {
                aborted = true;
                break;
            }
            let v = -raw;
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

        // Collect the split-off siblings. A cutoff already found here makes the
        // rest irrelevant, so tell them to stop — but still wait, since their
        // slots are borrowed by tasks that are already running.
        if let Some(stop) = &node_abort {
            let pool = self.pool.unwrap();
            if aborted || alpha >= beta {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            for (pos, flipped, slot, split_alpha) in std::mem::take(&mut pending) {
                // Wait even for tasks whose answer is already irrelevant: the
                // slot they write to is still live until they notice the stop.
                pool.wait(&slot);
                self.nodes += slot.nodes.load(std::sync::atomic::Ordering::Relaxed);
                if aborted || alpha >= beta {
                    continue;
                }
                let raw = f32::from_bits(slot.bits.load(std::sync::atomic::Ordering::Relaxed));
                if raw == ABORTED {
                    // `stop` is node-local and only ever raised by a sibling
                    // failing high (or by this node cutting), so an aborted
                    // task means "a sibling already decided this node" — not
                    // that the node is unresolved. Marking the node aborted
                    // here threw away the very fail-high that caused the abort,
                    // and propagated a bogus ABORTED to the root: worth 13
                    // points of win rate in self-play.
                    continue;
                }
                let mut v = -raw;
                // A null-window probe that beat its alpha is only a lower
                // bound. At a null-window node that is enough (it cuts), but on
                // the PV an exact value is needed, so re-search with the real
                // window — the same thing the sequential PVS path does.
                if !is_nws && v > split_alpha && v < beta {
                    let mut nb = *b;
                    nb.apply_flips(pos, flipped);
                    self.nn.ix_apply(acc, pos, flipped, mover);
                    let re = self.negamax(&nb, acc, depth - 1, -beta, -alpha.max(split_alpha));
                    self.nn.ix_undo(acc, pos, flipped, mover);
                    if re == ABORTED {
                        aborted = true;
                        continue;
                    }
                    v = -re;
                }
                if v > best {
                    best = v;
                    best_move = pos.index() as u8;
                }
                if best > alpha {
                    alpha = best;
                }
                if alpha >= beta {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }

        if aborted {
            // A truncated subtree carries no value. Returning here — and
            // crucially *not* writing the table — keeps the abort from
            // poisoning entries that other threads will trust. (Storing it
            // was measured as a total collapse: 0% score, -59 discs.)
            return ABORTED;
        }

        let flag = if best <= orig_alpha { 3 } else if best >= beta { 2 } else { 1 };
        self.tt.put(
            h,
            TtEntry {
                key: h,
                value: best,
                best: best_move,
                depth: depth as u8,
                flag,
                relax: self.mpc_relax as u8,
            },
        );
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
            if let Some(nn) = nnue.as_deref_mut() {
                clock.our_nodes += std::mem::take(&mut nn.nodes);
            }
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
    // The search borrows the model and the shared table; 2^20 entries (~24 MB),
    // cleared per game. The table outlives the search so workers can share it.
    // 2^24 entries (~384 MB), chosen by measurement: 2^20 -> 0.037 s/move,
    // 2^22 -> 0.036, 2^24 -> 0.032, 2^25/2^26 -> 0.034.
    //
    // An earlier sweep found *larger* tables slower, but that was with the
    // helpers writing to a table of their own — nothing they produced had to
    // survive. Now 16 threads share one table and the helpers exist precisely
    // so the main thread reuses their entries, so capacity buys hit rate until
    // the working set stops fitting the cache hierarchy around 2^24.
    let nnue: Option<&'static Nnue> = nnue.map(|n| &*Box::leak(Box::new(n)));
    let nnue_tt: Option<&'static SharedTt> = nnue.map(|_| &*Box::leak(Box::new(SharedTt::new(24))));
    let mut nnue_search = match (nnue, nnue_tt) {
        (Some(nn), Some(tt)) => {
            let mut s = NnueSearch::new(nn, tt);
            s.mpc = args.mpc; // same selectivity switch as the linear searcher
            s.threads = args.threads;
            Some(s)
        }
        _ => None,
    };

    // Parallel vs sequential, head to head. Same net, same depth, same endgame
    // threshold — only the thread count differs, so anything other than 50%
    // is the parallel search deciding differently, and worse.
    if let Some(vs_threads) = args.self_vs {
        let Some(nn) = nnue else {
            eprintln!("--self-vs needs --nnue");
            return ExitCode::FAILURE;
        };
        let tt_b: &'static SharedTt = Box::leak(Box::new(SharedTt::new(24)));
        let nn_b: &'static Nnue = match &args.nnue_b {
            Some(p) => {
                let mut other = Nnue::new(patterns);
                if let Err(e) = other.load(p) {
                    eprintln!("load {} failed: {e}", p.display());
                    return ExitCode::FAILURE;
                }
                other.quantize();
                println!("side B net: {}", p.display());
                &*Box::leak(Box::new(other))
            }
            None => nn,
        };
        let mut side_a = nnue_search.take().expect("nnue search");
        let mut side_b = NnueSearch::new(nn_b, tt_b);
        side_b.mpc = args.mpc;
        side_b.threads = vs_threads;
        let mut solver_a = Solver::new(if args.solve_empties >= 18 { 22 } else { 18 });
        solver_a.set_threads(args.threads);
        let mut solver_b = Solver::new(if args.solve_empties >= 18 { 22 } else { 18 });
        solver_b.set_threads(vs_threads);

        let mut rng = Rng::new(args.seed);
        let (mut wins, mut losses, mut draws) = (0usize, 0usize, 0usize);
        let mut disc_sum = 0i64;
        let (mut time_a, mut time_b) = (0.0f64, 0.0f64);
        for _ in 0..args.games / 2 {
            let (opening, _) = random_opening(&mut rng, args.random_plies);
            for a_is_black in [true, false] {
                side_a.clear();
                side_b.clear();
                let mut board = opening;
                loop {
                    if board.movable() == 0 {
                        let mut passed = board;
                        passed.pass();
                        if passed.movable() == 0 {
                            break;
                        }
                        board = passed;
                        continue;
                    }
                    let a_turn = (board.player() == Color::Black) == a_is_black;
                    let t0 = std::time::Instant::now();
                    let pos = {
                        let (search, solver) = if a_turn {
                            (&mut side_a, &mut solver_a)
                        } else {
                            (&mut side_b, &mut solver_b)
                        };
                        if args.solve_empties > 0 && board.empty_count() <= args.solve_empties {
                            match solver
                                .solve_with_eval(EndSolverMode::Perfect, &board, Some(&evaluator))
                                .best_move
                            {
                                Some(p) => p,
                                None => break,
                            }
                        } else {
                            match search.best_move(&board, args.depth as u32) {
                                Some(p) => p,
                                None => break,
                            }
                        }
                    };
                    if a_turn {
                        time_a += t0.elapsed().as_secs_f64();
                    } else {
                        time_b += t0.elapsed().as_secs_f64();
                    }
                    board.make_move_bits(pos);
                }
                let black = board.black.count_ones() as i32;
                let white = board.white.count_ones() as i32;
                let diff = if a_is_black { black - white } else { white - black };
                disc_sum += diff as i64;
                match diff.cmp(&0) {
                    std::cmp::Ordering::Greater => wins += 1,
                    std::cmp::Ordering::Less => losses += 1,
                    std::cmp::Ordering::Equal => draws += 1,
                }
            }
        }
        let n = (wins + losses + draws) as f64;
        let p = (wins as f64 + 0.5 * draws as f64) / n;
        let se = (p * (1.0 - p) / n).sqrt();
        println!(
            "threads {} vs {}: A {:.1}%  (95% CI {:.1}%..{:.1}%)  {}W {}L {}D  mean disc {:+.2}",
            args.threads,
            vs_threads,
            p * 100.0,
            (p - 1.96 * se) * 100.0,
            (p + 1.96 * se) * 100.0,
            wins,
            losses,
            draws,
            disc_sum as f64 / n
        );
        println!("time: A {time_a:.1}s  B {time_b:.1}s  (speedup {:.2}x)", time_b / time_a.max(1e-9));
        println!(
            "splits: {} accepted / {} offered",
            SPLIT_DONE.load(std::sync::atomic::Ordering::Relaxed),
            SPLIT_TRIED.load(std::sync::atomic::Ordering::Relaxed)
        );
        return ExitCode::SUCCESS;
    }

    // Equivalence check: the parallel search must decide the same move as the
    // sequential one. Win rate cannot see a small regression (200 games still
    // leave a +-7% band), but a move mismatch is a direct, per-position signal.
    if args.verify_parallel {
        let Some(nn) = nnue else {
            eprintln!("--verify-parallel needs --nnue");
            return ExitCode::FAILURE;
        };
        let mut rng = Rng::new(args.seed);
        let (mut same, mut diff, mut val_diff) = (0usize, 0usize, 0usize);
        let mut better = 0usize;
        let mut value_mismatch = 0usize;
        let positions = args.games.max(2);
        for _ in 0..positions {
            // Random midgame positions, deeper than the opening so the search
            // has something to decide.
            let (mut board, _) = random_opening(&mut rng, args.random_plies);
            for _ in 0..8 {
                let moves = board.movable();
                if moves == 0 {
                    break;
                }
                board.make_move_bits(nth_move(moves, rng.below(moves.count_ones())));
            }
            if board.movable() == 0 {
                continue;
            }

            // Fresh tables both times: a warm table would let one run inherit
            // the other's work and hide a divergence.
            let seq_tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(20)));
            let mut seq = NnueSearch::new(nn, seq_tt);
            seq.mpc = args.mpc;
            seq.threads = 1;
            let (seq_move, seq_value) = seq.best_move_valued(&board, args.depth as u32);

            let par_tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(20)));
            let mut par = NnueSearch::new(nn, par_tt);
            par.mpc = args.mpc;
            par.threads = args.threads.max(2);
            let (par_move, par_value) = par.best_move_valued(&board, args.depth as u32);

            // The root value must match too: an identical move with a different
            // value means the searches disagreed somewhere and only happened to
            // land on the same choice.
            if seq_value != par_value {
                value_mismatch += 1;
                println!("  VALUE DIFF: seq {seq_value:.4} vs par {par_value:.4} (move {seq_move:?} / {par_move:?})");
            }
            if seq_move == par_move {
                same += 1;
            } else {
                diff += 1;
                // A different move is only a real problem if it is also worse.
                // Score both at the same depth from the sequential searcher.
                let mut judge_tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(20)));
                let value_of = |mv: Option<Position>, tt: &'static SharedTt| -> f32 {
                    let Some(mv) = mv else { return f32::NAN };
                    let mut j = NnueSearch::new(nn, tt);
                    j.mpc = args.mpc;
                    let mut child = board;
                    let flipped = child.make_move_bits(mv);
                    let mut ix = nn.indices(board.black, board.white);
                    nn.ix_apply(&mut ix, mv, flipped, board.player());
                    -j.negamax(&child, &mut ix, args.depth as u32, f32::NEG_INFINITY, f32::INFINITY)
                };
                let vs = value_of(seq_move, &judge_tt);
                judge_tt = Box::leak(Box::new(SharedTt::new(20)));
                let vp = value_of(par_move, &judge_tt);
                // Only a *worse* parallel choice is a regression. Comparing the
                // absolute difference counted the parallel search's own
                // improvements as failures.
                if vs - vp > 0.5 {
                    val_diff += 1;
                    println!("  WORSE: seq {seq_move:?} ({vs:.2}) vs par {par_move:?} ({vp:.2})");
                } else if vp - vs > 0.5 {
                    better += 1;
                    println!("  better: seq {seq_move:?} ({vs:.2}) vs par {par_move:?} ({vp:.2})");
                }
            }
        }
        println!(
            "verify-parallel (depth {}, {} threads, {} positions): same move {same}, \
             different move {diff} (parallel worse {val_diff}, parallel better {better}, \
             equal {})",
            args.depth,
            args.threads.max(2),
            same + diff,
            diff - val_diff - better
        );
        println!("root value mismatches: {value_mismatch}");
        println!(
            "=> {}",
            if diff == 0 && value_mismatch == 0 {
                "parallel search reproduces the sequential search exactly (move and value)"
            } else {
                "PARALLEL SEARCH DIFFERS FROM SEQUENTIAL"
            }
        );
        return ExitCode::SUCCESS;
    }

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
        "our search: {} nodes, {:.2} Mnps, worker busy {:.1}%",
        clock.our_nodes,
        clock.our_nodes as f64 / clock.ours.max(1e-9) / 1e6,
        WORKER_BUSY_NS.load(std::sync::atomic::Ordering::Relaxed) as f64
            / (clock.ours * 1e9
                * POOL_WORKERS.load(std::sync::atomic::Ordering::Relaxed).max(1) as f64)
            * 100.0
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
