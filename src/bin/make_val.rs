//! Build a held-out validation set with a fixed quota per stage.
//!
//! The metric is the whole point of a training run, so how the set was built
//! has to be reproducible from the command line and checkable afterwards.
//! Three things went wrong with the set this replaces, each of which this
//! tool makes impossible or visible:
//!
//! 1. **Uneven stages.** It held between 1 and 4000 positions per stage. A
//!    stage scored on one position reports a number that moves by discs
//!    between epochs and means nothing. A quota is required, and stages that
//!    cannot meet it are named in the output rather than quietly thinned.
//! 2. **Unknown provenance.** 4.7% of its records were in none of the files
//!    it was supposed to come from. Every record here is copied verbatim
//!    from a named source, so the set is always a subset of its inputs.
//! 3. **Unproven holdout.** Nothing checked that the positions were absent
//!    from training. `--exclude` walks a corpus and drops anything that
//!    appears in it, and reports how many it dropped. The test holds the
//!    candidates and streams the corpus, never the reverse: the corpus is
//!    1.29 billion positions and would not fit.
//!
//! Sampling is a deterministic stride through each stage's positions, not a
//! random draw: the same inputs and quota give the same file, so a number
//! measured today can be compared with one measured next week.
//!
//! The opening is the exception. Stage 4 exists in 60 distinct positions in
//! this corpus and stage 1 in one; there is no held-out sample to take, and
//! excluding what training saw would leave those stages empty. They matter
//! anyway -- a shallow search reaches them and cannot fall back on the book
//! -- so `--overlap-upto` lets stages at or below a threshold keep positions
//! training has also seen. A stage covered that way is measuring fit, not
//! generalisation, and the output says so per stage.
//!
//! Records are copied whole, so the set keeps every field of its sources;
//! the trainer's filter flags (`--min-ply`, `--max-score-diff`,
//! `--drop-random`, `--keep-above-ply`) apply here too, so a set holds only
//! positions a run with the same flags would train on.
//!
//! Usage:
//!   make_val --out <file.data> [--per-stage N] [--overlap-upto N]
//!            [--exclude <dir-or-file>]... [filter flags] <source.data>...
use kuroobi::evaluator::STAGE_COUNT;
use kuroobi::record::{self, Filter, Record};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const REC: usize = record::SIZE;

/// Read a file as whole records, calling `f` for each.
fn for_each_record(path: &Path, mut f: impl FnMut(&[u8; REC])) -> std::io::Result<u64> {
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; REC * 1024 * 64];
    let mut carry = 0usize;
    let mut n = 0u64;
    loop {
        let got = match file.read(&mut buf[carry..])? {
            0 => break,
            g => g + carry,
        };
        let whole = got / REC * REC;
        for r in buf[..whole].as_chunks::<REC>().0 {
            f(r);
            n += 1;
        }
        carry = got - whole;
        buf.copy_within(whole..got, 0);
    }
    Ok(n)
}

/// Stage of a record, read the same way the trainer reads it.
fn stage_of(r: &Record) -> usize {
    60usize
        .saturating_sub(usize::from(r.empties()))
        .min(STAGE_COUNT - 1)
}

/// Every `.data` file under `path`, or `path` itself if it is one.
fn data_files(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        let mut out: Vec<PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "data"))
            .collect();
        out.sort();
        out
    } else {
        vec![path.to_path_buf()]
    }
}

fn main() -> ExitCode {
    let mut out: Option<PathBuf> = None;
    let mut per_stage = 4000usize;
    let mut overlap_upto: i64 = -1;
    let mut excludes: Vec<PathBuf> = Vec::new();
    let mut sources: Vec<PathBuf> = Vec::new();
    let mut filter = Filter::NONE;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match filter.take_flag(&a, &mut it) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
        match a.as_str() {
            "--out" => out = it.next().map(PathBuf::from),
            "--per-stage" => match it.next().map(|v| v.parse()) {
                Some(Ok(n)) => per_stage = n,
                _ => {
                    eprintln!("--per-stage needs a number");
                    return ExitCode::FAILURE;
                }
            },
            "--overlap-upto" => match it.next().map(|v| v.parse()) {
                Some(Ok(n)) => overlap_upto = n,
                _ => {
                    eprintln!("--overlap-upto needs a number");
                    return ExitCode::FAILURE;
                }
            },
            "--exclude" => match it.next() {
                Some(v) => excludes.push(PathBuf::from(v)),
                None => {
                    eprintln!("--exclude needs a path");
                    return ExitCode::FAILURE;
                }
            },
            other if other.starts_with("--") => {
                eprintln!("unknown flag {other}");
                return ExitCode::FAILURE;
            }
            f => sources.push(PathBuf::from(f)),
        }
    }
    let Some(out) = out else {
        eprintln!(
            "usage: make_val --out <file.data> [--per-stage N] [--exclude <path>]... <source.data>..."
        );
        return ExitCode::FAILURE;
    };
    if sources.is_empty() {
        eprintln!("no source files given");
        return ExitCode::FAILURE;
    }

    // Pass 1: bucket every source position by stage, keeping the board bytes
    // so duplicates across sources collapse.
    let mut by_stage: Vec<Vec<[u8; REC]>> = vec![Vec::new(); STAGE_COUNT];
    let mut seen: HashSet<[u8; 16]> = HashSet::new();
    let mut dup = 0u64;
    let mut filtered = 0u64;
    for p in &sources {
        let mut kept = 0u64;
        let total = match for_each_record(p, |r| {
            let rec = Record::from_bytes(r);
            if !filter.keeps(&rec) {
                filtered += 1;
                return;
            }
            let key: [u8; 16] = r[..16].try_into().unwrap();
            if seen.insert(key) {
                by_stage[stage_of(&rec)].push(*r);
                kept += 1;
            } else {
                dup += 1;
            }
        }) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("failed to read {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        };
        println!("source {}: {total} records, {kept} new", p.display());
    }
    println!("filter {}: {filtered} records dropped", filter.describe());
    println!("{dup} duplicate boards dropped");

    // Pass 2: drop anything that appears in the excluded corpora. Done after
    // bucketing so the report can say what each stage lost.
    if !excludes.is_empty() {
        // Hold the candidates, not the corpus. The training corpus here is
        // 1.29 billion positions -- 22 GB on disk and far more as a hash set
        // -- and loading it took 9.5 GB before it was a third of the way in,
        // on a 64 GB machine that was also running ten training processes.
        // The candidates are three million, so the membership test goes the
        // other way round and the corpus streams past.
        let mut candidates: HashSet<[u8; 16]> = HashSet::new();
        for (st, bucket) in by_stage.iter().enumerate() {
            if st as i64 <= overlap_upto {
                continue;
            }
            for r in bucket {
                candidates.insert(r[..16].try_into().unwrap());
            }
        }
        let mut leaked: HashSet<[u8; 16]> = HashSet::new();
        for e in &excludes {
            for p in data_files(e) {
                if let Err(err) = for_each_record(&p, |r| {
                    let key: [u8; 16] = r[..16].try_into().unwrap();
                    if candidates.contains(&key) {
                        leaked.insert(key);
                    }
                }) {
                    eprintln!("failed to read {}: {err}", p.display());
                    return ExitCode::FAILURE;
                }
            }
            println!("exclude {}: scanned", e.display());
        }
        let mut removed = 0u64;
        for (st, bucket) in by_stage.iter_mut().enumerate() {
            if st as i64 <= overlap_upto {
                continue;
            }
            bucket.retain(|r| {
                let key: [u8; 16] = r[..16].try_into().unwrap();
                if leaked.contains(&key) {
                    removed += 1;
                    false
                } else {
                    true
                }
            });
        }
        println!("{removed} positions dropped as present in the excluded corpus");
        if removed > 0 {
            println!("  (a nonzero count here means the previous split leaked)");
        }
    }

    // Pass 3: take an even stride so the sample spans the whole of each
    // stage's pool rather than whichever games happened to come first.
    let mut writer = match File::create(&out) {
        Ok(f) => BufWriter::with_capacity(1 << 20, f),
        Err(e) => {
            eprintln!("failed to create {}: {e}", out.display());
            return ExitCode::FAILURE;
        }
    };
    println!("| stage | empties | available | taken | held-out |");
    println!("|---:|---:|---:|---:|---|");
    let mut short: Vec<(usize, usize)> = Vec::new();
    let mut written = 0u64;
    for (st, bucket) in by_stage.iter().enumerate() {
        if bucket.is_empty() {
            continue;
        }
        let take = per_stage.min(bucket.len());
        if bucket.len() < per_stage {
            short.push((st, bucket.len()));
        }
        let stride = bucket.len() / take;
        for i in 0..take {
            if let Err(e) = writer.write_all(&bucket[i * stride]) {
                eprintln!("write error: {e}");
                return ExitCode::FAILURE;
            }
            written += 1;
        }
        println!(
            "| {} | {} | {} | {} | {} |",
            st,
            60 - st,
            bucket.len(),
            take,
            if st as i64 <= overlap_upto {
                "overlap allowed"
            } else {
                "yes"
            }
        );
    }
    if let Err(e) = writer.flush() {
        eprintln!("flush error: {e}");
        return ExitCode::FAILURE;
    }
    println!("\n{written} positions written to {}", out.display());
    if short.is_empty() {
        println!("every stage present met the quota of {per_stage}");
    } else {
        println!("below the quota of {per_stage}: {short:?}");
    }
    ExitCode::SUCCESS
}
