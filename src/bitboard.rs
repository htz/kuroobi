//! Low-level bitboard operations on raw u64 values.
//! All functions take and return u64 directly (no wrapper type).
//!
//! Square layout is file-major: bit = file*8 + rank. Direction offsets:
//!   ±1 = rank axis (vertical), ±8 = file axis (horizontal),
//!   ±7 / ±9 = diagonals.
//!
//! Edge-wrap safety uses opponent masks rather than per-step masks: a flip
//! run can never contain an edge square in the scan direction (its anchor
//! would be off-board), so stripping those squares from the opponent set
//! makes the unrolled shift scans wrap-free.

/// Opponent mask for ±1 scans: exclude ranks 0 and 7.
const MASK_RANK: u64 = 0x7E7E7E7E7E7E7E7E;
/// Opponent mask for ±8 scans: exclude files 0 and 7.
const MASK_FILE: u64 = 0x00FFFFFFFFFFFF00;
/// Opponent mask for ±7/±9 scans: exclude both.
const MASK_DIAG: u64 = 0x007E7E7E7E7E7E00;

/// Directional flip scan, branch-free with a Kogge-Stone doubling smear.
///
/// Smears the placed disc through a contiguous run of (masked) opponent
/// discs; a run is a real flip only if the square just past it holds a
/// player disc (the anchor). Doubling covers the max 6-disc run in
/// 1 + 1 + 2 + 2 steps (dependency chain of 4 instead of 6).
#[inline(always)]
fn flip_shift<const DIR: u32, const UP: bool>(player: u64, opp_masked: u64, pos: u64) -> u64 {
    #[inline(always)]
    fn sh<const N: u32, const UP: bool>(x: u64) -> u64 {
        if UP {
            x << N
        } else {
            x >> N
        }
    }

    let mut f = sh::<DIR, UP>(pos) & opp_masked; // run length 1
    if f == 0 {
        // Most directions miss immediately; skip the smear entirely.
        return 0;
    }

    // opp2: pairs of adjacent opponent discs, lets one step cover two squares
    let opp2 = opp_masked & sh::<DIR, UP>(opp_masked);

    f |= sh::<DIR, UP>(f) & opp_masked; // up to 2
    f |= match DIR {
        1 => sh::<2, UP>(f) & opp2,
        7 => sh::<14, UP>(f) & opp2,
        8 => sh::<16, UP>(f) & opp2,
        _ => sh::<18, UP>(f) & opp2,
    }; // up to 4
    f |= match DIR {
        1 => sh::<2, UP>(f) & opp2,
        7 => sh::<14, UP>(f) & opp2,
        8 => sh::<16, UP>(f) & opp2,
        _ => sh::<18, UP>(f) & opp2,
    }; // up to 6

    // Every cell in f is an opponent disc, so the only shifted cell that can
    // land on a player disc is the one just past the run's tip: exact anchor.
    if sh::<DIR, UP>(f) & player != 0 {
        f
    } else {
        0
    }
}

/// Returns all discs flipped by placing a player disc on `pos_bit`
/// (all 8 directions, not including the placed disc). 0 = illegal move.
#[inline]
pub fn flippable_generic(player_bb: u64, opponent_bb: u64, pos_bit: u64) -> u64 {
    let o_rank = opponent_bb & MASK_RANK;
    let o_file = opponent_bb & MASK_FILE;
    let o_diag = opponent_bb & MASK_DIAG;

    flip_shift::<1, true>(player_bb, o_rank, pos_bit)
        | flip_shift::<1, false>(player_bb, o_rank, pos_bit)
        | flip_shift::<8, true>(player_bb, o_file, pos_bit)
        | flip_shift::<8, false>(player_bb, o_file, pos_bit)
        | flip_shift::<7, true>(player_bb, o_diag, pos_bit)
        | flip_shift::<7, false>(player_bb, o_diag, pos_bit)
        | flip_shift::<9, true>(player_bb, o_diag, pos_bit)
        | flip_shift::<9, false>(player_bb, o_diag, pos_bit)
}

/// Check if a move at `pos_bit` is valid (empty square that flips something).
#[inline]
pub fn check(player_bb: u64, opponent_bb: u64, pos_bit: u64) -> bool {
    let mask = !(player_bb | opponent_bb) & pos_bit;
    if mask == 0 {
        return false; // Occupied
    }
    flippable(player_bb, opponent_bb, mask) != 0
}

/// Check if the current player has at least one legal move.
#[inline]
pub fn check_all(player_bb: u64, opponent_bb: u64) -> bool {
    let empty = !(player_bb | opponent_bb);
    mobility(player_bb, opponent_bb, empty) != 0
}

/// Kogge-Stone style scan in one axis: propagate player pieces through
/// contiguous (masked) opponent pieces, then step once more past the run.
/// Kogge-Stone smear along one axis, using the same doubling trick as
/// [`flip_shift`]: pairs of adjacent opponent discs (`opp2`) let one step
/// advance two squares, so the six-long maximum run needs four dependent
/// steps instead of six. The two directions are smeared separately, which
/// keeps each chain independent and lets the out-of-order core overlap them.
#[inline(always)]
fn some_mobility<const DIR: u32>(player_bb: u64, masked_opp: u64) -> u64 {
    let up2 = masked_opp & (masked_opp << DIR);
    let dn2 = masked_opp & (masked_opp >> DIR);

    let mut u = (player_bb << DIR) & masked_opp;
    u |= (u << DIR) & masked_opp;
    u |= (u << (2 * DIR)) & up2;
    u |= (u << (2 * DIR)) & up2;

    let mut d = (player_bb >> DIR) & masked_opp;
    d |= (d >> DIR) & masked_opp;
    d |= (d >> (2 * DIR)) & dn2;
    d |= (d >> (2 * DIR)) & dn2;

    (u << DIR) | (d >> DIR)
}

/// Returns a bitboard of all playable positions for the current player.
#[inline]
pub fn mobility(player_bb: u64, opponent_bb: u64, empty_bb: u64) -> u64 {
    (some_mobility::<1>(player_bb, opponent_bb & MASK_RANK)
        | some_mobility::<8>(player_bb, opponent_bb & MASK_FILE)
        | some_mobility::<7>(player_bb, opponent_bb & MASK_DIAG)
        | some_mobility::<9>(player_bb, opponent_bb & MASK_DIAG))
        & empty_bb
}

/// Count of playable positions.
#[inline]
pub fn mobility_count(player_bb: u64, opponent_bb: u64) -> u8 {
    let empty = !(player_bb | opponent_bb);
    mobility(player_bb, opponent_bb, empty).count_ones() as u8
}

/// Returns a bitboard of empty squares.
#[inline]
pub fn empty_bb(black: u64, white: u64) -> u64 {
    !(black | white)
}

/// Transpose along the a1-h8 diagonal: bit (file, rank) -> (rank, file).
/// Classic delta-swap transpose; index math is symmetric so it works for
/// file-major exactly like the well-known rank-major version.
///
/// Also converts between rank-major (rank*8+file) and file-major
/// (file*8+rank) bit layouts of the same position, in either direction.
#[inline]
pub fn transpose(mut x: u64) -> u64 {
    let t = (x ^ (x >> 7)) & 0x00AA00AA00AA00AA;
    x ^= t ^ (t << 7);
    let t = (x ^ (x >> 14)) & 0x0000CCCC0000CCCC;
    x ^= t ^ (t << 14);
    let t = (x ^ (x >> 28)) & 0x00000000F0F0F0F0;
    x ^= t ^ (t << 28);
    x
}

/// 90-degree clockwise rotation: (file, rank) -> (rank, 7-file).
/// swap_bytes mirrors the files (each byte is one file in file-major),
/// then the transpose swaps the axes.
pub fn rotate_90(board: u64) -> u64 {
    transpose(board.swap_bytes())
}

/// Horizontal mirror: (file, rank) -> (7-file, rank).
/// In file-major layout each byte is one file, so this is a byte reversal.
#[inline]
pub fn mirror_horizontal(board: u64) -> u64 {
    board.swap_bytes()
}

/// All 8 symmetries of a bitboard (identity, 3 rotations, mirror, 3 mirrored
/// rotations). The same index order applied to two bitboards yields
/// position-consistent pairs.
#[inline]
pub fn symmetries(board: u64) -> [u64; 8] {
    let r0 = board;
    let r1 = rotate_90(r0);
    let r2 = rotate_90(r1);
    let r3 = rotate_90(r2);
    let m0 = mirror_horizontal(board);
    let m1 = rotate_90(m0);
    let m2 = rotate_90(m1);
    let m3 = rotate_90(m2);
    [r0, r1, r2, r3, m0, m1, m2, m3]
}

/// Count set bits (popcount).
#[inline]
pub fn count_bits(bb: u64) -> u32 {
    bb.count_ones()
}

/// Iterate over set bit positions.
/// Yields the index (0..63) for each set bit.
#[inline]
pub fn iter_bits(bb: u64) -> impl Iterator<Item = u8> {
    let mut remaining = bb;
    std::iter::from_fn(move || -> Option<u8> {
        if remaining == 0 {
            None
        } else {
            let bit = remaining.trailing_zeros() as u8;
            remaining &= remaining - 1;
            Some(bit)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slow, obviously-correct reference: walk each of the 8 directions
    /// square by square using file/rank coordinates (no bit tricks at all).
    fn flippable_reference(player: u64, opp: u64, pos_bit: u64) -> u64 {
        let sq = pos_bit.trailing_zeros() as i32;
        let (pf, pr) = (sq / 8, sq % 8);
        let mut flipped = 0u64;
        for (df, dr) in [
            (-1, -1), (-1, 0), (-1, 1),
            (0, -1), (0, 1),
            (1, -1), (1, 0), (1, 1),
        ] {
            let mut run = 0u64;
            let (mut f, mut r) = (pf + df, pr + dr);
            loop {
                if !(0..8).contains(&f) || !(0..8).contains(&r) {
                    break; // ran off board: no anchor
                }
                let bit = 1u64 << (f * 8 + r);
                if opp & bit != 0 {
                    run |= bit;
                } else if player & bit != 0 {
                    flipped |= run; // anchored run
                    break;
                } else {
                    break; // empty: no anchor
                }
                f += df;
                r += dr;
            }
        }
        flipped
    }

    /// Deterministic pseudo-random (player, opponent) pairs.
    fn random_boards(n: usize) -> Vec<(u64, u64)> {
        let mut state = 0x243F6A8885A308D3u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        (0..n)
            .map(|_| {
                let a = next();
                let b = next();
                // Disjoint piece sets
                (a & !b, b & !a)
            })
            .collect()
    }

    #[test]
    fn test_flippable_matches_reference_on_random_boards() {
        for (player, opp) in random_boards(500) {
            let empty = !(player | opp);
            let mut e = empty;
            while e != 0 {
                let bit = 1u64 << e.trailing_zeros();
                e &= e - 1;
                assert_eq!(
                    flippable(player, opp, bit),
                    flippable_reference(player, opp, bit),
                    "player={player:#x} opp={opp:#x} pos={bit:#x}"
                );
            }
        }
    }

    #[test]
    fn test_flippable_all_eight_directions_from_center() {
        // Place at E4 (file 4, rank 3 -> bit 35); one opponent disc adjacent
        // in each direction with a player anchor beyond it.
        let pos = 1u64 << 35;
        for (df, dr) in [
            (-1i32, -1i32), (-1, 0), (-1, 1),
            (0, -1), (0, 1),
            (1, -1), (1, 0), (1, 1),
        ] {
            let opp_sq = ((4 + df) * 8 + (3 + dr)) as u64;
            let anchor_sq = ((4 + 2 * df) * 8 + (3 + 2 * dr)) as u64;
            let opp = 1u64 << opp_sq;
            let player = 1u64 << anchor_sq;
            assert_eq!(
                flippable(player, opp, pos),
                opp,
                "direction ({df},{dr}) must flip exactly the adjacent disc"
            );
        }
    }

    #[test]
    fn test_flippable_edge_wrap_regressions() {
        // Vertical wrap: A8 (bit 7) and B1 (bit 8) are adjacent bit indices
        // but not adjacent squares. Placing at A8 must not flip B1.
        let player = 1u64 << 9; // B2
        let opp = 1u64 << 8; // B1
        assert_eq!(
            flippable(player, opp, 1u64 << 7), // place at A8
            0,
            "A8 -> B1 crosses the board edge and must not flip"
        );

        // Horizontal full-rank flip along rank 0: A1 anchor, B1..G1 opponent,
        // place at H1. Legal and flips all six.
        let player = 1u64 << 0; // A1
        let opp = (1u64 << 8) | (1u64 << 16) | (1u64 << 24) | (1u64 << 32) | (1u64 << 40) | (1u64 << 48);
        assert_eq!(
            flippable(player, opp, 1u64 << 56), // H1
            opp,
            "full-rank horizontal flip"
        );

        // Diagonal wrap: placing at H2 (bit 57) must not reach A-file squares
        // via the +7/+9 bit offsets wrapping files.
        let player = 1u64 << 2; // A3
        let opp = 1u64 << 1; // A2
        assert_eq!(
            flippable(player, opp, 1u64 << 57), // H2
            0,
            "H2 -> A2 is not a real diagonal"
        );
    }

    #[test]
    fn test_flippable_no_anchor_no_flip() {
        // Run of opponents ending at the board edge (no anchor): no flip.
        let player = 0;
        let opp = (1u64 << 8) | (1u64 << 16) | (1u64 << 24); // B1, C1, D1
        assert_eq!(flippable(player, opp, 1u64 << 0), 0, "no anchor at edge");

        // Run ending on an empty square: no flip.
        let player = 1u64 << 40; // F1 present but gap at E1
        let opp = (1u64 << 8) | (1u64 << 16); // B1, C1 (D1 empty)
        assert_eq!(flippable(player, opp, 1u64 << 0), 0, "gap breaks the run");
    }

    #[test]
    fn test_flippable_long_run() {
        // Maximum-length run: six opponent discs between pos and anchor.
        let player = 1u64 << 7; // A8 (rank 7)
        let opp = (1u64 << 1) | (1u64 << 2) | (1u64 << 3) | (1u64 << 4) | (1u64 << 5) | (1u64 << 6);
        assert_eq!(
            flippable(player, opp, 1u64 << 0), // A1
            opp,
            "six-disc vertical run must flip entirely"
        );
    }

    #[test]
    fn test_check_valid_and_occupied() {
        let player = 1u64 << 0; // A1
        let opp = 1u64 << 8; // B1
        assert!(check(player, opp, 1u64 << 16), "C1 flips B1");
        assert!(!check(player, opp, 1u64 << 8), "occupied square");
        assert!(!check(player, opp, 1u64 << 32), "E1 flips nothing");
    }

    #[test]
    fn test_mobility_matches_flippable() {
        for (player, opp) in random_boards(200) {
            let empty = !(player | opp);
            let mob = mobility(player, opp, empty);
            let mut e = empty;
            while e != 0 {
                let bit = 1u64 << e.trailing_zeros();
                e &= e - 1;
                let can_flip = flippable(player, opp, bit) != 0;
                assert_eq!(
                    mob & bit != 0,
                    can_flip,
                    "mobility and flippable disagree at {bit:#x}"
                );
            }
        }
    }

    #[test]
    fn test_check_all_consistency() {
        for (player, opp) in random_boards(100) {
            let empty = !(player | opp);
            assert_eq!(
                check_all(player, opp),
                mobility(player, opp, empty) != 0
            );
        }
    }

    #[test]
    fn test_empty_bb() {
        assert_eq!(empty_bb(0, 0), u64::MAX);
        assert_eq!(empty_bb(1, 0), u64::MAX ^ 1);
        assert_eq!(empty_bb(u64::MAX, 0), 0);
    }

    #[test]
    fn test_count_bits() {
        assert_eq!(count_bits(0), 0);
        assert_eq!(count_bits(1), 1);
        assert_eq!(count_bits(0xFF), 8);
        assert_eq!(count_bits(u64::MAX), 64);
    }

    #[test]
    fn test_iter_bits() {
        let bb = (1u64 << 3) | (1u64 << 17) | (1u64 << 63);
        let collected: Vec<u8> = iter_bits(bb).collect();
        assert_eq!(collected, vec![3, 17, 63]);
        assert_eq!(iter_bits(0).count(), 0);
    }

    #[test]
    fn test_rotate_90() {
        // Corner cycle under (file, rank) -> (rank, 7-file):
        // A1 -> A8 -> H8 -> H1 -> A1 (file-major bits 0 -> 7 -> 63 -> 56 -> 0)
        assert_eq!(rotate_90(1u64 << 0), 1u64 << 7, "A1 -> A8");
        assert_eq!(rotate_90(1u64 << 7), 1u64 << 63, "A8 -> H8");
        assert_eq!(rotate_90(1u64 << 63), 1u64 << 56, "H8 -> H1");
        assert_eq!(rotate_90(1u64 << 56), 1u64 << 0, "H1 -> A1");

        // 4 rotations return to the original; popcount is preserved.
        let original: u64 = 0x123456789ABCDEF0;
        let r1 = rotate_90(original);
        assert_eq!(r1.count_ones(), original.count_ones());
        let r4 = rotate_90(rotate_90(rotate_90(r1)));
        assert_eq!(r4, original, "4x 90° rotation is identity");
    }
}

// ---------------------------------------------------------------------------
// Per-square specialized flip
//
// The generic routine walks all eight directions for every square, reading
// masks from memory. Specializing on the square instead lets the compiler bake
// each ray mask in as an immediate and delete the directions that do not exist
// there — a corner only has three. The const generic gives that
// per-square monomorphization without 64 hand-written functions behind a
// pointer table.
// ---------------------------------------------------------------------------

/// Ray masks out of every square, `[square][axis]`, towards higher bit
/// indices. Axis order matches [`LINE_DELTAS`].
const RAY_UP: [[u64; 4]; 64] = build_ray_masks(true);
/// Same, towards lower bit indices, stored bit-reversed so one routine
/// handles both halves (`reverse_bits` is a single `rbit` on aarch64).
const RAY_DOWN_REV: [[u64; 4]; 64] = {
    let src = build_ray_masks(false);
    let mut t = [[0u64; 4]; 64];
    let mut sq = 0usize;
    while sq < 64 {
        let mut a = 0usize;
        while a < 4 {
            t[63 - sq][a] = src[sq][a].reverse_bits();
            a += 1;
        }
        sq += 1;
    }
    t
};

const LINE_DELTAS: [(i32, i32); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];

const fn build_ray_masks(increasing: bool) -> [[u64; 4]; 64] {
    let mut t = [[0u64; 4]; 64];
    let mut sq = 0i32;
    while sq < 64 {
        let f0 = sq / 8;
        let r0 = sq % 8;
        let mut a = 0usize;
        while a < 4 {
            let (df, dr) = LINE_DELTAS[a];
            let (df, dr) = if increasing { (df, dr) } else { (-df, -dr) };
            let mut ray = 0u64;
            let mut f = f0 + df;
            let mut r = r0 + dr;
            while f >= 0 && f < 8 && r >= 0 && r < 8 {
                ray |= 1u64 << (f * 8 + r);
                f += df;
                r += dr;
            }
            t[sq as usize][a] = ray;
            a += 1;
        }
        sq += 1;
    }
    t
}

/// Discs flipped along one ray. The nearest square on the ray that is not an
/// opponent disc ends the run; the run flips only if the player owns it.
#[inline(always)]
fn ray_run(player: u64, opp: u64, ray: u64) -> u64 {
    let gap = ray & !opp;
    let stop = gap & gap.wrapping_neg();
    // All-ones when a player disc anchors the run, zero otherwise (which
    // also covers `stop == 0`: opponent discs all the way to the edge).
    let anchored = 0u64.wrapping_sub(((stop & player) != 0) as u64);
    ray & stop.wrapping_sub(1) & anchored
}

/// Flip for a square known at compile time. Empty rays fold away entirely.
fn flip_at<const SQ: usize>(player: u64, opponent: u64) -> u64 {
    let up = RAY_UP[SQ];
    let mut f = 0u64;
    if up[0] != 0 {
        f |= ray_run(player, opponent, up[0]);
    }
    if up[1] != 0 {
        f |= ray_run(player, opponent, up[1]);
    }
    if up[2] != 0 {
        f |= ray_run(player, opponent, up[2]);
    }
    if up[3] != 0 {
        f |= ray_run(player, opponent, up[3]);
    }

    let down = RAY_DOWN_REV[63 - SQ];
    if down[0] | down[1] | down[2] | down[3] != 0 {
        let rp = player.reverse_bits();
        let ro = opponent.reverse_bits();
        let mut g = 0u64;
        if down[0] != 0 {
            g |= ray_run(rp, ro, down[0]);
        }
        if down[1] != 0 {
            g |= ray_run(rp, ro, down[1]);
        }
        if down[2] != 0 {
            g |= ray_run(rp, ro, down[2]);
        }
        if down[3] != 0 {
            g |= ray_run(rp, ro, down[3]);
        }
        f |= g.reverse_bits();
    }
    f
}

macro_rules! flip_table {
    ($($i:literal),*) => {
        /// One specialized flip per square, selected by index.
        static FLIP_AT: [fn(u64, u64) -> u64; 64] = [$(flip_at::<$i>),*];
    };
}

flip_table!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63
);

/// Specialized flip.
#[inline]
pub fn flippable(player_bb: u64, opponent_bb: u64, pos_bit: u64) -> u64 {
    FLIP_AT[pos_bit.trailing_zeros() as usize](player_bb, opponent_bb)
}

#[cfg(test)]
mod spec_flip_tests {
    use super::*;

    #[test]
    fn specialized_flip_matches_generic() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        };
        for _ in 0..30_000 {
            let a = next();
            let b = next();
            let player = a & !b;
            let opponent = b & !a;
            for sq in 0..64u32 {
                let pos = 1u64 << sq;
                if (player | opponent) & pos != 0 {
                    continue;
                }
                assert_eq!(
                    flippable(player, opponent, pos),
                    flippable_generic(player, opponent, pos),
                    "sq={sq} player={player:#018x} opponent={opponent:#018x}"
                );
            }
        }
    }
}

#[cfg(test)]
mod mobility_smear_tests {
    use super::*;

    /// Mobility must agree with "some direction flips something" on random
    /// positions, including shapes real games never reach.
    #[test]
    fn mobility_agrees_with_flippable_random() {
        let mut state = 0x0123_4567_89AB_CDEFu64;
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for _ in 0..40_000 {
            let a = next();
            let b = next();
            let player = a & !b;
            let opponent = b & !a;
            let empty = !(player | opponent);
            let moves = mobility(player, opponent, empty);
            let mut e = empty;
            while e != 0 {
                let sq = e.trailing_zeros();
                e &= e - 1;
                let pos = 1u64 << sq;
                let flips = flippable(player, opponent, pos);
                assert_eq!(
                    flips != 0,
                    moves & pos != 0,
                    "sq={sq} player={player:#018x} opponent={opponent:#018x}"
                );
            }
        }
    }
}
