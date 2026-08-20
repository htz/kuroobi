//! **実戦の経路**で自己整合を見る。
//!
//! `stress_par` は `Solver` を直に叩くので、ウォームアップ・評価順・定石を
//! 通らない。実戦は `Engine` 経由なので、そちらでも「返した値が返した手から
//! 出た値か」を確かめる。読切に入る空きで走らせる。
//!
//! Usage: stress_engine [局面数] [空き] [スレッド]
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::{Board, Position};

fn rnd(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let empties: u32 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let threads: usize = std::env::args()
        .nth(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let mut e = Engine::new(EngineConfig {
        depth: 60,
        solve_empties: empties as u8,
        band: 0,
        threads,
        use_book: false,
        ..Default::default()
    })
    .expect("engine");

    let mut s: u64 = 0xABCD_0F0F_2222_3333;
    let (mut bad, mut done) = (0usize, 0usize);
    for i in 0..n {
        let mut b = Board::new();
        while b.empty_count() as u32 > empties {
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
        if b.empty_count() as u32 != empties || b.movable() == 0 {
            continue;
        }
        done += 1;
        e.clear_tables();
        let mv = e.choose(&b);
        let Some(p) = mv.pos else { continue };
        // 厳密解と、返した手の厳密値
        let truth = {
            let mut sv = Solver::new(22);
            sv.set_threads(1);
            sv.solve(EndSolverMode::Perfect, &b).value
        };
        let mut c = b;
        c.make_move_bits(p);
        let flip = if c.movable() == 0 {
            c.pass();
            false
        } else {
            true
        };
        let mut s2 = Solver::new(22);
        s2.set_threads(1);
        let v = s2.solve(EndSolverMode::Perfect, &c).value;
        let mine = if flip { -v } else { v };
        if mine != truth {
            bad += 1;
            println!(
                "局面 {i}: 厳密 {truth} だが選んだ手は {mine} (エンジンの値 {:+.1}, 厳密 {})",
                mv.value, mv.exact
            );
            if bad >= 5 {
                break;
            }
        }
    }
    println!("{done} 局面中 {bad} 件が最善を外した");
    if bad > 0 {
        std::process::exit(1);
    }
}
