//! What the self-play generators share: the shard files they write and the
//! record they write into them.
//!
//! Two generators write this format -- `gendata` from randomised openings,
//! `opening_gen` from every opening position of a stage -- and a shard from
//! either is read the same way, so the file handling lives here rather than
//! in one of them.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::record::{Record, SIZE as RECORD};
use crate::{Board, Color, Position};

/// One position of a game, kept until the game ends so the final result
/// can be filled in.
pub struct Pending {
    pub board: Board,
    pub value: f32,
    pub ply: u8,
    pub random: bool,
    pub mv: Position,
}

/// Serialise one position. `final_mover` is the final disc difference from
/// the point of view of this position's mover.
pub fn record(p: &Pending, final_mover: i8, game_id: u16) -> [u8; RECORD] {
    let b = &p.board;
    let (mover, opponent) = if b.player() == Color::Black {
        (b.black, b.white)
    } else {
        (b.white, b.black)
    };
    Record {
        mover,
        opponent,
        score: p.value,
        game_score: final_mover,
        ply: p.ply,
        random: p.random,
        sq: p.mv.index(),
        black_to_move: b.player() == Color::Black,
        game_id,
    }
    .to_bytes()
}

/// The final disc difference from Black's side, empties to the winner, of
/// a finished game.
pub fn final_black(b: &Board) -> i8 {
    let s = crate::solver::final_score(b);
    let s = if b.player() == Color::Black { s } else { -s };
    s.clamp(-64, 64) as i8
}

/// A shard being written. Renamed into place only once full, so a reader can
/// take any `.data` in the directory as complete.
pub struct Shard {
    dir: PathBuf,
    worker: usize,
    index: usize,
    file: Option<std::fs::File>,
    written: usize,
    cap: usize,
}

impl Shard {
    pub fn new(dir: PathBuf, worker: usize, cap: usize) -> Shard {
        Shard {
            dir,
            worker,
            index: 0,
            file: None,
            written: 0,
            cap,
        }
    }

    fn tmp_path(&self) -> PathBuf {
        self.dir
            .join(format!("shard_{:02}_{:04}.part", self.worker, self.index))
    }

    fn final_path(&self) -> PathBuf {
        self.dir
            .join(format!("shard_{:02}_{:04}.data", self.worker, self.index))
    }

    pub fn push(&mut self, rec: &[u8; RECORD]) -> std::io::Result<()> {
        if self.file.is_none() {
            self.file = Some(std::fs::File::create(self.tmp_path())?);
            self.written = 0;
        }
        self.file.as_mut().unwrap().write_all(rec)?;
        self.written += 1;
        if self.written >= self.cap {
            self.close()?;
        }
        Ok(())
    }

    fn close(&mut self) -> std::io::Result<()> {
        if let Some(mut f) = self.file.take() {
            f.flush()?;
            drop(f);
            std::fs::rename(self.tmp_path(), self.final_path())?;
            self.index += 1;
            self.written = 0;
        }
        Ok(())
    }

    /// Close whatever is in flight and keep it.
    ///
    /// A short shard is not a broken one. Records are fixed width and
    /// written whole, so a file cut off at any point holds nothing but
    /// complete records -- it is simply a smaller shard, and every reader
    /// takes it as such. Deleting it (which this used to do) threw away
    /// every position since the last full shard, which at a million
    /// positions per shard was most of a day's work.
    pub fn finish(&mut self) {
        if self.written > 0 {
            let _ = self.close();
        } else if self.file.take().is_some() {
            // Opened but empty: nothing to keep.
            let _ = std::fs::remove_file(self.tmp_path());
        }
    }
}

/// Rename any `.part` left by an earlier run into `.data`.
///
/// A `.part` is a shard that was being written when the process ended, and
/// its contents are complete: records are fixed width and written whole, so
/// the file holds nothing but whole records. It is simply a smaller shard.
///
/// Doing this at startup rather than asking for it in a README matters
/// because the next run starts numbering shards from zero again -- so a
/// leftover `shard_00_0000.part` would be overwritten by the new worker 0.
/// The moment that made this necessary is the one nobody controls: the
/// machine shutting down.
///
/// Names that would collide with an existing `.data` are given the next free
/// index instead of overwriting it.
pub fn adopt_leftovers(dir: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().and_then(|s| s.to_str()) != Some("part") {
            continue;
        }
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let mut target = dir.join(format!("{stem}.data"));
        // `shard_00_0000` from an old run and one from this run are
        // different shards that happen to share a name.
        let mut bump = 0u32;
        while target.exists() {
            bump += 1;
            target = dir.join(format!("{stem}_prev{bump}.data"));
        }
        std::fs::rename(&p, &target)?;
        n += 1;
    }
    Ok(n)
}
