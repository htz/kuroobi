//! Stage-based pattern evaluator with training support.
//!
//! Weights are indexed `[stage][pattern][ternary_index]` where
//! stage = 64 - empty - 4 (0..=60): the number of moves played so far.
//! Weight files use a simple little-endian binary format (magic, stage count,
//! per-pattern table sizes, then f32 tables) — portable and dependency-free.
//!
//! Training features for fast reinforcement-learning convergence:
//! - Adam optimizer (per-cell adaptive step) alongside plain SGD
//! - 8-fold symmetry data augmentation (`train`) — one labeled position
//!   trains all rotations/mirrors, an 8x effective sample multiplier
//! - TD(λ)-style whole-game credit assignment (`train_game`)

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

/// Fixed-point scale of the ordering-grade 16-bit weights.
const I16_SCALE: f32 = 256.0;

use crate::board::Board;
use crate::color::Color;
use crate::pattern::Pattern;
use crate::pattern_index::{PatternIndexer, PatternIndices};

/// Number of game stages (moves played: 0..=60).
pub const STAGE_COUNT: usize = 61;

/// Magic bytes identifying a weight file (v2: adds disc-count tables).
const WEIGHT_MAGIC: &[u8; 8] = b"BBRVWT02";
/// v1 format: pattern tables only. Still loadable (disc-count weights zero).
const WEIGHT_MAGIC_V1: &[u8; 8] = b"BBRVWT01";

/// Cells in the per-stage disc-count table (player disc count 0..=64).
const NUM_TABLE_SIZE: usize = 65;

/// Stage-based evaluator over a static pattern library.
///
/// Besides the pattern features it carries one extra
/// feature: a per-stage table indexed by the current player's disc count.
/// Since the total disc count is fixed within a stage, this is effectively
/// the disc differential — information the local patterns don't provide.
pub struct Evaluator {
    patterns: &'static [Pattern],
    /// weights[stage][pattern][ternary_index]
    weights: Vec<Vec<Vec<f32>>>,
    /// The same weights, per stage, with the pattern tables laid end to end.
    /// Evaluation walks one mask at a time, and reading them out of a
    /// `Vec<Vec<f32>>` costs a pointer load before every weight load; a flat
    /// array plus a small offset table (see `PatternIndexer::mask_offsets`)
    /// leaves a single dependent load per mask.
    flat_weights: Vec<Vec<f32>>,
    /// Start of each mask instance's table inside a `flat_weights` stage.
    mask_off: Vec<u32>,
    /// `flat_weights` quantized to 16 bits, used by the search's ordering
    /// where a fraction of a disc never changes the order but the halved
    /// cache footprint shows up in the profile.
    flat_i16: Vec<Vec<i16>>,
    /// The same, with the White digit swap baked in, so evaluation never
    /// pays for the swap lookup.
    flat_i16_white: Vec<Vec<i16>>,
    /// num_weights[stage][player disc count]
    num_weights: Vec<[f32; NUM_TABLE_SIZE]>,
    /// Lookup tables for incremental index maintenance during search.
    indexer: PatternIndexer,
}

/// Per-cell weight update rule used by the training entry points.
pub trait Optimizer {
    /// Weight delta for one active cell. `grad` is the (positive-direction)
    /// error `target - prediction`; `table_size` is the pattern's 3^size
    /// (for optimizers that allocate per-cell state lazily).
    fn step(&mut self, stage: usize, pattern: usize, index: usize, table_size: usize, grad: f32)
        -> f32;

    /// Called once per epoch by trainers that support schedules.
    fn next_epoch(&mut self) {}
}

/// Plain SGD with optional per-epoch learning-rate decay. Stateless per
/// cell, and the step is proportional to the error — on a linear model this
/// makes early training (large errors) converge much faster than Adam,
/// whose normalized step is capped near `lr` regardless of error size.
pub struct SgdOptimizer {
    pub learning_rate: f32,
    pub decay: f32,
}

impl SgdOptimizer {
    pub fn new(learning_rate: f32, decay: f32) -> SgdOptimizer {
        SgdOptimizer { learning_rate, decay }
    }
}

impl Optimizer for SgdOptimizer {
    #[inline]
    fn step(&mut self, _stage: usize, _pattern: usize, _index: usize, _table_size: usize, grad: f32) -> f32 {
        self.learning_rate * grad
    }

    fn next_epoch(&mut self) {
        self.learning_rate *= self.decay;
    }
}

/// One Adam moment cell: (first moment m, second moment v, step count t).
type MomentCell = (f32, f32, u32);
/// Lazily-allocated moment table for one (stage, pattern) pair.
type MomentTable = Option<Box<[MomentCell]>>;

/// Adam optimizer state: first/second moment per touched weight cell.
pub struct AdamOptimizer {
    pub learning_rate: f32,
    pub beta1: f32,
    pub beta2: f32,
    pub epsilon: f32,
    /// Per-(stage, pattern) moment tables, allocated lazily on first touch.
    /// Dense storage: full-corpus training touches most cells, where a
    /// HashMap's hashing and node overhead dominates.
    moments: Vec<Vec<MomentTable>>,
}

impl AdamOptimizer {
    pub fn new(learning_rate: f32) -> AdamOptimizer {
        AdamOptimizer {
            learning_rate,
            beta1: 0.9,
            beta2: 0.999,
            epsilon: 1e-8,
            moments: Vec::new(),
        }
    }
}

impl Optimizer for AdamOptimizer {
    /// Bias-corrected Adam step for one cell.
    #[inline]
    fn step(&mut self, stage: usize, pattern: usize, index: usize, table_size: usize, grad: f32) -> f32 {
        if self.moments.len() <= stage {
            self.moments.resize_with(stage + 1, Vec::new);
        }
        let stage_tables = &mut self.moments[stage];
        if stage_tables.len() <= pattern {
            stage_tables.resize_with(pattern + 1, || None);
        }
        let table = stage_tables[pattern]
            .get_or_insert_with(|| vec![(0.0f32, 0.0f32, 0u32); table_size].into_boxed_slice());

        let (m, v, t) = &mut table[index];
        *t += 1;
        *m = self.beta1 * *m + (1.0 - self.beta1) * grad;
        *v = self.beta2 * *v + (1.0 - self.beta2) * grad * grad;
        let m_hat = *m / (1.0 - self.beta1.powi(*t as i32));
        let v_hat = *v / (1.0 - self.beta2.powi(*t as i32));
        self.learning_rate * m_hat / (v_hat.sqrt() + self.epsilon)
    }
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
            flat_weights: Vec::new(),
            mask_off: Vec::new(),
            flat_i16: Vec::new(),
            flat_i16_white: Vec::new(),
            num_weights: vec![[0.0f32; NUM_TABLE_SIZE]; STAGE_COUNT],
            indexer: PatternIndexer::new(patterns),
        }
    }

    pub fn patterns(&self) -> &'static [Pattern] {
        self.patterns
    }

    /// Lookup tables for incremental pattern-index maintenance.
    pub fn indexer(&self) -> &PatternIndexer {
        &self.indexer
    }

    /// Game stage for a board: moves played so far, clamped to [0, 60].
    /// saturating_sub guards artificial positions with more than 60 empties
    /// (fewer than 4 discs).
    pub fn stage(board: &Board) -> usize {
        60usize
            .saturating_sub(board.empty_count() as usize)
            .min(STAGE_COUNT - 1)
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
        score + self.num_weights[stage][self.num_index(board)]
    }

    /// Disc-count feature index: the current player's disc count.
    #[inline]
    fn num_index(&self, board: &Board) -> usize {
        board.player_bb().count_ones() as usize
    }

    /// Evaluate from incrementally-maintained pattern indices (see
    /// [`PatternIndexer`]). `board` supplies only stage and side to move;
    /// `indices` must correspond to `board`'s discs. Bit-exact with
    /// [`eval`](Self::eval): same weights are summed in the same order.
    #[inline]
    pub fn eval_indices(&self, board: &Board, indices: &PatternIndices) -> f32 {
        let stage = Self::stage(board);
        let patterns = if self.flat_weights.is_empty() {
            self.indexer
                .eval_sum(indices, board.player(), &self.weights[stage])
        } else {
            self.indexer.eval_sum_flat(
                indices,
                board.player(),
                &self.flat_weights[stage],
                &self.mask_off,
            )
        };
        patterns + self.num_weights[stage][self.num_index(board)]
    }

    /// Rebuild the flat evaluation copy of the weights. Every mutation
    /// invalidates it (see `invalidate_flat`); search paths rebuild once
    /// after loading and then only read.
    fn rebuild_flat(&mut self) {
        let mut pattern_off = Vec::with_capacity(self.patterns.len());
        let mut off = 0u32;
        for p in self.patterns {
            pattern_off.push(off);
            off += p.table_size() as u32;
        }
        self.mask_off = self
            .indexer
            .mask_patterns()
            .iter()
            .map(|&pi| pattern_off[pi as usize])
            .collect();
        self.flat_weights = self
            .weights
            .iter()
            .map(|stage| stage.iter().flat_map(|t| t.iter().copied()).collect())
            .collect();
        self.flat_i16 = self
            .flat_weights
            .iter()
            .map(|stage| {
                stage
                    .iter()
                    .map(|&w| (w * I16_SCALE).clamp(-32768.0, 32767.0) as i16)
                    .collect()
            })
            .collect();
        // White's table: entry m,i holds the weight the swapped index selects.
        let n_masks = self.indexer.n_masks();
        self.flat_i16_white = self
            .flat_i16
            .iter()
            .map(|stage| {
                let mut out = stage.clone();
                for m in 0..n_masks {
                    let off = self.mask_off[m] as usize;
                    let size = self.patterns[self.indexer.mask_patterns()[m] as usize].table_size();
                    for i in 0..size {
                        out[off + i] = stage[off + self.indexer.swapped_index(m, i)];
                    }
                }
                out
            })
            .collect();
    }

    /// Drop the flat copy; the next evaluation falls back to the nested
    /// tables until it is rebuilt.
    fn invalidate_flat(&mut self) {
        self.flat_weights.clear();
        self.flat_i16.clear();
        self.flat_i16_white.clear();
        self.mask_off.clear();
    }

    /// Ordering-grade evaluation from raw bitboards: 16-bit weights, so a
    /// stage's table is half the size the exact path walks.
    pub fn eval_order_bb(
        &self,
        player: u64,
        opponent: u64,
        color: Color,
        indices: &PatternIndices,
    ) -> f32 {
        let empties = 64 - (player | opponent).count_ones() as usize;
        let stage = 60usize.saturating_sub(empties).min(STAGE_COUNT - 1);
        if self.flat_i16.is_empty() {
            // Weights were mutated since the last rebuild; fall back.
            let mut b = Board::new();
            b.black = if matches!(color, Color::Black) { player } else { opponent };
            b.white = if matches!(color, Color::Black) { opponent } else { player };
            b.player = color;
            b.empty_count = empties as u8;
            return self.eval_indices(&b, indices);
        }
        let table = if matches!(color, Color::Black) {
            &self.flat_i16[stage]
        } else {
            &self.flat_i16_white[stage]
        };
        let sum = self.indexer.eval_sum_i16(indices, table, &self.mask_off);
        sum as f32 / I16_SCALE + self.num_weights[stage][player.count_ones() as usize]
    }

    /// Direct access to one weight entry (for training).
    pub fn weight(&self, stage: usize, pattern: usize, index: usize) -> f32 {
        self.weights[stage][pattern][index]
    }

    /// Disc-count feature weight for `count` player discs at `stage`.
    pub fn num_weight(&self, stage: usize, count: usize) -> f32 {
        self.num_weights[stage][count]
    }

    pub fn set_weight(&mut self, stage: usize, pattern: usize, index: usize, value: f32) {
        self.invalidate_flat();
        self.weights[stage][pattern][index] = value;
    }

    /// One SGD step toward `target`. Returns the prediction error
    /// (target - prediction) BEFORE the update.
    ///
    /// The model is linear: prediction = Σ w[active cell]. For squared loss
    /// L = ½(target - pred)², the gradient wrt each active weight is -error,
    /// so the update is `w += lr * error` per active cell (a cell hit by k
    /// orientations receives the update k times = its true gradient). With
    /// N active cells
    /// the prediction moves by ≥ N*lr*error per step, so lr must be < 2/N
    /// for convergence (the 16-pattern set has N = 64 → lr ≲ 0.03; training
    /// here uses lr = 0.01).
    pub fn update_weights(&mut self, board: &Board, target: f32, learning_rate: f32) -> f32 {
        let prediction = self.eval(board);
        let error = target - prediction;

        let stage = Self::stage(board);
        let delta = learning_rate * error;

        for (pi, p) in self.patterns.iter().enumerate() {
            for idx in p.indices(board.black, board.white, board.player()) {
                self.weights[stage][pi][idx] += delta;
                self.flat_weights.clear();
            }
        }
        let num_idx = self.num_index(board);
        self.num_weights[stage][num_idx] += delta;
        error
    }

    /// One optimizer step toward `target`. Returns the pre-update error.
    pub fn update_weights_with(
        &mut self,
        board: &Board,
        target: f32,
        opt: &mut impl Optimizer,
    ) -> f32 {
        let prediction = self.eval(board);
        let error = target - prediction;
        let stage = Self::stage(board);

        for (pi, p) in self.patterns.iter().enumerate() {
            let table_size = p.table_size();
            for idx in p.indices(board.black, board.white, board.player()) {
                let delta = opt.step(stage, pi, idx, table_size, error);
                self.weights[stage][pi][idx] += delta;
                self.flat_weights.clear();
            }
        }
        // Disc-count cell: registered with the optimizer as a virtual
        // pattern one past the last real one.
        let num_idx = self.num_index(board);
        let delta = opt.step(stage, self.patterns.len(), num_idx, NUM_TABLE_SIZE, error);
        self.num_weights[stage][num_idx] += delta;
        error
    }

    /// Backwards-compatible alias for Adam-based single updates.
    pub fn update_weights_adam(
        &mut self,
        board: &Board,
        target: f32,
        opt: &mut AdamOptimizer,
    ) -> f32 {
        self.update_weights_with(board, target, opt)
    }

    /// Train on one labeled position with 8-fold symmetry augmentation:
    /// every rotation/mirror of the position shares the same value, so one
    /// sample teaches eight — this is the single biggest convergence win for
    /// pattern-based Othello evaluators. Returns the mean absolute error
    /// over the eight variants (before their respective updates).
    pub fn train(&mut self, board: &Board, target: f32, opt: &mut impl Optimizer) -> f32 {
        let mut total_abs_err = 0.0f32;
        for sym in board.symmetries() {
            total_abs_err += self.update_weights_with(&sym, target, opt).abs();
        }
        total_abs_err / 8.0
    }

    /// Train from a finished self-play game with TD(λ)-style targets.
    ///
    /// `history` is the sequence of positions from the first move to the
    /// final position; `final_score` is the game outcome from **Black's**
    /// perspective (disc difference). Each position's target blends the
    /// final outcome with the (bootstrapped) evaluation of the next
    /// position, geometrically weighted by `lambda`:
    ///   λ = 1  -> pure Monte-Carlo (every position labeled with the outcome)
    ///   λ = 0  -> pure TD(0) (each position pulls toward the next eval)
    /// Positions are trained late-to-early so bootstrap targets use
    /// already-updated (fresher) weights. Returns mean |error|.
    pub fn train_game(
        &mut self,
        history: &[Board],
        final_score: f32,
        lambda: f32,
        opt: &mut impl Optimizer,
    ) -> f32 {
        if history.is_empty() {
            return 0.0;
        }

        let mut total_abs_err = 0.0f32;
        // Value seen from the position *after* each board, in that
        // position's own perspective. Start from the terminal outcome.
        let mut next_value_black_view = final_score;

        for board in history.iter().rev() {
            // Convert the successor value into this board's perspective
            let outcome_here = if board.player() == crate::color::Color::Black {
                final_score
            } else {
                -final_score
            };
            let bootstrap_here = if board.player() == crate::color::Color::Black {
                next_value_black_view
            } else {
                -next_value_black_view
            };

            let target = lambda * outcome_here + (1.0 - lambda) * bootstrap_here;
            total_abs_err += self.train(board, target, opt);

            // The freshly-trained evaluation of this position becomes the
            // bootstrap for its predecessor (stored in Black's view).
            let v = self.eval(board);
            next_value_black_view = if board.player() == crate::color::Color::Black {
                v
            } else {
                -v
            };
        }

        total_abs_err / history.len() as f32
    }

    /// Save all stage weights to one binary file, atomically: data is
    /// written to a sibling temp file and renamed into place, so an
    /// interruption mid-write can never corrupt an existing weight file.
    ///
    /// Layout (little-endian):
    ///   magic [8] | stage_count u32 | pattern_count u32 |
    ///   table_size u32 per pattern | f32 tables in [stage][pattern] order |
    ///   (v2 only) f32 disc-count tables, 65 per stage
    pub fn save_weights(&self, path: &Path) -> io::Result<()> {
        let tmp_path = path.with_extension("tmp");
        {
            let mut w = BufWriter::new(File::create(&tmp_path)?);
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
            for table in &self.num_weights {
                for &v in table.iter() {
                    w.write_all(&v.to_le_bytes())?;
                }
            }
            w.flush()?;
            w.get_ref().sync_all()?;
        }
        std::fs::rename(&tmp_path, path)
    }

    /// Load weights previously written by `save_weights`. The file must match
    /// this evaluator's pattern library exactly. v1 files (pattern tables
    /// only) load with zeroed disc-count weights.
    pub fn load_weights(&mut self, path: &Path) -> io::Result<()> {
        let mut r = BufReader::new(File::open(path)?);

        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        let v1 = &magic == WEIGHT_MAGIC_V1;
        if !v1 && &magic != WEIGHT_MAGIC {
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
        if v1 {
            for table in self.num_weights.iter_mut() {
                table.fill(0.0);
            }
        } else {
            for table in self.num_weights.iter_mut() {
                for v in table.iter_mut() {
                    r.read_exact(&mut f32buf)?;
                    *v = f32::from_le_bytes(f32buf);
                }
            }
        }
        self.rebuild_flat();
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

    /// Effective gradient multiplier for one position: Σ k² over distinct
    /// active cells, where k = number of orientations hitting that cell,
    /// plus 1 for the disc-count cell (hit exactly once). A cell hit k
    /// times is updated k times and read back k times, so the prediction
    /// moves by lr * error * (Σk² + 1) per step. On asymmetric positions
    /// Σk² = 64 (all cells distinct); the color/rotation-symmetric initial
    /// position collapses many cells (Σk² = 216 there).
    fn gradient_multiplier(board: &Board) -> f32 {
        use std::collections::HashMap;
        let mut counts: HashMap<(usize, usize), u32> = HashMap::new();
        for (pi, p) in EGAROUCID_PATTERNS.iter().enumerate() {
            for idx in p.indices(board.black, board.white, board.player()) {
                *counts.entry((pi, idx)).or_insert(0) += 1;
            }
        }
        counts.values().map(|&k| (k * k) as f32).sum::<f32>() + 1.0
    }

    /// Learning rate safe for repeated single-position training on any
    /// position: lr * (Σk² + 1) < 2 with Σk² ≤ 216 → lr < 0.009. The Go
    /// trainer's lr = 0.01 is safe in its regime (SGD over many mostly-
    /// asymmetric examples), but not for this stress pattern.
    const LR: f32 = 0.005;

    #[test]
    fn test_update_weights_reduces_error() {
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();

        let err0 = e.update_weights(&b, 10.0, LR);
        assert_eq!(err0, 10.0, "first error is the full target");
        let err1 = e.update_weights(&b, 10.0, LR);
        assert!(
            err1.abs() < err0.abs(),
            "SGD must reduce error: {err0} -> {err1}"
        );
        assert!(e.eval(&b) > 0.0, "prediction moved toward the target");
    }

    #[test]
    fn test_update_weights_converges_to_target() {
        // Repeated steps on one position must converge to the exact target
        // (the position is always representable by a linear model).
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();
        for _ in 0..200 {
            e.update_weights(&b, 8.0, LR);
        }
        let final_err = (8.0 - e.eval(&b)).abs();
        assert!(final_err < 0.05, "must converge to target, residual {final_err}");
    }

    #[test]
    fn test_update_gradient_magnitude_matches_linear_model() {
        // One step from zero weights must move the prediction by exactly
        // lr * error * Σk² (see gradient_multiplier). Check on both the
        // symmetric initial position and an asymmetric one.
        for plies in [0, 1] {
            let mut b = Board::new();
            for _ in 0..plies {
                let pos =
                    crate::position::Position::from_index(b.movable().trailing_zeros()).unwrap();
                b.make_move_unchecked(pos);
            }

            let multiplier = gradient_multiplier(&b);
            if plies == 0 {
                // 8-fold symmetry of the start position collapses many cells
                assert_eq!(multiplier, 217.0, "symmetric start: Σk² + 1 = 217");
            } else {
                // Any first move retains one diagonal mirror symmetry
                assert_eq!(multiplier, 193.0, "first move keeps a mirror: Σk² + 1 = 193");
            }

            let mut e = Evaluator::new(EGAROUCID_PATTERNS);
            let target = 1.0f32;
            e.update_weights(&b, target, LR);
            let expected = LR * target * multiplier;
            let got = e.eval(&b);
            assert!(
                (got - expected).abs() < 1e-4,
                "plies={plies}: one-step prediction {got}, analytic {expected}"
            );
        }
    }

    #[test]
    fn test_update_only_touches_current_stage() {
        // Training a stage-0 position must leave every other stage at zero.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();
        assert_eq!(Evaluator::stage(&b), 0);
        e.update_weights(&b, 5.0, LR);

        let mut later = b;
        let pos = crate::position::Position::from_index(later.movable().trailing_zeros()).unwrap();
        later.make_move_unchecked(pos); // now stage 1
        assert_eq!(Evaluator::stage(&later), 1);
        assert_eq!(e.eval(&later), 0.0, "stage-1 weights must be untouched");
    }

    #[test]
    fn test_perspective_antisymmetry_after_training() {
        // Weights trained from one side apply to "player", so the same
        // position from the opponent's view uses different table cells.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();
        for _ in 0..300 {
            e.update_weights(&b, 8.0, LR);
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
    fn test_asymmetric_position_perspectives_differ() {
        // After one real move the position is no longer color-symmetric:
        // training black's view must not equally train white's view.
        let mut b = Board::new();
        let pos = crate::position::Position::from_index(b.movable().trailing_zeros()).unwrap();
        b.make_move_unchecked(pos);

        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        for _ in 0..300 {
            e.update_weights(&b, 8.0, LR);
        }
        let mut swapped = b;
        swapped.pass();
        let own = e.eval(&b);
        let other = e.eval(&swapped);
        assert!(own > 4.0, "trained perspective converges, got {own}");
        assert_ne!(own, other, "asymmetric position: views must differ");
    }

    #[test]
    fn test_adam_robust_where_sgd_diverges() {
        // Adam's practical advantage is robustness to the learning rate.
        // On the symmetric start position Σk² = 216, so SGD with lr = 0.02
        // has contraction factor |1 - 0.02*216| = 3.3 > 1: it diverges.
        // Adam with the same lr stays bounded (per-cell steps are capped at
        // ~lr regardless of gradient scale) and converges.
        let b = Board::new();
        let target = 8.0f32;
        let lr = 0.02f32;

        let mut sgd = Evaluator::new(EGAROUCID_PATTERNS);
        for _ in 0..30 {
            sgd.update_weights(&b, target, lr);
        }
        let sgd_residual = (target - sgd.eval(&b)).abs();
        assert!(
            sgd_residual > 100.0,
            "SGD at lr=0.02 must diverge on the symmetric position, residual {sgd_residual}"
        );

        let mut adam_eval = Evaluator::new(EGAROUCID_PATTERNS);
        let mut opt = AdamOptimizer::new(lr);
        for _ in 0..100 {
            adam_eval.update_weights_adam(&b, target, &mut opt);
        }
        let adam_residual = (target - adam_eval.eval(&b)).abs();
        // Adam's steady-state oscillation is bounded by ~(active cells * lr)
        let band = 65.0 * lr * 2.0;
        assert!(
            adam_residual < band,
            "Adam at the same lr stays convergent, residual {adam_residual} (band {band})"
        );
    }

    #[test]
    fn test_symmetry_augmented_training_generalizes() {
        // Training with `train` (8 symmetries) must make the evaluator score
        // a rotated variant identically to the original — without ever
        // having seen the rotation as a separate sample.
        let mut b = Board::new();
        let pos = Position::from_index(b.movable().trailing_zeros()).unwrap();
        b.make_move_unchecked(pos);

        // Small lr: Adam's steady-state oscillation is ~lr * active cells,
        // so lr = 0.01 keeps the residual band at ~0.64.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let mut opt = AdamOptimizer::new(0.01);
        for _ in 0..300 {
            e.train(&b, 6.0, &mut opt);
        }

        let syms = b.symmetries();
        let base = e.eval(&syms[0]);
        assert!((6.0 - base).abs() < 1.0, "training target reached, got {base}");
        // The 8 views are updated sequentially inside `train`, so at any
        // instant they differ by at most Adam's steady-state oscillation.
        for (i, sym) in syms.iter().enumerate() {
            let v = e.eval(sym);
            assert!(
                (v - base).abs() < 0.5,
                "symmetry {i} evaluates to {v}, base {base}: all views must
                 agree within the optimizer's oscillation band"
            );
            assert!(
                (6.0 - v).abs() < 1.0,
                "symmetry {i} must also be near the target, got {v}"
            );
        }
    }

    #[test]
    fn test_train_game_monte_carlo_labels_all_stages() {
        // λ=1 labels every recorded position with the final outcome
        // (sign-adjusted per side to move). After training, each recorded
        // position must evaluate toward ±score in its own perspective.
        let mut board = Board::new();
        let mut history = vec![board];
        for _ in 0..6 {
            let moves = board.movable();
            if moves == 0 {
                break;
            }
            board.make_move_unchecked(Position::from_index(moves.trailing_zeros()).unwrap());
            history.push(board);
        }
        let final_score = 10.0f32; // pretend Black wins by 10

        // lr sizing: Adam's steady-state swing is ~(active cells + 8 num
        // steps) * lr = 72 * lr per position; 0.03 keeps it inside the
        // ±3.0 tolerance below.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let mut opt = AdamOptimizer::new(0.03);
        let mut last_err = f32::MAX;
        for _ in 0..200 {
            last_err = e.train_game(&history, final_score, 1.0, &mut opt);
        }
        assert!(last_err < 2.0, "game training converges, err {last_err}");

        for b in &history {
            let v = e.eval(b);
            let expected = if b.player() == crate::color::Color::Black {
                final_score
            } else {
                -final_score
            };
            assert!(
                (v - expected).abs() < 3.0,
                "position (stage {}) evaluates {v}, expected ~{expected}",
                Evaluator::stage(b)
            );
        }
    }

    #[test]
    fn test_train_game_td_bootstrap_direction() {
        // λ=0 (pure TD): early positions bootstrap from later evaluations.
        // With a winning outcome, all positions must still drift positive
        // for Black because credit flows backward through the chain.
        let mut board = Board::new();
        let mut history = vec![board];
        for _ in 0..4 {
            let moves = board.movable();
            if moves == 0 {
                break;
            }
            board.make_move_unchecked(Position::from_index(moves.trailing_zeros()).unwrap());
            history.push(board);
        }

        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let mut opt = AdamOptimizer::new(0.05);
        for _ in 0..60 {
            e.train_game(&history, 12.0, 0.0, &mut opt);
        }

        // The first position is Black to move; its value must have moved
        // clearly positive via bootstrap alone.
        let first = &history[0];
        assert_eq!(first.player(), crate::color::Color::Black);
        assert!(
            e.eval(first) > 1.0,
            "TD(0) must propagate the win backward, got {}",
            e.eval(first)
        );
    }

    #[test]
    fn test_eval_indices_bit_exact_with_eval() {
        // The incremental path must return the *bit-identical* f32 as the
        // recomputing path (same weights summed in the same order), for both
        // sides to move, at every position of a random game.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let mut state = 0x2545f4914f6cdd1du64;
        for stage in 0..STAGE_COUNT {
            for (pi, p) in EGAROUCID_PATTERNS.iter().enumerate() {
                for idx in 0..p.table_size() {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    e.set_weight(stage, pi, idx, ((state >> 40) as i32 % 256) as f32 / 32.0);
                }
            }
        }

        let indexer = e.indexer();
        let mut board = Board::new();
        let mut indices = indexer.init(board.black, board.white);
        let mut rng = 7u64;
        loop {
            let moves = board.movable();
            if moves == 0 {
                let mut p = board;
                p.pass();
                if p.movable() == 0 {
                    break;
                }
                board = p;
                continue;
            }
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut nth = (rng >> 33) % moves.count_ones() as u64;
            let mut m = moves;
            while nth > 0 {
                m &= m - 1;
                nth -= 1;
            }
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            let mover = board.player();
            let mut next = board;
            let flipped = next.make_move_bits(pos);
            indexer.apply(&mut indices, pos, flipped, mover);
            board = next;

            assert_eq!(
                e.eval_indices(&board, &indices),
                e.eval(&board),
                "incremental eval diverged at stage {}",
                Evaluator::stage(&board)
            );
            let mut swapped = board;
            swapped.pass();
            assert_eq!(
                e.eval_indices(&swapped, &indices),
                e.eval(&swapped),
                "incremental eval diverged for the passing side"
            );
        }
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
    fn test_num_weights_roundtrip_and_v1_compat() {
        let dir = std::env::temp_dir().join("bbrv_weight_test_v2");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("weights_v2.bin");

        // Train one step so the disc-count cell becomes nonzero.
        let mut e = Evaluator::new(EGAROUCID_PATTERNS);
        let b = Board::new();
        e.update_weights(&b, 10.0, 0.005);
        let expected = e.eval(&b);
        e.save_weights(&path).unwrap();

        // v2 roundtrip preserves the disc-count contribution exactly.
        let mut e2 = Evaluator::new(EGAROUCID_PATTERNS);
        e2.load_weights(&path).unwrap();
        assert_eq!(e2.eval(&b), expected, "v2 roundtrip must be lossless");

        // A v1 file is the same bytes minus the trailing disc-count tables
        // and with the old magic; it must load with zeroed num weights.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[..8].copy_from_slice(b"BBRVWT01");
        bytes.truncate(bytes.len() - STAGE_COUNT * 65 * 4);
        let v1_path = dir.join("weights_v1.bin");
        std::fs::write(&v1_path, bytes).unwrap();

        let mut e3 = Evaluator::new(EGAROUCID_PATTERNS);
        e3.load_weights(&v1_path).unwrap();
        let delta = expected - e3.eval(&b);
        // Difference is exactly the (single) trained disc-count cell.
        assert!(
            delta.abs() > 0.0,
            "v1 load must zero the disc-count weights"
        );
        assert!(
            (delta - 0.005 * 10.0).abs() < 1e-4,
            "missing term is one SGD step on the num cell, got {delta}"
        );

        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&v1_path).ok();
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
