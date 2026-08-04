//! 停止ハンドルを立ててから探索が抜けるまでの実時間。
//! 重みファイルが必要なので #[ignore] (ローカルで cargo test -- --ignored)。
//!
//! 「止めたのに CPU が解放されない」の切り分け用。中盤探索と読み切りの
//! それぞれで、止めてから返るまでを測る。

use std::time::{Duration, Instant};

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Position};

const KIFU: &str = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2\
                    a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";

fn replay_until_empties(kifu: &str, empties: u8) -> Board {
    let mut board = Board::new();
    for mv in kifu.chars().collect::<Vec<_>>().chunks(2) {
        if board.empty_count() == empties {
            break;
        }
        let pos = Position::from_file_rank(mv[0] as u8 - b'a', mv[1] as u8 - b'1').unwrap();
        if board.movable() == 0 {
            board.pass();
        }
        board.make_move_bits(pos);
    }
    board
}

/// 立ててから返るまでこの時間を超えたら、止まりが遅いとみなす。
const LIMIT: Duration = Duration::from_millis(300);

/// 中盤探索だけは**まだこの水準に届いていない**。`negamax` は 512 ノード
/// ごとに停止を見ているので理屈上は一瞬のはずだが、実測で 0.9〜1.8 秒
/// かかる (Lazy SMP がヘルパーを段ごとに join する構造が疑わしい)。
/// 原因を切り分けるまでは、悪化だけを捕まえる緩い線で置いておく。
const MIDGAME_LIMIT: Duration = Duration::from_millis(2500);

#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn midgame_search_stops_promptly() {
    let cfg = EngineConfig {
        depth: 24,
        solve_empties: 8, // 読み切りに逃げないよう浅く
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 30);
    let stop = engine.stop_handle();

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        stop.stop();
    });
    let t0 = Instant::now();
    engine.eval_position(&board, 24);
    let took = t0.elapsed();
    let after_stop = took.saturating_sub(Duration::from_secs(2));
    println!("中盤探索: 停止から {after_stop:?}");
    assert!(
        after_stop < MIDGAME_LIMIT,
        "中盤探索: 停止から {after_stop:?} かかった"
    );
}

#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn endgame_solve_stops_promptly() {
    let cfg = EngineConfig {
        depth: 12,
        solve_empties: 30, // 空き 26 を読み切りに入れる (長い)
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 26);
    let stop = engine.stop_handle();

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        stop.stop();
    });
    let t0 = Instant::now();
    engine.eval_position(&board, 12);
    let took = t0.elapsed();
    let after_stop = took.saturating_sub(Duration::from_secs(2));
    println!("読み切り: 停止から {after_stop:?}");
    assert!(
        after_stop < LIMIT,
        "読み切り: 停止から {after_stop:?} かかった"
    );
}

/// 停止を入れる前後で探索そのものが遅くなっていないかを見るための基準。
/// 止めずに同じ局面を解いて時間を出す (CLAUDE.md「測ってから決める」)。
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn solve_speed_baseline() {
    let cfg = EngineConfig {
        depth: 12,
        solve_empties: 30,
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 24);
    let t0 = Instant::now();
    let mv = engine.eval_position(&board, 12);
    println!("空き 24 の読み切り: {:?} (値 {})", t0.elapsed(), mv.value);
}

/// 停止確認の頻度 (ABORT_CHECK_INTERVAL) が探索速度に効いているかを見る
/// ための基準。固定深さで探索して nps を出す。
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn midgame_speed_baseline() {
    let cfg = EngineConfig {
        depth: 14,
        solve_empties: 8,
        threads: 1, // 並列の揺れを避けて 1 スレッドで測る
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 40);
    let n0 = engine.nodes();
    let t0 = Instant::now();
    engine.eval_position(&board, 14);
    let (el, nodes) = (t0.elapsed(), engine.nodes() - n0);
    println!(
        "中盤 深さ14: {:?} / {} ノード / {:.2}M nps",
        el,
        nodes,
        nodes as f64 / el.as_secs_f64() / 1e6
    );
}
