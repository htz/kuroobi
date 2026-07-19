//! Pattern definitions for AI evaluation: the Egaroucid pattern set (16)
//! and the Edax pattern set (12).
//!
//! Each pattern has several masks (board orientations). A mask is an ordered
//! list of squares; evaluating a mask yields a base-3 index (digit per square:
//! 0=player, 1=opponent, 2=empty, consumed in mask order) into a weight table
//! of size 3^size.

use crate::color::Color;

/// Square constants in this crate's file-major indexing (file*8 + rank).
#[rustfmt::skip]
pub mod sq {
    pub const A1: u8 = 0;  pub const A2: u8 = 1;  pub const A3: u8 = 2;  pub const A4: u8 = 3;
    pub const A5: u8 = 4;  pub const A6: u8 = 5;  pub const A7: u8 = 6;  pub const A8: u8 = 7;
    pub const B1: u8 = 8;  pub const B2: u8 = 9;  pub const B3: u8 = 10; pub const B4: u8 = 11;
    pub const B5: u8 = 12; pub const B6: u8 = 13; pub const B7: u8 = 14; pub const B8: u8 = 15;
    pub const C1: u8 = 16; pub const C2: u8 = 17; pub const C3: u8 = 18; pub const C4: u8 = 19;
    pub const C5: u8 = 20; pub const C6: u8 = 21; pub const C7: u8 = 22; pub const C8: u8 = 23;
    pub const D1: u8 = 24; pub const D2: u8 = 25; pub const D3: u8 = 26; pub const D4: u8 = 27;
    pub const D5: u8 = 28; pub const D6: u8 = 29; pub const D7: u8 = 30; pub const D8: u8 = 31;
    pub const E1: u8 = 32; pub const E2: u8 = 33; pub const E3: u8 = 34; pub const E4: u8 = 35;
    pub const E5: u8 = 36; pub const E6: u8 = 37; pub const E7: u8 = 38; pub const E8: u8 = 39;
    pub const F1: u8 = 40; pub const F2: u8 = 41; pub const F3: u8 = 42; pub const F4: u8 = 43;
    pub const F5: u8 = 44; pub const F6: u8 = 45; pub const F7: u8 = 46; pub const F8: u8 = 47;
    pub const G1: u8 = 48; pub const G2: u8 = 49; pub const G3: u8 = 50; pub const G4: u8 = 51;
    pub const G5: u8 = 52; pub const G6: u8 = 53; pub const G7: u8 = 54; pub const G8: u8 = 55;
    pub const H1: u8 = 56; pub const H2: u8 = 57; pub const H3: u8 = 58; pub const H4: u8 = 59;
    pub const H5: u8 = 60; pub const H6: u8 = 61; pub const H7: u8 = 62; pub const H8: u8 = 63;
}

use sq::*;

/// A single evaluation pattern: `size` squares per mask, several masks
/// (board orientations). Fully static — no allocation.
#[derive(Debug, Clone, Copy)]
pub struct Pattern {
    pub name: &'static str,
    pub size: usize,
    pub masks: &'static [&'static [u8]],
}

impl Pattern {
    /// Number of weight entries this pattern needs (3^size).
    pub fn table_size(&self) -> usize {
        3usize.pow(self.size as u32)
    }

    /// Ternary index for one mask: squares consumed in mask order,
    /// digit 0=player, 1=opponent, 2=empty.
    pub fn mask_index(mask: &[u8], black: u64, white: u64, player: Color) -> usize {
        let player_bb = [black, white][player.index()];
        let opponent_bb = [black, white][player.opponent().index()];

        let mut index = 0usize;
        for &s in mask {
            let bit = 1u64 << s;
            let digit = if player_bb & bit != 0 {
                0
            } else if opponent_bb & bit != 0 {
                1
            } else {
                2
            };
            index = index * 3 + digit;
        }
        index
    }

    /// Indices for all orientations of this pattern.
    pub fn indices(&self, black: u64, white: u64, player: Color) -> impl Iterator<Item = usize> + '_ {
        self.masks
            .iter()
            .map(move |mask| Self::mask_index(mask, black, white, player))
    }
}

// ---------------------------------------------------------------------------
// Egaroucid pattern set (16)
// ---------------------------------------------------------------------------

pub const EGAROUCID_PATTERNS: &[Pattern] = &[
    Pattern { name: "Line2", size: 8, masks: &[
        &[A2, B2, C2, D2, E2, F2, G2, H2],
        &[A7, B7, C7, D7, E7, F7, G7, H7],
        &[B1, B2, B3, B4, B5, B6, B7, B8],
        &[G1, G2, G3, G4, G5, G6, G7, G8],
    ]},
    Pattern { name: "Line3", size: 8, masks: &[
        &[A3, B3, C3, D3, E3, F3, G3, H3],
        &[A6, B6, C6, D6, E6, F6, G6, H6],
        &[C1, C2, C3, C4, C5, C6, C7, C8],
        &[F1, F2, F3, F4, F5, F6, F7, F8],
    ]},
    Pattern { name: "Line4", size: 8, masks: &[
        &[A4, B4, C4, D4, E4, F4, G4, H4],
        &[A5, B5, C5, D5, E5, F5, G5, H5],
        &[D1, D2, D3, D4, D5, D6, D7, D8],
        &[E1, E2, E3, E4, E5, E6, E7, E8],
    ]},
    Pattern { name: "Corner3x3", size: 9, masks: &[
        &[A1, B1, A2, B2, C1, A3, C2, B3, C3],
        &[H1, G1, H2, G2, F1, H3, F2, G3, F3],
        &[A8, A7, B8, B7, A6, C8, B6, C7, C6],
        &[H8, H7, G8, G7, H6, F8, G6, F7, F6],
    ]},
    Pattern { name: "Diagonal5", size: 7, masks: &[
        &[B2, D1, E2, F3, G4, H5, G7],
        &[B2, A4, B5, C6, D7, E8, G7],
        &[G2, E1, D2, C3, B4, A5, B7],
        &[G2, H4, G5, F6, E7, D8, B7],
    ]},
    Pattern { name: "Diagonal6", size: 9, masks: &[
        &[B1, C1, D2, E3, G2, F4, G5, H6, H7],
        &[A2, A3, B4, C5, B7, D6, E7, F8, G8],
        &[G1, F1, E2, D3, B2, C4, B5, A6, A7],
        &[H2, H3, G4, F5, G7, E6, D7, C8, B8],
    ]},
    Pattern { name: "Diagonal7", size: 9, masks: &[
        &[A1, B1, C2, D3, E4, F5, G6, H7, H8],
        &[H1, H2, G3, F4, E5, D6, C7, B8, A8],
        &[A1, A2, B3, C4, D5, E6, F7, G8, H8],
        &[H1, G1, F2, E3, D4, C5, B6, A7, A8],
    ]},
    Pattern { name: "Diagonal8+2C", size: 10, masks: &[
        &[A1, B2, C3, D4, E5, F6, G7, H8, A2, B1],
        &[A8, B7, C6, D5, E4, F3, G2, H1, B8, A7],
        &[H8, G7, F6, E5, D4, C3, B2, A1, H7, G8],
        &[H1, G2, F3, E4, D5, C6, B7, A8, G1, H2],
    ]},
    Pattern { name: "Edge+2x", size: 10, masks: &[
        &[B2, A1, B1, C1, D1, E1, F1, G1, H1, G2],
        &[B7, A8, B8, C8, D8, E8, F8, G8, H8, G7],
        &[B2, A1, A2, A3, A4, A5, A6, A7, A8, B7],
        &[G2, H1, H2, H3, H4, H5, H6, H7, H8, G7],
    ]},
    Pattern { name: "Triangle", size: 10, masks: &[
        &[A1, A2, B1, A3, B2, C1, A4, B3, C2, D1],
        &[H1, G1, H2, F1, G2, H3, E1, F2, G3, H4],
        &[H8, H7, G8, H6, G7, F8, H5, G6, F7, E8],
        &[A8, B8, A7, C8, B7, A6, D8, C7, B6, A5],
    ]},
    Pattern { name: "Corner + Block", size: 10, masks: &[
        &[A1, C1, D1, C2, D2, E2, F2, E1, F1, H1],
        &[H1, H3, H4, G3, G4, G5, G6, H5, H6, H8],
        &[H8, F8, E8, F7, E7, D7, C7, D8, C8, A8],
        &[A8, A6, A5, B6, B5, B4, B3, A4, A3, A1],
    ]},
    Pattern { name: "Cross", size: 10, masks: &[
        &[A1, B2, C3, D4, B1, C2, D3, A2, B3, C4],
        &[H1, G2, F3, E4, G1, F2, E3, H2, G3, F4],
        &[A8, B7, C6, D5, B8, C7, D6, A7, B6, C5],
        &[H8, G7, F6, E5, G8, F7, E6, H7, G6, F5],
    ]},
    Pattern { name: "Edge+Y", size: 10, masks: &[
        &[C2, A1, B1, C1, D1, E1, F1, G1, H1, F2],
        &[C7, A8, B8, C8, D8, E8, F8, G8, H8, F7],
        &[B3, A1, A2, A3, A4, A5, A6, A7, A8, B6],
        &[G3, H1, H2, H3, H4, H5, H6, H7, H8, G6],
    ]},
    Pattern { name: "Narrow Triangle", size: 10, masks: &[
        &[A1, B1, C1, D1, E1, A2, B2, A3, A4, A5],
        &[H1, G1, F1, E1, D1, H2, G2, H3, H4, H5],
        &[A8, B8, C8, D8, E8, A7, B7, A6, A5, A4],
        &[H8, G8, F8, E8, D8, H7, G7, H6, H5, H4],
    ]},
    Pattern { name: "Fish", size: 10, masks: &[
        &[A1, B1, A2, B2, C2, D2, B3, C3, B4, D4],
        &[H1, G1, H2, G2, F2, E2, G3, F3, G4, E4],
        &[A8, B8, A7, B7, C7, D7, B6, C6, B5, D5],
        &[H8, G8, H7, G7, F7, E7, G6, F6, G5, E5],
    ]},
    Pattern { name: "Anvil", size: 10, masks: &[
        &[C6, D6, D7, D8, C8, F8, E8, E7, E6, F6],
        &[C3, C4, B4, A4, A3, A6, A5, B5, C5, C6],
        &[F3, E3, E2, E1, F1, C1, D1, D2, D3, C3],
        &[F6, F5, G5, H5, H6, H3, H4, G4, F4, F3],
    ]},
];

// ---------------------------------------------------------------------------
// Edax pattern set (12)
// ---------------------------------------------------------------------------

pub const EDAX_PATTERNS: &[Pattern] = &[
    Pattern { name: "Corner3x3", size: 9, masks: &[
        &[A1, B1, A2, B2, C1, A3, C2, B3, C3],
        &[H1, G1, H2, G2, F1, H3, F2, G3, F3],
        &[A8, A7, B8, B7, A6, C8, B6, C7, C6],
        &[H8, H7, G8, G7, H6, F8, G6, F7, F6],
    ]},
    Pattern { name: "Angle+X", size: 10, masks: &[
        &[A5, A4, A3, A2, A1, B2, B1, C1, D1, E1],
        &[H5, H4, H3, H2, H1, G2, G1, F1, E1, D1],
        &[A4, A5, A6, A7, A8, B7, B8, C8, D8, E8],
        &[H4, H5, H6, H7, H8, G7, G8, F8, E8, D8],
    ]},
    Pattern { name: "Edge+2x", size: 10, masks: &[
        &[B2, A1, B1, C1, D1, E1, F1, G1, H1, G2],
        &[B7, A8, B8, C8, D8, E8, F8, G8, H8, G7],
        &[B2, A1, A2, A3, A4, A5, A6, A7, A8, B7],
        &[G2, H1, H2, H3, H4, H5, H6, H7, H8, G7],
    ]},
    Pattern { name: "Corner+Block", size: 10, masks: &[
        &[A1, C1, D1, C2, D2, E2, F2, E1, F1, H1],
        &[A8, C8, D8, C7, D7, E7, F7, E8, F8, H8],
        &[A1, A3, A4, B3, B4, B5, B6, A5, A6, A8],
        &[H1, H3, H4, G3, G4, G5, G6, H5, H6, H8],
    ]},
    Pattern { name: "Line2", size: 8, masks: &[
        &[A2, B2, C2, D2, E2, F2, G2, H2],
        &[A7, B7, C7, D7, E7, F7, G7, H7],
        &[B1, B2, B3, B4, B5, B6, B7, B8],
        &[G1, G2, G3, G4, G5, G6, G7, G8],
    ]},
    Pattern { name: "Line3", size: 8, masks: &[
        &[A3, B3, C3, D3, E3, F3, G3, H3],
        &[A6, B6, C6, D6, E6, F6, G6, H6],
        &[C1, C2, C3, C4, C5, C6, C7, C8],
        &[F1, F2, F3, F4, F5, F6, F7, F8],
    ]},
    Pattern { name: "Line4", size: 8, masks: &[
        &[A4, B4, C4, D4, E4, F4, G4, H4],
        &[A5, B5, C5, D5, E5, F5, G5, H5],
        &[D1, D2, D3, D4, D5, D6, D7, D8],
        &[E1, E2, E3, E4, E5, E6, E7, E8],
    ]},
    Pattern { name: "Diagonal8", size: 8, masks: &[
        &[A1, B2, C3, D4, E5, F6, G7, H8],
        &[A8, B7, C6, D5, E4, F3, G2, H1],
    ]},
    Pattern { name: "Diagonal7", size: 7, masks: &[
        &[B1, C2, D3, E4, F5, G6, H7],
        &[H2, G3, F4, E5, D6, C7, B8],
        &[A2, B3, C4, D5, E6, F7, G8],
        &[G1, F2, E3, D4, C5, B6, A7],
    ]},
    Pattern { name: "Diagonal6", size: 6, masks: &[
        &[C1, D2, E3, F4, G5, H6],
        &[A3, B4, C5, D6, E7, F8],
        &[F1, E2, D3, C4, B5, A6],
        &[H3, G4, F5, E6, D7, C8],
    ]},
    Pattern { name: "Diagonal5", size: 5, masks: &[
        &[D1, E2, F3, G4, H5],
        &[A4, B5, C6, D7, E8],
        &[E1, D2, C3, B4, A5],
        &[H4, G5, F6, E7, D8],
    ]},
    Pattern { name: "Diagonal4", size: 4, masks: &[
        &[D1, C2, B3, A4],
        &[A5, B6, C7, D8],
        &[E1, F2, G3, H4],
        &[H5, G6, F7, E8],
    ]},
];

// ---------------------------------------------------------------------------
// Egaroucid-plus (18): the full Egaroucid set extended with the two Edax
// patterns it lacks. Motivation (weight statistics of an evaluator trained
// on the Egaroucid set): opening-stage tables of 10-cell patterns are <6%
// visited on 25M teacher positions, so small/extra shapes give the early
// stages dense, fully-trained features while stage-wise weights let later
// stages keep relying on the big patterns.
// ---------------------------------------------------------------------------

/// Angle+X (corner region + X square), from the Edax pattern set.
const ANGLE_X: Pattern = Pattern { name: "Angle+X", size: 10, masks: &[
    &[A5, A4, A3, A2, A1, B2, B1, C1, D1, E1],
    &[H5, H4, H3, H2, H1, G2, G1, F1, E1, D1],
    &[A4, A5, A6, A7, A8, B7, B8, C8, D8, E8],
    &[H4, H5, H6, H7, H8, G7, G8, F8, E8, D8],
]};

/// Diagonal4 (3^4 = 81 cells: fully trainable even in the opening), from
/// the Edax pattern set.
const DIAGONAL4: Pattern = Pattern { name: "Diagonal4", size: 4, masks: &[
    &[D1, C2, B3, A4],
    &[A5, B6, C7, D8],
    &[E1, F2, G3, H4],
    &[H5, G6, F7, E8],
]};

pub const EGAROUCID_PLUS_PATTERNS: &[Pattern] = &[
    EGAROUCID_PATTERNS[0], EGAROUCID_PATTERNS[1], EGAROUCID_PATTERNS[2],
    EGAROUCID_PATTERNS[3], EGAROUCID_PATTERNS[4], EGAROUCID_PATTERNS[5],
    EGAROUCID_PATTERNS[6], EGAROUCID_PATTERNS[7], EGAROUCID_PATTERNS[8],
    EGAROUCID_PATTERNS[9], EGAROUCID_PATTERNS[10], EGAROUCID_PATTERNS[11],
    EGAROUCID_PATTERNS[12], EGAROUCID_PATTERNS[13], EGAROUCID_PATTERNS[14],
    EGAROUCID_PATTERNS[15],
    ANGLE_X,
    DIAGONAL4,
];

/// Convenience holder pairing both pattern libraries.
#[derive(Debug, Clone, Copy)]
pub struct PatternSet {
    pub egaroucid: &'static [Pattern],
    pub edax: &'static [Pattern],
}

impl PatternSet {
    pub fn all() -> Self {
        PatternSet {
            egaroucid: EGAROUCID_PATTERNS,
            edax: EDAX_PATTERNS,
        }
    }
}

/// Weight tables for one pattern library: `weights[pattern][ternary_index]`.
#[derive(Debug, Clone)]
pub struct PatternWeights {
    patterns: &'static [Pattern],
    weights: Vec<Vec<i32>>,
}

impl PatternWeights {
    /// Zero-initialized weights sized 3^size per pattern.
    pub fn zeros(patterns: &'static [Pattern]) -> Self {
        let weights = patterns.iter().map(|p| vec![0i32; p.table_size()]).collect();
        PatternWeights { patterns, weights }
    }

    pub fn patterns(&self) -> &'static [Pattern] {
        self.patterns
    }

    /// Set a single weight entry.
    pub fn set(&mut self, pattern_idx: usize, index: usize, value: i32) {
        self.weights[pattern_idx][index] = value;
    }

    pub fn get(&self, pattern_idx: usize, index: usize) -> i32 {
        self.weights[pattern_idx][index]
    }

    /// Evaluate a position: sum of weights over every pattern orientation.
    pub fn evaluate(&self, black: u64, white: u64, player: Color) -> i32 {
        let mut score = 0i32;
        for (p, table) in self.patterns.iter().zip(&self.weights) {
            for idx in p.indices(black, white, player) {
                score += table[idx];
            }
        }
        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    #[test]
    fn test_pattern_counts() {
        assert_eq!(EGAROUCID_PATTERNS.len(), 16, "Egaroucid has 16 patterns");
        assert_eq!(EDAX_PATTERNS.len(), 12, "Edax has 12 patterns");
    }

    #[test]
    fn test_mask_sizes_match_declared_size() {
        for p in EGAROUCID_PATTERNS.iter().chain(EDAX_PATTERNS) {
            assert!(!p.masks.is_empty(), "{}: needs at least one mask", p.name);
            for (i, mask) in p.masks.iter().enumerate() {
                assert_eq!(
                    mask.len(),
                    p.size,
                    "{} mask {} has {} squares, declared size {}",
                    p.name, i, mask.len(), p.size
                );
            }
        }
    }

    #[test]
    fn test_no_duplicate_squares_within_mask() {
        for p in EGAROUCID_PATTERNS.iter().chain(EDAX_PATTERNS) {
            for (i, mask) in p.masks.iter().enumerate() {
                let mut seen = 0u64;
                for &s in mask.iter() {
                    assert!(s < 64, "{} mask {}: square {} out of range", p.name, i, s);
                    let bit = 1u64 << s;
                    assert_eq!(seen & bit, 0, "{} mask {}: duplicate square {}", p.name, i, s);
                    seen |= bit;
                }
            }
        }
    }

    #[test]
    fn test_empty_board_index_is_all_twos() {
        // Every square empty -> every ternary digit is 2 -> index = 3^size - 1
        for p in EGAROUCID_PATTERNS.iter().chain(EDAX_PATTERNS) {
            for mask in p.masks.iter() {
                let idx = Pattern::mask_index(mask, 0, 0, Color::Black);
                assert_eq!(idx, p.table_size() - 1, "{}: empty-board index", p.name);
            }
        }
    }

    #[test]
    fn test_mask_index_respects_order_and_colors() {
        // Mask [A1, B1]: A1=player -> digit 0, B1=opponent -> digit 1.
        // Index = 0*3 + 1 = 1 for Black; swapped for White = 1*3 + 0 = 3.
        let black = 1u64 << sq::A1;
        let white = 1u64 << sq::B1;
        let mask: &[u8] = &[sq::A1, sq::B1];
        assert_eq!(Pattern::mask_index(mask, black, white, Color::Black), 1);
        assert_eq!(Pattern::mask_index(mask, black, white, Color::White), 3);
    }

    #[test]
    fn test_orientation_symmetry_on_initial_board() {
        // The initial position is 180-degree rotationally symmetric with
        // colors swapped. Corner3x3 doesn't touch the center, so all four
        // orientations must give the identical (all-empty) index.
        let b = Board::new();
        let corner = &EGAROUCID_PATTERNS[3];
        assert_eq!(corner.name, "Corner3x3");
        let indices: Vec<usize> = corner.indices(b.black, b.white, b.player()).collect();
        assert!(indices.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(indices[0], corner.table_size() - 1);
    }

    #[test]
    fn test_line4_sees_initial_pieces() {
        // Line4's rank-4/rank-5 and file-D/file-E masks each cross exactly
        // two initial center discs, so their indices must differ from empty.
        let b = Board::new();
        let line4 = &EGAROUCID_PATTERNS[2];
        assert_eq!(line4.name, "Line4");
        for idx in line4.indices(b.black, b.white, b.player()) {
            assert_ne!(idx, line4.table_size() - 1, "Line4 must see center discs");
        }
    }

    #[test]
    fn test_weights_zeros_and_evaluate() {
        let mut w = PatternWeights::zeros(EGAROUCID_PATTERNS);
        for (i, p) in EGAROUCID_PATTERNS.iter().enumerate() {
            assert_eq!(w.weights[i].len(), p.table_size());
        }

        let b = Board::new();
        assert_eq!(w.evaluate(b.black, b.white, b.player()), 0, "all-zero weights");

        // Set the weight for Line2's current (all-empty) index; Line2 has 4
        // orientations, all all-empty on the initial board -> score = 4 * 7.
        let line2 = &EGAROUCID_PATTERNS[0];
        w.set(0, line2.table_size() - 1, 7);
        assert_eq!(w.evaluate(b.black, b.white, b.player()), 28);
    }

    #[test]
    fn test_player_perspective_flips_digits() {
        let b = Board::new();
        let line4 = &EGAROUCID_PATTERNS[2];
        let black_view: Vec<usize> = line4.indices(b.black, b.white, Color::Black).collect();
        let white_view: Vec<usize> = line4.indices(b.black, b.white, Color::White).collect();
        assert_ne!(
            black_view, white_view,
            "swapping perspective must swap player/opponent digits"
        );
    }
}
