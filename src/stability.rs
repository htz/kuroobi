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
    // Horizontal (rank) lines: AND-reduce the 8 file bytes; bit r of the
    // result is set iff rank r is occupied in every file. Broadcast back.
    let mut h = occ;
    h &= h >> 32;
    h &= h >> 16;
    h &= h >> 8;
    let full_h = (h & 0xFF) * RANK0;

    // Vertical (file) lines: AND-reduce the 8 bits inside each byte down
    // to bit 0, then broadcast each byte.
    let mut v = occ & (occ >> 4) & 0x0F0F_0F0F_0F0F_0F0F;
    v &= v >> 2;
    v &= v >> 1;
    let full_v = (v & RANK0) * 0xFF;

    // Diagonals: 15 per direction, via precomputed masks
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

// ---------------------------------------------------------------------------
// Exact edge stability: an edge disc can only ever be flipped along its own
// edge, so each edge is a self-contained 1-D game over 3^8 configurations.
// A disc is edge-stable iff NO sequence of placements (either color on any
// empty square — a superset of what 2-D legality allows, hence conservative)
// can ever flip it. Precomputed by greatest-fixpoint iteration into a
// 64 KiB table: index = own_byte << 8 | opp_byte, value = stable own mask.
// ---------------------------------------------------------------------------

use std::sync::OnceLock;

/// 1-D placement with mandatory flips: `mover` places at empty square `s`.
/// Returns (new_mover_bits, new_other_bits).
fn place_1d(mover: u8, other: u8, s: u8) -> (u8, u8) {
    let mut flipped = 0u8;
    // Left of s
    let mut run = 0u8;
    let mut i = s;
    while i > 0 {
        i -= 1;
        let b = 1u8 << i;
        if other & b != 0 {
            run |= b;
        } else {
            if mover & b != 0 {
                flipped |= run;
            }
            break;
        }
    }
    // Right of s
    run = 0;
    i = s;
    while i < 7 {
        i += 1;
        let b = 1u8 << i;
        if other & b != 0 {
            run |= b;
        } else {
            if mover & b != 0 {
                flipped |= run;
            }
            break;
        }
    }
    (mover | (1 << s) | flipped, other & !flipped)
}

/// Greatest-fixpoint edge-stability table.
fn build_edge_table() -> Box<[u8; 65536]> {
    let mut stable = vec![0u8; 65536].into_boxed_slice();
    // Initialize: every own disc assumed stable (invalid configs stay 0)
    for own in 0..256usize {
        for opp in 0..256usize {
            if own & opp == 0 {
                stable[(own << 8) | opp] = own as u8;
            }
        }
    }
    // Iterate: a disc stays stable only if every single placement keeps it
    // unflipped and stable in the successor configuration.
    loop {
        let mut changed = false;
        for own in 0..256usize {
            for opp in 0..256usize {
                if own & opp != 0 {
                    continue;
                }
                let idx = (own << 8) | opp;
                let mut s_mask = stable[idx];
                if s_mask == 0 {
                    continue;
                }
                let empty = !(own | opp) as u8;
                let mut e = empty;
                while e != 0 {
                    let sq = e.trailing_zeros() as u8;
                    e &= e - 1;
                    // Own places: own discs can't flip, but successor matters
                    let (no, nx) = place_1d(own as u8, opp as u8, sq);
                    s_mask &= stable[((no as usize) << 8) | nx as usize];
                    // Opponent places: flips own discs directly
                    let (po, px) = place_1d(opp as u8, own as u8, sq);
                    // px = surviving own discs; own & !px were flipped
                    s_mask &= px & stable[((px as usize) << 8) | po as usize];
                    if s_mask == 0 {
                        break;
                    }
                }
                if s_mask != stable[idx] {
                    stable[idx] = s_mask;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    stable.try_into().expect("size 65536")
}

fn edge_table() -> &'static [u8; 65536] {
    static TABLE: OnceLock<Box<[u8; 65536]>> = OnceLock::new();
    TABLE.get_or_init(build_edge_table)
}

/// Gather the 8 bits of rank `r` (one per file) into a byte, bit = file.
#[inline]
fn gather_rank(x: u64, r: u32) -> u8 {
    let mut out = 0u8;
    let mut f = 0;
    while f < 8 {
        if x & (1u64 << (f * 8 + r)) != 0 {
            out |= 1 << f;
        }
        f += 1;
    }
    out
}

/// Scatter a byte back onto rank `r` (bit f -> square f*8 + r).
#[inline]
fn scatter_rank(mask: u8, r: u32) -> u64 {
    let mut out = 0u64;
    let mut f = 0;
    while f < 8 {
        if mask & (1 << f) != 0 {
            out |= 1u64 << (f * 8 + r);
        }
        f += 1;
    }
    out
}

/// Exact stable own discs on the four edges.
fn edge_stable_all(own: u64, opp: u64) -> u64 {
    let t = edge_table();
    let mut stable = 0u64;
    // Rank edges (r = 0 and 7)
    for r in [0u32, 7] {
        let o = gather_rank(own, r);
        let x = gather_rank(opp, r);
        stable |= scatter_rank(t[((o as usize) << 8) | x as usize], r);
    }
    // File edges (f = 0 and 7): file bytes are contiguous
    for f in [0u32, 7] {
        let o = ((own >> (8 * f)) & 0xFF) as usize;
        let x = ((opp >> (8 * f)) & 0xFF) as usize;
        stable |= ((t[(o << 8) | x] as u64) << (8 * f)) & (0xFFu64 << (8 * f));
    }
    stable
}

/// Conservative set of `own`'s stable discs.
pub fn stable_discs(own: u64, opp: u64) -> u64 {
    let occ = own | opp;
    let (full_h, full_v, full_d9, full_d7) = full_lines(occ);

    // Seed: exact edge stability + interior discs on four full lines
    let mut stable = edge_stable_all(own, opp);
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
