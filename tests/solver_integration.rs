//! Endgame-solver integration tests: deeper positions and self-consistency.

use kuroobi::{Board, EndSolverMode, Position, Solver};

/// Play a deterministic pseudo-random game until `empties` squares remain.
/// A simple LCG picks among legal moves so different seeds give different
/// positions without needing the rand crate.
fn position_with_empties(empties: u8, seed: u64) -> Board {
    let mut board = Board::new();
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    while board.empty_count() > empties {
        let moves = board.movable();
        if moves == 0 {
            let mut p = board;
            p.pass();
            if p.movable() == 0 {
                break;
            }
            board = p;
            continue;
        }
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let count = moves.count_ones() as u64;
        let mut nth = (state >> 33) % count;
        let mut m = moves;
        while nth > 0 {
            m &= m - 1;
            nth -= 1;
        }
        let pos = Position::from_index(m.trailing_zeros()).unwrap();
        board.make_move_unchecked(pos);
    }
    board
}

/// Reference negamax with no pruning, no tables (slow but obviously correct).
fn negamax(board: &Board, passed: bool) -> i32 {
    let moves = board.movable();
    if moves == 0 {
        if passed {
            return board.score();
        }
        let mut b = *board;
        b.pass();
        return -negamax(&b, true);
    }
    let mut best = i32::MIN;
    let mut m = moves;
    while m != 0 {
        let bit = m.trailing_zeros();
        m &= m - 1;
        let mut child = *board;
        child.make_move_unchecked(Position::from_index(bit).unwrap());
        best = best.max(-negamax(&child, false));
    }
    best
}

#[test]
fn solver_exact_matches_bruteforce_across_seeds() {
    for seed in 1..=6u64 {
        let board = position_with_empties(8, seed);
        if board.is_game_over() {
            continue;
        }
        let expected = negamax(&board, false);
        let mut solver = Solver::new(16);
        let got = solver.solve(EndSolverMode::Perfect, &board);
        assert_eq!(
            got.value,
            expected,
            "seed {seed}: perfect solve mismatch at {} empties",
            board.empty_count()
        );
    }
}

#[test]
fn solver_deep_endgame_16_empties() {
    let board = position_with_empties(16, 42);
    assert!(!board.is_game_over());

    let mut solver = Solver::new(18);
    let perfect = solver.solve(EndSolverMode::Perfect, &board);
    assert!(perfect.nodes > 0);
    let best = perfect.best_move.expect("legal move must exist");
    assert!(board.movable() & best.to_bit() != 0);

    // Score is a reachable Othello score: |score| <= 64 and parity matches
    // the board size (both players' discs + empties sum to 64).
    assert!(perfect.value.abs() <= 64);

    // Playing the returned best move must not make the score better for the
    // opponent than promised: re-solve the child and negate.
    let mut child = board;
    child.make_move_unchecked(best);
    let reply = solver.solve(EndSolverMode::Perfect, &child);
    assert_eq!(
        -reply.value, perfect.value,
        "value must be consistent one ply deeper (PV consistency)"
    );
}

#[test]
fn solver_modes_are_consistent() {
    let board = position_with_empties(12, 7);
    let mut solver = Solver::new(16);

    let perfect = solver.solve(EndSolverMode::Perfect, &board);
    let wld = solver.solve(EndSolverMode::WinLossDraw, &board);
    assert_eq!(wld.value.signum(), perfect.value.signum(), "WLD sign");

    let wd = solver.solve(EndSolverMode::WinDraw, &board);
    if perfect.value > 0 {
        assert!(wd.value > 0, "WinDraw must report a win when winning");
    } else {
        assert!(
            wd.value <= 0,
            "WinDraw must not report a win when not winning"
        );
    }

    let dl = solver.solve(EndSolverMode::DrawLoss, &board);
    if perfect.value < 0 {
        assert!(dl.value < 0, "DrawLoss must report a loss when losing");
    } else {
        assert!(
            dl.value >= 0,
            "DrawLoss must not report a loss when not losing"
        );
    }
}
