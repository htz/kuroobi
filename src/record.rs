//! The training record: one position with everything the game that produced it can say about it.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::bitboard;
use crate::trainer::Example;

pub const SIZE: usize = 27;
pub const NO_SQUARE: u8 = 64;
pub const NO_GAME_SCORE: i8 = i8::MIN;

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
    pub game_id: u16,
}

impl Record {
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
        if r.ply <= 1 {
            return 0.0;
        }
        match self.search_value_to_ply {
            Some(t) if r.ply <= t => r.score,
            _ => r.teacher(),
        }
    }
}

pub fn count(path: &Path) -> io::Result<usize> {
    Ok(std::fs::metadata(path)?.len() as usize / SIZE)
}

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

pub fn for_each_range(
    path: &Path,
    start: usize,
    len: usize,
    mut f: impl FnMut(Record) -> bool,
) -> io::Result<()> {
    const BLOCK: usize = SIZE * 4096;
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start((start * SIZE) as u64))?;
    let mut buf = vec![0u8; BLOCK];
    let mut left = len;
    let mut carry = 0usize;
    while left > 0 {
        let want = (left * SIZE).min(BLOCK);
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

pub fn read_all(path: &Path) -> io::Result<Vec<Record>> {
    let mut v = Vec::with_capacity(count(path)?);
    for_each(path, |r| {
        v.push(r);
        true
    })?;
    Ok(v)
}

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
        assert_eq!(b[23], 26);
        assert_eq!(b[24], 1);
        assert_eq!(b[22], 1);
        assert_eq!(b[21], 12);
        assert_eq!(b[20] as i8, -6);
        assert_eq!(f32::from_le_bytes(b[16..20].try_into().unwrap()), -3.5);
        assert_eq!(u16::from_le_bytes([b[25], b[26]]), 0xBEEF);
    }

    #[test]
    fn a_policy_can_prefer_the_search_value() {
        let mut r = sample();
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

    #[test]
    fn range_reads_exactly_the_records_asked_for() {
        let dir = std::env::temp_dir().join(format!("kuroobi-range-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.data");
        let mut w = Writer::create(&path).unwrap();
        for ply in 0..10_000u32 {
            w.write(&Record {
                ply: (ply % 60) as u8,
                game_id: ply as u16,
                ..sample()
            })
            .unwrap();
        }
        w.finish().unwrap();
        let ids = |start: usize, len: usize| {
            let mut v = Vec::new();
            for_each_range(&path, start, len, |r| {
                v.push(r.game_id as usize);
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
}
