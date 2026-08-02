//! 学習 (実戦の対局の取り込み) の統合確認。
//! 重みファイルが必要なので #[ignore] (ローカルで cargo test -- --ignored)。

use std::path::Path;

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::resources::Resources;
use kuroobi::{Board, Position};

/// GGS のアーカイブから取った実対局 (黒 +54 で終局、パスを含む)。
const KIFU: &str = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2\
                    a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";

#[test]
#[ignore]
fn absorbs_a_game_and_biases_the_choice() {
    let res = Resources::load(Path::new("/nonexistent"));
    let dir = std::env::temp_dir().join("kuroobi_learn_it");
    std::fs::create_dir_all(&dir).unwrap();
    let book_path = dir.join("book.txt"); // 存在しない (定石なしから学ぶ)
    let learn_path = dir.join("book_learn.txt");
    let _ = std::fs::remove_file(&learn_path);

    let cfg = EngineConfig {
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: book_path.clone(),
        depth: 6,
        solve_empties: 12,
        band: 0,
        threads: 1,
        ..Default::default()
    };
    let mut e = Engine::new(cfg.clone()).expect("エンジンを作れる (weights が必要)");
    assert_eq!(e.book_size(), 0, "定石なしで開始");

    // 1 探索ずつ進めて最後まで取り込む
    let mut job = e.learn_start(None, KIFU, 6).expect("取り込みを用意できる");
    let mut steps = 0;
    let out = loop {
        steps += 1;
        assert!(steps < 10_000, "取り込みが終わらない");
        if let Some(out) = e.learn_step(&mut job, 6).expect("学習できる") {
            break out;
        }
    };
    assert!(
        out.updated > 50,
        "全手の値が付け替わる (実際 {})",
        out.updated
    );
    assert!(out.added > 50, "通った局面が足される (実際 {})", out.added);
    assert!(learn_path.exists(), "学習分が保存される");
    assert_eq!(e.learned_size(), out.added, "学習分の局面数が一致する");

    // 保存した学習分は次のエンジンで定石として重なって読まれる
    let mut e2 = Engine::new(cfg).expect("エンジンを作れる");
    assert_eq!(e2.book_size(), out.added, "学習分が定石として読める");

    // 学習した局面は定石として引かれ、実戦の学習由来だと分かる
    let mut b = Board::new();
    b.make_move(Position::from_kifu("e6").unwrap()).unwrap();
    let mv = e2.choose(&b);
    assert!(mv.from_book, "学習した局面は定石として引かれる");
    assert!(mv.learned, "実戦の学習由来だと分かる");

    // この対局は黒の +54 勝ち。負けの帰結は「代替ならまだ良かった」地点
    // (敗着) に局在して書き戻される。序盤の互角の手が巻き添えで避けられる
    // ことはない代わり、同じラインをたどれば敗着で必ず逸れる:
    // 白番の実戦手のうち、最善から許容幅 (1 石) を超えて悪い値が付いた
    // 局面が存在し、そこでの選択は実戦手にならない。
    let (line, _) = kuroobi::learn::replay(None, KIFU).unwrap();
    let mut diverged = 0;
    let mut losing_recorded = 0;
    for (board, mv) in &line {
        let Some(mv) = mv else { continue };
        if board.player() != kuroobi::Color::White {
            continue; // 負けた側 (白) の手だけ見る
        }
        let choice = e2.choose(board);
        if choice.pos != Some(*mv) {
            diverged += 1;
        }
        // 負けが確定した区間では最善候補ごと大負けの値になっている
        // (帰結が book の値として見えている)
        if choice.from_book && choice.value < -10.0 {
            losing_recorded += 1;
        }
    }
    assert!(
        diverged > 0,
        "同じラインをたどっても敗着のどこかで実戦手から逸れる"
    );
    assert!(
        losing_recorded > 0,
        "負けの帰結が値として書き戻されている (diverged={diverged})"
    );
    let _ = std::fs::remove_file(&learn_path);
}
