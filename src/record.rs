//! The training record: one position with everything the game that produced it can say about it.
//!
//! On disk a record is 24 bytes in a `.rec` file: mover, opponent (u64, this crate's square layout)
//! and one u64 packing score f32 (bits 0-31), score kind (32-33), search depth (34-39),
//! game_score i8 (40-47), game_score_from (48-53), move (54-60), random (61), black to move (62)
//! and game start (63). `ply` is `60 - empties`.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::trainer::Example;

pub const SIZE: usize = 24;
pub const NO_SQUARE: u8 = 64;
pub const NO_GAME_SCORE: i8 = i8::MIN;

/// What `score` is. `Exact` is set only where every record has been checked against the solver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScoreKind {
    /// Not a search value the record can vouch for (a copy of the game result, or of unknown origin).
    #[default]
    Unknown = 0,
    Midgame = 1,
    /// A selective endgame solve; `score_depth` holds its ProbCut t in tenths, not a depth.
    Selective = 2,
    Exact = 3,
}

impl ScoreKind {
    fn from_bits(b: u64) -> ScoreKind {
        match b & 3 {
            1 => ScoreKind::Midgame,
            2 => ScoreKind::Selective,
            3 => ScoreKind::Exact,
            _ => ScoreKind::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Record {
    pub mover: u64,
    pub opponent: u64,
    pub score: f32,
    pub game_score: i8,
    pub ply: u8,
    pub random: bool,
    pub sq: u8,
    pub black_to_move: bool,
    pub score_kind: ScoreKind,
    /// Midgame search depth; 0 when solved or unknown.
    pub score_depth: u8,
    /// 0: `game_score` is the game's own result. K: the solved value of this game's position at K
    /// empties, carried back along the moves before it (or, at K itself, this position's own value).
    pub game_score_from: u8,
    /// First record of a game in a file that keeps games in order.
    pub game_start: bool,
}

impl Default for Record {
    fn default() -> Record {
        Record {
            mover: 0,
            opponent: 0,
            score: 0.0,
            game_score: NO_GAME_SCORE,
            ply: 0,
            random: false,
            sq: NO_SQUARE,
            black_to_move: true,
            score_kind: ScoreKind::Unknown,
            score_depth: 0,
            game_score_from: 0,
            game_start: false,
        }
    }
}

impl Record {
    pub fn from_bytes(b: &[u8; SIZE]) -> Record {
        let mover = u64::from_le_bytes(b[0..8].try_into().unwrap());
        let opponent = u64::from_le_bytes(b[8..16].try_into().unwrap());
        let x = u64::from_le_bytes(b[16..24].try_into().unwrap());
        Record {
            mover,
            opponent,
            score: f32::from_bits(x as u32),
            score_kind: ScoreKind::from_bits(x >> 32),
            score_depth: ((x >> 34) & 63) as u8,
            game_score: (x >> 40) as u8 as i8,
            game_score_from: ((x >> 48) & 63) as u8,
            sq: ((x >> 54) & 127) as u8,
            random: (x >> 61) & 1 != 0,
            black_to_move: (x >> 62) & 1 != 0,
            game_start: (x >> 63) != 0,
            ply: 60u8.saturating_sub((!(mover | opponent)).count_ones() as u8),
        }
    }

    pub fn to_bytes(&self) -> [u8; SIZE] {
        debug_assert!(self.score_depth < 64 && self.game_score_from < 64 && self.sq <= NO_SQUARE);
        let x = u64::from(self.score.to_bits())
            | (self.score_kind as u64) << 32
            | u64::from(self.score_depth & 63) << 34
            | u64::from(self.game_score as u8) << 40
            | u64::from(self.game_score_from & 63) << 48
            | u64::from(self.sq & 127) << 54
            | u64::from(self.random) << 61
            | u64::from(self.black_to_move) << 62
            | u64::from(self.game_start) << 63;
        let mut b = [0u8; SIZE];
        b[0..8].copy_from_slice(&self.mover.to_le_bytes());
        b[8..16].copy_from_slice(&self.opponent.to_le_bytes());
        b[16..24].copy_from_slice(&x.to_le_bytes());
        b
    }

    pub fn teacher(&self) -> f32 {
        if self.ply <= 1 {
            0.0
        } else if self.random || self.game_score == NO_GAME_SCORE {
            self.score
        } else {
            f32::from(self.game_score)
        }
    }

    pub fn example_with(&self, policy: &TeacherPolicy) -> Example {
        Example {
            black: self.mover,
            white: self.opponent,
            score: policy.value(self),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Filter {
    pub min_ply: u8,
    pub max_score_diff: Option<f32>,
    pub drop_random: bool,
    pub keep_above_ply: Option<u8>,
}

impl Filter {
    pub const NONE: Filter = Filter {
        min_ply: 0,
        max_score_diff: None,
        drop_random: false,
        keep_above_ply: None,
    };

    pub const TRAINING: Filter = Filter {
        min_ply: 8,
        max_score_diff: Some(12.0),
        drop_random: true,
        keep_above_ply: Some(50),
    };

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeacherPolicy {
    pub search_value_to_ply: Option<u8>,
}

impl TeacherPolicy {
    pub const DEFAULT: TeacherPolicy = TeacherPolicy {
        search_value_to_ply: None,
    };

    pub fn describe(&self) -> String {
        match self.search_value_to_ply {
            None => String::from("game result"),
            Some(t) => format!("search value to ply {t}, game result after"),
        }
    }

    pub fn value(&self, r: &Record) -> f32 {
        match self.search_value_to_ply {
            Some(t) if r.ply > 1 && r.ply <= t => r.score,
            _ => r.teacher(),
        }
    }
}

/// Records are read from and written to `.rec` files only.
fn check(path: &Path) -> io::Result<()> {
    if path.extension().is_some_and(|e| e == "rec") {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: training records live in .rec files", path.display()),
        ))
    }
}

pub fn count(path: &Path) -> io::Result<usize> {
    check(path)?;
    Ok(std::fs::metadata(path)?.len() as usize / SIZE)
}

pub fn for_each(path: &Path, f: impl FnMut(Record) -> bool) -> io::Result<()> {
    for_each_range(path, 0, usize::MAX, f)
}

pub fn for_each_range(
    path: &Path,
    start: usize,
    len: usize,
    mut f: impl FnMut(Record) -> bool,
) -> io::Result<()> {
    check(path)?;
    const BLOCK: usize = SIZE * 4096;
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start((start * SIZE) as u64))?;
    let mut buf = vec![0u8; BLOCK];
    let mut left = len;
    let mut carry = 0usize;
    while left > 0 {
        let want = left.saturating_mul(SIZE).min(BLOCK);
        let mut filled = carry;
        while filled < want {
            match file.read(&mut buf[filled..want])? {
                0 => break,
                got => filled += got,
            }
        }
        let eof = filled < want;
        for rec in buf[..filled].as_chunks::<SIZE>().0 {
            if left == 0 || !f(Record::from_bytes(rec)) {
                return Ok(());
            }
            left -= 1;
        }
        let used = (filled / SIZE) * SIZE;
        carry = filled - used;
        buf.copy_within(used..filled, 0);
        if eof {
            return Ok(());
        }
    }
    Ok(())
}

pub fn write_all(path: &Path, records: &[Record]) -> io::Result<()> {
    let mut w = Writer::create(path)?;
    for r in records {
        w.push(r)?;
    }
    w.finish()
}

pub struct Writer {
    out: BufWriter<File>,
}

impl Writer {
    pub fn create(path: &Path) -> io::Result<Writer> {
        check(path)?;
        Ok(Writer {
            out: BufWriter::with_capacity(1 << 20, File::create(path)?),
        })
    }

    pub fn push(&mut self, r: &Record) -> io::Result<()> {
        self.out.write_all(&r.to_bytes())
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.out.flush()?;
        self.out
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Record {
        let (mover, opponent) = (0x0000_0008_1000_0000, 0x0000_0010_0800_0000);
        Record {
            mover,
            opponent,
            score: -3.140_625,
            game_score: -6,
            ply: 60 - (!(mover | opponent)).count_ones() as u8,
            random: true,
            sq: 19,
            black_to_move: false,
            score_kind: ScoreKind::Midgame,
            score_depth: 63,
            game_score_from: 14,
            game_start: true,
        }
    }

    #[test]
    fn round_trips_every_field() {
        let r = sample();
        assert_eq!(Record::from_bytes(&r.to_bytes()), r);
        let r = Record {
            sq: NO_SQUARE,
            game_score: NO_GAME_SCORE,
            score: f32::NAN,
            score_kind: ScoreKind::Exact,
            score_depth: 0,
            random: false,
            black_to_move: true,
            game_start: false,
            ..sample()
        };
        let back = Record::from_bytes(&r.to_bytes());
        assert!(back.score.is_nan());
        assert_eq!(Record { score: 0.0, ..back }, Record { score: 0.0, ..r });
    }

    #[test]
    fn disk_layout_is_fixed() {
        let r = sample();
        let b = r.to_bytes();
        assert_eq!(u64::from_le_bytes(b[0..8].try_into().unwrap()), r.mover);
        let x = u64::from_le_bytes(b[16..24].try_into().unwrap());
        assert_eq!(f32::from_bits(x as u32), -3.140_625);
        assert_eq!((x >> 32) & 3, 1);
        assert_eq!((x >> 34) & 63, 63);
        assert_eq!((x >> 40) as u8 as i8, -6);
        assert_eq!((x >> 48) & 63, 14);
        assert_eq!((x >> 54) & 127, 19);
        assert_eq!((x >> 61) & 7, 0b101);
    }

    #[test]
    fn a_policy_can_prefer_the_search_value() {
        let mut r = sample();
        r.score = -3.5;
        r.ply = 20;
        r.random = false;
        assert_eq!(TeacherPolicy::DEFAULT.value(&r), -6.0);
        let p = TeacherPolicy {
            search_value_to_ply: Some(32),
        };
        assert_eq!(p.value(&r), -3.5);
        r.ply = 33;
        assert_eq!(p.value(&r), -6.0);
        r.ply = 1;
        assert_eq!(p.value(&r), 0.0);
    }

    #[test]
    fn teacher_follows_the_teacher_rule() {
        let mut r = sample();
        r.score = -3.5;
        r.ply = 20;
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
        r.ply = 20;
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

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kuroobi-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn file_round_trip_ignores_a_cut_record() {
        let dir = temp_dir("record");
        let path = dir.join("t.rec");
        let second = Record {
            game_start: false,
            ..sample()
        };
        write_all(&path, &[sample(), second]).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend([1, 2, 3]);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(count(&path).unwrap(), 2);
        let mut v = Vec::new();
        for_each(&path, |r| {
            v.push(r);
            true
        })
        .unwrap();
        assert_eq!(v, vec![sample(), second]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn range_reads_exactly_the_records_asked_for() {
        let dir = temp_dir("range");
        let path = dir.join("t.rec");
        let records: Vec<Record> = (0..10_000u32)
            .map(|i| Record {
                score: i as f32,
                ..sample()
            })
            .collect();
        write_all(&path, &records).unwrap();
        let ids = |start: usize, len: usize| {
            let mut v = Vec::new();
            for_each_range(&path, start, len, |r| {
                v.push(r.score as usize);
                true
            })
            .unwrap();
            v
        };
        assert_eq!(ids(0, 3), vec![0, 1, 2]);
        assert_eq!(ids(4_090, 12), (4_090..4_102).collect::<Vec<_>>());
        assert_eq!(ids(9_998, 10), vec![9_998, 9_999]);
        assert_eq!(ids(0, 0), Vec::<usize>::new());
        assert_eq!(ids(0, 10_000).len(), 10_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_rec_files_are_read_or_written() {
        let dir = temp_dir("ext");
        let other = dir.join("t.bin");
        std::fs::write(&other, [0u8; SIZE]).unwrap();
        assert!(count(&other).is_err());
        assert!(for_each(&other, |_| true).is_err());
        assert!(Writer::create(&dir.join("u.bin")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
