//! Kifu/example-based training: load labeled positions and run epoch-style
//! training with symmetry augmentation (the Go trainer's workflow).
//!
//! Two input formats are supported:
//! - Text: one position per line, `<64 board chars> <score>` where the board
//!   uses 'X' (black), 'O' (white), anything else = empty, in **rank-major**
//!   order (the classic kifu dump layout, same as the Go converter input)
//! - Binary: packed `Example` records (black u64, white u64, score i8,
//!   little-endian, 17 bytes each) — rank-major bit layout for compatibility
//!   with data produced by the Go converter
//!
//! Scores are from **Black's perspective** with Black to move (the Go
//! pipeline stores positions normalized that way).

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::bitboard;
use crate::board::Board;
use crate::color::Color;
use crate::evaluator::{AdamOptimizer, Evaluator, Optimizer, STAGE_COUNT};

/// One labeled training position: bitboards plus the final-score label.
/// Bit layout in memory is this crate's file-major; converters translate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Example {
    pub black: u64,
    pub white: u64,
    pub score: i8,
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

/// Parse one text line: 64 board characters (rank-major), whitespace, score.
fn parse_text_line(line: &str) -> Option<Example> {
    let line = line.trim();
    if line.len() < 65 {
        return None;
    }
    let (board_part, score_part) = line.split_at(64);

    let mut black_rank_major = 0u64;
    let mut white_rank_major = 0u64;
    for (i, ch) in board_part.chars().enumerate() {
        match ch {
            'X' | 'x' | '*' => black_rank_major |= 1u64 << i,
            'O' | 'o' => white_rank_major |= 1u64 << i,
            _ => {}
        }
    }

    let score: i8 = score_part.trim().parse().ok()?;
    Some(Example {
        // transpose converts rank-major -> file-major
        black: bitboard::transpose(black_rank_major),
        white: bitboard::transpose(white_rank_major),
        score,
    })
}

/// Load examples from a text file (one `<board> <score>` per line).
/// Empty lines are skipped; malformed lines are an error.
pub fn load_examples_text(path: &Path) -> io::Result<Vec<Example>> {
    let mut examples = Vec::new();
    load_examples_text_into(path, &mut examples, None)?;
    Ok(examples)
}

/// Append a text file's examples to `out`, stopping after `limit` of them.
///
/// The appending form exists so a caller can fill one buffer from several
/// files without an intermediate `Vec` per file — with multi-gigabyte
/// datasets that temporary is the difference between fitting in RAM and not.
pub fn load_examples_text_into(
    path: &Path,
    out: &mut Vec<Example>,
    limit: Option<usize>,
) -> io::Result<usize> {
    let reader = BufReader::new(File::open(path)?);
    let mut n = 0;
    for (no, line) in reader.lines().enumerate() {
        if limit.is_some_and(|l| n >= l) {
            break;
        }
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let ex = parse_text_line(&line).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad training line {}", no + 1),
            )
        })?;
        out.push(ex);
        n += 1;
    }
    Ok(n)
}

/// Binary record: black u64 LE, white u64 LE, score i8 (17 bytes).
/// Bit layout on disk is rank-major (Go-converter compatible).
const BIN_RECORD_SIZE: usize = 17;

/// Save examples in the packed binary format.
pub fn save_examples_binary(path: &Path, examples: &[Example]) -> io::Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    for ex in examples {
        // file-major (memory) -> rank-major (disk)
        w.write_all(&bitboard::transpose(ex.black).to_le_bytes())?;
        w.write_all(&bitboard::transpose(ex.white).to_le_bytes())?;
        w.write_all(&[ex.score as u8])?;
    }
    w.flush()
}

/// Load examples from the packed binary format.
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
    Ok(std::fs::metadata(path)?.len() as usize / BIN_RECORD_SIZE)
}

/// Append a binary file's examples to `out`, stopping after `limit` of them.
///
/// Reads in whole blocks rather than one 17-byte record at a time: with the
/// data re-read every epoch, per-record `read_exact` calls are a measurable
/// share of the wall clock.
pub fn load_examples_binary_into(
    path: &Path,
    out: &mut Vec<Example>,
    limit: Option<usize>,
) -> io::Result<usize> {
    // A multiple of the record size, so a full buffer never splits a record.
    const BLOCK: usize = BIN_RECORD_SIZE * 4096;

    let mut f = File::open(path)?;
    let want = match limit {
        Some(l) => l.min(count_examples_binary(path)?),
        None => count_examples_binary(path)?,
    };
    out.reserve(want);

    let mut buf = vec![0u8; BLOCK];
    let mut carry = 0usize; // bytes of a split record held over from last block
    let mut n = 0usize;
    loop {
        // Fill the buffer past the carried-over prefix, or until EOF.
        let mut filled = carry;
        while filled < BLOCK {
            match f.read(&mut buf[filled..])? {
                0 => break,
                got => filled += got,
            }
        }
        let eof = filled < BLOCK;

        for rec in buf[..filled].chunks_exact(BIN_RECORD_SIZE) {
            if n >= want {
                return Ok(n);
            }
            let black = u64::from_le_bytes(rec[0..8].try_into().unwrap());
            let white = u64::from_le_bytes(rec[8..16].try_into().unwrap());
            out.push(Example {
                // rank-major (disk) -> file-major (memory)
                black: bitboard::transpose(black),
                white: bitboard::transpose(white),
                score: rec[16] as i8,
            });
            n += 1;
        }

        let used = (filled / BIN_RECORD_SIZE) * BIN_RECORD_SIZE;
        carry = filled - used;
        buf.copy_within(used..filled, 0);
        if eof {
            // A trailing partial record means a truncated file; ignore it,
            // matching the previous loader's EOF handling.
            return Ok(n);
        }
    }
}

/// Convert a text training file to the binary format (Go's
/// traindata_converter). Returns the number of examples written.
pub fn convert_text_to_binary(text_path: &Path, bin_path: &Path) -> io::Result<usize> {
    let examples = load_examples_text(text_path)?;
    save_examples_binary(bin_path, &examples)?;
    Ok(examples.len())
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
        Trainer { evaluator, optimizer }
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
            let mse = self
                .evaluator
                .train(&board, ex.score as f32, &mut self.optimizer);
            stats.loss_sum[stage] += mse as f64;
            stats.samples[stage] += 1;

            if (i + 1) & (PROGRESS_INTERVAL - 1) == 0 {
                progress(i + 1, total);
            }
        }
        progress(total, total);
        stats
    }

    /// One pass over `examples` spread across `threads` workers sharing the
    /// weights. Like `train_pass`, this does not advance the optimizer's
    /// epoch schedule — the caller passes `lr` for the epoch explicitly.
    ///
    /// Each worker takes a contiguous slice of the examples and updates the
    /// shared tables without locking. Updates are sparse — one cell per
    /// pattern per symmetry — so collisions are rare and a lost update costs
    /// one small step, not correctness. Only plain SGD is supported: Adam
    /// keeps per-cell moments that racing threads would corrupt in ways the
    /// sparsity argument does not cover.
    pub fn train_epoch_parallel(
        &mut self,
        examples: &[Example],
        lr: f32,
        threads: usize,
        mut progress: impl FnMut(usize, usize),
    ) -> EpochStats {
        let total = examples.len();
        if threads <= 1 || total == 0 {
            let stats = self.train_epoch_seq_lr(examples, lr, &mut progress);
            return stats;
        }

        use std::sync::atomic::{AtomicUsize, Ordering};

        let view = self.evaluator.weight_view();
        let ev = &self.evaluator;
        let done = AtomicUsize::new(0);
        let running = AtomicUsize::new(0);
        let chunk = total.div_ceil(threads);

        let parts: Vec<EpochStats> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for part in examples.chunks(chunk) {
                let view = &view;
                let done = &done;
                let running = &running;
                running.fetch_add(1, Ordering::SeqCst);
                handles.push(scope.spawn(move || {
                    let mut st = EpochStats::default();
                    for (i, ex) in part.iter().enumerate() {
                        let board = ex.board();
                        let stage = Evaluator::stage(&board);
                        // SAFETY: the view came from `ev`, which is borrowed
                        // immutably for the whole scope, so no other kind of
                        // access to the weights is live.
                        // train_shared returns MSE already; see train_pass.
                        let mse = unsafe { ev.train_shared(view, &board, ex.score as f32, lr) };
                        st.loss_sum[stage] += mse as f64;
                        st.samples[stage] += 1;
                        if (i + 1) & ((1 << 16) - 1) == 0 {
                            done.fetch_add(1 << 16, Ordering::Relaxed);
                        }
                    }
                    running.fetch_sub(1, Ordering::SeqCst);
                    st
                }));
            }
            // `progress` is not Sync, so the workers cannot call it; poll the
            // shared counter here instead. Without this the bar sits frozen
            // for the whole pass, which on this dataset is many minutes.
            while running.load(Ordering::SeqCst) > 0 {
                progress(done.load(Ordering::Relaxed).min(total), total);
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            let mut out = Vec::new();
            for h in handles {
                out.push(h.join().unwrap());
            }
            out
        });

        let mut stats = EpochStats::default();
        for p in parts {
            for i in 0..stats.loss_sum.len() {
                stats.loss_sum[i] += p.loss_sum[i];
                stats.samples[i] += p.samples[i];
            }
        }
        progress(total, total);
        stats
    }

    /// The sequential fallback, at an explicit learning rate.
    fn train_epoch_seq_lr(
        &mut self,
        examples: &[Example],
        lr: f32,
        progress: &mut impl FnMut(usize, usize),
    ) -> EpochStats {
        let view = self.evaluator.weight_view();
        let ev = &self.evaluator;
        let total = examples.len();
        let mut stats = EpochStats::default();
        for (i, ex) in examples.iter().enumerate() {
            let board = ex.board();
            let stage = Evaluator::stage(&board);
            // SAFETY: single-threaded here; see `train_epoch_parallel`.
            // train_shared returns MSE already; see train_pass.
            let mse = unsafe { ev.train_shared(&view, &board, ex.score as f32, lr) };
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

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("bbrv_trainer_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// A rank-major board string for the standard initial position.
    fn initial_board_text() -> String {
        // Same layout as BOARD_INIT_STRING's board part
        "---------------------------OX------XO---------------------------".replace('\u{0}', "")
    }

    #[test]
    fn test_parse_text_line_roundtrips_board() {
        let line = format!("{} -12", initial_board_text());
        let ex = parse_text_line(&line).expect("valid line");
        assert_eq!(ex.score, -12);

        let board = ex.board();
        // The initial position: D4/E5 white, E4/D5 black in this crate's
        // coordinates (matches Board::new modulo player)
        let reference = Board::new();
        assert_eq!(board.black, reference.black, "black bits map correctly");
        assert_eq!(board.white, reference.white, "white bits map correctly");
        assert_eq!(board.empty_count(), 60);
    }

    #[test]
    fn test_parse_text_line_rejects_garbage() {
        assert!(parse_text_line("short 3").is_none());
        let bad_score = format!("{} notanumber", initial_board_text());
        assert!(parse_text_line(&bad_score).is_none());
    }

    #[test]
    fn test_text_and_binary_roundtrip() {
        let text_path = temp_path("examples.txt");
        let bin_path = temp_path("examples.data");

        let mut content = String::new();
        content.push_str(&format!("{} 8\n", initial_board_text()));
        content.push_str(&format!("{} -30\n", initial_board_text()));
        content.push('\n'); // blank line is skipped
        std::fs::write(&text_path, content).unwrap();

        let n = convert_text_to_binary(&text_path, &bin_path).unwrap();
        assert_eq!(n, 2);

        let from_text = load_examples_text(&text_path).unwrap();
        let from_bin = load_examples_binary(&bin_path).unwrap();
        assert_eq!(from_text, from_bin, "binary roundtrip preserves examples");
        assert_eq!(from_bin[0].score, 8);
        assert_eq!(from_bin[1].score, -30);

        std::fs::remove_file(&text_path).ok();
        std::fs::remove_file(&bin_path).ok();
    }

    #[test]
    fn test_binary_format_is_go_compatible_layout() {
        // The on-disk record must be exactly 17 bytes with rank-major bits:
        // A1 = bit 0, B1 = bit 1 (rank-major) even though memory layout is
        // file-major (B1 = bit 8).
        let bin_path = temp_path("layout.data");
        let ex = Example {
            black: 1u64 << 8, // B1 in file-major memory layout
            white: 0,
            score: 5,
        };
        save_examples_binary(&bin_path, &[ex]).unwrap();

        let raw = std::fs::read(&bin_path).unwrap();
        assert_eq!(raw.len(), 17);
        let disk_black = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        assert_eq!(disk_black, 1u64 << 1, "B1 must be bit 1 in rank-major on disk");
        assert_eq!(raw[16] as i8, 5);

        std::fs::remove_file(&bin_path).ok();
    }

    #[test]
    fn test_binary_load_spans_blocks_and_appends() {
        // More records than the loader's read block holds, so the multi-block
        // path (and its record-boundary bookkeeping) is exercised.
        let bin_path = temp_path("multiblock.data");
        let written: Vec<Example> = (0..10_000u64)
            .map(|i| Example {
                black: i.wrapping_mul(0x9E3779B97F4A7C15),
                white: i.wrapping_mul(0xC2B2AE3D27D4EB4F),
                score: (i % 128) as i8 - 64,
            })
            .collect();
        save_examples_binary(&bin_path, &written).unwrap();

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
    fn test_text_load_honours_limit_and_appends() {
        let text_path = temp_path("limited.txt");
        let mut content = String::new();
        for score in 0..10 {
            content.push_str(&format!("{} {}\n", initial_board_text(), score));
        }
        std::fs::write(&text_path, content).unwrap();

        let mut out = Vec::new();
        let n = load_examples_text_into(&text_path, &mut out, Some(3)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].score, 2);

        load_examples_text_into(&text_path, &mut out, Some(2)).unwrap();
        assert_eq!(out.len(), 5, "second load appends");

        std::fs::remove_file(&text_path).ok();
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
            score: 2,
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
            examples.push(Example { black, white, score });
        }

        let mut trainer = Trainer::new(
            Evaluator::new(EGAROUCID_PATTERNS),
            AdamOptimizer::new(0.01),
        );
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
            score: 2,
        }];
        let mut trainer = Trainer::new(
            Evaluator::new(EGAROUCID_PATTERNS),
            AdamOptimizer::new(0.01),
        );
        let stats = trainer.train_epoch(&examples);
        assert_eq!(stats.samples[0], 1, "initial position is stage 0");
        assert_eq!(stats.samples[1..].iter().sum::<u64>(), 0);
        assert!(stats.stage_mse(0) > 0.0, "first-epoch loss is nonzero");
    }
}
