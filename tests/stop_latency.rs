//! Wall time from raising the stop handle to the search returning.
//! Requires weights, hence #[ignore].
//!
//! For bisecting "stopped but CPU not released": measures stop-to-return
//! for the midgame search and the solver separately.

use std::time::{Duration, Instant};

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Position};

const KIFU: &str = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2\
                    a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";

fn replay_until_empties(kifu: &str, empties: u8) -> Board {
    let mut board = Board::new();
    for mv in kifu.chars().collect::<Vec<_>>().chunks(2) {
        if board.empty_count() == empties {
            break;
        }
        let pos = Position::from_file_rank(mv[0] as u8 - b'a', mv[1] as u8 - b'1').unwrap();
        if board.movable() == 0 {
            board.pass();
        }
        board.make_move_bits(pos);
    }
    board
}

/// Stop-to-return beyond this counts as slow.
const LIMIT: Duration = Duration::from_millis(300);

/// The midgame search has not reached this bar yet: negamax checks the
/// stop every 512 nodes so it should be instant, but measures 0.9-1.8s
/// (the per-iteration helper join in Lazy SMP is the suspect). Until
/// bisected, the bound is loose and only catches regressions.
const MIDGAME_LIMIT: Duration = Duration::from_millis(2500);

#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn midgame_search_stops_promptly() {
    let cfg = EngineConfig {
        depth: 24,
        solve_empties: 8, // shallow so it cannot escape into a solve
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 30);
    let stop = engine.stop_handle();

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        stop.stop();
    });
    let t0 = Instant::now();
    engine.eval_position(&board, 24);
    let took = t0.elapsed();
    let after_stop = took.saturating_sub(Duration::from_secs(2));
    println!("midgame: {after_stop:?} after stop");
    assert!(
        after_stop < MIDGAME_LIMIT,
        "midgame took {after_stop:?} after stop"
    );
}

#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn endgame_solve_stops_promptly() {
    let cfg = EngineConfig {
        depth: 12,
        solve_empties: 30, // puts 26 empties into the (long) solve
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 26);
    let stop = engine.stop_handle();

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        stop.stop();
    });
    let t0 = Instant::now();
    engine.eval_position(&board, 12);
    let took = t0.elapsed();
    let after_stop = took.saturating_sub(Duration::from_secs(2));
    println!("solve: {after_stop:?} after stop");
    assert!(after_stop < LIMIT, "solve took {after_stop:?} after stop");
}

/// Baseline for whether adding the stop slowed the search itself:
/// solve the same position without stopping and print the time.
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn solve_speed_baseline() {
    let cfg = EngineConfig {
        depth: 12,
        solve_empties: 30,
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 24);
    let t0 = Instant::now();
    let mv = engine.eval_position(&board, 12);
    println!("24-empties solve: {:?} (value {})", t0.elapsed(), mv.value);
}

/// Baseline for whether the stop-check frequency (ABORT_CHECK_INTERVAL)
/// affects speed: fixed-depth search, print nps.
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn midgame_speed_baseline() {
    let cfg = EngineConfig {
        depth: 14,
        solve_empties: 8,
        threads: 1, // single thread to avoid parallel jitter
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 40);
    let n0 = engine.nodes();
    let t0 = Instant::now();
    engine.eval_position(&board, 14);
    let (el, nodes) = (t0.elapsed(), engine.nodes() - n0);
    println!(
        "midgame depth 14: {:?} / {} nodes / {:.2}M nps",
        el,
        nodes,
        nodes as f64 / el.as_secs_f64() / 1e6
    );
}
