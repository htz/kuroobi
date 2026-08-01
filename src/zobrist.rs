//! Zobrist hashing for board state deduplication and transposition tables.

use crate::color::Color;
use crate::position::Position;
use std::sync::OnceLock;

/// Zobrist hash table for board state identification.
pub struct ZobristTable {
    /// Random u64 per position (64) per color (3: Black, White, Pass).
    pub set_zobrist: [[u64; 3]; 64],
    /// XOR table for flipped pieces (black XOR white per position).
    pub flip_zobrist: [u64; 64],
    /// Random u64 for player swap (pass).
    pub swap_player_zobrist: u64,
}

impl ZobristTable {
    /// Generate a new Zobrist table with random 64-bit keys.
    pub fn generate() -> Self {
        let mut table = ZobristTable {
            set_zobrist: [[0u64; 3]; 64],
            flip_zobrist: [0u64; 64],
            swap_player_zobrist: 0,
        };

        // Simple 64-bit LCG (Lehmer RNG)
        let mut seed = 0xdead_beef_cafe_babe;
        let gen = |s: &mut u64| -> u64 {
            *s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *s
        };

        for i in 0..64 {
            table.set_zobrist[i][Color::Black as usize] = gen(&mut seed);
            table.set_zobrist[i][Color::White as usize] = gen(&mut seed);
            table.flip_zobrist[i] = table.set_zobrist[i][Color::Black as usize]
                ^ table.set_zobrist[i][Color::White as usize];
        }
        table.swap_player_zobrist = gen(&mut seed);

        table
    }
}

/// Global Zobrist table (initialized once).
static TABLE: OnceLock<ZobristTable> = OnceLock::new();

/// Get the global Zobrist table (initialized once).
pub fn zobrist_table() -> &'static ZobristTable {
    TABLE.get_or_init(ZobristTable::generate)
}

/// Compute the Zobrist hash of the current board state.
pub fn compute_hash(black: u64, white: u64, player: Color) -> u64 {
    let table = zobrist_table();
    let mut hash = 0u64;

    // XOR for each black piece
    let mut b = black;
    while b != 0 {
        let pos = b.trailing_zeros() as u8;
        hash ^= table.set_zobrist[pos as usize][Color::Black as usize];
        b &= b - 1;
    }

    // XOR for each white piece
    let mut w = white;
    while w != 0 {
        let pos = w.trailing_zeros() as u8;
        hash ^= table.set_zobrist[pos as usize][Color::White as usize];
        w &= w - 1;
    }

    // XOR swap player key if current player is White
    if player == Color::White {
        hash ^= table.swap_player_zobrist;
    }

    hash
}

/// Incrementally update hash after a move.
pub fn update_hash_on_move(prev_hash: u64, pos: Position, flipped: u64, player: Color) -> u64 {
    let table = zobrist_table();
    let mut hash = prev_hash;

    // XOR flip keys for each flipped piece
    let mut f = flipped;
    while f != 0 {
        let pos_idx = f.trailing_zeros() as u8;
        hash ^= table.flip_zobrist[pos_idx as usize];
        f &= f - 1;
    }

    // XOR set key for the placed piece
    hash ^= table.set_zobrist[pos.index() as usize][player.index()];

    // The move also passes the turn to the opponent
    hash ^= table.swap_player_zobrist;

    hash
}

/// Update hash after a pass (player swap only).
pub fn update_hash_on_pass(prev_hash: u64) -> u64 {
    prev_hash ^ zobrist_table().swap_player_zobrist
}

/// Board hash from the two bitboards: two hardware CRC32C instructions
/// rather than an incremental Zobrist update.
///
/// The incremental form had to XOR a table entry for *every flipped disc* of
/// *every generated move*, including the many moves a cut node never
/// searches. Two instructions on the child's bitboards is far less work, and
/// the table only needs the hash to spread — entries carry the position and
/// are compared exactly, so the function itself is free to change.
///
/// `player` is the side to move, so passing swaps the arguments.
#[inline]
pub fn board_hash(player: u64, opponent: u64) -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        // SAFETY: CRC32 is mandatory on every arm64 Apple platform and on
        // every ARMv8.1-A part; the fallback below covers anything else.
        #[target_feature(enable = "crc")]
        unsafe fn crc(player: u64, opponent: u64) -> u64 {
            let c = core::arch::aarch64::__crc32cd(0, player);
            ((c as u64) << 32) | core::arch::aarch64::__crc32cd(c, opponent) as u64
        }
        if std::arch::is_aarch64_feature_detected!("crc") {
            return unsafe { crc(player, opponent) };
        }
    }
    // Portable fallback: a 128-bit multiply-fold mixer.
    let mut h = player.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 29;
    h = h.wrapping_add(opponent.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    h ^= h >> 32;
    h.wrapping_mul(0x94D0_49BB_1331_11EB)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    #[test]
    fn test_table_generation() {
        let table = ZobristTable::generate();
        let mut seen = 0u64;
        for i in 0..64 {
            let key = table.set_zobrist[i][Color::Black as usize];
            if key != 0 {
                seen |= 1;
            }
        }
        assert!(seen != 0, "Table should have non-zero keys");
    }

    #[test]
    fn test_compute_hash_deterministic() {
        let b1 = Board::new();
        let h1 = compute_hash(b1.black, b1.white, b1.player());
        let b2 = Board::new();
        let h2 = compute_hash(b2.black, b2.white, b2.player());
        assert_eq!(h1, h2, "Same board state should produce same hash");
    }

    #[test]
    fn test_hash_changes_on_move() {
        let mut b = Board::new();
        let h1 = compute_hash(b.black, b.white, b.player());

        let movable = b.movable();
        let first = movable.trailing_zeros();
        let pos = Position::from_index(first).unwrap();
        b.make_move_unchecked(pos);

        let h2 = compute_hash(b.black, b.white, b.player());
        assert_ne!(h1, h2, "Hash should change after a move");
    }

    #[test]
    fn test_update_hash_matches_compute() {
        let mut b = Board::new();
        let h1 = compute_hash(b.black, b.white, b.player());

        // Make a move
        let movable = b.movable();
        let first = movable.trailing_zeros();
        let pos = Position::from_index(first).unwrap();
        let pos_bit = pos.to_bit();
        let player_bb = b.player_bb();
        let opponent_bb = b.opponent_bb();
        let flipped = crate::bitboard::flippable(player_bb, opponent_bb, pos_bit);

        // Incremental update
        let h2 = update_hash_on_move(h1, pos, flipped, b.player());

        // Apply the move
        b.make_move_unchecked(pos);

        // Recompute
        let h3 = compute_hash(b.black, b.white, b.player());

        assert_eq!(
            h2, h3,
            "Incremental hash update should match recomputed hash"
        );
    }

    #[test]
    fn test_swap_player_hash() {
        let b = Board::new();
        let h1 = compute_hash(b.black, b.white, b.player());

        // After pass, player changes
        let h2 = update_hash_on_pass(h1);

        assert_ne!(h1, h2, "Hash should change after pass");
    }

    #[test]
    fn test_global_table() {
        let t1 = zobrist_table();
        let t2 = zobrist_table();
        assert!(
            std::ptr::eq(t1, t2),
            "Global table should be the same instance"
        );
    }
}
