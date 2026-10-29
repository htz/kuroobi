//! Training examples: loading records, symmetric forms, and the linear evaluator's epoch loop.

use std::io;
use std::path::Path;

use crate::bitboard::symmetry;
use crate::board::Board;
use crate::color::Color;
use crate::linear::{Linear, Optimizer, STAGE_COUNT};
use crate::record::{self, Filter, TeacherPolicy};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Example {
    pub black: u64,
    pub white: u64,
    pub score: f32,
}

#[derive(Clone, Copy, Default)]
pub struct SymPlan {
    pub seed: u64,
    pub fixed: Option<u8>,
}

impl SymPlan {
    pub fn apply(&self, ex: &Example, rs: &mut u64) -> Example {
        let i = match self.fixed {
            Some(i) => i,
            None if self.seed == 0 => return *ex,
            None => {
                *rs ^= *rs >> 12;
                *rs ^= *rs << 25;
                *rs ^= *rs >> 27;
                (rs.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 61) as u8
            }
        };
        if i == 0 {
            return *ex;
        }
        Example {
            black: symmetry(ex.black, i),
            white: symmetry(ex.white, i),
            score: ex.score,
        }
    }

    pub fn stream(&self, salt: u64) -> u64 {
        self.seed ^ 0x9E37_79B9_7F4A_7C15u64.wrapping_mul(salt)
    }
}

impl Example {
    pub fn board(&self) -> Board {
        Board {
            black: self.black,
            white: self.white,
            player: Color::Black,
            empty_count: 64 - (self.black | self.white).count_ones() as u8,
        }
    }
}

/// Every record of the file, unfiltered, under the default teacher rule.
pub fn load_examples_binary(path: &Path) -> io::Result<Vec<Example>> {
    let mut examples = Vec::new();
    load_examples_filtered_into(
        path,
        &mut examples,
        None,
        &Filter::NONE,
        &TeacherPolicy::DEFAULT,
    )?;
    Ok(examples)
}

pub fn load_examples_filtered_into(
    path: &Path,
    out: &mut Vec<Example>,
    limit: Option<usize>,
    filter: &Filter,
    policy: &TeacherPolicy,
) -> io::Result<usize> {
    let in_file = record::count(path)?;
    let want = limit.map_or(in_file, |l| l.min(in_file));
    out.reserve(want);
    let mut n = 0usize;
    record::for_each(path, |r| {
        if n >= want {
            return false;
        }
        if filter.keeps(&r) {
            out.push(r.example_with(policy));
            n += 1;
        }
        true
    })?;
    Ok(n)
}

pub fn load_examples_range_into(
    path: &Path,
    out: &mut Vec<Example>,
    start: usize,
    len: usize,
    filter: &Filter,
    policy: &TeacherPolicy,
) -> io::Result<usize> {
    out.reserve(len);
    let mut n = 0usize;
    record::for_each_range(path, start, len, |r| {
        if filter.keeps(&r) {
            out.push(r.example_with(policy));
            n += 1;
        }
        true
    })?;
    Ok(n)
}

#[derive(Debug, Clone)]
pub struct EpochStats {
    pub loss_sum: [f64; STAGE_COUNT],
    pub samples: [u64; STAGE_COUNT],
}

impl Default for EpochStats {
    fn default() -> Self {
        EpochStats {
            loss_sum: [0.0; STAGE_COUNT],
            samples: [0; STAGE_COUNT],
        }
    }
}

impl EpochStats {
    pub fn mse(&self) -> f64 {
        let total: f64 = self.loss_sum.iter().sum();
        let n: u64 = self.samples.iter().sum();
        if n == 0 {
            0.0
        } else {
            total / n as f64
        }
    }

    pub fn add(&mut self, other: &EpochStats) {
        for stage in 0..STAGE_COUNT {
            self.loss_sum[stage] += other.loss_sum[stage];
            self.samples[stage] += other.samples[stage];
        }
    }

    pub fn stage_mse(&self, stage: usize) -> f64 {
        if self.samples[stage] == 0 {
            0.0
        } else {
            self.loss_sum[stage] / self.samples[stage] as f64
        }
    }
}

/// The held-out number that picks a snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SelectBy {
    Mse,
    Mae,
    /// The error with the constant offset taken out.
    Spread,
}

impl std::str::FromStr for SelectBy {
    type Err = String;

    fn from_str(s: &str) -> Result<SelectBy, String> {
        match s {
            "mse" => Ok(SelectBy::Mse),
            "mae" => Ok(SelectBy::Mae),
            "spread" => Ok(SelectBy::Spread),
            other => Err(format!("--select-by {other}: expected mse, mae or spread")),
        }
    }
}

impl std::fmt::Display for SelectBy {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(match self {
            SelectBy::Mse => "mse",
            SelectBy::Mae => "mae",
            SelectBy::Spread => "spread",
        })
    }
}

/// Sums of an evaluator's error over held-out positions.
#[derive(Clone, Copy, Debug, Default)]
pub struct ValError {
    pub n: f64,
    pub sq: f64,
    pub abs: f64,
    pub sum: f64,
}

impl ValError {
    pub fn add(&mut self, e: f64) {
        self.n += 1.0;
        self.sq += e * e;
        self.abs += e.abs();
        self.sum += e;
    }

    pub fn merge(&mut self, o: &ValError) {
        self.n += o.n;
        self.sq += o.sq;
        self.abs += o.abs;
        self.sum += o.sum;
    }

    /// (MSE, MAE, bias, spread); all NaN without positions.
    pub fn stats(&self) -> (f64, f64, f64, f64) {
        let n = self.n;
        if n == 0.0 {
            return (f64::NAN, f64::NAN, f64::NAN, f64::NAN);
        }
        let (mse, mae, bias) = (self.sq / n, self.abs / n, self.sum / n);
        (mse, mae, bias, (mse - bias * bias).max(0.0).sqrt())
    }

    pub fn score(&self, by: SelectBy) -> f64 {
        let (mse, mae, _, spread) = self.stats();
        match by {
            SelectBy::Mse => mse,
            SelectBy::Mae => mae,
            SelectBy::Spread => spread,
        }
    }
}

const PROGRESS_INTERVAL: usize = 1 << 16;

pub struct Trainer<O: Optimizer> {
    pub linear: Linear,
    pub optimizer: O,
}

impl<O: Optimizer> Trainer<O> {
    pub fn new(linear: Linear, optimizer: O) -> Trainer<O> {
        Trainer { linear, optimizer }
    }

    pub fn train_pass(
        &mut self,
        examples: &[Example],
        mut progress: impl FnMut(usize, usize),
    ) -> EpochStats {
        let total = examples.len();
        let mut stats = EpochStats::default();
        for (i, ex) in examples.iter().enumerate() {
            let board = ex.board();
            let stage = Linear::stage(&board);
            let mse = self.linear.train(&board, ex.score, &mut self.optimizer);
            stats.loss_sum[stage] += mse as f64;
            stats.samples[stage] += 1;

            if (i + 1) & (PROGRESS_INTERVAL - 1) == 0 {
                progress(i + 1, total);
            }
        }
        progress(total, total);
        stats
    }

    pub fn train_stage_epoch(
        &mut self,
        examples: &[Example],
        stage: usize,
        lr: f32,
        mut progress: impl FnMut(usize, usize),
    ) -> EpochStats {
        let view = self.linear.weight_view();
        let ev = &self.linear;
        let total = examples.len();
        let mut stats = EpochStats::default();
        for (i, ex) in examples.iter().enumerate() {
            let board = ex.board();
            if Linear::stage(&board) != stage {
                continue;
            }
            let mse = unsafe { ev.train_shared(&view, &board, ex.score, lr) };
            stats.loss_sum[stage] += mse as f64;
            stats.samples[stage] += 1;
            if (i + 1) & (PROGRESS_INTERVAL - 1) == 0 {
                progress(i + 1, total);
            }
        }
        progress(total, total);
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linear::{AdamOptimizer, SgdOptimizer};
    use crate::pattern::LINEAR_PATTERNS;
    use crate::position::Position;
    use crate::record::Record;

    #[test]
    fn sym_plan_fixed_walks_eight_distinct_forms() {
        let ex = Example {
            black: 1 << 1,
            white: 1 << 10,
            score: 3.0,
        };
        let mut rs = 0u64;
        let mut seen: Vec<(u64, u64)> = Vec::new();
        for i in 0..8u8 {
            let p = SymPlan {
                seed: 0,
                fixed: Some(i),
            };
            let e = p.apply(&ex, &mut rs);
            assert_eq!(e.score, ex.score, "rotation must not touch the label");
            assert_eq!(e.black.count_ones(), ex.black.count_ones());
            seen.push((e.black, e.white));
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 8, "the eight forms must differ");
    }

    #[test]
    fn sym_plan_default_is_the_identity() {
        let ex = Example {
            black: 0x0000_0008_1000_0000,
            white: 0x0000_0010_0800_0000,
            score: -2.0,
        };
        let mut rs = 12345u64;
        let e = SymPlan::default().apply(&ex, &mut rs);
        assert_eq!((e.black, e.white), (ex.black, ex.white));
        assert_eq!(rs, 12345, "no draw, no advance");
    }

    #[test]
    fn val_error_spread_drops_the_constant_offset() {
        let mut v = ValError::default();
        assert!(v.score(SelectBy::Mse).is_nan(), "no positions, no score");
        for e in [3.0, 3.0, 3.0] {
            v.add(e);
        }
        assert_eq!(v.stats(), (9.0, 3.0, 3.0, 0.0));
        assert_eq!("spread".parse::<SelectBy>(), Ok(SelectBy::Spread));
        assert!("rmse".parse::<SelectBy>().is_err());
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("kuroobi_trainer_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn searched(mover: u64, opponent: u64, result: i8) -> Record {
        Record {
            mover,
            opponent,
            score: f32::from(result),
            game_score: result,
            ply: 20,
            random: false,
            sq: record::NO_SQUARE,
            black_to_move: true,
            game_id: 0,
        }
    }

    fn write_records(path: &Path, records: &[Record]) {
        record::write_all(path, records).unwrap();
    }

    #[test]
    fn test_example_is_the_record_in_this_crates_layout() {
        let bin_path = temp_path("layout.data");
        let mut raw = searched(0, 0, 5).to_bytes();
        raw[0..8].copy_from_slice(&(1u64 << 1).to_le_bytes());
        std::fs::write(&bin_path, raw).unwrap();

        let ex = load_examples_binary(&bin_path).unwrap();
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].black, 1u64 << 8, "B1 on disk is bit 8 in memory");
        assert_eq!(ex[0].white, 0);
        assert_eq!(ex[0].score, 5.0);
        assert_eq!(ex[0].board().player, Color::Black);

        std::fs::remove_file(&bin_path).ok();
    }

    #[test]
    fn test_binary_load_applies_teacher_rule_and_filter() {
        let bin_path = temp_path("rule.data");
        let base = Record {
            mover: 0x0000_0008_1000_0000,
            opponent: 0x0000_0010_0800_0000,
            score: 3.0,
            game_score: 10,
            ply: 20,
            random: false,
            sq: record::NO_SQUARE,
            black_to_move: true,
            game_id: 1,
        };
        write_records(
            &bin_path,
            &[
                base, // searched: teacher = result
                Record {
                    random: true,
                    ..base
                }, // random: teacher = search value; dropped by the filter
                Record {
                    game_score: 30,
                    ..base
                }, // disagrees with the search by 27: dropped
                Record { ply: 5, ..base }, // before min ply: dropped
            ],
        );

        let all = load_examples_binary(&bin_path).unwrap();
        assert_eq!(
            all.iter().map(|e| e.score).collect::<Vec<_>>(),
            [10.0, 3.0, 30.0, 10.0]
        );
        let mut kept = Vec::new();
        let n = load_examples_filtered_into(
            &bin_path,
            &mut kept,
            None,
            &Filter::TRAINING,
            &TeacherPolicy::DEFAULT,
        )
        .unwrap();
        assert_eq!(n, 1);
        assert_eq!(kept[0].score, 10.0);

        std::fs::remove_file(&bin_path).ok();
    }

    #[test]
    fn test_binary_load_spans_blocks_and_appends() {
        let bin_path = temp_path("multiblock.data");
        let records: Vec<Record> = (0..10_000u64)
            .map(|i| {
                searched(
                    i.wrapping_mul(0x9E3779B97F4A7C15),
                    i.wrapping_mul(0xC2B2AE3D27D4EB4F),
                    (i % 128) as i8 - 64,
                )
            })
            .collect();
        write_records(&bin_path, &records);
        let written: Vec<Example> = records
            .iter()
            .map(|r| r.example_with(&TeacherPolicy::DEFAULT))
            .collect();

        assert_eq!(record::count(&bin_path).unwrap(), written.len());
        assert_eq!(load_examples_binary(&bin_path).unwrap(), written);

        let load = |out: &mut Vec<Example>, limit| {
            load_examples_filtered_into(
                &bin_path,
                out,
                limit,
                &Filter::NONE,
                &TeacherPolicy::DEFAULT,
            )
            .unwrap()
        };
        let mut out = vec![written[0]];
        assert_eq!(load(&mut out, None), written.len());
        assert_eq!(out.len(), written.len() + 1);
        assert_eq!(&out[1..], &written[..]);

        let mut capped = Vec::new();
        assert_eq!(load(&mut capped, Some(5_000)), 5_000);
        assert_eq!(capped, written[..5_000]);

        let mut over = Vec::new();
        assert_eq!(load(&mut over, Some(usize::MAX)), written.len());

        let mut range = Vec::new();
        let n = load_examples_range_into(
            &bin_path,
            &mut range,
            9_000,
            500,
            &Filter::NONE,
            &TeacherPolicy::DEFAULT,
        )
        .unwrap();
        assert_eq!(n, 500);
        assert_eq!(range, written[9_000..9_500]);

        std::fs::remove_file(&bin_path).ok();
    }

    #[test]
    fn test_train_pass_does_not_advance_lr_schedule() {
        let b = Board::new();
        let examples = [Example {
            black: b.black,
            white: b.white,
            score: 2.0,
        }];
        let mut trainer = Trainer::new(Linear::new(LINEAR_PATTERNS), SgdOptimizer::new(0.01, 0.5));
        let before = trainer.optimizer.learning_rate;
        trainer.train_pass(&examples, |_, _| {});
        assert_eq!(trainer.optimizer.learning_rate, before, "pass holds lr");
        trainer.optimizer.next_epoch();
        assert!(
            trainer.optimizer.learning_rate < before,
            "an epoch decays lr"
        );
    }

    #[test]
    fn test_trainer_epoch_reduces_loss() {
        let mut b = Board::new();
        let mut examples = Vec::new();
        for score in [4i8, -6, 10] {
            let pos = Position::from_index(b.movable().trailing_zeros()).unwrap();
            b.make_move_bits(pos);
            let (black, white, score) = if b.player() == Color::Black {
                (b.black, b.white, score)
            } else {
                (b.white, b.black, -score)
            };
            examples.push(Example {
                black,
                white,
                score: f32::from(score),
            });
        }

        let mut trainer = Trainer::new(Linear::new(LINEAR_PATTERNS), AdamOptimizer::new(0.01));
        let stats: Vec<EpochStats> = (0..60)
            .map(|_| {
                let st = trainer.train_pass(&examples, |_, _| {});
                trainer.optimizer.next_epoch();
                st
            })
            .collect();
        let first = stats.first().unwrap().mse();
        let last = stats.last().unwrap().mse();
        assert!(
            last < first * 0.05,
            "epoch training must reduce MSE by >95%: {first} -> {last}"
        );
    }

    #[test]
    fn test_trainer_stats_track_stages() {
        let b = Board::new();
        let examples = [Example {
            black: b.black,
            white: b.white,
            score: 2.0,
        }];
        let mut trainer = Trainer::new(Linear::new(LINEAR_PATTERNS), AdamOptimizer::new(0.01));
        let stats = trainer.train_pass(&examples, |_, _| {});
        assert_eq!(stats.samples[0], 1, "initial position is stage 0");
        assert_eq!(stats.samples[1..].iter().sum::<u64>(), 0);
        assert!(stats.stage_mse(0) > 0.0, "first-epoch loss is nonzero");
    }
}
