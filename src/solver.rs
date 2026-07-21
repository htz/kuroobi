//! Endgame solver: PVS (principal variation search) with a Zobrist-keyed
//! transposition table, move ordering, and specialized last-4/3/2/1 fast
//! paths.
//!
//! Scores are disc differences from the current player's perspective, with
//! the empty-square bonus applied to the winner (Board::score semantics).

use crate::bitboard;
use crate::board::Board;
use crate::evaluator::Evaluator;
use crate::pattern_index::{PatternIndexer, PatternIndices};
use crate::color::Color;
use crate::position::Position;
use crate::zobrist;

/// Search depth thresholds (empties remaining) for switching strategies.
const PVS_LIMIT: u8 = 7;
const MOVE_ORDERING_LIMIT: u8 = 7;
/// From this many empties upward, an evaluator (when provided) orders
/// moves instead of the static heuristic.
const EVAL_ORDER_EMPTIES: u8 = 14;
/// From this many empties upward, ordering refines the evaluation with a
/// one-ply lookahead (max over the opponent's replies).
const DEEP_ORDER_EMPTIES: u8 = 16;
/// Terminal score with the empty-square bonus awarded to the winner
/// (FFO convention; also what the game pipeline records). The old
/// plain disc difference disagreed whenever a game ended with empties left
/// — last1 already awarded its single empty, the general terminals did not.
#[inline]
fn final_score(board: &Board) -> i32 {
    let diff = board.score();
    let empties = board.empty_count() as i32;
    match diff.cmp(&0) {
        std::cmp::Ordering::Greater => diff + empties,
        std::cmp::Ordering::Less => diff - empties,
        std::cmp::Ordering::Equal => 0,
    }
}

/// Wipeout: a side with no discs can never move again, so the game is
/// over the moment it happens (worth handling explicitly; without it
/// the search still terminates but only after mobility churn).
#[inline]
fn wipeout_score(board: &Board) -> Option<i32> {
    if board.opponent_bb() == 0 {
        return Some(64);
    }
    if board.player_bb() == 0 {
        return Some(-64);
    }
    None
}

/// Stability cutoff precondition: the bound 64 - 2*S can only cut when the
/// opponent has at least ceil((64-alpha)/2) stable discs, so their total
/// disc count (cheap popcount) must reach that first.
#[inline]
fn stability_cut(board: &Board, alpha: i32, beta: i32) -> Option<i32> {
    // Upper bound via the opponent's stable discs (fail low)
    let need = (64 - alpha + 1) / 2;
    if need <= 32 && (board.opponent_bb().count_ones() as i32) >= need {
        let bound =
            64 - 2 * crate::stability::stable_count(board.opponent_bb(), board.player_bb()) as i32;
        if bound <= alpha {
            return Some(bound);
        }
    }
    // Lower bound via our own stable discs (fail high). Nearly free on
    // balanced positions thanks to the popcount gate, and it collapses
    // one-sided positions (e.g. FFO#59) that otherwise explode.
    let need = (64 + beta + 1) / 2;
    if need <= 32 && (board.player_bb().count_ones() as i32) >= need {
        let bound =
            2 * crate::stability::stable_count(board.player_bb(), board.opponent_bb()) as i32 - 64;
        if bound >= beta {
            return Some(bound);
        }
    }
    None
}
/// From this many empties upward, ordering uses a two-ply lookahead.
const DEEP2_ORDER_EMPTIES: u8 = 21;
/// Enhanced transposition cutoff: from this many empties upward, probe
/// every child's hash entry before searching — a proven fail-high there
/// cuts this node without any search.
const ETC_EMPTIES: u8 = 8;
/// Deep solves run an evaluation-guided iterative pre-search that fills
/// the table with move-ordering seeds.
/// Ordering weight of one opponent reply, in eighths of a disc (the same
/// scale the evaluation term uses).
const MOBILITY_ORDER_WEIGHT: i32 = 12;
/// Slack below the node's alpha for the ordering lookahead's window.
const SORT_ALPHA_DELTA: i32 = 8;
/// Ordering weight of one stable edge disc, in eighths of a disc.
const EDGE_STABILITY_ORDER_WEIGHT: i32 = 1;
/// Half-width of the first aspiration window around the warm-up score.
const ASPIRATION_WIDTH: i32 = 6;
/// Half-width used by the warm-up passes, whose centre is only an
/// evaluation estimate rather than a searched score.
const WARM_ASPIRATION_WIDTH: i32 = 6;
/// Depth of the evaluation search that centres the first warm-up window.
const ESTIMATE_DEPTH: u8 = 8;
/// Warm-up passes only prune at this many empties or more.
const SELECTIVE_MIN_EMPTIES: u8 = 14;
/// Depth of the evaluation probe used by a warm-up pass.
const SELECTIVE_PROBE_DEPTH: u8 = 4;
/// Roots this deep run the warm-up ladder before the exact pass. Below 20
/// empties the exact search is cheap enough that the pass cannot pay for
/// itself (measured on FFO1-19: +6% nodes and +24% time at 16, +14%/+62%
/// at 14), while from 20 up it pays for itself several times over.
const SELECTIVE_PASS_MIN_EMPTIES: u8 = 20;
/// Confidence levels (standard deviations) of the warm-up passes, from
/// most selective to least. Each pass is aspirated around the previous
/// pass's score, so the estimate handed to the exact search converges —
/// an estimate that is off by even two discs makes the exact search pay
/// for a failed window, which is the dominant cost on hard positions.
const SELECTIVE_LADDER: [f32; 1] = [1.8];

/// Squares adjacent to each square (used to skip moves that cannot flip:
/// a legal move must touch at least one opponent disc).
/// Index = file-major square; built at first use.
fn neighbour_bit(sq: u8) -> u64 {
    // Compute the 8-neighbourhood mask in file-major layout.
    // (Small enough to compute on the fly; the compiler folds it well.)
    let file = (sq / 8) as i32;
    let rank = (sq % 8) as i32;
    let mut mask = 0u64;
    for df in -1..=1i32 {
        for dr in -1..=1i32 {
            if df == 0 && dr == 0 {
                continue;
            }
            let f = file + df;
            let r = rank + dr;
            if (0..8).contains(&f) && (0..8).contains(&r) {
                mask |= 1u64 << (f * 8 + r);
            }
        }
    }
    mask
}

/// Precomputed neighbour masks for all 64 squares.
struct NeighbourTable([u64; 64]);

impl NeighbourTable {
    fn new() -> Self {
        let mut t = [0u64; 64];
        for (sq, slot) in t.iter_mut().enumerate() {
            *slot = neighbour_bit(sq as u8);
        }
        NeighbourTable(t)
    }

    #[inline]
    fn get(&self, sq: u8) -> u64 {
        self.0[sq as usize]
    }
}

// ---------------------------------------------------------------------------
// Quadrant parity (odd/even number of empties per board quadrant).
// Endgame heuristic: prefer moves in quadrants with an odd number of empties.
// ---------------------------------------------------------------------------

#[inline]
fn quadrant_id(sq: u8) -> u8 {
    let file = sq / 8;
    let rank = sq % 8;
    match (file < 4, rank < 4) {
        (true, true) => 1,
        (false, true) => 2,
        (true, false) => 4,
        (false, false) => 8,
    }
}

/// Board mask of each quadrant (file-major), indexed by quadrant_id bit.
const QUADRANT_MASKS: [(u8, u64); 4] = [
    (1, 0x0000_0000_0F0F_0F0F), // files 0-3, ranks 0-3
    (2, 0x0F0F_0F0F_0000_0000), // files 4-7, ranks 0-3
    (4, 0x0000_0000_F0F0_F0F0), // files 0-3, ranks 4-7
    (8, 0xF0F0_F0F0_0000_0000), // files 4-7, ranks 4-7
];

/// Squares lying in quadrants with an odd number of empties.
#[inline]
fn odd_quadrant_mask(parity: u8) -> u64 {
    let mut m = 0u64;
    for (id, mask) in QUADRANT_MASKS {
        if parity & id != 0 {
            m |= mask;
        }
    }
    m
}

#[inline]
fn parity_of(board: &Board) -> u8 {
    let mut parity = 0u8;
    let mut e = board.empty();
    while e != 0 {
        let sq = e.trailing_zeros() as u8;
        e &= e - 1;
        parity ^= quadrant_id(sq);
    }
    parity
}

// ---------------------------------------------------------------------------
// Transposition table
// ---------------------------------------------------------------------------

const VALUE_INF: i32 = i32::MAX / 2;

/// Compact 32-byte entry: two fit in one cache line, so a 2-way bucket
/// costs a single memory access. `(black, white, player)` fully identifies
/// the position — no separate hash key is needed (the hash only picks the
/// bucket). Scores are stored as i8 with MIN/MAX as -/+infinity sentinels.
#[derive(Clone, Copy)]
struct HashEntry {
    black: u64,
    white: u64,
    lower8: i8,
    upper8: i8,
    depth: u8,
    /// Square index of the best move; 255 = none.
    best8: u8,
    /// Bit 0: used. Bit 1: player is White.
    flags: u8,
    _pad: [u8; 3],
}

impl HashEntry {
    const EMPTY: HashEntry = HashEntry {
        black: 0,
        white: 0,
        lower8: i8::MIN,
        upper8: i8::MAX,
        depth: 0,
        best8: 255,
        flags: 0,
        _pad: [0; 3],
    };

    #[inline]
    fn used(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Seed entries come from the evaluation-guided pre-search: their move
    /// is a good ordering hint but their bounds are heuristic and must
    /// never cut the exact search.
    #[inline]
    fn is_seed(&self) -> bool {
        self.flags & 4 != 0
    }

    #[inline]
    fn lower(&self) -> i32 {
        if self.lower8 == i8::MIN { -VALUE_INF } else { self.lower8 as i32 }
    }

    #[inline]
    fn upper(&self) -> i32 {
        if self.upper8 == i8::MAX { VALUE_INF } else { self.upper8 as i32 }
    }

    #[inline]
    fn best(&self) -> Option<Position> {
        (self.best8 < 64).then_some(Position(self.best8))
    }

    #[inline]
    fn matches(&self, board: &Board) -> bool {
        self.used()
            && self.black == board.black
            && self.white == board.white
            && (self.flags >> 1) & 1 == board.player as u8
    }
}

struct HashTable {
    mask: u64,
    entries: Vec<HashEntry>,
}

impl HashTable {
    /// 2-way associative: same total entry count, half as many buckets.
    /// A deep entry no longer gets evicted just because a shallow position
    /// happens to share its bucket.
    fn new(bit_size: u32) -> HashTable {
        let size = 1usize << bit_size;
        HashTable {
            mask: ((size >> 1) - 1) as u64,
            entries: vec![HashEntry::EMPTY; size],
        }
    }

    fn clear(&mut self) {
        self.entries.fill(HashEntry::EMPTY);
    }

    #[inline]
    fn get(&self, board: &Board, hash: u64) -> Option<&HashEntry> {
        let base = ((hash & self.mask) as usize) << 1;
        let pair = &self.entries[base..base + 2];
        if pair[0].matches(board) {
            return Some(&pair[0]);
        }
        if pair[1].matches(board) {
            return Some(&pair[1]);
        }
        None
    }

    fn update(
        &mut self,
        board: &Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        value: i32,
        best: Option<Position>,
    ) {
        let best8 = best.map_or(255, |p| p.index());
        let base = ((hash & self.mask) as usize) << 1;
        let slot = if self.entries[base].matches(board) {
            base
        } else if self.entries[base + 1].matches(board) {
            base + 1
        } else {
            // Evict the shallower slot of the pair
            let victim = if self.entries[base].depth <= self.entries[base + 1].depth {
                base
            } else {
                base + 1
            };
            let entry = &mut self.entries[victim];
            if !entry.used() || entry.depth <= board.empty_count() {
                *entry = HashEntry {
                    black: board.black,
                    white: board.white,
                    lower8: if value > alpha { value as i8 } else { i8::MIN },
                    upper8: if value < beta { value as i8 } else { i8::MAX },
                    depth: board.empty_count(),
                    best8,
                    flags: 1 | ((board.player as u8) << 1),
                    _pad: [0; 3],
                };
            }
            return;
        };
        let entry = &mut self.entries[slot];
        if entry.is_seed() {
            // Heuristic bounds from the pre-search: replace, don't tighten
            entry.lower8 = if value > alpha { value as i8 } else { i8::MIN };
            entry.upper8 = if value < beta { value as i8 } else { i8::MAX };
            entry.depth = board.empty_count();
            entry.flags = 1 | ((board.player as u8) << 1);
            entry.best8 = best8;
            return;
        }
        if value < beta && (value as i8) < entry.upper8 {
            entry.upper8 = value as i8;
        }
        if value > alpha && (value as i8) > entry.lower8 {
            entry.lower8 = value as i8;
        }
        entry.best8 = best8;
    }

    /// Store a seed entry (evaluation-guided pre-search): best move plus
    /// heuristic bounds, flagged so the exact search only uses the move.
    /// Mark every live entry as a seed: its best move stays usable for
    /// ordering while its bounds stop being treated as proven.
    fn demote_to_seed(&mut self) {
        for e in self.entries.iter_mut() {
            if e.used() {
                e.flags |= 4;
            }
        }
    }

    fn seed_update(
        &mut self,
        board: &Board,
        hash: u64,
        depth: u8,
        lower8: i8,
        upper8: i8,
        best: Option<Position>,
    ) {
        let base = ((hash & self.mask) as usize) << 1;
        let slot = if self.entries[base].matches(board) {
            base
        } else if self.entries[base + 1].matches(board) {
            base + 1
        } else if !self.entries[base].used() {
            base
        } else if !self.entries[base + 1].used() {
            base + 1
        } else if self.entries[base].is_seed() && self.entries[base].depth <= depth {
            base
        } else if self.entries[base + 1].is_seed() && self.entries[base + 1].depth <= depth {
            base + 1
        } else {
            return; // never evict a real entry for a seed
        };
        let entry = &mut self.entries[slot];
        if entry.used() && !entry.is_seed() {
            // Same position already solved exactly: keep it
            return;
        }
        *entry = HashEntry {
            black: board.black,
            white: board.white,
            lower8,
            upper8,
            depth,
            best8: best.map_or(255, |p| p.index()),
            flags: 1 | ((board.player as u8) << 1) | 4,
            _pad: [0; 3],
        };
    }
}

// ---------------------------------------------------------------------------
// Solver
// ---------------------------------------------------------------------------

/// Search objective for [`Solver::solve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndSolverMode {
    /// Win/Loss/Draw: only the sign of the result is exact.
    WinLossDraw,
    /// Win vs not-win.
    WinDraw,
    /// Draw vs loss.
    DrawLoss,
    /// Exact final score.
    Perfect,
}

/// Result of an endgame search.
#[derive(Debug, Clone)]
pub struct EndSolverResult {
    /// Best move (None = current player must pass).
    pub best_move: Option<Position>,
    /// Score (disc difference with empty bonus) from the current player's view.
    pub value: i32,
    /// Empty squares at the root.
    pub empty: u8,
    /// Nodes visited.
    pub nodes: u64,
}

pub struct Solver {
    nodes: u64,
    /// Score returned by the warm-up pass, used to centre the exact
    /// search's aspiration window (carrying the score between passes —
    /// that, not the warmed table, is the main benefit).
    warm_score: Option<i32>,
    /// When set, the search is *selective*: probable cutoffs are taken at
    /// this confidence (standard deviations of the evaluation's prediction
    /// error). Only the warm-up passes run this way; their table entries
    /// are demoted so the exact pass trusts their moves but not their bounds.
    selective_t: Option<f32>,
    best: Option<Position>,
    hash_table: HashTable,
    /// Small dedicated table for the shallow region (5-6 empties), where
    /// transpositions are dense but entries would lose the depth-preferred
    /// replacement race in the main table.
    shallow_table: HashTable,
    neighbours: NeighbourTable,
}

impl Solver {
    /// `bit_size`: transposition table has 2^bit_size entries
    /// (each entry is ~48 bytes; 20 -> ~50 MB).
    pub fn new(bit_size: u32) -> Solver {
        Solver {
            nodes: 0,
            best: None,
            warm_score: None,
            selective_t: None,
            hash_table: HashTable::new(bit_size),
            shallow_table: HashTable::new(16),
            neighbours: NeighbourTable::new(),
        }
    }

    /// Solve the endgame for `board` under the given mode.
    pub fn solve(&mut self, mode: EndSolverMode, board: &Board) -> EndSolverResult {
        self.solve_with_eval(mode, board, None)
    }

    /// Evaluation-guided iterative pre-search: before the
    /// exact passes, run shallow alpha-beta over the evaluator at
    /// increasing depths, storing best moves as flagged seed entries.
    /// The exact search then starts with near-complete move ordering.
    #[allow(clippy::too_many_arguments)]
    fn seed_search(
        &mut self,
        board: &Board,
        hash: u64,
        ev: &Evaluator,
        ix: &PatternIndexer,
        indices: &mut PatternIndices,
        depth: u8,
        alpha: f32,
        beta: f32,
        store: bool,
    ) -> f32 {
        self.nodes += 1;

        if let Some(v) = wipeout_score(board) {
            return v as f32;
        }
        if depth == 0 {
            return ev.eval_indices(board, indices);
        }

        let mut tt_move: Option<Position> = None;
        if let Some(e) = self.hash_table.get(board, hash) {
            tt_move = e.best();
            // Exactly solved already: the true value beats any estimate
            if !e.is_seed() && e.lower() >= e.upper() {
                return e.lower() as f32;
            }
        }

        let moves = board.movable();
        if moves == 0 {
            let mut child = *board;
            child.pass();
            if child.movable() == 0 {
                return final_score(board) as f32;
            }
            return -self.seed_search(
                &child, zobrist::update_hash_on_pass(hash), ev, ix, indices, depth,
                -beta, -alpha, store,
            );
        }

        // Order children: TT/seed move first, then 0-ply evaluation
        let mover = board.player();
        let mut children = [(Position(0), *board, 0u64, 0u64, 0i32); 34];
        let mut n = 0usize;
        let mut m = moves;
        while m != 0 {
            let sq = m.trailing_zeros() as u8;
            m &= m - 1;
            let pos = Position(sq);
            let mut child = *board;
            let flipped = child.make_move_bits(pos);
            let child_hash = zobrist::update_hash_on_move(hash, pos, flipped, mover);
            let key = if child.player_bb() == 0 || Some(pos) == tt_move {
                i32::MIN
            } else if depth >= 2 {
                ix.apply(indices, pos, flipped, mover);
                let v = (ev.eval_indices(&child, indices) * 8.0) as i32;
                ix.undo(indices, pos, flipped, mover);
                v
            } else {
                0
            };
            children[n] = (pos, child, child_hash, flipped, key);
            n += 1;
        }
        let children = &mut children[..n];
        if depth >= 2 || tt_move.is_some() {
            children.sort_unstable_by_key(|c| c.4);
        }

        let mut alpha = alpha;
        let mut best_val = f32::NEG_INFINITY;
        let mut best_move = None;
        let orig_alpha = alpha;
        for (pos, child, child_hash, flipped, _) in children.iter() {
            ix.apply(indices, *pos, *flipped, mover);
            let v = -self.seed_search(
                child, *child_hash, ev, ix, indices, depth - 1, -beta, -alpha, store,
            );
            ix.undo(indices, *pos, *flipped, mover);
            if v > best_val {
                best_val = v;
                best_move = Some(*pos);
                if v > alpha {
                    alpha = v;
                }
            }
            if alpha >= beta {
                break;
            }
        }

        if store {
            let v8 = best_val.round().clamp(-64.0, 64.0) as i8;
            let lower8 = if best_val > orig_alpha { v8 } else { i8::MIN };
            let upper8 = if best_val < beta { v8 } else { i8::MAX };
            self.hash_table.seed_update(board, hash, depth, lower8, upper8, best_move);
        }
        best_val
    }

    /// Solve with an optional evaluator used purely for move ordering in
    /// the upper (many-empties) region of the tree. Ordering never changes
    /// the exact result — only the node count.
    pub fn solve_with_eval(
        &mut self,
        mode: EndSolverMode,
        board: &Board,
        ev: Option<&Evaluator>,
    ) -> EndSolverResult {
        self.nodes = 0;
        self.best = None;

        if !board.check_all() {
            return EndSolverResult {
                best_move: None,
                value: 0,
                empty: board.empty_count(),
                nodes: 0,
            };
        }

        self.hash_table.clear();
        self.shallow_table.clear();
        self.warm_score = None;
        let mut b = *board;

        // Warm-up ladder (iterative selectivity): solve the same
        // full-depth endgame selectively first, leaving real full-depth
        // best moves in the table for the exact pass that follows.
        if let Some(e) = ev {
            if board.empty_count() >= SELECTIVE_PASS_MIN_EMPTIES {
                let mut guess = self.estimate_score(board, Some(e));
                for t in SELECTIVE_LADDER {
                    self.selective_t = Some(t);
                    let mut sb = *board;
                    let s = self.aspiration_width(&mut sb, guess, WARM_ASPIRATION_WIDTH, Some(e));
                    self.selective_t = None;
                    guess = s - (s & 1);
                    // Each rung must re-derive its own bounds: an entry
                    // stored by a more aggressive (less reliable) pass would
                    // otherwise be taken at face value, and the ladder would
                    // just re-confirm the first pass's error instead of
                    // converging. Best moves survive the demotion.
                    self.hash_table.demote_to_seed();
                    self.warm_score = Some(s - (s & 1));
                    if std::env::var("BBR_DIAG").is_ok() {
                        eprintln!("  warm(t={t}) -> {s} (nodes {})", self.nodes);
                    }
                }
                self.hash_table.demote_to_seed();
            }
        }

        let value = match mode {
            EndSolverMode::WinLossDraw => self.pvs_root(&mut b, -1, 1, ev),
            EndSolverMode::WinDraw => self.pvs_root(&mut b, 0, 1, ev),
            EndSolverMode::DrawLoss => self.pvs_root(&mut b, -1, 0, ev),
            EndSolverMode::Perfect => self.perfect(&mut b, ev),
        };

        EndSolverResult {
            best_move: self.best,
            value,
            empty: board.empty_count(),
            nodes: self.nodes,
        }
    }

    /// Exact score. With a warm-up score available the window is a narrow
    /// aspiration around it, widened only
    /// on the side that failed. Without one, fall back to the win/loss
    /// probe and a +-8 band.
    fn perfect(&mut self, board: &mut Board, ev: Option<&Evaluator>) -> i32 {
        if let Some(score) = self.warm_score {
            return self.aspiration(board, score, ev);
        }

        let mut val = self.pvs_root(board, -1, 1, ev);
        if val > 0 {
            let bound = val + 8;
            val = self.pvs_root(board, val, bound, ev);
            if val >= bound {
                val = self.pvs_root(board, val, 64, ev);
            }
        } else if val < 0 {
            let bound = val - 8;
            val = self.pvs_root(board, bound, val, ev);
            if val <= bound {
                val = self.pvs_root(board, -64, val, ev);
            }
        }
        val
    }

    /// Cheap evaluation-based guess of the final score, on the even grid
    /// that terminal scores live on.
    fn estimate_score(&mut self, board: &Board, ev: Option<&Evaluator>) -> i32 {
        let Some(e) = ev else { return 0 };
        let ix = e.indexer();
        let mut indices = ix.init(board.black, board.white);
        let hash = zobrist::compute_hash(board.black, board.white, board.player());
        let depth = ESTIMATE_DEPTH.min(board.empty_count());
        let v = self.seed_search(
            board, hash, e, ix, &mut indices, depth, f32::NEG_INFINITY, f32::INFINITY, false,
        );
        let rounded = (v / 2.0).round() as i32 * 2;
        rounded.clamp(-62, 62)
    }

    /// Aspiration around a prior score: search a narrow window, and on a
    /// failure re-centre on the failing bound and double that side only.
    fn aspiration(&mut self, board: &mut Board, score: i32, ev: Option<&Evaluator>) -> i32 {
        self.aspiration_width(board, score, ASPIRATION_WIDTH, ev)
    }

    /// Aspiration with an explicit starting half-width: the warm-up passes
    /// start from a rougher estimate than the exact pass does.
    fn aspiration_width(
        &mut self,
        board: &mut Board,
        mut score: i32,
        width: i32,
        ev: Option<&Evaluator>,
    ) -> i32 {
        let mut left = width;
        let mut right = width;
        for _ in 0..12 {
            let lo = (score - left).max(-64);
            let hi = (score + right).min(64);
            if lo >= hi || (lo <= -64 && hi >= 64) {
                break;
            }
            let val = self.pvs_root(board, lo, hi, ev);
            if val <= lo && lo > -64 {
                score = val;
                left = (left * 2).min(128);
                right = 0;
            } else if val >= hi && hi < 64 {
                score = val;
                left = 0;
                right = (right * 2).min(128);
            } else {
                return val;
            }
        }
        self.pvs_root(board, -64, 64, ev)
    }

    fn pvs_root(&mut self, board: &mut Board, alpha: i32, beta: i32, ev: Option<&Evaluator>) -> i32 {
        let mut lower = alpha;
        let upper = beta;
        // Root computes the hash from scratch once; children update it
        // incrementally (flipped discs + placed disc + player swap).
        let hash = zobrist::compute_hash(board.black, board.white, board.player());

        self.nodes += 1;

        let mut moves = MoveBuf::new();
        self.sorted_moves(board, hash, ev, &mut moves);
        if moves.is_empty() {
            return final_score(board);
        }

        let mut best = moves[0].pos;
        let mut max;

        // First move: full window
        {
            let mut child = moves[0].board;
            max = -self.pvs(&mut child, moves[0].hash, -upper, -lower, false, ev);
            if max > lower {
                lower = max;
            }
        }

        // Remaining moves: null-window probe, re-search on fail-high
        for m in &moves[1..] {
            if lower >= upper {
                break;
            }
            let mut child = m.board;
            let mut val = -self.pvs(&mut child, m.hash, -lower - 1, -lower, false, ev);
            if lower < val && val < upper {
                val = -self.pvs(&mut child, m.hash, -upper, -val, false, ev);
            }
            if val > max {
                max = val;
                best = m.pos;
                if max > lower {
                    lower = max;
                }
            }
        }

        self.hash_table
            .update(board, hash, alpha, beta, max, Some(best));
        self.best = Some(best);
        max
    }

    #[allow(clippy::too_many_arguments)]
    fn pvs(&mut self, board: &mut Board, hash: u64, alpha: i32, beta: i32, passed: bool, ev: Option<&Evaluator>) -> i32 {
        let mut lower = alpha;
        let mut upper = beta;

        self.nodes += 1;

        if let Some(v) = wipeout_score(board) {
            return v;
        }

        if let Some(v) = self.hash_table.get(board, hash) {
            if !v.is_seed() {
                if v.lower() >= v.upper() {
                    return v.lower();
                }
                if upper > v.upper() {
                    upper = v.upper();
                    if upper <= lower {
                        return upper;
                    }
                }
                if lower < v.lower() {
                    lower = v.lower();
                    if lower >= upper {
                        return lower;
                    }
                }
            }
        }

        // Stability cutoff: the opponent's stable discs can never be
        // flipped, so our final score is at most 64 - 2*|opp stable|.
        if let Some(bound) = stability_cut(board, lower, upper) {
            return bound;
        }

        // Warm-up (selective) pass: take probable cutoffs from a shallow
        // evaluation search instead of proving them.
        if let (Some(t), Some(e)) = (self.selective_t, ev) {
            if board.empty_count() >= SELECTIVE_MIN_EMPTIES && upper - lower <= 1 {
                if let Some(v) = self.selective_cut(board, hash, e, t, lower, upper) {
                    return v;
                }
            }
        }

        // Children are generated without their (expensive) ordering values:
        // a node whose transposition-table move already cuts must not pay
        // for pattern evaluations and lookaheads it never uses.
        let mut moves = MoveBuf::new();
        self.gen_moves(board, hash, &mut moves);

        if moves.is_empty() {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.pvs(board, zobrist::update_hash_on_pass(hash), -upper, -lower, true, ev);
            board.pass();
            self.hash_table.update(board, hash, alpha, beta, val, None);
            return val;
        }

        // A move that wipes out the opponent ends the game at exactly +64,
        // which is the maximum possible score — so it *is* this node's value.
        if moves.iter().any(|m| m.board.player_bb() == 0) {
            self.hash_table.update(board, hash, alpha, beta, 64, None);
            return 64;
        }

        // Enhanced transposition cutoff: a child whose stored upper bound
        // already proves our value >= upper ends this node for free.
        if board.empty_count() >= ETC_EMPTIES {
            for m in moves.iter() {
                if let Some(e) = self.hash_table.get(&m.board, m.hash) {
                    if !e.is_seed() && -e.upper() >= upper {
                        return -e.upper();
                    }
                }
            }
        }

        let mut max = i32::MIN;
        let mut best = None;

        // Stage 1: the transposition-table move, searched before any
        // ordering work is done. Most cut nodes end here.
        let tt_best = self.hash_table.get(board, hash).and_then(|e| e.best());
        if let Some(bpos) = tt_best {
            if let Some(idx) = moves.iter().position(|m| m.pos == bpos) {
                let m = moves.swap_remove(idx);
                let mut child = m.board;
                max = self.descend(&mut child, m.hash, -upper, -lower, ev);
                best = Some(m.pos);
                if max > lower {
                    lower = max;
                }
                if lower >= upper {
                    self.hash_table.update(board, hash, alpha, beta, max, best);
                    return max;
                }
            }
        }

        // Stage 2: the rest, now ordered properly.
        self.score_and_sort(board, &mut moves, None, ev, lower);

        let mut rest = moves.iter();
        if max == i32::MIN {
            // No table move: the best-ordered child takes the full window.
            let m = rest.next().expect("non-empty move list");
            let mut child = m.board;
            max = self.descend(&mut child, m.hash, -upper, -lower, ev);
            best = Some(m.pos);
            if max > lower {
                lower = max;
            }
        }

        // Remaining moves: null-window probe, re-search on fail-high
        for m in rest {
            if lower >= upper {
                break;
            }
            let mut child = m.board;
            let val = self.descend_null_window(&mut child, m.hash, lower, upper, ev);
            if val > max {
                max = val;
                best = Some(m.pos);
                if max > lower {
                    lower = max;
                }
            }
        }

        self.hash_table.update(board, hash, alpha, beta, max, best);
        max
    }

    /// Full-window recursive descent picking the right strategy by depth.
    #[inline]
    fn descend(&mut self, child: &mut Board, hash: u64, alpha: i32, beta: i32, ev: Option<&Evaluator>) -> i32 {
        if child.empty_count() >= PVS_LIMIT {
            -self.pvs(child, hash, alpha, beta, false, ev)
        } else if child.empty_count() >= MOVE_ORDERING_LIMIT {
            -self.alpha_beta_ordered(child, hash, alpha, beta, false, ev)
        } else {
            -self.alpha_beta(child, hash, alpha, beta, false)
        }
    }

    /// Null-window probe then re-search, at the strategy for this depth.
    #[inline]
    fn descend_null_window(&mut self, child: &mut Board, hash: u64, lower: i32, upper: i32, ev: Option<&Evaluator>) -> i32 {
        let mut val = self.descend(child, hash, -lower - 1, -lower, ev);
        if lower < val && val < upper {
            val = self.descend(child, hash, -upper, -val, ev);
        }
        val
    }

    #[allow(clippy::too_many_arguments)]
    fn alpha_beta_ordered(
        &mut self,
        board: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        ev: Option<&Evaluator>,
    ) -> i32 {
        let mut alpha = alpha;
        let mut beta = beta;

        self.nodes += 1;

        if let Some(v) = wipeout_score(board) {
            return v;
        }

        if let Some(v) = self.hash_table.get(board, hash) {
            if !v.is_seed() {
                if v.lower() >= v.upper() {
                    return v.lower();
                }
                if beta > v.upper() {
                    beta = v.upper();
                    if beta <= alpha {
                        return beta;
                    }
                }
                if alpha < v.lower() {
                    alpha = v.lower();
                    if alpha >= beta {
                        return alpha;
                    }
                }
            }
        }

        if let Some(bound) = stability_cut(board, alpha, beta) {
            return bound;
        }

        let orig_alpha = alpha;
        let mut moves = MoveBuf::new();
        self.sorted_moves(board, hash, ev, &mut moves);

        if moves.is_empty() {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.alpha_beta_ordered(
                board,
                zobrist::update_hash_on_pass(hash),
                -beta,
                -alpha,
                true,
                ev,
            );
            board.pass();
            self.hash_table.update(board, hash, orig_alpha, beta, val, None);
            return val;
        }

        let mut best = None;
        for m in moves.iter() {
            let mut child = m.board;
            let val = if child.empty_count() >= MOVE_ORDERING_LIMIT {
                -self.alpha_beta_ordered(&mut child, m.hash, -beta, -alpha, false, ev)
            } else {
                -self.alpha_beta(&mut child, m.hash, -beta, -alpha, false)
            };
            if val > alpha {
                alpha = val;
            }
            if alpha >= beta {
                best = Some(m.pos);
                break;
            }
        }

        self.hash_table.update(board, hash, orig_alpha, beta, alpha, best);
        alpha
    }

    /// Plain alpha-beta over the empty list, with the last-4 fast path and
    /// a dedicated shallow transposition table (5-6 empties).
    fn alpha_beta(&mut self, board: &mut Board, hash: u64, alpha: i32, beta: i32, passed: bool) -> i32 {
        self.nodes += 1;

        if let Some(v) = wipeout_score(board) {
            return v;
        }

        if let Some(bound) = stability_cut(board, alpha, beta) {
            return bound;
        }

        if board.empty_count() == 4 {
            let mut e = board.empty();
            let p1 = e.trailing_zeros() as u8;
            e &= e - 1;
            let p2 = e.trailing_zeros() as u8;
            e &= e - 1;
            let p3 = e.trailing_zeros() as u8;
            e &= e - 1;
            let p4 = e.trailing_zeros() as u8;
            return self.last4(board, p1, p2, p3, p4, alpha, beta, passed);
        }

        let mut lower = alpha;
        let mut upper = beta;
        if let Some(v) = self.shallow_table.get(board, hash) {
            if v.lower() >= v.upper() {
                return v.lower();
            }
            if upper > v.upper() {
                upper = v.upper();
                if upper <= lower {
                    return upper;
                }
            }
            if lower < v.lower() {
                lower = v.lower();
                if lower >= upper {
                    return lower;
                }
            }
        }

        let orig_lower = lower;
        let mut best = lower;
        let mut any = false;
        let mut cut = false;
        let opponent_bb = board.opponent_bb();
        // Children of a 5-empty node dispatch to last4 and never probe, so
        // their hashes are only needed one level up.
        let need_child_hash = board.empty_count() > 5;
        let mover = board.player();

        // Odd-quadrant moves first (quadrant parity: filling the last
        // empty of a region tends to keep the tempo).
        let empties = board.empty();
        let odd = odd_quadrant_mask(parity_of(board));
        'passes: for pass_mask in [empties & odd, empties & !odd] {
            let mut e = pass_mask;
            while e != 0 {
                let sq = e.trailing_zeros() as u8;
                e &= e - 1;

                if self.neighbours.get(sq) & opponent_bb == 0 {
                    continue;
                }
                let pos = Position(sq);
                let pos_bit = pos.to_bit();
                let flips = bitboard::flippable(board.player_bb(), board.opponent_bb(), pos_bit);
                if flips == 0 {
                    continue;
                }

                any = true;
                let mut child = *board;
                child.apply_flips(pos, flips);
                let child_hash = if need_child_hash {
                    zobrist::update_hash_on_move(hash, pos, flips, mover)
                } else {
                    0
                };
                let val =
                    -self.alpha_beta(&mut child, child_hash, -upper, -best.max(orig_lower), false);

                if val > best {
                    best = val;
                }
                if best >= upper {
                    cut = true;
                    break 'passes;
                }
            }
        }
        let _ = cut;

        if !any {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.alpha_beta(board, zobrist::update_hash_on_pass(hash), -upper, -orig_lower, true);
            board.pass();
            return val;
        }

        self.shallow_table.update(board, hash, orig_lower, upper, best, None);
        best
    }

    /// Specialized 4-empties search with quadrant-parity move ordering.
    #[allow(clippy::too_many_arguments)]
    fn last4(
        &mut self,
        board: &mut Board,
        p1: u8,
        p2: u8,
        p3: u8,
        p4: u8,
        alpha: i32,
        beta: i32,
        passed: bool,
    ) -> i32 {
        self.nodes += 1;

        // Parity ordering: squares in odd quadrants first (odd sorts before
        // even because !odd is false < true)
        let parity = parity_of(board);
        let odd = |sq: u8| parity & quadrant_id(sq) != 0;
        let mut arr = [p1, p2, p3, p4];
        arr.sort_by_key(|&s| !odd(s));
        let [p1, p2, p3, p4] = arr;

        let mut alpha = alpha;
        let mut any = false;

        for (a, rest) in [
            (p1, [p2, p3, p4]),
            (p2, [p1, p3, p4]),
            (p3, [p1, p2, p4]),
            (p4, [p1, p2, p3]),
        ] {
            if self.neighbours.get(a) & board.opponent_bb() == 0 {
                continue;
            }
            let pos = Position(a);
            let flips = bitboard::flippable(board.player_bb(), board.opponent_bb(), pos.to_bit());
            if flips == 0 {
                continue;
            }
            any = true;
            let mut child = *board;
            child.apply_flips(pos, flips);
            let val = -self.last3(&mut child, rest[0], rest[1], rest[2], -beta, -alpha, false);
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }

        if !any {
            if passed {
                // Terminal: score with empty bonus
                return final_score(board);
            }
            board.pass();
            let val = -self.last4(board, p1, p2, p3, p4, -beta, -alpha, true);
            board.pass();
            return val;
        }

        alpha
    }

    #[allow(clippy::too_many_arguments)]
    fn last3(
        &mut self,
        board: &mut Board,
        p1: u8,
        p2: u8,
        p3: u8,
        alpha: i32,
        beta: i32,
        passed: bool,
    ) -> i32 {
        self.nodes += 1;

        let parity = parity_of(board);
        let odd = |sq: u8| parity & quadrant_id(sq) != 0;
        let mut arr = [p1, p2, p3];
        arr.sort_by_key(|&s| !odd(s));
        let [p1, p2, p3] = arr;

        let mut alpha = alpha;
        let mut any = false;

        for (a, rest) in [(p1, [p2, p3]), (p2, [p1, p3]), (p3, [p1, p2])] {
            if self.neighbours.get(a) & board.opponent_bb() == 0 {
                continue;
            }
            let pos = Position(a);
            let flips = bitboard::flippable(board.player_bb(), board.opponent_bb(), pos.to_bit());
            if flips == 0 {
                continue;
            }
            any = true;
            let mut child = *board;
            child.apply_flips(pos, flips);
            let val = -self.last2(&mut child, rest[0], rest[1], -beta, -alpha, false);
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }

        if !any {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.last3(board, p1, p2, p3, -beta, -alpha, true);
            board.pass();
            return val;
        }

        alpha
    }

    fn last2(
        &mut self,
        board: &mut Board,
        p1: u8,
        p2: u8,
        alpha: i32,
        beta: i32,
        passed: bool,
    ) -> i32 {
        self.nodes += 1;

        let mut alpha = alpha;
        let mut any = false;

        for (a, b) in [(p1, p2), (p2, p1)] {
            if self.neighbours.get(a) & board.opponent_bb() == 0 {
                continue;
            }
            let pos = Position(a);
            let flips = bitboard::flippable(board.player_bb(), board.opponent_bb(), pos.to_bit());
            if flips == 0 {
                continue;
            }
            any = true;
            let mut child = *board;
            child.apply_flips(pos, flips);
            let val = -self.last1(&mut child, b);
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }

        if !any {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.last2(board, p1, p2, -beta, -alpha, true);
            board.pass();
            return val;
        }

        alpha
    }

    /// Exactly one empty square left: resolve without recursion.
    fn last1(&mut self, board: &mut Board, p1: u8) -> i32 {
        let pos_bit = 1u64 << p1;
        let player_bb = board.player_bb();
        let opponent_bb = board.opponent_bb();
        let diff = player_bb.count_ones() as i32 - opponent_bb.count_ones() as i32;

        self.nodes += 1;
        // Current player fills the last square
        if self.neighbours.get(p1) & opponent_bb != 0 {
            let n = bitboard::flippable(player_bb, opponent_bb, pos_bit).count_ones() as i32;
            if n > 0 {
                return diff + 2 * n + 1;
            }
        }

        self.nodes += 1;
        // Current player passes; opponent fills the last square
        if self.neighbours.get(p1) & player_bb != 0 {
            let n = bitboard::flippable(opponent_bb, player_bb, pos_bit).count_ones() as i32;
            if n > 0 {
                return diff - 2 * n - 1;
            }
        }

        // Nobody can play the last square: empty goes to the winner
        if diff > 0 {
            diff + 1
        } else if diff < 0 {
            diff - 1
        } else {
            0
        }
    }

    /// Probable cutoff for a warm-up pass: a shallow evaluation search that
    /// clears the window by a confidence margin stands in for the exact
    /// result. Never reached by the final (exact) pass.
    fn selective_cut(
        &mut self,
        board: &Board,
        hash: u64,
        ev: &Evaluator,
        t: f32,
        lower: i32,
        upper: i32,
    ) -> Option<i32> {
        let empties = board.empty_count();
        let error = t * crate::search::mpc_sigma(empties as u32, empties, SELECTIVE_PROBE_DEPTH);
        let ix = ev.indexer();
        let mut indices = ix.init(board.black, board.white);

        let hi = upper as f32 + error;
        if hi < 64.0 {
            let v = self.seed_search(
                board, hash, ev, ix, &mut indices, SELECTIVE_PROBE_DEPTH, hi - 1.0, hi, false,
            );
            if v >= hi {
                return Some(upper);
            }
        }
        let lo = lower as f32 - error;
        if lo > -64.0 {
            let v = self.seed_search(
                board, hash, ev, ix, &mut indices, SELECTIVE_PROBE_DEPTH, lo, lo + 1.0, false,
            );
            if v <= lo {
                return Some(lower);
            }
        }
        None
    }

    /// Generate children sorted by an endgame move-ordering heuristic
    /// (fastest-first: minimize opponent mobility, corner stability, parity).
    /// The transposition table's best move, when present, is searched first.
    /// Each child carries its incrementally-updated Zobrist hash.
    fn sorted_moves(&self, board: &Board, hash: u64, ev: Option<&Evaluator>, out: &mut MoveBuf) {
        let tt_best = self.hash_table.get(board, hash).and_then(|e| e.best());
        self.gen_moves(board, hash, out);
        self.score_and_sort(board, out, tt_best, ev, i32::MIN / 2);
    }

    /// Generate children with their incremental hashes but WITHOUT the
    /// expensive ordering evaluation. Wipeout moves are flagged: they end
    /// the game at +64, so a node that has one needs no search at all.
    fn gen_moves(&self, board: &Board, hash: u64, out: &mut MoveBuf) {
        out.len = 0;
        let mover = board.player();
        let mut m = board.movable();
        while m != 0 {
            let sq = m.trailing_zeros() as u8;
            m &= m - 1;
            let pos = Position(sq);
            let mut child = *board;
            let flipped = child.make_move_bits(pos);
            let child_hash = zobrist::update_hash_on_move(hash, pos, flipped, mover);
            out.push(ScoredMove { pos, board: child, hash: child_hash, value: 0 });
        }
    }

    /// Fill in ordering values (the expensive part: pattern evaluation and
    /// pruned lookaheads) and sort. Split from generation so that a node
    /// whose transposition-table move already cuts never pays for it.
    fn score_and_sort(
        &self,
        board: &Board,
        moves: &mut [ScoredMove],
        tt_best: Option<Position>,
        ev: Option<&Evaluator>,
        alpha: i32,
    ) {
        let eval_order = board.empty_count() >= EVAL_ORDER_EMPTIES;
        // Incremental pattern indices for ordering evaluation: initialized
        // once per node, then updated per candidate move — far cheaper than
        // recomputing every pattern from scratch inside the lookahead.
        let mut order_ix = if eval_order {
            ev.map(|e| (e.indexer(), e.indexer().init(board.black, board.white)))
        } else {
            None
        };
        let parity = parity_of(board);
        let mover = board.player();
        // The ordering lookahead only has to distinguish moves that could
        // matter at this node, so it is bounded from below by the node's own
        // alpha (less a margin); a full window wastes work
        // proving exact values for moves that are already hopeless.
        // The lookahead runs from the child's point of view, so the node's
        // alpha becomes an upper bound there.
        let sort_hi = if alpha <= i32::MIN / 4 {
            f32::INFINITY
        } else {
            -(alpha - SORT_ALPHA_DELTA) as f32
        };

        for sm in moves.iter_mut() {
            let pos = sm.pos;
            let child = sm.board;
            let flipped = (board.player_bb() ^ child.opponent_bb()) & !pos.to_bit();
            sm.value = if child.player_bb() == 0 {
                // Wiping out the opponent ends the game at +64: try first
                i32::MIN
            } else if Some(pos) == tt_best {
                i32::MIN + 1
            } else if let (Some(e), Some((ix, indices))) = (ev, order_ix.as_mut()) {
                // Pattern evaluation of the child (opponent view: lower =
                // better for the mover). Far stronger ordering than the
                // static heuristic in the many-empties region; the topmost
                // region refines it with a pruned lookahead.
                ix.apply(indices, pos, flipped, mover);
                let v = if board.empty_count() >= DEEP2_ORDER_EMPTIES {
                    shallow_search(&child, e, ix, indices, 4, f32::NEG_INFINITY, sort_hi)
                } else if board.empty_count() >= DEEP_ORDER_EMPTIES {
                    shallow_search(&child, e, ix, indices, 2, f32::NEG_INFINITY, sort_hi)
                } else {
                    e.eval_indices(&child, indices)
                };
                ix.undo(indices, pos, flipped, mover);
                // Weight mobility as heavily as the evaluation itself; the
                // evaluator alone is blind to how many replies it leaves the
                // opponent. One reply counts for about one disc.
                // Also credit the edge discs the move makes stable, at about
                // a sixteenth of mobility's weight. `child`'s opponent is us,
                // having just moved.
                let edge = crate::stability::edge_stable_all(
                    child.opponent_bb(),
                    child.player_bb(),
                )
                .count_ones() as i32;
                (v * 8.0) as i32 + (child.movable_count() as i32) * MOBILITY_ORDER_WEIGHT
                    - edge * EDGE_STABILITY_ORDER_WEIGHT
            } else {
                move_ordering_value(pos, &child, parity)
            };
        }

        // Ascending: "fastest-first" — smaller value = search first
        moves.sort_by_key(|m| m.value);
    }
}

#[derive(Clone, Copy)]
struct ScoredMove {
    pos: Position,
    board: Board,
    hash: u64,
    value: i32,
}

/// A position has at most 32 legal moves; 34 gives slack. Move lists live
/// in this stack buffer instead of a heap `Vec` — allocation showed up as
/// ~3% of solver time in deep-search profiles, and every node builds one.
/// The backing store is uninitialized (`MaybeUninit`) so there is no
/// per-node memset; only slots `0..len` are ever read.
const MAX_MOVES: usize = 34;

struct MoveBuf {
    buf: [std::mem::MaybeUninit<ScoredMove>; MAX_MOVES],
    len: usize,
}

impl MoveBuf {
    #[inline]
    fn new() -> MoveBuf {
        MoveBuf {
            // SAFETY: an array of MaybeUninit needs no initialization.
            buf: unsafe { std::mem::MaybeUninit::uninit().assume_init() },
            len: 0,
        }
    }

    #[inline]
    fn push(&mut self, m: ScoredMove) {
        self.buf[self.len].write(m);
        self.len += 1;
    }

    /// Remove by index, moving the last element into the hole. `ScoredMove`
    /// is `Copy`, so nothing needs dropping.
    #[inline]
    fn swap_remove(&mut self, i: usize) -> ScoredMove {
        // SAFETY: callers pass i < len, and 0..len are initialized.
        let out = unsafe { self.buf[i].assume_init() };
        self.len -= 1;
        self.buf[i] = self.buf[self.len];
        out
    }
}

impl std::ops::Deref for MoveBuf {
    type Target = [ScoredMove];
    #[inline]
    fn deref(&self) -> &[ScoredMove] {
        // SAFETY: slots 0..len were initialized by push.
        unsafe { std::slice::from_raw_parts(self.buf.as_ptr() as *const ScoredMove, self.len) }
    }
}

impl std::ops::DerefMut for MoveBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut [ScoredMove] {
        // SAFETY: as above.
        unsafe { std::slice::from_raw_parts_mut(self.buf.as_mut_ptr() as *mut ScoredMove, self.len) }
    }
}

/// Static square-visit priority for the ordering lookahead (file-major):
/// corners first, C/X squares last. Trying good squares first improves the
/// lookahead's alpha-beta cutoffs; because it only reorders sibling moves,
/// the value the lookahead returns is unchanged (and so is the resulting
/// move order and the main search's node count).
#[rustfmt::skip]
const SHALLOW_ORDER: [u8; 64] = {
    // priority weight per square, rank-major, then transposed to file-major
    let rm: [u8; 64] = [
         0,  9,  3,  5,  5,  3,  9,  0,
         9, 12,  7,  8,  8,  7, 12,  9,
         3,  7,  1,  4,  4,  1,  7,  3,
         5,  8,  4,  6,  6,  4,  8,  5,
         5,  8,  4,  6,  6,  4,  8,  5,
         3,  7,  1,  4,  4,  1,  7,  3,
         9, 12,  7,  8,  8,  7, 12,  9,
         0,  9,  3,  5,  5,  3,  9,  0,
    ];
    let mut t = [0u8; 64];
    let mut f = 0;
    while f < 8 { let mut r = 0; while r < 8 { t[f*8+r] = rm[r*8+f]; r += 1; } f += 1; }
    t
};

/// Shallow alpha-beta refinement for ordering: the position's value from
/// its own player's view, looking `depth` replies ahead with the
/// evaluator. Pruned — same root value as a full-width lookahead at a
/// fraction of the cost, which buys deeper (= better-sorted) lookaheads.
#[allow(clippy::too_many_arguments)]
fn shallow_search(
    board: &Board,
    ev: &Evaluator,
    ix: &PatternIndexer,
    indices: &mut PatternIndices,
    depth: u8,
    alpha: f32,
    beta: f32,
) -> f32 {
    if depth == 0 {
        return ev.eval_indices(board, indices);
    }
    let moves = board.movable();
    if moves == 0 {
        let mut p = *board;
        p.pass();
        if p.movable() == 0 {
            return final_score(board) as f32 * 1000.0;
        }
        // A pass leaves the discs (and thus the indices) unchanged
        return -shallow_search(&p, ev, ix, indices, depth, -beta, -alpha);
    }
    let mut alpha = alpha;
    let mut best = f32::NEG_INFINITY;
    let mover = board.player();

    // Collect legal squares and visit them corner-first. Reordering
    // siblings cannot change the value alpha-beta returns, only how quickly
    // it prunes.
    let mut sqs = [0u8; MAX_MOVES];
    let mut n = 0usize;
    let mut m = moves;
    while m != 0 {
        sqs[n] = m.trailing_zeros() as u8;
        n += 1;
        m &= m - 1;
    }
    let sqs = &mut sqs[..n];
    sqs.sort_unstable_by_key(|&sq| SHALLOW_ORDER[sq as usize]);

    for &sq in sqs.iter() {
        let pos = Position(sq);
        let mut child = *board;
        let flipped = child.make_move_bits(pos);
        ix.apply(indices, pos, flipped, mover);
        let v = -shallow_search(&child, ev, ix, indices, depth - 1, -beta, -alpha);
        ix.undo(indices, pos, flipped, mover);
        if v > best {
            best = v;
            if v > alpha {
                alpha = v;
            }
            if alpha >= beta {
                break;
            }
        }
    }
    best
}

/// Move-ordering heuristic (lower = searched earlier): after the move, the
/// board is from the opponent's perspective, so high opponent mobility is
/// bad for us.
/// 8-neighbourhood dilation (file-major layout).
#[inline]
fn dilate(x: u64) -> u64 {
    let v = x | ((x & 0x7F7F_7F7F_7F7F_7F7F) << 1) | ((x & 0xFEFE_FEFE_FEFE_FEFE) >> 1);
    v | (v << 8) | (v >> 8)
}

fn move_ordering_value(pos: Position, child: &Board, parity: u8) -> i32 {
    use crate::pattern::sq::*;

    let mut point = 0i32;

    // child.player is our opponent now
    let opp_mobility = child.movable_count() as i32;
    let our_mobility =
        bitboard::mobility_count(child.opponent_bb(), child.player_bb()) as i32;
    point += (opp_mobility - our_mobility) * 5;

    // Potential mobility: frontier discs (adjacent to an empty square) are
    // attack surface — many of ours after the move is bad for us.
    let empties_next = crate::bitboard::empty_bb(child.black, child.white);
    let frontier = dilate(empties_next);
    let our_frontier = (frontier & child.opponent_bb()).count_ones() as i32;
    let opp_frontier = (frontier & child.player_bb()).count_ones() as i32;
    point += (our_frontier - opp_frontier) * 2;

    point += (corner_stability(child, child.player())
        - corner_stability(child, child.player().opponent()))
        * 3;

    let s = pos.index();
    // X-squares (B2, G2, B7, G7): tends to give away a corner — try late? No:
    // the Go reference *adds* 10 (sorted ascending = searched first means
    // fail-fast). Keep identical behaviour.
    if s == B2 || s == G2 || s == B7 || s == G7 {
        point += 10;
    }
    // C-squares
    if s == B1 || s == G1 || s == A2 || s == H2 || s == A7 || s == H7 || s == B8 || s == G8 {
        point += 4;
    }
    if parity & quadrant_id(s) != 0 {
        point += 3;
    }
    point
}

/// Corner stability count: corners owned plus adjacent edge discs.
fn corner_stability(board: &Board, color: Color) -> i32 {
    use crate::pattern::sq::*;

    let bb = match color {
        Color::Black => board.black,
        Color::White => board.white,
    };
    let has = |s: u8| bb & (1u64 << s) != 0;

    let mut count = 0;
    for (corner, edge1, edge2) in [(A1, B1, A2), (A8, B8, A7), (H1, G1, H2), (H8, G8, H7)] {
        if has(corner) {
            count += 1;
            if has(edge1) {
                count += 1;
            }
            if has(edge2) {
                count += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    /// Brute-force negamax over full boards for cross-checking the solver.
    fn negamax(board: &Board, passed: bool) -> i32 {
        let moves = board.movable();
        if moves == 0 {
            if passed {
                return final_score(board);
            }
            let mut b = *board;
            b.pass();
            return -negamax(&b, true);
        }
        let mut best = -VALUE_INF;
        let mut m = moves;
        while m != 0 {
            let bit = m.trailing_zeros();
            m &= m - 1;
            let mut child = *board;
            child.make_move_unchecked(Position::from_index(bit).unwrap());
            let v = -negamax(&child, false);
            if v > best {
                best = v;
            }
        }
        best
    }

    /// Play a deterministic game until `empties` squares remain.
    fn position_with_empties(empties: u8) -> Board {
        let mut b = Board::new();
        let mut ply = 0usize;
        while b.empty_count() > empties {
            let moves = b.movable();
            if moves == 0 {
                let mut p = b;
                p.pass();
                if p.movable() == 0 {
                    break; // game ended early
                }
                b = p;
                continue;
            }
            let bit = if ply.is_multiple_of(2) {
                moves.trailing_zeros()
            } else {
                63 - moves.leading_zeros()
            };
            b.make_move_unchecked(Position::from_index(bit).unwrap());
            ply += 1;
        }
        b
    }

    #[test]
    fn test_solver_matches_negamax_shallow() {
        // Cross-check exact scores against brute force at several depths
        for empties in [4u8, 6, 8] {
            let board = position_with_empties(empties);
            if board.is_game_over() {
                continue;
            }
            let expected = negamax(&board, false);
            let mut solver = Solver::new(14);
            let result = solver.solve(EndSolverMode::Perfect, &board);
            assert_eq!(
                result.value, expected,
                "perfect solve mismatch at {} empties",
                board.empty_count()
            );
        }
    }

    #[test]
    fn test_solver_wld_sign_matches_perfect() {
        let board = position_with_empties(10);
        let mut solver = Solver::new(14);
        let perfect = solver.solve(EndSolverMode::Perfect, &board);
        let wld = solver.solve(EndSolverMode::WinLossDraw, &board);
        assert_eq!(
            wld.value.signum(),
            perfect.value.signum(),
            "WLD sign must match the exact result"
        );
    }

    #[test]
    fn test_solver_best_move_is_legal() {
        let board = position_with_empties(12);
        let mut solver = Solver::new(14);
        let result = solver.solve(EndSolverMode::Perfect, &board);
        let best = result.best_move.expect("a legal move exists");
        assert!(
            board.movable() & best.to_bit() != 0,
            "solver's best move must be legal"
        );
        assert!(result.nodes > 0);
    }

    #[test]
    fn test_solver_pass_position() {
        // A board where the current player cannot move at all
        let mut b = Board::new();
        b.black = 1u64 << 0; // A1
        b.white = 1u64 << 8; // B1
        b.empty_count = 62;
        b.player = crate::color::Color::White; // White has no move
        let mut solver = Solver::new(10);
        let result = solver.solve(EndSolverMode::Perfect, &b);
        assert_eq!(result.best_move, None, "pass position returns no move");
    }

    #[test]
    fn test_last1_exact() {
        // Fill a game down to exactly 1 empty and compare against negamax
        let board = position_with_empties(1);
        if board.empty_count() == 1 && !board.is_game_over() {
            let expected = negamax(&board, false);
            let mut solver = Solver::new(10);
            let result = solver.solve(EndSolverMode::Perfect, &board);
            assert_eq!(result.value, expected, "last1 exact score");
        }
    }

    #[test]
    fn test_neighbour_table() {
        let t = NeighbourTable::new();
        // A1 (corner, sq 0): neighbours are A2(1), B1(8), B2(9)
        assert_eq!(t.get(0), (1u64 << 1) | (1u64 << 8) | (1u64 << 9));
        // Center square E5 (sq 36) has 8 neighbours
        assert_eq!(t.get(36).count_ones(), 8);
    }

    #[test]
    fn test_quadrant_parity() {
        let b = Board::new();
        // 60 empties, 15 per quadrant (each quadrant has 16 squares, minus
        // one initial disc each) -> every quadrant parity is odd
        assert_eq!(parity_of(&b), 0b1111);
    }
}
