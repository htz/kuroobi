//! Cross-checks the parallel solver against the sequential one.
//!
//! Race bugs are intermittent, so stream many random positions and
//! compare parallel values against single-threaded truth; combine with
//! `SOLVER_ABORT=1 SOLVER_CHAOS=n` to make aborts dense.
//!
//! Usage: stress_par [positions] [empties] [threads]
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
        // Random moves down to the target empty count.
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
        /* Check the move too: values alone miss "right value, wrong
        move" (which once sold a -20 move as +16). Play the returned
        move, solve exactly, and match the negation against the value. */
        let move_val = r.best_move.map(|p| {
            let mut c = b;
            c.make_move_bits(p);
            /* Insert a pass before solving: if the opponent cannot
            move, solving as-is returns 0 and false-positives. If neither
            can move it is game over — use the disc difference. */
            /* After a pass the mover is us again, so do not negate;
            either direction of sign error false-positives. */
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
                "position {i}: sequential {truth} vs {threads}T {got} / returned move value {:?}",
                move_val
            );
            // Print mismatches as OBF for standalone follow-up.
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
    println!("{bad} of {done} positions mismatched");
    if bad > 0 {
        std::process::exit(1);
    }
}
