//! Verifies NNUE evaluation symmetry invariance.
//! cargo test --release --test nnue_symmetry -- --ignored --nocapture

use kuroobi::bitboard;
use kuroobi::nnue::Nnue;
use kuroobi::pattern::LINEAR_PATTERNS;
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

/// The shipped weights themselves must already be symmetric.
///
/// Asymmetric weights spread the opening's equal moves (seen as -1.3/-1.9/-2.1/-2.1 before the 2026-08-10 swap).
/// Search is deterministic, so symmetric eval implies symmetric search.
#[test]
#[ignore = "requires weights/"]
fn shipped_weights_are_symmetric() {
    let path = std::path::Path::new("weights/nnue.bin");
    let mut nn = Nnue::new(LINEAR_PATTERNS);
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
