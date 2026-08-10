//! **同じ局面を別の向きから見たら同じ値になるか。**
//!
//! 盤の 8 対称 (回転 4 + 鏡像 4) と白黒逆、その組み合わせの 16 通りで、
//! 評価値と探索値が一致することを確かめる。
//!
//! 局面は**棋譜を対称変換して並べ直して**作る。盤を直接いじるのではなく
//! 画面と同じ入口 (棋譜) を通すので、**座標変換の取り違え**もここで出る。
//!
//! `cargo test --release --test symmetry_eval -- --ignored --nocapture`

use kuroobi::bitboard;
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::{Board, Color, Position};

/// GGS のアーカイブから取った実対局。
const KIFU: &str = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2\
                    a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";

/// 8 対称の名前 (どの向きで落ちたか報告に出すため)。
const SYM_NAMES: [&str; 8] = [
    "そのまま",
    "回転 90",
    "回転 180 (点対称)",
    "回転 270",
    "左右反転",
    "左右反転 + 90",
    "左右反転 + 180",
    "左右反転 + 270",
];

/// 1 マスを `i` 番目の対称へ写す。**盤に使うのと同じ変換**を 1 ビットに
/// 通すので、棋譜と盤面がずれない。
fn sym_pos(p: Position, i: usize) -> Position {
    let bit = bitboard::symmetries(p.to_bit())[i];
    Position::from_index(bit.trailing_zeros()).expect("1 ビットは必ず盤内")
}

/// 棋譜を `i` 番目の対称へ写す。
fn sym_kifu(kifu: &str, i: usize) -> String {
    kifu.as_bytes()
        .chunks(2)
        .map(|mv| {
            let s = std::str::from_utf8(mv).expect("ascii");
            sym_pos(Position::from_kifu(s).expect("kifu"), i).to_kifu()
        })
        .collect()
}

/// 棋譜を `plies` 手だけ並べる。パスは自分で入れる。
fn replay(kifu: &str, plies: usize) -> Board {
    let mut board = Board::new();
    for (n, mv) in kifu.as_bytes().chunks(2).enumerate() {
        if n == plies {
            break;
        }
        let s = std::str::from_utf8(mv).expect("ascii");
        let p = Position::from_kifu(s).expect("kifu");
        if board.movable() & p.to_bit() == 0 {
            board.pass();
        }
        board.make_move_bits(p);
    }
    board
}

/// **白黒逆。** 石の色と手番を同時に入れ替える。
///
/// ゲームとしては同じ局面 (「手番側から見た値」は変わらない)。内部表現が
/// player / opponent 相対なら自明に一致するが、**そうなっている保証を
/// テストで持つ**。
fn flip_colors(b: &Board) -> Board {
    let mut out = *b;
    std::mem::swap(&mut out.black, &mut out.white);
    out.player = match b.player {
        Color::Black => Color::White,
        Color::White => Color::Black,
    };
    out
}

/// `i` 番目の対称で**初期配置の白黒が入れ替わる**か。
///
/// **オセロの初期配置は 90 度回転で色が入れ替わる** (d4 白・e4 黒・d5 黒・
/// e5 白 → d4 黒・e4 白…)。180 度回転では保存される。したがって棋譜を
/// 90 度回して並べ直すと、**盤を 90 度回したものの白黒逆**が出てくる。
/// バグではなく初期配置の性質。
fn rotation_swaps_colors(i: usize) -> bool {
    let init = Board::new();
    bitboard::symmetries(init.black)[i] != init.black
}

/// **棋譜を変換してから並べたものと、並べてから盤を変換したものが一致するか。**
///
/// 座標変換 (`sym_pos`) と盤の変換 (`bitboard::symmetries`) が同じ向きで
/// ないと、以降の評価の比較そのものが無意味になる。先にここを固める。
#[test]
fn kifu_and_board_symmetries_agree() {
    for plies in [4, 12, 30, 44] {
        let base = replay(KIFU, plies);
        for i in 0..8 {
            let from_kifu = replay(&sym_kifu(KIFU, i), plies);
            let black = bitboard::symmetries(base.black)[i];
            let white = bitboard::symmetries(base.white)[i];
            // 初期配置の色が入れ替わる向きでは、並べた結果も入れ替わる
            let want = if rotation_swaps_colors(i) {
                (white, black)
            } else {
                (black, white)
            };
            assert_eq!(
                (from_kifu.black, from_kifu.white),
                want,
                "{plies} 手目・{}: 棋譜を変換した結果と盤を変換した結果が違う",
                SYM_NAMES[i]
            );
        }
    }
}

/// 16 通り (8 対称 × 白黒逆) を作る。
fn all_views(kifu: &str, plies: usize) -> Vec<(String, Board)> {
    let mut out = Vec::new();
    for i in 0..8 {
        let b = replay(&sym_kifu(kifu, i), plies);
        out.push((SYM_NAMES[i].to_string(), b));
        out.push((format!("{} + 白黒逆", SYM_NAMES[i]), flip_colors(&b)));
    }
    out
}

/// 評価関数そのものが 16 通りで同じ値を返すか。
#[test]
#[ignore = "weights/ が必要 (git 管理外)"]
fn eval_is_the_same_from_every_view() {
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(std::path::Path::new("weights/nnue-h16.bin"))
        .expect("nnue");
    nn.quantize();

    for plies in [0, 4, 12, 30, 44] {
        let views = all_views(KIFU, plies);
        let vals: Vec<f32> = views.iter().map(|(_, b)| nn.eval(b)).collect();
        let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
            - vals.iter().cloned().fold(f32::MAX, f32::min);
        println!(
            "{plies:>2} 手目: 16 通りの評価幅 {spread:.6}  (例 {:.4})",
            vals[0]
        );
        if spread > 1e-3 {
            for ((name, _), v) in views.iter().zip(&vals) {
                println!("    {name:<24} {v:.6}");
            }
        }
        assert!(
            spread <= 1e-3,
            "{plies} 手目で評価がばらついている (幅 {spread})"
        );
    }
}

/// **画面に出る値** (探索を通した値) が 16 通りで同じか。
///
/// 評価が対称でも、探索の並べ替えや枝刈りが向きに依存していれば値は動く。
/// 実際に画面へ出るのはこちらなので、別に確かめる。
#[test]
#[ignore = "weights/ が必要 (git 管理外)"]
fn search_value_is_the_same_from_every_view() {
    let mut engine = Engine::new(EngineConfig {
        depth: 8,
        solve_empties: 0, // 読み切りに入ると自明に一致するので中盤だけで見る
        threads: 1,       // 並列だと走査順が揺れる。向きの影響だけを見たい
        ..Default::default()
    })
    .expect("engine init");
    engine.set_use_book(false);

    for plies in [0, 4, 12, 30] {
        let views = all_views(KIFU, plies);
        let mut vals = Vec::new();
        for (_, b) in &views {
            // 表を持ち越すと「前の向きで書いた値」を拾って一致してしまう
            engine.clear_tables();
            vals.push(engine.eval_position(b, 8).value);
        }
        let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
            - vals.iter().cloned().fold(f32::MAX, f32::min);
        println!(
            "{plies:>2} 手目: 16 通りの探索値の幅 {spread:.6}  (例 {:.4})",
            vals[0]
        );
        if spread > 1e-3 {
            for ((name, _), v) in views.iter().zip(&vals) {
                println!("    {name:<24} {v:.6}");
            }
        }
        assert!(
            spread <= 1e-3,
            "{plies} 手目で探索値がばらついている (幅 {spread})"
        );
    }
}
