//! Verifies NNUE evaluation symmetry invariance.
//! cargo test --release --test nnue_symmetry -- --ignored --nocapture

use kuroobi::bitboard;
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::Board;

fn sym_board(b: &Board, i: u8) -> Board {
    let mut black = b.black;
    let mut white = b.white;
    if i >= 4 {
        black = bitboard::mirror_horizontal(black);
        white = bitboard::mirror_horizontal(white);
        for _ in 0..(i - 4) {
            black = bitboard::rotate_90(black);
            white = bitboard::rotate_90(white);
        }
    } else {
        for _ in 0..i {
            black = bitboard::rotate_90(black);
            white = bitboard::rotate_90(white);
        }
    }
    let mut out = *b;
    out.black = black;
    out.white = white;
    out
}

#[test]
#[ignore = "requires weights/"]
fn symmetrize_makes_eval_invariant() {
    // This test needs asymmetric weights (it asserts a pre-symmetrize
    // spread existed). It used to point at a nonexistent file and fail
    // at load; point it at the retired asymmetric weights.
    let path = std::path::Path::new("weights/archive/nnue-h16-20260727-asym.bin");
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(path).expect("nnue");

    // Probe positions: a few moves into the opening (symmetry remains).
    let mut boards = vec![Board::new()];
    let mut b = Board::new();
    for mv in ["f5", "d6", "c3", "d3"] {
        let p = kuroobi::Position::from_kifu(mv).unwrap();
        b.make_move_bits(p);
        boards.push(b);
    }

    let spread = |nn: &Nnue, b: &Board| -> f32 {
        let vals: Vec<f32> = (0..8).map(|i| nn.eval(&sym_board(b, i))).collect();
        vals.iter().cloned().fold(f32::MIN, f32::max)
            - vals.iter().cloned().fold(f32::MAX, f32::min)
    };

    let before: Vec<f32> = boards.iter().map(|b| spread(&nn, b)).collect();
    nn.symmetrize();
    let after: Vec<f32> = boards.iter().map(|b| spread(&nn, b)).collect();

    for (i, (b4, af)) in before.iter().zip(&after).enumerate() {
        println!("position {i}: 8-symmetry eval spread  {b4:.4} -> {af:.4}");
        assert!(
            *af <= 1e-3,
            "post-symmetrize evals must agree (spread {af})"
        );
    }
    assert!(
        before.iter().any(|&x| x > 1e-3),
        "a pre-symmetrize spread should exist"
    );
}

/// The shipped weights themselves must already be symmetric.
///
/// The test above only proves `symmetrize()` works, not that the file
/// actually loaded is symmetric; asymmetric weights spread the opening's
/// equal moves (seen as -1.3/-1.9/-2.1/-2.1 before the 2026-08-10 swap).
/// Search is deterministic, so symmetric eval implies symmetric search.
#[test]
#[ignore = "requires weights/"]
fn shipped_weights_are_symmetric() {
    let path = std::path::Path::new("weights/nnue-h16.bin");
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(path).expect("nnue");
    nn.quantize();

    let mut boards = vec![Board::new()];
    let mut b = Board::new();
    for mv in ["f5", "d6", "c3", "d3", "c4"] {
        let p = kuroobi::Position::from_kifu(mv).unwrap();
        b.make_move_bits(p);
        boards.push(b);
    }

    for (i, bd) in boards.iter().enumerate() {
        let vals: Vec<f32> = (0..8).map(|k| nn.eval(&sym_board(bd, k))).collect();
        let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
            - vals.iter().cloned().fold(f32::MAX, f32::min);
        println!("position {i}: 8-symmetry eval spread {spread:.6}");
        assert!(
            spread <= 1e-3,
            "shipped weights must be symmetric (position {i} spread {spread})"
        );
    }
}
