//! 中盤の並列探索を 1 スレッドと突き合わせる。
//!
//! 同じ深さ・同じ設定なら 1 スレッドの答えが正解。並列で違う手や値が出たら、
//! それは並列化の欠陥であって「探索の揺らぎ」ではない。**読切側は 1 局面や
//! FFO では見つからず、この形の検証器で初めて出た。**
//!
//! Usage: stress_mid [局面数] [深さ] [スレッド]
use kuroobi::midgame::{NnueSearch, SharedTt};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::{Board, Position};

fn rnd(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn name(p: Position) -> String {
    format!("{}{}", (b'A' + p.index() / 8) as char, p.index() % 8 + 1)
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let depth: u32 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let threads: usize = std::env::args()
        .nth(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(std::path::Path::new("weights/nnue-h16.bin"))
        .expect("nnue");
    nn.quantize();
    let nn: &'static Nnue = Box::leak(Box::new(nn));

    let mut s: u64 = 0xDEAD_BEEF_1234_5678;
    let (mut bad, mut done) = (0usize, 0usize);
    for i in 0..n {
        let mut b = Board::new();
        let plies = 12 + (rnd(&mut s) % 26) as usize;
        for _ in 0..plies {
            let m = b.movable();
            if m == 0 {
                b.pass();
                if b.movable() == 0 {
                    break;
                }
                continue;
            }
            let k = (rnd(&mut s) % m.count_ones() as u64) as u32;
            let mut mm = m;
            for _ in 0..k {
                mm &= mm - 1;
            }
            b.make_move_bits(Position::from_index(mm.trailing_zeros()).unwrap());
        }
        if b.movable() == 0 {
            continue;
        }
        done += 1;
        let solve = |th: usize| {
            let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(22)));
            let mut w = NnueSearch::new(nn, tt);
            w.threads = th;
            /* **MPC は切る。** 確率的な枝刈りは探索順序で結果が動くので、
            並列との差が「並列の欠陥」なのか「MPC の揺らぎ」なのか区別が
            つかない。切れば同じ深さの答えは一意になる。 */
            w.mpc = std::env::var("MID_MPC").is_ok_and(|v| v != "0");
            w.best_move_valued(&b, depth)
        };
        let (p1, v1) = solve(1);
        let (pn, vn) = solve(threads);
        // 手が違うのは同値の別解かもしれないので、値の差だけを数える
        // 値も手も一致すべき (MPC を切れば一意)
        if (v1 - vn).abs() > 0.001 || p1 != pn {
            bad += 1;
            println!(
                "局面 {i}: 1 スレッド {:?} {v1:+.2} 対 {threads} スレッド {:?} {vn:+.2}",
                p1.map(name),
                pn.map(name)
            );
            if bad >= 5 {
                break;
            }
        }
    }
    println!("{done} 局面中 {bad} 件が食い違い");
    if bad > 0 {
        std::process::exit(1);
    }
}
