//! Head-to-head match against an external engine.
//!
//! Three opponents are supported, each speaking its own dialect:
//!
//! * `edax`      — Edax's console: `setboard <board>` / `go`, replies "Edax plays XX"
//! * `zebra`     — the engine mode added to Zebra's sources for this driver,
//!   which answers `setboard`/`go` with "move xx"
//! * `egaroucid` — GTP, which Egaroucid implements natively: the driver
//!   replays the move list rather than setting a board, since
//!   GTP has no position command
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
//!   parallelises the exact endgame; Edax parallelises
//!   its whole search, so this favours Edax.
//!   --games <n>          Total games, rounded up to even (default 200)
//!   --random-plies <n>   Random opening plies (default 6)
//!   --seed <n>           RNG seed (default 7)
//!   --per-game           One line per game (`game <pair> <B|W> <disc-diff>`),
//!   so two runs over the same seed can be paired

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio};

use kuroobi::evaluator::Evaluator;
use kuroobi::midgame::*;
use kuroobi::nnue::Nnue;
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
    mid_sigma_calib: Option<PathBuf>,
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
        mid_sigma_calib: None,
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
            "--depth" => {
                args.depth = value("--depth")?
                    .parse()
                    .map_err(|e| format!("--depth: {e}"))?
            }
            "--mpc" => args.mpc = true,
            "--solve-empties" => {
                args.solve_empties = value("--solve-empties")?
                    .parse()
                    .map_err(|e| format!("--solve-empties: {e}"))?
            }
            "--edax-level" => {
                args.edax_level = value("--edax-level")?
                    .parse()
                    .map_err(|e| format!("--edax-level: {e}"))?
            }
            "--protocol" => {
                args.protocol = match value("--protocol")?.as_str() {
                    "edax" => "edax",
                    "zebra" => "zebra",
                    "egaroucid" | "gtp" => "egaroucid",
                    other => return Err(format!("--protocol: unknown {other}")),
                }
            }
            "--threads" => {
                args.threads = value("--threads")?
                    .parse()
                    .map_err(|e| format!("--threads: {e}"))?
            }
            "--per-game" => args.per_game = true,
            "--edax-threads" => {
                args.edax_threads = Some(
                    value("--edax-threads")?
                        .parse()
                        .map_err(|e| format!("--edax-threads: {e}"))?,
                )
            }
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
            "--seed" => {
                args.seed = value("--seed")?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            "--verify-parallel" => args.verify_parallel = true,
            "--mpc-calib" => args.mpc_calib = Some(PathBuf::from(value("--mpc-calib")?)),
            "--calib-stride" => {
                args.calib_stride = value("--calib-stride")?
                    .parse()
                    .map_err(|e| format!("--calib-stride: {e}"))?
            }
            "--calib-max" => {
                args.calib_max = value("--calib-max")?
                    .parse()
                    .map_err(|e| format!("--calib-max: {e}"))?
            }
            "--selective-band" => {
                args.selective_band = value("--selective-band")?
                    .parse()
                    .map_err(|e| format!("--selective-band: {e}"))?
            }
            "--self-vs" => {
                args.self_vs = Some(
                    value("--self-vs")?
                        .parse()
                        .map_err(|e| format!("--self-vs: {e}"))?,
                )
            }
            "--band-probe" => {
                args.band_probe = Some(
                    value("--band-probe")?
                        .parse()
                        .map_err(|e| format!("--band-probe: {e}"))?,
                )
            }
            "--mid-sigma-calib" => {
                args.mid_sigma_calib = Some(PathBuf::from(value("--mid-sigma-calib")?))
            }
            "--sigma-calib" => args.sigma_calib = Some(PathBuf::from(value("--sigma-calib")?)),
            "--solver-hash" => {
                args.solver_hash = Some(
                    value("--solver-hash")?
                        .parse()
                        .map_err(|e| format!("--solver-hash: {e}"))?,
                )
            }
            "--gen-obf" => args.gen_obf = Some(PathBuf::from(value("--gen-obf")?)),
            "--obf" => args.obf = Some(PathBuf::from(value("--obf")?)),
            "--band-empties" => {
                args.band_empties = value("--band-empties")?
                    .parse()
                    .map_err(|e| format!("--band-empties: {e}"))?
            }
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
        Ok(Edax {
            child,
            stdin,
            stdout,
            protocol,
            history: Vec::new(),
        })
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
        let color = if board.player() == Color::Black {
            "B"
        } else {
            "W"
        };
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
                let c0 = kuroobi::solver::CLEAR_NS.load(std::sync::atomic::Ordering::Relaxed);
                let mv = solver
                    .solve_with_eval(EndSolverMode::Perfect, &board, Some(evaluator))
                    .best_move
                    .ok_or("solver returned no move")?;
                clear_ns +=
                    kuroobi::solver::CLEAR_NS.load(std::sync::atomic::Ordering::Relaxed) - c0;
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
                let c0 = kuroobi::solver::CLEAR_NS.load(std::sync::atomic::Ordering::Relaxed);
                let mv = solver
                    .solve_selective(&board, Some(evaluator), t)
                    .best_move
                    .ok_or("selective solver returned no move")?;
                clear_ns +=
                    kuroobi::solver::CLEAR_NS.load(std::sync::atomic::Ordering::Relaxed) - c0;
                mv
            } else if let Some(nn) = nnue.as_deref_mut() {
                let p = nn
                    .best_move(&board, depth as u32)
                    .ok_or("nnue returned no move")?;
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
    Ok(if we_are_black {
        score_black
    } else {
        -score_black
    })
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
    // Midgame ProbCut sigma, measured instead of assumed: for positions the
    // NNUE search actually reaches, the error between the reduced-depth probe
    // and the full-depth search, per (empties, depth). The old model was a fit
    // for the *linear* evaluator carried over on the assumption that a more
    // accurate evaluator makes it conservative; the level-10-rig matches
    // measured that assumption to cost ~3.5 points of win rate.
    if let Some(path) = &args.mid_sigma_calib {
        let Some(search) = nnue_search.as_mut() else {
            eprintln!("--mid-sigma-calib needs --nnue");
            return ExitCode::FAILURE;
        };
        // Sequential and unpruned: the probe/full values must be the search's
        // own opinions, not MPC's, and parallel values are not reproducible.
        search.threads = 1;
        search.mpc = false;
        const MAX_CAL_DEPTH: u32 = 12;
        let n_games = args.band_probe.unwrap_or(40);
        let mut rng = Rng::new(args.seed);
        let mut out = String::from("empties,depth,pd,v_probe,v_full\n");
        let mut rows = 0usize;
        for game in 0..n_games {
            let (mut board, _) = random_opening(&mut rng, args.random_plies);
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
                let e = board.empty_count() as u32;
                if e < 10 {
                    break;
                }
                // Every second ply is plenty of positions and halves the cost.
                if e <= 50 && e.is_multiple_of(2) {
                    search.clear();
                    let mut acc = search.nn.indices(board.black, board.white);
                    let top = MAX_CAL_DEPTH.min(e.saturating_sub(2));
                    let mut vals = [f32::NAN; MAX_CAL_DEPTH as usize + 1];
                    for d in 1..=top {
                        vals[d as usize] =
                            search.negamax(&board, &mut acc, d, f32::NEG_INFINITY, f32::INFINITY);
                    }
                    for dd in mpc_min_depth()..=top {
                        let pd = mpc_reduced_depth(dd);
                        if pd >= 1 && pd < dd {
                            out.push_str(&format!(
                                "{e},{dd},{pd},{:.3},{:.3}\n",
                                vals[pd as usize], vals[dd as usize]
                            ));
                            rows += 1;
                        }
                    }
                }
                match search.best_move(&board, args.depth as u32) {
                    Some(m) => {
                        board.make_move_bits(m);
                    }
                    None => break,
                }
            }
            eprintln!("game {}/{n_games} ({rows} rows)", game + 1);
        }
        if let Err(e) = std::fs::write(path, out) {
            eprintln!("failed to write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        eprintln!("{rows} rows -> {}", path.display());
        return ExitCode::SUCCESS;
    }

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
            if let (Some(nn), Some(mtt)) = (nnue, nnue_tt) {
                solver.set_nnue(nn, mtt);
            }
            solver.set_threads(args.threads);
            const PROBES: [u8; 5] = [2, 4, 6, 8, 10];
            print!("empties,exact");
            for d in PROBES {
                print!(",probe{d}");
            }
            for d in PROBES {
                print!(",nprobe{d}");
            }
            println!();
            for line in text.lines() {
                let Some(board) = parse_obf(line) else {
                    continue;
                };
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
                let nprobes: Vec<f32> = PROBES
                    .iter()
                    .map(|&d| solver.probe_value_nnue(&board, d).unwrap_or(f32::NAN))
                    .collect();
                let exact = solver
                    .solve_with_eval(EndSolverMode::Perfect, &board, Some(&evaluator))
                    .value;
                print!("{},{exact}", board.empty_count());
                for v in probes {
                    print!(",{v:.2}");
                }
                for v in nprobes {
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
        let mut solver =
            Solver::new(
                args.solver_hash
                    .unwrap_or(if args.solve_empties >= 18 { 22 } else { 18 })
                    as u32,
            );
        if let (Some(nn), Some(mtt)) = (nnue, nnue_tt) {
            solver.set_nnue(nn, mtt);
        }
        solver.set_threads(args.threads);
        println!("empties,regime,depth,nodes,seconds,nps");
        for line in text.lines() {
            let Some(board) = parse_obf(line) else {
                continue;
            };
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
            let c0 = kuroobi::solver::CLEAR_NS.load(std::sync::atomic::Ordering::Relaxed);
            let t0 = std::time::Instant::now();
            let (regime, depth, nodes) = if args.solve_empties > 0 && e <= args.solve_empties {
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
            let clear = (kuroobi::solver::CLEAR_NS.load(std::sync::atomic::Ordering::Relaxed) - c0)
                as f64
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
        if let (Some(nn), Some(mtt)) = (nnue, nnue_tt) {
            solver.set_nnue(nn, mtt);
        }
        solver.set_threads(args.threads);
        let mut rng = Rng::new(args.seed);

        /// Exact value of `m` from the point of view of the side playing it.
        /// The solver always answers for the side to move, so every pass on the
        /// way back flips the sign.
        fn exact_after(solver: &mut Solver, ev: &Evaluator, board: &Board, m: Position) -> i32 {
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

        println!(
            "band probe: {n_pos} positions at {target} empties, t={t}, depth {}",
            args.depth
        );
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
            let Some(best) = exact.best_move else {
                continue;
            };
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

            let (Some(a), Some(s), Some(b)) = (a, s, b) else {
                continue;
            };
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
        if let (Some(nn), Some(mtt)) = (nnue, nnue_tt) {
            solver_a.set_nnue(nn, mtt);
        }
        solver_a.set_threads(args.threads);
        let mut solver_b = Solver::new(if args.solve_empties >= 18 { 22 } else { 18 });
        if let (Some(nn), Some(mtt)) = (nnue, nnue_tt) {
            solver_b.set_nnue(nn, mtt);
        }
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
                let diff = if a_is_black {
                    black - white
                } else {
                    white - black
                };
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
        println!(
            "time: A {time_a:.1}s  B {time_b:.1}s  (speedup {:.2}x)",
            time_b / time_a.max(1e-9)
        );
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
                    -j.negamax(
                        &child,
                        &mut ix,
                        args.depth as u32,
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                    )
                };
                let vs = value_of(seq_move, judge_tt);
                judge_tt = Box::leak(Box::new(SharedTt::new(20)));
                let vp = value_of(par_move, judge_tt);
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

    let mut edax = match Edax::spawn(
        &args.edax_path,
        args.edax_level,
        args.edax_threads.unwrap_or(args.threads),
        args.protocol,
    ) {
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
    if let (Some(nn), Some(mtt)) = (nnue, nnue_tt) {
        solver.set_nnue(nn, mtt);
    }
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
                &opening,
                we_are_black,
                &evaluator,
                nnue_search.as_mut(),
                &mut searcher,
                &mut solver,
                &mut edax,
                args.depth,
                args.solve_empties,
                args.selective_band,
                &mut clock,
                &opening_moves,
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
