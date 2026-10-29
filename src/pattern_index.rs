//! Incremental (differential) pattern-index maintenance.

use crate::color::Color;
use crate::pattern::Pattern;
use crate::position::Position;

pub const MAX_MASKS: usize = 80;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PatternIndices {
    idx: [u16; MAX_MASKS],
}

impl PatternIndices {
    pub const ZERO: PatternIndices = PatternIndices {
        idx: [0u16; MAX_MASKS],
    };

    #[inline]
    pub fn raw(&self) -> &[u16; MAX_MASKS] {
        &self.idx
    }
}

pub struct PatternIndexer {
    patterns: &'static [Pattern],
    n_masks: usize,
    mask_pattern: [u8; MAX_MASKS],
    dense_pow3: Vec<[u16; MAX_MASKS]>,
    swap_tables: [Vec<u16>; 11],
}

#[inline]
fn absolute_digit(black: u64, white: u64, sq: u8) -> u16 {
    let bit = 1u64 << sq;
    if black & bit != 0 {
        0
    } else if white & bit != 0 {
        1
    } else {
        2
    }
}

impl PatternIndexer {
    pub fn new(patterns: &'static [Pattern]) -> PatternIndexer {
        let n_masks: usize = patterns.iter().map(|p| p.masks.len()).sum();
        assert!(n_masks <= MAX_MASKS, "pattern library exceeds MAX_MASKS");

        let mut mask_pattern = [0u8; MAX_MASKS];
        let mut dense_pow3 = vec![[0u16; MAX_MASKS]; 64];
        let mut mask_id = 0usize;
        for (pi, p) in patterns.iter().enumerate() {
            for mask in p.masks {
                mask_pattern[mask_id] = pi as u8;
                for (j, &sq) in mask.iter().enumerate() {
                    dense_pow3[sq as usize][mask_id] = 3u16.pow((p.size - 1 - j) as u32);
                }
                mask_id += 1;
            }
        }

        let mut swap_tables: [Vec<u16>; 11] = Default::default();
        for p in patterns {
            let size = p.size;
            if !swap_tables[size].is_empty() {
                continue;
            }
            let table_size = p.table_size();
            let mut table = vec![0u16; table_size];
            for (idx, slot) in table.iter_mut().enumerate() {
                let mut swapped = 0usize;
                let mut rest = idx;
                let mut pow = 1usize;
                for _ in 0..size {
                    let digit = rest % 3;
                    rest /= 3;
                    let flipped = if digit == 2 { 2 } else { 1 - digit };
                    swapped += flipped * pow;
                    pow *= 3;
                }
                *slot = swapped as u16;
            }
            swap_tables[size] = table;
        }

        PatternIndexer {
            patterns,
            n_masks,
            mask_pattern,
            dense_pow3,
            swap_tables,
        }
    }

    pub fn init(&self, black: u64, white: u64) -> PatternIndices {
        let mut indices = PatternIndices::ZERO;
        let mut mask_id = 0usize;
        for p in self.patterns {
            for mask in p.masks {
                let mut index = 0u16;
                for &sq in *mask {
                    index = index * 3 + absolute_digit(black, white, sq);
                }
                indices.idx[mask_id] = index;
                mask_id += 1;
            }
        }
        indices
    }

    #[inline]
    pub fn apply(&self, indices: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        let mover_digit = mover.index() as u16;
        self.update_square(indices, pos.index(), mover_digit.wrapping_sub(2));
        let flip_diff = mover_digit.wrapping_sub(1 - mover_digit);
        let mut f = flipped;
        while f != 0 {
            let sq = f.trailing_zeros() as u8;
            f &= f - 1;
            self.update_square(indices, sq, flip_diff);
        }
    }

    #[inline]
    fn update_square(&self, indices: &mut PatternIndices, sq: u8, digit_diff: u16) {
        let row = unsafe { self.dense_pow3.get_unchecked(sq as usize) };
        for (slot, &p) in indices.idx.iter_mut().zip(row.iter()) {
            *slot = slot.wrapping_add(digit_diff.wrapping_mul(p));
        }
    }

    #[inline]
    pub fn mask_patterns(&self) -> &[u8] {
        &self.mask_pattern[..self.n_masks]
    }

    pub fn n_masks(&self) -> usize {
        self.n_masks
    }

    pub fn eval_sum_flat(
        &self,
        indices: &PatternIndices,
        player: Color,
        flat: &[f32],
        mask_off: &[u32],
    ) -> f32 {
        let mut score = 0.0f32;
        unsafe {
            if player == Color::Black {
                for m in 0..self.n_masks {
                    let off = *mask_off.get_unchecked(m) as usize;
                    score += *flat.get_unchecked(off + *indices.idx.get_unchecked(m) as usize);
                }
            } else {
                for m in 0..self.n_masks {
                    let pi = *self.mask_pattern.get_unchecked(m) as usize;
                    let swap = self.swap_tables.get_unchecked(self.patterns[pi].size);
                    let idx = *swap.get_unchecked(*indices.idx.get_unchecked(m) as usize);
                    let off = *mask_off.get_unchecked(m) as usize;
                    score += *flat.get_unchecked(off + idx as usize);
                }
            }
        }
        score
    }

    pub fn eval_sum_i8(&self, indices: &PatternIndices, flat: &[i8], mask_off: &[u32]) -> i32 {
        let mut score = 0i32;
        unsafe {
            for m in 0..self.n_masks {
                let off = *mask_off.get_unchecked(m) as usize;
                score += *flat.get_unchecked(off + *indices.idx.get_unchecked(m) as usize) as i32;
            }
        }
        score
    }

    pub fn swapped_index(&self, m: usize, idx: usize) -> usize {
        let pi = self.mask_pattern[m] as usize;
        self.swap_tables[self.patterns[pi].size][idx] as usize
    }

    /// Where each mask's table starts when the patterns' tables are laid end to end.
    pub fn mask_offsets(&self) -> Vec<u32> {
        let mut pattern_off = Vec::with_capacity(self.patterns.len());
        let mut off = 0u32;
        for p in self.patterns {
            pattern_off.push(off);
            off += p.table_size() as u32;
        }
        self.mask_patterns()
            .iter()
            .map(|&pi| pattern_off[pi as usize])
            .collect()
    }

    /// `table`, rows of `width` laid out by `mask_off`, with each row moved to its colour-swapped index.
    pub fn swap_colors<T: Copy>(&self, table: &[T], mask_off: &[u32], width: usize) -> Vec<T> {
        let mut out = table.to_vec();
        for (m, &off) in mask_off.iter().enumerate() {
            let off = off as usize;
            for i in 0..self.patterns[self.mask_pattern[m] as usize].table_size() {
                let j = self.swapped_index(m, i);
                out[(off + i) * width..][..width]
                    .copy_from_slice(&table[(off + j) * width..][..width]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;
    use crate::pattern::{LINEAR_PATTERNS, NNUE_PATTERNS};

    fn reference_indices(patterns: &[Pattern], b: &Board, player: Color) -> Vec<usize> {
        patterns
            .iter()
            .flat_map(|p| p.indices(b.black, b.white, player).collect::<Vec<_>>())
            .collect()
    }

    fn indexer_view(ix: &PatternIndexer, indices: &PatternIndices, player: Color) -> Vec<usize> {
        (0..ix.n_masks)
            .map(|m| {
                let idx = indices.idx[m] as usize;
                if player == Color::Black {
                    idx
                } else {
                    ix.swapped_index(m, idx)
                }
            })
            .collect()
    }

    fn deterministic_game(seed: u64) -> Vec<(Board, Position, u64, Color)> {
        let mut board = Board::new();
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut moves = Vec::new();
        loop {
            let mob = board.movable();
            if mob == 0 {
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
            let nth = (state >> 33) % mob.count_ones() as u64;
            let pos = Position(crate::bitboard::iter_bits(mob).nth(nth as usize).unwrap());
            let before = board;
            let mover = board.player();
            let flipped = board.make_move_bits(pos);
            moves.push((before, pos, flipped, mover));
        }
        moves
    }

    #[test]
    fn test_init_matches_reference_both_sets() {
        for patterns in [LINEAR_PATTERNS, NNUE_PATTERNS] {
            let ix = PatternIndexer::new(patterns);
            let b = Board::new();
            let indices = ix.init(b.black, b.white);
            for player in [Color::Black, Color::White] {
                assert_eq!(
                    indexer_view(&ix, &indices, player),
                    reference_indices(patterns, &b, player),
                );
            }
        }
    }

    #[test]
    fn test_apply_tracks_full_games() {
        for patterns in [LINEAR_PATTERNS, NNUE_PATTERNS] {
            let ix = PatternIndexer::new(patterns);
            for seed in 1..=5u64 {
                let game = deterministic_game(seed);
                let first = &game[0].0;
                let mut indices = ix.init(first.black, first.white);
                for (before, pos, flipped, mover) in &game {
                    ix.apply(&mut indices, *pos, *flipped, *mover);
                    let mut after = *before;
                    after.make_move_bits(*pos);
                    for player in [Color::Black, Color::White] {
                        assert_eq!(
                            indexer_view(&ix, &indices, player),
                            reference_indices(patterns, &after, player),
                            "seed {seed}: divergence after move {pos:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_swap_tables_are_involutions() {
        let ix = PatternIndexer::new(LINEAR_PATTERNS);
        for (size, table) in ix.swap_tables.iter().enumerate() {
            for (idx, &swapped) in table.iter().enumerate() {
                assert_eq!(
                    table[swapped as usize] as usize, idx,
                    "size {size}: swap must be an involution"
                );
            }
        }
    }
}
