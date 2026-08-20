//! 中盤の並列探索が**自己整合**しているかを見る。
//!
//! **逐次と一致することは求めない。** Lazy SMP のヘルパーは
//! `main_depth + ctz(idx+1)` と違う深さを読み、その結果を共有の表に置く。
//! 本探索がそれを使うのは設計どおりで、深さ N の逐次と同じ答えになったら
//! ヘルパーが働いていないことになる。同値の別解を選ぶのも普通に起こる。
//!
//! 見るのは「**返した値が、返した手から出た値か**」。実戦の事故はそこが
//! 壊れていた — 零幅窓で沈んだ手の上界を値として返し、真値 -20 の手を
//! 「+16」と称して指した。手を実際に打って読み直し、大きく食い違ったら
//! 並列化の欠陥を疑う。
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
        /* **返した手を打って読み直す。** 中盤は近似なので多少はずれるが、
        指し手と値が別物なら大きく開く。許容は 3 石 (`MID_TOL` で変えられる)。 */
        let tol: f32 = std::env::var("MID_TOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3.0);
        let own = pn.map(|p| {
            let mut nb = b;
            let flipped = nb.make_move_bits(p);
            let _ = flipped;
            let flip = if nb.movable() == 0 {
                nb.pass();
                false
            } else {
                true
            };
            let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(22)));
            let mut w = NnueSearch::new(nn, tt);
            w.threads = 1;
            w.mpc = false;
            let (_, v) = w.best_move_valued(&nb, depth.saturating_sub(1));
            if flip {
                -v
            } else {
                v
            }
        });
        if own.is_some_and(|o| (o - vn).abs() > tol) {
            bad += 1;
            println!(
                "局面 {i}: {threads} スレッドは {:?} {vn:+.2} と言うが、その手を読み直すと {:+.2}",
                pn.map(name),
                own.unwrap()
            );
            if bad >= 5 {
                break;
            }
            continue;
        }
        /* **食い違ったら同じ局面を繰り返す。** 競合は毎回出るとは限らないの
        で、1 回の食い違いだけでは「たまたま」なのか「必ず壊れる」のか分から
        ない。`MID_REPEAT=n` で n 回まわして出方を見る。 */
        if (v1 - vn).abs() > 0.001 || p1 != pn {
            if let Ok(r) = std::env::var("MID_REPEAT") {
                let r: usize = r.parse().unwrap_or(5);
                print!("局面 {i} を {r} 回:");
                for _ in 0..r {
                    let (p, v) = solve(threads);
                    print!(" {}{v:+.2}", p.map(name).unwrap_or_default());
                }
                println!(" / 逐次 {}{v1:+.2}", p1.map(name).unwrap_or_default());
            }
        }
        // 手が違うのは同値の別解かもしれないので、値の差だけを数える
        // 逐次との差は参考。既定では数えない (`MID_STRICT=1` で数える)
        let strict = std::env::var("MID_STRICT").is_ok_and(|v| v != "0");
        if strict && ((v1 - vn).abs() > 0.001 || p1 != pn) {
            bad += 1;
            println!(
                "局面 {i}: 1 スレッド {:?} {v1:+.2} 対 {threads} スレッド {:?} {vn:+.2}",
                p1.map(name),
                pn.map(name)
            );
            let mut obf = String::new();
            for k in 0..64 {
                let bit = 1u64 << k;
                obf.push(if b.player_bb() & bit != 0 {
                    'X'
                } else if b.opponent_bb() & bit != 0 {
                    'O'
                } else {
                    '-'
                });
            }
            println!("  obf: {obf} X  (空き {})", b.empty_count());
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
