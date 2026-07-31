//! NNUE 評価の対称不変性を検証する。
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
#[ignore = "weights/ が必要"]
fn symmetrize_makes_eval_invariant() {
    let path = std::path::Path::new("weights/nnue_champion.bin");
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(path).expect("nnue");

    // 検査局面: 初期局面から数手進めたもの (対称性が残る形)
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
        println!("局面{i}: 対称 8 通りの評価幅  {b4:.4} → {af:.4}");
        assert!(*af <= 1e-3, "対称化後は評価が一致する (幅 {af})");
    }
    assert!(before.iter().any(|&x| x > 1e-3), "対称化前は差があったはず");
}
