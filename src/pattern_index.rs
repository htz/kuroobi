//! Incremental (differential) pattern-index maintenance.
//!
//! `Evaluator::eval` recomputes every pattern orientation's ternary index
//! from the bitboards on each call (~64 masks x ~9 squares). During search
//! successive positions differ by one placed disc plus a few flips, so the
//! indices can instead be updated square-by-square from the move's flip
//! bitboard at a fraction of the cost.
//!
//! Representation: indices are kept in **absolute colors** (digit 0 = black,
//! 1 = white, 2 = empty), which makes apply/undo independent of the side to
//! move. Evaluation from White's perspective maps each index through a
//! precomputed digit-swap table (0 <-> 1), giving exactly the player-relative
//! index that `Pattern::mask_index` computes.

use crate::color::Color;
use crate::pattern::Pattern;
use crate::position::Position;

/// Upper bound on total orientations across a pattern library
/// (Egaroucid: 16 patterns x 4 masks = 64; Edax: 46; Egaroucid-plus: 72).
pub const MAX_MASKS: usize = 80;

/// One differential update: mask `mask`'s index changes by
/// `digit_diff * pow3` when the square owning this entry changes color.
#[derive(Clone, Copy)]
struct UpdateEntry {
    mask: u16,
    pow3: u16,
}

/// The per-mask ternary indices of one position (absolute colors).
/// Copy-sized so search can snapshot it cheaply if ever needed.
#[derive(Clone, Copy)]
pub struct PatternIndices {
    idx: [u16; MAX_MASKS],
}

/// Static lookup tables for one pattern library: square -> affected masks,
/// plus digit-swap tables for White-perspective evaluation.
pub struct PatternIndexer {
    patterns: &'static [Pattern],
    n_masks: usize,
    /// mask -> owning pattern index (masks flattened in pattern order).
    mask_pattern: [u8; MAX_MASKS],
    /// CSR layout: entries for square `sq` live at
    /// `entries[offsets[sq]..offsets[sq + 1]]`.
    offsets: [u32; 65],
    entries: Vec<UpdateEntry>,
    /// swap_tables[size][index] = index with digits 0 and 1 swapped.
    /// Shared across patterns of the same size; unused sizes stay empty.
    swap_tables: [Vec<u16>; 11],
}

/// Ternary digit for a square in absolute colors.
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

        // Flatten masks in pattern order and build the square -> mask lists.
        let mut mask_pattern = [0u8; MAX_MASKS];
        let mut per_square: Vec<Vec<UpdateEntry>> = vec![Vec::new(); 64];
        let mut mask_id = 0u16;
        for (pi, p) in patterns.iter().enumerate() {
            for mask in p.masks {
                mask_pattern[mask_id as usize] = pi as u8;
                for (j, &sq) in mask.iter().enumerate() {
                    // First square in the mask carries the highest power.
                    let pow3 = 3u16.pow((p.size - 1 - j) as u32);
                    per_square[sq as usize].push(UpdateEntry { mask: mask_id, pow3 });
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

        // Digit-swap tables per distinct pattern size.
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
            swap_tables,
        }
    }

    pub fn patterns(&self) -> &'static [Pattern] {
        self.patterns
    }

    /// Compute all mask indices from scratch (absolute colors).
    pub fn init(&self, black: u64, white: u64) -> PatternIndices {
        let mut indices = PatternIndices { idx: [0u16; MAX_MASKS] };
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

    /// Update `indices` for a move by `mover` placing on `pos` and flipping
    /// `flipped`. Must mirror `Board::make_move_bits` exactly.
    #[inline]
    pub fn apply(&self, indices: &mut PatternIndices, pos: Position, flipped: u64, mover: Color) {
        let mover_digit = mover.index() as u16; // Black = 0, White = 1
        // Placed disc: empty (2) -> mover.
        self.update_square(indices, pos.index(), mover_digit.wrapping_sub(2));
        // Flipped discs: opponent (1 - mover) -> mover.
        let flip_diff = mover_digit.wrapping_sub(1 - mover_digit);
        let mut f = flipped;
        while f != 0 {
            let sq = f.trailing_zeros() as u8;
            f &= f - 1;
            self.update_square(indices, sq, flip_diff);
        }
    }

    /// Exact inverse of [`apply`](Self::apply).
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

    /// Add `digit_diff * pow3` to every mask index containing `sq`.
    /// `digit_diff` is a two's-complement u16; wrapping arithmetic is exact
    /// because every true result stays within 0..3^size.
    #[inline]
    fn update_square(&self, indices: &mut PatternIndices, sq: u8, digit_diff: u16) {
        // SAFETY: `offsets` has 65 entries and `sq < 64`, so both reads are in
        // range and `start <= end <= entries.len()` by construction. Every
        // `e.mask` was assigned in `new` from `0..n_masks`, and `n_masks` is
        // asserted to be at most `MAX_MASKS`, the length of `indices.idx`.
        unsafe {
            let start = *self.offsets.get_unchecked(sq as usize) as usize;
            let end = *self.offsets.get_unchecked(sq as usize + 1) as usize;
            for e in self.entries.get_unchecked(start..end) {
                let delta = digit_diff.wrapping_mul(e.pow3);
                let slot = indices.idx.get_unchecked_mut(e.mask as usize);
                *slot = slot.wrapping_add(delta);
            }
        }
    }

    /// Sum pattern weights for the maintained indices from `player`'s
    /// perspective. `weights[pattern][ternary_index]` must match this
    /// indexer's pattern library. Summation order (patterns, then masks)
    /// is identical to `Evaluator::eval` so results are bit-exact.
    #[inline]
    /// Pattern id of each mask instance, in mask order.
    pub fn mask_patterns(&self) -> &[u8] {
        &self.mask_pattern[..self.n_masks]
    }

    /// Number of mask instances.
    pub fn n_masks(&self) -> usize {
        self.n_masks
    }

    /// `eval_sum` over weights laid end to end, with `mask_off[m]` giving the
    /// start of mask `m`'s pattern table. One dependent load per mask instead
    /// of the pointer-then-weight pair a `Vec<Vec<f32>>` forces.
    pub fn eval_sum_flat(
        &self,
        indices: &PatternIndices,
        player: Color,
        flat: &[f32],
        mask_off: &[u32],
    ) -> f32 {
        let mut score = 0.0f32;
        // SAFETY: `mask_off` is built with one entry per mask, each pointing
        // at a table of `3^size` inside `flat`, and the maintained indices
        // stay inside that range (same invariant `eval_sum` relies on).
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

    /// `eval_sum_flat` over 16-bit weights. Halving the table halves the
    /// cache footprint of the one array the ordering touches millions of
    /// times.
    /// `eval_sum_i16` over 8-bit weights: a stage's table is a quarter of the
    /// f32 one, which is what the ordering walks millions of times.
    pub fn eval_sum_i8(&self, indices: &PatternIndices, flat: &[i8], mask_off: &[u32]) -> i32 {
        let mut score = 0i32;
        // SAFETY: same invariant as `eval_sum_flat`.
        unsafe {
            for m in 0..self.n_masks {
                let off = *mask_off.get_unchecked(m) as usize;
                score += *flat.get_unchecked(off + *indices.idx.get_unchecked(m) as usize) as i32;
            }
        }
        score
    }

    pub fn eval_sum_i16(&self, indices: &PatternIndices, flat: &[i16], mask_off: &[u32]) -> i32 {
        let mut score = 0i32;
        // SAFETY: same invariant as `eval_sum_flat`. The White table has the
        // digit swap already applied, so both colours take this one path.
        unsafe {
            for m in 0..self.n_masks {
                let off = *mask_off.get_unchecked(m) as usize;
                score += *flat.get_unchecked(off + *indices.idx.get_unchecked(m) as usize) as i32;
            }
        }
        score
    }

    /// Index that mask `m`'s entry would have from White's perspective.
    pub fn swapped_index(&self, m: usize, idx: usize) -> usize {
        let pi = self.mask_pattern[m] as usize;
        self.swap_tables[self.patterns[pi].size][idx] as usize
    }

    pub fn eval_sum(&self, indices: &PatternIndices, player: Color, weights: &[Vec<f32>]) -> f32 {
        let mut score = 0.0f32;
        // SAFETY: `m < n_masks <= MAX_MASKS` bounds `mask_pattern` and
        // `indices.idx`; `mask_pattern` holds pattern ids, and `weights` is
        // checked on load to have one table per pattern sized `3^size`, which
        // is exactly the range the maintained indices live in. The swap
        // tables are built with one entry per index of their size.
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
    use crate::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};

    /// Expected indices via the existing per-position recomputation, in
    /// `player`'s perspective.
    fn reference_indices(patterns: &[Pattern], b: &Board, player: Color) -> Vec<usize> {
        patterns
            .iter()
            .flat_map(|p| p.indices(b.black, b.white, player).collect::<Vec<_>>())
            .collect()
    }

    /// Indexer view of the same thing: absolute indices, swapped for White.
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
        // (board BEFORE move, pos, flipped, mover) for a full random game
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
    fn test_init_matches_reference_both_libraries() {
        for patterns in [EGAROUCID_PATTERNS, EDAX_PATTERNS, EGAROUCID_PLUS_PATTERNS] {
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
        for patterns in [EGAROUCID_PATTERNS, EDAX_PATTERNS, EGAROUCID_PLUS_PATTERNS] {
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
        let ix = PatternIndexer::new(EGAROUCID_PATTERNS);
        for seed in 1..=3u64 {
            let game = deterministic_game(seed);
            let first = &game[0].0;
            let mut indices = ix.init(first.black, first.white);
            for (before, pos, flipped, mover) in &game {
                let snapshot = indices.idx;
                ix.apply(&mut indices, *pos, *flipped, *mover);
                ix.undo(&mut indices, *pos, *flipped, *mover);
                assert_eq!(indices.idx, snapshot, "undo must restore indices");
                // Advance for the next iteration
                let _ = before;
                ix.apply(&mut indices, *pos, *flipped, *mover);
            }
        }
    }

    #[test]
    fn test_swap_tables_are_involutions() {
        let ix = PatternIndexer::new(EGAROUCID_PATTERNS);
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
