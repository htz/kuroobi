//! Record-based training: load positions and run epoch-style training with
//! symmetry augmentation (the Go trainer's workflow).
//!
//! The one input format is the record of [`crate::record`], from which the
//! teacher value is derived by the record's rule. In memory an example
//! is the mover's discs as Black, the opponent's as White, and the teacher
//! value from the mover's view.

use std::io;
use std::path::Path;

use crate::board::Board;
use crate::color::Color;
use crate::evaluator::{AdamOptimizer, Evaluator, Optimizer, STAGE_COUNT};
use crate::record::{self, Filter, TeacherPolicy};

/// One training position: bitboards plus the teacher value in discs.
/// Bit layout in memory is this crate's file-major; converters translate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Example {
    pub black: u64,
    pub white: u64,
    pub score: f32,
}

impl Example {
    /// Reconstruct the Board (Black to move, per the data convention).
    pub fn board(&self) -> Board {
        Board {
            black: self.black,
            white: self.white,
            player: Color::Black,
            empty_count: 64 - (self.black | self.white).count_ones() as u8,
        }
    }
}

/// Load examples from a record file.
pub fn load_examples_binary(path: &Path) -> io::Result<Vec<Example>> {
    let mut examples = Vec::new();
    load_examples_binary_into(path, &mut examples, None)?;
    Ok(examples)
}

/// Number of records a binary file holds, from its size alone — no read.
///
/// The format is fixed-width, so this is exact, which lets a caller plan how
/// many files fit in a memory budget before touching the data.
pub fn count_examples_binary(path: &Path) -> io::Result<usize> {
    record::count(path)
}

/// Append a record file's examples to `out`, stopping after `limit` of them.
pub fn load_examples_binary_into(
    path: &Path,
    out: &mut Vec<Example>,
    limit: Option<usize>,
) -> io::Result<usize> {
    load_examples_filtered_into(path, out, limit, &Filter::NONE, &TeacherPolicy::DEFAULT)
}

/// Append the examples of a record file that pass `filter` to `out`,
/// stopping after `limit` of them.
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

/// Append the examples of records `[start, start + len)` of a record file
/// that pass `filter` to `out`.
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

/// Per-stage loss statistics for one epoch.
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
    /// Mean squared error over all stages.
    pub fn mse(&self) -> f64 {
        let total: f64 = self.loss_sum.iter().sum();
        let n: u64 = self.samples.iter().sum();
        if n == 0 {
            0.0
        } else {
            total / n as f64
        }
    }

    /// Fold another pass's statistics in (an epoch spread over shards).
    pub fn add(&mut self, other: &EpochStats) {
        for stage in 0..STAGE_COUNT {
            self.loss_sum[stage] += other.loss_sum[stage];
            self.samples[stage] += other.samples[stage];
        }
    }

    /// Total number of examples seen.
    pub fn total_samples(&self) -> u64 {
        self.samples.iter().sum()
    }

    pub fn stage_mse(&self, stage: usize) -> f64 {
        if self.samples[stage] == 0 {
            0.0
        } else {
            self.loss_sum[stage] / self.samples[stage] as f64
        }
    }
}

/// Epoch trainer over labeled examples (kifu-derived positions),
/// generic over the optimizer (SgdOptimizer or AdamOptimizer).
pub struct Trainer<O: Optimizer = AdamOptimizer> {
    pub evaluator: Evaluator,
    pub optimizer: O,
}

impl<O: Optimizer> Trainer<O> {
    pub fn new(evaluator: Evaluator, optimizer: O) -> Trainer<O> {
        Trainer {
            evaluator,
            optimizer,
        }
    }

    /// Train one epoch over the examples (single pass, in order — shuffle
    /// upstream if desired). Uses 8-fold symmetry augmentation per example.
    /// Advances the optimizer's epoch schedule (e.g. SGD lr decay) at the
    /// end. Returns per-stage loss statistics.
    pub fn train_epoch(&mut self, examples: &[Example]) -> EpochStats {
        self.train_epoch_with_progress(examples, |_, _| {})
    }

    /// Like `train_epoch`, invoking `progress(done, total)` roughly every
    /// 64k examples (and once at the end) for progress reporting.
    pub fn train_epoch_with_progress(
        &mut self,
        examples: &[Example],
        progress: impl FnMut(usize, usize),
    ) -> EpochStats {
        let stats = self.train_pass(examples, progress);
        self.optimizer.next_epoch();
        stats
    }

    /// One pass over `examples` **without** advancing the optimizer's epoch
    /// schedule. An epoch split across several shards is several passes but
    /// one schedule step, so the lr decay must not fire per shard.
    pub fn train_pass(
        &mut self,
        examples: &[Example],
        mut progress: impl FnMut(usize, usize),
    ) -> EpochStats {
        // Power-of-two interval lets the hot loop use a cheap mask test.
        const PROGRESS_INTERVAL: usize = 1 << 16;

        let total = examples.len();
        let mut stats = EpochStats::default();
        for (i, ex) in examples.iter().enumerate() {
            let board = ex.board();
            let stage = Evaluator::stage(&board);
            // `train` already returns the mean squared error over the eight
            // symmetries, so accumulate it directly — squaring it again gives
            // (MSE)², which drifts away from the true loss as the model fits.
            let mse = self.evaluator.train(&board, ex.score, &mut self.optimizer);
            stats.loss_sum[stage] += mse as f64;
            stats.samples[stage] += 1;

            if (i + 1) & (PROGRESS_INTERVAL - 1) == 0 {
                progress(i + 1, total);
            }
        }
        progress(total, total);
        stats
    }

    /// One epoch over one stage, on this thread.
    ///
    /// There is no threaded variant and no multi-stage variant on purpose.
    /// The stages are independent tables, so a run aimed at one of them only
    /// ever has one stage's work to do -- measured here, a single stage held
    /// the machine at 39% of one core out of ten. Eight stages therefore want
    /// eight processes, not eight threads, and each of
    /// those reads only its own stage's file.
    ///
    /// Keeping a second, threaded path cost more than it bought: it read the
    /// per-stage rates and the per-cell appearance counts, the sequential one
    /// read neither, and anything added to the first was silently inert under
    /// `--threads 1`. Per-cell scaling was, and the raw rate it then trained
    /// at read as the scaling diverging.
    ///
    /// Examples from other stages are skipped rather than rejected: a corpus
    /// split by stage has none, but a mixed file stays usable.
    pub fn train_stage_epoch(
        &mut self,
        examples: &[Example],
        stage: usize,
        lr: f32,
        mut progress: impl FnMut(usize, usize),
    ) -> EpochStats {
        let view = self.evaluator.weight_view();
        let ev = &self.evaluator;
        let total = examples.len();
        let mut stats = EpochStats::default();
        for (i, ex) in examples.iter().enumerate() {
            let board = ex.board();
            if Evaluator::stage(&board) != stage {
                continue;
            }
            // SAFETY: single-threaded, and the view came from `ev`, borrowed
            // immutably for the rest of the call. train_shared returns MSE
            // already; see train_pass.
            let mse = unsafe { ev.train_shared(&view, &board, ex.score, lr) };
            stats.loss_sum[stage] += mse as f64;
            stats.samples[stage] += 1;
            if (i + 1) & ((1 << 16) - 1) == 0 {
                progress(i + 1, total);
            }
        }
        progress(total, total);
        stats
    }

    /// Run `epochs` passes, returning the stats of each epoch.
    pub fn run(&mut self, epochs: usize, examples: &[Example]) -> Vec<EpochStats> {
        (0..epochs).map(|_| self.train_epoch(examples)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::EGAROUCID_PATTERNS;
    use crate::position::Position;
    use crate::record::Record;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("bbrv_trainer_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// A record whose teacher value is its result, at a ply past the
    /// opening's forced zero.
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
        let mut w = record::Writer::create(path).unwrap();
        for r in records {
            w.write(r).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn test_example_is_the_record_in_this_crates_layout() {
        // On disk bits are rank-major (A1 = bit 0, B1 = bit 1); in memory an
        // example is file-major (B1 = bit 8), the mover as Black.
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
        let mut w = record::Writer::create(&bin_path).unwrap();
        w.write(&base).unwrap(); // searched: teacher = result
        w.write(&Record {
            random: true,
            ..base
        })
        .unwrap(); // random: teacher = search value; dropped by the filter
        w.write(&Record {
            game_score: 30,
            ..base
        })
        .unwrap(); // disagrees with the search by 27: dropped
        w.write(&Record { ply: 5, ..base }).unwrap(); // before min ply: dropped
        w.finish().unwrap();

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
        // More records than the loader's read block holds, so the multi-block
        // path (and its record-boundary bookkeeping) is exercised.
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
        let written: Vec<Example> = records.iter().map(Record::example).collect();

        assert_eq!(count_examples_binary(&bin_path).unwrap(), written.len());
        assert_eq!(load_examples_binary(&bin_path).unwrap(), written);

        // Appending must extend an existing buffer, not replace it.
        let mut out = vec![written[0]];
        let n = load_examples_binary_into(&bin_path, &mut out, None).unwrap();
        assert_eq!(n, written.len());
        assert_eq!(out.len(), written.len() + 1);
        assert_eq!(&out[1..], &written[..]);

        // A limit smaller than the file stops mid-stream, keeping the prefix.
        let mut capped = Vec::new();
        let n = load_examples_binary_into(&bin_path, &mut capped, Some(5_000)).unwrap();
        assert_eq!(n, 5_000);
        assert_eq!(capped, written[..5_000]);

        // A limit larger than the file is not an error.
        let mut over = Vec::new();
        let n = load_examples_binary_into(&bin_path, &mut over, Some(usize::MAX)).unwrap();
        assert_eq!(n, written.len());

        std::fs::remove_file(&bin_path).ok();
    }

    #[test]
    fn test_train_pass_does_not_advance_lr_schedule() {
        // An epoch split across shards is several passes but one schedule
        // step; if a pass advanced the schedule, lr would decay per shard.
        use crate::evaluator::SgdOptimizer;

        let b = Board::new();
        let examples = [Example {
            black: b.black,
            white: b.white,
            score: 2.0,
        }];
        let mut trainer = Trainer::new(
            Evaluator::new(EGAROUCID_PATTERNS),
            SgdOptimizer::new(0.01, 0.5),
        );
        let before = trainer.optimizer.learning_rate;
        trainer.train_pass(&examples, |_, _| {});
        assert_eq!(trainer.optimizer.learning_rate, before, "pass holds lr");
        trainer.train_epoch(&examples);
        assert!(
            trainer.optimizer.learning_rate < before,
            "an epoch decays lr"
        );
    }

    #[test]
    fn test_trainer_epoch_reduces_loss() {
        // A few mid-game positions labeled with distinct scores: epoch loss
        // must drop substantially across epochs.
        let mut b = Board::new();
        let mut examples = Vec::new();
        for score in [4i8, -6, 10] {
            let pos = Position::from_index(b.movable().trailing_zeros()).unwrap();
            b.make_move_unchecked(pos);
            // Normalize to Black to move (the data convention): if it's
            // White's turn, flip the color planes and negate the score.
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

        let mut trainer =
            Trainer::new(Evaluator::new(EGAROUCID_PATTERNS), AdamOptimizer::new(0.01));
        let stats = trainer.run(60, &examples);
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
        let mut trainer =
            Trainer::new(Evaluator::new(EGAROUCID_PATTERNS), AdamOptimizer::new(0.01));
        let stats = trainer.train_epoch(&examples);
        assert_eq!(stats.samples[0], 1, "initial position is stage 0");
        assert_eq!(stats.samples[1..].iter().sum::<u64>(), 0);
        assert!(stats.stage_mse(0) > 0.0, "first-epoch loss is nonzero");
    }
}
