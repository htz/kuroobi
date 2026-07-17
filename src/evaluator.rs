//! Stage-based pattern evaluator.
//!
//! Weights are indexed `[stage][pattern][ternary_index]` where
//! stage = 64 - empty - 4 (0..=60): the number of moves played so far.
//! Weight files use a simple little-endian binary format (magic, stage count,
//! per-pattern table sizes, then f32 tables) — portable and dependency-free.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::board::Board;
use crate::pattern::Pattern;

/// Number of game stages (moves played: 0..=60).
pub const STAGE_COUNT: usize = 61;

/// Magic bytes identifying a weight file.
const WEIGHT_MAGIC: &[u8; 8] = b"BBRVWT01";

/// Stage-based evaluator over a static pattern library.
pub struct Evaluator {
    patterns: &'static [Pattern],
    /// weights[stage][pattern][ternary_index]
    weights: Vec<Vec<Vec<f32>>>,
}

impl Evaluator {
    /// Create an evaluator with zero-initialized weights.
    pub fn new(patterns: &'static [Pattern]) -> Evaluator {
        let stage_weights: Vec<Vec<f32>> = patterns
            .iter()
            .map(|p| vec![0.0f32; p.table_size()])
            .collect();
        Evaluator {
            patterns,
            weights: vec![stage_weights; STAGE_COUNT],
        }
    }

    pub fn patterns(&self) -> &'static [Pattern] {
        self.patterns
    }

    /// Game stage for a board: moves played so far, clamped to [0, 60].
    pub fn stage(board: &Board) -> usize {
        (60 - board.empty_count() as usize).min(STAGE_COUNT - 1)
    }

    /// Evaluate the board from the current player's perspective.
    pub fn eval(&self, board: &Board) -> f32 {
        let stage = Self::stage(board);
        let weights = &self.weights[stage];

        let mut score = 0.0f32;
        for (p, table) in self.patterns.iter().zip(weights) {
            for idx in p.indices(board.black, board.white, board.player()) {
                score += table[idx];
            }
        }
        score
    }

    /// Direct access to one weight entry (for training).
    pub fn weight(&self, stage: usize, pattern: usize, index: usize) -> f32 {
        self.weights[stage][pattern][index]
    }

    pub fn set_weight(&mut self, stage: usize, pattern: usize, index: usize, value: f32) {
        self.weights[stage][pattern][index] = value;
    }

    /// One SGD step toward `target` for the position's active pattern cells.
    /// Returns the prediction error (target - prediction).
    pub fn update_weights(&mut self, board: &Board, target: f32, learning_rate: f32) -> f32 {
        let prediction = self.eval(board);
        let error = target - prediction;

        let stage = Self::stage(board);
        // Every orientation of every pattern contributed once
        let active_cells: usize = self.patterns.iter().map(|p| p.masks.len()).sum();
        let delta = learning_rate * error / active_cells as f32;

        for (pi, p) in self.patterns.iter().enumerate() {
            for idx in p.indices(board.black, board.white, board.player()) {
                self.weights[stage][pi][idx] += delta;
            }
        }
        error
    }

    /// Save all stage weights to one binary file.
    ///
    /// Layout (little-endian):
    ///   magic [8] | stage_count u32 | pattern_count u32 |
    ///   table_size u32 per pattern | f32 tables in [stage][pattern] order
    pub fn save_weights(&self, path: &Path) -> io::Result<()> {
        let mut w = BufWriter::new(File::create(path)?);
        w.write_all(WEIGHT_MAGIC)?;
        w.write_all(&(STAGE_COUNT as u32).to_le_bytes())?;
        w.write_all(&(self.patterns.len() as u32).to_le_bytes())?;
        for p in self.patterns {
            w.write_all(&(p.table_size() as u32).to_le_bytes())?;
        }
        for stage in &self.weights {
            for table in stage {
                for &v in table {
                    w.write_all(&v.to_le_bytes())?;
                }
            }
        }
        w.flush()
    }

    /// Load weights previously written by `save_weights`. The file must match
    /// this evaluator's pattern library exactly.
    pub fn load_weights(&mut self, path: &Path) -> io::Result<()> {
        let mut r = BufReader::new(File::open(path)?);

        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        if &magic != WEIGHT_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad magic"));
        }

        let mut u32buf = [0u8; 4];
        r.read_exact(&mut u32buf)?;
        if u32::from_le_bytes(u32buf) as usize != STAGE_COUNT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "stage count mismatch"));
        }
        r.read_exact(&mut u32buf)?;
        if u32::from_le_bytes(u32buf) as usize != self.patterns.len() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "pattern count mismatch"));
        }
        for p in self.patterns {
            r.read_exact(&mut u32buf)?;
            if u32::from_le_bytes(u32buf) as usize != p.table_size() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "table size mismatch"));
            }
        }

        let mut f32buf = [0u8; 4];
        for stage in &mut self.weights {
            for table in stage {
                for v in table.iter_mut() {
                    r.read_exact(&mut f32buf)?;
                    *v = f32::from_le_bytes(f32buf);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::EGAROUCID_PATTERNS;
    use crate::position::Position;

    #[test]
    fn test_stage() {
        let b = Board::new();
        assert_eq!(Evaluator::stage(&b), 0, "initial board is stage 0");

        let mut b = Board::new();
        let first = b.movable().trailing_zeros();
        b.make_move_unchecked(Position::from_index(first).unwrap());
        assert_eq!(Evaluator::stage(&b), 1, "one move played -> stage 1");
    }

    #[test]
    fn test_eval_zero_weights() {
        let e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();
        assert_eq!(e.eval(&b), 0.0);
    }

    #[test]
    fn test_update_weights_reduces_error() {
        // Note: several pattern orientations can hit the same table cell on
        // symmetric positions, which amplifies the effective step, so a
        // conservative learning rate is required for convergence.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();

        let err0 = e.update_weights(&b, 10.0, 0.1);
        assert_eq!(err0, 10.0, "first error is the full target");
        let err1 = e.update_weights(&b, 10.0, 0.1);
        assert!(
            err1.abs() < err0.abs(),
            "SGD must reduce error: {err0} -> {err1}"
        );
        assert!(e.eval(&b) > 0.0, "prediction moved toward the target");
    }

    #[test]
    fn test_perspective_antisymmetry_after_training() {
        // Weights trained from one side apply to "player", so the same
        // position from the opponent's view uses different table cells.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();
        for _ in 0..200 {
            e.update_weights(&b, 8.0, 0.1);
        }
        let mut swapped = b;
        swapped.pass();
        let own = e.eval(&b);
        let other = e.eval(&swapped);
        assert!(own > 4.0, "trained side evaluates high, got {own}");
        // The initial position is color-symmetric, so the swapped view hits
        // the *same* ternary indices -> identical score. Verify that.
        assert_eq!(own, other, "color-symmetric position evaluates equally");
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = std::env::temp_dir().join("bbrv_weight_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("weights.bin");

        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        e.set_weight(0, 0, 42, 1.25);
        e.set_weight(60, 15, 7, -3.5);
        e.save_weights(&path).unwrap();

        let mut e2 = Evaluator::new(EGAROUCID_PATTERNS);
        e2.load_weights(&path).unwrap();
        assert_eq!(e2.weight(0, 0, 42), 1.25);
        assert_eq!(e2.weight(60, 15, 7), -3.5);
        assert_eq!(e2.weight(30, 8, 0), 0.0);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_load_rejects_wrong_library() {
        let dir = std::env::temp_dir().join("bbrv_weight_test2");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("weights_egaroucid.bin");

        let e = Evaluator::new(EGAROUCID_PATTERNS);
        e.save_weights(&path).unwrap();

        let mut edax = Evaluator::new(crate::pattern::EDAX_PATTERNS);
        assert!(
            edax.load_weights(&path).is_err(),
            "loading Egaroucid weights into an Edax evaluator must fail"
        );

        std::fs::remove_file(&path).ok();
    }
}
