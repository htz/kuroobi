//! Checks that the parallel midgame search is self-consistent.
//!
//! It does NOT require matching the sequential search: Lazy SMP helpers
//! read other depths into the shared table by design, and equal-value
//! alternatives are normal. What must hold is "the returned value came
//! from the returned move" — the real-game accident broke exactly that
//! (a fail-low upper bound reported as +16 for a -20 move). Play the
//! returned move, re-search, and flag large mismatches.
//!
//! Usage: stress_mid [positions] [depth] [threads]
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
            /* MPC off: probabilistic pruning varies with visit order,
            making parallel defects indistinguishable from MPC jitter.
            Without it, same-depth answers are unique. */
            w.mpc = std::env::var("MID_MPC").is_ok_and(|v| v != "0");
            w.best_move_valued(&b, depth)
        };
        let (p1, v1) = solve(1);
        let (pn, vn) = solve(threads);
        /* Play the returned move and re-search: approximation drifts a
        little, but a value belonging to another move opens wide.
        Tolerance 3 discs (`MID_TOL`). */
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
        /* On mismatch, repeat the position: races are intermittent, and
        one hit cannot distinguish flake from certainty. `MID_REPEAT=n`. */
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
        // Different moves may be equal-value alternatives; count value
        // gaps only. Sequential deltas are informational (`MID_STRICT=1`).
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
