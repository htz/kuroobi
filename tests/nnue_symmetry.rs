//! Verifies NNUE evaluation symmetry invariance.

use kuroobi::nnue::Nnue;
use kuroobi::pattern::LINEAR_PATTERNS;
use kuroobi::Board;

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
        let vals: Vec<f32> = bd.symmetries().iter().map(|s| nn.eval(s)).collect();
        let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
            - vals.iter().cloned().fold(f32::MAX, f32::min);
        println!("position {i}: 8-symmetry eval spread {spread:.6}");
        assert!(
            spread <= 1e-3,
            "shipped weights must be symmetric (position {i} spread {spread})"
        );
    }
}
