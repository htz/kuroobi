//! Count how many distinct positions a random opening of N plies can reach.
//!
//! The generator's diversity is bounded by this number: after the random
//! opening the search is deterministic, so two games that share an opening
//! position are the same game. Four random plies reach 236 positions, which
//! is why a corpus built that way came out 56% duplicate.
//!
//! Sampling, not enumeration: the space is far too large to walk at the N
//! that matter, and what the generator needs to know is not the true size
//! but how much of it a given number of games would actually hit.
//!
//! Usage: openings [--plies 4,8,12,16] [--samples N] [--seed N]

use std::collections::HashSet;

use kuroobi::{Board, Position};

fn main() {
    let mut plies: Vec<u32> = vec![4, 8, 12, 16, 20, 24];
    let mut samples: usize = 1_000_000;
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--plies" => {
                plies = it
                    .next()
                    .unwrap()
                    .split(',')
                    .map(|s| s.trim().parse().unwrap())
                    .collect()
            }
            "--samples" => samples = it.next().unwrap().parse().unwrap(),
            "--seed" => seed = it.next().unwrap().parse().unwrap(),
            other => panic!("unknown flag {other}"),
        }
    }

    println!("{samples} samples per depth\n");
    println!("plies    reached   distinct  duplicate  hit/position");
    for &n in &plies {
        let mut s = seed ^ (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        let mut seen: HashSet<(u64, u64)> = HashSet::with_capacity(samples);
        let mut reached = 0usize;
        for _ in 0..samples {
            let mut b = Board::new();
            let mut ok = true;
            for _ in 0..n {
                let m = b.movable();
                if m == 0 {
                    b.pass();
                    if b.movable() == 0 {
                        ok = false;
                        break;
                    }
                    continue;
                }
                let k = (rnd() % m.count_ones() as u64) as u32;
                let mut mm = m;
                for _ in 0..k {
                    mm &= mm - 1;
                }
                let _ = b.make_move(Position(mm.trailing_zeros() as u8));
            }
            if ok {
                reached += 1;
                seen.insert((b.black, b.white));
            }
        }
        let d = seen.len();
        println!(
            "{n:5}  {reached:9}  {d:9}  {:8.2}%  {:9.2}",
            (reached - d) as f64 * 100.0 / reached as f64,
            reached as f64 / d as f64
        );
    }
}
