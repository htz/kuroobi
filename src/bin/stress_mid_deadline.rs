//! Checks the deadline-bounded midgame search never returns abort values.
//!
//! Live signature: value +0.00 (a rounded non-finite), depth beyond the
//! empty count, and "cut" — the move carrying all three was an X-square
//! blunder.
//!
//! Usage: stress_mid_deadline [positions] [threads]
use kuroobi::engine::{Engine, EngineConfig};
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
    let threads: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let mut cfg = EngineConfig {
        depth: 60,
        solve_empties: 29,
        band: 8,
        threads,
        use_book: false,
        ..Default::default()
    };
    cfg.nnue = std::path::PathBuf::from("weights/nnue-h16.bin");
    cfg.weights = std::path::PathBuf::from("weights/linear.bin");
    let mut eng = Engine::new(cfg).expect("engine");

    let mut s: u64 = 0xC0FF_EE12_3456_789A;
    let (mut bad, mut cut, mut done) = (0usize, 0usize, 0usize);
    for i in 0..n {
        // Just past a drawn opening (the live incidents' 38-46 empties).
        let target = 38 + (rnd(&mut s) % 9) as u32;
        let mut b = Board::new();
        while b.empty_count() as u32 > target {
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
        if b.empty_count() as u32 != target || b.movable() == 0 {
            continue;
        }
        done += 1;
        // Live moves run 80-150s; shorter is the same path (cut mid-iteration).
        let ms = 200 + (rnd(&mut s) % 2800);
        let mv = eng.choose_within(
            &b,
            Some(std::time::Instant::now() + std::time::Duration::from_millis(ms)),
        );
        if mv.cut {
            cut += 1;
        }
        let empties = b.empty_count() as u32;
        if !mv.value.is_finite() || mv.depth > empties {
            bad += 1;
            println!(
                "position {i}: empties {empties} deadline {ms}ms -> move {:?} value {} depth {}{}",
                mv.pos.map(|p| p.index()),
                mv.value,
                mv.depth,
                if mv.cut { " cut" } else { "" }
            );
        }
    }
    let nf = kuroobi::engine::NON_FINITE_VALUES.load(std::sync::atomic::Ordering::Relaxed);
    println!(
        "{bad} of {done} positions ({cut} cut) had broken value/depth; {nf} non-finite roundings"
    );
}
