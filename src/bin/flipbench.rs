//! Microbenchmark of the flip and move-generation routines, mirroring the C
//! benchmark used to time Edax's `flip()` and `get_moves()` so the two sets
//! of numbers are directly comparable.
//!
//! Treat the results as a pointer to where cost might be, never as a verdict.
//! Random squares defeat the indirect-branch predictor and random positions
//! stretch the fill chains, so both are pessimistic in different directions
//! from what the search actually sees: dispatching flips through a function
//! table measured 14.3 ns here against roughly 2.6 ns in a real solve, and
//! scalar move generation wins here while losing in the solver. Decide
//! adoption on FFO40-59, not on this.
use kuroobi::bitboard;
use std::time::Instant;

const N: usize = 1 << 16;
const ROUNDS: usize = 200;

fn bench(label: &str, mut f: impl FnMut() -> u64) -> u64 {
    let mut acc = f();
    let t0 = Instant::now();
    for _ in 0..ROUNDS {
        acc ^= f();
    }
    let ns = t0.elapsed().as_nanos() as f64 / (ROUNDS * N) as f64;
    println!("{label:26} {ns:6.3} ns/call");
    acc
}

fn main() {
    let mut p = vec![0u64; N];
    let mut o = vec![0u64; N];
    let mut x = vec![0u32; N];
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for i in 0..N {
        let a = next();
        let b = next();
        p[i] = a & !b;
        o[i] = b & !a;
        x[i] = (b % 64) as u32;
    }

    let mut acc = 0u64;
    acc ^= bench("flip (square varies)", || {
        (0..N).fold(0, |a, i| a ^ bitboard::flippable(p[i], o[i], 1 << x[i]))
    });
    // Same work with the square fixed, so the call site folds to one square's
    // ray masks. The gap against the line above is the dispatch, not the flip.
    acc ^= bench("flip (square fixed, D4)", || {
        (0..N).fold(0, |a, i| a ^ bitboard::flippable(p[i], o[i], 1 << 27))
    });
    acc ^= bench("mobility (NEON)", || {
        (0..N).fold(0, |a, i| a ^ bitboard::mobility(p[i], o[i], !(p[i] | o[i])))
    });
    acc ^= bench("mobility (scalar)", || {
        (0..N).fold(0, |a, i| a ^ bitboard::mobility_scalar(p[i], o[i], !(p[i] | o[i])))
    });
    println!("acc={acc}");
}
