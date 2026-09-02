//! Remove repeated positions from training data, writing a new set of files.
//!
//! The trainer has no notion of a duplicate: a position present twice takes
//! two gradient steps and ends up weighted twice. That would be tolerable if
//! duplicates were spread evenly, but they are not -- they concentrate in
//! whatever part of the corpus was generated with the least variety, and
//! that part then dominates.
//!
//! `nnue_train --dedup` does the same thing in memory, but pays for it on
//! every run. Doing it once, here, leaves files that need no further care.
//!
//! No sort and no temporary files: a hash of the board is enough, at 17
//! bytes per distinct position (500M positions in 17 GB).
//!
//! The key is the raw board, not the smallest of its eight symmetries.
//! `--sym-train` picks one symmetry at random per example, so two records
//! that differ only by rotation are two different training inputs;
//! collapsing them would throw that augmentation away. `--symmetric` asks
//! for the other behaviour.
//!
//! Usage: dedup_data --out DIR [--symmetric] [--shard-size N] <file.data>...

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;

use kuroobi::nnue::sym_board;

const RECORD: usize = 17;

fn main() -> std::process::ExitCode {
    let mut out_dir: Option<PathBuf> = None;
    let mut symmetric = false;
    let mut shard_size: usize = 1_000_000;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_dir = Some(PathBuf::from(it.next().expect("--out"))),
            "--symmetric" => symmetric = true,
            "--shard-size" => shard_size = it.next().expect("--shard-size").parse().unwrap(),
            other if other.starts_with("--") => panic!("unknown flag {other}"),
            other => files.push(PathBuf::from(other)),
        }
    }
    let Some(out_dir) = out_dir else {
        eprintln!("usage: dedup_data --out DIR [--symmetric] [--shard-size N] <file.data>...");
        return std::process::ExitCode::FAILURE;
    };
    if files.is_empty() {
        eprintln!("no input files");
        return std::process::ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("create {}: {e}", out_dir.display());
        return std::process::ExitCode::FAILURE;
    }

    /// The smallest of a board's eight symmetries, so that rotations and
    /// reflections of one position share a key.
    fn canonical(black: u64, white: u64) -> (u64, u64) {
        (0..8)
            .map(|i| (sym_board(black, i), sym_board(white, i)))
            .min()
            .unwrap()
    }

    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut read = 0usize;
    let mut kept = 0usize;
    let mut shard = 0usize;
    let mut written = 0usize;
    let mut sink: Option<std::fs::File> = None;
    let t0 = std::time::Instant::now();

    for f in &files {
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("read {}: {e}", f.display());
                return std::process::ExitCode::FAILURE;
            }
        };
        // Fixed-width records, so a file that is not a whole number of them
        // is truncated or misaligned rather than merely short.
        if !bytes.len().is_multiple_of(RECORD) {
            eprintln!(
                "{}: {} bytes is not a whole number of {RECORD}-byte records",
                f.display(),
                bytes.len()
            );
            return std::process::ExitCode::FAILURE;
        }
        for r in bytes.as_chunks::<RECORD>().0 {
            read += 1;
            let black = u64::from_le_bytes(r[0..8].try_into().unwrap());
            let white = u64::from_le_bytes(r[8..16].try_into().unwrap());
            let key = if symmetric {
                canonical(black, white)
            } else {
                (black, white)
            };
            if !seen.insert(key) {
                continue;
            }
            if sink.is_none() {
                let p = out_dir.join(format!("dedup_{shard:04}.data"));
                sink = match std::fs::File::create(&p) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        eprintln!("create {}: {e}", p.display());
                        return std::process::ExitCode::FAILURE;
                    }
                };
                written = 0;
            }
            if let Err(e) = sink.as_mut().unwrap().write_all(r) {
                eprintln!("write failed: {e}");
                return std::process::ExitCode::FAILURE;
            }
            kept += 1;
            written += 1;
            if written >= shard_size {
                sink = None;
                shard += 1;
            }
        }
        eprintln!(
            "  {}: {read} read, {kept} kept ({:.1}% dropped)",
            f.display(),
            (read - kept) as f64 * 100.0 / read.max(1) as f64
        );
    }

    println!(
        "{read} positions read, {kept} kept, {} dropped ({:.1}%), {} files, {:.1}s",
        read - kept,
        (read - kept) as f64 * 100.0 / read.max(1) as f64,
        shard + usize::from(written > 0),
        t0.elapsed().as_secs_f64()
    );
    std::process::ExitCode::SUCCESS
}
