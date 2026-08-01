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

/// Masks for the diagonal full-line cascade, for our file-major layout
/// (bit = file*8 + rank).
/// `L<k>` holds the squares with no predecessor k steps back along the
/// diagonal, `R<k>` the squares with no successor k steps ahead.
/// d9 runs along +9 (file+1, rank+1); d7 along +7 (file+1, rank-1).
const D9_L1: u64 = 0x0101_0101_0101_01FF;
const D9_L2: u64 = 0x0303_0303_0303_FFFF;
const D9_L4: u64 = 0x0F0F_0F0F_FFFF_FFFF;
const D9_R1: u64 = 0xFF80_8080_8080_8080;
const D9_R2: u64 = 0xFFFF_C0C0_C0C0_C0C0;
const D9_R4: u64 = 0xFFFF_FFFF_F0F0_F0F0;
const D7_L1: u64 = 0x8080_8080_8080_80FF;
const D7_L2: u64 = 0xC0C0_C0C0_C0C0_FFFF;
const D7_L4: u64 = 0xF0F0_F0F0_FFFF_FFFF;
const D7_R1: u64 = 0xFF01_0101_0101_0101;
const D7_R2: u64 = 0xFFFF_0303_0303_0303;
const D7_R4: u64 = 0xFFFF_FFFF_0F0F_0F0F;

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

    // Diagonals: a doubling cascade rather than testing 15 masks per
    // direction. Each step doubles the
    // run length proved occupied; the mask covers squares with no
    // predecessor that far back, where the shifted-in bits are meaningless.
    // `x >> k` carries each square's *successor* k steps along the diagonal
    // into it, so the `R` masks (no successor that far ahead) pair with the
    // right shifts and the `L` masks with the left ones.
    let mut fwd = occ;
    fwd &= D9_R1 | (fwd >> 9);
    fwd &= D9_R2 | (fwd >> 18);
    fwd &= D9_R4 | (fwd >> 36);
    let mut back = occ;
    back &= D9_L1 | (back << 9);
    back &= D9_L2 | (back << 18);
    back &= D9_L4 | (back << 36);
    let full_d9 = fwd & back;

    let mut fwd = occ;
    fwd &= D7_R1 | (fwd >> 7);
    fwd &= D7_R2 | (fwd >> 14);
    fwd &= D7_R4 | (fwd >> 28);
    let mut back = occ;
    back &= D7_L1 | (back << 7);
    back &= D7_L2 | (back << 14);
    back &= D7_L4 | (back << 28);
    let full_d7 = fwd & back;

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

/// Multiplier that folds one bit per byte into the top byte: a bit at
/// position 8f lands at 56+f, so the whole rank arrives as a single byte.
const GATHER_MAGIC: u64 = 0x0102_0408_1020_4080;
/// Its inverse as a table: the multiply that would spread a byte back over
/// one bit per byte carries across lanes, so a 2 KiB table does it instead.
const SCATTER_RANK0: [u64; 256] = {
    let mut t = [0u64; 256];
    let mut m = 0usize;
    while m < 256 {
        let mut out = 0u64;
        let mut f = 0;
        while f < 8 {
            if m & (1 << f) != 0 {
                out |= 1u64 << (f * 8);
            }
            f += 1;
        }
        t[m] = out;
        m += 1;
    }
    t
};

/// Gather the 8 bits of rank `r` (one per file) into a byte, bit = file.
#[inline]
fn gather_rank(x: u64, r: u32) -> u8 {
    (((x >> r) & RANK0).wrapping_mul(GATHER_MAGIC) >> 56) as u8
}

/// Scatter a byte back onto rank `r` (bit f -> square f*8 + r).
#[inline]
fn scatter_rank(mask: u8, r: u32) -> u64 {
    SCATTER_RANK0[mask as usize] << r
}

/// Exact stable own discs on the four edges.
pub(crate) fn edge_stable_all(own: u64, opp: u64) -> u64 {
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

    #[test]
    fn rank_gather_scatter_match_the_bit_loops() {
        fn gather_ref(x: u64, r: u32) -> u8 {
            let mut out = 0u8;
            for f in 0..8 {
                if x & (1u64 << (f * 8 + r)) != 0 {
                    out |= 1 << f;
                }
            }
            out
        }
        fn scatter_ref(mask: u8, r: u32) -> u64 {
            let mut out = 0u64;
            for f in 0..8 {
                if mask & (1 << f) != 0 {
                    out |= 1u64 << (f * 8 + r);
                }
            }
            out
        }
        // Scatter is exhaustive over its whole domain.
        for r in 0..8u32 {
            for m in 0..=255u8 {
                assert_eq!(scatter_rank(m, r), scatter_ref(m, r), "m={m} r={r}");
            }
        }
        // Gather: every single-bit board, then random dense ones.
        for r in 0..8u32 {
            for b in 0..64 {
                let x = 1u64 << b;
                assert_eq!(gather_rank(x, r), gather_ref(x, r), "b={b} r={r}");
            }
        }
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            for r in 0..8u32 {
                assert_eq!(gather_rank(x, r), gather_ref(x, r), "x={x:#018x} r={r}");
            }
        }
    }

    /// Reference: a diagonal is full iff every one of its squares is
    /// occupied. Tests the doubling cascade in `full_lines` against the
    /// direct definition.
    fn full_diags_ref(occ: u64) -> (u64, u64) {
        let mut d9 = [0u64; 15];
        let mut d7 = [0u64; 15];
        for sq in 0..64u32 {
            let (file, rank) = (sq / 8, sq % 8);
            d9[(rank + 7 - file) as usize] |= 1u64 << sq;
            d7[(rank + file) as usize] |= 1u64 << sq;
        }
        let (mut f9, mut f7) = (0u64, 0u64);
        for i in 0..15 {
            if occ & d9[i] == d9[i] {
                f9 |= d9[i];
            }
            if occ & d7[i] == d7[i] {
                f7 |= d7[i];
            }
        }
        (f9, f7)
    }

    #[test]
    fn full_line_cascade_matches_definition() {
        // xorshift64 — deterministic, no dev-dependency needed.
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for i in 0..200_000 {
            // Mix densities: sparse, balanced and nearly full boards all
            // exercise different parts of the cascade.
            let occ = match i % 4 {
                0 => next(),
                1 => next() & next(),
                2 => next() | next(),
                _ => !(next() & next() & next()),
            };
            let (_, _, d9, d7) = full_lines(occ);
            assert_eq!((d9, d7), full_diags_ref(occ), "occ = {occ:#018x}");
        }
        // Boundary cases the random draws are unlikely to hit.
        for occ in [0u64, !0u64, RANK0, RANK7, 0xFF, 0xFF00_0000_0000_0000] {
            let (_, _, d9, d7) = full_lines(occ);
            assert_eq!((d9, d7), full_diags_ref(occ), "occ = {occ:#018x}");
        }
    }
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
        let mut state = seed
            .wrapping_mul(2862933555777941757)
            .wrapping_add(3037000493);
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
            for (own, opp, own_is_black) in [
                (board.black, board.white, true),
                (board.white, board.black, false),
            ] {
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
