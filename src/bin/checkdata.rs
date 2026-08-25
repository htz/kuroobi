//! Check that a ground-truth file says what it claims: solve every position
//! again and compare against the stored label.
//!
//! Everything downstream trusts these labels absolutely -- they are the
//! yardstick every model is scored by -- so a wrong one does not show up as
//! an error, it shows up as a model that looks bad, or good, for no reason.
//! The check has to be independent of any model: read the record exactly as
//! the scorer reads it (side to move is Black, by the format's convention),
//! solve that position, and require the answer to match.
//!
//! Usage: checkdata [--threads n] [--limit n] <file.data>...
use kuroobi::evaluator::Evaluator;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::{Board, Color};

fn main() {
    let mut threads = 6usize;
    let mut limit = usize::MAX;
    let mut files: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(threads),
            "--limit" => limit = it.next().and_then(|v| v.parse().ok()).unwrap_or(limit),
            other if other.starts_with("--") => panic!("unknown flag {other}"),
            other => files.push(other.to_string()),
        }
    }

    let mut ev = Evaluator::new(EGAROUCID_PATTERNS);
    ev.load_weights(std::path::Path::new("weights/linear.bin"))
        .expect("linear weights");
    let ev = &ev;

    for f in &files {
        let bytes = std::fs::read(f).unwrap_or_else(|e| panic!("read {f}: {e}"));
        // Refuse a file that is not a whole number of records rather than
        // checking the prefix that happens to fit: a checker that silently
        // covers a subset is the failure mode it exists to prevent.
        assert_eq!(
            bytes.len() % 17,
            0,
            "{f}: {} bytes is not a whole number of 17-byte records",
            bytes.len()
        );
        let n = (bytes.len() / 17).min(limit);
        let boards: Vec<(Board, i32)> = (0..n)
            .map(|i| {
                let r = &bytes[i * 17..i * 17 + 17];
                let black = u64::from_le_bytes(r[0..8].try_into().unwrap());
                let white = u64::from_le_bytes(r[8..16].try_into().unwrap());
                (
                    Board {
                        black,
                        white,
                        player: Color::Black,
                        empty_count: 64 - (black | white).count_ones() as u8,
                    },
                    r[16] as i8 as i32,
                )
            })
            .collect();

        let next = std::sync::atomic::AtomicUsize::new(0);
        let bad = std::sync::Mutex::new(Vec::<(usize, i32, i32)>::new());
        // A negated label is the failure this is most likely to meet: the
        // generator flips a White-to-move position into the format's
        // Black-to-move convention, and whether the value flips with it
        // depends on which perspective the solver reports. Count them
        // separately so the report names the cause instead of just the count.
        let negated = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|sc| {
            for _ in 0..threads {
                sc.spawn(|| {
                    let mut solver = Solver::new(22);
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= boards.len() {
                            break;
                        }
                        let (b, label) = &boards[i];
                        let got = solver
                            .solve_with_eval(EndSolverMode::Perfect, b, Some(ev))
                            .value;
                        if got != *label {
                            if got == -*label {
                                negated.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            bad.lock().unwrap().push((i, *label, got));
                        }
                    }
                });
            }
        });

        let mut bad = bad.into_inner().unwrap();
        bad.sort_unstable();
        let neg = negated.load(std::sync::atomic::Ordering::Relaxed);
        if bad.is_empty() {
            println!("{f}: {n} positions, every label matches a fresh solve");
        } else {
            println!(
                "{f}: {} of {n} labels wrong ({} of them are the value negated)",
                bad.len(),
                neg
            );
            for (i, label, got) in bad.iter().take(5) {
                println!("  record {i}: file says {label}, solver says {got}");
            }
        }
    }
}
