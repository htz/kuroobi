//! Engine session layer shared by the GUI, CLI and protocol frontends.

use std::path::PathBuf;

use crate::book::{Book, BookCandidate};
use crate::clock::ClockSearch;
use crate::linear::Linear;
use crate::midgame::{NnueSearch, SharedTt, StopHandle};
use crate::nnue::Nnue;
use crate::pattern::{LINEAR_PATTERNS, NNUE_PATTERNS};
use crate::solver::{final_score, EndSolverMode, EndSolverResult, Solver};
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
    pub threads: usize,
    pub mpc: bool,
    pub midgame_hash_bits: u32,
    pub solver_hash_bits: u32,
    pub weights: PathBuf,
    pub nnue: PathBuf,
    pub book: PathBuf,
    pub use_book: bool,
    pub book_tolerance: f32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            depth: 12,
            solve_empties: 18,
            threads: 4,
            mpc: true,
            midgame_hash_bits: 22,
            solver_hash_bits: 22,
            weights: PathBuf::from("weights/linear.bin"),
            nnue: PathBuf::from("weights/nnue.bin"),
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
    /// The ProbCut confidence the move was searched at, in sigmas; `None` for the midgame's default.
    pub mpc_t: Option<f32>,
}

#[derive(Debug, Default)]
pub struct Progress {
    pub kind: std::sync::atomic::AtomicU8,
    pub depth: std::sync::atomic::AtomicU32,
    pub best: std::sync::atomic::AtomicU32,
    pub milli: std::sync::atomic::AtomicI32,
}

impl Progress {
    pub const IDLE: u8 = 0;
    pub const THINK: u8 = 1;
    pub const SOLVE: u8 = 3;

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
    }

    pub fn reached(&self, depth: u32, best: Option<Position>, value: f32) {
        use std::sync::atomic::Ordering::Relaxed;
        self.depth.store(depth, Relaxed);
        self.best
            .store(best.map(|p| p.index() as u32).unwrap_or(64), Relaxed);
        if value.is_finite() {
            self.milli.store((value * 1000.0) as i32, Relaxed);
        }
    }

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
    linear: std::sync::Arc<Linear>,
    search: NnueSearch,
    /// Built on the first move played on a clock.
    clock: Option<ClockSearch>,
    nnue: std::sync::Arc<Nnue>,
    solver: Solver,
    config: EngineConfig,
    stop: StopHandle,
    book: Option<std::sync::Arc<Book>>,
    book_rand: u64,
    learned: Book,
    book_base: Option<Book>,
    learn_path: std::path::PathBuf,
    /// Nodes outside the midgame search: the solver's and the clock search's.
    other_nodes: u64,
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
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.load(&config.nnue)
            .map_err(|e| format!("nnue {}: {e}", config.nnue.display()))?;
        nn.quantize();
        Ok(EngineAssets {
            linear: std::sync::Arc::new(linear),
            nnue: std::sync::Arc::new(nn),
        })
    }
}

fn merged_book(base: Option<Book>, learned: &Book) -> Option<Book> {
    let mut book = base.filter(|b| !b.is_empty());
    if !learned.is_empty() {
        crate::learn::merge_learned(book.get_or_insert_with(Book::new), learned);
    }
    book
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
        let nnue = nn.clone();
        search.set_progress(Some(progress.clone()));
        search.threads = config.threads;
        search.mpc = config.mpc;
        let mut solver = Solver::new(config.solver_hash_bits);
        solver.set_nnue(nn, tt);
        solver.set_threads(config.threads);
        let stop = StopHandle::new();
        search.set_stop(Some(stop.clone()));
        solver.set_stop(Some(stop.clone()));
        let learn_path = config.book.with_file_name("book_learn.txt");
        let book_base = Book::load(&config.book).ok().filter(|b| !b.is_empty());
        let learned = Book::load(&learn_path).unwrap_or_default();
        let book = merged_book(book_base.clone(), &learned).map(std::sync::Arc::new);
        let book_rand = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15)
            | 1;
        Ok(Engine {
            linear,
            search,
            clock: None,
            nnue,
            solver,
            config,
            stop,
            book,
            book_base,
            book_rand,
            learned,
            learn_path,
            other_nodes: 0,
            progress,
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    fn watch_deadline(
        &self,
        deadline: Option<std::time::Instant>,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        Some(crate::midgame::Deadline::new(deadline?).watch(self.stop.clone()))
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

    /// The backup move comes back only when the deadline cut the solve.
    fn solve_within(
        &mut self,
        board: &Board,
        deadline: Option<std::time::Instant>,
        kind: u8,
        solve: impl FnOnce(&mut Solver, &Linear) -> EndSolverResult,
    ) -> (EndSolverResult, bool, Option<MoveEval>) {
        let backup = deadline.map(|d| self.backup_move(board, d));
        self.progress.set_kind(kind);
        let watcher = self.watch_deadline(deadline);
        let r = solve(&mut self.solver, &self.linear);
        self.other_nodes += r.nodes;
        let cut = self.stop_watch_done(watcher);
        let backup = backup
            .filter(|(p, _)| cut && p.is_some())
            .map(|(pos, value)| MoveEval {
                pos,
                value: stone_scale(value),
                depth: BACKUP_DEPTH,
                cut: true,
                ..Default::default()
            });
        (r, cut, backup)
    }

    pub fn nodes(&self) -> u64 {
        self.search.nodes + self.other_nodes
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
        if let Some(c) = self.clock.as_ref() {
            c.seed_move(board, pos);
        }
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

    pub fn book_node(&self, board: &Board) -> Option<(Vec<BookCandidate>, bool)> {
        let moves = self.book.as_ref()?.candidates_detailed(board)?;
        Some((moves, self.learned.has(board)))
    }

    pub fn clear_tables(&mut self) {
        self.search.clear();
        self.solver.clear_tables();
        if let Some(c) = self.clock.as_mut() {
            c.new_game();
        }
    }

    fn tt_best(&self, board: &Board) -> Option<Position> {
        let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
        self.search
            .tt
            .best_move(h)
            .and_then(|i| Position::from_index(i as u32))
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

    pub fn set_levels(&mut self, depth: u32, solve_empties: u8) {
        self.config.depth = depth;
        self.config.solve_empties = solve_empties;
    }

    pub fn set_threads(&mut self, n: usize) {
        let n = n.max(1);
        self.config.threads = n;
        self.search.threads = n;
        self.solver.set_threads(n);
        if let Some(c) = self.clock.as_mut() {
            c.set_threads(n);
        }
    }

    pub fn choose(&mut self, board: &Board) -> MoveEval {
        self.choose_within(board, None)
    }

    pub fn progress(&self) -> std::sync::Arc<Progress> {
        self.progress.clone()
    }

    /// A book move, or `None` after seeding the search with the book's best line.
    fn book_move(&mut self, board: &Board) -> Option<MoveEval> {
        let book = self.book.as_ref().filter(|_| self.config.use_book)?;
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
            return Some(MoveEval {
                pos: Some(pos),
                value,
                from_book: true,
                learned,
                ..Default::default()
            });
        }
        if let Some(p) = book
            .candidates(board)
            .and_then(|v| v.first().map(|(p, _)| *p))
        {
            self.hint_move(board, p);
        }
        None
    }

    /// A move on a game clock; `clock` is the time this side has left for the rest of the game.
    pub fn play_clock(&mut self, board: &Board, clock: std::time::Duration) -> MoveEval {
        self.stop.reset();
        self.progress.clear();
        self.progress.set_kind(Progress::THINK);
        if let Some(mv) = self.book_move(board) {
            return mv;
        }
        if is_game_over(board) {
            return MoveEval {
                value: final_score(board) as f32,
                exact: true,
                ..Default::default()
            };
        }
        let (nnue, bits, threads, stop) = (
            self.nnue.clone(),
            self.config.midgame_hash_bits,
            self.config.threads,
            self.stop.clone(),
        );
        let clock_search = self
            .clock
            .get_or_insert_with(|| ClockSearch::new(nnue, bits, threads, Some(stop)));
        let progress = self.progress.clone();
        let c = clock_search.choose(board, clock, |p| {
            progress.reached(p.depth as u32, Some(p.mv), p.score as f32)
        });
        self.other_nodes += c.nodes;
        let mut out = MoveEval {
            pos: c.mv,
            value: c.score as f32,
            depth: c.depth as u32,
            mpc_t: (c.depth > 0).then(|| c.margin as f32 / 2.0),
            ..Default::default()
        };
        if c.solve && !self.stop.is_stopped() && std::time::Instant::now() < c.deadline {
            self.progress.set_kind(Progress::SOLVE);
            let watcher = self.watch_deadline(Some(c.deadline));
            let r = self
                .solver
                .solve_with_eval(EndSolverMode::Perfect, board, Some(&*self.linear));
            self.other_nodes += r.nodes;
            out.cut = self.stop_watch_done(watcher);
            if !out.cut && r.best_move.is_some() {
                out.pos = r.best_move;
                out.value = r.value as f32;
                out.exact = true;
            }
        }
        self.stop.reset();
        out
    }

    pub fn choose_within(
        &mut self,
        board: &Board,
        deadline: Option<std::time::Instant>,
    ) -> MoveEval {
        self.stop.reset();
        self.progress.clear();
        self.progress.set_kind(Progress::THINK);
        if let Some(mv) = self.book_move(board) {
            return mv;
        }
        let c = &self.config;
        if is_game_over(board) {
            return MoveEval {
                value: final_score(board) as f32,
                exact: true,
                ..Default::default()
            };
        }
        if board.empty_count() <= c.solve_empties {
            let (r, cut, backup) =
                self.solve_within(board, deadline, Progress::SOLVE, |solver, linear| {
                    solver.solve_with_eval(EndSolverMode::Perfect, board, Some(linear))
                });
            if cut {
                if let Some((value, pos)) = self.solver.warm_rung.filter(|(_, p)| p.is_some()) {
                    return MoveEval {
                        pos,
                        value: stone_scale(value as f32),
                        cut: true,
                        ..Default::default()
                    };
                }
            }
            if let Some(backup) = backup {
                return backup;
            }
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: !cut,
                ..Default::default()
            }
        } else {
            let (pos, value, reached) = self.search.best_move_deadline(board, c.depth, deadline);
            let cut = self.stop.is_stopped();
            self.stop.reset();
            MoveEval {
                pos,
                value: stone_scale(value),
                exact: false,
                from_book: false,
                learned: false,
                depth: reached,
                cut,
                ..Default::default()
            }
        }
    }

    /// The line the tables still hold after `first`, `None` for a pass; it ends where an entry was overwritten.
    pub fn principal_variation(
        &self,
        board: &Board,
        first: Position,
        len: usize,
    ) -> Vec<Option<Position>> {
        let mut pv = vec![Some(first)];
        let mut b = *board;
        b.make_move_bits(first);
        while pv.len() < len {
            if b.movable() == 0 {
                b.pass();
                if b.movable() == 0 {
                    break;
                }
                pv.push(None);
            }
            let Some(p) = self
                .solver
                .table_move(&b)
                .or_else(|| self.clock.as_ref()?.table_move(&b))
                .or_else(|| self.tt_best(&b))
                .filter(|p| b.movable() >> p.index() & 1 == 1)
            else {
                break;
            };
            b.make_move_bits(p);
            pv.push(Some(p));
        }
        pv
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
        self.book =
            merged_book(Book::load(&self.config.book).ok(), &self.learned).map(std::sync::Arc::new);
        Ok(n)
    }

    pub fn eval_position(&mut self, board: &Board, depth: u32) -> MoveEval {
        self.stop.reset();
        self.eval_position_inner(board, depth)
    }

    fn eval_position_inner(&mut self, board: &Board, depth: u32) -> MoveEval {
        if is_game_over(board) {
            return MoveEval {
                value: final_score(board) as f32,
                exact: true,
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
                ..Default::default()
            }
        } else {
            let (pos, value) = self.search.best_move_valued(board, depth);
            MoveEval {
                pos,
                value: stone_scale(value),
                ..Default::default()
            }
        }
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
                        ..Default::default()
                    }
                } else if u32::from(child.empty_count()) <= depth {
                    let r = self.solver.solve_with_eval(
                        EndSolverMode::Perfect,
                        &child,
                        Some(&*self.linear),
                    );
                    self.other_nodes += r.nodes;
                    MoveEval {
                        pos: Some(pos),
                        value: stone_scale(-(r.value as f32)),
                        exact: true,
                        ..Default::default()
                    }
                } else {
                    all_exact = false;
                    self.search.clear();
                    let (_, v, reached) = self.search.best_move_deadline(&child, depth, None);
                    MoveEval {
                        pos: Some(pos),
                        value: stone_scale(-v),
                        depth: reached + 1, // child at d = d+1 plies from the parent
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
mod assets_tests {
    use super::*;

    #[test]
    #[ignore = "requires weights/"]
    fn the_principal_variation_is_a_legal_line_from_the_move() {
        let mut engine = Engine::new(EngineConfig {
            use_book: false,
            ..Default::default()
        })
        .expect("engine");
        engine.set_levels(12, 0);
        let board = Board::new();
        let mv = engine.choose_within(&board, None);
        let pv = engine.principal_variation(&board, mv.pos.unwrap(), 8);
        assert_eq!(pv[0], mv.pos);
        assert!(pv.len() >= 4, "{pv:?}");
        let mut b = board;
        for p in pv.into_iter().flatten() {
            if b.movable() == 0 {
                b.pass();
            }
            assert!(b.movable() & p.to_bit() != 0);
            b.make_move_bits(p);
        }
    }

    #[test]
    #[ignore = "requires weights/"]
    fn a_clock_move_is_legal_and_the_end_is_solved_exactly() {
        let mut engine = Engine::new(EngineConfig {
            use_book: false,
            ..Default::default()
        })
        .expect("engine");
        let t0 = std::time::Instant::now();
        let mv = engine.play_clock(&Board::new(), std::time::Duration::from_secs(60));
        assert!(Board::new().movable() & mv.pos.unwrap().to_bit() != 0);
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));

        let board = Board::from_string(
            "OOOOOOOOXOOXXX--XXOOXOO-XOXOOO--XOOOOX--XOOXOO--XOOOOO--XXXX---- O",
        )
        .unwrap();
        let end = engine.play_clock(&board, std::time::Duration::from_secs(60));
        let mut solver = Solver::new(20);
        let exact = solver.solve(EndSolverMode::Perfect, &board);
        assert!(end.exact);
        assert_eq!(end.value, exact.value as f32);
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
