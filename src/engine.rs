//! Engine session layer shared by the GUI, CLI and protocol frontends.

use std::path::PathBuf;

use crate::book::{Book, BookCandidate};
use crate::linear::Linear;
use crate::midgame::{selective_band, NnueSearch, SharedTt, StopHandle};
use crate::nnue::{Nnue, ACT_UNITS};
use crate::pattern::{Pattern, LINEAR_PATTERNS, NNUE_PATTERNS};
use crate::solver::{final_score, EndSolverMode, Solver};
use crate::{Board, Position};

fn stone_scale(v: f32) -> f32 {
    let v = if v.abs() >= 999.0 { v / 1000.0 } else { v };
    if v.is_finite() {
        v.clamp(-64.0, 64.0)
    } else {
        NON_FINITE_VALUES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        0.0
    }
}

pub static NON_FINITE_VALUES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const PONDER_DEPTH: u32 = 60;

const BACKUP_DEPTH: u32 = 60;

const BACKUP_SHARE: f32 = 0.05;

fn is_game_over(board: &Board) -> bool {
    if board.movable() != 0 {
        return false;
    }
    let mut b = *board;
    b.pass();
    b.movable() == 0
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub depth: u32,
    pub solve_empties: u8,
    pub band: u8,
    pub threads: usize,
    pub mpc: bool,
    pub midgame_hash_bits: u32,
    pub solver_hash_bits: u32,
    pub weights: PathBuf,
    pub nnue: PathBuf,
    pub nnue_base: PathBuf,
    pub nnue_patterns: &'static [Pattern],
    pub head_f32: bool,
    pub act_units: f32,
    pub book: PathBuf,
    pub use_book: bool,
    pub book_tolerance: f32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            depth: 12,
            head_f32: false,
            act_units: ACT_UNITS,
            solve_empties: 18,
            band: 0,
            threads: 4,
            mpc: true,
            midgame_hash_bits: 22,
            solver_hash_bits: 22,
            weights: PathBuf::from("weights/linear.bin"),
            nnue: PathBuf::from("weights/nnue.bin"),
            nnue_base: PathBuf::new(),
            nnue_patterns: NNUE_PATTERNS,
            book: PathBuf::from("weights/book.txt"),
            use_book: true,
            book_tolerance: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MoveEval {
    pub pos: Option<Position>,
    pub value: f32,
    pub exact: bool,
    pub from_book: bool,
    pub learned: bool,
    pub depth: u32,
    pub cut: bool,
    /// `gap` is a lower bound, `None` where no lead was shown (zero would read as a tie).
    pub stop_reason: crate::midgame::Stopped,
    pub second: Option<Position>,
    pub gap: Option<f32>,
    pub extended: bool,
}

#[derive(Debug, Default)]
pub struct Progress {
    pub kind: std::sync::atomic::AtomicU8,
    pub depth: std::sync::atomic::AtomicU32,
    pub best: std::sync::atomic::AtomicU32,
    pub milli: std::sync::atomic::AtomicI32,
    predicted: std::sync::atomic::AtomicU32,
    flip: std::sync::atomic::AtomicBool,
}

impl Progress {
    pub const IDLE: u8 = 0;
    pub const THINK: u8 = 1;
    pub const PONDER: u8 = 2;
    pub const SOLVE: u8 = 3;
    pub const SELECT: u8 = 4;

    pub fn set_kind(&self, kind: u8) {
        self.kind.store(kind, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn clear(&self) {
        self.kind
            .store(Self::IDLE, std::sync::atomic::Ordering::Relaxed);
        self.depth.store(0, std::sync::atomic::Ordering::Relaxed);
        self.best.store(64, std::sync::atomic::Ordering::Relaxed);
        self.milli
            .store(i32::MIN, std::sync::atomic::Ordering::Relaxed);
        self.predicted
            .store(64, std::sync::atomic::Ordering::Relaxed);
        self.flip.store(false, std::sync::atomic::Ordering::Relaxed);
    }

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

    pub fn predict(&self, pos: Position) {
        self.predicted
            .store(pos.index() as u32, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (u8, u32, Option<u32>, Option<f32>, Option<u32>) {
        use std::sync::atomic::Ordering::Relaxed;
        let b = self.best.load(Relaxed);
        let m = self.milli.load(Relaxed);
        let p = self.predicted.load(Relaxed);
        (
            self.kind.load(Relaxed),
            self.depth.load(Relaxed),
            (b < 64).then_some(b),
            (m != i32::MIN).then(|| m as f32 / 1000.0),
            (p < 64).then_some(p),
        )
    }
}

/// Our next move is solved (fully or selectively) from the solver's own tables, which pondering never fills.
pub fn ponder_pays(after_my_move: &Board, solve_empties: u8, band: u8) -> bool {
    after_my_move.empty_count() as u32 > solve_empties as u32 + band as u32 + 1
}

pub struct Engine {
    linear: std::sync::Arc<Linear>,
    search: NnueSearch,
    solver: Solver,
    config: EngineConfig,
    stop: StopHandle,
    book: Option<std::sync::Arc<Book>>,
    book_rand: u64,
    learned: Book,
    book_base: Option<Book>,
    learn_path: std::path::PathBuf,
    solver_nodes: u64,
    progress: std::sync::Arc<Progress>,
}

#[derive(Clone)]
pub struct EngineAssets {
    linear: std::sync::Arc<Linear>,
    nnue: std::sync::Arc<Nnue>,
}

impl EngineAssets {
    pub fn load(config: &EngineConfig) -> Result<EngineAssets, String> {
        let mut linear = Linear::new(LINEAR_PATTERNS);
        linear
            .load_weights(&config.weights)
            .map_err(|e| format!("weights {}: {e}", config.weights.display()))?;
        let mut nn = Nnue::new(config.nnue_patterns);
        nn.load(&config.nnue)
            .map_err(|e| format!("nnue {}: {e}", config.nnue.display()))?;
        if !config.nnue_base.as_os_str().is_empty() {
            let mut b = Linear::new(config.nnue_patterns);
            b.load_weights(&config.nnue_base)
                .map_err(|e| format!("nnue base {}: {e}", config.nnue_base.display()))?;
            nn.set_base(b);
        }
        nn.act_units = config.act_units;
        nn.quantize();
        nn.head_f32 = config.head_f32;
        Ok(EngineAssets {
            linear: std::sync::Arc::new(linear),
            nnue: std::sync::Arc::new(nn),
        })
    }

    pub fn nnue(&self) -> &Nnue {
        &self.nnue
    }
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Engine, String> {
        let assets = EngineAssets::load(&config)?;
        Engine::with_assets(assets, config)
    }

    pub fn with_assets(assets: EngineAssets, config: EngineConfig) -> Result<Engine, String> {
        let linear = assets.linear;
        let nn = assets.nnue;
        let tt = std::sync::Arc::new(SharedTt::new(config.midgame_hash_bits));
        let progress = std::sync::Arc::new(Progress::default());
        let mut search = NnueSearch::new(nn.clone(), tt.clone());
        search.set_progress(Some(progress.clone()));
        search.threads = config.threads;
        search.mpc = config.mpc;
        let mut solver = Solver::new(config.solver_hash_bits);
        solver.set_nnue(nn, tt);
        solver.set_threads(config.threads);
        let stop = StopHandle::new();
        search.set_stop(Some(stop.clone()));
        solver.set_stop(Some(stop.clone()));
        let mut book = match Book::load(&config.book) {
            Ok(b) if !b.is_empty() => Some(b),
            _ => None,
        };
        let learn_path = config.book.with_file_name("book_learn.txt");
        let book_base = match Book::load(&config.book) {
            Ok(b) if !b.is_empty() => Some(b),
            _ => None,
        };
        let learned = Book::load(&learn_path).unwrap_or_default();
        if !learned.is_empty() {
            let base = book.get_or_insert_with(Book::new);
            crate::learn::merge_learned(base, &learned);
        }
        let book = book.map(std::sync::Arc::new);
        let book_rand = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15)
            | 1;
        Ok(Engine {
            linear,
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

    fn backup_move(
        &mut self,
        board: &Board,
        deadline: std::time::Instant,
    ) -> (Option<Position>, f32) {
        let now = std::time::Instant::now();
        let until = now
            + deadline
                .saturating_duration_since(now)
                .mul_f32(BACKUP_SHARE);
        let (pos, value, _) = self
            .search
            .best_move_deadline(board, BACKUP_DEPTH, Some(until));
        // The backup's own deadline trips the shared stop; left set, it kills the solve at once.
        if std::time::Instant::now() >= until {
            self.stop.reset();
        }
        (pos, value)
    }

    fn stop_watch_done(
        &mut self,
        watcher: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> bool {
        let Some(done) = watcher else { return false };
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let cut = self.stop.is_stopped();
        self.stop.reset();
        cut
    }

    pub fn nodes(&self) -> u64 {
        self.search.nodes + self.solver_nodes
    }

    pub fn book_size(&self) -> usize {
        self.book.as_ref().map_or(0, |b| b.len())
    }

    pub fn learned_size(&self) -> usize {
        self.learned.len()
    }

    pub fn hint_move(&mut self, board: &Board, pos: Position) {
        let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
        self.search.tt.seed_move(h, pos.index());
    }

    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    pub fn set_use_book(&mut self, on: bool) {
        self.config.use_book = on;
    }

    pub fn has_book(&self) -> bool {
        self.book.is_some()
    }

    pub fn book_hints(&self, board: &Board) -> Option<Vec<(Position, f32)>> {
        self.book
            .as_ref()
            .filter(|_| self.config.use_book)?
            .candidates(board)
    }

    pub fn book_node(&self, board: &Board) -> Option<(Vec<BookCandidate>, bool)> {
        let moves = self.book.as_ref()?.candidates_detailed(board)?;
        Some((moves, self.learned.has(board)))
    }

    pub fn clear_tables(&mut self) {
        self.search.clear();
        self.solver.clear_tables();
    }

    pub fn measure_solve_nps(&mut self) -> f64 {
        let a = self.measure_solve_nps_once();
        let b = self.measure_solve_nps_once();
        a.max(b)
    }

    fn measure_solve_nps_once(&mut self) -> f64 {
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
            self.solver.clear_tables();
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, &board, Some(&*self.linear));
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

    pub fn ponder(&mut self, after_my_move: &Board, deadline: std::time::Instant) -> u64 {
        let base = self.nodes();
        if is_game_over(after_my_move) || after_my_move.movable_count() == 0 {
            return 0;
        }
        if !ponder_pays(after_my_move, self.config.solve_empties, self.config.band) {
            return 0;
        }
        let Some(pred) = self.tt_best(after_my_move) else {
            // Nothing searched here yet (second to move, or a book move): search their side for a reply.
            self.progress.clear();
            self.progress.set_kind(Progress::PONDER);
            self.stop.reset();
            self.search
                .best_move_deadline(after_my_move, PONDER_DEPTH, Some(deadline));
            return self.nodes() - base;
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
        self.stop.reset();
        self.search
            .best_move_deadline(&child, PONDER_DEPTH, Some(deadline));
        self.nodes() - base
    }

    pub fn tt_best(&self, board: &Board) -> Option<Position> {
        let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
        self.search
            .tt
            .best_move(h)
            .and_then(|i| Position::from_index(i as u32))
    }

    pub fn book_value(&self, board: &Board) -> Option<(f32, bool)> {
        let hints = self.book_hints(board)?;
        let best = hints
            .iter()
            .map(|(_, v)| *v)
            .fold(f32::NEG_INFINITY, f32::max);
        let learned = self.learned.get_raw(Book::key(board).0).is_some();
        Some((best, learned))
    }

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

    pub fn book_entry(&self, board: &Board) -> Option<(f32, bool, u8)> {
        let book = self.book.as_ref()?;
        let value = book.candidates(board)?.first()?.1;
        let depth = book.get_raw(Book::key(board).0).map_or(0, |e| e.depth);
        let learned = self.learned.get_raw(Book::key(board).0).is_some();
        Some((value, learned, depth))
    }

    pub fn book_is_complete(&self, board: &Board) -> bool {
        self.book.as_ref().is_some_and(|b| b.is_complete(board))
    }

    pub fn set_levels(&mut self, depth: u32, solve_empties: u8, band: u8) {
        self.config.depth = depth;
        self.config.solve_empties = solve_empties;
        self.config.band = band;
    }

    pub fn set_threads(&mut self, n: usize) {
        let n = n.max(1);
        self.config.threads = n;
        self.search.threads = n;
        self.solver.set_threads(n);
    }

    pub fn choose(&mut self, board: &Board) -> MoveEval {
        self.choose_within(board, None)
    }

    pub fn progress(&self) -> std::sync::Arc<Progress> {
        self.progress.clone()
    }

    pub fn choose_within(
        &mut self,
        board: &Board,
        deadline: Option<std::time::Instant>,
    ) -> MoveEval {
        self.choose_until(board, deadline, None)
    }

    /// A contested midgame move may run on to `extend`; solves keep `deadline`.
    pub fn choose_until(
        &mut self,
        board: &Board,
        deadline: Option<std::time::Instant>,
        extend: Option<std::time::Instant>,
    ) -> MoveEval {
        self.stop.reset();
        self.progress.clear();
        self.progress.set_kind(Progress::THINK);
        let mut hint = None;
        if let Some(book) = self.book.as_ref().filter(|_| self.config.use_book) {
            let hit = if self.config.book_tolerance > 0.0 {
                self.book_rand = self
                    .book_rand
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                book.probe_varied(board, self.config.book_tolerance, self.book_rand >> 11)
            } else {
                book.probe(board)
            };
            if let Some((pos, value, _depth)) = hit {
                let learned = self.learned.get_raw(Book::key(board).0).is_some();
                return MoveEval {
                    pos: Some(pos),
                    value,
                    exact: false,
                    from_book: true,
                    learned,
                    depth: 0,
                    cut: false,
                    ..Default::default()
                };
            }
            hint = book
                .candidates(board)
                .and_then(|v| v.first().map(|(p, _)| *p));
        }
        if let Some(p) = hint {
            self.hint_move(board, p);
        }
        let c = &self.config;
        if is_game_over(board) {
            return MoveEval {
                pos: None,
                value: final_score(board) as f32,
                exact: true,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
                ..Default::default()
            };
        }
        if board.empty_count() <= c.solve_empties {
            let backup = deadline.map(|d| self.backup_move(board, d));
            self.progress.set_kind(Progress::SOLVE);
            let watcher = self.watch_deadline(deadline);
            let r = self
                .solver
                .solve_with_eval(EndSolverMode::Perfect, board, Some(&*self.linear));
            self.solver_nodes += r.nodes;
            let cut = self.stop_watch_done(watcher);
            if cut {
                if let Some((value, pos)) = self.solver.warm_rung.filter(|(_, p)| p.is_some()) {
                    return MoveEval {
                        pos,
                        value: stone_scale(value as f32),
                        exact: false,
                        from_book: false,
                        learned: false,
                        depth: 0,
                        cut: true,
                        ..Default::default()
                    };
                }
                if let Some((pos, value)) = backup.filter(|(p, _)| p.is_some()) {
                    return MoveEval {
                        pos,
                        value: stone_scale(value),
                        exact: false,
                        from_book: false,
                        learned: false,
                        depth: BACKUP_DEPTH,
                        cut: true,
                        ..Default::default()
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
                ..Default::default()
            }
        } else if let Some(t) = selective_band(board.empty_count(), c.solve_empties, c.band) {
            let backup = deadline.map(|d| self.backup_move(board, d));
            self.progress.set_kind(Progress::SELECT);
            let watcher = self.watch_deadline(deadline);
            let r = self.solver.solve_selective(board, Some(&*self.linear), t);
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
                        ..Default::default()
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
                ..Default::default()
            }
        } else {
            let (pos, value, reached) = self.search.best_move_until(
                board,
                c.depth,
                deadline.map(crate::midgame::Deadline::new),
                extend,
            );
            let cut = self.stop.is_stopped();
            self.stop.reset();
            if std::env::var("ROOT_TRACE").is_ok() {
                let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
                eprintln!(
                    "  ret [{h:016x}] {:+8.2} (raw {value:+.2}) depth {reached} {:?}{} stop {}{}{}",
                    stone_scale(value),
                    pos,
                    if cut { " cut" } else { "" },
                    self.search.stop_reason.as_str(),
                    if self.search.extended {
                        " extended"
                    } else {
                        ""
                    },
                    match (self.search.second, self.search.gap) {
                        (Some(p), Some(g)) => format!(" second {p:?} gap >={g:.2}"),
                        (Some(p), None) => format!(" second {p:?} gap ?"),
                        (None, _) => String::new(),
                    }
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
                stop_reason: self.search.stop_reason,
                second: self.search.second,
                gap: self.search.gap,
                extended: self.search.extended,
            }
        }
    }

    pub fn learn_start(
        &self,
        start: Option<&str>,
        kifu: &str,
        learn_depth: u32,
    ) -> Result<crate::learn::BackupJob, String> {
        crate::learn::BackupJob::new(start, kifu, learn_depth.min(u8::MAX as u32) as u8)
    }

    pub fn learn_step(
        &mut self,
        job: &mut crate::learn::BackupJob,
        learn_depth: u32,
    ) -> Result<Option<crate::learn::BackupOutcome>, String> {
        use crate::learn::JobStep;
        self.stop.reset();
        let mut base = self
            .book
            .take()
            .map(|b| std::sync::Arc::try_unwrap(b).unwrap_or_else(|a| (*a).clone()))
            .unwrap_or_default();
        let mut learned = std::mem::take(&mut self.learned);
        let done = match job.next(&mut learned, &mut base) {
            JobStep::Search(b) => {
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
        self.book = (!base.is_empty()).then(|| std::sync::Arc::new(base));
        self.learned = learned;
        save.map_err(|e| format!("saving learned book {}: {e}", self.learn_path.display()))?;
        Ok(done)
    }

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
        self.book = book.map(std::sync::Arc::new);
        Ok(n)
    }

    pub fn eval_position(&mut self, board: &Board, depth: u32) -> MoveEval {
        self.stop.reset();
        self.eval_position_inner(board, depth)
    }

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
                ..Default::default()
            };
        }
        if board.empty_count() <= self.config.solve_empties {
            let r = self
                .solver
                .solve_with_eval(EndSolverMode::Perfect, board, Some(&*self.linear));
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: true,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
                ..Default::default()
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
                ..Default::default()
            }
        }
    }

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
                    ..Default::default()
                }
            } else if child.empty_count() <= self.config.solve_empties {
                let r = self.solver.solve_with_eval(
                    EndSolverMode::Perfect,
                    &child,
                    Some(&*self.linear),
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
                    ..Default::default()
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
                    ..Default::default()
                }
            };
            out.push((pos, ev));
        }
        self.search.threads = saved_threads;
        self.search.clear();
        out.sort_by(|a, b| b.1.value.total_cmp(&a.1.value));
        out
    }

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
                        ..Default::default()
                    }
                } else if u32::from(child.empty_count()) <= depth {
                    let r = self.solver.solve_with_eval(
                        EndSolverMode::Perfect,
                        &child,
                        Some(&*self.linear),
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
                        ..Default::default()
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
                        ..Default::default()
                    }
                };
                out.push((pos, ev));
            }
            out.sort_by(|a, b| b.1.value.total_cmp(&a.1.value));
            let go_on = on_pass(depth, &out, self.nodes() - base_nodes);
            if !go_on || all_exact || depth >= 60 {
                break;
            }
            depth += 1;
        }
        self.search.threads = saved_threads;
        self.search.clear();
    }
}

#[cfg(test)]
mod progress_tests {
    use super::Progress;
    use crate::Position;

    #[test]
    fn pondering_keeps_the_reply_it_assumed() {
        let p = Progress::default();
        p.clear();
        p.set_kind(Progress::PONDER);
        p.predict(Position::from_index(19).unwrap());
        assert_eq!(p.snapshot().4, Some(19), "the assumed reply is readable");

        p.reached(6, Position::from_index(42), 4.0);
        assert_eq!(p.snapshot().2, Some(42), "best move is the search's");
        assert_eq!(p.snapshot().4, Some(19), "the assumption is unchanged");

        p.clear();
        assert_eq!(p.snapshot().4, None);
    }

    #[test]
    fn clear_drops_the_flip() {
        let p = Progress::default();
        let mv = Position::from_index(19);

        p.set_kind(Progress::PONDER);
        p.flip.store(true, std::sync::atomic::Ordering::Relaxed);
        p.reached(6, mv, 4.0);
        assert_eq!(p.snapshot().3, Some(-4.0), "ponder stores negated");

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

#[cfg(test)]
mod assets_tests {
    use super::*;

    #[test]
    #[ignore = "requires weights/"]
    fn pondering_a_position_never_searched_finds_a_reply_to_ponder_on() {
        let mut engine = Engine::new(EngineConfig {
            use_book: false,
            ..Default::default()
        })
        .expect("engine");
        let board = Board::new();
        assert!(engine.tt_best(&board).is_none());

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
        assert!(engine.ponder(&board, deadline) > 0);
        assert!(engine.tt_best(&board).is_some());
    }

    #[test]
    #[ignore = "requires weights/"]
    fn dropping_an_engine_frees_the_network() {
        let config = EngineConfig::default();
        let assets = EngineAssets::load(&config).expect("assets");
        let net = assets.nnue.clone();
        let engine = Engine::with_assets(assets, config).expect("engine");
        assert!(
            std::sync::Arc::strong_count(&net) > 1,
            "the engine should hold the network it was given"
        );
        drop(engine);
        assert_eq!(
            std::sync::Arc::strong_count(&net),
            1,
            "dropping the engine must leave no handle behind"
        );
    }

    #[test]
    #[ignore = "requires weights/"]
    fn building_on_loaded_assets_reads_no_files() {
        let assets = EngineAssets::load(&EngineConfig::default()).expect("assets");
        let config = EngineConfig {
            weights: PathBuf::from("/nonexistent/linear.bin"),
            nnue: PathBuf::from("/nonexistent/nnue.bin"),
            ..Default::default()
        };
        Engine::with_assets(assets, config).expect("built without touching disk");
    }
}
