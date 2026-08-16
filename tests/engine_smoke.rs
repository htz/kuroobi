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

/// **中盤の期限が守られるか。**
///
/// **これは砦であって、再現ではない。** 実戦 (15 分の同期対局、空き 37) で
/// 期限 43.5 秒の手が 132.6 秒走ったが、同じ空き・同じ期限で 1 局面を
/// 読ませても再現しない — 本番との違いは同期対局が CPU を取り合うことで、
/// 単独なら 30 秒の段が 3 倍に伸びる。ここで見ているのは「素直な条件では
/// 期限が守られる」ことだけ。
///
/// 読切にも選択読みにも入らない空きで測る (どちらも別経路で見張り済み)。
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn the_midgame_deadline_is_honoured() {
    let cfg = EngineConfig {
        // **深さで止めない。** 持ち時間があるときの設定 (`DEPTH_BY_CLOCK`)
        // と同じにして、期限だけが探索を止める状況を作る
        depth: 60,
        solve_empties: 12,
        band: 0,
        threads: 4,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");

    // 実戦で踏んだのと同じ空き 37。読切 (12) にも選択読み (帯 0) にも入らない
    let board = replay_until_empties(KIFU, 44);
    assert_eq!(board.empty_count(), 44);

    let cap = std::time::Duration::from_secs(5);
    let t0 = std::time::Instant::now();
    let mv = engine.choose_within(&board, Some(t0 + cap));
    let took = t0.elapsed();

    assert!(mv.pos.is_some(), "手が返らない");
    // 見張りの粒度と後始末のぶんは見込む。3 倍は外側の保険が動く境目
    assert!(
        took < cap * 3,
        "期限 {:.1}s に対して {:.1}s かかった",
        cap.as_secs_f32(),
        took.as_secs_f32()
    );
}

/// **同期対局と同じ取り合いで期限が守られるか。**
///
/// 1 局面を単独で読ませる分には期限は守られる。実戦で 3〜4 倍に膨らんだのは
/// **2 面が同時に読むから**なので、そこを再現して測る。エンジンを 2 つ、
/// それぞれ 4 スレッドで同じ期限を渡して同時に走らせる。
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn the_deadline_holds_when_two_engines_share_the_machine() {
    let cap = std::time::Duration::from_secs(5);
    let handles: Vec<_> = (0..2)
        .map(|i| {
            std::thread::spawn(move || {
                let cfg = EngineConfig {
                    depth: 60,
                    solve_empties: 12,
                    band: 0,
                    threads: 4,
                    ..Default::default()
                };
                let mut engine = Engine::new(cfg).expect("engine init");
                // 2 面は同じ開局を色違いで打つので、空きは 1 つずれる
                let board = replay_until_empties(KIFU, 44 - i);
                // 1 手では足りない。**詰まりは積み上がって出る** (実戦でも
                // 序盤 3 手は期限内で、そこから 1.2 → 2.0 → 3.1 倍と伸びた)
                let mut worst = std::time::Duration::ZERO;
                let mut ok = true;
                for _ in 0..6 {
                    let t0 = std::time::Instant::now();
                    let mv = engine.choose_within(&board, Some(t0 + cap));
                    ok &= mv.pos.is_some();
                    worst = worst.max(t0.elapsed());
                }
                (ok, worst)
            })
        })
        .collect();
    for h in handles {
        let (got, took) = h.join().expect("探索スレッド");
        assert!(got, "手が返らない");
        assert!(
            took < cap * 2,
            "期限 {:.1}s に対して {:.1}s かかった (2 面同時)",
            cap.as_secs_f32(),
            took.as_secs_f32()
        );
    }
}

/// **期限で本当に切れているか。**
///
/// これまでの計測は「期限より先に読み終わる」条件だったので、切る力を
/// 測れていなかった。空きを増やして**絶対に読み終わらない**木にし、
/// 期限だけが止められる状況で測る。
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn a_search_that_cannot_finish_is_still_cut() {
    let cfg = EngineConfig {
        depth: 60,
        solve_empties: 12,
        band: 0,
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    // 空き 43。深さ 60 まで読ませたら終わらない
    let board = replay_until_empties(KIFU, 43);
    let cap = std::time::Duration::from_secs(10);
    let t0 = std::time::Instant::now();
    let mv = engine.choose_within(&board, Some(t0 + cap));
    let took = t0.elapsed();
    assert!(mv.pos.is_some(), "手が返らない");
    assert!(
        took < cap * 2,
        "期限 {:.1}s に対して {:.1}s かかった",
        cap.as_secs_f32(),
        took.as_secs_f32()
    );
}

/// 上の測り方を 1 面だけで。**取り合いが原因かどうかの対照。**
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn the_deadline_holds_for_a_single_engine() {
    let cfg = EngineConfig {
        depth: 60,
        solve_empties: 12,
        band: 0,
        threads: 4,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 44);
    let cap = std::time::Duration::from_secs(5);
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..6 {
        let t0 = std::time::Instant::now();
        let mv = engine.choose_within(&board, Some(t0 + cap));
        assert!(mv.pos.is_some());
        worst = worst.max(t0.elapsed());
    }
    assert!(
        worst < cap * 2,
        "期限 {:.1}s に対して最悪 {:.1}s (1 面)",
        cap.as_secs_f32(),
        worst.as_secs_f32()
    );
}

/// **読切も期限で切れるか。**
///
/// 中盤 (YBWC) で「分割した先に停止が届いていない」穴を踏んだので、
/// ソルバも同じかを疑って測った。**こちらは元から正しかった。**
/// 兄弟タスクは停止ハンドルを持たないが、専用の見張りスレッドが根の
/// 打ち切りフラグを立て、`aborted()` が親をたどるので全員に届く
/// (葉に近い層へ判定を足さないための意図的な設計)。
///
/// 疑って測ったこと自体を残す。**次に同じ疑いが出たときのため。**
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn the_deadline_cuts_the_endgame_solver() {
    let cfg = EngineConfig {
        depth: 60,
        // **読切に必ず入る空きにする。** ここを浅くすると中盤で終わって
        // しまい、ソルバの切れ方を測れない
        solve_empties: 26,
        band: 0,
        threads: 4,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    let board = replay_until_empties(KIFU, 26);
    assert_eq!(board.empty_count(), 26);
    let cap = std::time::Duration::from_secs(2);
    let t0 = std::time::Instant::now();
    let mv = engine.choose_within(&board, Some(t0 + cap));
    let took = t0.elapsed();
    assert!(mv.pos.is_some(), "手が返らない");
    assert!(
        took < cap * 3,
        "期限 {:.1}s に対して {:.1}s かかった",
        cap.as_secs_f32(),
        took.as_secs_f32()
    );
}

/// **期限をどれだけ使えているか。**
///
/// GGS のレート戦で、持ち時間 15 分に対し実際に使ったのは 40〜46% だった。
/// 反復深化は「次の段が期限に収まらなければ始めない」ので、収まらないと
/// 判断した時点で予算を残して返る。その取りこぼしを測る。
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn report_deadline_utilisation() {
    let cfg = EngineConfig {
        depth: 60,
        solve_empties: 26,
        band: 6,
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    println!("  空き  期限   実測   使用率");
    let mut sum = 0.0;
    let mut n = 0;
    for empties in [48u8, 44, 40, 36, 32] {
        let board = replay_until_empties(KIFU, empties);
        for cap_s in [20u64, 60] {
            let cap = std::time::Duration::from_secs(cap_s);
            let t0 = std::time::Instant::now();
            let _ = engine.choose_within(&board, Some(t0 + cap));
            let took = t0.elapsed().as_secs_f64();
            let r = took / cap_s as f64;
            sum += r;
            n += 1;
            println!("  {empties:4}  {cap_s:3}s  {took:5.1}s  {:5.0}%", r * 100.0);
        }
    }
    println!("  平均使用率 {:.0}%", sum / n as f64 * 100.0);
}

/// **時間を足すと深く読めるのか。**
///
/// 予算を伸ばす価値は「1 段でも深く届くか」で決まる。届かないなら伸ばす
/// 意味は無い (使い切ることが目的ではない)。期限を 2 倍・3 倍にして、
/// 到達深さがどう動くかを見る。
#[test]
#[ignore = "weights/ の実ファイルが必要 (git 管理外)"]
fn report_depth_per_extra_time() {
    let cfg = EngineConfig {
        depth: 60,
        solve_empties: 26,
        band: 6,
        threads: 8,
        ..Default::default()
    };
    let mut engine = Engine::new(cfg).expect("engine init");
    println!("  空き   期限   到達深さ");
    for empties in [48u8, 44, 40] {
        let board = replay_until_empties(KIFU, empties);
        let mut line = format!("  {empties:4}  ");
        for cap_s in [20u64, 40, 60] {
            let t0 = std::time::Instant::now();
            let mv = engine.choose_within(&board, Some(t0 + std::time::Duration::from_secs(cap_s)));
            line.push_str(&format!(" {cap_s:3}s→d{:<3}", mv.depth));
        }
        println!("{line}");
    }
}
