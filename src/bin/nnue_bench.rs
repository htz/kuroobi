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
use kuroobi::{Board, Position};

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

// i32 / f32 accumulator traversals (mirror walk_nnue for the precision bench).
macro_rules! walk_variant {
    ($name:ident, $AccTy:ty, $build:ident, $apply:ident, $undo:ident, $eval:ident) => {
        fn $name(b: &Board, nn: &Nnue, acc: &mut $AccTy, depth: u32, nodes: &mut u64) -> f32 {
            *nodes += 1;
            if depth == 0 || b.is_game_over() {
                return nn.$eval(acc, b);
            }
            let moves = b.movable();
            if moves == 0 {
                let mut nb = *b;
                nb.pass();
                return -$name(&nb, nn, acc, depth, nodes);
            }
            let mut best = f32::NEG_INFINITY;
            let mut m = moves;
            while m != 0 {
                let pos = Position::from_index(m.trailing_zeros()).unwrap();
                m &= m - 1;
                let mover = b.player();
                let mut nb = *b;
                let flipped = nb.make_move_bits(pos);
                nn.$apply(acc, pos, flipped, mover);
                let v = -$name(&nb, nn, acc, depth - 1, nodes);
                nn.$undo(acc, pos, flipped, mover);
                if v > best {
                    best = v;
                }
            }
            best
        }
    };
}
walk_variant!(
    walk_i32,
    kuroobi::nnue::Accumulator32,
    accumulator_i32,
    acc_apply_i32,
    acc_undo_i32,
    eval_acc_i32
);
walk_variant!(
    walk_f32,
    kuroobi::nnue::AccumulatorF,
    accumulator_f32,
    acc_apply_f32,
    acc_undo_f32,
    eval_acc_f32
);

/// From-scratch-at-leaf variant: the interior nodes only pay the cheap 2-byte
/// pattern-index update (exactly what the linear search already does), and the
/// H accumulator is rebuilt at the leaf. Trades per-node accumulator upkeep for
/// per-leaf accumulation — which wins depends on the leaf fraction and cache
/// behaviour, so measure rather than guess.
fn walk_scratch(
    b: &Board,
    nn: &Nnue,
    ixr: &PatternIndexer,
    ix: &mut PatternIndices,
    depth: u32,
    nodes: &mut u64,
) -> f32 {
    *nodes += 1;
    if depth == 0 || b.is_game_over() {
        return nn.eval_from_indices(ix, b);
    }
    let moves = b.movable();
    if moves == 0 {
        let mut nb = *b;
        nb.pass();
        return -walk_scratch(&nb, nn, ixr, ix, depth, nodes);
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
        let v = -walk_scratch(&nb, nn, ixr, ix, depth - 1, nodes);
        ixr.undo(ix, pos, flipped, mover);
        if v > best {
            best = v;
        }
    }
    best
}

fn main() {
    let mut nnue_path = PathBuf::from("weights/nnue_champion.bin");
    let mut depth = 8u32;
    let mut val_files: Vec<PathBuf> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--nnue" => nnue_path = PathBuf::from(it.next().unwrap()),
            "--depth" => depth = it.next().unwrap().parse().unwrap(),
            "--val" => val_files.push(PathBuf::from(it.next().unwrap())),
            _ => {}
        }
    }

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(&nnue_path).expect("load nnue");
    nn.quantize();
    nn.build_precision_variants(); // i32/f32 tables for the comparison

    // MSE comparison: f32 forward vs i16 quantized accumulator, over val.
    if !val_files.is_empty() {
        use kuroobi::trainer::load_examples_binary_into;
        let mut val = Vec::new();
        for f in &val_files {
            load_examples_binary_into(f, &mut val, None).expect("load val");
        }
        let (mut sq_f32, mut sq_i16, mut sq_i32) = (0.0f64, 0.0f64, 0.0f64);
        for ex in &val {
            let b = ex.board();
            let ef = nn.eval(&b) as f64; // f32 (from-scratch, reference)
            let ai = nn.accumulator(&b);
            let a3 = nn.accumulator_i32(&b);
            let ei = nn.eval_acc(&ai, &b) as f64; // i16
            let e3 = nn.eval_acc_i32(&a3, &b) as f64; // i32
            sq_f32 += (ex.score as f64 - ef).powi(2);
            sq_i16 += (ex.score as f64 - ei).powi(2);
            sq_i32 += (ex.score as f64 - e3).powi(2);
        }
        let n = val.len() as f64;
        println!("val MSE over {} positions:", val.len());
        println!("  f32: {:.4}", sq_f32 / n);
        println!("  i32: {:.4}", sq_i32 / n);
        println!("  i16: {:.4}", sq_i16 / n);
        return;
    }
    let mut lin = Evaluator::new(EGAROUCID_PATTERNS);
    lin.load_weights(std::path::Path::new("weights/weights_full.bin"))
        .expect("load linear");

    // A mid-game start position (a few plies in).
    let mut b = Board::new();
    for sq in [19u32, 26, 20, 21, 34, 18, 12, 5] {
        let pos = Position::from_index(sq).unwrap();
        if b.movable() & (1u64 << sq) != 0 {
            b.make_move_unchecked(pos);
        }
    }
    println!(
        "bench from a {}-empty position, full-width depth {depth}",
        b.empty_count()
    );

    // Correctness: incremental accumulator must match from-scratch eval along
    // a short move sequence (both colours).
    {
        let mut acc = nn.accumulator(&b);
        let mut tb = b;
        let mut ok = true;
        for _ in 0..6 {
            let inc = nn.eval_acc(&acc, &tb);
            let scratch = nn.eval(&tb);
            // Tolerance covers int16 quantization of the incremental path.
            if (inc - scratch).abs() > 1.0 {
                println!("MISMATCH: incremental(i16) {inc:.4} vs scratch(f32) {scratch:.4} (player {:?})", tb.player());
                ok = false;
            }
            let moves = tb.movable();
            if moves == 0 {
                break;
            }
            let pos = Position::from_index(moves.trailing_zeros()).unwrap();
            let mover = tb.player();
            let flipped = tb.make_move_bits(pos);
            nn.acc_apply(&mut acc, pos, flipped, mover);
        }
        println!(
            "incremental correctness: {}",
            if ok { "OK" } else { "FAILED" }
        );
    }

    // Linear throughput.
    let ixr = PatternIndexer::new(EGAROUCID_PATTERNS);
    let mut ix = ixr.init(b.black, b.white);
    let mut nodes_l = 0u64;
    let t = Instant::now();
    walk_linear(&b, &lin, &ixr, &mut ix, depth, &mut nodes_l);
    let sec_l = t.elapsed().as_secs_f64();

    // NNUE i16 throughput.
    let mut acc = nn.accumulator(&b);
    let mut nn_nodes = 0u64;
    let t = Instant::now();
    walk_nnue(&b, &nn, &mut acc, depth, &mut nn_nodes);
    let sec_n = t.elapsed().as_secs_f64();

    // i32 and f32 accumulator throughput (macro-generated methods).
    let sec_32 = {
        let mut a = nn.accumulator_i32(&b);
        let mut nodes = 0u64;
        let t = Instant::now();
        walk_i32(&b, &nn, &mut a, depth, &mut nodes);
        t.elapsed().as_secs_f64()
    };
    let sec_f = {
        let mut a = nn.accumulator_f32(&b);
        let mut nodes = 0u64;
        let t = Instant::now();
        walk_f32(&b, &nn, &mut a, depth, &mut nodes);
        t.elapsed().as_secs_f64()
    };

    // Leaf-rebuild variant (no per-node accumulator upkeep).
    let sec_s = {
        let mut ix2 = ixr.init(b.black, b.white);
        let mut nodes = 0u64;
        let t = Instant::now();
        walk_scratch(&b, &nn, &ixr, &mut ix2, depth, &mut nodes);
        t.elapsed().as_secs_f64()
    };

    let mnps = |s: f64| nodes_l as f64 / s / 1e6;
    println!("linear:  {:.2} Mnps  (1.00x)", mnps(sec_l));
    println!(
        "nnue leaf-rebuild: {:.2} Mnps  ({:.2}x)",
        mnps(sec_s),
        sec_s / sec_l
    );
    println!("nnue f32: {:.2} Mnps  ({:.2}x)", mnps(sec_f), sec_f / sec_l);
    println!(
        "nnue i32: {:.2} Mnps  ({:.2}x)",
        mnps(sec_32),
        sec_32 / sec_l
    );
    println!("nnue i16: {:.2} Mnps  ({:.2}x)", mnps(sec_n), sec_n / sec_l);
}
