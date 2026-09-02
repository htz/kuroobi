//! Times the primitives the exact solve leans on, over a corpus of real
//! endgame positions, in a form a second implementation can reproduce
//! exactly.
//!
//! Two things make this different from `flipbench`, and both matter:
//!
//! * **The positions and squares come from play, not from a generator.**
//!   Random bitboards and random squares defeat the branch predictor and
//!   stretch the fill chains, which is why `flipbench` measures 14.3 ns for
//!   a dispatched flip that costs about 2.6 ns inside a solve. Cases here
//!   are sampled by playing legal moves down to a target empty count, and
//!   the square is always one of that position's legal moves.
//! * **The corpus is a file, so two implementations time the same work.**
//!   Each run prints a checksum per primitive; if the checksums do not
//!   match across implementations, the comparison is meaningless and the
//!   numbers must be thrown away, not interpreted.
//!
//! Even so: a microbenchmark reproduces neither the cache state nor the
//! branch history of a search. Use it to find where to look, never to
//! decide what to keep. Adoption is decided on the full problem sets.
//!
//! Usage:
//!   microbench gen <file.obf> <empties> <count> > corpus.txt
//!   microbench run <corpus.txt> [rounds] [trials]
use kuroobi::board::Board;
use kuroobi::position::Position;
use kuroobi::{bitboard, stability, zobrist};
use std::time::Instant;

struct Case {
    player: u64,
    opponent: u64,
    sq: u8,
}

/// xorshift64, so the corpus is reproducible from the seed alone.
fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// Plays legal moves at random until `target` empties remain, passing when
/// forced and giving up if the game ends first.
fn playout(mut b: Board, target: u32, s: &mut u64) -> Option<Board> {
    loop {
        if b.empty().count_ones() == target {
            return Some(b);
        }
        if b.empty().count_ones() < target {
            return None;
        }
        let mut m = b.movable();
        if m == 0 {
            b.pass();
            if b.movable() == 0 {
                return None;
            }
            m = b.movable();
        }
        let k = (next(s) % m.count_ones() as u64) as u32;
        let mut left = k;
        let mut bit = m;
        loop {
            let sq = bit.trailing_zeros() as u8;
            if left == 0 {
                b.make_move_bits(Position(sq));
                break;
            }
            left -= 1;
            bit &= bit - 1;
        }
    }
}

fn gen(path: &str, empties: u32, count: usize) {
    let text = std::fs::read_to_string(path).expect("read obf");
    let boards: Vec<Board> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| Board::from_string(l.split(';').next().unwrap_or(l)).ok())
        .collect();
    assert!(!boards.is_empty(), "no positions parsed from {path}");
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    let mut done = 0usize;
    // Bounded so a target the file cannot reach fails loudly instead of
    // spinning.
    for attempt in 0..count * 200 {
        if done == count {
            break;
        }
        let b = boards[attempt % boards.len()];
        let Some(p) = playout(b, empties, &mut s) else {
            continue;
        };
        let m = p.movable();
        if m == 0 {
            continue;
        }
        let k = (next(&mut s) % m.count_ones() as u64) as u32;
        let mut left = k;
        let mut bit = m;
        let sq = loop {
            let sq = bit.trailing_zeros() as u8;
            if left == 0 {
                break sq;
            }
            left -= 1;
            bit &= bit - 1;
        };
        println!("{:016x} {:016x} {}", p.player_bb(), p.opponent_bb(), sq);
        done += 1;
    }
    assert_eq!(
        done, count,
        "could not reach {empties} empties {count} times"
    );
}

fn read(path: &str) -> Vec<Case> {
    std::fs::read_to_string(path)
        .expect("read corpus")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            Case {
                player: u64::from_str_radix(it.next().unwrap(), 16).unwrap(),
                opponent: u64::from_str_radix(it.next().unwrap(), 16).unwrap(),
                sq: it.next().unwrap().parse().unwrap(),
            }
        })
        .collect()
}

/// Runs `f` over the whole corpus `rounds` times, `trials` times over, and
/// reports the fastest trial. The minimum is the right statistic under a
/// shared machine: interference can only slow a trial down.
fn bench(label: &str, n: usize, rounds: usize, trials: usize, mut f: impl FnMut() -> u64) {
    let mut sink = f();
    let mut best = f64::INFINITY;
    for _ in 0..trials {
        let t0 = Instant::now();
        for _ in 0..rounds {
            sink ^= f();
        }
        let ns = t0.elapsed().as_nanos() as f64 / (rounds * n) as f64;
        best = best.min(ns);
    }
    println!("{label:24} {best:7.3} ns/op   checksum {sink:016x}");
}

fn run(path: &str, rounds: usize, trials: usize) {
    let c = read(path);
    let n = c.len();
    println!("corpus {path}  {n} cases  rounds {rounds}  trials {trials}");

    bench("flip", n, rounds, trials, || {
        let mut a = 0u64;
        for k in &c {
            a ^= bitboard::flippable(k.player, k.opponent, 1u64 << k.sq);
        }
        a
    });
    bench("moves", n, rounds, trials, || {
        let mut a = 0u64;
        for k in &c {
            a ^= bitboard::mobility(k.player, k.opponent, !(k.player | k.opponent));
        }
        a
    });
    bench("stable_count", n, rounds, trials, || {
        let mut a = 0u64;
        for k in &c {
            a = a.wrapping_add(stability::stable_count(k.player, k.opponent) as u64);
        }
        a
    });
    // The one-empty leaf, the most-executed routine in the search. Mirrors
    // `last1` minus its profiling and node counting: one popcount for the
    // difference, one shared gather for both sides' flip counts.
    bench("last1", n, rounds, trials, || {
        let mut a = 0u64;
        for k in &c {
            let diff = 2 * k.player.count_ones() as i32 - 63;
            let (mine, theirs) = bitboard::count_last_flips(k.player, k.sq);
            let v = if mine > 0 {
                diff + 2 * mine as i32 + 1
            } else if theirs > 0 {
                diff - 2 * theirs as i32 - 1
            } else if diff > 0 {
                diff + 1
            } else if diff < 0 {
                diff - 1
            } else {
                0
            };
            a = a.wrapping_add(v as i64 as u64);
        }
        a
    });
    // The gather alone, without the scoring arithmetic, to attribute the
    // difference against another implementation's combined routine.
    bench("last1:count", n, rounds, trials, || {
        let mut a = 0u64;
        for k in &c {
            let (mine, theirs) = bitboard::count_last_flips(k.player, k.sq);
            a = a
                .wrapping_add(mine as u64)
                .wrapping_add((theirs as u64) << 32);
        }
        a
    });
    bench("hash", n, rounds, trials, || {
        let mut a = 0u64;
        for k in &c {
            a ^= zobrist::board_hash(k.player, k.opponent);
        }
        a
    });
    bench("make_move", n, rounds, trials, || {
        let mut a = 0u64;
        for k in &c {
            let f = bitboard::flippable(k.player, k.opponent, 1u64 << k.sq);
            a ^= (k.opponent ^ f) ^ (k.player | f | (1u64 << k.sq));
        }
        a
    });

    // Move-list build: legal squares, batched flips, child hashes - the
    // work gen_moves does per node minus the table prefetches.
    bench("movelist", n, rounds, trials, || {
        let mut a = 0u64;
        for k in &c {
            let mut sqs = [0u8; 34];
            let mut m = bitboard::mobility(k.player, k.opponent, !(k.player | k.opponent));
            let mut cnt = 0usize;
            while m != 0 {
                sqs[cnt] = m.trailing_zeros() as u8;
                m &= m - 1;
                cnt += 1;
            }
            let mut i = 0;
            while i + 4 <= cnt {
                let (f0, f1, f2, f3) = bitboard::flippable4(
                    k.player,
                    k.opponent,
                    sqs[i],
                    sqs[i + 1],
                    sqs[i + 2],
                    sqs[i + 3],
                );
                for (j, f) in [f0, f1, f2, f3].into_iter().enumerate() {
                    let bit = 1u64 << sqs[i + j];
                    a ^= zobrist::board_hash(k.opponent ^ f, k.player | f | bit);
                }
                i += 4;
            }
            while i < cnt {
                let f = bitboard::flippable(k.player, k.opponent, 1u64 << sqs[i]);
                let bit = 1u64 << sqs[i];
                a ^= zobrist::board_hash(k.opponent ^ f, k.player | f | bit);
                i += 1;
            }
            a = a.wrapping_add(cnt as u64);
        }
        a
    });

    let tt_cases: Vec<(u64, u64)> = c.iter().map(|k| (k.player, k.opponent)).collect();
    let (ons, omoves) = kuroobi::solver::Solver::bench_order(&tt_cases, rounds);
    println!("order_static          {ons:7.3} ns/move ({omoves} moves)");
    let (l2, l3, l4, ncase) = kuroobi::solver::Solver::bench_leaves(&tt_cases, rounds);
    if ncase > 0 || l3 > 0.0 || l4 > 0.0 {
        println!("last2                 {l2:7.3} ns/pos");
        println!("last3                 {l3:7.3} ns/pos");
        println!("last4                 {l4:7.3} ns/pos");
    }
    let (st, hit, miss) = kuroobi::solver::Solver::bench_tt(&tt_cases, rounds);
    println!("tt_store              {st:7.3} ns/op");
    println!("tt_probe_hit          {hit:7.3} ns/op");
    println!("tt_probe_miss         {miss:7.3} ns/op");
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("gen") if a.len() >= 4 => gen(
            &a[1],
            a[2].parse().expect("empties"),
            a[3].parse().expect("count"),
        ),
        Some("run") if a.len() >= 2 => run(
            &a[1],
            a.get(2).and_then(|v| v.parse().ok()).unwrap_or(200),
            a.get(3).and_then(|v| v.parse().ok()).unwrap_or(5),
        ),
        _ => {
            eprintln!("usage: microbench gen <file.obf> <empties> <count>");
            eprintln!("       microbench run <corpus.txt> [rounds] [trials]");
            std::process::exit(2);
        }
    }
}
