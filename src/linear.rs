//! Stage-based pattern evaluator with training support.

#[cfg(feature = "gpu")]
pub mod gpu;

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

const I8_SCALE: f32 = 4.0;

use crate::board::Board;
use crate::color::Color;
use crate::pattern::Pattern;
use crate::pattern_index::{PatternIndexer, PatternIndices};

pub const STAGE_COUNT: usize = 61;

const WEIGHT_MAGIC: &[u8; 8] = b"BBRVWT02";
const WEIGHT_MAGIC_V1: &[u8; 8] = b"BBRVWT01";

pub const NUM_TABLE_SIZE: usize = 65;

pub struct Linear {
    patterns: &'static [Pattern],
    weights: Vec<Vec<Vec<f32>>>,
    flat_weights: Vec<Vec<f32>>,
    mask_off: Vec<u32>,
    flat_i8: Vec<Vec<i8>>,
    flat_i8_white: Vec<Vec<i8>>,
    num_weights: Vec<[f32; NUM_TABLE_SIZE]>,
    appear: Vec<Vec<Vec<u32>>>,
    appear_num: Vec<[u32; NUM_TABLE_SIZE]>,
    min_appear: u32,
    indexer: PatternIndexer,
}

pub struct LinearView {
    stages: Vec<Vec<*mut f32>>,
    num: Vec<*mut f32>,
    counts: Vec<Vec<*const u32>>,
    counts_num: Vec<*const u32>,
}

unsafe impl Send for LinearView {}
unsafe impl Sync for LinearView {}

impl Linear {
    pub fn weight_view(&mut self) -> LinearView {
        self.flat_weights.clear();
        self.flat_i8.clear();
        self.flat_i8_white.clear();
        let counts = if self.appear.is_empty() {
            Vec::new()
        } else {
            self.appear
                .iter()
                .map(|st| st.iter().map(|t| t.as_ptr()).collect())
                .collect()
        };
        LinearView {
            counts,
            stages: self
                .weights
                .iter_mut()
                .map(|st| st.iter_mut().map(|t| t.as_mut_ptr()).collect())
                .collect(),
            num: self
                .num_weights
                .iter_mut()
                .map(|t| t.as_mut_ptr())
                .collect(),
            counts_num: self.appear_num.iter().map(|t| t.as_ptr()).collect(),
        }
    }

    /// # Safety
    ///
    /// Callers must not write the same weight cell concurrently.
    pub unsafe fn train_shared(
        &self,
        view: &LinearView,
        board: &Board,
        target: f32,
        lr: f32,
    ) -> f32 {
        let mut total = 0.0f32;
        for sym in board.symmetries() {
            let stage = Self::stage(&sym);
            let mut prediction = 0.0f32;
            for (pi, p) in self.patterns.iter().enumerate() {
                let table = view.stages[stage][pi];
                for idx in p.indices(sym.black, sym.white, sym.player()) {
                    prediction += *table.add(idx);
                }
            }
            let num_idx = self.num_index(&sym);
            prediction += *view.num[stage].add(num_idx);

            let error = target - prediction;
            let delta = lr * error;
            let min_appear = self.min_appear;
            if view.counts.is_empty() {
                for (pi, p) in self.patterns.iter().enumerate() {
                    let table = view.stages[stage][pi];
                    for idx in p.indices(sym.black, sym.white, sym.player()) {
                        *table.add(idx) += delta;
                    }
                }
            } else {
                for (pi, p) in self.patterns.iter().enumerate() {
                    let table = view.stages[stage][pi];
                    let counts = view.counts[stage][pi];
                    for idx in p.indices(sym.black, sym.white, sym.player()) {
                        let n = *counts.add(idx);
                        if n > min_appear {
                            *table.add(idx) += delta / n as f32;
                        }
                    }
                }
            }
            if view.counts_num.is_empty() {
                *view.num[stage].add(num_idx) += delta;
            } else {
                let n = *view.counts_num[stage].add(num_idx);
                if n > min_appear {
                    *view.num[stage].add(num_idx) += delta / n as f32;
                }
            }
            total += error * error;
        }
        total / 8.0
    }
}

pub trait Optimizer {
    fn step(
        &mut self,
        stage: usize,
        pattern: usize,
        index: usize,
        table_size: usize,
        grad: f32,
    ) -> f32;

    fn next_epoch(&mut self) {}

    fn set_lr(&mut self, _lr: f32) {}
}

pub struct SgdOptimizer {
    pub learning_rate: f32,
    pub decay: f32,
}

impl SgdOptimizer {
    pub fn new(learning_rate: f32, decay: f32) -> SgdOptimizer {
        SgdOptimizer {
            learning_rate,
            decay,
        }
    }
}

impl Optimizer for SgdOptimizer {
    #[inline]
    fn step(
        &mut self,
        _stage: usize,
        _pattern: usize,
        _index: usize,
        _table_size: usize,
        grad: f32,
    ) -> f32 {
        self.learning_rate * grad
    }

    fn next_epoch(&mut self) {
        self.learning_rate *= self.decay;
    }

    fn set_lr(&mut self, lr: f32) {
        self.learning_rate = lr;
    }
}

type MomentCell = (f32, f32, u32);
type MomentTable = Option<Box<[MomentCell]>>;

pub struct AdamOptimizer {
    pub learning_rate: f32,
    pub beta1: f32,
    pub beta2: f32,
    pub epsilon: f32,
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
    #[inline]
    fn step(
        &mut self,
        stage: usize,
        pattern: usize,
        index: usize,
        table_size: usize,
        grad: f32,
    ) -> f32 {
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

    fn set_lr(&mut self, lr: f32) {
        self.learning_rate = lr;
    }
}

impl Linear {
    pub fn new(patterns: &'static [Pattern]) -> Linear {
        let stage_weights: Vec<Vec<f32>> = patterns
            .iter()
            .map(|p| vec![0.0f32; p.table_size()])
            .collect();
        Linear {
            patterns,
            weights: vec![stage_weights; STAGE_COUNT],
            flat_weights: Vec::new(),
            mask_off: Vec::new(),
            flat_i8: Vec::new(),
            flat_i8_white: Vec::new(),
            num_weights: vec![[0.0f32; NUM_TABLE_SIZE]; STAGE_COUNT],
            appear: Vec::new(),
            appear_num: Vec::new(),
            min_appear: 0,
            indexer: PatternIndexer::new(patterns),
        }
    }

    pub fn patterns(&self) -> &'static [Pattern] {
        self.patterns
    }

    pub fn indexer(&self) -> &PatternIndexer {
        &self.indexer
    }

    pub fn stage(board: &Board) -> usize {
        60usize
            .saturating_sub(board.empty_count() as usize)
            .min(STAGE_COUNT - 1)
    }

    pub fn count_appearances(&mut self, boards: impl Iterator<Item = Board>) {
        if self.appear.is_empty() {
            self.appear = self
                .weights
                .iter()
                .map(|st| st.iter().map(|t| vec![0u32; t.len()]).collect())
                .collect();
            self.appear_num = vec![[0u32; NUM_TABLE_SIZE]; STAGE_COUNT];
        }
        for b in boards {
            for sym in b.symmetries() {
                let stage = Self::stage(&sym);
                let ni = self.num_index(&sym);
                for (pi, p) in self.patterns.iter().enumerate() {
                    for idx in p.indices(sym.black, sym.white, sym.player()) {
                        self.appear[stage][pi][idx] += 1;
                    }
                }
                self.appear_num[stage][ni] += 1;
            }
        }
    }

    pub fn appearance_spread(&self, stage: usize) -> Option<(usize, usize, [u32; 5])> {
        let st = self.appear.get(stage)?;
        let mut seen: Vec<u32> = st.iter().flatten().copied().filter(|&n| n > 0).collect();
        let unseen = st.iter().flatten().filter(|&&n| n == 0).count();
        if seen.is_empty() {
            return None;
        }
        seen.sort_unstable();
        let at = |f: f64| seen[((seen.len() - 1) as f64 * f) as usize];
        Some((
            seen.len(),
            unseen,
            [at(0.0), at(0.01), at(0.5), at(0.99), at(1.0)],
        ))
    }

    pub fn set_min_appear(&mut self, n: u32) {
        self.min_appear = n;
    }

    pub fn has_appearances(&self) -> bool {
        !self.appear.is_empty()
    }

    pub fn stage_weights(&self, stage: usize) -> (Vec<Vec<f32>>, Vec<f32>) {
        (
            self.weights[stage].clone(),
            self.num_weights[stage].to_vec(),
        )
    }

    pub fn set_stage_weights(&mut self, stage: usize, w: &[Vec<f32>], num: &[f32]) {
        self.weights[stage].clone_from_slice(w);
        self.num_weights[stage].copy_from_slice(num);
    }

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

    pub fn flat_all(&self) -> Vec<f32> {
        self.flat_stages(0, STAGE_COUNT - 1)
    }

    pub fn flat_stages(&self, lo: usize, hi: usize) -> Vec<f32> {
        let stride: usize =
            self.patterns.iter().map(|p| p.table_size()).sum::<usize>() + NUM_TABLE_SIZE;
        let mut out = Vec::with_capacity(stride * (hi + 1 - lo));
        for s in lo..=hi {
            for t in &self.weights[s] {
                out.extend_from_slice(t);
            }
            out.extend_from_slice(&self.num_weights[s]);
        }
        out
    }

    pub fn set_flat_all(&mut self, v: &[f32]) {
        self.set_flat_stages(0, STAGE_COUNT - 1, v)
    }

    pub fn set_flat_stages(&mut self, lo: usize, hi: usize, v: &[f32]) {
        let mut k = 0usize;
        for s in lo..=hi {
            for t in self.weights[s].iter_mut() {
                let n = t.len();
                t.copy_from_slice(&v[k..k + n]);
                k += n;
            }
            self.num_weights[s].copy_from_slice(&v[k..k + NUM_TABLE_SIZE]);
            k += NUM_TABLE_SIZE;
        }
        self.flat_weights.clear();
    }

    #[inline]
    fn num_index(&self, board: &Board) -> usize {
        board.player_bb().count_ones() as usize
    }

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
        let n_masks = self.indexer.n_masks();
        self.flat_i8 = self
            .flat_weights
            .iter()
            .map(|stage| {
                stage
                    .iter()
                    .map(|&w| (w * I8_SCALE).clamp(-128.0, 127.0) as i8)
                    .collect()
            })
            .collect();
        self.flat_i8_white = self
            .flat_i8
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

    fn invalidate_flat(&mut self) {
        self.flat_weights.clear();
        self.flat_i8.clear();
        self.flat_i8_white.clear();
        self.mask_off.clear();
    }

    #[inline(never)]
    pub fn eval_order_bb(
        &self,
        player: u64,
        opponent: u64,
        color: Color,
        indices: &PatternIndices,
    ) -> f32 {
        let empties = 64 - (player | opponent).count_ones() as usize;
        let stage = 60usize.saturating_sub(empties).min(STAGE_COUNT - 1);
        if self.flat_i8.is_empty() {
            let mut b = Board::new();
            b.black = if matches!(color, Color::Black) {
                player
            } else {
                opponent
            };
            b.white = if matches!(color, Color::Black) {
                opponent
            } else {
                player
            };
            b.player = color;
            b.empty_count = empties as u8;
            return self.eval_indices(&b, indices);
        }
        let table = if matches!(color, Color::Black) {
            &self.flat_i8[stage]
        } else {
            &self.flat_i8_white[stage]
        };
        let sum = self.indexer.eval_sum_i8(indices, table, &self.mask_off);
        sum as f32 / I8_SCALE + self.num_weights[stage][player.count_ones() as usize]
    }

    pub fn weight(&self, stage: usize, pattern: usize, index: usize) -> f32 {
        self.weights[stage][pattern][index]
    }

    pub fn set_weight(&mut self, stage: usize, pattern: usize, index: usize, value: f32) {
        self.invalidate_flat();
        self.weights[stage][pattern][index] = value;
    }

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
        let num_idx = self.num_index(board);
        let delta = opt.step(stage, self.patterns.len(), num_idx, NUM_TABLE_SIZE, error);
        self.num_weights[stage][num_idx] += delta;
        error
    }

    pub fn update_weights_adam(
        &mut self,
        board: &Board,
        target: f32,
        opt: &mut AdamOptimizer,
    ) -> f32 {
        self.update_weights_with(board, target, opt)
    }

    pub fn train(&mut self, board: &Board, target: f32, opt: &mut impl Optimizer) -> f32 {
        let mut total_sq_err = 0.0f32;
        for sym in board.symmetries() {
            let e = self.update_weights_with(&sym, target, opt);
            total_sq_err += e * e;
        }
        total_sq_err / 8.0
    }

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

        let mut total_sq_err = 0.0f32;
        let mut next_value_black_view = final_score;

        for board in history.iter().rev() {
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
            total_sq_err += self.train(board, target, opt);

            let v = self.eval(board);
            next_value_black_view = if board.player() == crate::color::Color::Black {
                v
            } else {
                -v
            };
        }

        total_sq_err / history.len() as f32
    }

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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "stage count mismatch",
            ));
        }
        r.read_exact(&mut u32buf)?;
        if u32::from_le_bytes(u32buf) as usize != self.patterns.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pattern count mismatch",
            ));
        }
        for p in self.patterns {
            r.read_exact(&mut u32buf)?;
            if u32::from_le_bytes(u32buf) as usize != p.table_size() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "table size mismatch",
                ));
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
    use crate::pattern::LINEAR_PATTERNS;
    use crate::position::Position;

    #[test]
    fn test_stage() {
        let b = Board::new();
        assert_eq!(Linear::stage(&b), 0, "initial board is stage 0");

        let mut b = Board::new();
        let first = b.movable().trailing_zeros();
        b.make_move_unchecked(Position::from_index(first).unwrap());
        assert_eq!(Linear::stage(&b), 1, "one move played -> stage 1");
    }

    #[test]
    fn test_eval_zero_weights() {
        let e = Linear::new(LINEAR_PATTERNS);
        let b = Board::new();
        assert_eq!(e.eval(&b), 0.0);
    }

    fn gradient_multiplier(board: &Board) -> f32 {
        use std::collections::HashMap;
        let mut counts: HashMap<(usize, usize), u32> = HashMap::new();
        for (pi, p) in LINEAR_PATTERNS.iter().enumerate() {
            for idx in p.indices(board.black, board.white, board.player()) {
                *counts.entry((pi, idx)).or_insert(0) += 1;
            }
        }
        counts.values().map(|&k| (k * k) as f32).sum::<f32>() + 1.0
    }

    const LR: f32 = 0.005;

    #[test]
    fn test_update_weights_reduces_error() {
        let mut e = Linear::new(LINEAR_PATTERNS);
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
        let mut e = Linear::new(LINEAR_PATTERNS);
        let b = Board::new();
        for _ in 0..200 {
            e.update_weights(&b, 8.0, LR);
        }
        let final_err = (8.0 - e.eval(&b)).abs();
        assert!(
            final_err < 0.05,
            "must converge to target, residual {final_err}"
        );
    }

    #[test]
    fn test_update_gradient_magnitude_matches_linear_model() {
        for plies in [0, 1] {
            let mut b = Board::new();
            for _ in 0..plies {
                let pos =
                    crate::position::Position::from_index(b.movable().trailing_zeros()).unwrap();
                b.make_move_unchecked(pos);
            }

            let multiplier = gradient_multiplier(&b);
            if plies == 0 {
                assert_eq!(multiplier, 217.0, "symmetric start: Σk² + 1 = 217");
            } else {
                assert_eq!(
                    multiplier, 193.0,
                    "first move keeps a mirror: Σk² + 1 = 193"
                );
            }

            let mut e = Linear::new(LINEAR_PATTERNS);
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
        let mut e = Linear::new(LINEAR_PATTERNS);
        let b = Board::new();
        assert_eq!(Linear::stage(&b), 0);
        e.update_weights(&b, 5.0, LR);

        let mut later = b;
        let pos = crate::position::Position::from_index(later.movable().trailing_zeros()).unwrap();
        later.make_move_unchecked(pos); // now stage 1
        assert_eq!(Linear::stage(&later), 1);
        assert_eq!(e.eval(&later), 0.0, "stage-1 weights must be untouched");
    }

    #[test]
    fn test_perspective_antisymmetry_after_training() {
        let mut e = Linear::new(LINEAR_PATTERNS);
        let b = Board::new();
        for _ in 0..300 {
            e.update_weights(&b, 8.0, LR);
        }
        let mut swapped = b;
        swapped.pass();
        let own = e.eval(&b);
        let other = e.eval(&swapped);
        assert!(own > 4.0, "trained side evaluates high, got {own}");
        assert_eq!(own, other, "color-symmetric position evaluates equally");
    }

    #[test]
    fn test_asymmetric_position_perspectives_differ() {
        let mut b = Board::new();
        let pos = crate::position::Position::from_index(b.movable().trailing_zeros()).unwrap();
        b.make_move_unchecked(pos);

        let mut e = Linear::new(LINEAR_PATTERNS);
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
        let b = Board::new();
        let target = 8.0f32;
        let lr = 0.02f32;

        let mut sgd = Linear::new(LINEAR_PATTERNS);
        for _ in 0..30 {
            sgd.update_weights(&b, target, lr);
        }
        let sgd_residual = (target - sgd.eval(&b)).abs();
        assert!(
            sgd_residual > 100.0,
            "SGD at lr=0.02 must diverge on the symmetric position, residual {sgd_residual}"
        );

        let mut adam_eval = Linear::new(LINEAR_PATTERNS);
        let mut opt = AdamOptimizer::new(lr);
        for _ in 0..100 {
            adam_eval.update_weights_adam(&b, target, &mut opt);
        }
        let adam_residual = (target - adam_eval.eval(&b)).abs();
        let band = 65.0 * lr * 2.0;
        assert!(
            adam_residual < band,
            "Adam at the same lr stays convergent, residual {adam_residual} (band {band})"
        );
    }

    #[test]
    fn test_symmetry_augmented_training_generalizes() {
        let mut b = Board::new();
        let pos = Position::from_index(b.movable().trailing_zeros()).unwrap();
        b.make_move_unchecked(pos);

        let mut e = Linear::new(LINEAR_PATTERNS);
        let mut opt = AdamOptimizer::new(0.01);
        for _ in 0..300 {
            e.train(&b, 6.0, &mut opt);
        }

        let syms = b.symmetries();
        let base = e.eval(&syms[0]);
        assert!(
            (6.0 - base).abs() < 1.0,
            "training target reached, got {base}"
        );
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

        let mut e = Linear::new(LINEAR_PATTERNS);
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
                Linear::stage(b)
            );
        }
    }

    #[test]
    fn test_train_game_td_bootstrap_direction() {
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

        let mut e = Linear::new(LINEAR_PATTERNS);
        let mut opt = AdamOptimizer::new(0.05);
        for _ in 0..60 {
            e.train_game(&history, 12.0, 0.0, &mut opt);
        }

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
        let mut e = Linear::new(LINEAR_PATTERNS);
        let mut state = 0x2545f4914f6cdd1du64;
        for stage in 0..STAGE_COUNT {
            for (pi, p) in LINEAR_PATTERNS.iter().enumerate() {
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
                Linear::stage(&board)
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

    fn step(e: &mut Linear, b: &Board) {
        let view = e.weight_view();
        unsafe { e.train_shared(&view, b, 10.0, 0.005) };
    }

    #[test]
    fn test_cell_lr_shrinks_the_step_and_skips_unseen() {
        let b = Board::new();
        let mut plain = Linear::new(LINEAR_PATTERNS);
        step(&mut plain, &b);

        let mut scaled = Linear::new(LINEAR_PATTERNS);
        scaled.count_appearances(std::iter::once(b));
        assert!(scaled.has_appearances());
        step(&mut scaled, &b);

        let st = Linear::stage(&b);
        let (pw, _) = plain.stage_weights(st);
        let (sw, _) = scaled.stage_weights(st);
        let mut moved = 0usize;
        for (pi, table) in pw.iter().enumerate() {
            for (ci, &v) in table.iter().enumerate() {
                let scaled_v = sw[pi][ci];
                if v == 0.0 {
                    assert_eq!(scaled_v, 0.0, "unseen cell must stay put");
                    continue;
                }
                moved += 1;
                assert!(
                    scaled_v * v > 0.0,
                    "scaled step must keep the sign: {v} vs {scaled_v}"
                );
                assert!(
                    scaled_v.abs() < v.abs(),
                    "a cell seen many times must take a shorter step: \
                     {v} vs {scaled_v}"
                );
            }
        }
        assert!(moved > 0, "the step must have moved something");
    }

    #[test]
    fn test_min_appear_freezes_thin_cells() {
        let b = Board::new();
        let mut e = Linear::new(LINEAR_PATTERNS);
        e.count_appearances(std::iter::once(b));
        e.set_min_appear(u32::MAX);
        let before = e.eval(&b);
        step(&mut e, &b);
        assert_eq!(
            e.eval(&b),
            before,
            "cells under the threshold must not move"
        );

        e.set_min_appear(0);
        step(&mut e, &b);
        assert_ne!(e.eval(&b), before, "cells over the threshold must move");
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = std::env::temp_dir().join("bbrv_weight_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("weights.bin");

        let mut e = Linear::new(LINEAR_PATTERNS);
        e.set_weight(0, 0, 42, 1.25);
        e.set_weight(60, 15, 7, -3.5);
        e.save_weights(&path).unwrap();

        let mut e2 = Linear::new(LINEAR_PATTERNS);
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

        let mut e = Linear::new(LINEAR_PATTERNS);
        let b = Board::new();
        e.update_weights(&b, 10.0, 0.005);
        let expected = e.eval(&b);
        e.save_weights(&path).unwrap();

        let mut e2 = Linear::new(LINEAR_PATTERNS);
        e2.load_weights(&path).unwrap();
        assert_eq!(e2.eval(&b), expected, "v2 roundtrip must be lossless");

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[..8].copy_from_slice(b"BBRVWT01");
        bytes.truncate(bytes.len() - STAGE_COUNT * 65 * 4);
        let v1_path = dir.join("weights_v1.bin");
        std::fs::write(&v1_path, bytes).unwrap();

        let mut e3 = Linear::new(LINEAR_PATTERNS);
        e3.load_weights(&v1_path).unwrap();
        let delta = expected - e3.eval(&b);
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
        let path = dir.join("weights_linear.bin");

        let e = Linear::new(LINEAR_PATTERNS);
        e.save_weights(&path).unwrap();

        let mut other = Linear::new(crate::pattern::NNUE_PATTERNS);
        assert!(
            other.load_weights(&path).is_err(),
            "loading one set's weights into another set's linear must fail"
        );

        std::fs::remove_file(&path).ok();
    }
}
