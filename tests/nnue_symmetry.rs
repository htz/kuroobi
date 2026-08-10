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
    // **非対称な重みでないと意味がないテスト** (最後に「対称化前は差が
    // あったはず」を確かめている)。`nnue_champion.bin` を見ていたが、その
    // 名前のファイルは存在せず、ずっと読み込みで落ちていた。出荷から
    // 外した非対称版を指す
    let path = std::path::Path::new("weights/archive/nnue-h16-20260727-asym.bin");
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

/// **いま出荷している重みが既に対称であること。**
///
/// 上のテストは「`symmetrize()` を呼べば揃う」ことの検証で、実際に読まれる
/// ファイルが対称かは見ていない。非対称な重みだと、**盤自身が対称な局面で
/// 同値のはずの手がばらつく** — 初期局面の 4 手が `-1.3 / -1.9 / -2.1 /
/// -2.1` になっていた (2026-08-10 に対称化済みへ差し替え)。
///
/// 探索は決定的なので、評価が対称なら探索値も揃う。
#[test]
#[ignore = "weights/ が必要"]
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
        println!("局面{i}: 対称 8 通りの評価幅 {spread:.6}");
        assert!(
            spread <= 1e-3,
            "出荷する重みは対称であること (局面{i} の幅 {spread})"
        );
    }
}
