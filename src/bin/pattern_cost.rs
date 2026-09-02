//! What a pattern set costs, before any of it is measured on a clock.
//!
//! The two hot operations scale with different things, so "make the set
//! smaller" is not one decision but two:
//!
//! * The incremental index update walks the masks covering each square that
//!   changed, so it scales with **masks per square**. Measured at 34 ns a
//!   move -- the larger half of a 57 ns move.
//! * The read-out touches every table once, so it scales with the **total
//!   mask count**. Measured at 23 ns.
//!
//! A square covered by nine masks costs nine scattered read-modify-writes
//! every time a disc lands on it, and the squares discs actually land on are
//! not the squares the set covers evenly. This prints both distributions,
//! weights the per-square one by how often play reaches that square when a
//! corpus is given, and estimates what dropping each pattern would buy.
//!
//! Usage: pattern_cost [--patterns nnue|egaroucid|compact] [--corpus <file.data>]
use kuroobi::pattern::{Pattern, COMPACT_PATTERNS, EGAROUCID_PATTERNS, NNUE_PATTERNS};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

const REC: usize = 17;

/// Masks covering each of the 64 squares.
fn per_square(patterns: &[Pattern]) -> [usize; 64] {
    let mut n = [0usize; 64];
    for p in patterns {
        for m in p.masks {
            for &sq in m.iter() {
                n[sq as usize] += 1;
            }
        }
    }
    n
}

/// How often each square is occupied across a corpus, as a share of records.
fn square_frequency(path: &PathBuf) -> std::io::Result<[f64; 64]> {
    let bytes = std::fs::read(path)?;
    let mut hits = [0u64; 64];
    let mut n = 0u64;
    for r in bytes.as_chunks::<REC>().0 {
        let black = u64::from_le_bytes(r[0..8].try_into().unwrap());
        let white = u64::from_le_bytes(r[8..16].try_into().unwrap());
        let occ = black | white;
        for (sq, h) in hits.iter_mut().enumerate() {
            if occ >> sq & 1 == 1 {
                *h += 1;
            }
        }
        n += 1;
    }
    let mut f = [0.0f64; 64];
    if n > 0 {
        for (sq, h) in hits.iter().enumerate() {
            f[sq] = *h as f64 / n as f64;
        }
    }
    Ok(f)
}

fn name_of(sq: usize) -> String {
    // Squares are file-major: sq = file * 8 + rank.
    let file = (b'A' + (sq / 8) as u8) as char;
    let rank = sq % 8 + 1;
    format!("{file}{rank}")
}

fn main() -> ExitCode {
    let mut which = String::from("egaroucid");
    let mut corpus: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--patterns" => which = it.next().unwrap_or_default(),
            "--corpus" => corpus = it.next().map(PathBuf::from),
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let patterns: &[Pattern] = match which.as_str() {
        "egaroucid" => EGAROUCID_PATTERNS,
        "compact" => COMPACT_PATTERNS,
        "nnue" => NNUE_PATTERNS,
        other => {
            eprintln!("unknown pattern set {other}");
            return ExitCode::FAILURE;
        }
    };

    let total_masks: usize = patterns.iter().map(|p| p.masks.len()).sum();
    let rows: usize = patterns
        .iter()
        .map(|p| p.masks.len() * 3usize.pow(p.size as u32))
        .sum();
    let n = per_square(patterns);
    let covered = n.iter().filter(|&&c| c > 0).count();
    let sum: usize = n.iter().sum();

    println!(
        "パターン集合 {which}: {} 種、{total_masks} マスク、{rows} 行",
        patterns.len()
    );
    println!(
        "マスあたりのマスク数: 平均 {:.2} (被覆 {covered}/64 マス、最大 {}、最小 {})",
        sum as f64 / covered.max(1) as f64,
        n.iter().max().unwrap(),
        n.iter().filter(|&&c| c > 0).min().unwrap_or(&0)
    );

    // Per-pattern: what it costs on both axes.
    println!("\n| パターン | サイズ | マスク | 行 | 総マスクの | マス被覆の |");
    println!("|---|---:|---:|---:|---:|---:|");
    for p in patterns {
        let cells: usize = p.masks.iter().map(|m| m.len()).sum();
        println!(
            "| {} | {} | {} | {} | {:.1}% | {:.1}% |",
            p.name,
            p.size,
            p.masks.len(),
            p.masks.len() * 3usize.pow(p.size as u32),
            100.0 * p.masks.len() as f64 / total_masks as f64,
            100.0 * cells as f64 / sum as f64
        );
    }

    // The per-square histogram: where the update cost actually sits.
    println!("\n| マスク数 | マス数 | 該当マス |");
    println!("|---:|---:|---|");
    let mut hist: HashMap<usize, Vec<usize>> = HashMap::new();
    for (sq, &c) in n.iter().enumerate() {
        hist.entry(c).or_default().push(sq);
    }
    let mut keys: Vec<usize> = hist.keys().copied().collect();
    keys.sort_unstable_by(|a, b| b.cmp(a));
    for k in keys {
        let v = &hist[&k];
        let names: Vec<String> = v.iter().take(12).map(|&s| name_of(s)).collect();
        println!(
            "| {k} | {} | {}{} |",
            v.len(),
            names.join(" "),
            if v.len() > 12 { " …" } else { "" }
        );
    }

    // Weighting by how often a square is actually occupied turns "masks per
    // square" into "masks the update really walks". A corner is covered by
    // many masks but reached late; the centre is reached in every game.
    if let Some(c) = corpus {
        match square_frequency(&c) {
            Ok(f) => {
                let weighted: f64 = (0..64).map(|s| n[s] as f64 * f[s]).sum();
                let occupied: f64 = f.iter().sum();
                println!(
                    "\nコーパス {} で重み付け: 占有マスあたり {:.2} マスク (単純平均 {:.2})",
                    c.display(),
                    weighted / occupied.max(1e-9),
                    sum as f64 / covered.max(1) as f64
                );
                let mut by_sq: Vec<(usize, f64)> =
                    (0..64).map(|s| (s, n[s] as f64 * f[s])).collect();
                by_sq.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
                let top: Vec<String> = by_sq
                    .iter()
                    .take(8)
                    .map(|(s, w)| format!("{}({:.2})", name_of(*s), w))
                    .collect();
                println!("更新コストの重いマス: {}", top.join(" "));
            }
            Err(e) => {
                eprintln!("failed to read {}: {e}", c.display());
                return ExitCode::FAILURE;
            }
        }
    }

    // What each pattern would give back if it were dropped, on both axes.
    println!("\n| 落とす候補 | 総マスク減 | マスあたり平均 | 行数減 |");
    println!("|---|---:|---:|---:|");
    for (i, p) in patterns.iter().enumerate() {
        let kept: Vec<Pattern> = patterns
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, q)| Pattern {
                name: q.name,
                size: q.size,
                masks: q.masks,
            })
            .collect();
        let k = per_square(&kept);
        let kc = k.iter().filter(|&&c| c > 0).count();
        let ks: usize = k.iter().sum();
        println!(
            "| {} | {} → {} | {:.2} → {:.2} | -{} |",
            p.name,
            total_masks,
            total_masks - p.masks.len(),
            sum as f64 / covered.max(1) as f64,
            ks as f64 / kc.max(1) as f64,
            p.masks.len() * 3usize.pow(p.size as u32)
        );
    }
    ExitCode::SUCCESS
}
