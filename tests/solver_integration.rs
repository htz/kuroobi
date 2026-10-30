//! Endgame-solver integration tests: deeper positions and self-consistency.

use kuroobi::{Board, EndSolverMode, Position, Solver};

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
        board.make_move_bits(pos);
    }
    board
}

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
        child.make_move_bits(Position::from_index(bit).unwrap());
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

    assert!(perfect.value.abs() <= 64);

    let mut child = board;
    child.make_move_bits(best);
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

/// White to move with b1, c1 and h8 empty; after h8 black cannot move and white plays on.
fn black_must_pass() -> Board {
    let rows = [
        "O--OOOOO", "XXXXXOXX", "XXOXOXXX", "XOXOXXOX", "OOOXOXOX", "OXXXXOXX", "OXXXXXXX",
        "XXXXXXX-",
    ];
    let mut b = Board::from_string(&(rows.concat() + " O")).unwrap();
    b.make_move(Position::from_kifu("h8").unwrap()).unwrap();
    b
}

#[test]
fn solver_passes_for_a_side_without_moves() {
    let board = black_must_pass();
    assert_eq!(board.movable(), 0);
    let expected = negamax(&board, false);
    assert_eq!(expected, -2);
    let mut solver = Solver::new(16);
    assert_eq!(solver.solve(EndSolverMode::Perfect, &board).value, expected);
    assert!(solver.solve(EndSolverMode::WinLossDraw, &board).value < 0);
    assert!(solver.solve(EndSolverMode::DrawLoss, &board).value < 0);
    assert!(solver.solve(EndSolverMode::WinDraw, &board).value <= 0);

    let mut over = board;
    over.pass();
    for p in ["b1", "c1"] {
        if over.check(Position::from_kifu(p).unwrap()) {
            over.make_move(Position::from_kifu(p).unwrap()).unwrap();
        }
    }
    while !over.is_game_over() {
        let m = over.movable();
        if m == 0 {
            over.pass();
            continue;
        }
        over.make_move_bits(Position::from_index(m.trailing_zeros()).unwrap());
    }
    assert_eq!(
        solver.solve(EndSolverMode::Perfect, &over).value,
        over.score()
    );
}
