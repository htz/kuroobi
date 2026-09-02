//! Split a training corpus into one file per game stage.
//!
//! Training a few stages costs a full pass over the corpus even though the
//! other stages' examples are thrown away: 30 GB read per epoch to update
//! tables that 400 MB of it touches. Written out once per stage, a run
//! aimed at one stage reads only that stage, which fits in RAM and turns
//! every epoch after the first into pure compute.
//!
//! Stage is moves played (`60 - empties`), the same index the weight tables
//! use, so `stage_19.data` is exactly what `--stages 19-19` would keep.
//!
//! Usage: split_by_stage --out <dir> <data.bin>...
use kuroobi::evaluator::STAGE_COUNT;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const REC: usize = 17;
/// Records buffered per stage before hitting the disk. 64 Ki records is a
/// megabyte a stage, so all 61 open writers together stay well inside cache.
const FLUSH_AT: usize = 64 * 1024;

fn main() -> ExitCode {
    let mut out_dir: Option<PathBuf> = None;
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_dir = it.next().map(PathBuf::from),
            other if other.starts_with("--") => {
                eprintln!("unknown flag {other}");
                return ExitCode::FAILURE;
            }
            f => inputs.push(PathBuf::from(f)),
        }
    }
    let Some(out_dir) = out_dir else {
        eprintln!("usage: split_by_stage --out <dir> <data.bin>...");
        return ExitCode::FAILURE;
    };
    if inputs.is_empty() {
        eprintln!("no input files");
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("failed to create {}: {e}", out_dir.display());
        return ExitCode::FAILURE;
    }

    let mut writers: Vec<BufWriter<File>> = Vec::with_capacity(STAGE_COUNT);
    for st in 0..STAGE_COUNT {
        let p = out_dir.join(format!("stage_{st:02}.data"));
        match File::create(&p) {
            Ok(f) => writers.push(BufWriter::with_capacity(1 << 20, f)),
            Err(e) => {
                eprintln!("failed to create {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
        }
    }
    let mut bufs: Vec<Vec<u8>> = (0..STAGE_COUNT)
        .map(|_| Vec::with_capacity(FLUSH_AT * REC))
        .collect();
    let mut counts = vec![0u64; STAGE_COUNT];

    let mut chunk = vec![0u8; REC * 1024 * 1024];
    for path in &inputs {
        let mut f = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("failed to open {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let mut carry = 0usize;
        loop {
            let n = match f.read(&mut chunk[carry..]) {
                Ok(0) => break,
                Ok(n) => n + carry,
                Err(e) => {
                    eprintln!("read error on {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            let whole = n / REC * REC;
            for r in chunk[..whole].as_chunks::<REC>().0 {
                let black = u64::from_le_bytes(r[0..8].try_into().unwrap());
                let white = u64::from_le_bytes(r[8..16].try_into().unwrap());
                let empties = 64 - (black | white).count_ones() as usize;
                let st = 60usize.saturating_sub(empties).min(STAGE_COUNT - 1);
                bufs[st].extend_from_slice(r);
                counts[st] += 1;
                if bufs[st].len() >= FLUSH_AT * REC {
                    if let Err(e) = writers[st].write_all(&bufs[st]) {
                        eprintln!("write error on stage {st}: {e}");
                        return ExitCode::FAILURE;
                    }
                    bufs[st].clear();
                }
            }
            carry = n - whole;
            chunk.copy_within(whole..n, 0);
        }
        eprintln!("read {}", path.display());
    }
    for st in 0..STAGE_COUNT {
        if !bufs[st].is_empty() {
            if let Err(e) = writers[st].write_all(&bufs[st]) {
                eprintln!("write error on stage {st}: {e}");
                return ExitCode::FAILURE;
            }
        }
        if let Err(e) = writers[st].flush() {
            eprintln!("flush error on stage {st}: {e}");
            return ExitCode::FAILURE;
        }
    }
    println!("| stage | 空き | 局面数 | MB |");
    println!("|---:|---:|---:|---:|");
    for (st, &c) in counts.iter().enumerate() {
        if c > 0 {
            println!(
                "| {} | {} | {} | {:.0} |",
                st,
                60 - st,
                c,
                c as f64 * REC as f64 / 1e6
            );
        }
    }
    println!("total {} records", counts.iter().sum::<u64>());
    ExitCode::SUCCESS
}
