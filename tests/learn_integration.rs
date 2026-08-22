//! Integration checks for learning (importing played games).
//! Requires weight files, hence #[ignore] (cargo test -- --ignored).

use std::path::Path;

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::resources::Resources;
use kuroobi::{Board, Color, Position};

/// A real game from the GGS archive (Black +54, contains a pass).
const KIFU: &str = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2\
                    a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";

#[test]
#[ignore]
fn absorbs_a_game_and_biases_the_choice() {
    let res = Resources::load(Path::new("/nonexistent"));
    let dir = std::env::temp_dir().join("kuroobi_learn_it");
    std::fs::create_dir_all(&dir).unwrap();
    let book_path = dir.join("book.txt"); // nonexistent: learn from scratch
    let learn_path = dir.join("book_learn.txt");
    let _ = std::fs::remove_file(&learn_path);

    let cfg = EngineConfig {
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: book_path.clone(),
        depth: 6,
        solve_empties: 12,
        band: 0,
        threads: 1,
        ..Default::default()
    };
    let mut e = Engine::new(cfg.clone()).expect("engine builds (weights required)");
    assert_eq!(e.book_size(), 0, "starts with no book");

    // Drive the import one search at a time.
    let mut job = e.learn_start(None, KIFU, 6).expect("import prepares");
    let mut steps = 0;
    let out = loop {
        steps += 1;
        assert!(steps < 10_000, "import never finishes");
        if let Some(out) = e.learn_step(&mut job, 6).expect("learn step works") {
            break out;
        }
    };
    assert!(
        out.updated > 50,
        "every move should be re-valued (got {})",
        out.updated
    );
    assert!(
        out.added > 50,
        "visited positions get added (got {})",
        out.added
    );
    assert!(learn_path.exists(), "the overlay gets saved");
    assert_eq!(
        e.learned_size(),
        out.added,
        "overlay position count matches"
    );

    // The saved overlay loads as a book in the next engine.
    let mut e2 = Engine::new(cfg).expect("engine builds");
    assert_eq!(e2.book_size(), out.added, "overlay loads as a book");

    // Learned positions resolve as book moves and are marked learned.
    let mut b = Board::new();
    b.make_move(Position::from_kifu("e6").unwrap()).unwrap();
    let mv = e2.choose(&b);
    assert!(mv.from_book, "learned positions resolve as book moves");
    assert!(mv.learned, "marked as game-learned");

    // Black won +54. The losing outcome localizes at the losing move
    // (where an alternative was still fine); neutral opening moves are
    // not collateral, but replaying the same line must diverge there:
    // some White game move ends up worse than tolerance (1 disc) below
    // best, so selection abandons it.
    let (line, _) = kuroobi::learn::replay(None, KIFU).unwrap();
    let mut diverged = 0;
    let mut losing_recorded = 0;
    for (board, mv) in &line {
        let Some(mv) = mv else { continue };
        if board.player() != kuroobi::Color::White {
            continue; // only the losing side's (White's) moves
        }
        let choice = e2.choose(board);
        if choice.pos != Some(*mv) {
            diverged += 1;
        }
        // In the lost span even the best candidate carries the losing
        // value (the outcome is visible as book values).
        if choice.from_book && choice.value < -10.0 {
            losing_recorded += 1;
        }
    }
    assert!(
        diverged > 0,
        "replaying the line must diverge at some losing move"
    );
    assert!(
        losing_recorded > 0,
        "the losing outcome must be written back (diverged={diverged})"
    );
    let _ = std::fs::remove_file(&learn_path);
}

/// Against a deterministic opponent, lost games must not repeat.
///
/// Black learns (shallow); White is the opponent (deeper, no learning).
/// Engines are rebuilt per game so White is fully deterministic; the
/// overlay carries over via book_learn.txt, as in real use across
/// restarts.
#[test]
#[ignore]
fn repeated_matches_diverge_after_losses() {
    let res = Resources::load(Path::new("/nonexistent"));
    let dir_a = std::env::temp_dir().join("kuroobi_learn_arena_a");
    let dir_b = std::env::temp_dir().join("kuroobi_learn_arena_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let _ = std::fs::remove_file(dir_a.join("book_learn.txt"));
    let _ = std::fs::remove_file(dir_b.join("book_learn.txt"));

    let mk = |dir: &Path, depth: u32, solve: u8| EngineConfig {
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: dir.join("book.txt"), // nonexistent (no book)
        depth,
        solve_empties: solve,
        band: 0,
        threads: 1,
        midgame_hash_bits: 18,
        solver_hash_bits: 18,
        ..Default::default()
    };
    let cfg_a = mk(&dir_a, 2, 8); // learner (Black); shallow so it loses
    let cfg_b = mk(&dir_b, 6, 10); // opponent (White); no learning

    // Control: without learning the exact game repeats (both
    // deterministic); if this premise breaks, later "changes" prove
    // nothing about learning.
    let control1 = play_game(&cfg_a, &cfg_b);
    let control2 = play_game(&cfg_a, &cfg_b);
    assert_eq!(
        control1.0, control2.0,
        "without learning the game must repeat (premise check)"
    );
    println!(
        "control (no learning): repeating identical game, Black {:+} ({}...)",
        control1.1,
        &control1.0[..24]
    );

    // Series with per-game import.
    let mut games: Vec<(String, i32)> = Vec::new();
    for g in 0..8 {
        let (kifu, diff) = play_game(&cfg_a, &cfg_b);
        if let Some((prev, prev_diff)) = games.last() {
            if *prev_diff < 0 {
                assert_ne!(&kifu, prev, "game {g}: repeated a lost game verbatim");
            }
        }
        println!(
            "game {g}: Black {diff:+}  {}...",
            &kifu[..kifu.len().min(28)]
        );
        // Import win or lose (saved to book_learn.txt for the next game).
        let mut a = Engine::new(cfg_a.clone()).unwrap();
        let mut job = a.learn_start(None, &kifu, 4).unwrap();
        while a.learn_step(&mut job, 4).unwrap().is_none() {}
        games.push((kifu, diff));
    }

    let uniq: std::collections::HashSet<&String> = games.iter().map(|(k, _)| k).collect();
    let losses = games.iter().filter(|(_, d)| *d < 0).count();
    println!(
        "{} games, {} distinct records / Black lost {} times",
        games.len(),
        uniq.len(),
        losses
    );
    assert!(losses > 0, "Black should lose some games at this depth gap");
    assert!(uniq.len() > 1, "game records must vary");
    let _ = std::fs::remove_file(dir_a.join("book_learn.txt"));
}

/// Play one game with freshly built engines (no table carry-over);
/// returns (record, disc difference from Black's view).
fn play_game(cfg_black: &EngineConfig, cfg_white: &EngineConfig) -> (String, i32) {
    let mut black = Engine::new(cfg_black.clone()).expect("weights required");
    let mut white = Engine::new(cfg_white.clone()).unwrap();
    let mut board = Board::new();
    let mut kifu = String::new();
    for _ in 0..200 {
        if board.is_game_over() {
            break;
        }
        if board.movable() == 0 {
            board.pass();
            continue;
        }
        let e = if board.player() == Color::Black {
            &mut black
        } else {
            &mut white
        };
        let mv = e.choose(&board);
        let p = mv.pos.expect("a move returns when legal moves exist");
        board.make_move(p).expect("engine moves are legal");
        kifu.push_str(&p.to_kifu().to_lowercase());
    }
    let diff = board.black_count() as i32 - board.white_count() as i32;
    (kifu, diff)
}

/// Measure the quality of learned deviations with deep search.
///
/// The alternative values used during learning are shallow, so a
/// deviation might be a blunder. At each divergence point, score the
/// old move, the new move and the position's best with a deep engine
/// that does not read the overlay. This is a measurement; failure
/// conditions are lax and meant for human reading.
#[test]
#[ignore]
fn measure_deviation_quality() {
    let res = Resources::load(Path::new("/nonexistent"));
    let dir_a = std::env::temp_dir().join("kuroobi_learn_devq_a");
    let dir_b = std::env::temp_dir().join("kuroobi_learn_devq_b");
    let dir_j = std::env::temp_dir().join("kuroobi_learn_devq_judge");
    for d in [&dir_a, &dir_b, &dir_j] {
        std::fs::create_dir_all(d).unwrap();
        let _ = std::fs::remove_file(d.join("book_learn.txt"));
    }

    let mk = |dir: &Path, depth: u32, solve: u8, threads: usize| EngineConfig {
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: dir.join("book.txt"),
        depth,
        solve_empties: solve,
        band: 0,
        threads,
        midgame_hash_bits: 20,
        solver_hash_bits: 20,
        ..Default::default()
    };
    let cfg_a = mk(&dir_a, 2, 8, 1); // learner (Black)
    let cfg_b = mk(&dir_b, 6, 10, 1); // opponent (White)

    // Series with per-game import (as repeated_matches_diverge_after_losses).
    let mut games: Vec<(String, i32)> = Vec::new();
    for _ in 0..8 {
        let (kifu, diff) = play_game(&cfg_a, &cfg_b);
        let mut a = Engine::new(cfg_a.clone()).unwrap();
        let mut job = a.learn_start(None, &kifu, 4).unwrap();
        while a.learn_step(&mut job, 4).unwrap().is_none() {}
        games.push((kifu, diff));
    }

    // Judge: deep settings, never reading the overlay.
    let mut judge = Engine::new(mk(&dir_j, 16, 18, 8)).unwrap();

    println!("-- deviation deep-dive (depth 16 / solve 18) --");
    let mut worst: f32 = 0.0;
    let mut count = 0;
    for g in 1..games.len() {
        let (prev, _) = &games[g - 1];
        let (cur, _) = &games[g];
        // Find the first differing move.
        let n = (0..prev.len().min(cur.len()) / 2)
            .find(|i| prev[i * 2..i * 2 + 2] != cur[i * 2..i * 2 + 2]);
        let Some(n) = n else { continue };
        let old_mv = &prev[n * 2..n * 2 + 2];
        let new_mv = &cur[n * 2..n * 2 + 2];
        // Rebuild the divergence position.
        let (line, _) = kuroobi::learn::replay(None, cur).unwrap();
        let board = line
            .iter()
            .filter(|(_, m)| m.is_some())
            .nth(n)
            .expect("divergent move is within the record")
            .0;
        let side = if board.player() == Color::Black {
            "Black"
        } else {
            "White"
        };
        // Deep-score the position's best plus the old and new moves.
        let best = judge.eval_position(&board, 16);
        let mut val_of = |mv: &str| -> f32 {
            let p = Position::from_kifu(mv).unwrap();
            let mut c = board;
            c.make_move(p).unwrap();
            -judge.eval_position(&c, 15).value
        };
        let v_old = val_of(old_mv);
        let v_new = val_of(new_mv);
        let best_mv = best
            .pos
            .map(|p| p.to_kifu().to_lowercase())
            .unwrap_or_default();
        let loss_vs_old = v_old - v_new;
        let loss_vs_best = best.value - v_new;
        println!(
            "game {g}: move {} ({side}) {old_mv}->{new_mv}  deep: old {v_old:+.1} / new {v_new:+.1} / best {best_mv} {:+.1}  (vs old {:+.1}, vs best {:+.1})",
            n + 1,
            best.value,
            -loss_vs_old,
            -loss_vs_best,
        );
        worst = worst.max(loss_vs_best);
        count += 1;
    }
    println!("{count} divergences / worst loss vs best {worst:+.1} discs");
    assert!(count > 0, "divergences must be observable");
}
