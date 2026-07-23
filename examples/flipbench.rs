//! 石返しと着手生成の単体計測。Edax の同等ベンチ (scratchpad の
//! flipbench/bench.c) と同じ乱数列を使うので、数値を直接比較できる。
//!
//! 注意: マイクロベンチは実探索の分岐予測を再現しないので過大評価する。
//! 採否は必ず FFO40-59 全問の実測で決めること。
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
    println!("{label:28} {ns:6.3} ns/call");
    acc
}

fn main() {
    let (mut p, mut o, mut x) = (vec![0u64; N], vec![0u64; N], vec![0usize; N]);
    let mut s: u64 = 0x2545_F491_4F6C_DD1D;
    let next = |s: &mut u64| {
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        *s
    };
    for i in 0..N {
        let a = next(&mut s);
        let b = next(&mut s);
        p[i] = a & !b;
        o[i] = b & !a;
        x[i] = (s % 64) as usize;
    }

    let mut acc = 0u64;
    acc ^= bench("flip (マス可変)", || {
        (0..N).fold(0, |a, i| a ^ bitboard::flippable(p[i], o[i], 1 << x[i]))
    });
    acc ^= bench("flip (マス固定 D4)", || {
        (0..N).fold(0, |a, i| a ^ bitboard::flippable(p[i], o[i], 1 << 27))
    });
    acc ^= bench("mobility (NEON)", || {
        (0..N).fold(0, |a, i| a ^ bitboard::mobility(p[i], o[i], !(p[i] | o[i])))
    });
    acc ^= bench("mobility (スカラ)", || {
        (0..N).fold(0, |a, i| a ^ bitboard::mobility_scalar(p[i], o[i], !(p[i] | o[i])))
    });
    println!("acc={acc}");
}
