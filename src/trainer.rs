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
    let reader = BufReader::new(File::open(path)?);
    let mut examples = Vec::new();
    for (no, line) in reader.lines().enumerate() {
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
        examples.push(ex);
    }
    Ok(examples)
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
    let mut r = BufReader::new(File::open(path)?);
    let mut examples = Vec::new();
    let mut buf = [0u8; BIN_RECORD_SIZE];
    loop {
        match r.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let black = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let white = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let score = buf[16] as i8;
        examples.push(Example {
            // rank-major (disk) -> file-major (memory)
            black: bitboard::transpose(black),
            white: bitboard::transpose(white),
            score,
        });
    }
    Ok(examples)
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
        mut progress: impl FnMut(usize, usize),
    ) -> EpochStats {
        // Power-of-two interval lets the hot loop use a cheap mask test.
        const PROGRESS_INTERVAL: usize = 1 << 16;

        let total = examples.len();
        let mut stats = EpochStats::default();
        for (i, ex) in examples.iter().enumerate() {
            let board = ex.board();
            let stage = Evaluator::stage(&board);
            let err = self
                .evaluator
                .train(&board, ex.score as f32, &mut self.optimizer);
            stats.loss_sum[stage] += (err * err) as f64;
            stats.samples[stage] += 1;

            if (i + 1) & (PROGRESS_INTERVAL - 1) == 0 {
                progress(i + 1, total);
            }
        }
        progress(total, total);
        self.optimizer.next_epoch();
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
