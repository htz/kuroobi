//! Microbenchmark of the flip routine, mirroring the C benchmark used to
//! time Edax's `flip()` so the two numbers are directly comparable.
use kuroobi::bitboard;
use std::time::Instant;

fn main() {
    const N: usize = 1 << 16;
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
    for i in 0..N {
        acc ^= bitboard::flippable(p[i], o[i], 1u64 << x[i]);
    }
    let t0 = Instant::now();
    for _ in 0..200 {
        for i in 0..N {
            acc ^= bitboard::flippable(p[i], o[i], 1u64 << x[i]);
        }
    }
    let ns = t0.elapsed().as_nanos() as f64 / (200.0 * N as f64);
    println!("ours flip: {ns:.3} ns/call (acc={acc})");
}
