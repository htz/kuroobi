//! Node-throughput bench: linear eval vs incremental NNUE, in a search-shaped
//! traversal. A full-width negamax to a fixed depth visits an identical node
//! set for both, so the wall-time ratio is the NPS impact of swapping the leaf
//! evaluator — the number the search integration will inherit.
//!
//! Also checks that the incremental accumulator matches a from-scratch eval.

use std::path::PathBuf;
use std::time::Instant;

use kuroobi::evaluator::Evaluator;
use kuroobi::nnue::{Accumulator, Nnue};
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::pattern_index::{PatternIndexer, PatternIndices};
use kuroobi::{Board, Color, Position};

/// Full-width negamax to `depth`, linear eval at the leaves, maintaining
/// pattern indices incrementally like the real search. Returns (value, nodes).
fn walk_linear(
    b: &Board,
    e: &Evaluator,
    ixr: &PatternIndexer,
    ix: &mut PatternIndices,
    depth: u32,
    nodes: &mut u64,
) -> f32 {
    *nodes += 1;
    if depth == 0 || b.is_game_over() {
        return e.eval_indices(b, ix);
    }
    let moves = b.movable();
    if moves == 0 {
        let mut nb = *b;
        nb.pass();
        return -walk_linear(&nb, e, ixr, ix, depth, nodes);
    }
    let mut best = f32::NEG_INFINITY;
    let mut m = moves;
    while m != 0 {
        let pos = Position::from_index(m.trailing_zeros()).unwrap();
        m &= m - 1;
        let mover = b.player();
        let mut nb = *b;
        let flipped = nb.make_move_bits(pos);
        ixr.apply(ix, pos, flipped, mover);
        let v = -walk_linear(&nb, e, ixr, ix, depth - 1, nodes);
        ixr.undo(ix, pos, flipped, mover);
        if v > best {
            best = v;
        }
    }
    best
}

/// Same traversal with the incremental NNUE accumulator.
fn walk_nnue(b: &Board, nn: &Nnue, acc: &mut Accumulator, depth: u32, nodes: &mut u64) -> f32 {
    *nodes += 1;
    if depth == 0 || b.is_game_over() {
        return nn.eval_acc(acc, b);
    }
    let moves = b.movable();
    if moves == 0 {
        let mut nb = *b;
        nb.pass();
        return -walk_nnue(&nb, nn, acc, depth, nodes);
    }
    let mut best = f32::NEG_INFINITY;
    let mut m = moves;
    while m != 0 {
        let pos = Position::from_index(m.trailing_zeros()).unwrap();
        m &= m - 1;
        let mover = b.player();
        let mut nb = *b;
        let flipped = nb.make_move_bits(pos);
        nn.acc_apply(acc, pos, flipped, mover);
        let v = -walk_nnue(&nb, nn, acc, depth - 1, nodes);
        nn.acc_undo(acc, pos, flipped, mover);
        if v > best {
            best = v;
        }
    }
    best
}

fn main() {
    let mut nnue_path = PathBuf::from("nnue.bin");
    let mut depth = 8u32;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--nnue" => nnue_path = PathBuf::from(it.next().unwrap()),
            "--depth" => depth = it.next().unwrap().parse().unwrap(),
            _ => {}
        }
    }

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(&nnue_path).expect("load nnue");
    let mut lin = Evaluator::new(EGAROUCID_PATTERNS);
    lin.load_weights(std::path::Path::new("weights_full.bin")).expect("load linear");

    // A mid-game start position (a few plies in).
    let mut b = Board::new();
    for sq in [19u32, 26, 20, 21, 34, 18, 12, 5] {
        let pos = Position::from_index(sq).unwrap();
        if b.movable() & (1u64 << sq) != 0 {
            b.make_move_unchecked(pos);
        }
    }
    println!("bench from a {}-empty position, full-width depth {depth}", b.empty_count());

    // Correctness: incremental accumulator must match from-scratch eval along
    // a short move sequence (both colours).
    {
        let mut acc = nn.accumulator(&b);
        let mut tb = b;
        let mut ok = true;
        for _ in 0..6 {
            let inc = nn.eval_acc(&acc, &tb);
            let scratch = nn.eval(&tb);
            if (inc - scratch).abs() > 1e-2 {
                println!("MISMATCH: incremental {inc:.4} vs scratch {scratch:.4} (player {:?})", tb.player());
                ok = false;
            }
            let moves = tb.movable();
            if moves == 0 { break; }
            let pos = Position::from_index(moves.trailing_zeros()).unwrap();
            let mover = tb.player();
            let flipped = tb.make_move_bits(pos);
            nn.acc_apply(&mut acc, pos, flipped, mover);
        }
        println!("incremental correctness: {}", if ok { "OK" } else { "FAILED" });
    }

    // Linear throughput.
    let ixr = PatternIndexer::new(EGAROUCID_PATTERNS);
    let mut ix = ixr.init(b.black, b.white);
    let mut nodes_l = 0u64;
    let t = Instant::now();
    walk_linear(&b, &lin, &ixr, &mut ix, depth, &mut nodes_l);
    let sec_l = t.elapsed().as_secs_f64();

    // NNUE throughput.
    let mut acc = nn.accumulator(&b);
    let mut nodes_n = 0u64;
    let t = Instant::now();
    walk_nnue(&b, &nn, &mut acc, depth, &mut nodes_n);
    let sec_n = t.elapsed().as_secs_f64();

    println!("linear:  {nodes_l} nodes in {sec_l:.3}s = {:.2} Mnps", nodes_l as f64 / sec_l / 1e6);
    println!("nnue:    {nodes_n} nodes in {sec_n:.3}s = {:.2} Mnps", nodes_n as f64 / sec_n / 1e6);
    println!("slowdown: {:.2}x", sec_n / sec_l);
    let _ = Color::Black;
}
