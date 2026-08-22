//! Smoke tests for the engine session layer. Requires real weights in
//! weights/: `cargo test --release --test engine_smoke -- --ignored`

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Position};

/// A real game from the GGS archive (fills the board).
const KIFU: &str = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2\
                    a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";

/// Replay the record until `empties` squares remain, inserting passes.
fn replay_until_empties(kifu: &str, empties: u8) -> Board {
    let b0: Vec<char> = kifu.chars().collect();
    let mut board = Board::new();
    for mv in b0.chunks(2) {
        if board.empty_count() == empties {
            break;
        }
        let file = mv[0] as u8 - b'a';
        let rank = mv[1] as u8 - b'1';
        let pos = Position::from_file_rank(file, rank).expect("record coordinates");
        if board.movable() == 0 {
            board.pass();
        }
        board.make_move_bits(pos);
    }
    board
}

#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn choose_and_analyze_on_opening() {
    let cfg = EngineConfig {
        depth: 8,
        solve_empties: 12,
        threads: 2,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");

    let board = Board::new();
    let mv = engine.choose(&board);
    assert!(mv.pos.is_some(), "the opening has legal moves");
    assert!(!mv.exact, "60 empties is not in the solve region");
    assert!(mv.value.abs() < 64.0);

    let hints = engine.analyze(&board, 6);
    assert_eq!(hints.len(), 4, "the opening has 4 legal moves");
    /* The opening board is symmetric, so the 4 moves must be equal.
    A loose `< 2.0` bound let asymmetric weights through (spread 0.8);
    since shipping symmetrized weights the spread is exactly 0, so check
    exact equality (search is deterministic). */
    let vals: Vec<f32> = hints.iter().map(|(_, e)| e.value).collect();
    let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
        - vals.iter().cloned().fold(f32::MAX, f32::min);
    println!("opening moves: {vals:?}  spread {spread:.4}");
    assert!(
        spread <= 1e-3,
        "symmetric opening has unequal move values: {vals:?}"
    );
}

/// The eval display's deepening enters solves on depth alone. Reading
/// the strength setting (`solve_empties`) instead once left shallow
/// settings deepening forever, stuck showing "N plies".
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn deepening_solves_when_depth_reaches_the_end() {
    // Deliberately narrow the solve to 2: if it were consulted, no
    // solve could ever happen here.
    let cfg = EngineConfig {
        depth: 8,
        solve_empties: 2,
        threads: 1,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");

    // Replay a real game to 10 empties (surer than a hand-written board).
    let board = replay_until_empties(KIFU, 10);
    assert_eq!(board.empty_count(), 10, "measuring at 10 empties");
    assert!(board.movable() != 0, "the mover has legal moves");

    let mut last_depth = 0;
    let mut exact_at = None;
    engine.analyze_deepening(&board, 1, |depth, hints, _nodes| {
        last_depth = depth;
        if exact_at.is_none() && hints.iter().all(|(_, e)| e.exact) {
            exact_at = Some(depth);
        }
        true // never stop: it exits itself once everything solves
    });

    let d = exact_at.expect("deepening must eventually solve");
    assert!(d <= 9, "a 9-empties child must solve by depth 9 (got {d})");
    assert_eq!(last_depth, d, "deepening stops once everything solves");
}

/// Midgame deadline compliance.
///
/// A bulwark, not a reproduction: a live 43.5s budget once ran 132.6s,
/// but a single position at the same empties/deadline does not
/// reproduce it — the difference was synchro CPU contention. This only
/// checks that the plain case honors the deadline. Measured at empties
/// that enter neither the solve nor the band (both watched elsewhere).
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn the_midgame_deadline_is_honoured() {
    let cfg = EngineConfig {
        // No depth cap (matching the timed-play setting), so only the
        // deadline can stop the search.
        depth: 60,
        solve_empties: 12,
        band: 0,
        threads: 4,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");

    // The same 37 empties as the live incident; enters neither the
    // solve (12) nor the band (0).
    let board = replay_until_empties(KIFU, 44);
    assert_eq!(board.empty_count(), 44);

    let cap = std::time::Duration::from_secs(5);
    let t0 = std::time::Instant::now();
    let mv = engine.choose_within(&board, Some(t0 + cap));
    let took = t0.elapsed();

    assert!(mv.pos.is_some(), "no move returned");
    // Allow for watcher granularity and teardown; 3x is where the
    // outer backup would fire.
    assert!(
        took < cap * 3,
        "took {1:.1}s against a {0:.1}s deadline",
        cap.as_secs_f32(),
        took.as_secs_f32()
    );
}

/// Deadline compliance under synchro-style contention: two engines,
/// 4 threads each, same deadline, running simultaneously — the
/// condition that inflated live moves 3-4x.
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn the_deadline_holds_when_two_engines_share_the_machine() {
    let cap = std::time::Duration::from_secs(5);
    let handles: Vec<_> = (0..2)
        .map(|i| {
            std::thread::spawn(move || {
                let cfg = EngineConfig {
                    depth: 60,
                    solve_empties: 12,
                    band: 0,
                    threads: 4,
                    ..Default::default()
                };
                let mut engine = Engine::new(cfg).expect("engine init");
                // The two boards play the same opening in opposite
                // colors, so empties differ by one.
                let board = replay_until_empties(KIFU, 44 - i);
                // One move is not enough: congestion accumulates (live
                // it grew 1.2x -> 2.0x -> 3.1x over successive moves).
                let mut worst = std::time::Duration::ZERO;
                let mut ok = true;
                for _ in 0..6 {
                    let t0 = std::time::Instant::now();
                    let mv = engine.choose_within(&board, Some(t0 + cap));
                    ok &= mv.pos.is_some();
                    worst = worst.max(t0.elapsed());
                }
                (ok, worst)
            })
        })
        .collect();
    for h in handles {
        let (got, took) = h.join().expect("search thread");
        assert!(got, "no move returned");
        assert!(
            took < cap * 2,
            "took {1:.1}s against a {0:.1}s deadline (two boards)",
            cap.as_secs_f32(),
            took.as_secs_f32()
        );
    }
}

/// Does the deadline actually cut? Earlier measurements finished before
/// the deadline and never exercised the cut; use a tree that cannot
/// finish so only the deadline can stop it.
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn a_search_that_cannot_finish_is_still_cut() {
    let cfg = EngineConfig {
        depth: 60,
        solve_empties: 12,
        band: 0,
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    // 43 empties; depth 60 would never finish.
    let board = replay_until_empties(KIFU, 43);
    let cap = std::time::Duration::from_secs(10);
    let t0 = std::time::Instant::now();
    let mv = engine.choose_within(&board, Some(t0 + cap));
    let took = t0.elapsed();
    assert!(mv.pos.is_some(), "no move returned");
    assert!(
        took < cap * 2,
        "took {1:.1}s against a {0:.1}s deadline",
        cap.as_secs_f32(),
        took.as_secs_f32()
    );
}

/// Same measurement single-board: the control for contention.
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn the_deadline_holds_for_a_single_engine() {
    let cfg = EngineConfig {
        depth: 60,
        solve_empties: 12,
        band: 0,
        threads: 4,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 44);
    let cap = std::time::Duration::from_secs(5);
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..6 {
        let t0 = std::time::Instant::now();
        let mv = engine.choose_within(&board, Some(t0 + cap));
        assert!(mv.pos.is_some());
        worst = worst.max(t0.elapsed());
    }
    assert!(
        worst < cap * 2,
        "worst {1:.1}s against a {0:.1}s deadline (single board)",
        cap.as_secs_f32(),
        worst.as_secs_f32()
    );
}

/// Do solves cut on deadline too? Suspected after the midgame YBWC
/// stop-propagation hole — the solver turned out correct all along:
/// the watcher raises the root abort flag and `aborted()` walks up the
/// parents, reaching every task. The test stays as a record that the
/// suspicion was checked.
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn the_deadline_cuts_the_endgame_solver() {
    let cfg = EngineConfig {
        depth: 60,
        // Guarantee the solve region; shallower would end in the
        // midgame and measure nothing.
        solve_empties: 26,
        band: 0,
        threads: 4,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 26);
    assert_eq!(board.empty_count(), 26);
    let cap = std::time::Duration::from_secs(2);
    let t0 = std::time::Instant::now();
    let mv = engine.choose_within(&board, Some(t0 + cap));
    let took = t0.elapsed();
    assert!(mv.pos.is_some(), "no move returned");
    assert!(
        took < cap * 3,
        "took {1:.1}s against a {0:.1}s deadline",
        cap.as_secs_f32(),
        took.as_secs_f32()
    );
}

/// Deadline utilization: rated games used only 40-46% of the clock
/// because deepening returns once the next iteration would not fit.
/// This measures that shortfall.
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn report_deadline_utilisation() {
    let cfg = EngineConfig {
        depth: 60,
        solve_empties: 26,
        band: 6,
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    println!("  empties  deadline  actual  utilization");
    let mut sum = 0.0;
    let mut n = 0;
    for empties in [48u8, 44, 40, 36, 32] {
        let board = replay_until_empties(KIFU, empties);
        for cap_s in [20u64, 60] {
            let cap = std::time::Duration::from_secs(cap_s);
            let t0 = std::time::Instant::now();
            let _ = engine.choose_within(&board, Some(t0 + cap));
            let took = t0.elapsed().as_secs_f64();
            let r = took / cap_s as f64;
            sum += r;
            n += 1;
            println!("  {empties:4}  {cap_s:3}s  {took:5.1}s  {:5.0}%", r * 100.0);
        }
    }
    println!("  mean utilization {:.0}%", sum / n as f64 * 100.0);
}

/// Does more time buy depth? Stretching the budget is only worth it if
/// it reaches at least one more ply; double and triple the deadline and
/// watch the reached depth.
#[test]
#[ignore = "requires real files in weights/ (not in git)"]
fn report_depth_per_extra_time() {
    let cfg = EngineConfig {
        depth: 60,
        solve_empties: 26,
        band: 6,
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    println!("  empties  deadline  reached depth");
    for empties in [48u8, 44, 40] {
        let board = replay_until_empties(KIFU, empties);
        let mut line = format!("  {empties:4}  ");
        for cap_s in [20u64, 40, 60] {
            let t0 = std::time::Instant::now();
            let mv = engine.choose_within(&board, Some(t0 + std::time::Duration::from_secs(cap_s)));
            line.push_str(&format!(" {cap_s:3}s→d{:<3}", mv.depth));
        }
        println!("{line}");
    }
}
