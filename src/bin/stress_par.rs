//! 並列の読切を逐次と突き合わせる。
//!
//! **競合の不具合は走るたびに出たり出なかったりする。** 1 局面を眺めても
//! 直ったか分からないので、ランダムな局面を大量に流して 1 スレッド (正解) と
//! 並列の値を比べる。`SOLVER_ABORT=1 SOLVER_CHAOS=n` と併せて、打ち切りを
//! 濃く起こした状態で回す。
//!
//! Usage: stress_par [局面数] [空き] [スレッド]
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
        .unwrap_or(200);
    let empties: u32 = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let threads: usize = std::env::args()
        .nth(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let mut s: u64 = 0x1234_5678_9ABC_DEF0;
    let mut bad = 0usize;
    let mut done = 0usize;
    for i in 0..n {
        // ランダムに打ち進めて目当ての空きにする
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
        let truth = {
            let mut sv = Solver::new(22);
            sv.set_threads(1);
            sv.solve(EndSolverMode::Perfect, &b).value
        };
        let r = {
            let mut sv = Solver::new(22);
            sv.set_threads(threads);
            sv.solve(EndSolverMode::Perfect, &b)
        };
        let got = r.value;
        /* **手も見る。** 値だけ比べると「値は正しいが手が違う」を見逃す。
        実際それで -20 の手を +16 と称して選んでいた。返した手を実際に打って
        厳密に解き、符号を反転したものが値と一致するかを確かめる。 */
        let move_val = r.best_move.map(|p| {
            let mut c = b;
            c.make_move_bits(p);
            /* **パスを入れてから解く。** 打った直後に相手が打てない局面が
            あり、そのまま解くと 0 が返って「値と手が食い違う」と誤検出する。
            両者打てなければ終局なので石差をそのまま使う。 */
            /* パスすると手番が自分に戻るので、そのときは符号を返さない。
            反転し忘れ / 反転しすぎのどちらでも誤検出になる。 */
            let flip = if c.movable() == 0 {
                c.pass();
                false
            } else {
                true
            };
            let mut sv = Solver::new(22);
            sv.set_threads(1);
            let v = sv.solve(EndSolverMode::Perfect, &c).value;
            if flip {
                -v
            } else {
                v
            }
        });
        if truth != got || move_val.is_some_and(|v| v != truth) {
            bad += 1;
            println!(
                "局面 {i}: 逐次 {truth} 対 {threads} スレッド {got} / 返した手の値 {:?}",
                move_val
            );
            // 食い違った局面は obf で出す (単独で追えるように)
            let mut obf = String::new();
            for k in 0..64 {
                let bit = 1u64 << k;
                let (me, op) = (b.player_bb(), b.opponent_bb());
                obf.push(if me & bit != 0 {
                    'X'
                } else if op & bit != 0 {
                    'O'
                } else {
                    '-'
                });
            }
            println!("  obf: {obf} X");
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
