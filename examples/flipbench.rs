use kuroobi::bitboard;
use std::time::Instant;

fn main() {
    const N: usize = 1 << 16;
    let (mut p, mut o, mut x) = (vec![0u64; N], vec![0u64; N], vec![0usize; N]);
    let mut s: u64 = 0x2545F4914F6CDD1D;
    let mut next = |s: &mut u64| { *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17; *s };
    for i in 0..N {
        let a = next(&mut s);
        let b = next(&mut s);
        p[i] = a & !b; o[i] = b & !a; x[i] = (s % 64) as usize;
    }
    let mut acc = 0u64;

    // (1) 実運用と同じ間接呼び出し
    for i in 0..N { acc ^= bitboard::flippable(p[i], o[i], 1u64 << x[i]); }
    let t0 = Instant::now();
    for _ in 0..200 { for i in 0..N { acc ^= bitboard::flippable(p[i], o[i], 1u64 << x[i]); } }
    let ns = t0.elapsed().as_nanos() as f64 / (200.0 * N as f64);
    println!("(1) 間接呼び出し (実運用と同じ) : {ns:.3} ns/call");

    // (2) マスを固定 = 直接呼び出し・完全インライン化
    let t0 = Instant::now();
    for _ in 0..200 { for i in 0..N { acc ^= bitboard::flippable(p[i], o[i], 1u64 << 27); } }
    let ns = t0.elapsed().as_nanos() as f64 / (200.0 * N as f64);
    println!("(2) マス固定 (D4, 中央)        : {ns:.3} ns/call");

    let t0 = Instant::now();
    for _ in 0..200 { for i in 0..N { acc ^= bitboard::flippable(p[i], o[i], 1u64 << 0); } }
    let ns = t0.elapsed().as_nanos() as f64 / (200.0 * N as f64);
    println!("(3) マス固定 (A1, 隅)          : {ns:.3} ns/call");
    println!("acc={acc}");
}
