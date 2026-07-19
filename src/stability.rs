//! Stable-disc computation for endgame pruning.
//!
//! A disc is *stable* when no legal continuation can ever flip it. This
//! module computes a **conservative subset** of a side's stable discs —
//! under-counting only weakens the pruning bound, never its correctness.
//!
//! Rules used (file-major layout: bit = file*8 + rank):
//! 1. Edge discs can only be flipped along their own edge (every other
//!    line through an edge square has the board border on one side), so
//!    per edge: a completely full edge makes all its discs stable, and a
//!    corner-anchored run of same-colored discs is stable.
//! 2. An interior disc is stable if, in each of the 4 line directions, the
//!    full line through it is completely occupied or an adjacent stable
//!    friendly disc shields it on either side. Iterated to a fixpoint.

/// Rank-boundary masks (file-major: rank 0 bits are 0,8,16,…; rank 7 bits
/// are 7,15,23,…).
const RANK0: u64 = 0x0101_0101_0101_0101;
const RANK7: u64 = 0x8080_8080_8080_8080;
const NOT_RANK0: u64 = !RANK0;
const NOT_RANK7: u64 = !RANK7;

/// Masks of the 15 diagonals in each direction, built at compile time.
/// d9 diagonals run along +9 (file+1, rank+1); d7 along +7 (file+1, rank-1).
const fn diag_masks() -> ([u64; 15], [u64; 15]) {
    let mut d9 = [0u64; 15];
    let mut d7 = [0u64; 15];
    let mut sq = 0;
    while sq < 64 {
        let file = sq / 8;
        let rank = sq % 8;
        // d9 diagonal id: rank - file + 7 (constant along +9 steps)
        d9[(rank + 7 - file) as usize] |= 1u64 << sq;
        // d7 diagonal id: rank + file (constant along +7 steps)
        d7[(rank + file) as usize] |= 1u64 << sq;
        sq += 1;
    }
    (d9, d7)
}
const DIAGS: ([u64; 15], [u64; 15]) = diag_masks();

/// Squares whose full line (in one direction) is completely occupied.
#[inline]
fn full_lines(occ: u64) -> (u64, u64, u64, u64) {
    // Horizontal line for rank r = same bit r of every file
    let mut full_h = 0u64;
    let mut r = 0;
    while r < 8 {
        let m = RANK0 << r;
        if occ & m == m {
            full_h |= m;
        }
        r += 1;
    }
    // Vertical line = one file byte
    let mut full_v = 0u64;
    let mut f = 0;
    while f < 8 {
        let m = 0xFFu64 << (8 * f);
        if occ & m == m {
            full_v |= m;
        }
        f += 1;
    }
    let (d9s, d7s) = DIAGS;
    let mut full_d9 = 0u64;
    let mut full_d7 = 0u64;
    let mut i = 0;
    while i < 15 {
        if occ & d9s[i] == d9s[i] {
            full_d9 |= d9s[i];
        }
        if occ & d7s[i] == d7s[i] {
            full_d7 |= d7s[i];
        }
        i += 1;
    }
    (full_h, full_v, full_d9, full_d7)
}

/// Edge masks (file-major): rank-0 edge, rank-7 edge, file-0 edge, file-7.
const EDGE_R0: u64 = RANK0;
const EDGE_R7: u64 = RANK7;
const EDGE_F0: u64 = 0xFF;
const EDGE_F7: u64 = 0xFFu64 << 56;

/// Stable own discs on one edge: full edge -> every own disc on it;
/// otherwise own-colored runs anchored at either corner.
/// `step` advances along the edge from `corner_a` to `corner_b`.
#[inline]
fn edge_stable(own: u64, occ: u64, edge: u64, corner_a: u8, step: u8) -> u64 {
    if occ & edge == edge {
        return own & edge;
    }
    let mut stable = 0u64;
    // Run from corner_a forward
    let mut sq = corner_a;
    for _ in 0..8 {
        let bit = 1u64 << sq;
        if own & bit == 0 {
            break;
        }
        stable |= bit;
        sq = sq.wrapping_add(step);
    }
    // Run from corner_b backward
    let mut sq = corner_a.wrapping_add(step.wrapping_mul(7));
    for _ in 0..8 {
        let bit = 1u64 << sq;
        if own & bit == 0 {
            break;
        }
        stable |= bit;
        sq = sq.wrapping_sub(step);
    }
    stable
}

/// Conservative set of `own`'s stable discs.
pub fn stable_discs(own: u64, opp: u64) -> u64 {
    let occ = own | opp;
    let (full_h, full_v, full_d9, full_d7) = full_lines(occ);

    // Seed: edge stability + interior discs on four full lines
    let mut stable = edge_stable(own, occ, EDGE_R0, 0, 8)
        | edge_stable(own, occ, EDGE_R7, 7, 8)
        | edge_stable(own, occ, EDGE_F0, 0, 1)
        | edge_stable(own, occ, EDGE_F7, 56, 1);
    stable |= own & full_h & full_v & full_d9 & full_d7;

    // Propagate: a friendly disc shielded in all four directions is stable
    loop {
        let safe_h = full_h | (stable << 8) | (stable >> 8);
        let safe_v = full_v | ((stable & NOT_RANK7) << 1) | ((stable & NOT_RANK0) >> 1);
        let safe_d9 = full_d9 | ((stable & NOT_RANK7) << 9) | ((stable & NOT_RANK0) >> 9);
        let safe_d7 = full_d7 | ((stable & NOT_RANK0) << 7) | ((stable & NOT_RANK7) >> 7);
        let next = stable | (own & safe_h & safe_v & safe_d9 & safe_d7);
        if next == stable {
            return stable;
        }
        stable = next;
    }
}

/// Number of stable discs for `own`.
#[inline]
pub fn stable_count(own: u64, opp: u64) -> u32 {
    stable_discs(own, opp).count_ones()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;
    use crate::position::Position;

    /// Deterministic playout helper
    fn random_playout(mut board: Board, seed: u64) -> Board {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        loop {
            let moves = board.movable();
            if moves == 0 {
                let mut p = board;
                p.pass();
                if p.movable() == 0 {
                    return board;
                }
                board = p;
                continue;
            }
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut nth = (state >> 33) % moves.count_ones() as u64;
            let mut m = moves;
            while nth > 0 {
                m &= m - 1;
                nth -= 1;
            }
            board.make_move_unchecked(Position::from_index(m.trailing_zeros()).unwrap());
        }
    }

    fn random_position(seed: u64, plies: usize) -> Board {
        let mut board = Board::new();
        let mut state = seed.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
        for _ in 0..plies {
            let moves = board.movable();
            if moves == 0 {
                board.pass();
                if board.movable() == 0 {
                    break;
                }
                continue;
            }
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut nth = (state >> 33) % moves.count_ones() as u64;
            let mut m = moves;
            while nth > 0 {
                m &= m - 1;
                nth -= 1;
            }
            board.make_move_unchecked(Position::from_index(m.trailing_zeros()).unwrap());
        }
        board
    }

    #[test]
    fn test_empty_and_initial_have_no_stable() {
        assert_eq!(stable_discs(0, 0), 0);
        let b = Board::new();
        assert_eq!(stable_discs(b.black, b.white), 0);
        assert_eq!(stable_discs(b.white, b.black), 0);
    }

    #[test]
    fn test_corner_is_stable() {
        // A lone corner disc is stable
        let own = 1u64; // A1
        assert_eq!(stable_discs(own, 0), own);
    }

    #[test]
    fn test_full_board_all_stable() {
        // Any full board: every disc is stable
        let own = 0xAAAA_5555_F0F0_0F0Fu64;
        let opp = !own;
        assert_eq!(stable_discs(own, opp), own);
        assert_eq!(stable_discs(opp, own), opp);
    }

    /// The load-bearing property: a disc reported stable must still belong
    /// to the same side at the end of ANY continuation. Checked over many
    /// random positions × many random playouts.
    #[test]
    fn test_stable_discs_never_flip_in_playouts() {
        for seed in 1..=30u64 {
            let board = random_position(seed, 20 + (seed as usize % 30));
            for (own, opp, own_is_black) in
                [(board.black, board.white, true), (board.white, board.black, false)]
            {
                let stable = stable_discs(own, opp);
                if stable == 0 {
                    continue;
                }
                for playout_seed in 1..=10u64 {
                    let end = random_playout(board, seed * 1000 + playout_seed);
                    let end_own = if own_is_black { end.black } else { end.white };
                    assert_eq!(
                        stable & end_own,
                        stable,
                        "seed {seed}/{playout_seed}: a 'stable' disc flipped"
                    );
                }
            }
        }
    }
}
