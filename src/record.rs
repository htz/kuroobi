//! The training record: one position with everything the game that produced
//! it can say about it. The layout is fixed byte for byte, so a corpus
//! outlives the code that wrote it.
//!
//! The previous format kept a board and one teacher value, and nothing else.
//! That lost the distinction training needs: a position's teacher
//! is the **final disc difference** of the game (a search value is only kept
//! to filter games whose result disagrees with it), and the positions
//! reached by the opening's random moves are flagged so a filter can drop
//! them. 33 million positions written in the old format with search values
//! as the teacher could not be repaired, because nothing else was there.
//!
//! On disk (27 bytes, little-endian): mover's discs u64, opponent's discs
//! u64, search value f32 (mover's view), final disc difference i8 (mover's
//! view, empties to the winner, [`NO_GAME_SCORE`] when unknown), ply u8
//! (60 - empties), random-move flag u8, move played u8 ([`NO_SQUARE`] when
//! unknown), side to move u8 (0 = Black), game id u16. Bitboards and the
//! square are rank-major on disk (A1 = 0, B1 = 1); in memory they are this
//! crate's file-major, and the conversion
//! happens here and nowhere else.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

use crate::bitboard;
use crate::trainer::Example;

/// Bytes per record on disk.
pub const SIZE: usize = 27;
/// `sq` of a record whose move is not known.
pub const NO_SQUARE: u8 = 64;
/// `game_score` of a record whose game has no final result.
pub const NO_GAME_SCORE: i8 = i8::MIN;

/// One position of one game. Bitboards file-major, the mover's discs first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Record {
    pub mover: u64,
    pub opponent: u64,
    /// The value the search assigned, mover's view, in discs.
    pub score: f32,
    /// Final disc difference of the game, mover's view, empties to the
    /// winner; [`NO_GAME_SCORE`] when the position has no game.
    pub game_score: i8,
    /// Moves played so far, i.e. 60 - empties.
    pub ply: u8,
    /// The move played from here was chosen at random, not by search.
    pub random: bool,
    /// The move played from here (this crate's index), or [`NO_SQUARE`].
    pub sq: u8,
    pub black_to_move: bool,
    pub game_id: u16,
}

impl Record {
    /// Decode one on-disk record.
    pub fn from_bytes(b: &[u8; SIZE]) -> Record {
        let sq = b[23];
        Record {
            mover: bitboard::transpose(u64::from_le_bytes(b[0..8].try_into().unwrap())),
            opponent: bitboard::transpose(u64::from_le_bytes(b[8..16].try_into().unwrap())),
            score: f32::from_le_bytes(b[16..20].try_into().unwrap()),
            game_score: b[20] as i8,
            ply: b[21],
            random: b[22] != 0,
            sq: if sq < 64 {
                (sq % 8) * 8 + sq / 8
            } else {
                NO_SQUARE
            },
            black_to_move: b[24] == 0,
            game_id: u16::from_le_bytes([b[25], b[26]]),
        }
    }

    /// Encode for disk.
    pub fn to_bytes(&self) -> [u8; SIZE] {
        let mut b = [0u8; SIZE];
        b[0..8].copy_from_slice(&bitboard::transpose(self.mover).to_le_bytes());
        b[8..16].copy_from_slice(&bitboard::transpose(self.opponent).to_le_bytes());
        b[16..20].copy_from_slice(&self.score.to_le_bytes());
        b[20] = self.game_score as u8;
        b[21] = self.ply;
        b[22] = u8::from(self.random);
        b[23] = if self.sq < 64 {
            (self.sq % 8) * 8 + self.sq / 8
        } else {
            NO_SQUARE
        };
        b[24] = u8::from(!self.black_to_move);
        b[25..27].copy_from_slice(&self.game_id.to_le_bytes());
        b
    }

    /// The value the trainer fits: the first two plies are 0 by symmetry, a
    /// position whose move was random takes the search value (its game
    /// went on at random, so its result says nothing about it), every other
    /// position takes the game's final disc difference.
    pub fn teacher(&self) -> f32 {
        if self.ply <= 1 {
            0.0
        } else if self.random || self.game_score == NO_GAME_SCORE {
            self.score
        } else {
            f32::from(self.game_score)
        }
    }

    /// The position as the trainer sees it: mover as Black, teacher as score.
    pub fn example(&self) -> Example {
        Example {
            black: self.mover,
            white: self.opponent,
            score: self.teacher(),
        }
    }

    pub fn empties(&self) -> u8 {
        64 - (self.mover | self.opponent).count_ones() as u8
    }
}

/// Which records to train on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Filter {
    /// Drop positions before this ply.
    pub min_ply: u8,
    /// Drop positions whose search value and game result disagree by more
    /// than this many discs (skipped when the result is unknown).
    pub max_score_diff: Option<f32>,
    /// Drop positions whose move was random.
    pub drop_random: bool,
    /// From this ply on, keep everything regardless of the two above.
    pub keep_above_ply: Option<u8>,
}

impl Filter {
    /// Keep every record.
    pub const NONE: Filter = Filter {
        min_ply: 0,
        max_score_diff: None,
        drop_random: false,
        keep_above_ply: None,
    };

    /// The training filter: `--min-ply 8 --max-score-diff 12 --drop-random
    /// --keep-above-ply 50`.
    pub const TRAINING: Filter = Filter {
        min_ply: 8,
        max_score_diff: Some(12.0),
        drop_random: true,
        keep_above_ply: Some(50),
    };

    /// Consume one of the filter's command-line flags (`--min-ply N`,
    /// `--max-score-diff D`, `--drop-random`, `--keep-above-ply N`). Returns
    /// `Ok(false)` when `flag` is not one of them, `Err` when its value is
    /// missing or malformed.
    pub fn take_flag(
        &mut self,
        flag: &str,
        args: &mut impl Iterator<Item = String>,
    ) -> Result<bool, String> {
        fn value<T: std::str::FromStr>(
            flag: &str,
            args: &mut impl Iterator<Item = String>,
        ) -> Result<T, String> {
            args.next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| format!("{flag} needs a number"))
        }
        match flag {
            "--min-ply" => self.min_ply = value(flag, args)?,
            "--max-score-diff" => self.max_score_diff = Some(value(flag, args)?),
            "--drop-random" => self.drop_random = true,
            "--keep-above-ply" => self.keep_above_ply = Some(value(flag, args)?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The flags that would reproduce this filter, for a run's log.
    pub fn describe(&self) -> String {
        let mut s = format!("--min-ply {}", self.min_ply);
        if let Some(d) = self.max_score_diff {
            s.push_str(&format!(" --max-score-diff {d}"));
        }
        if self.drop_random {
            s.push_str(" --drop-random");
        }
        if let Some(p) = self.keep_above_ply {
            s.push_str(&format!(" --keep-above-ply {p}"));
        }
        s
    }

    pub fn keeps(&self, r: &Record) -> bool {
        if r.ply < self.min_ply {
            return false;
        }
        if self.keep_above_ply.is_some_and(|p| r.ply >= p) {
            return true;
        }
        if self.drop_random && r.random {
            return false;
        }
        if let Some(d) = self.max_score_diff {
            if r.game_score != NO_GAME_SCORE && (r.score - f32::from(r.game_score)).abs() > d {
                return false;
            }
        }
        true
    }
}

/// Number of records in a file, from its size alone.
pub fn count(path: &Path) -> io::Result<usize> {
    Ok(std::fs::metadata(path)?.len() as usize / SIZE)
}

/// Read a file front to back, handing each record to `f`; stops early when
/// `f` returns false. A trailing partial record (a file cut short) is
/// ignored. Reads in blocks: the trainer re-reads its data every epoch, and
/// per-record reads were a measurable share of the wall clock.
pub fn for_each(path: &Path, mut f: impl FnMut(Record) -> bool) -> io::Result<()> {
    const BLOCK: usize = SIZE * 4096;
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; BLOCK];
    let mut carry = 0usize;
    loop {
        let mut filled = carry;
        while filled < BLOCK {
            match file.read(&mut buf[filled..])? {
                0 => break,
                got => filled += got,
            }
        }
        let eof = filled < BLOCK;
        for rec in buf[..filled].as_chunks::<SIZE>().0 {
            if !f(Record::from_bytes(rec)) {
                return Ok(());
            }
        }
        let used = (filled / SIZE) * SIZE;
        carry = filled - used;
        buf.copy_within(used..filled, 0);
        if eof {
            return Ok(());
        }
    }
}

/// All records of a file.
pub fn read_all(path: &Path) -> io::Result<Vec<Record>> {
    let mut v = Vec::with_capacity(count(path)?);
    for_each(path, |r| {
        v.push(r);
        true
    })?;
    Ok(v)
}

/// A file being written, one record at a time.
pub struct Writer {
    w: BufWriter<File>,
    n: usize,
}

impl Writer {
    pub fn create(path: &Path) -> io::Result<Writer> {
        Ok(Writer {
            w: BufWriter::new(File::create(path)?),
            n: 0,
        })
    }

    pub fn write(&mut self, r: &Record) -> io::Result<()> {
        self.n += 1;
        self.w.write_all(&r.to_bytes())
    }

    /// Records written so far.
    pub fn written(&self) -> usize {
        self.n
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.w.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Record {
        Record {
            mover: 0x0000_0008_1000_0000,
            opponent: 0x0000_0010_0800_0000,
            score: -3.5,
            game_score: -6,
            ply: 12,
            random: true,
            sq: 19, // c4 in this crate's file-major index
            black_to_move: false,
            game_id: 0xBEEF,
        }
    }

    #[test]
    fn round_trips_through_bytes() {
        let r = sample();
        assert_eq!(Record::from_bytes(&r.to_bytes()), r);
        let mut r = sample();
        r.sq = NO_SQUARE;
        r.game_score = NO_GAME_SCORE;
        assert_eq!(Record::from_bytes(&r.to_bytes()), r);
    }

    #[test]
    fn disk_layout_is_fixed() {
        let b = sample().to_bytes();
        // Boards are stored rank-major: file-major bit 27 (d4) is rank-major bit 27
        // too, but c4 (file 2, rank 3) is 2*8+3 = 19 here and 3*8+2 = 26 there.
        assert_eq!(b[23], 26);
        assert_eq!(b[24], 1);
        assert_eq!(b[22], 1);
        assert_eq!(b[21], 12);
        assert_eq!(b[20] as i8, -6);
        assert_eq!(f32::from_le_bytes(b[16..20].try_into().unwrap()), -3.5);
        assert_eq!(u16::from_le_bytes([b[25], b[26]]), 0xBEEF);
    }

    #[test]
    fn teacher_follows_the_teacher_rule() {
        let mut r = sample();
        r.random = false;
        assert_eq!(r.teacher(), -6.0);
        r.random = true;
        assert_eq!(r.teacher(), -3.5);
        r.random = false;
        r.game_score = NO_GAME_SCORE;
        assert_eq!(r.teacher(), -3.5);
        r.ply = 1;
        assert_eq!(r.teacher(), 0.0);
    }

    #[test]
    fn training_filter_keeps_what_it_should() {
        let f = Filter::TRAINING;
        let mut r = sample();
        r.random = false;
        r.score = -3.5;
        r.game_score = -6;
        assert!(f.keeps(&r));
        r.game_score = -20;
        assert!(!f.keeps(&r));
        r.game_score = NO_GAME_SCORE;
        assert!(f.keeps(&r));
        r.random = true;
        assert!(!f.keeps(&r));
        r.ply = 50;
        assert!(f.keeps(&r));
        r.ply = 7;
        assert!(!f.keeps(&r));
        assert!(Filter::NONE.keeps(&r));
    }

    #[test]
    fn file_round_trip_ignores_a_cut_record() {
        let dir = std::env::temp_dir().join(format!("kuroobi-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.data");
        let mut w = Writer::create(&path).unwrap();
        w.write(&sample()).unwrap();
        w.write(&Record { ply: 0, ..sample() }).unwrap();
        w.finish().unwrap();
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(&[1, 2, 3]).unwrap();
        drop(f);
        assert_eq!(count(&path).unwrap(), 2);
        let v = read_all(&path).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], sample());
        assert_eq!(v[1].ply, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
