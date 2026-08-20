//! 期限で打ち切られたときに、おかしな手を返さないかを見る。
//!
//! **GGS は毎手期限で打ち切る。** 打ち切りは実運用の主経路なのに、そこで
//! 中断値が漏れて誤った手を指していないかを確かめていなかった。ランダムな
//! 期限を与えて解き、返ってきた手が「まとも」かを厳密解と突き合わせる。
//!
//! Usage: stress_stop [局面数] [空き] [スレッド]
use kuroobi::midgame::StopHandle;
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
        .unwrap_or(60);
    let empties: u32 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let threads: usize = std::env::args()
        .nth(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let mut s: u64 = 0x5EED_1234_ABCD_0001;
    let (mut bad, mut done, mut cut) = (0usize, 0usize, 0usize);
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
        // 期限なしの厳密解 (正解)
        let truth = {
            let mut sv = Solver::new(22);
            sv.set_threads(1);
            sv.solve(EndSolverMode::Perfect, &b).value
        };
        /* **途中で止める。** 解き終わる前に止めたいので、別スレッドから
        ばらついた時刻に落とす。止まったあとの答えを使うかどうかは呼ぶ側の
        判断だが、**使うなら「その手を指して大丈夫か」が要る**。 */
        let stop = StopHandle::new();
        let ms = 1 + (rnd(&mut s) % 40);
        let st = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            st.stop();
        });
        let mut sv = Solver::new(22);
        sv.set_threads(threads);
        sv.set_stop(Some(stop.clone()));
        let r = sv.solve(EndSolverMode::Perfect, &b);
        if !stop.is_stopped() {
            continue; // 止まる前に解けた
        }
        cut += 1;
        // 打ち切られたときに手を返すなら、その手の厳密値を見る
        if let Some(p) = r.best_move {
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
            let mv = if flip { -v } else { v };
            // 打ち切りなので最善とは限らない。大きく外していないかだけ見る
            let tol: i32 = std::env::var("STOP_TOL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8);
            if truth - mv > tol {
                bad += 1;
                println!(
                    "局面 {i}: 厳密 {truth} だが返した手は {mv} ({}ms で打ち切り)",
                    ms
                );
            }
        }
    }
    println!("{done} 局面 (うち打ち切り {cut}) 中 {bad} 件がひどい手");
    if bad > 0 {
        std::process::exit(1);
    }
}
