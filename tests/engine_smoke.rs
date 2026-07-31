//! engine セッション層の煙テスト。weights/ の実重みが必要なので既定では
//! 走らせない: `cargo test --release --test engine_smoke -- --ignored`

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::Board;

#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn choose_and_analyze_on_opening() {
    let mut cfg = EngineConfig::default();
    cfg.depth = 8;
    cfg.solve_empties = 12;
    cfg.threads = 2;
    let mut engine = Engine::new(cfg).expect("engine init");

    let board = Board::new();
    let mv = engine.choose(&board);
    assert!(mv.pos.is_some(), "初期局面に合法手がある");
    assert!(!mv.exact, "空き 60 は完全読み域ではない");
    assert!(mv.value.abs() < 64.0);

    let hints = engine.analyze(&board, 6);
    assert_eq!(hints.len(), 4, "初期局面の合法手は 4");
    // 対称局面なので 4 手の評価は一致するはず (探索順の揺れは許容して幅で見る)
    let vals: Vec<f32> = hints.iter().map(|(_, e)| e.value).collect();
    let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
        - vals.iter().cloned().fold(f32::MAX, f32::min);
    assert!(spread < 2.0, "対称 4 手の評価差が大きすぎる: {vals:?}");
}
