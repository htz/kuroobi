//! Midgame search: iterative-deepening alpha-beta (negamax) over the
//! pattern evaluator, with a small transposition table and move ordering.
//!
//! Scores are evaluator units (approximate final disc difference) from the
//! current player's perspective. When the remaining depth reaches the
//! terminal (game over) the exact score is used, scaled by SCORE_SCALE to
//! dominate heuristic values.

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
pub struct Searcher {
    tt: Vec<TtEntry>,
    mask: u64,
    nodes: u64,
    /// Killer moves per ply: moves that recently caused a beta cutoff at
    /// the same distance from the root.
    killers: [[Option<Position>; 2]; MAX_PLY],
    /// History heuristic: cutoff credit per (player, square).
    history: [[u32; 64]; 2],
}

impl Searcher {
    /// `bit_size`: transposition table entries = 2^bit_size (~48 B each).
    pub fn new(bit_size: u32) -> Searcher {
        let size = 1usize << bit_size;
        Searcher {
            tt: vec![TtEntry::EMPTY; size],
            mask: (size - 1) as u64,
            nodes: 0,
            killers: [[None; 2]; MAX_PLY],
            history: [[0; 64]; 2],
        }
    }

    /// Invalidate all cached entries. Required whenever the evaluator's
    /// weights change (stored values embed the old evaluation) or when the
    /// same Searcher must serve a different evaluator: TT entries are keyed
    /// by position only, so cross-evaluator reuse silently corrupts search.
    pub fn clear(&mut self) {
        self.tt.fill(TtEntry::EMPTY);
    }

    /// Search to `depth` plies with iterative deepening (better move
    /// ordering from shallower passes via the transposition table).
    /// Returns None best_move if the side to move must pass.
    pub fn search(&mut self, board: &Board, evaluator: &Evaluator, depth: u8) -> SearchResult {
        self.nodes = 0;

        if board.movable() == 0 {
            return SearchResult {
                best_move: None,
                value: 0.0,
                nodes: 0,
                depth: 0,
            };
        }

        let mut best_move = None;
        let mut value = 0.0f32;
        let mut completed = 0u8;

        // Pattern indices are maintained incrementally (apply/undo around
        // each recursion) instead of recomputed per eval — the dominant
        // cost of the previous implementation.
        let mut indices = evaluator.indexer().init(board.black, board.white);

        // Ordering heuristics are per-decision: fresh for this search call,
        // shared across the iterative-deepening iterations inside it.
        self.killers = [[None; 2]; MAX_PLY];
        self.history = [[0; 64]; 2];

        // (Aspiration windows were measured to interact unsoundly with
        // PVS null-window entries in the shared TT — root exactness broke.
        // PVS + ETC alone keep the exactness invariant.)
        for d in 1..=depth {
            let hash = zobrist::compute_hash(board.black, board.white, board.player());
            let v = self.alpha_beta(board, evaluator, &mut indices, hash, d, 0, -INF, INF, false);
            // Root best move comes from the TT entry just stored
            if let Some(entry) = self.tt_probe(board, hash) {
                if entry.best.is_some() {
                    best_move = entry.best;
                }
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

    fn tt_probe(&self, board: &Board, hash: u64) -> Option<&TtEntry> {
        let e = &self.tt[(hash & self.mask) as usize];
        (e.used && e.key == hash && e.black == board.black && e.white == board.white)
            .then_some(e)
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
        let e = &mut self.tt[(hash & self.mask) as usize];
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
        let mut children = [(Position(0), *board, 0u64, 0u64, 0i64); 34];
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
                // opponent's view: lower = better for us
                indexer.apply(indices, pos, flipped, mover);
                let v = evaluator.eval_indices(&child, indices);
                indexer.undo(indices, pos, flipped, mover);
                (v * 256.0) as i64
            } else if Some(pos) == killer0 {
                i64::MIN + 1
            } else if Some(pos) == killer1 {
                i64::MIN + 2
            } else if depth >= 2 {
                ((child.movable_count() as i64) << 32)
                    + SQUARE_BIAS[pos.index() as usize]
                    - ((self.history[mover.index()][pos.index() as usize] as i64) << 8)
            } else {
                0
            };
            children[n_children] = (pos, child, child_hash, flipped, order_key);
            n_children += 1;
        }
        let children = &mut children[..n_children];
        if depth >= 2 || tt_move.is_some() {
            children.sort_unstable_by_key(|c| c.4);
        }

        // Enhanced transposition cutoff: a child entry that already proves
        // our fail-high ends this node without any search.
        if depth >= 4 {
            for (_, child, child_hash, _, _) in children.iter() {
                if let Some(e) = self.tt_probe(child, *child_hash) {
                    if e.depth >= depth - 1 && !matches!(e.bound, Bound::Lower) && -e.value >= beta
                    {
                        return -e.value;
                    }
                }
            }
        }

        let mut best_val = -INF;
        let mut best_move = None;

        for (i, (pos, child, child_hash, flipped, _)) in children.iter().enumerate() {
            indexer.apply(indices, *pos, *flipped, mover);
            // PVS: full window on the first child, then null-window probes
            // with a full re-search only when a child actually improves.
            let v = if i == 0 || alpha >= beta {
                -self.alpha_beta(
                    child, evaluator, indices, *child_hash, depth - 1, ply + 1,
                    -beta, -alpha, false,
                )
            } else {
                let probe = -self.alpha_beta(
                    child, evaluator, indices, *child_hash, depth - 1, ply + 1,
                    -(alpha + PVS_EPSILON), -alpha, false,
                );
                if probe > alpha && probe < beta {
                    // Re-search with the regular window: using probe as the
                    // lower bound is unsound under TT-induced instability
                    -self.alpha_beta(
                        child, evaluator, indices, *child_hash, depth - 1, ply + 1,
                        -beta, -alpha, false,
                    )
                } else {
                    probe
                }
            };
            indexer.undo(indices, *pos, *flipped, mover);
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
