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
//!   --weights <path>     Our weight file (default weights/weights_full.bin)
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
//!   --per-game           One line per game (`game <pair> <B|W> <disc-diff>`),
//!                        so two runs over the same seed can be paired

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
    /// Threads for the opponent. Separate from ours so the opponent's own
    /// parallel efficiency can be measured with our side held fixed — the
    /// only honest way to know what speedup this machine actually supports.
    edax_threads: Option<usize>,
    per_game: bool,
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
    band_probe: Option<usize>,
    band_empties: u8,
    gen_obf: Option<PathBuf>,
    solver_hash: Option<u8>,
    sigma_calib: Option<PathBuf>,
    obf: Option<PathBuf>,
    selective_band: u8,
    mpc_calib: Option<PathBuf>,
    calib_stride: usize,
    calib_max: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        edax_path: PathBuf::new(),
        weights: PathBuf::from("weights/weights_full.bin"),
        nnue: None,
        patterns: "egaroucid",
        depth: 6,
        mpc: false,
        solve_empties: 14,
        edax_level: 5,
        protocol: "edax",
        threads: 1,
        edax_threads: None,
        per_game: false,
        games: 200,
        random_plies: 6,
        seed: 7,
        verify_parallel: false,
        self_vs: None,
        band_probe: None,
        band_empties: 27,
        gen_obf: None,
        solver_hash: None,
        sigma_calib: None,
        obf: None,
        selective_band: 0,
        mpc_calib: None,
        calib_stride: 997,
        calib_max: 3000,
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
            "--per-game" => args.per_game = true,
            "--edax-threads" => args.edax_threads = Some(value("--edax-threads")?.parse().map_err(|e| format!("--edax-threads: {e}"))?),
            "--games" => args.games = value("--games")?.parse().map_err(|e| format!("--games: {e}"))?,
            "--random-plies" => args.random_plies = value("--random-plies")?.parse().map_err(|e| format!("--random-plies: {e}"))?,
            "--seed" => args.seed = value("--seed")?.parse().map_err(|e| format!("--seed: {e}"))?,
            "--verify-parallel" => args.verify_parallel = true,
            "--mpc-calib" => args.mpc_calib = Some(PathBuf::from(value("--mpc-calib")?)),
            "--calib-stride" => args.calib_stride = value("--calib-stride")?.parse().map_err(|e| format!("--calib-stride: {e}"))?,
            "--calib-max" => args.calib_max = value("--calib-max")?.parse().map_err(|e| format!("--calib-max: {e}"))?,
            "--selective-band" => args.selective_band = value("--selective-band")?.parse().map_err(|e| format!("--selective-band: {e}"))?,
            "--self-vs" => args.self_vs = Some(value("--self-vs")?.parse().map_err(|e| format!("--self-vs: {e}"))?),
            "--band-probe" => args.band_probe = Some(value("--band-probe")?.parse().map_err(|e| format!("--band-probe: {e}"))?),
            "--sigma-calib" => args.sigma_calib = Some(PathBuf::from(value("--sigma-calib")?)),
            "--solver-hash" => args.solver_hash = Some(value("--solver-hash")?.parse().map_err(|e| format!("--solver-hash: {e}"))?),
            "--gen-obf" => args.gen_obf = Some(PathBuf::from(value("--gen-obf")?)),
            "--obf" => args.obf = Some(PathBuf::from(value("--obf")?)),
            "--band-empties" => args.band_empties = value("--band-empties")?.parse().map_err(|e| format!("--band-empties: {e}"))?,
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

/// One `.obf` record: 64 board characters, a space, and the side to move.
/// Anything after the position (FFO files carry the solution list) is ignored,
/// as is anything that is not a record.
fn parse_obf(line: &str) -> Option<Board> {
    let s = line.trim();
    if s.len() < 66 {
        return None;
    }
    Board::from_string(&s[..66]).ok()
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
struct Clock {
    ours: f64,
    edax: f64,
    our_moves: u64,
    edax_moves: u64,
    /// Time spent in the NNUE midgame search only. The endgame solver has its
    /// own threads and leaves the YBWC pool idle, so counting it would make
    /// worker utilisation look far worse than it is.
    our_mid: f64,
    /// Nodes our midgame search visited, so parallel *throughput* can be told
    /// apart from parallel *speedup*: if nodes per second scale with the core
    /// count but wall-clock does not, the threads are being fed and the extra
    /// work is simply wasted; if nps saturates too, the cores are starved and
    /// no scheduling change can help.
    our_nodes: u64,
    /// Seconds and moves by empty count, for both sides. A per-game average
    /// hides the only thing that matters here: the two engines search the same
    /// way everywhere except the six plies between 25 and 30 empties, where one
    /// reads to the end and the other does not. Averaged over a whole game that
    /// difference is a single number that could come from anywhere.
    our_by_empties: [f64; 65],
    edax_by_empties: [f64; 65],
    our_n_by_empties: [u32; 65],
    edax_n_by_empties: [u32; 65],
}

// `derive(Default)` stops at 32-element arrays.
impl Default for Clock {
    fn default() -> Clock {
        Clock {
            ours: 0.0,
            edax: 0.0,
            our_moves: 0,
            edax_moves: 0,
            our_mid: 0.0,
            our_nodes: 0,
            our_by_empties: [0.0; 65],
            edax_by_empties: [0.0; 65],
            our_n_by_empties: [0; 65],
            edax_n_by_empties: [0; 65],
        }
    }
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
/// Tunable so it can be swept: work below `ybwc_min_depth` cannot be split, so
/// in a parallel search a second of it costs as much wall-clock as six seconds
/// of splittable work. The sequential optimum is not the parallel optimum.
fn eval_order_depth() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EVAL_ORDER_DEPTH").ok().and_then(|v| v.parse().ok()).unwrap_or(3)
    })
}

/// A worker whose subtree became irrelevant returns this instead of a score,
/// so the caller knows to discard it rather than treat it as an evaluation.
const ABORTED: f32 = f32::NEG_INFINITY;

/// Nodes between abort checks: the flag is shared, so reading it every node
/// would put a contended load in the hot path.
/// Below `ybwc_min_depth` a subtree is cheaper to
/// search than to hand off.
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

/// Confidence to read the rest of the game with, or `None` to search the
/// midgame instead. The schedule, anchored at an exact solve from 24 empties,
/// reads to the end at 93% from 30 empties, 98% from 28, 99% from 26
/// and exactly from 24. The sigma multipliers are the two-sided z-scores for
/// those percentages, the same scale `MPC_T` uses.
fn selective_band(empties: u8, solve_empties: u8, width: u8) -> Option<f32> {
    if empties <= solve_empties || empties > solve_empties + width {
        return None;
    }
    match empties - solve_empties {
        1..=2 => Some(2.58), // 99%
        3..=4 => Some(2.33), // 98%
        5..=6 => Some(1.81), // 93%
        _ => None,
    }
}

/// Static square priority for cheap ordering: corner, box, block, then
/// X and C squares. The four centre squares are missing from the union, which is
/// safe — they are occupied from the opening position on and can never be a
/// legal move.
const STATIC_CELL_PRIORITY: [u64; 4] = [
    0x8100000000000081, // corner
    0x00003C24243C0000, // box
    0x3C3CC3C3C3C33C3C, // block
    0x42C300000000C342, // X, C
];

/// Corner mask: a legal move into a corner counts one
/// extra in the weighted mobility.
const CORNERS: u64 = 0x8100000000000081;

/// Potential mobility: empty squares next to an opponent disc.
/// A move that leaves the opponent few of those is good even when it does not
/// reduce their legal moves yet.
#[inline]
fn potential_mobility(discs: u64, empties: u64) -> u32 {
    let hmask = discs & 0x7E7E7E7E7E7E7E7E;
    let vmask = discs & 0x00FFFFFFFFFFFF00;
    let hvmask = discs & 0x007E7E7E7E7E7E00;
    let res = (hmask << 1)
        | (hmask >> 1)
        | (vmask << 8)
        | (vmask >> 8)
        | (hvmask << 7)
        | (hvmask >> 7)
        | (hvmask << 9)
        | (hvmask >> 9);
    (res & empties).count_ones()
}


/// How many tasks may be queued beyond the workers that are idle right now.
/// Measured here (d16, 20 games, min of 3):
/// 0 -> 2.25x, 2 -> 2.39x, 8 -> 2.43x, 32 -> 2.20x. Two is the robust choice —
/// it is the best of the four at 8 threads and within noise of the best at 6.
fn pool_slack() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("POOL_SLACK").ok().and_then(|v| v.parse().ok()).unwrap_or(2)
    })
}

/// Slots probed per position. Four
/// 16-byte entries are exactly one 64-byte cache line, so the whole probe is a
/// single memory transaction.
const TT_WAYS: usize = 4;

/// How valuable an entry is: depth first,
/// accuracy as the tie-break.
#[inline]
fn tt_level(depth: u8, relax: u8) -> u32 {
    ((depth as u32) << 8) | relax as u32
}

/// A transposition table shared by the root-split workers.
///
/// Writes race, but every entry carries the position key and is compared
/// exactly, so a torn read can only produce a *mismatch* (recomputed), never a
/// wrong value for another position. This is the same argument the linear
/// searcher's shared table relies on.
struct SharedTt {
    buckets: std::cell::UnsafeCell<Vec<TtBucket>>,
    mask: u64,
}
// SAFETY: see the note above — entries are self-validating.
unsafe impl Sync for SharedTt {}

/// One cache line of the table. Probing *consecutive*
/// slots from the hash would straddle two lines whenever the
/// hash lands near a boundary — every probe then costs two memory transactions
/// instead of one, and every store dirties two lines that other threads may
/// hold. Aligning the group instead makes the whole probe one line.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct TtBucket([TtEntry; TT_WAYS]);

impl SharedTt {
    fn new(bits: u32) -> SharedTt {
        let n = 1usize << bits.saturating_sub(2);
        SharedTt {
            buckets: std::cell::UnsafeCell::new(vec![
                TtBucket(
                    [TtEntry::EMPTY; TT_WAYS]
                );
                n
            ]),
            mask: (n - 1) as u64,
        }
    }

    #[inline]
    fn bucket(&self, hash: u64) -> &mut TtBucket {
        // SAFETY: index is masked into range; a racing write can only make the
        // key mismatch, which the caller treats as a miss.
        unsafe {
            let v = &mut *self.buckets.get();
            v.get_unchecked_mut((hash & self.mask) as usize)
        }
    }

    #[inline]
    fn slot(&self, hash: u64, i: u64) -> &mut TtEntry {
        // SAFETY: `i` is always below TT_WAYS.
        unsafe { self.bucket(hash).0.get_unchecked_mut(i as usize) }
    }

    /// Scan the bucket for this position.
    #[inline]
    fn get(&self, hash: u64) -> TtEntry {
        let b = self.bucket(hash);
        for e in b.0.iter() {
            if e.key == hash && e.flag != 0 {
                return *e;
            }
        }
        TtEntry::EMPTY
    }

    /// Store: walk the bucket and take the first slot that is worth
    /// no more than what is being stored, where worth is `(depth, accuracy)`.
    /// If all three hold deeper or more accurate results, store nothing.
    ///
    /// A one-way table cannot express that: every store evicts whatever was
    /// there, so a shallow probe near a leaf throws away the deep entry the
    /// iteration above it will ask for. Under six threads the shallow stores
    /// arrive from every worker at once and the deep entries barely survive.
    #[inline]
    /// `alpha` / `beta` are the window the node *started* with, not the one the
    /// table narrowed it to. Registering against the narrowed window would claim
    /// a bound the search never established.
    #[allow(clippy::too_many_arguments)]
    fn put(
        &self,
        hash: u64,
        depth: u8,
        relax: u8,
        alpha: f32,
        beta: f32,
        value: f32,
        best_move: u8,
    ) {
        let level = tt_level(depth, relax);
        let upper = if value < beta { value } else { f32::INFINITY };
        let lower = if value > alpha { value } else { f32::NEG_INFINITY };
        for i in 0..TT_WAYS {
            let slot = self.slot(hash, i as u64);
            if slot.key == hash && slot.flag != 0 {
                let slot_level = tt_level(slot.depth, slot.relax);
                if slot_level > level {
                    return;
                }
                if slot_level == level {
                    // Same level: the same node searched again
                    // through a different window, so the two bounds combine —
                    // *and each assignment repairs the other end if it would
                    // cross it*. ProbCut makes every stored value probabilistic
                    // and two threads at the same level can disagree, so
                    // `lower > upper` really does happen; leaving those repair
                    // clauses out cost 20% more nodes than a single tagged
                    // value, which is what made this look like a bad idea the
                    // first time it was tried.
                    if value < beta && value < slot.upper {
                        slot.upper = value;
                        if value > alpha && value < slot.lower {
                            slot.lower = value;
                        }
                    }
                    if value > alpha && slot.lower < value {
                        slot.lower = value;
                        if value < beta && slot.upper < value {
                            slot.upper = value;
                        }
                    }
                } else {
                    // Deeper result: the old bounds no
                    // longer apply and are replaced outright.
                    slot.lower = lower;
                    slot.upper = upper;
                    slot.depth = depth;
                    slot.relax = relax;
                }
                if value > alpha && best_move < 64 && slot.best != best_move {
                    slot.best2 = slot.best;
                    slot.best = best_move;
                }
                return;
            }
        }
        for i in 0..TT_WAYS {
            let slot = self.slot(hash, i as u64);
            if slot.flag == 0 || tt_level(slot.depth, slot.relax) <= level {
                *slot = TtEntry {
                    key: hash,
                    lower,
                    upper,
                    // The move is dropped when the search
                    // failed low, since nothing was proven about it. That was
                    // ruinous while the ordering fell back to bit order without
                    // a table move; with the scored ordering pass in place it
                    // costs only the head start.
                    best: best_move,
                    best2: 64,
                    depth,
                    flag: 1,
                    relax,
                };
                return;
            }
        }
    }

    fn clear(&self) {
        // SAFETY: called between games, with no workers running.
        unsafe {
            for b in (*self.buckets.get()).iter_mut() {
                for e in b.0.iter_mut() {
                    e.flag = 0;
                }
            }
        }
    }
}

/// One transposition-table slot for the NNUE search. `flag`: 0 empty,
/// 1 exact, 2 lower bound, 3 upper bound.
#[derive(Clone, Copy)]
struct TtEntry {
    key: u64,
    /// The entry keeps a **pair of bounds** rather than one value
    /// plus a lower/upper/exact tag. A tagged value answers only half the
    /// questions asked of it: a node that failed low here leaves an upper bound,
    /// and the next visit — arriving with a window that needs a lower bound —
    /// learns nothing and searches again. Both ends accumulate into
    /// the same entry over successive visits and the table hands out
    /// whichever end the caller can use.
    lower: f32,
    upper: f32,
    best: u8,
    /// The move the current best displaced, kept as the
    /// runner-up for ordering.
    best2: u8,
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

impl TtEntry {
    const EMPTY: TtEntry = TtEntry {
        key: 0,
        lower: f32::NEG_INFINITY,
        upper: f32::INFINITY,
        best: 64,
        best2: 64,
        depth: 0,
        flag: 0,
        relax: 0,
    };
}

/// How often a split was attempted and how often the pool accepted it. A large
/// gap means the workers are saturated; a small `SPLIT_TRIED` means the search
/// is not offering them work in the first place.
static SPLIT_TRIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nanoseconds pool workers spent inside tasks. Against (workers x our search
/// time) this is worker utilisation: it separates "the workers are idle" from
/// "the workers are busy but contending".
static WORKER_BUSY_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static POOL_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SPLIT_DONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nanoseconds a splitting thread spent inside `Pool::wait` with nothing left
/// to steal, split by who was waiting. This is the part of the machine that
/// YBWC is *paying* for and not using: the parent is blocked on a child while
/// the queue is empty. Separating it from real search time is the only way to
/// tell "the workers are starved" from "the work is there but redundant".
static WAIT_IDLE_MAIN_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WAIT_IDLE_WORKER_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// How often the pool was seen with k workers busy, sampled on a timer. The
/// averages cannot tell "steadily half loaded" from "alternately saturated and
/// empty", and the two call for opposite fixes: the first needs more split
/// points, the second needs the bursts smoothed out.
static BUSY_HIST: [std::sync::atomic::AtomicU64; 17] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 17];
/// Raised only while the NNUE midgame search is running. Without it the sampler
/// also counts the opponent's turns and the endgame solver, where the pool idles
/// by construction — three quarters of the samples, which made the pool look
/// empty when it was simply not in use.
static SEARCH_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    static IS_POOL_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SPLIT_TRIED_LOCAL: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A node's own "stop the fan-out"
/// flag chained to every ancestor's. A task carries the whole chain, so an
/// abort raised anywhere above it reaches it — `is_searching` walks the vector.
///
/// Without the chain a split subtree only ever sees the flag of the node that
/// created it: when a grandparent cuts off, the grandchild keeps searching a
/// tree nobody will read, and the parent sits blocked in `wait` for it.
struct AbortChain {
    flag: std::sync::atomic::AtomicBool,
    parent: Option<std::sync::Arc<AbortChain>>,
}

impl AbortChain {
    fn child(parent: Option<std::sync::Arc<AbortChain>>) -> std::sync::Arc<AbortChain> {
        std::sync::Arc::new(AbortChain {
            flag: std::sync::atomic::AtomicBool::new(false),
            parent,
        })
    }
    fn raise(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn stopped(&self) -> bool {
        let mut n = self;
        loop {
            if n.flag.load(std::sync::atomic::Ordering::Relaxed) {
                return true;
            }
            match &n.parent {
                Some(p) => n = p,
                None => return false,
            }
        }
    }
}

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
    /// Workers currently blocked waiting for work.
    idle: std::sync::atomic::AtomicUsize,
    /// Queue length, readable without taking the lock. A thread waiting on a
    /// split polls for work; if that poll locks the mutex every iteration it
    /// serialises every worker against the waiters and throughput collapses.
    queued: std::sync::atomic::AtomicUsize,
    workers: usize,
    /// Set at shutdown; the workers only look at it when woken, so raising it
    /// has to be followed by `cv.notify_all()`.
    #[allow(dead_code)]
    stop: std::sync::atomic::AtomicBool,
}

impl Pool {
    fn new(workers: usize) -> Pool {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        Pool {
            q: std::sync::Mutex::new(std::collections::VecDeque::new()),
            cv: std::sync::Condvar::new(),
            idle: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            workers,
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
        // Counted thread-locally and flushed in batches: a shared atomic
        // incremented on every split *attempt* is a contended cache line hit
        // hundreds of thousands of times a second, which would be measurement
        // that costs what it measures.
        SPLIT_TRIED_LOCAL.with(|c| {
            let n = c.get() + 1;
            if n >= 4096 {
                c.set(0);
                SPLIT_TRIED.fetch_add(n, Ordering::Relaxed);
            } else {
                c.set(n);
            }
        });
        // Only hand work over when a worker is actually free to start it now,
        // reporting the failure to the caller, which then searches the child
        // itself. Queueing beyond that looks like handing work off but really
        // parks it: the task waits in line while the parent blocks on it.
        //
        // The test runs *unlocked* first, and only
        // then takes the lock to re-check. Nearly every split attempt is a
        // rejection — the pool is busy — and taking the mutex to discover that
        // funnels every searching thread through one lock several hundred
        // thousand times a second.
        // Handing work over only when a worker is idle *now* (zero slack) was
        // the rule here once before; queueing beyond it was measured as a loss:
        // the table said 1.87x the sequential node count, and
        // parking a task behind a full queue while its parent blocks on it is
        // exactly how that happens.
        //
        // That node blow-up is gone (1.03x now that aborts propagate through
        // the whole ancestor chain), so the trade is worth re-measuring. The
        // reason to want a queue: a node with nine young brothers and five idle
        // workers hands out five and then searches the other four itself, one
        // after another — and the five tasks finish long before it is done, so
        // the workers sit idle through the rest of the fan-out.
        let slack = pool_slack();
        let free = |queued: usize| self.idle.load(Ordering::Relaxed) + slack > queued;
        if !free(self.queued.load(Ordering::Relaxed)) {
            return false;
        }
        let mut q = self.q.lock().unwrap();
        if !free(q.len()) {
            return false;
        }
        SPLIT_DONE.fetch_add(1, Ordering::Relaxed);
        q.push_back(Box::new(f));
        self.queued.fetch_add(1, Ordering::Relaxed);
        drop(q);
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
        let mut starved: Option<std::time::Instant> = None;
        while !slot.done.load(Ordering::Acquire) {
            if self.run_one() {
                idle = 0;
                if let Some(t0) = starved.take() {
                    let ns = t0.elapsed().as_nanos() as u64;
                    if IS_POOL_WORKER.with(|c| c.get()) {
                        WAIT_IDLE_WORKER_NS.fetch_add(ns, Ordering::Relaxed);
                    } else {
                        WAIT_IDLE_MAIN_NS.fetch_add(ns, Ordering::Relaxed);
                    }
                }
                continue;
            }
            // Nothing to steal. `run_one` reads `queued`, and that line is
            // written by every push and every pop — spinning on it in a tight
            // loop does not just waste this core, it slows down the threads
            // that are actually searching. Back off geometrically so a waiter
            // touches the line rarely once it is clear no work is coming.
            if starved.is_none() {
                starved = Some(std::time::Instant::now());
            }
            idle += 1;
            if idle < 8 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
                if idle > 64 {
                    std::thread::sleep(std::time::Duration::from_micros(20));
                }
            }
        }
        if let Some(t0) = starved {
            let ns = t0.elapsed().as_nanos() as u64;
            if IS_POOL_WORKER.with(|c| c.get()) {
                WAIT_IDLE_WORKER_NS.fetch_add(ns, Ordering::Relaxed);
            } else {
                WAIT_IDLE_MAIN_NS.fetch_add(ns, Ordering::Relaxed);
            }
        }
    }

    fn worker_loop(&self) {
        use std::sync::atomic::Ordering;
        IS_POOL_WORKER.with(|c| c.set(true));
        loop {
            // The worker counts as idle for the whole time it is looking for
            // work, spinning included — the split gate is `idle > queued`, so a
            // spinning worker that did not count itself would not be offered
            // anything.
            self.idle.fetch_add(1, Ordering::Relaxed);
            let task = 'get: loop {
                // Sleep until woken,
                // with no timeout. A 200us poll instead had all the idle workers
                // waking twenty-five thousand times a second to take the queue
                // mutex, find nothing, and go back to sleep — every one of those
                // acquisitions serialises against a thread trying to hand work
                // over.
                //
                // Spinning before parking to hide the measured 9us handoff was
                // tried and is not here: it only pays when tasks are small
                // enough for 9us to matter, and splitting that shallow loses
                // more than the latency costs (see `ybwc_min_depth`).
                let mut q = self.q.lock().unwrap();
                loop {
                    if let Some(t) = q.pop_front() {
                        self.queued.fetch_sub(1, Ordering::Relaxed);
                        break 'get Some(t);
                    }
                    if self.stop.load(Ordering::Relaxed) {
                        break 'get None;
                    }
                    q = self.cv.wait(q).unwrap();
                }
            };
            self.idle.fetch_sub(1, Ordering::Relaxed);
            match task {
                Some(t) => {
                    let t0 = std::time::Instant::now();
                    t();
                    WORKER_BUSY_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                None => return,
            }
        }
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
    abort: Option<std::sync::Arc<AbortChain>>,
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
            if std::env::var("POOL_SAMPLE").is_ok() {
                std::thread::spawn(move || loop {
                    if SEARCH_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
                        let idle = pool.idle.load(std::sync::atomic::Ordering::Relaxed);
                        let busy = workers.saturating_sub(idle).min(16);
                        BUSY_HIST[busy].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    std::thread::sleep(std::time::Duration::from_micros(100));
                });
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
        flag.stopped()
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
        SEARCH_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        let r = self.best_move_valued_inner(b, depth);
        SEARCH_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        r
    }

    fn best_move_valued_inner(&mut self, b: &Board, depth: u32) -> (Option<Position>, f32) {
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
        let n = self.ordered_into(b, acc, 1, b.movable(), &mut kids, false, 64, 64);
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
    /// The shape matters more than the numbers. *Every* move is scored
    /// as a weighted sum and the table move wins on a huge constant;
    /// the ordering is never skipped. And the two
    /// signals are weighted differently by node type: at a PV node the
    /// evaluation dominates (269 against 35 for mobility), at a null-window
    /// node it barely counts (7 against 17). Null-window nodes
    /// are most of the tree, so most of the tree is ordered by mobility.
    ///
    /// That split matters all the more with this evaluator: a full NNUE eval
    /// per child is by far the most expensive thing this search does — it is
    /// not even counted as a node — where a pattern evaluator would pay only a
    /// table lookup. Ordering every null-window node by NNUE eval, which is
    /// what this used to do from remaining depth 3 upward, spends the budget in
    /// exactly the wrong place.
    ///
    /// Mobility is weighted moves (moves counted double,
    /// corner moves once more) plus potential mobility (empties adjacent
    /// to opponent discs) at PV nodes only, matching which weights are non-zero
    /// in each variant.
    ///
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
        null_window: bool,
        tt_move: u8,
        tt_move2: u8,
    ) -> usize {
        let mover = b.player();
        // Ordering weights, scaled so the eval term stays in disc units.
        let (w_mob, w_pm, w_val) = if null_window {
            (17.0 / 7.0, 0.0, 1.0)
        } else {
            (35.0 / 269.0, 17.0 / 269.0, 1.0)
        };
        let eval_order = depth >= eval_order_depth();
        let mut n = 0usize;
        let mut m = moves;
        while m != 0 {
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            m &= m - 1;
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            // The table's moves are
            // scored so far above everything else that the sort puts them first
            // and second. Doing it with the score rather than by swapping them
            // to the front matters — two swaps do not land the runner-up in
            // second place, they displace whatever the ordering had chosen.
            let key = if pos.index() as u8 == tt_move {
                1.0e9
            } else if pos.index() as u8 == tt_move2 {
                1.0e8
            } else {
                let legal = nb.movable();
                // Offset minus weighted moves, so fewer replies scores higher.
                let mob = 38.0
                    - (legal.count_ones() * 2 + (legal & CORNERS).count_ones()) as f32;
                let mut k = mob * w_mob;
                if w_pm != 0.0 {
                    let empties = !(nb.black | nb.white);
                    k += (38.0 - potential_mobility(nb.opponent_bb(), empties) as f32) * w_pm;
                }
                if eval_order {
                    self.nn.ix_apply(acc, pos, flipped, mover);
                    k += -self.nn.eval_from_indices(acc, &nb) * w_val;
                    self.nn.ix_undo(acc, pos, flipped, mover);
                }
                k
            };
            out[n] = (pos, flipped, key);
            n += 1;
        }
        out[..n].sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
        n
    }

    /// Depth-1 null-window fast path. No table, no move list, no ordering
    /// pass — the children are leaves, so their eval *is* the ordering.
    fn eval1_nws(&mut self, b: &Board, acc: &mut PatternIndices, alpha: f32) -> f32 {
        self.nodes += 1;
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
            let raw = self.eval1_nws(&nb, acc, -alpha - PVS_EPS);
            return -raw;
        }
        let mover = b.player();
        let mut v = f32::NEG_INFINITY;
        for mask in STATIC_CELL_PRIORITY {
            let mut l = moves & mask;
            while l != 0 {
                let pos = Position::from_index(l.trailing_zeros()).unwrap();
                l &= l - 1;
                let mut nb = *b;
                let flipped = nb.make_move_bits(pos);
                self.nodes += 1;
                self.nn.ix_apply(acc, pos, flipped, mover);
                let g = -self.nn.eval_from_indices(acc, &nb);
                self.nn.ix_undo(acc, pos, flipped, mover);
                if g > v {
                    if g > alpha {
                        return g;
                    }
                    v = g;
                }
            }
        }
        v
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

        // Depth-1 fast path: a null-window node one ply from
        // the leaves touches no transposition table and builds no move list. It
        // walks the legal moves in static square-priority order and returns the
        // moment one beats alpha.
        //
        // These are the most numerous interior nodes in the tree, and they sit
        // below the depth where a subtree can be split — so the two probes into
        // a 400 MB table that the general path does here are paid entirely out
        // of the sequential part of the search.
        if depth == 1 && beta - alpha <= PVS_EPS * 1.5 {
            return self.eval1_nws(b, acc, alpha);
        }

        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        // The window this node was *asked* about. The table may narrow the
        // working window below, but what gets registered has to be measured
        // against this one.
        let (first_alpha, first_beta) = (alpha, beta);
        let mut beta = beta;
        let mut tt_move = 64u8;
        let mut tt_move2 = 64u8;
        {
            let e = self.tt.get(h);
            if e.key == h && e.flag != 0 {
                // The stored moves are always worth having for ordering; the
                // stored *bounds* only if the entry was searched at least as
                // deep and at least as accurately as this node needs.
                tt_move = e.best;
                tt_move2 = e.best2;
                if e.depth as u32 >= depth && e.relax >= self.mpc_relax as u8 {
                    // Transposition cutoff.
                    if e.upper <= alpha || e.upper == e.lower {
                        return e.upper;
                    }
                    if beta <= e.lower {
                        return e.lower;
                    }
                    // Narrow the working window to what the table already
                    // knows. Measured worth having (36.1M nodes against 37.2M
                    // without) even though ProbCut makes the stored bounds
                    // probabilistic rather than sound.
                    if alpha < e.lower {
                        alpha = e.lower;
                    }
                    if e.upper < beta {
                        beta = e.upper;
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

        // Which of the two ordering weight sets to use. Null-window nodes
        // are most of the tree and get the cheap one.
        let mover = b.player();
        let mut best = f32::NEG_INFINITY;
        let mut best_move = 64u8;
        let mut moves = moves;

        // Search the table's move on its own *before* the ordering
        // pass. If it cuts, the pass never
        // happens — and here that pass costs a full NNUE evaluation per child,
        // so on a cut node it is the single most expensive thing avoided. About
        // half the interior nodes of an alpha-beta tree are cut nodes.
        let mut pre_searched = false;
        if tt_move < 64 && depth >= 2 && moves.count_ones() > 1 && moves >> tt_move & 1 == 1 {
            let pos = Position::from_index(tt_move as u32).unwrap();
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            self.nn.ix_apply(acc, pos, flipped, mover);
            let raw = self.negamax(&nb, acc, depth - 1, -beta, -alpha);
            self.nn.ix_undo(acc, pos, flipped, mover);
            if raw == ABORTED {
                return ABORTED;
            }
            best = -raw;
            best_move = tt_move;
            if best > alpha {
                alpha = best;
            }
            moves &= !(1u64 << tt_move);
            if alpha >= beta || moves == 0 {
                self.tt.put(
                    h,
                    depth as u8,
                    self.mpc_relax as u8,
                    first_alpha,
                    first_beta,
                    best,
                    best_move,
                );
                return best;
            }
            pre_searched = true;
        }
        // After that search alpha may have moved, and the updated alpha is
        // what decides the weight set.
        let null_window = beta - alpha <= PVS_EPS * 1.5;

        // Score every move, and let the table
        // move win on a constant rather than skip the scoring.
        // Skipping it — which is what this used to do whenever the table had a
        // move — left every child but the first in bit order, and those are
        // exactly the young brothers the pool searches. An unordered young
        // brother that beats alpha stops the fan-out and forces a re-search of
        // everything behind it.
        //
        // The work is also skipped when there is nothing to order
        // (a single legal move).
        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0.0); MAX_KIDS];
        let will_split = self.pool.is_some() && depth >= ybwc_min_depth();
        let ordered = depth >= 2 && moves.count_ones() > 1;
        let n_kids = if ordered {
            self.ordered_into(b, acc, depth, moves, &mut kids, null_window, 64, tt_move2)
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
        // The scored path already put the table moves first; the
        // unscored path (one legal move, or depth below 2) still needs the swap.
        if !ordered && tt_move < 64 {
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
        // about to make unnecessary.
        //
        // Every non-eldest child is searched with a null window whatever kind
        // of node its parent is, so PV nodes have young brothers to give away
        // too — and being at the top of the tree, theirs are the largest
        // subtrees there are. Both kinds split here;
        // restricting this to null-window *parents*
        // excluded the whole PV spine and left the workers idle 68% of the
        // time.
        //
        // Splits start at `ybwc_min_depth`; below that a
        // subtree is cheaper than the handoff.
        let split_ok = will_split;

        let mut aborted = false;
        // Which children already have a final answer.
        let mut settled = [false; MAX_KIDS];
        let mut n_settled = 0usize;
        // The table move already fixed the window, so the rest are young
        // brothers even though none of them has been searched here yet.
        let mut first = !pre_searched;

        // Fan the young brothers out
        // against the alpha in force *now*, and the moment one of them beats
        // it, stop the fan-out, re-search the winners with the real window, and
        // fan the rest out again against the improved alpha.
        //
        // Letting the fan-out run to completion instead — which is what this
        // used to do — leaves every sibling searching against an alpha that is
        // known to be too low, and the extra nodes are what eats the parallel
        // gain (1.87x the sequential node count at 6 threads).
        // Only split-eligible nodes ever swap `self.abort`, so only they need a
        // copy of the enclosing chain. Cloning it at *every* interior node put
        // an atomic increment on one shared refcount in the hot path — and
        // under six threads that line is being incremented by all of them.
        let outer = if split_ok { self.abort.clone() } else { None };
        'fanout: loop {
            // The fan-out flag, with the polarity this codebase uses
            // for aborts: raised by whoever beats alpha, which ends this
            // fan-out (not the node). It is handed to the tasks as their abort
            // flag, so it *must* start clear — initialising it the other way
            // round made every task stop the instant it started, leaving the
            // node to be decided by its eldest child alone (28% in self-play).
            //
            // Only nodes that can actually split get one. The flag is heap
            // allocated, and allocating it at every interior node cost 13% of
            // single-thread throughput for a flag nobody could ever raise.
            let fan_stop = if split_ok { Some(AbortChain::child(outer.clone())) } else { None };
            // The loop control is a plain local, never the shared flag: a node
            // that cannot split has no flag, and gating the break on the flag
            // meant a fail-high stopped nothing — every remaining brother was
            // still probed against an alpha already known to be too low. That
            // is the fan-out flag losing its only job, and it cost 4x
            // the nodes single-threaded.
            let mut stop_fanout = false;
            let mut pending: Vec<(usize, std::sync::Arc<Slot>)> = Vec::new();
            let mut research: Vec<usize> = Vec::new();
            let mut next_alpha = alpha;

            for i in 0..n_kids {
                if settled[i] {
                    continue;
                }
                if stop_fanout || fan_stop.as_ref().is_some_and(|f| f.stopped()) {
                    break;
                }
                let (pos, flipped, _) = kids[i];
                // The flips are already known from the ordering pass; applying
                // them directly avoids recomputing a full flip per child.
                let mut nb = *b;
                nb.apply_flips(pos, flipped);

                // The eldest brother fixes the window before anything is given
                // away, and is always searched here with the full window.
                if first {
                    self.nn.ix_apply(acc, pos, flipped, mover);
                    let raw = self.negamax(&nb, acc, depth - 1, -beta, -alpha);
                    self.nn.ix_undo(acc, pos, flipped, mover);
                    // Inspect the child's own return value: negating it would
                    // turn the ABORTED sentinel into +inf and hide the abort.
                    if raw == ABORTED {
                        aborted = true;
                        break;
                    }
                    first = false;
                    settled[i] = true;
                    n_settled += 1;
                    let v = -raw;
                    if v > best {
                        best = v;
                        best_move = pos.index() as u8;
                    }
                    if best > alpha {
                        alpha = best;
                        next_alpha = alpha;
                    }
                    if alpha >= beta {
                        break;
                    }
                    continue;
                }

                // The last unsettled young brother is never split, it is what
                // the splitting thread does while the others run.
                let is_last = (i + 1..n_kids).all(|j| settled[j]);
                if split_ok && !is_last {
                    let pool = self.pool.unwrap();
                    let slot = std::sync::Arc::new(Slot::new());
                    let (nn, tt, mpc, relax) = (self.nn, self.tt, self.mpc, self.mpc_relax);
                    let (gen, my_gen) = (self.done.clone(), self.my_gen);
                    let stop = fan_stop.clone().unwrap();
                    let (task_slot, child, a, d) = (slot.clone(), nb, alpha, depth - 1);
                    let pushed = pool.try_push(move || {
                        // Check before doing anything. A task that waited in the
                        // queue may have been made irrelevant while it sat
                        // there — a brother beat alpha and the fan-out stopped —
                        // and the periodic check inside the search only fires
                        // every ABORT_CHECK_INTERVAL nodes, so without this the
                        // task searches half a thousand nodes of a tree nobody
                        // will read. That waste is exactly what made a queue
                        // unprofitable: nodes grew 30% at a queue depth of 32.
                        if stop.stopped() {
                            task_slot.set(ABORTED, 0);
                            return;
                        }
                        let mut w = NnueSearch::new(nn, tt);
                        w.mpc = mpc;
                        w.mpc_relax = relax;
                        w.pool = Some(pool);
                        // The fan-out flag doubles as the task's abort signal:
                        // once someone has beaten alpha every other subtree is
                        // about to be restarted against a better window.
                        w.abort = Some(stop.clone());
                        // Look at the flag on the first node too, not only after
                        // the first full interval.
                        w.abort_countdown = 1;
                        w.done = gen;
                        w.my_gen = my_gen;
                        let mut cacc = w.nn.indices(child.black, child.white);
                        let v = w.negamax(&child, &mut cacc, d, -(a + PVS_EPS), -a);
                        if v != ABORTED && -v > a {
                            stop.raise();
                        }
                        task_slot.set(v, w.nodes);
                    });
                    if pushed {
                        pending.push((i, slot));
                        continue;
                    }
                }

                // No worker free: search this young brother here. It runs under
                // the fan-out flag — so a sibling task that beats
                // alpha cuts this search short too instead of letting the
                // splitting thread finish a probe against a stale window.
                if split_ok {
                    self.abort = fan_stop.clone();
                }
                self.nn.ix_apply(acc, pos, flipped, mover);
                let raw = self.negamax(&nb, acc, depth - 1, -(alpha + PVS_EPS), -alpha);
                self.nn.ix_undo(acc, pos, flipped, mover);
                if split_ok {
                    self.abort = outer.clone();
                }
                if raw == ABORTED {
                    // Told to stop by an ancestor: the node is dead. Told to
                    // stop by *this* fan-out: only the probe is dead, and the
                    // move stays unsettled for the next round. A node that
                    // cannot split has no fan-out of its own, so any abort
                    // reaching it came from above.
                    if !split_ok || outer.as_ref().is_some_and(|o| o.stopped()) {
                        aborted = true;
                    }
                    break;
                }
                let g = -raw;
                if g > best {
                    best = g;
                    best_move = pos.index() as u8;
                }
                if g > alpha {
                    next_alpha = next_alpha.max(g);
                    stop_fanout = true;
                    if let Some(f) = &fan_stop {
                        f.raise();
                    }
                    research.push(i);
                } else {
                    settled[i] = true;
                    n_settled += 1;
                }
            }

            // Collect this fan-out. Tasks that were stopped stay unsettled and
            // go into the next round against the improved alpha.
            for (i, slot) in pending {
                self.pool.unwrap().wait(&slot);
                self.nodes += slot.nodes.load(std::sync::atomic::Ordering::Relaxed);
                let raw = f32::from_bits(slot.bits.load(std::sync::atomic::Ordering::Relaxed));
                if raw == ABORTED || aborted {
                    continue;
                }
                let g = -raw;
                if g > best {
                    best = g;
                    best_move = kids[i].0.index() as u8;
                }
                if g > alpha {
                    next_alpha = next_alpha.max(g);
                    research.push(i);
                } else {
                    settled[i] = true;
                    n_settled += 1;
                }
            }

            // An ancestor cut off while this fan-out was running: every task
            // came back ABORTED, so there is nothing to restart and nothing
            // worth storing.
            if outer.as_ref().is_some_and(|o| o.stopped()) {
                aborted = true;
            }
            if aborted {
                break 'fanout;
            }
            if research.is_empty() {
                // Nobody beat alpha: either every child is settled, or the
                // eldest already produced a cutoff.
                break 'fanout;
            }
            if next_alpha >= beta {
                break 'fanout;
            }

            // A null-window probe that beat its alpha is only a lower bound, so
            // the winners are re-searched with the real window before the rest
            // are fanned out again.
            alpha = next_alpha;
            for &i in &research {
                let (pos, flipped, _) = kids[i];
                let mut nb = *b;
                nb.apply_flips(pos, flipped);
                self.nn.ix_apply(acc, pos, flipped, mover);
                let raw = self.negamax(&nb, acc, depth - 1, -beta, -alpha);
                self.nn.ix_undo(acc, pos, flipped, mover);
                if raw == ABORTED {
                    aborted = true;
                    break 'fanout;
                }
                settled[i] = true;
                n_settled += 1;
                let g = -raw;
                if g > best {
                    best = g;
                    best_move = pos.index() as u8;
                }
                if best > alpha {
                    alpha = best;
                    if alpha >= beta {
                        break;
                    }
                }
            }
            if alpha >= beta || n_settled == n_kids {
                break 'fanout;
            }
        }

        if aborted {
            // A truncated subtree carries no value. Returning here — and
            // crucially *not* writing the table — keeps the abort from
            // poisoning entries that other threads will trust. (Storing it
            // was measured as a total collapse: 0% score, -59 discs.)
            return ABORTED;
        }

        self.tt.put(
            h,
            depth as u8,
            self.mpc_relax as u8,
            first_alpha,
            first_beta,
            best,
            best_move,
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
    band_width: u8,
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
        let mut clear_ns: u64 = 0;
        // Which branch took the move, so the clock can split midgame from
        // endgame. Only the NNUE path used to report this, which made every
        // pattern-evaluator match read as "100% endgame, 0 nodes".
        let mut mid_nodes: Option<u64> = None;
        let t0 = std::time::Instant::now();
        let pos = if our_turn {
            if solve_empties > 0 && board.empty_count() <= solve_empties {
                // The solver empties its table on entry; that is harness
                // scaffolding, not thinking, and it is not in what Egaroucid
                // reports either. Subtract it, the way `solve_obf` does.
                let c0 = kuroobi::solver::CLEAR_NS
                    .load(std::sync::atomic::Ordering::Relaxed);
                let mv = solver
                    .solve_with_eval(EndSolverMode::Perfect, &board, Some(evaluator))
                    .best_move
                    .ok_or("solver returned no move")?;
                clear_ns += kuroobi::solver::CLEAR_NS
                    .load(std::sync::atomic::Ordering::Relaxed)
                    - c0;
                mv
            } else if let Some(t) = selective_band(board.empty_count(), solve_empties, band_width) {
                // Above the exact threshold but close enough that reading to
                // the end selectively beats a fixed-depth midgame search: the
                // selective band, where the search depth becomes the number of
                // empties rather than a fixed midgame lookahead.
                //
                // This clears the table on entry exactly as the exact solve
                // does, so it has to discount it exactly as the exact solve
                // does — charging scaffolding to one branch and not the other
                // is how a comparison between them stops meaning anything.
                let c0 = kuroobi::solver::CLEAR_NS
                    .load(std::sync::atomic::Ordering::Relaxed);
                let mv = solver
                    .solve_selective(&board, Some(evaluator), t)
                    .best_move
                    .ok_or("selective solver returned no move")?;
                clear_ns += kuroobi::solver::CLEAR_NS
                    .load(std::sync::atomic::Ordering::Relaxed)
                    - c0;
                mv
            } else if let Some(nn) = nnue.as_deref_mut() {
                let p = nn.best_move(&board, depth as u32).ok_or("nnue returned no move")?;
                mid_nodes = Some(std::mem::take(&mut nn.nodes));
                p
            } else {
                let r = searcher.search(&board, evaluator, depth);
                mid_nodes = Some(r.nodes);
                r.best_move.ok_or("searcher returned no move")?
            }
        } else {
            match edax.best_move(&board) {
                Ok(Some(p)) => p,
                Ok(None) => return Err("edax passed although moves exist".into()),
                Err(e) => return Err(format!("edax io error: {e}")),
            }
        };

        let dt = t0.elapsed().as_secs_f64() - std::mem::take(&mut clear_ns) as f64 / 1e9;
        // Bucketed by the empties the mover faced, which is what selects the
        // regime on both sides.
        let e = board.empty_count() as usize;
        if our_turn {
            clock.ours += dt;
            clock.our_moves += 1;
            clock.our_by_empties[e] += dt;
            clock.our_n_by_empties[e] += 1;
            if let Some(n) = mid_nodes {
                clock.our_nodes += n;
                clock.our_mid += dt;
            }
        } else {
            clock.edax += dt;
            clock.edax_moves += 1;
            clock.edax_by_empties[e] += dt;
            clock.edax_n_by_empties[e] += 1;
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

    // ProbCut calibration for the NNUE, the same shape `mpccalib` produces for
    // the linear evaluator. The margins the search prunes against are
    // `t * sigma(empties, depth, probe_depth)`, and sigma has to be fitted
    // against the evaluator actually doing the searching: a model borrowed from
    // a *less* accurate evaluator overestimates the error, widens every margin
    // and leaves the tree bigger than it needs to be.
    if let Some(path) = &args.mpc_calib {
        let Some(nn) = nnue else {
            eprintln!("--mpc-calib needs --nnue");
            return ExitCode::FAILURE;
        };
        let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(22)));
        let examples = match kuroobi::trainer::load_examples_binary(path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("failed to load {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        const CAL_DEPTHS: [u32; 7] = [1, 2, 4, 6, 8, 10, 12];
        print!("empties");
        for d in CAL_DEPTHS {
            print!(",d{d}");
        }
        println!();
        let mut n = 0usize;
        for ex in examples.iter().step_by(args.calib_stride) {
            let board = ex.board();
            let empties = board.empty_count() as u32;
            if !(8..=45).contains(&empties) || board.movable() == 0 {
                continue;
            }
            let mut row = format!("{empties}");
            for d in CAL_DEPTHS {
                let mut w = NnueSearch::new(nn, tt);
                // Calibration measures the *unpruned* error, so MPC stays off:
                // sigma fitted against an already-pruned search would fold the
                // pruning error back into the margin that controls it.
                w.mpc = false;
                tt.clear();
                let (_, v) = w.best_move_valued(&board, d);
                row.push_str(&format!(",{v:.3}"));
            }
            println!("{row}");
            n += 1;
            if n >= args.calib_max {
                break;
            }
        }
        eprintln!("{n} positions calibrated");
        return ExitCode::SUCCESS;
    }

    // Positions this engine actually reaches at a chosen empty count, written as
    // 65-character records Egaroucid's `-solve` accepts. Both engines then answer
    // the *same* questions, which is the only way a node count means anything.
    if let Some(path) = &args.gen_obf {
        let Some(search) = nnue_search.as_mut() else {
            eprintln!("--gen-obf needs --nnue");
            return ExitCode::FAILURE;
        };
        search.threads = 1; // reproducible: see the note in --band-probe
        let target = args.band_empties;
        let mut rng = Rng::new(args.seed);
        let mut out = String::new();
        let mut done = 0usize;
        while done < args.band_probe.unwrap_or(20) {
            let (mut board, _) = random_opening(&mut rng, args.random_plies);
            let mut usable = true;
            while board.empty_count() > target {
                if board.movable() == 0 {
                    let mut passed = board;
                    passed.pass();
                    if passed.movable() == 0 {
                        usable = false;
                        break;
                    }
                    board = passed;
                    continue;
                }
                match search.best_move(&board, args.depth as u32) {
                    Some(m) => {
                        board.make_move_bits(m);
                    }
                    None => {
                        usable = false;
                        break;
                    }
                }
            }
            if !usable || board.empty_count() != target || board.movable() == 0 {
                continue;
            }
            // Exactly 65 characters, no trailing punctuation: Egaroucid's
            // `setboard` rejects anything longer ("expected 65") and then
            // searches whatever board it still had, which looks like a
            // successful run that answered a different question.
            out.push_str(&format!("{board}\n"));
            done += 1;
        }
        if let Err(e) = std::fs::write(path, out) {
            eprintln!("failed to write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        eprintln!("{done} positions at {target} empties -> {}", path.display());
        return ExitCode::SUCCESS;
    }

    // Sigma for the *endgame* ProbCut, measured instead of extrapolated: for
    // each position, the exact value and what a probe of each depth reported.
    // The spread of `exact - probe` per (empties, probe depth) cell is the
    // standard deviation the margin should be built from.
    if let Some(path) = &args.sigma_calib {
        {
            let text = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("failed to read {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            let mut solver = Solver::new(26);
            solver.set_threads(args.threads);
            const PROBES: [u8; 5] = [2, 4, 6, 8, 10];
            print!("empties,exact");
            for d in PROBES {
                print!(",probe{d}");
            }
            println!();
            for line in text.lines() {
                let Some(board) = parse_obf(line) else { continue };
                if board.movable() == 0 {
                    continue;
                }
                // Probes first: an exact solve leaves entries the probe would
                // read back as truth, which would make it look far better than
                // it is.
                let probes: Vec<f32> = PROBES
                    .iter()
                    .map(|&d| solver.probe_value(&board, &evaluator, d))
                    .collect();
                let exact =
                    solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(&evaluator)).value;
                print!("{},{exact}", board.empty_count());
                for v in probes {
                    print!(",{v:.2}");
                }
                println!();
            }
            return ExitCode::SUCCESS;
        }
    }

    // Answer each position in a file with whatever the game path would use for
    // it, and report *only* what the search did: nodes, search seconds, nps.
    // Table clears are scaffolding and are subtracted; nothing is summed across
    // a game, and no position is averaged with a position of another regime.
    if let Some(path) = &args.obf {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("failed to read {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        // The head-to-head opponent runs with a 2^25-entry table; measuring
        // our endgame through a 2^22 table conflates tree size with table
        // starvation.
        let mut solver = Solver::new(args.solver_hash.unwrap_or(if args.solve_empties >= 18 { 22 } else { 18 }) as u32);
        solver.set_threads(args.threads);
        println!("empties,regime,depth,nodes,seconds,nps");
        for line in text.lines() {
            let Some(board) = parse_obf(line) else { continue };
            let e = board.empty_count();
            if board.movable() == 0 {
                continue;
            }
            // Egaroucid's `-solve` clears its cache between problems unless told
            // not to, so this does too: a position that inherits the previous
            // one's table is not answering the same question.
            if let Some(tt) = nnue_tt {
                tt.clear();
            }
            let c0 = kuroobi::solver::CLEAR_NS
                .load(std::sync::atomic::Ordering::Relaxed);
            let t0 = std::time::Instant::now();
            let (regime, depth, nodes) =
                if args.solve_empties > 0 && e <= args.solve_empties {
                    let r = solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(&evaluator));
                    ("exact", e as u32, r.nodes)
                } else if let Some(t) = selective_band(e, args.solve_empties, args.selective_band) {
                    let r = solver.solve_selective(&board, Some(&evaluator), t);
                    ("selective", e as u32, r.nodes)
                } else {
                    let Some(nn) = nnue_search.as_mut() else {
                        eprintln!("--obf needs --nnue for midgame positions");
                        return ExitCode::FAILURE;
                    };
                    nn.best_move(&board, args.depth as u32);
                    ("midgame", args.depth as u32, std::mem::take(&mut nn.nodes))
                };
            let clear = (kuroobi::solver::CLEAR_NS
                .load(std::sync::atomic::Ordering::Relaxed)
                - c0) as f64
                / 1e9;
            let secs = t0.elapsed().as_secs_f64() - clear;
            println!(
                "{e},{regime},{depth},{nodes},{secs:.4},{:.0}",
                nodes as f64 / secs.max(1e-9)
            );
        }
        return ExitCode::SUCCESS;
    }

    // Is the selective band actually worse than the midgame search it replaces?
    // A 100-game match answered that with a +-10 point interval — wider than any
    // effect worth keeping, so the earlier rejection proved nothing. Scoring both
    // candidate moves against an exact solve measures the same question with far
    // less variance: every position yields a disc-difference loss instead of a
    // coin flip, and the two branches are scored on the *same* position, so the
    // opening noise cancels.
    if let Some(n_pos) = args.band_probe {
        if nnue_search.is_none() {
            eprintln!("--band-probe needs --nnue");
            return ExitCode::FAILURE;
        }
        let target = args.band_empties;
        let Some(t) = selective_band(target, args.solve_empties, 6) else {
            eprintln!(
                "--band-empties {target} is not in the band above --solve-empties {}",
                args.solve_empties
            );
            return ExitCode::FAILURE;
        };
        let search = nnue_search.as_mut().unwrap();
        // 2^26 rather than the 2^22 a game uses: this solver is the oracle, not
        // the engine under test, and at 29 empties a 134 MB table is so far past
        // saturation that a position takes minutes. The answer is exact either
        // way — only the time to reach it changes.
        let mut solver = Solver::new(26);
        solver.set_threads(args.threads);
        let mut rng = Rng::new(args.seed);

        /// Exact value of `m` from the point of view of the side playing it.
        /// The solver always answers for the side to move, so every pass on the
        /// way back flips the sign.
        fn exact_after(
            solver: &mut Solver,
            ev: &Evaluator,
            board: &Board,
            m: Position,
        ) -> i32 {
            let mut child = *board;
            child.make_move_bits(m);
            let mut sign = -1i32;
            loop {
                if child.movable() != 0 {
                    let r = solver.solve_with_eval(EndSolverMode::Perfect, &child, Some(ev));
                    return sign * r.value;
                }
                let mut passed = child;
                passed.pass();
                if passed.movable() == 0 {
                    let (p, o) = (
                        child.player_bb().count_ones() as i32,
                        child.opponent_bb().count_ones() as i32,
                    );
                    let empty = 64 - p - o;
                    let diff = p - o;
                    let final_score = match diff.cmp(&0) {
                        std::cmp::Ordering::Greater => diff + empty,
                        std::cmp::Ordering::Less => diff - empty,
                        std::cmp::Ordering::Equal => 0,
                    };
                    return sign * final_score;
                }
                child = passed;
                sign = -sign;
            }
        }

        println!("band probe: {n_pos} positions at {target} empties, t={t}, depth {}", args.depth);
        // The move a probe shape picks is a coarse instrument: the blow-ups that
        // decide whether the band is usable are rare enough that a hundred
        // positions can contain none. The *score* a selective solve returns is
        // available on every position and says directly how far the probes let
        // it drift, so it separates the two shapes with far more resolution.
        println!("pos,exact,flat_loss,scaled_loss,mid_loss,flat_err,scaled_err");
        let (mut sum_a, mut sum_s, mut sum_b) = (0i64, 0i64, 0i64);
        let (mut bad_a, mut bad_s, mut bad_b, mut agree) = (0usize, 0usize, 0usize, 0usize);
        let mut done = 0usize;
        while done < n_pos {
            // Play down to the band with the same search that would be making
            // these moves in a game, so the positions are ones this engine
            // actually reaches — random play down to 27 empties is a different
            // distribution and would not answer the question asked.
            //
            // Single-threaded, unlike everything measured below it. A parallel
            // search is not reproducible, and one move decided differently
            // early puts every later position on a different line: two runs of
            // this probe then score different position sets and cannot be
            // compared. Costs a few seconds a position and makes the seed mean
            // what it says.
            let played_threads = std::mem::replace(&mut search.threads, 1);
            let (mut board, _) = random_opening(&mut rng, args.random_plies);
            let mut usable = true;
            while board.empty_count() > target {
                if board.movable() == 0 {
                    let mut passed = board;
                    passed.pass();
                    if passed.movable() == 0 {
                        usable = false;
                        break;
                    }
                    board = passed;
                    continue;
                }
                match search.best_move(&board, args.depth as u32) {
                    Some(m) => {
                        board.make_move_bits(m);
                    }
                    None => {
                        usable = false;
                        break;
                    }
                }
            }
            search.threads = played_threads;
            if !usable || board.empty_count() != target || board.movable() == 0 {
                continue;
            }

            let exact = solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(&evaluator));
            let Some(best) = exact.best_move else { continue };
            let star = exact.value;

            // Both probe shapes on the same position: the flat depth-4 probe
            // this ships with, and the depth-scaled `depth/3` probe. What
            // separates them is not the average — it is how often the probe
            // blows up.
            kuroobi::solver::set_selective_probe_scaled(false);
            let ra = solver.solve_selective(&board, Some(&evaluator), t);
            kuroobi::solver::set_selective_probe_scaled(true);
            let rs = solver.solve_selective(&board, Some(&evaluator), t);
            let (a, s) = (ra.best_move, rs.best_move);
            let (err_a, err_s) = (ra.value - star, rs.value - star);
            let b = search.best_move(&board, args.depth as u32);

            let (Some(a), Some(s), Some(b)) = (a, s, b) else { continue };
            // Each of these solves costs what the oracle did, so a move already
            // scored is never scored twice.
            let mut scored: Vec<(Position, i32)> = Vec::new();
            let mut loss_of = |m: Position, solver: &mut Solver| -> i32 {
                if m == best {
                    return 0;
                }
                if let Some((_, l)) = scored.iter().find(|(p, _)| *p == m) {
                    return *l;
                }
                let l = star - exact_after(solver, &evaluator, &board, m);
                scored.push((m, l));
                l
            };
            let loss_a = loss_of(a, &mut solver);
            let loss_s = loss_of(s, &mut solver);
            let loss_b = loss_of(b, &mut solver);

            sum_a += loss_a as i64;
            sum_s += loss_s as i64;
            sum_b += loss_b as i64;
            bad_a += (loss_a > 0) as usize;
            bad_s += (loss_s > 0) as usize;
            bad_b += (loss_b > 0) as usize;
            agree += (a == s) as usize;
            done += 1;
            println!("{done},{star},{loss_a},{loss_s},{loss_b},{err_a},{err_s}");
        }
        let n = done as f64;
        println!(
            "band flat  : mean loss {:.3}, wrong move {bad_a}/{done}",
            sum_a as f64 / n
        );
        println!(
            "band scaled: mean loss {:.3}, wrong move {bad_s}/{done}",
            sum_s as f64 / n
        );
        println!(
            "midgame    : mean loss {:.3}, wrong move {bad_b}/{done}",
            sum_b as f64 / n
        );
        println!("flat and scaled agree {agree}/{done}");
        return ExitCode::SUCCESS;
    }

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

    let mut edax = match Edax::spawn(&args.edax_path, args.edax_level, args.edax_threads.unwrap_or(args.threads), args.protocol) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to start edax: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "lab: us={} (depth {}, solve {}) vs Edax level {}  ({} games, {} random plies, {} threads)",
        // The midgame runs on the NNUE when one is given, so naming the linear
        // file here mislabelled every NNUE match as a pattern-evaluator one.
        args.nnue.as_ref().unwrap_or(&args.weights).display(),
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
                &mut edax, args.depth, args.solve_empties, args.selective_band, &mut clock, &opening_moves,
            ) {
                Ok(s) => {
                    // One line per game so two runs over the same seed can be
                    // compared game by game. Comparing only the two win rates
                    // throws away the pairing: the openings are identical, so
                    // the per-game difference has far less variance than two
                    // independent proportions do.
                    if args.per_game {
                        println!("game {pair} {} {s}", if we_are_black { 'B' } else { 'W' });
                    }
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
    {
        use std::sync::atomic::Ordering::Relaxed;
        let workers = POOL_WORKERS.load(Relaxed).max(1) as f64;
        let mid_ns = clock.our_mid * 1e9;
        let busy = WORKER_BUSY_NS.load(Relaxed) as f64;
        let wmain = WAIT_IDLE_MAIN_NS.load(Relaxed) as f64;
        let wwork = WAIT_IDLE_WORKER_NS.load(Relaxed) as f64;
        println!(
            "our search: {} nodes, {:.2} Mnps, worker busy {:.1}%",
            clock.our_nodes,
            // Midgame nodes over *midgame* time. Dividing by `clock.ours`
            // mixed in the endgame solver's seconds, which contribute no nodes
            // to this counter, and understated the search rate by whatever
            // share the solver took (15% at depth 10 / solve 20) — enough to
            // read as a regression against a figure measured on the midgame
            // alone.
            clock.our_nodes as f64 / clock.our_mid.max(1e-9) / 1e6,
            busy / (mid_ns * workers) * 100.0
        );
        // Where the (1 + workers) x midgame-time budget actually goes. Anything
        // in "starved" is a core the split scheme paid for and left blocked on
        // a child with an empty queue; "endgame" is time the pool cannot help
        // with at all because the solver runs on its own.
        println!(
            "  midgame {:.1}s of {:.1}s thinking ({:.0}% endgame); \
             starved: main {:.1}s, workers {:.1}s of {:.1}s worker-time; \
             splits {}/{} pushed",
            clock.our_mid,
            clock.ours,
            (1.0 - clock.our_mid / clock.ours.max(1e-9)) * 100.0,
            wmain / 1e9,
            wwork / 1e9,
            mid_ns * workers / 1e9,
            SPLIT_DONE.load(Relaxed),
            SPLIT_TRIED.load(Relaxed),
        );
        let hist: Vec<u64> = BUSY_HIST.iter().map(|c| c.load(Relaxed)).collect();
        let total: u64 = hist.iter().sum();
        if total > 0 {
            let w = POOL_WORKERS.load(Relaxed).min(16);
            let shown: Vec<String> = (0..=w)
                .map(|k| format!("{k}:{:.0}%", hist[k] as f64 / total as f64 * 100.0))
                .collect();
            println!("  busy workers over time: {}", shown.join(" "));
        }
    }
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
    {
        // The bands are the regime boundaries, not round numbers: above the
        // selective band both engines run a fixed-depth midgame search, below
        // `solve_empties` both prove the result exactly, and in between only
        // the opponent reads to the end. Comparing anything coarser than this
        // averages the difference away.
        let hi = args.solve_empties.max(1) as usize;
        let bands: [(usize, usize, &str); 5] = [
            (hi + 7, 60, "midgame"),
            (hi + 5, hi + 6, "sel 93%"),
            (hi + 3, hi + 4, "sel 98%"),
            (hi + 1, hi + 2, "sel 99%"),
            (1, hi, "exact"),
        ];
        println!("time per move by empties (ours / edax):");
        for (lo, up, name) in bands {
            let (mut ot, mut on, mut et, mut en) = (0.0, 0u32, 0.0, 0u32);
            for e in lo..=up.min(64) {
                ot += clock.our_by_empties[e];
                on += clock.our_n_by_empties[e];
                et += clock.edax_by_empties[e];
                en += clock.edax_n_by_empties[e];
            }
            if on == 0 && en == 0 {
                continue;
            }
            let (o, d) = (ot / on.max(1) as f64, et / en.max(1) as f64);
            println!(
                "  {name:<8} empties {lo:>2}-{up:<2}  {o:.4}s ({on:>5} moves)  \
                 {d:.4}s ({en:>5} moves)  ratio {:.2}x",
                o / d.max(1e-9)
            );
        }
    }
    // No separate NNUE node line: the move loop drains `nn.nodes` into
    // `clock.our_nodes` every move, so reading it here always printed 0. The
    // running total is the `our search:` line above.
    ExitCode::SUCCESS
}
