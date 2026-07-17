//! Core-operation benchmarks: mobility, flippable, make_move, perft, solver,
//! and NPS (nodes/sec) throughput for the midgame search and endgame solver.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use kuroobi::{
    bitboard, Board, EndSolverMode, Evaluator, Position, Searcher, Solver, EGAROUCID_PATTERNS,
};

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

/// Evaluator with random-ish nonzero weights: eval cost is weight-independent
/// but nonzero weights keep move ordering realistic in the search benches.
fn bench_evaluator() -> Evaluator {
    let mut e = Evaluator::new(EGAROUCID_PATTERNS);
    let mut state = 0x9e3779b97f4a7c15u64;
    for stage in 0..kuroobi::STAGE_COUNT {
        for (pi, p) in EGAROUCID_PATTERNS.iter().enumerate() {
            for idx in 0..p.table_size() {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                // Small values in [-4, 4)
                let w = ((state >> 40) as i32 % 256) as f32 / 32.0;
                e.set_weight(stage, pi, idx, w);
            }
        }
    }
    e
}

fn bench_eval(c: &mut Criterion) {
    let e = bench_evaluator();
    let mid = position_with_empties(30, 3);
    let mut group = c.benchmark_group("eval");
    group.throughput(Throughput::Elements(1));
    group.bench_function("eval_30_empties", |b| b.iter(|| e.eval(black_box(&mid))));
    group.finish();
}

/// Midgame search NPS: nodes/sec at depths 4/6/8 from a ~30-empties position.
/// The TT is cleared before every search so node counts are deterministic;
/// throughput (Elements) = nodes of one cold-TT search -> criterion reports
/// NPS directly as Kelem/s / Melem/s.
fn bench_search_nps(c: &mut Criterion) {
    let e = bench_evaluator();
    let board = position_with_empties(30, 3);

    let mut group = c.benchmark_group("search_nps");
    for depth in [4u8, 6, 8] {
        let mut s = Searcher::new(14);
        s.clear();
        let nodes = s.search(&board, &e, depth).nodes;
        group.throughput(Throughput::Elements(nodes));
        group.bench_function(format!("depth_{depth}"), |b| {
            b.iter(|| {
                s.clear();
                s.search(black_box(&board), &e, depth)
            })
        });
    }
    group.finish();
}

/// Endgame solver NPS at 14/16/18 empties (solve() clears its TT itself).
fn bench_solver_nps(c: &mut Criterion) {
    let mut group = c.benchmark_group("solver_nps");
    group.sample_size(10);
    for empties in [14u8, 16, 18] {
        let board = position_with_empties(empties, 42);
        let mut solver = Solver::new(18);
        let nodes = solver.solve(EndSolverMode::Perfect, &board).nodes;
        group.throughput(Throughput::Elements(nodes));
        group.bench_function(format!("empties_{empties}"), |b| {
            b.iter(|| solver.solve(EndSolverMode::Perfect, black_box(&board)))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_primitives,
    bench_perft,
    bench_solver,
    bench_eval,
    bench_search_nps,
    bench_solver_nps
);
criterion_main!(benches);
