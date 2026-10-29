//! Pattern sets: the linear evaluator's KUROOBI patterns (17 shapes, orientations sharing a table) and the NNUE's (32 shapes, one table each).

use crate::color::Color;

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

#[derive(Debug, Clone, Copy)]
pub struct Pattern {
    pub name: &'static str,
    pub size: usize,
    pub masks: &'static [&'static [u8]],
}

impl Pattern {
    pub fn table_size(&self) -> usize {
        3usize.pow(self.size as u32)
    }

    fn mask_index(mask: &[u8], black: u64, white: u64, player: Color) -> usize {
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

    pub fn indices(
        &self,
        black: u64,
        white: u64,
        player: Color,
    ) -> impl Iterator<Item = usize> + '_ {
        self.masks
            .iter()
            .map(move |mask| Self::mask_index(mask, black, white, player))
    }
}

pub const LINEAR_PATTERNS: &[Pattern] = &[
    Pattern {
        name: "Row2+Corners",
        size: 10,
        masks: &[
            &[A2, B2, C2, D2, E2, F2, G2, H2, A1, H1],
            &[A7, B7, C7, D7, E7, F7, G7, H7, A8, H8],
            &[B1, B2, B3, B4, B5, B6, B7, B8, A1, A8],
            &[G1, G2, G3, G4, G5, G6, G7, G8, H1, H8],
        ],
    },
    Pattern {
        name: "Row3+2X",
        size: 10,
        masks: &[
            &[A3, B3, C3, D3, E3, F3, G3, H3, B2, G2],
            &[A6, B6, C6, D6, E6, F6, G6, H6, B7, G7],
            &[C1, C2, C3, C4, C5, C6, C7, C8, B2, B7],
            &[F1, F2, F3, F4, F5, F6, F7, F8, G2, G7],
        ],
    },
    Pattern {
        name: "Row4+Centre",
        size: 10,
        masks: &[
            &[A4, B4, C4, D4, E4, F4, G4, H4, D5, E5],
            &[A5, B5, C5, D5, E5, F5, G5, H5, D4, E4],
            &[D1, D2, D3, D4, D5, D6, D7, D8, E4, E5],
            &[E1, E2, E3, E4, E5, E6, E7, E8, D4, D5],
        ],
    },
    Pattern {
        name: "Corner3x3+D4",
        size: 10,
        masks: &[
            &[A1, B1, A2, B2, C1, A3, C2, B3, C3, D4],
            &[H1, G1, H2, G2, F1, H3, F2, G3, F3, E4],
            &[A8, B8, A7, B7, C8, A6, C7, B6, C6, D5],
            &[H8, G8, H7, G7, F8, H6, F7, G6, F6, E5],
        ],
    },
    Pattern {
        name: "Diag4+5",
        size: 9,
        masks: &[
            &[A4, B3, C2, D1, A5, B4, C3, D2, E1],
            &[H4, G3, F2, E1, H5, G4, F3, E2, D1],
            &[A5, B6, C7, D8, A4, B5, C6, D7, E8],
            &[H5, G6, F7, E8, H4, G5, F6, E7, D8],
        ],
    },
    Pattern {
        name: "Diagonal6",
        size: 9,
        masks: &[
            &[B1, C1, D2, E3, G2, F4, G5, H6, H7],
            &[G1, F1, E2, D3, B2, C4, B5, A6, A7],
            &[B8, C8, D7, E6, G7, F5, G4, H3, H2],
            &[G8, F8, E7, D6, B7, C5, B4, A3, A2],
        ],
    },
    Pattern {
        name: "Diagonal7",
        size: 9,
        masks: &[
            &[A1, B1, C2, D3, E4, F5, G6, H7, H8],
            &[H1, G1, F2, E3, D4, C5, B6, A7, A8],
            &[A8, B8, C7, D6, E5, F4, G3, H2, H1],
            &[H8, G8, F7, E6, D5, C4, B3, A2, A1],
        ],
    },
    Pattern {
        name: "Diagonal8+2C",
        size: 10,
        masks: &[
            &[A1, B2, C3, D4, E5, F6, G7, H8, A2, B1],
            &[H1, G2, F3, E4, D5, C6, B7, A8, H2, G1],
            &[A8, B7, C6, D5, E4, F3, G2, H1, A7, B8],
            &[H8, G7, F6, E5, D4, C3, B2, A1, H7, G8],
        ],
    },
    Pattern {
        name: "Edge+2X",
        size: 10,
        masks: &[
            &[B2, A1, B1, C1, D1, E1, F1, G1, H1, G2],
            &[B7, A8, B8, C8, D8, E8, F8, G8, H8, G7],
            &[B2, A1, A2, A3, A4, A5, A6, A7, A8, B7],
            &[G2, H1, H2, H3, H4, H5, H6, H7, H8, G7],
        ],
    },
    Pattern {
        name: "Triangle",
        size: 10,
        masks: &[
            &[A1, A2, B1, A3, B2, C1, A4, B3, C2, D1],
            &[H1, H2, G1, H3, G2, F1, H4, G3, F2, E1],
            &[A8, A7, B8, A6, B7, C8, A5, B6, C7, D8],
            &[H8, H7, G8, H6, G7, F8, H5, G6, F7, E8],
        ],
    },
    Pattern {
        name: "Corner+Block",
        size: 10,
        masks: &[
            &[A1, C1, D1, C2, D2, E2, F2, E1, F1, H1],
            &[A8, C8, D8, C7, D7, E7, F7, E8, F8, H8],
            &[A1, A3, A4, B3, B4, B5, B6, A5, A6, A8],
            &[H1, H3, H4, G3, G4, G5, G6, H5, H6, H8],
        ],
    },
    Pattern {
        name: "Cross",
        size: 10,
        masks: &[
            &[A1, B2, C3, D4, B1, C2, D3, A2, B3, C4],
            &[H1, G2, F3, E4, G1, F2, E3, H2, G3, F4],
            &[A8, B7, C6, D5, B8, C7, D6, A7, B6, C5],
            &[H8, G7, F6, E5, G8, F7, E6, H7, G6, F5],
        ],
    },
    Pattern {
        name: "Edge+Y",
        size: 10,
        masks: &[
            &[C2, A1, B1, C1, D1, E1, F1, G1, H1, F2],
            &[C7, A8, B8, C8, D8, E8, F8, G8, H8, F7],
            &[B3, A1, A2, A3, A4, A5, A6, A7, A8, B6],
            &[G3, H1, H2, H3, H4, H5, H6, H7, H8, G6],
        ],
    },
    Pattern {
        name: "NarrowTriangle",
        size: 10,
        masks: &[
            &[A1, B1, C1, D1, E1, A2, B2, A3, A4, A5],
            &[H1, G1, F1, E1, D1, H2, G2, H3, H4, H5],
            &[A8, B8, C8, D8, E8, A7, B7, A6, A5, A4],
            &[H8, G8, F8, E8, D8, H7, G7, H6, H5, H4],
        ],
    },
    Pattern {
        name: "Fish",
        size: 10,
        masks: &[
            &[A1, B1, A2, B2, C2, D2, B3, C3, B4, D4],
            &[H1, G1, H2, G2, F2, E2, G3, F3, G4, E4],
            &[A8, B8, A7, B7, C7, D7, B6, C6, B5, D5],
            &[H8, G8, H7, G7, F7, E7, G6, F6, G5, E5],
        ],
    },
    Pattern {
        name: "Anvil",
        size: 10,
        masks: &[
            &[C1, D1, E1, F1, D2, E2, C3, D3, E3, F3],
            &[C8, D8, E8, F8, D7, E7, C6, D6, E6, F6],
            &[A3, A4, A5, A6, B4, B5, C3, C4, C5, C6],
            &[H3, H4, H5, H6, G4, G5, F3, F4, F5, F6],
        ],
    },
    Pattern {
        name: "Corner2x5",
        size: 10,
        masks: &[
            &[A1, B1, C1, D1, E1, A2, B2, C2, D2, E2],
            &[H1, G1, F1, E1, D1, H2, G2, F2, E2, D2],
            &[A8, B8, C8, D8, E8, A7, B7, C7, D7, E7],
            &[H8, G8, F8, E8, D8, H7, G7, F7, E7, D7],
            &[A1, A2, A3, A4, A5, B1, B2, B3, B4, B5],
            &[H1, H2, H3, H4, H5, G1, G2, G3, G4, G5],
            &[A8, A7, A6, A5, A4, B8, B7, B6, B5, B4],
            &[H8, H7, H6, H5, H4, G8, G7, G6, G5, G4],
        ],
    },
];

pub const NNUE_PATTERNS: &[Pattern] = &[
    Pattern {
        name: "InnerTop",
        size: 8,
        masks: &[&[C2, D2, E2, F2, C3, D3, E3, F3]],
    },
    Pattern {
        name: "InnerBottom",
        size: 8,
        masks: &[&[C7, D7, E7, F7, C6, D6, E6, F6]],
    },
    Pattern {
        name: "InnerLeft",
        size: 8,
        masks: &[&[B3, B4, B5, B6, C3, C4, C5, C6]],
    },
    Pattern {
        name: "InnerRight",
        size: 8,
        masks: &[&[G3, G4, G5, G6, F3, F4, F5, F6]],
    },
    Pattern {
        name: "DiagA1H8",
        size: 8,
        masks: &[&[A1, B2, C3, D4, E5, F6, G7, H8]],
    },
    Pattern {
        name: "DiagH1A8",
        size: 8,
        masks: &[&[H1, G2, F3, E4, D5, C6, B7, A8]],
    },
    Pattern {
        name: "Center2x4H",
        size: 8,
        masks: &[&[C4, D4, E4, F4, C5, D5, E5, F5]],
    },
    Pattern {
        name: "Center2x4V",
        size: 8,
        masks: &[&[D3, E3, D4, E4, D5, E5, D6, E6]],
    },
    Pattern {
        name: "Row1",
        size: 8,
        masks: &[&[A1, B1, C1, D1, E1, F1, G1, H1]],
    },
    Pattern {
        name: "Row8",
        size: 8,
        masks: &[&[A8, B8, C8, D8, E8, F8, G8, H8]],
    },
    Pattern {
        name: "ColA",
        size: 8,
        masks: &[&[A1, A2, A3, A4, A5, A6, A7, A8]],
    },
    Pattern {
        name: "ColH",
        size: 8,
        masks: &[&[H1, H2, H3, H4, H5, H6, H7, H8]],
    },
    Pattern {
        name: "EdgeTop",
        size: 8,
        masks: &[&[B1, C1, D1, E1, B2, C2, D2, E2]],
    },
    Pattern {
        name: "EdgeTopMirror",
        size: 8,
        masks: &[&[G1, F1, E1, D1, G2, F2, E2, D2]],
    },
    Pattern {
        name: "EdgeBottom",
        size: 8,
        masks: &[&[B8, C8, D8, E8, B7, C7, D7, E7]],
    },
    Pattern {
        name: "EdgeBottomMirror",
        size: 8,
        masks: &[&[G8, F8, E8, D8, G7, F7, E7, D7]],
    },
    Pattern {
        name: "EdgeLeft",
        size: 8,
        masks: &[&[A2, A3, A4, A5, B2, B3, B4, B5]],
    },
    Pattern {
        name: "EdgeLeftMirror",
        size: 8,
        masks: &[&[A7, A6, A5, A4, B7, B6, B5, B4]],
    },
    Pattern {
        name: "EdgeRight",
        size: 8,
        masks: &[&[H2, H3, H4, H5, G2, G3, G4, G5]],
    },
    Pattern {
        name: "EdgeRightMirror",
        size: 8,
        masks: &[&[H7, H6, H5, H4, G7, G6, G5, G4]],
    },
    Pattern {
        name: "CornerA1",
        size: 9,
        masks: &[&[A1, B1, C1, A2, B2, C2, A3, B3, C3]],
    },
    Pattern {
        name: "CornerH1",
        size: 9,
        masks: &[&[H1, G1, F1, H2, G2, F2, H3, G3, F3]],
    },
    Pattern {
        name: "CornerA8",
        size: 9,
        masks: &[&[A8, B8, C8, A7, B7, C7, A6, B6, C6]],
    },
    Pattern {
        name: "CornerH8",
        size: 9,
        masks: &[&[H8, G8, F8, H7, G7, F7, H6, G6, F6]],
    },
    Pattern {
        name: "Center3x3NW",
        size: 9,
        masks: &[&[B2, C2, D2, B3, C3, D3, B4, C4, D4]],
    },
    Pattern {
        name: "Center3x3NE",
        size: 9,
        masks: &[&[G2, F2, E2, G3, F3, E3, G4, F4, E4]],
    },
    Pattern {
        name: "Center3x3SW",
        size: 9,
        masks: &[&[B7, C7, D7, B6, C6, D6, B5, C5, D5]],
    },
    Pattern {
        name: "Center3x3SE",
        size: 9,
        masks: &[&[G7, F7, E7, G6, F6, E6, G5, F5, E5]],
    },
    Pattern {
        name: "DiagB1H7",
        size: 7,
        masks: &[&[B1, C2, D3, E4, F5, G6, H7]],
    },
    Pattern {
        name: "DiagA2G8",
        size: 7,
        masks: &[&[A2, B3, C4, D5, E6, F7, G8]],
    },
    Pattern {
        name: "DiagG1A7",
        size: 7,
        masks: &[&[G1, F2, E3, D4, C5, B6, A7]],
    },
    Pattern {
        name: "DiagH2B8",
        size: 7,
        masks: &[&[H2, G3, F4, E5, D6, C7, B8]],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    #[test]
    fn test_pattern_counts() {
        assert_eq!(LINEAR_PATTERNS.len(), 17, "the linear set has 17 patterns");
        assert_eq!(NNUE_PATTERNS.len(), 32, "the unshared set has 32 patterns");
    }

    #[test]
    fn test_mask_sizes_match_declared_size() {
        for p in LINEAR_PATTERNS.iter().chain(NNUE_PATTERNS) {
            assert!(!p.masks.is_empty(), "{}: needs at least one mask", p.name);
            for (i, mask) in p.masks.iter().enumerate() {
                assert_eq!(
                    mask.len(),
                    p.size,
                    "{} mask {} has {} squares, declared size {}",
                    p.name,
                    i,
                    mask.len(),
                    p.size
                );
            }
        }
    }

    #[test]
    fn test_no_duplicate_squares_within_mask() {
        for p in LINEAR_PATTERNS.iter().chain(NNUE_PATTERNS) {
            for (i, mask) in p.masks.iter().enumerate() {
                let mut seen = 0u64;
                for &s in mask.iter() {
                    assert!(s < 64, "{} mask {}: square {} out of range", p.name, i, s);
                    let bit = 1u64 << s;
                    assert_eq!(
                        seen & bit,
                        0,
                        "{} mask {}: duplicate square {}",
                        p.name,
                        i,
                        s
                    );
                    seen |= bit;
                }
            }
        }
    }

    #[test]
    fn test_empty_board_index_is_all_twos() {
        for p in LINEAR_PATTERNS.iter().chain(NNUE_PATTERNS) {
            for mask in p.masks.iter() {
                let idx = Pattern::mask_index(mask, 0, 0, Color::Black);
                assert_eq!(idx, p.table_size() - 1, "{}: empty-board index", p.name);
            }
        }
    }

    #[test]
    fn test_mask_index_respects_order_and_colors() {
        let black = 1u64 << sq::A1;
        let white = 1u64 << sq::B1;
        let mask: &[u8] = &[sq::A1, sq::B1];
        assert_eq!(Pattern::mask_index(mask, black, white, Color::Black), 1);
        assert_eq!(Pattern::mask_index(mask, black, white, Color::White), 3);
    }

    #[test]
    fn test_orientation_symmetry_on_initial_board() {
        let b = Board::new();
        let triangle = &LINEAR_PATTERNS[9];
        assert_eq!(triangle.name, "Triangle");
        let indices: Vec<usize> = triangle.indices(b.black, b.white, b.player()).collect();
        assert!(indices.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(indices[0], triangle.table_size() - 1);
    }

    #[test]
    fn test_row4_centre_sees_initial_pieces() {
        let b = Board::new();
        let row4 = &LINEAR_PATTERNS[2];
        assert_eq!(row4.name, "Row4+Centre");
        for idx in row4.indices(b.black, b.white, b.player()) {
            assert_ne!(
                idx,
                row4.table_size() - 1,
                "Row4+Centre must see center discs"
            );
        }
    }

    #[test]
    fn test_player_perspective_flips_digits() {
        let b = Board::new();
        let row4 = &LINEAR_PATTERNS[2];
        let black_view: Vec<usize> = row4.indices(b.black, b.white, Color::Black).collect();
        let white_view: Vec<usize> = row4.indices(b.black, b.white, Color::White).collect();
        assert_ne!(
            black_view, white_view,
            "swapping perspective must swap player/opponent digits"
        );
    }
}
