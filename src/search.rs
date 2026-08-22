//! Midgame search: iterative-deepening alpha-beta (negamax) over the
//! pattern evaluator, with a small transposition table and move ordering.
//!
//! Scores are evaluator units (approximate final disc difference) from the
//! current player's perspective. When the remaining depth reaches the
//! terminal (game over) the exact score is used, scaled by SCORE_SCALE to
//! dominate heuristic values.

// Indexed loops here iterate in an order that matters (contiguous scans,
// SIMD-style unrolling), so the iterator lints are not taken. Search
// functions keep their long argument lists: bundling them into a struct
// would add per-call construction on a hot path.
#![allow(clippy::too_many_arguments)]

use crate::board::Board;
use crate::evaluator::Evaluator;
use crate::pattern_index::PatternIndices;
use crate::position::Position;
use crate::zobrist;

/// Terminal (exact) scores are worth this many evaluator points per disc,
/// so a proven win always outranks any heuristic evaluation.
const SCORE_SCALE: f32 = 1000.0;
/// Upper bound on any score (used as ±infinity).
const INF: f32 = f32::MAX / 4.0;
/// Null-window width for PVS probes (evaluator units are continuous f32,
/// so a "zero window" needs a small epsilon).
const PVS_EPSILON: f32 = 0.001;
/// Plies from the root at which a node may hand its remaining moves to other
/// threads. Splitting only at the root leaves the principal variation — the
/// biggest subtree — on one thread, which caps the speedup near 1.3x however
/// many cores are available.
const SPLIT_MAX_PLY: u8 = 8;
/// A node needs at least this much depth left to be worth splitting; below it
/// the subtrees are shorter than the hand-off.
const SPLIT_MIN_DEPTH: u8 = 3;

/// Terminal score with empties awarded to the winner (same convention as
/// the endgame solver and the game pipeline).
#[inline]
fn final_score_discs(board: &Board) -> i32 {
    let diff = board.score();
    let empties = board.empty_count() as i32;
    match diff.cmp(&0) {
        std::cmp::Ordering::Greater => diff + empties,
        std::cmp::Ordering::Less => diff - empties,
        std::cmp::Ordering::Equal => 0,
    }
}

/// Leaf evaluations are snapped to a half-disc grid inside the search:
/// continuous f32 values make almost every second child "slightly better",
/// which defeats PVS null-window confirmations and value-equal TT hits.
/// Half a disc is far below the evaluator's own noise floor.
#[inline]
fn quantize_leaf(v: f32) -> f32 {
    (v * 2.0).round() * 0.5
}

/// Maximum search ply (bounded by the number of board squares).
const MAX_PLY: usize = 64;

/// ProbCut (multi-prob-cut): at null-window nodes, a reduced-depth search
/// statistically predicts the full-depth result; when the prediction clears
/// the window by a confidence margin, the deep search is skipped. Cut
/// confidence is `MPC_T` standard deviations of the measured prediction
/// error (fitted on held-out positions with independent per-depth searches).
const MPC_MIN_DEPTH: u8 = 4;
/// Default confidence multiplier (a selectivity ladder typically spans
/// t = 1.1 (73%) .. 3.3 (99%); errors compound over the tree far less than
/// per-node odds suggest because both cut directions must fail to flip the
/// root).
const MPC_T: f32 = 1.1;

/// Experiment overrides for the strength-decomposition matches: which of
/// depth vs forward pruning costs us games. `MPC_MIN_DEPTH_ENV` moves the
/// depth at which probcut starts firing; `MPC_T_ENV` rescales its margin.
/// Both read once and default to the constants above.
pub(crate) fn mpc_min_depth() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MPC_MIN_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MPC_MIN_DEPTH)
    })
}

pub(crate) fn mpc_t_default() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MPC_T")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MPC_T)
    })
}
/// Allow probcut inside probcut's own reduced searches up to this depth of
/// nesting.
const MPC_MAX_LEVEL: u8 = 2;

/// Reduced probe depth: about a quarter of the remaining depth, keeping
/// the original parity (tempo parity strongly affects evaluation error).
#[inline]
fn mpc_reduced_depth(depth: u8) -> u8 {
    2 * (depth / 4) + (depth & 1)
}

/// Standard deviation (in discs) of `search(depth) - search(pc_depth)` as a
/// function of empties, fitted on positions from the training corpus with
/// this evaluator family. `pc_depth = 0` gives the static-eval error.
#[inline]
pub(crate) fn mpc_sigma(empties: u32, depth: u8, pc_depth: u8) -> f32 {
    const A: f32 = -0.068941;
    const B: f32 = 0.368775;
    const C: f32 = -0.713476;
    const QA: f32 = 0.010223;
    const QB: f32 = 0.647219;
    const QC: f32 = 4.050545;
    let s = A * empties as f32 + B * depth as f32 + C * pc_depth as f32;
    QA * s * s + QB * s + QC
}
/// Nodes at this remaining depth or more order children by full evaluation;
/// shallower nodes use cheap heuristics (mobility/killer/history). Full
/// evaluation of every child dominated per-node cost, and near the leaves
/// its extra cutoff precision cannot pay for itself.
const EVAL_ORDER_MIN_DEPTH: u8 = 6;

/// Static square bias for cheap ordering (file-major index): corners first,
/// X-squares last. Scaled to sit between mobility steps.
#[rustfmt::skip]
const fn square_bias() -> [i64; 64] {
    let corner = -(1i64 << 28);
    let x_sq = 1i64 << 28;
    let mut t = [0i64; 64];
    // Corners: A1=0, A8=7, H1=56, H8=63; X-squares: B2=9, B7=14, G2=49, G7=54
    t[0] = corner; t[7] = corner; t[56] = corner; t[63] = corner;
    t[9] = x_sq; t[14] = x_sq; t[49] = x_sq; t[54] = x_sq;
    t
}
const SQUARE_BIAS: [i64; 64] = square_bias();

#[derive(Clone, Copy, PartialEq)]
enum Bound {
    Exact,
    Lower,
    Upper,
}

#[derive(Clone, Copy)]
struct TtEntry {
    key: u64,
    black: u64,
    white: u64,
    depth: u8,
    value: f32,
    bound: Bound,
    best: Option<Position>,
    used: bool,
}

impl TtEntry {
    const EMPTY: TtEntry = TtEntry {
        key: 0,
        black: 0,
        white: 0,
        depth: 0,
        value: 0.0,
        bound: Bound::Exact,
        best: None,
        used: false,
    };
}

/// Result of a midgame search.
#[derive(Debug, Clone, Copy)]
pub struct SearchResult {
    pub best_move: Option<Position>,
    /// Evaluator-scale score from the side to move.
    pub value: f32,
    pub nodes: u64,
    /// Depth actually completed.
    pub depth: u8,
}

/// Iterative-deepening alpha-beta searcher over an evaluator.
/// Transposition table shared by every search thread.
///
/// Entries are wider than any atomic, so a concurrent reader can observe a
/// half-written one. That is safe here because a probe verifies the full
/// position (`key`, `black`, `white`) and the fields it then uses were
/// written before the position was: a torn entry fails the check and reads
/// as a miss. The table is advisory — losing or mis-reading an entry costs
/// nodes, never correctness — which is what makes lock-free sharing sound.
struct SharedTt {
    entries: std::cell::UnsafeCell<Vec<TtEntry>>,
    mask: u64,
}

// SAFETY: see the type comment — every access verifies the position it
// belongs to, and entries carry no pointers or ownership.
unsafe impl Sync for SharedTt {}
unsafe impl Send for SharedTt {}

impl SharedTt {
    fn new(bit_size: u32) -> SharedTt {
        let size = 1usize << bit_size;
        SharedTt {
            entries: std::cell::UnsafeCell::new(vec![TtEntry::EMPTY; size]),
            mask: (size - 1) as u64,
        }
    }

    /// SAFETY: callers must not hold two overlapping mutable views; every
    /// use below is a short read or a single-entry write.
    #[allow(clippy::mut_from_ref)]
    unsafe fn slots(&self) -> &mut Vec<TtEntry> {
        &mut *self.entries.get()
    }

    fn clear(&self) {
        // SAFETY: called between searches, with no other thread running.
        unsafe { self.slots() }.fill(TtEntry::EMPTY);
    }
}

pub struct Searcher {
    tt: std::sync::Arc<SharedTt>,
    nodes: u64,
    /// Search threads. Above 1 the helpers run the same iterative deepening
    /// on their own stacks and share only the table — the knowledge one
    /// thread proves shows up as another thread's cutoff. Splitting the tree
    /// explicitly would need the whole recursion restructured; this gets a
    /// useful part of the win from a table that already exists.
    pub threads: usize,
    /// Set when the main thread has its answer and the helpers should stop.
    stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Depth offset for a helper, so the threads do not all walk the same
    /// iteration in lockstep.
    depth_skew: u8,
    /// Enable ProbCut selective pruning (off by default: the search is then
    /// exact for its depth, which the equivalence tests rely on).
    pub mpc: bool,
    /// ProbCut confidence multiplier in error standard deviations.
    pub mpc_t: f32,
    /// Current probcut nesting depth (bounded by MPC_MAX_LEVEL).
    probcut_level: u8,
    /// Killer moves per ply: moves that recently caused a beta cutoff at
    /// the same distance from the root.
    killers: [[Option<Position>; 2]; MAX_PLY],
    /// History heuristic: cutoff credit per (player, square).
    history: [[u32; 64]; 2],
}

impl Searcher {
    /// `bit_size`: transposition table entries = 2^bit_size (~48 B each).
    pub fn new(bit_size: u32) -> Searcher {
        Searcher {
            tt: std::sync::Arc::new(SharedTt::new(bit_size)),
            nodes: 0,
            threads: 1,
            stop: None,
            depth_skew: 0,
            mpc: false,
            mpc_t: mpc_t_default(),
            probcut_level: 0,
            killers: [[None; 2]; MAX_PLY],
            history: [[0; 64]; 2],
        }
    }

    /// Invalidate all cached entries. Required whenever the evaluator's
    /// weights change (stored values embed the old evaluation) or when the
    /// same Searcher must serve a different evaluator: TT entries are keyed
    /// by position only, so cross-evaluator reuse silently corrupts search.
    pub fn clear(&mut self) {
        self.tt.clear();
    }

    #[inline]
    fn should_stop(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Search to `depth` plies with iterative deepening (better move
    /// ordering from shallower passes via the transposition table).
    /// Returns None best_move if the side to move must pass.
    pub fn search(&mut self, board: &Board, evaluator: &Evaluator, depth: u8) -> SearchResult {
        self.nodes = 0;
        self.probcut_level = 0;

        if board.movable() == 0 {
            return SearchResult {
                best_move: None,
                value: 0.0,
                nodes: 0,
                depth: 0,
            };
        }

        // Ordering heuristics are per-decision: fresh for this search call,
        // shared across the iterative-deepening iterations inside it.
        self.killers = [[None; 2]; MAX_PLY];
        self.history = [[0; 64]; 2];

        // (Aspiration windows were measured to interact unsoundly with
        // PVS null-window entries in the shared TT — root exactness broke.
        // PVS + ETC alone keep the exactness invariant.)
        // Helpers run the same deepening on their own stacks, skewed so the
        // threads are not all proving the same iteration at the same moment.
        // Everything they learn reaches this thread through the table.
        self.deepen(board, evaluator, depth)
    }

    /// One root iteration, split across threads.
    ///
    /// The first move is searched alone to establish alpha — that is the
    /// "young brother" that makes the rest cheap — and the remaining moves
    /// are then handed out to workers that probe with a null window and
    /// re-search only when a move actually beats the best so far. Workers
    /// share the table and nothing else, so each keeps its own killers and
    /// history and they diverge instead of retracing one another.
    ///
    /// A worker reads alpha when it picks up a move, so it may probe against
    /// a bound another worker has already improved. That costs a little
    /// search, never correctness: a stale (lower) alpha only makes the
    /// window wider than it needed to be.
    fn root_split(
        &mut self,
        board: &Board,
        evaluator: &Evaluator,
        depth: u8,
        hash: u64,
    ) -> (f32, Option<Position>) {
        let indexer = evaluator.indexer();
        let mover = board.player();
        let tt_move = self.tt_probe(board, hash).and_then(|e| e.best);

        let mut kids: Vec<(Position, Board, u64, i64)> = Vec::with_capacity(34);
        let mut m = board.movable();
        while m != 0 {
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            m &= m - 1;
            let mut child = *board;
            let flipped = child.make_move_bits(pos);
            let child_hash = zobrist::update_hash_on_move(hash, pos, flipped, mover);
            let key = if Some(pos) == tt_move {
                i64::MIN
            } else {
                let ix = indexer.init(child.black, child.white);
                (evaluator.eval_indices(&child, &ix) * 256.0) as i64
            };
            kids.push((pos, child, child_hash, key));
        }
        kids.sort_by_key(|k| k.3);

        // First move with the full window, on this thread.
        let mut ix = indexer.init(kids[0].1.black, kids[0].1.white);
        let alpha0 = -self.alpha_beta(
            &kids[0].1,
            evaluator,
            &mut ix,
            kids[0].2,
            depth - 1,
            1,
            -INF,
            INF,
            false,
        );
        if kids.len() == 1 || self.should_stop() {
            return (alpha0, Some(kids[0].0));
        }

        let best = std::sync::Mutex::new((alpha0, Some(kids[0].0)));
        let next = std::sync::atomic::AtomicUsize::new(1);
        let stop = self.stop.clone();
        let kids_ref = &kids;
        let nodes = std::sync::atomic::AtomicU64::new(0);

        std::thread::scope(|scope| {
            for _ in 0..self.threads {
                let mut w = self.worker();
                let best = &best;
                let next = &next;
                let nodes = &nodes;
                let stop = stop.clone();
                scope.spawn(move || {
                    loop {
                        if stop
                            .as_ref()
                            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
                        {
                            break;
                        }
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(k) = kids_ref.get(i) else { break };
                        let a = best.lock().unwrap().0;
                        let mut ix = indexer.init(k.1.black, k.1.white);
                        let probe = -w.alpha_beta(
                            &k.1,
                            evaluator,
                            &mut ix,
                            k.2,
                            depth - 1,
                            1,
                            -(a + PVS_EPSILON),
                            -a,
                            false,
                        );
                        let v = if probe > a {
                            let mut ix = indexer.init(k.1.black, k.1.white);
                            -w.alpha_beta(
                                &k.1,
                                evaluator,
                                &mut ix,
                                k.2,
                                depth - 1,
                                1,
                                -INF,
                                -a,
                                false,
                            )
                        } else {
                            probe
                        };
                        let mut g = best.lock().unwrap();
                        if v > g.0 {
                            *g = (v, Some(k.0));
                        }
                    }
                    nodes.fetch_add(w.nodes, std::sync::atomic::Ordering::Relaxed);
                });
            }
        });

        self.nodes += nodes.load(std::sync::atomic::Ordering::Relaxed);
        let g = *best.lock().unwrap();
        (g.0, g.1)
    }

    /// The move loop of `alpha_beta`, run across threads.
    ///
    /// The first child is searched here with the full window so the others
    /// have a real bound to probe against; the rest are handed out one at a
    /// time. Each worker owns its ordering state and re-derives the pattern
    /// indices for its child, so nothing but the table is shared.
    #[allow(clippy::too_many_arguments)]
    fn split_children(
        &mut self,
        board: &Board,
        evaluator: &Evaluator,
        children: &[(Position, u64, u64, i64)],
        hash: u64,
        depth: u8,
        ply: u8,
        alpha: f32,
        beta: f32,
    ) -> (f32, Option<Position>) {
        let _ = hash;
        let indexer = evaluator.indexer();
        // Children are rebuilt from the flip mask here too; the array the
        // caller hands over deliberately carries no boards.
        let kid = |k: &(Position, u64, u64, i64)| {
            let mut c = *board;
            c.apply_flips(k.0, k.2);
            c
        };
        let first = &children[0];
        let first_board = kid(first);
        let mut ix = indexer.init(first_board.black, first_board.white);
        let mut best_val = -self.alpha_beta(
            &first_board,
            evaluator,
            &mut ix,
            first.1,
            depth - 1,
            ply + 1,
            -beta,
            -alpha,
            false,
        );
        let mut best_move = Some(first.0);
        if best_val >= beta || self.should_stop() {
            return (best_val, best_move);
        }

        let best = std::sync::Mutex::new((best_val, best_move));
        let next = std::sync::atomic::AtomicUsize::new(1);
        let nodes = std::sync::atomic::AtomicU64::new(0);
        let cut = std::sync::atomic::AtomicBool::new(false);
        let stop = self.stop.clone();
        let threads = self.threads.min(children.len() - 1);

        std::thread::scope(|scope| {
            for _ in 0..threads {
                let mut w = self.worker();
                let (best, next, nodes, cut) = (&best, &next, &nodes, &cut);
                let stop = stop.clone();
                scope.spawn(move || {
                    loop {
                        if cut.load(std::sync::atomic::Ordering::Relaxed)
                            || stop
                                .as_ref()
                                .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
                        {
                            break;
                        }
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(k) = children.get(i) else { break };
                        let a = best.lock().unwrap().0.max(alpha);
                        let kb = kid(k);
                        let mut ix = indexer.init(kb.black, kb.white);
                        let probe = -w.alpha_beta(
                            &kb,
                            evaluator,
                            &mut ix,
                            k.1,
                            depth - 1,
                            ply + 1,
                            -(a + PVS_EPSILON),
                            -a,
                            false,
                        );
                        let v = if probe > a && probe < beta {
                            let mut ix = indexer.init(kb.black, kb.white);
                            -w.alpha_beta(
                                &kb,
                                evaluator,
                                &mut ix,
                                k.1,
                                depth - 1,
                                ply + 1,
                                -beta,
                                -a,
                                false,
                            )
                        } else {
                            probe
                        };
                        let mut g = best.lock().unwrap();
                        if v > g.0 {
                            *g = (v, Some(k.0));
                        }
                        if g.0 >= beta {
                            cut.store(true, std::sync::atomic::Ordering::Relaxed);
                            break;
                        }
                    }
                    nodes.fetch_add(w.nodes, std::sync::atomic::Ordering::Relaxed);
                });
            }
        });

        self.nodes += nodes.load(std::sync::atomic::Ordering::Relaxed);
        let g = *best.lock().unwrap();
        best_val = g.0;
        best_move = g.1;
        (best_val, best_move)
    }

    /// A worker for `root_split`: shares the table, keeps its own ordering
    /// state, and searches sequentially inside its subtree.
    fn worker(&self) -> Searcher {
        Searcher {
            tt: std::sync::Arc::clone(&self.tt),
            nodes: 0,
            threads: 1,
            stop: self.stop.clone(),
            depth_skew: 0,
            mpc: self.mpc,
            mpc_t: self.mpc_t,
            probcut_level: 0,
            killers: [[None; 2]; MAX_PLY],
            history: [[0; 64]; 2],
        }
    }

    /// Iterative deepening on one thread.
    fn deepen(&mut self, board: &Board, evaluator: &Evaluator, depth: u8) -> SearchResult {
        let mut indices = evaluator.indexer().init(board.black, board.white);
        let mut best_move = None;
        let mut value = 0.0f32;
        let mut completed = 0u8;

        // A helper starts one iteration further in so that the threads
        // populate different parts of the table first.
        let start = 1 + self.depth_skew.min(depth.saturating_sub(1));
        for d in start..=depth {
            if self.should_stop() {
                break;
            }
            let hash = zobrist::compute_hash(board.black, board.white, board.player());
            let (v, mv) = if self.threads > 1 && d >= 3 {
                self.root_split(board, evaluator, d, hash)
            } else {
                let v =
                    self.alpha_beta(board, evaluator, &mut indices, hash, d, 0, -INF, INF, false);
                // Root best move comes from the TT entry just stored
                (v, self.tt_probe(board, hash).and_then(|e| e.best))
            };
            if self.should_stop() {
                break;
            }
            if mv.is_some() {
                best_move = mv;
            }
            value = v;
            completed = d;
        }

        SearchResult {
            best_move,
            value,
            nodes: self.nodes,
            depth: completed,
        }
    }

    fn tt_probe(&self, board: &Board, hash: u64) -> Option<TtEntry> {
        // SAFETY: a short read; a concurrently written entry fails the
        // position check below and is discarded.
        let e = unsafe { self.tt.slots() }[(hash & self.tt.mask) as usize];
        (e.used && e.key == hash && e.black == board.black && e.white == board.white).then_some(e)
    }

    #[allow(clippy::too_many_arguments)]
    fn tt_store(
        &mut self,
        board: &Board,
        hash: u64,
        depth: u8,
        value: f32,
        bound: Bound,
        best: Option<Position>,
    ) {
        // SAFETY: a single-entry write; readers verify the position.
        let e = &mut unsafe { self.tt.slots() }[(hash & self.tt.mask) as usize];
        // Depth-preferred replacement
        if !e.used || e.depth <= depth {
            *e = TtEntry {
                key: hash,
                black: board.black,
                white: board.white,
                depth,
                value,
                bound,
                best,
                used: true,
            };
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Try a probable cutoff at a null-window node: a reduced-depth search
    /// around a margin-shifted window. Returns the window bound to fail
    /// with when the probe clears the margin, None to search normally.
    /// The result is NOT stored in the TT for this depth — only the probe's
    /// own (depth-tagged) entries are, so no unproven bound masquerades as
    /// a full-depth result.
    #[allow(clippy::too_many_arguments)]
    fn probcut(
        &mut self,
        board: &Board,
        evaluator: &Evaluator,
        indices: &mut PatternIndices,
        hash: u64,
        depth: u8,
        ply: u8,
        alpha: f32,
        beta: f32,
    ) -> Option<f32> {
        let empties = board.empty_count() as u32;
        let pc_depth = mpc_reduced_depth(depth);
        let t = self.mpc_t;
        let pc_error = t * mpc_sigma(empties, depth, pc_depth);
        // Gate on the static eval first: only probe when depth 0 already
        // points past the window (average of both prediction errors).
        let eval_error =
            t * 0.5 * (mpc_sigma(empties, depth, 0) + mpc_sigma(empties, depth, pc_depth));
        let eval_score = evaluator.eval_indices(board, indices);

        // Probable fail-high
        let pc_beta = beta + pc_error;
        if eval_score >= beta - eval_error && pc_beta < 64.0 {
            self.probcut_level += 1;
            let v = self.alpha_beta(
                board,
                evaluator,
                indices,
                hash,
                pc_depth,
                ply,
                pc_beta - PVS_EPSILON,
                pc_beta,
                false,
            );
            self.probcut_level -= 1;
            if v >= pc_beta {
                return Some(beta);
            }
        }

        // Probable fail-low
        let pc_alpha = alpha - pc_error;
        if eval_score < alpha + eval_error && pc_alpha > -64.0 {
            self.probcut_level += 1;
            let v = self.alpha_beta(
                board,
                evaluator,
                indices,
                hash,
                pc_depth,
                ply,
                pc_alpha,
                pc_alpha + PVS_EPSILON,
                false,
            );
            self.probcut_level -= 1;
            if v <= pc_alpha {
                return Some(alpha);
            }
        }

        None
    }

    fn alpha_beta(
        &mut self,
        board: &Board,
        evaluator: &Evaluator,
        indices: &mut PatternIndices,
        hash: u64,
        depth: u8,
        ply: u8,
        mut alpha: f32,
        mut beta: f32,
        passed: bool,
    ) -> f32 {
        self.nodes += 1;

        // Leaf fast path: a TT probe is a near-certain cache miss and can
        // only save the (cheaper) eval itself — skip the table entirely.
        // Handles passes inline so depth-0 nodes never need a hash, which
        // in turn lets depth-1 nodes skip computing child hashes at all.
        let moves = board.movable();
        if depth == 0 {
            if moves != 0 {
                return quantize_leaf(evaluator.eval_indices(board, indices));
            }
            let mut child = *board;
            child.pass();
            if child.movable() == 0 {
                // Both sides stuck: exact terminal score
                return final_score_discs(board) as f32 * SCORE_SCALE;
            }
            self.nodes += 1; // the pass "node", as the recursion counted it
            return -quantize_leaf(evaluator.eval_indices(&child, indices));
        }

        let orig_alpha = alpha;
        let mut tt_move: Option<Position> = None;

        if let Some(e) = self.tt_probe(board, hash) {
            tt_move = e.best;
            if e.depth >= depth {
                match e.bound {
                    Bound::Exact => return e.value,
                    Bound::Lower => alpha = alpha.max(e.value),
                    Bound::Upper => beta = beta.min(e.value),
                }
                if alpha >= beta {
                    return e.value;
                }
            }
        }

        if moves == 0 {
            if passed {
                // Game over: exact score dominates heuristics
                return final_score_discs(board) as f32 * SCORE_SCALE;
            }
            let mut child = *board;
            child.pass();
            let child_hash = zobrist::update_hash_on_pass(hash);
            // A pass changes no discs, so `indices` carries over unchanged.
            return -self.alpha_beta(
                &child, evaluator, indices, child_hash, depth, ply, -beta, -alpha, true,
            );
        }

        if depth == 0 {
            return evaluator.eval_indices(board, indices);
        }

        // ProbCut: only at null-window nodes (PVS probes), never on the PV.
        if self.mpc
            && depth >= mpc_min_depth()
            && self.probcut_level < MPC_MAX_LEVEL
            && ply > 0
            && beta - alpha <= PVS_EPSILON * 1.5
        {
            if let Some(v) = self.probcut(board, evaluator, indices, hash, depth, ply, alpha, beta)
            {
                return v;
            }
        }

        // Move ordering (ascending key = searched first). TT move always
        // leads. Deep nodes order by full child evaluation (few nodes, big
        // subtrees: precision pays). Shallow nodes use cheap heuristics:
        // killers, opponent mobility, history credit and corner/X bias —
        // evaluating every child there cost more than it cut.
        let indexer = evaluator.indexer();
        let mover = board.player();
        let use_eval_order = depth >= EVAL_ORDER_MIN_DEPTH;
        let [killer0, killer1] = self.killers[ply as usize];
        // Fixed-size stack buffer: a position has at most 33 legal moves,
        // and a heap allocation per node showed up as real per-node cost.
        // No child board in here. Materialising one per legal move meant
        // writing 34 * 48 bytes of stack at every node — including the copies
        // of `*board` — when a cut node searches one child and throws the rest
        // away. The flip mask is enough to rebuild a child with `apply_flips`,
        // which costs nothing next to recomputing the flip, so the array holds
        // 34 * 32 bytes of plain zeros instead.
        let mut children = [(Position(0), 0u64, 0u64, 0i64); 34];
        let mut n_children = 0usize;
        let mut m = moves;
        while m != 0 {
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            m &= m - 1;
            let mut child = *board;
            let flipped = child.make_move_bits(pos);
            // Depth-1 children are leaves and never touch the TT.
            let child_hash = if depth == 1 {
                0
            } else {
                zobrist::update_hash_on_move(hash, pos, flipped, mover)
            };
            let order_key: i64 = if Some(pos) == tt_move {
                i64::MIN
            } else if use_eval_order {
                // opponent's view: lower = better for us. Restoring the
                // indices from a saved copy beats calling `undo`: undo walks
                // the flipped discs and reads the pattern table for each,
                // while `PatternIndices` is 160 flat bytes. Ordering pays this
                // for *every* legal move, so it is the hottest undo in the
                // search (measured 16.7% of node time against 17.9% for the
                // evaluation itself).
                let saved = *indices;
                indexer.apply(indices, pos, flipped, mover);
                // The i8 ordering table, not the f32 exact one. Ordering walks
                // every legal move at every deep node, and each mask is a
                // random probe into the stage's table — so what it costs is
                // cache misses, and the i8 table is a quarter the size. The
                // endgame solver already ordered this way; the midgame was
                // still paying full precision for a comparison key.
                let v = evaluator.eval_order_bb(
                    child.player_bb(),
                    child.opponent_bb(),
                    child.player(),
                    indices,
                );
                *indices = saved;
                (v * 256.0) as i64
            } else if Some(pos) == killer0 {
                i64::MIN + 1
            } else if Some(pos) == killer1 {
                i64::MIN + 2
            } else if depth >= 2 {
                ((child.movable_count() as i64) << 32) + SQUARE_BIAS[pos.index() as usize]
                    - ((self.history[mover.index()][pos.index() as usize] as i64) << 8)
            } else {
                0
            };
            children[n_children] = (pos, child_hash, flipped, order_key);
            n_children += 1;
        }
        let children = &mut children[..n_children];
        // No full sort. A cut node searches one or two children and throws the
        // rest away, so ordering all of them is work nobody reads; the loop
        // below picks the next best on demand
        // instead. Selection is O(k*n) for the k children
        // actually searched against O(n log n) plus the element moves a sort
        // costs, and k is almost always 1.
        let order_children = depth >= 2 || tt_move.is_some();

        // Enhanced transposition cutoff: a child entry that already proves
        // our fail-high ends this node without any search.
        if depth >= 4 {
            for (pos, child_hash, flipped, _) in children.iter() {
                let mut child = *board;
                child.apply_flips(*pos, *flipped);
                if let Some(e) = self.tt_probe(&child, *child_hash) {
                    if e.depth >= depth - 1 && !matches!(e.bound, Bound::Lower) && -e.value >= beta
                    {
                        return -e.value;
                    }
                }
            }
        }

        // Young-brothers-wait: prove the first child on this thread, then
        // let other threads take the rest. Splitting before that would have
        // every thread searching against an unproven window.
        if self.threads > 1 && ply < SPLIT_MAX_PLY && depth >= SPLIT_MIN_DEPTH && n_children > 2 {
            let (v, mv) = self.split_children(
                board,
                evaluator,
                &children[..n_children],
                hash,
                depth,
                ply,
                alpha,
                beta,
            );
            let bound = if v <= orig_alpha {
                Bound::Upper
            } else if v >= beta {
                Bound::Lower
            } else {
                Bound::Exact
            };
            self.tt_store(board, hash, depth, v, bound, mv);
            return v;
        }

        let mut best_val = -INF;
        let mut best_move = None;

        for i in 0..children.len() {
            if order_children {
                // Pull the best remaining child into slot `i`.
                let mut best_j = i;
                for j in i + 1..children.len() {
                    if children[j].3 < children[best_j].3 {
                        best_j = j;
                    }
                }
                children.swap(i, best_j);
            }
            let (pos, child_hash, flipped, _) = &children[i];
            let mut child = *board;
            child.apply_flips(*pos, *flipped);
            let child = &child;
            let saved = *indices;
            indexer.apply(indices, *pos, *flipped, mover);
            // PVS: full window on the first child, then null-window probes
            // with a full re-search only when a child actually improves.
            let v = if i == 0 || alpha >= beta {
                -self.alpha_beta(
                    child,
                    evaluator,
                    indices,
                    *child_hash,
                    depth - 1,
                    ply + 1,
                    -beta,
                    -alpha,
                    false,
                )
            } else {
                let probe = -self.alpha_beta(
                    child,
                    evaluator,
                    indices,
                    *child_hash,
                    depth - 1,
                    ply + 1,
                    -(alpha + PVS_EPSILON),
                    -alpha,
                    false,
                );
                if probe > alpha && probe < beta {
                    // Re-search with the regular window: using probe as the
                    // lower bound is unsound under TT-induced instability
                    -self.alpha_beta(
                        child,
                        evaluator,
                        indices,
                        *child_hash,
                        depth - 1,
                        ply + 1,
                        -beta,
                        -alpha,
                        false,
                    )
                } else {
                    probe
                }
            };
            *indices = saved;
            if v > best_val {
                best_val = v;
                best_move = Some(*pos);
                if v > alpha {
                    alpha = v;
                }
            }
            if alpha >= beta {
                // Credit the cutoff move for future ordering
                let ply = ply as usize;
                if self.killers[ply][0] != Some(*pos) {
                    self.killers[ply][1] = self.killers[ply][0];
                    self.killers[ply][0] = Some(*pos);
                }
                self.history[mover.index()][pos.index() as usize] +=
                    (depth as u32) * (depth as u32);
                break;
            }
        }

        let bound = if best_val <= orig_alpha {
            Bound::Upper
        } else if best_val >= beta {
            Bound::Lower
        } else {
            Bound::Exact
        };
        self.tt_store(board, hash, depth, best_val, bound, best_move);

        best_val
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::EGAROUCID_PATTERNS;

    fn trained_evaluator() -> Evaluator {
        // A tiny hand-trained evaluator: reward corner ownership so the
        // search has a real signal to optimize.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();
        // Train stages 0..8 lightly toward positive for the initial-ish
        // positions so eval() is non-degenerate.
        let mut opt = crate::evaluator::SgdOptimizer::new(0.001, 1.0);
        for _ in 0..20 {
            e.update_weights_with(&b, 4.0, &mut opt);
        }
        e
    }

    #[test]
    fn test_search_returns_legal_move() {
        let e = trained_evaluator();
        let mut s = Searcher::new(14);
        let b = Board::new();
        for depth in 1..=5 {
            let r = s.search(&b, &e, depth);
            let best = r.best_move.expect("moves exist");
            assert!(b.movable() & best.to_bit() != 0, "depth {depth}: legal");
            assert_eq!(r.depth, depth);
            assert!(r.nodes > 0);
        }
    }

    #[test]
    fn test_search_pass_position() {
        let e = trained_evaluator();
        let mut s = Searcher::new(12);
        let mut b = Board::new();
        b.black = 1u64 << 0;
        b.white = 1u64 << 8;
        b.empty_count = 62;
        b.player = crate::color::Color::White; // White has no move
        let r = s.search(&b, &e, 4);
        assert_eq!(r.best_move, None);
    }

    #[test]
    fn test_deeper_search_finds_endgame_win() {
        // Near the end of a game the search should return terminal-scaled
        // scores (|value| >= SCORE_SCALE) once it reads to the end.
        let e = trained_evaluator();
        let mut s = Searcher::new(14);

        // Play a deterministic game down to 6 empties
        let mut b = Board::new();
        let mut ply = 0;
        while b.empty_count() > 6 {
            let moves = b.movable();
            if moves == 0 {
                let mut p = b;
                p.pass();
                if p.movable() == 0 {
                    break;
                }
                b = p;
                continue;
            }
            let bit = if ply % 2 == 0 {
                moves.trailing_zeros()
            } else {
                63 - moves.leading_zeros()
            };
            b.make_move_unchecked(Position::from_index(bit).unwrap());
            ply += 1;
        }

        if b.movable() != 0 && b.empty_count() >= 2 {
            let r = s.search(&b, &e, 10); // deep enough to hit terminal
            assert!(
                r.value.abs() >= SCORE_SCALE || r.value == 0.0,
                "deep search near game end must return exact-scaled score, got {}",
                r.value
            );
        }
    }

    /// Deterministic pseudo-random position with the given empty count.
    fn position_with_empties(empties: u8, seed: u64) -> Board {
        let mut board = Board::new();
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        while board.empty_count() > empties {
            let moves = board.movable();
            if moves == 0 {
                let mut p = board;
                p.pass();
                if p.movable() == 0 {
                    break;
                }
                board = p;
                continue;
            }
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let count = moves.count_ones() as u64;
            let mut nth = (state >> 33) % count;
            let mut m = moves;
            while nth > 0 {
                m &= m - 1;
                nth -= 1;
            }
            board.make_move_unchecked(Position::from_index(m.trailing_zeros()).unwrap());
        }
        board
    }

    /// Plain fixed-depth negamax mirroring alpha_beta's semantics (eval at
    /// depth 0, pass keeps depth, terminal scaled) with no TT, no ordering,
    /// no windows. Alpha-beta must return exactly this value at the root
    /// regardless of move ordering — the invariant that keeps ordering
    /// changes honest.
    fn reference_negamax(board: &Board, evaluator: &Evaluator, depth: u8, passed: bool) -> f32 {
        let moves = board.movable();
        if moves == 0 {
            if passed {
                return final_score_discs(board) as f32 * SCORE_SCALE;
            }
            let mut child = *board;
            child.pass();
            return -reference_negamax(&child, evaluator, depth, true);
        }
        if depth == 0 {
            return quantize_leaf(evaluator.eval(board));
        }
        let mut best = -INF;
        let mut m = moves;
        while m != 0 {
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            m &= m - 1;
            let mut child = *board;
            child.make_move_bits(pos);
            let v = -reference_negamax(&child, evaluator, depth - 1, false);
            if v > best {
                best = v;
            }
        }
        best
    }

    #[test]
    fn test_search_value_equals_reference_negamax() {
        let e = trained_evaluator();
        let mut s = Searcher::new(14);
        for seed in 1..=6u64 {
            for empties in [40u8, 30, 22] {
                let b = position_with_empties(empties, seed);
                if b.is_game_over() || b.movable() == 0 {
                    continue;
                }
                for depth in 1..=4u8 {
                    let searched = s.search(&b, &e, depth);
                    let reference = reference_negamax(&b, &e, depth, false);
                    assert_eq!(
                        searched.value, reference,
                        "seed {seed} empties {empties} depth {depth}: \
                         ordering must not change the root value"
                    );
                }
            }
        }
    }

    #[test]
    fn test_full_depth_search_matches_endgame_solver() {
        // When depth >= empties the search reads to terminal everywhere, so
        // its value must equal the endgame solver's Perfect result (scaled).
        // This validates alpha-beta, pass handling, and TT correctness
        // independent of evaluator quality.
        let e = trained_evaluator();
        let mut searcher = Searcher::new(15);
        let mut solver = crate::solver::Solver::new(15);

        for seed in 1..=4u64 {
            let b = position_with_empties(9, seed);
            if b.is_game_over() || b.movable() == 0 {
                continue;
            }
            let exact = solver.solve(crate::solver::EndSolverMode::Perfect, &b);
            let searched = searcher.search(&b, &e, b.empty_count() + 2);
            assert_eq!(
                searched.value,
                exact.value as f32 * SCORE_SCALE,
                "seed {seed}: full-depth search must equal solver's exact score"
            );
        }
    }
}
