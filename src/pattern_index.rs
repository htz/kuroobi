//! Incremental (differential) pattern-index maintenance.

use crate::color::Color;
use crate::pattern::Pattern;
use crate::position::Position;

pub const MAX_MASKS: usize = 80;

#[derive(Clone, Copy)]
pub struct UpdateEntry {
    pub mask: u16,
    pub pow3: u16,
}

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

    #[inline]
    pub fn raw_mut(&mut self) -> &mut [u16; MAX_MASKS] {
        &mut self.idx
    }
}

pub struct PatternIndexer {
    patterns: &'static [Pattern],
    n_masks: usize,
    mask_pattern: [u8; MAX_MASKS],
    dense_pow3: Vec<[u16; MAX_MASKS]>,
    offsets: [u32; 65],
    entries: Vec<UpdateEntry>,
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
        let mut per_square: Vec<Vec<UpdateEntry>> = vec![Vec::new(); 64];
        let mut mask_id = 0u16;
        for (pi, p) in patterns.iter().enumerate() {
            for mask in p.masks {
                mask_pattern[mask_id as usize] = pi as u8;
                for (j, &sq) in mask.iter().enumerate() {
                    let pow3 = 3u16.pow((p.size - 1 - j) as u32);
                    per_square[sq as usize].push(UpdateEntry {
                        mask: mask_id,
                        pow3,
                    });
                }
                mask_id += 1;
            }
        }

        let mut offsets = [0u32; 65];
        let mut entries = Vec::new();
        for sq in 0..64 {
            offsets[sq] = entries.len() as u32;
            entries.extend_from_slice(&per_square[sq]);
        }
        offsets[64] = entries.len() as u32;
        let mut dense_pow3 = vec![[0u16; MAX_MASKS]; 64];
        for (sq, row) in dense_pow3.iter_mut().enumerate() {
            for e in &per_square[sq] {
                row[e.mask as usize] = e.pow3;
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
            offsets,
            entries,
            dense_pow3,
            swap_tables,
        }
    }

    pub fn patterns(&self) -> &'static [Pattern] {
        self.patterns
    }

    pub fn init(&self, black: u64, white: u64) -> PatternIndices {
        let mut indices = PatternIndices {
            idx: [0u16; MAX_MASKS],
        };
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
        let mover_digit = mover.index() as u16; // Black = 0, White = 1
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
    pub fn undo(&self, indices: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        let mover_digit = mover.index() as u16;
        self.update_square(indices, pos.index(), 2u16.wrapping_sub(mover_digit));
        let flip_diff = (1 - mover_digit).wrapping_sub(mover_digit);
        let mut f = flipped;
        while f != 0 {
            let sq = f.trailing_zeros() as u8;
            f &= f - 1;
            self.update_square(indices, sq, flip_diff);
        }
    }

    #[inline]
    pub fn square_entries(&self, sq: u8) -> &[UpdateEntry] {
        let start = self.offsets[sq as usize] as usize;
        let end = self.offsets[sq as usize + 1] as usize;
        &self.entries[start..end]
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

    pub fn eval_sum(&self, indices: &PatternIndices, player: Color, weights: &[Vec<f32>]) -> f32 {
        let mut score = 0.0f32;
        unsafe {
            if player == Color::Black {
                for m in 0..self.n_masks {
                    let pi = *self.mask_pattern.get_unchecked(m) as usize;
                    let table = weights.get_unchecked(pi);
                    score += *table.get_unchecked(*indices.idx.get_unchecked(m) as usize);
                }
            } else {
                for m in 0..self.n_masks {
                    let pi = *self.mask_pattern.get_unchecked(m) as usize;
                    let table = weights.get_unchecked(pi);
                    let swap = self.swap_tables.get_unchecked(self.patterns[pi].size);
                    let idx = *swap.get_unchecked(*indices.idx.get_unchecked(m) as usize);
                    score += *table.get_unchecked(idx as usize);
                }
            }
        }
        score
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
                    let pi = ix.mask_pattern[m] as usize;
                    ix.swap_tables[ix.patterns[pi].size][idx] as usize
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
            let count = mob.count_ones() as u64;
            let mut nth = (state >> 33) % count;
            let mut m = mob;
            while nth > 0 {
                m &= m - 1;
                nth -= 1;
            }
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
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
                    after.make_move_unchecked(*pos);
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
    fn test_undo_restores_exactly() {
        let ix = PatternIndexer::new(LINEAR_PATTERNS);
        for seed in 1..=3u64 {
            let game = deterministic_game(seed);
            let first = &game[0].0;
            let mut indices = ix.init(first.black, first.white);
            for (before, pos, flipped, mover) in &game {
                let snapshot = indices.idx;
                ix.apply(&mut indices, *pos, *flipped, *mover);
                ix.undo(&mut indices, *pos, *flipped, *mover);
                assert_eq!(indices.idx, snapshot, "undo must restore indices");
                let _ = before;
                ix.apply(&mut indices, *pos, *flipped, *mover);
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
