//! Core-operation benchmarks: mobility, flippable, make_move, perft, solver.

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use kuroobi::{bitboard, Board, EndSolverMode, Position, Solver};

/// Deterministic mid-game position (see tests/solver_integration.rs).
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
        board.make_move_unchecked(Position::from_index(m.trailing_zeros()).unwrap());
    }
    board
}

fn perft(board: &Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = board.movable();
    if moves == 0 {
        let mut passed = *board;
        passed.pass();
        if passed.movable() == 0 {
            return 1;
        }
        return perft(&passed, depth);
    }
    let mut nodes = 0;
    let mut m = moves;
    while m != 0 {
        let bit = m.trailing_zeros();
        m &= m - 1;
        let mut child = *board;
        child.make_move_unchecked(Position::from_index(bit).unwrap());
        nodes += perft(&child, depth - 1);
    }
    nodes
}

fn bench_primitives(c: &mut Criterion) {
    let mid = position_with_empties(30, 3);

    c.bench_function("mobility", |b| {
        b.iter(|| {
            bitboard::mobility(
                black_box(mid.player_bb()),
                black_box(mid.opponent_bb()),
                black_box(mid.empty()),
            )
        })
    });

    let first_move = Position::from_index(mid.movable().trailing_zeros()).unwrap();
    c.bench_function("flippable", |b| {
        b.iter(|| {
            bitboard::flippable(
                black_box(mid.player_bb()),
                black_box(mid.opponent_bb()),
                black_box(first_move.to_bit()),
            )
        })
    });

    c.bench_function("make_move", |b| {
        b.iter(|| {
            let mut board = black_box(mid);
            board.make_move_bits(black_box(first_move))
        })
    });

    c.bench_function("rotate_90", |b| {
        b.iter(|| bitboard::rotate_90(black_box(mid.black)))
    });
}

fn bench_perft(c: &mut Criterion) {
    let board = Board::new();
    c.bench_function("perft_5", |b| b.iter(|| perft(black_box(&board), 5)));
}

fn bench_solver(c: &mut Criterion) {
    let board = position_with_empties(14, 42);
    c.bench_function("solve_perfect_14_empties", |b| {
        let mut solver = Solver::new(18);
        b.iter(|| solver.solve(EndSolverMode::Perfect, black_box(&board)))
    });
}

criterion_group!(benches, bench_primitives, bench_perft, bench_solver);
criterion_main!(benches);
