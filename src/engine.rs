//! Engine session layer shared by the GUI, CLI and protocol frontends.
//!
//! Bundles the same selection policy as play (midgame NNUE search /
//! selective band / perfect solve) into one struct: give it a position,
//! get a move and a value. Search runs synchronously; UI callers should
//! invoke it on a worker thread.

use std::path::PathBuf;

use crate::book::{Book, BookCandidate};
use crate::evaluator::Evaluator;
use crate::midgame::{selective_band, NnueSearch, SharedTt, StopHandle};
use crate::nnue::Nnue;
use crate::pattern::EGAROUCID_PATTERNS;
use crate::solver::{final_score, EndSolverMode, Solver};
use crate::{Board, Position};

/// Convert a search value to disc scale.
///
/// The midgame search encodes "solved to the end mid-search" as discs x
/// 1000 (so it always outranks heuristic values); everything leaving the
/// engine returns to disc scale here, including values learn.rs writes
/// into the book. Also clamps to +/-64: aborted searches return sentinel
/// extremes that would otherwise surface as absurd on-screen values.
///
/// Rounding non-finite values to 0 here is a last-resort guard, not an
/// accepted path — a leaked abort sentinel shows up as a plausible
/// "+0.00" (which happened in a rated game, paired with an X-square
/// blunder). Hence the counter: it must stay at zero.
fn stone_scale(v: f32) -> f32 {
    let v = if v.abs() >= 999.0 { v / 1000.0 } else { v };
    if v.is_finite() {
        v.clamp(-64.0, 64.0)
    } else {
        NON_FINITE_VALUES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        0.0
    }
}

/// Times `stone_scale` rounded a non-finite value. Non-zero means something is broken.
pub static NON_FINITE_VALUES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Ponder depth cap — effectively unlimited, the deadline decides.
/// Capping ponder below the real search would leave only shallow
/// answers in the table.
const PONDER_DEPTH: u32 = 60;

/// Depth of the backup move played when a solve hits the deadline.
/// Shallow on purpose: it is only used when the estimate missed, a
/// decent move suffices, and a deeper backup would eat the solve's time.
const BACKUP_DEPTH: u32 = 8;

/// Fraction of the remaining budget the backup move may use.
const BACKUP_SHARE: f32 = 0.05;

/// No legal move for either side = game over.
fn is_game_over(board: &Board) -> bool {
    if board.movable() != 0 {
        return false;
    }
    let mut b = *board;
    b.pass();
    b.movable() == 0
}

/// Search settings and resources. Defaults are light, tuned for the GUI
/// and local analysis; play raises depth / solve_empties itself.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Midgame search depth in plies.
    pub depth: u32,
    /// Perfect solve at and below this many empties.
    pub solve_empties: u8,
    /// Selective band width in empties; 0 = none.
    pub band: u8,
    /// Search threads (midgame and endgame).
    pub threads: usize,
    /// Midgame ProbCut (MPC); normally on for both play and analysis.
    pub mpc: bool,
    /// Shared midgame table size (2^bits entries).
    pub midgame_hash_bits: u32,
    /// Endgame table size (2^bits entries).
    pub solver_hash_bits: u32,
    /// Linear evaluation weights (endgame move ordering).
    pub weights: PathBuf,
    /// NNUE weights (midgame search and band probes).
    pub nnue: PathBuf,
    /// Opening book (optional).
    pub book: PathBuf,
    /// Whether to consult the book; a knob because study wants it off.
    pub use_book: bool,
    /// Randomized-book tolerance in discs: candidates within this of
    /// best. 0 = always best (deterministic). Prevents replaying the
    /// same game against the same opponent.
    pub book_tolerance: f32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            depth: 12,
            solve_empties: 18,
            band: 0,
            threads: 4,
            mpc: true,
            midgame_hash_bits: 22,
            solver_hash_bits: 22,
            weights: PathBuf::from("weights/linear.bin"),
            nnue: PathBuf::from("weights/nnue-h16.bin"),
            book: PathBuf::from("weights/book.txt"),
            use_book: true,
            book_tolerance: 1.0,
        }
    }
}

/// One move decision; `value` is mover-view discs (exact when solved).
#[derive(Clone, Copy, Debug, Default)]
pub struct MoveEval {
    pub pos: Option<Position>,
    pub value: f32,
    /// Whether the value is exact (perfect solve).
    pub exact: bool,
    /// Whether the move came from the book.
    pub from_book: bool,
    /// Whether it came from a game-learned book entry (display only).
    pub learned: bool,
    /// Depth the midgame search reached; 0 for solves and book moves.
    pub depth: u32,
    /// The solve (or selective solve) hit the deadline and the backup
    /// move was played — a sign the entry estimate was too optimistic.
    pub cut: bool,
}

/// Live search progress, rewritten after each completed iteration.
///
/// The only window into "what is it reading right now" during a game.
/// The reader (GUI) is another thread, so everything is atomics — no
/// locks. It never changes search behavior: writes happen at iteration
/// boundaries (a dozen per move), never inside the search.
#[derive(Debug, Default)]
pub struct Progress {
    /// Current activity ([`Progress::IDLE`] / [`Progress::THINK`] /
    /// [`Progress::PONDER`] / [`Progress::SOLVE`] / [`Progress::SELECT`])。
    pub kind: std::sync::atomic::AtomicU8,
    /// Reached depth; 0 during solves.
    pub depth: std::sync::atomic::AtomicU32,
    /// Current best move (0..64); >= 64 means none yet.
    pub best: std::sync::atomic::AtomicU32,
    /// Its value x1000 in discs; `i32::MIN` means none yet.
    pub milli: std::sync::atomic::AtomicI32,
    /// Whether to negate values on write. Ponder reads the position
    /// after our move, where the opponent is to move, so search values
    /// are from their view; the display always wants ours.
    flip: std::sync::atomic::AtomicBool,
}

impl Progress {
    pub const IDLE: u8 = 0;
    pub const THINK: u8 = 1;
    pub const PONDER: u8 = 2;
    pub const SOLVE: u8 = 3;
    pub const SELECT: u8 = 4;

    /// Set only the activity kind (depth and move untouched).
    pub fn set_kind(&self, kind: u8) {
        self.kind.store(kind, std::sync::atomic::Ordering::Relaxed);
    }

    /// Reset to idle.
    ///
    /// This must also clear the negate flag: leaving it set once made
    /// every post-ponder think display with inverted sign (exposed by a
    /// -35 live value against a +35.3 final). Ponder re-raises the flag
    /// itself right after `clear()`.
    pub fn clear(&self) {
        self.kind
            .store(Self::IDLE, std::sync::atomic::Ordering::Relaxed);
        self.depth.store(0, std::sync::atomic::Ordering::Relaxed);
        self.best.store(64, std::sync::atomic::Ordering::Relaxed);
        self.milli
            .store(i32::MIN, std::sync::atomic::Ordering::Relaxed);
        self.flip.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// One iteration finished. Values are stored from our view (ponder
    /// negates, since it reads an opponent-to-move position).
    pub fn reached(&self, depth: u32, best: Option<Position>, value: f32) {
        use std::sync::atomic::Ordering::Relaxed;
        self.depth.store(depth, Relaxed);
        self.best
            .store(best.map(|p| p.index() as u32).unwrap_or(64), Relaxed);
        if value.is_finite() {
            let v = if self.flip.load(Relaxed) {
                -value
            } else {
                value
            };
            self.milli.store((v * 1000.0) as i32, Relaxed);
        }
    }

    /// Record the predicted opponent move (ponder).
    pub fn predict(&self, pos: Position) {
        self.best
            .store(pos.index() as u32, std::sync::atomic::Ordering::Relaxed);
    }

    /// Snapshot for readers.
    pub fn snapshot(&self) -> (u8, u32, Option<u32>, Option<f32>) {
        use std::sync::atomic::Ordering::Relaxed;
        let b = self.best.load(Relaxed);
        let m = self.milli.load(Relaxed);
        (
            self.kind.load(Relaxed),
            self.depth.load(Relaxed),
            (b < 64).then_some(b),
            (m != i32::MIN).then(|| m as f32 / 1000.0),
        )
    }
}

pub struct Engine {
    evaluator: Evaluator,
    search: NnueSearch,
    solver: Solver,
    config: EngineConfig,
    stop: StopHandle,
    book: Option<Book>,
    /// RNG state for randomized book picks (fresh per launch).
    book_rand: u64,
    /// Game-learned overlay (persisted). It is merged into `book` at
    /// startup and on import; move selection happens on `book`.
    learned: Book,
    /// The base book before the learned overlay. Analysis graphs may
    /// only use this side: base values come from deep search while
    /// learned values are backed-up game outcomes — not evaluations.
    /// "Is it in the overlay?" is not a substitute filter: opening
    /// positions appear in both, and filtering on the overlay once
    /// dropped genuine base entries.
    book_base: Option<Book>,
    /// Overlay save path (book_learn.txt next to the book).
    learn_path: std::path::PathBuf,
    /// Cumulative solver nodes; the midgame search tracks its own but
    /// the Solver only keeps the last run's count.
    solver_nodes: u64,
    /// Search progress (the external view hook).
    progress: std::sync::Arc<Progress>,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Engine, String> {
        let mut evaluator = Evaluator::new(EGAROUCID_PATTERNS);
        evaluator
            .load_weights(&config.weights)
            .map_err(|e| format!("weights {}: {e}", config.weights.display()))?;
        let mut nn = Nnue::new(EGAROUCID_PATTERNS);
        nn.load(&config.nnue)
            .map_err(|e| format!("nnue {}: {e}", config.nnue.display()))?;
        // Build the int16 tables; skipping this makes eval read
        // uninitialized memory.
        nn.quantize();
        // NnueSearch / Solver want process-lifetime references; Engine
        // itself is one-per-process, so leak to satisfy them.
        let nn: &'static Nnue = Box::leak(Box::new(nn));
        let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(config.midgame_hash_bits)));
        let progress = std::sync::Arc::new(Progress::default());
        let mut search = NnueSearch::new(nn, tt);
        search.set_progress(Some(progress.clone()));
        search.threads = config.threads;
        search.mpc = config.mpc;
        let mut solver = Solver::new(config.solver_hash_bits);
        solver.set_nnue(nn, tt);
        solver.set_threads(config.threads);
        let stop = StopHandle::new();
        search.set_stop(Some(stop.clone()));
        solver.set_stop(Some(stop.clone()));
        // The book is optional. Loading is always attempted; `use_book`
        // only toggles consulting it, so switching mid-game needs no reload.
        let mut book = match Book::load(&config.book) {
            Ok(b) if !b.is_empty() => Some(b),
            _ => None,
        };
        // Overlay the game-learned entries (learn.rs). The overlay alone
        // works as a book too — experience accumulates even without a
        // book file — and being a separate file it never conflicts with
        // a bookgen rebuild.
        let learn_path = config.book.with_file_name("book_learn.txt");
        // Keep the pre-overlay base; after merging they are inseparable.
        let book_base = match Book::load(&config.book) {
            Ok(b) if !b.is_empty() => Some(b),
            _ => None,
        };
        let learned = Book::load(&learn_path).unwrap_or_default();
        if !learned.is_empty() {
            let base = book.get_or_insert_with(Book::new);
            crate::learn::merge_learned(base, &learned);
        }
        let book_rand = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15)
            | 1;
        Ok(Engine {
            evaluator,
            search,
            solver,
            config,
            stop,
            book,
            book_base,
            book_rand,
            learned,
            learn_path,
            solver_nodes: 0,
            progress: progress.clone(),
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Start the deadline watcher: it raises the stop handle when time
    /// is up, aborting a running solve. Iterative deepening can watch
    /// its own deadline at iteration boundaries; a solve has no
    /// boundaries and can only be stopped from outside.
    fn watch_deadline(
        &self,
        deadline: Option<std::time::Instant>,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        let dl = deadline?;
        let stop = self.stop.clone();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d2 = done.clone();
        std::thread::spawn(move || {
            while !d2.load(std::sync::atomic::Ordering::Relaxed) {
                let now = std::time::Instant::now();
                if now >= dl {
                    stop.stop();
                    return;
                }
                std::thread::sleep((dl - now).min(std::time::Duration::from_millis(20)));
            }
        });
        Some(done)
    }

    /// Tear down the watcher and report whether the deadline fired.
    fn stop_watch_done(
        &mut self,
        watcher: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> bool {
        let Some(done) = watcher else { return false };
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let cut = self.stop.is_stopped();
        // Reset for the next search (`choose_within` also resets).
        self.stop.reset();
        cut
    }

    /// Lifetime node count (midgame + solver); diff two readings for a
    /// single search. Display only.
    pub fn nodes(&self) -> u64 {
        self.search.nodes + self.solver_nodes
    }

    /// Loaded book positions (0 = no book).
    pub fn book_size(&self) -> usize {
        self.book.as_ref().map_or(0, |b| b.len())
    }

    /// Game-learned overlay positions.
    pub fn learned_size(&self) -> usize {
        self.learned.len()
    }

    /// Tell the next search "try this move first".
    ///
    /// Synchro boards mirror each other, so the opponent's move on one
    /// board is a candidate on the other — and a move a 2650 spent
    /// minutes on is worth ordering first. This never delegates the
    /// choice: no bounds are stored, so no cutoffs; only ordering changes.
    pub fn hint_move(&mut self, board: &Board, pos: Position) {
        let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
        self.search.tt.seed_move(h, pos.index());
    }

    /// Stop handle for the running search (used from other threads).
    /// Results after raising it are incomplete — discard them.
    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    /// Swap search settings (depth / solve entry / band) only; tables
    /// and weights stay.
    /// Toggle book consultation (off to see the engine's own moves).
    pub fn set_use_book(&mut self, on: bool) {
        self.config.use_book = on;
    }

    /// Whether a book is loaded (drives the "no book" indicator).
    pub fn has_book(&self) -> bool {
        self.book.is_some()
    }

    /// Display: candidate values for a book position (board
    /// orientation, mover view). Empty when the book is disabled, to
    /// match play.
    pub fn book_hints(&self, board: &Board) -> Option<Vec<(Position, f32)>> {
        self.book
            .as_ref()
            .filter(|_| self.config.use_book)?
            .candidates(board)
    }

    /// Book browsing: candidates (value, adoption count) plus whether
    /// the position was game-learned. Unlike `book_hints` this ignores
    /// `use_book` — browsing while the book is off is legitimate.
    pub fn book_node(&self, board: &Board) -> Option<(Vec<BookCandidate>, bool)> {
        let moves = self.book.as_ref()?.candidates_detailed(board)?;
        Some((moves, self.learned.has(board)))
    }

    /// Clear the midgame table.
    ///
    /// For measurement: without a per-game clear, a warm table biases
    /// even same-vs-same matches. Never call it mid-game — pondering
    /// exists precisely to carry the table over.
    pub fn clear_tables(&mut self) {
        self.search.clear();
    }

    /// Measure this machine's solve speed (nodes/sec).
    ///
    /// Deriving the solve entry point from the clock
    /// ([`crate::timectl::solve_entry`]) needs a nodes-to-seconds
    /// factor, and that factor alone is machine-dependent; measure here,
    /// record in `resources.conf`.
    ///
    /// It measures this Engine's own Solver so thread count, table size
    /// and NNUE match play. Three 22-empty positions keep it under ~2s
    /// single-threaded — cheap enough to run at startup. Table-clear
    /// time is subtracted (fixed cost, not proportional to nodes);
    /// deeper positions lose ~10% nps to table overflow, absorbed by
    /// `timectl`'s `DEEP_NPS_RATIO`.
    ///
    /// Two passes, keep the faster: interference only slows things down,
    /// so the faster pass is closer to the truth and never an
    /// overestimate.
    pub fn measure_solve_nps(&mut self) -> f64 {
        let a = self.measure_solve_nps_once();
        let b = self.measure_solve_nps_once();
        a.max(b)
    }

    /// One measurement pass; [`Engine::measure_solve_nps`] calls it twice.
    fn measure_solve_nps_once(&mut self) -> f64 {
        /// 22-empty calibration positions (OBF).
        const POSITIONS: [&str; 3] = [
            "--XOOO----XOOOOO-XXOOOOX-XXXOXXX-XXXOXXX-XOOOOXX--OO-O------O--- X",
            "----X-----OX-O---XXXXO--XXXXXO--OOXXXOO-OOOXOXOO-OOOOOX--XXXXXX- X",
            "-OOOOX---OXXOX--XXXOOOOOXXXXO-O-XXXOOO--XXXXXXX-X--XXX------X--- X",
        ];
        use std::sync::atomic::Ordering;
        let clear0 = crate::solver::CLEAR_NS.load(Ordering::Relaxed);
        let t0 = std::time::Instant::now();
        let mut nodes = 0u64;
        for p in POSITIONS {
            let Ok(board) = Board::from_string(p) else {
                continue;
            };
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, &board, Some(&self.evaluator));
            nodes += r.nodes;
        }
        self.solver_nodes += nodes;
        let clear = (crate::solver::CLEAR_NS.load(Ordering::Relaxed) - clear0) as f64 / 1e9;
        let secs = t0.elapsed().as_secs_f64() - clear;
        if secs <= 0.0 || nodes == 0 {
            return 0.0;
        }
        nodes as f64 / secs
    }

    /// Ponder during the opponent's turn. Does not move the board.
    ///
    /// `after_my_move` is the position after our move (opponent to
    /// move). Deepens the single predicted reply from the table until
    /// the deadline or `stop`, returning nodes visited.
    ///
    /// Spreading time across all legal replies was measured and
    /// rejected: 11 moves reach only 4-6 plies each, useless against a
    /// ~17-ply real search (+1.6-1.8 plies for single-move ponder, 0 for
    /// spread). The analysis path is unusable here — it clears the table
    /// around each move for fairness, the exact opposite of pondering's
    /// purpose. Works with fixed depth too: same depth in 1/3 the time
    /// (measured -62 to -65%).
    pub fn ponder(&mut self, after_my_move: &Board, deadline: std::time::Instant) -> u64 {
        let base = self.nodes();
        if is_game_over(after_my_move) || after_my_move.movable_count() == 0 {
            return 0;
        }
        /* The prediction may be missing (the endgame solver uses its
        own table; measured 47% of endgame moves). Do not ponder a
        made-up move — that just burns time with no chance of a hit. */
        let Some(pred) = self.tt_best(after_my_move) else {
            return 0;
        };
        self.progress.clear();
        self.progress.set_kind(Progress::PONDER);
        self.progress
            .flip
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.progress.predict(pred);
        let mut child = *after_my_move;
        child.make_move_bits(pred);
        if is_game_over(&child) {
            return 0;
        }
        /* No pondering in the solve region: tried, measured, rejected
        (121.8 -> 122.6 ms per move — no change — despite the ponder
        demonstrably running). Why it doesn't help is unresolved, and an
        unexplained gain is not kept; a solve also cannot be sliced by
        deadline, risking a blocked receive loop. */
        if child.empty_count() <= self.config.solve_empties {
            return 0;
        }
        self.stop.reset();
        /* Hand the deadline into the search: checking between
        iterations cannot stop a long iteration already underway, which
        would block the GGS receive loop for seconds. */
        /* No depth cap: only ponder that reaches real-search depth is
        useful (same reason the spread form was rejected). */
        self.search
            .best_move_deadline(&child, PONDER_DEPTH, Some(deadline));
        self.nodes() - base
    }

    /// Best move stored for this position; never re-searches.
    /// Pondering's prediction source: pass the position after our move,
    /// get the opponent's best as far as the last search saw (None if
    /// evicted).
    pub fn tt_best(&self, board: &Board) -> Option<Position> {
        let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
        self.search
            .tt
            .best_move(h)
            .and_then(|i| Position::from_index(i as u32))
    }

    /// Display: (best value, game-learned?) for a book position; the
    /// eval graph uses it instead of searching.
    pub fn book_value(&self, board: &Board) -> Option<(f32, bool)> {
        let hints = self.book_hints(board)?;
        let best = hints
            .iter()
            .map(|(_, v)| *v)
            .fold(f32::NEG_INFINITY, f32::max);
        let learned = self.learned.get_raw(Book::key(board).0).is_some();
        Some((best, learned))
    }

    /// Best value from the base book only (mover view), for analysis
    /// graphs. The learned overlay is excluded — its values are
    /// backed-up game outcomes, not evaluations. Respects `use_book`.
    pub fn book_base_value(&self, board: &Board) -> Option<f32> {
        let hints = self
            .book_base
            .as_ref()
            .filter(|_| self.config.use_book)?
            .candidates(board)?;
        hints
            .iter()
            .map(|(_, v)| *v)
            .fold(f32::NEG_INFINITY, f32::max)
            .into()
    }

    /// (best value, game-learned?, search depth) for a book position;
    /// the depth tells the book screen how trustworthy the value is.
    pub fn book_entry(&self, board: &Board) -> Option<(f32, bool, u8)> {
        let book = self.book.as_ref()?;
        let (_, value, depth) = book.probe(board)?;
        let learned = self.learned.get_raw(Book::key(board).0).is_some();
        Some((value, learned, depth))
    }

    pub fn set_levels(&mut self, depth: u32, solve_empties: u8, band: u8) {
        self.config.depth = depth;
        self.config.solve_empties = solve_empties;
        self.config.band = band;
    }

    /// Change the thread count (midgame and endgame); takes effect on
    /// the next search.
    pub fn set_threads(&mut self, n: usize) {
        let n = n.max(1);
        self.config.threads = n;
        self.search.threads = n;
        self.solver.set_threads(n);
    }

    /// Choose a move with the same policy as play; `pos: None` = pass.
    /// Value is mover-view discs. The book includes the learned overlay
    /// and both go through the same randomized pick, so moves devalued
    /// by losses naturally stop being chosen — no special avoidance.
    pub fn choose(&mut self, board: &Board) -> MoveEval {
        self.choose_within(board, None)
    }

    /// Search progress hook (safe to read from other threads).
    pub fn progress(&self) -> std::sync::Arc<Progress> {
        self.progress.clone()
    }

    /// Deadline-bounded move choice. Iterative deepening returns the
    /// last completed depth when time runs out; solves and book moves
    /// cannot be sliced, so the caller decides before entering whether
    /// the clock allows them.
    pub fn choose_within(
        &mut self,
        board: &Board,
        deadline: Option<std::time::Instant>,
    ) -> MoveEval {
        self.stop.reset();
        self.progress.clear();
        self.progress.set_kind(Progress::THINK);
        // Book: answers come from deeper-than-game search; return immediately.
        if let Some(book) = self.book.as_ref().filter(|_| self.config.use_book) {
            let hit = if self.config.book_tolerance > 0.0 {
                // Pick among near-equal candidates to avoid repeats.
                self.book_rand = self
                    .book_rand
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                book.probe_varied(board, self.config.book_tolerance, self.book_rand >> 11)
            } else {
                book.probe(board)
            };
            if let Some((pos, value, _depth)) = hit {
                // Whether it is game-learned (display only).
                let learned = self.learned.get_raw(Book::key(board).0).is_some();
                return MoveEval {
                    pos: Some(pos),
                    value,
                    exact: false,
                    from_book: true,
                    learned,
                    depth: 0,
                    cut: false,
                };
            }
        }
        let c = &self.config;
        if is_game_over(board) {
            // Game over: searching would return 0; return the board's
            // disc difference (empties to the winner, FFO convention).
            return MoveEval {
                pos: None,
                value: final_score(board) as f32,
                exact: true,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
            };
        }
        if board.empty_count() <= c.solve_empties {
            /* Solves get a deadline too. `timectl` estimates the entry
            cost, but estimates miss (5x spread at the same empties), and
            an unbounded solve means a flag fall. Aborting into a shallow
            answer loses less. The backup move is taken first — a
            half-aborted solve's value is unusable. */
            let backup = deadline.map(|_| {
                let (pos, value, _) = self.search.best_move_deadline(
                    board,
                    BACKUP_DEPTH,
                    // Only a sliver of the budget: overspending here
                    // starves the actual solve.
                    deadline.map(|d| {
                        let now = std::time::Instant::now();
                        now + (d - now).mul_f32(BACKUP_SHARE)
                    }),
                );
                (pos, value)
            });
            self.progress.set_kind(Progress::SOLVE);
            let watcher = self.watch_deadline(deadline);
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            self.solver_nodes += r.nodes;
            let cut = self.stop_watch_done(watcher);
            if cut {
                if let Some((pos, value)) = backup.filter(|(p, _)| p.is_some()) {
                    return MoveEval {
                        pos,
                        value: stone_scale(value),
                        exact: false,
                        from_book: false,
                        learned: false,
                        depth: BACKUP_DEPTH,
                        cut: true,
                    };
                }
            }
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: !cut,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
            }
        } else if let Some(t) = selective_band(board.empty_count(), c.solve_empties, c.band) {
            // Selective solve: same policy, backup move on deadline.
            let backup = deadline.map(|_| {
                let (pos, value, _) = self.search.best_move_deadline(
                    board,
                    BACKUP_DEPTH,
                    deadline.map(|d| {
                        let now = std::time::Instant::now();
                        now + (d - now).mul_f32(BACKUP_SHARE)
                    }),
                );
                (pos, value)
            });
            self.progress.set_kind(Progress::SELECT);
            let watcher = self.watch_deadline(deadline);
            let r = self.solver.solve_selective(board, Some(&self.evaluator), t);
            self.solver_nodes += r.nodes;
            let cut = self.stop_watch_done(watcher);
            if cut {
                if let Some((pos, value)) = backup.filter(|(p, _)| p.is_some()) {
                    return MoveEval {
                        pos,
                        value: stone_scale(value),
                        exact: false,
                        from_book: false,
                        learned: false,
                        depth: BACKUP_DEPTH,
                        cut: true,
                    };
                }
            }
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: false,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
            }
        } else {
            /* The midgame needs the watcher too. It was omitted here on
            the theory that deepening checks the deadline between
            iterations — insufficient: a single long iteration cannot be
            stopped that way, and under synchro CPU contention a 43.5s
            budget once ran 132.6s (not reproducible single-board). The
            watcher's stop exits mid-iteration; lazy_smp discards the cut
            iteration and returns the previous answer, so no move is
            lost. */
            let watcher = self.watch_deadline(deadline);
            let (pos, value, reached) = self.search.best_move_deadline(board, c.depth, deadline);
            let cut = self.stop_watch_done(watcher);
            /* Diagnostics: also print the returned value so it can be
            cross-checked against the per-iteration log (the in-game
            mismatches were exactly "value matches no iteration"). */
            if std::env::var("ROOT_TRACE").is_ok() {
                let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
                eprintln!(
                    "  ret [{h:016x}] {:+8.2} (raw {value:+.2}) depth {reached} {:?}{}",
                    stone_scale(value),
                    pos,
                    if cut { " cut" } else { "" }
                );
            }
            MoveEval {
                pos,
                value: stone_scale(value),
                exact: false,
                from_book: false,
                learned: false,
                depth: reached,
                cut,
            }
        }
    }

    /// Prepare a game import (learn.rs); drive it with `learn_step`.
    pub fn learn_start(
        &self,
        start: Option<&str>,
        kifu: &str,
        learn_depth: u32,
    ) -> Result<crate::learn::BackupJob, String> {
        crate::learn::BackupJob::new(start, kifu, learn_depth.min(u8::MAX as u32) as u8)
    }

    /// Advance the import by one search; on completion returns the
    /// outcome and saves the overlay. Each call performs at most one
    /// evaluation, so it can run between games without hurting
    /// responsiveness. `learn_depth` is the (shallower) evaluation depth.
    pub fn learn_step(
        &mut self,
        job: &mut crate::learn::BackupJob,
        learn_depth: u32,
    ) -> Result<Option<crate::learn::BackupOutcome>, String> {
        use crate::learn::JobStep;
        self.stop.reset();
        // Detach learned/book for the job; self is only used to search.
        let mut base = self.book.take().unwrap_or_default();
        let mut learned = std::mem::take(&mut self.learned);
        let done = match job.next(&mut learned, &mut base) {
            JobStep::Search(b) => {
                // Learning evaluates every legal move per position;
                // temporarily narrow the solve entry to keep it cheap.
                let saved_solve = self.config.solve_empties;
                self.config.solve_empties = saved_solve.min(20);
                let v = self.eval_position_inner(&b, learn_depth);
                self.config.solve_empties = saved_solve;
                job.feed(v.value);
                None
            }
            JobStep::Done(out) => Some(out),
        };
        let save = if done.is_some() {
            learned.save(&self.learn_path)
        } else {
            Ok(())
        };
        self.book = (!base.is_empty()).then_some(base);
        self.learned = learned;
        save.map_err(|e| format!("saving learned book {}: {e}", self.learn_path.display()))?;
        Ok(done)
    }

    /// Undo one game's import; returns how many moves were reverted.
    /// After restoring and saving the overlay, the base book is reloaded
    /// from file and re-merged — it is file + overlay, so rebuild rather
    /// than guess what the file contained.
    pub fn undo_learn(
        &mut self,
        start: Option<&str>,
        kifu: &str,
        changes: &[crate::learn::BackupChange],
    ) -> Result<usize, String> {
        let n = crate::learn::undo_backup(&mut self.learned, start, kifu, changes)?;
        self.learned
            .save(&self.learn_path)
            .map_err(|e| format!("saving learned book {}: {e}", self.learn_path.display()))?;
        let mut book = match Book::load(&self.config.book) {
            Ok(b) if !b.is_empty() => Some(b),
            _ => None,
        };
        if !self.learned.is_empty() {
            let base = book.get_or_insert_with(Book::new);
            crate::learn::merge_learned(base, &self.learned);
        }
        self.book = book;
        Ok(n)
    }

    /// Evaluate a position at a given depth (eval graph / study); exact
    /// in the solve region. Mover view.
    pub fn eval_position(&mut self, board: &Board, depth: u32) -> MoveEval {
        self.stop.reset();
        self.eval_position_inner(board, depth)
    }

    /// Body of `eval_position`; does not reset the stop handle so long
    /// externally-stoppable jobs (learning) can call it repeatedly.
    fn eval_position_inner(&mut self, board: &Board, depth: u32) -> MoveEval {
        if is_game_over(board) {
            return MoveEval {
                pos: None,
                value: final_score(board) as f32,
                exact: true,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
            };
        }
        if board.empty_count() <= self.config.solve_empties {
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: true,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
            }
        } else {
            let (pos, value) = self.search.best_move_valued(board, depth);
            MoveEval {
                pos,
                value: stone_scale(value),
                exact: false,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
            }
        }
    }

    /// Score every legal move (WZebra-style hints): each child at
    /// `depth - 1` (exact in the solve region), parent mover view,
    /// sorted descending. Comparability first: each child is measured
    /// under identical conditions (cleared table, single thread) —
    /// a shared warm table skews even symmetric positions by discs.
    pub fn analyze(&mut self, board: &Board, depth: u32) -> Vec<(Position, MoveEval)> {
        self.stop.reset();
        let saved_threads = self.search.threads;
        self.search.threads = 1;
        let mut out = Vec::new();
        for pos in board.movable_iter() {
            if self.stop.is_stopped() {
                break; // stopped: return what has been scored
            }
            let mut child = *board;
            child.make_move_bits(pos);
            let ev = if is_game_over(&child) {
                MoveEval {
                    pos: Some(pos),
                    value: -(final_score(&child) as f32),
                    exact: true,
                    from_book: false,
                    learned: false,
                    depth: 0,
                    cut: false,
                }
            } else if child.empty_count() <= self.config.solve_empties {
                let r = self.solver.solve_with_eval(
                    EndSolverMode::Perfect,
                    &child,
                    Some(&self.evaluator),
                );
                self.solver_nodes += r.nodes;
                MoveEval {
                    pos: Some(pos),
                    value: -(r.value as f32),
                    exact: true,
                    from_book: false,
                    learned: false,
                    depth: 0,
                    cut: false,
                }
            } else {
                self.search.clear();
                let d = depth.saturating_sub(1).max(1);
                let (_, v) = self.search.best_move_valued(&child, d);
                MoveEval {
                    pos: Some(pos),
                    value: stone_scale(-v),
                    exact: false,
                    from_book: false,
                    learned: false,
                    depth: 0,
                    cut: false,
                }
            };
            out.push((pos, ev));
        }
        self.search.threads = saved_threads;
        // Don't carry analysis' shallow entries into play searches.
        self.search.clear();
        out.sort_by(|a, b| b.1.value.total_cmp(&a.1.value));
        out
    }

    /// Score all legal moves at increasing depth, reporting per pass.
    ///
    /// Deepens until `on_pass` returns false or the stop handle rises;
    /// each pass replaces the last, so stopping keeps the best so far.
    /// Solved moves stay fixed in later passes. `on_pass`'s third
    /// argument is nodes visited (display only). `solve_empties` is
    /// deliberately ignored: this reports how far deepening got, and a
    /// move solves when depth reaches the empty count.
    pub fn analyze_deepening(
        &mut self,
        board: &Board,
        from_depth: u32,
        mut on_pass: impl FnMut(u32, &[(Position, MoveEval)], u64) -> bool,
    ) {
        let base_nodes = self.nodes();
        self.stop.reset();
        let saved_threads = self.search.threads;
        self.search.threads = 1;
        let mut depth = from_depth.max(1);
        loop {
            let mut out: Vec<(Position, MoveEval)> = Vec::new();
            let mut all_exact = true;
            for pos in board.movable_iter() {
                if self.stop.is_stopped() {
                    self.search.threads = saved_threads;
                    self.search.clear();
                    return;
                }
                let mut child = *board;
                child.make_move_bits(pos);
                let ev = if is_game_over(&child) {
                    MoveEval {
                        pos: Some(pos),
                        value: -(final_score(&child) as f32),
                        exact: true,
                        from_book: false,
                        learned: false,
                        depth: 0,
                        cut: false,
                    }
                } else if u32::from(child.empty_count()) <= depth {
                    // Deepening reached this move's end. MPC makes the
                    // midgame value inexact even at full depth, so hand
                    // it to the solver and pin it as exact.
                    let r = self.solver.solve_with_eval(
                        EndSolverMode::Perfect,
                        &child,
                        Some(&self.evaluator),
                    );
                    self.solver_nodes += r.nodes;
                    MoveEval {
                        pos: Some(pos),
                        value: stone_scale(-(r.value as f32)),
                        exact: true,
                        from_book: false,
                        learned: false,
                        depth: 0,
                        cut: false,
                    }
                } else {
                    all_exact = false;
                    self.search.clear();
                    let (_, v, reached) = self.search.best_move_deadline(&child, depth, None);
                    MoveEval {
                        pos: Some(pos),
                        value: stone_scale(-v),
                        exact: false,
                        from_book: false,
                        learned: false,
                        depth: reached + 1,
                        cut: false, // child at d = d+1 plies from the parent
                    }
                };
                out.push((pos, ev));
            }
            out.sort_by(|a, b| b.1.value.total_cmp(&a.1.value));
            let go_on = on_pass(depth, &out, self.nodes() - base_nodes);
            // All moves solved: deeper passes cannot change anything.
            if !go_on || all_exact || depth >= 60 {
                break;
            }
            depth += 1;
        }
        self.search.threads = saved_threads;
        // Don't carry analysis' shallow entries into play searches.
        self.search.clear();
    }
}

#[cfg(test)]
mod progress_tests {
    use super::Progress;
    use crate::Position;

    /// `clear()` must reset the negate flag.
    ///
    /// It once didn't, so a single ponder made every later think display
    /// with inverted sign (a -35 live value against a +35.3 final; moves
    /// themselves were fine — display-only damage).
    #[test]
    fn clear_drops_the_flip() {
        let p = Progress::default();
        let mv = Position::from_index(19);

        // Ponder: opponent-view value, stored negated.
        p.set_kind(Progress::PONDER);
        p.flip.store(true, std::sync::atomic::Ordering::Relaxed);
        p.reached(6, mv, 4.0);
        assert_eq!(p.snapshot().3, Some(-4.0), "ponder stores negated");

        // Moving to think: no negation after clear().
        p.clear();
        p.set_kind(Progress::THINK);
        p.reached(6, mv, 4.0);
        assert_eq!(
            p.snapshot().3,
            Some(4.0),
            "clear() must not carry the negate flag over"
        );
    }
}
