//! engine セッション層の煙テスト。weights/ の実重みが必要なので既定では
//! 走らせない: `cargo test --release --test engine_smoke -- --ignored`

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Position};

/// GGS のアーカイブから取った実対局 (最後まで埋まる)。
const KIFU: &str = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2\
                    a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";

/// 棋譜を空きが `empties` になるまで並べる。パスは自分で入れる。
fn replay_until_empties(kifu: &str, empties: u8) -> Board {
    let b0: Vec<char> = kifu.chars().collect();
    let mut board = Board::new();
    for mv in b0.chunks(2) {
        if board.empty_count() == empties {
            break;
        }
        let file = mv[0] as u8 - b'a';
        let rank = mv[1] as u8 - b'1';
        let pos = Position::from_file_rank(file, rank).expect("棋譜の座標");
        if board.movable() == 0 {
            board.pass();
        }
        board.make_move_bits(pos);
    }
    board
}

#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn choose_and_analyze_on_opening() {
    let cfg = EngineConfig {
        depth: 8,
        solve_empties: 12,
        threads: 2,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");

    let board = Board::new();
    let mv = engine.choose(&board);
    assert!(mv.pos.is_some(), "初期局面に合法手がある");
    assert!(!mv.exact, "空き 60 は完全読み域ではない");
    assert!(mv.value.abs() < 64.0);

    let hints = engine.analyze(&board, 6);
    assert_eq!(hints.len(), 4, "初期局面の合法手は 4");
    /* 初期局面は盤自身が対称なので、**4 手は同値でなければならない**。
    基準は `< 2.0` と緩かったが、それでは非対称な重み (実際に
    `-1.3 / -1.9 / -2.1 / -2.1` = 幅 0.8 だった) を通してしまう。
    出荷する重みを対称化済みに替えた 2026-08-10 から幅は 0 なので、
    **完全一致で見る** (探索は決定的なので揺れない)。 */
    let vals: Vec<f32> = hints.iter().map(|(_, e)| e.value).collect();
    let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
        - vals.iter().cloned().fold(f32::MAX, f32::min);
    println!("初期局面の 4 手: {vals:?}  幅 {spread:.4}");
    assert!(
        spread <= 1e-3,
        "対称な初期局面で 4 手の値がばらついている: {vals:?}"
    );
}

/// 画面の評価値表示 (反復深化) が読み切りに入る条件は**深さだけ**で決まる。
/// 強さの設定 (`solve_empties`) を見てしまうと、設定が浅いときに深さだけが
/// 際限なく上がって永久に「N 手」のままになる (実際にそうなっていた)。
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn deepening_solves_when_depth_reaches_the_end() {
    // 読切をわざと 2 に絞る。ここを見ているなら読み切りには入れない
    let cfg = EngineConfig {
        depth: 8,
        solve_empties: 2,
        threads: 1,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");

    // 実対局を空き 10 まで並べ直す (手で書いた盤面より確実)
    let board = replay_until_empties(KIFU, 10);
    assert_eq!(board.empty_count(), 10, "空き 10 の局面で測る");
    assert!(board.movable() != 0, "手番に合法手がある");

    let mut last_depth = 0;
    let mut exact_at = None;
    engine.analyze_deepening(&board, 1, |depth, hints, _nodes| {
        last_depth = depth;
        if exact_at.is_none() && hints.iter().all(|(_, e)| e.exact) {
            exact_at = Some(depth);
        }
        true // 止めない — 全部が読み切りになれば自分で抜ける
    });

    let d = exact_at.expect("深化を続ければいつかは読み切りに入る");
    assert!(
        d <= 9,
        "空き 9 の子なら深さ 9 までに読み切れる (実際は {d})"
    );
    assert_eq!(last_depth, d, "全部読み切ったらそこで深化が止まる");
}
