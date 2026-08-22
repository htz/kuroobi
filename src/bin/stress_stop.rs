//! Checks that deadline cuts never return absurd moves.
//!
//! GGS cuts every move on a deadline — the production main path — yet
//! leaked abort values there were never checked. Solve with randomized
//! deadlines and sanity-check returned moves against exact solutions.
//!
//! Usage: stress_stop [positions] [empties] [threads]
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
        // Unbounded exact solve (the truth).
        let truth = {
            let mut sv = Solver::new(22);
            sv.set_threads(1);
            sv.solve(EndSolverMode::Perfect, &b).value
        };
        /* Stop mid-solve from another thread at jittered times. Whether
        to use the post-stop answer is the caller's call — but if used,
        the move must be safe to play. */
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
            continue; // solved before the stop
        }
        cut += 1;
        // If a cut still returns a move, check its exact value.
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
            // A cut move need not be best; only flag large misses.
            let tol: i32 = std::env::var("STOP_TOL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8);
            if truth - mv > tol {
                bad += 1;
                println!(
                    "position {i}: exact {truth} but returned {mv} (cut at {}ms)",
                    ms
                );
            }
        }
    }
    println!("{bad} of {done} positions ({cut} cut) returned terrible moves");
    if bad > 0 {
        std::process::exit(1);
    }
}
