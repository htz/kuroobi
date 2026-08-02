//! 学習 (実戦の対局の取り込み) の統合確認。
//! 重みファイルが必要なので #[ignore] (ローカルで cargo test -- --ignored)。

use std::path::Path;

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::resources::Resources;
use kuroobi::{Board, Color, Position};

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

/// 決定的な同じ相手との連戦で、負けた棋譜を繰り返さないこと。
///
/// 黒 = 学習する側 (浅い)、白 = 相手役 (深く、学習しない)。毎局エンジンを
/// 作り直して置換表の持ち越しを排除するので、白は完全に決定的。学習分は
/// book_learn.txt 経由で次の局へ引き継がれる (再起動を跨ぐ実運用と同じ形)。
#[test]
#[ignore]
fn repeated_matches_diverge_after_losses() {
    let res = Resources::load(Path::new("/nonexistent"));
    let dir_a = std::env::temp_dir().join("kuroobi_learn_arena_a");
    let dir_b = std::env::temp_dir().join("kuroobi_learn_arena_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let _ = std::fs::remove_file(dir_a.join("book_learn.txt"));
    let _ = std::fs::remove_file(dir_b.join("book_learn.txt"));

    let mk = |dir: &Path, depth: u32, solve: u8| EngineConfig {
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: dir.join("book.txt"), // 存在しない (定石なし)
        depth,
        solve_empties: solve,
        band: 0,
        threads: 1,
        midgame_hash_bits: 18,
        solver_hash_bits: 18,
        ..Default::default()
    };
    let cfg_a = mk(&dir_a, 2, 8); // 学習する側 (黒)。浅くして負けやすく
    let cfg_b = mk(&dir_b, 6, 10); // 相手役 (白)。学習しない

    // 対照実験: 学習しなければ同じ棋譜を正確に繰り返す (両者決定的)。
    // この前提が崩れたら、以降の「変化」を学習の効果と言えない。
    let control1 = play_game(&cfg_a, &cfg_b);
    let control2 = play_game(&cfg_a, &cfg_b);
    assert_eq!(
        control1.0, control2.0,
        "学習しなければ同一棋譜になる (前提の確認)"
    );
    println!(
        "対照 (学習なし): 黒 {:+} の同一棋譜を反復 ({}…)",
        control1.1,
        &control1.0[..24]
    );

    // 連戦 + 毎局取り込み
    let mut games: Vec<(String, i32)> = Vec::new();
    for g in 0..8 {
        let (kifu, diff) = play_game(&cfg_a, &cfg_b);
        if let Some((prev, prev_diff)) = games.last() {
            if *prev_diff < 0 {
                assert_ne!(&kifu, prev, "{g} 局目: 負けた棋譜をそのまま繰り返した");
            }
        }
        println!("game {g}: 黒 {diff:+}  {}…", &kifu[..kifu.len().min(28)]);
        // 勝敗問わず取り込む (book_learn.txt に保存され次の局に効く)
        let mut a = Engine::new(cfg_a.clone()).unwrap();
        let mut job = a.learn_start(None, &kifu, 4).unwrap();
        while a.learn_step(&mut job, 4).unwrap().is_none() {}
        games.push((kifu, diff));
    }

    let uniq: std::collections::HashSet<&String> = games.iter().map(|(k, _)| k).collect();
    let losses = games.iter().filter(|(_, d)| *d < 0).count();
    println!(
        "{} 局中 {} 種類の棋譜 / 黒の負け {} 回",
        games.len(),
        uniq.len(),
        losses
    );
    assert!(losses > 0, "この深さ差なら黒が負ける局が出るはず");
    assert!(uniq.len() > 1, "棋譜が変化する");
    let _ = std::fs::remove_file(dir_a.join("book_learn.txt"));
}

/// 1 局打つ。エンジンは毎回作り直し (置換表の持ち越しなし)。
/// 戻り値は (棋譜, 黒視点の石差)。
fn play_game(cfg_black: &EngineConfig, cfg_white: &EngineConfig) -> (String, i32) {
    let mut black = Engine::new(cfg_black.clone()).expect("weights が必要");
    let mut white = Engine::new(cfg_white.clone()).unwrap();
    let mut board = Board::new();
    let mut kifu = String::new();
    for _ in 0..200 {
        if board.is_game_over() {
            break;
        }
        if board.movable() == 0 {
            board.pass();
            continue;
        }
        let e = if board.player() == Color::Black {
            &mut black
        } else {
            &mut white
        };
        let mv = e.choose(&board);
        let p = mv.pos.expect("合法手がある局面で手が返る");
        board.make_move(p).expect("エンジンの手は合法");
        kifu.push_str(&p.to_kifu().to_lowercase());
    }
    let diff = board.black_count() as i32 - board.white_count() as i32;
    (kifu, diff)
}

/// 手替わり (学習で実戦ラインから逸れて選んだ手) の質を深い探索で測る。
///
/// 学習の代替評価は浅い速報値なので、深く見ると悪手へ逸れている恐れが
/// ある。連戦の各分岐点について、逸れる前の手・逸れた先の手・その局面の
/// 最善を深い設定 (学習を読まない別エンジン) で採点して比較する。
/// 計測が目的なので、失敗条件は「表示した評価損が大きすぎないか」を
/// 人が見る前提の緩いものにしてある。
#[test]
#[ignore]
fn measure_deviation_quality() {
    let res = Resources::load(Path::new("/nonexistent"));
    let dir_a = std::env::temp_dir().join("kuroobi_learn_devq_a");
    let dir_b = std::env::temp_dir().join("kuroobi_learn_devq_b");
    let dir_j = std::env::temp_dir().join("kuroobi_learn_devq_judge");
    for d in [&dir_a, &dir_b, &dir_j] {
        std::fs::create_dir_all(d).unwrap();
        let _ = std::fs::remove_file(d.join("book_learn.txt"));
    }

    let mk = |dir: &Path, depth: u32, solve: u8, threads: usize| EngineConfig {
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: dir.join("book.txt"),
        depth,
        solve_empties: solve,
        band: 0,
        threads,
        midgame_hash_bits: 20,
        solver_hash_bits: 20,
        ..Default::default()
    };
    let cfg_a = mk(&dir_a, 2, 8, 1); // 学習する側 (黒)
    let cfg_b = mk(&dir_b, 6, 10, 1); // 相手役 (白)

    // 連戦 + 毎局取り込み (repeated_matches_diverge_after_losses と同じ)
    let mut games: Vec<(String, i32)> = Vec::new();
    for _ in 0..8 {
        let (kifu, diff) = play_game(&cfg_a, &cfg_b);
        let mut a = Engine::new(cfg_a.clone()).unwrap();
        let mut job = a.learn_start(None, &kifu, 4).unwrap();
        while a.learn_step(&mut job, 4).unwrap().is_none() {}
        games.push((kifu, diff));
    }

    // 審判: 深い設定。学習ファイルを読まない (判定を汚さない)
    let mut judge = Engine::new(mk(&dir_j, 16, 18, 8)).unwrap();

    println!("-- 手替わりの深掘り (深さ16 / 読切18) --");
    let mut worst: f32 = 0.0;
    let mut count = 0;
    for g in 1..games.len() {
        let (prev, _) = &games[g - 1];
        let (cur, _) = &games[g];
        // 最初に違う手 (手数) を探す
        let n = (0..prev.len().min(cur.len()) / 2)
            .find(|i| prev[i * 2..i * 2 + 2] != cur[i * 2..i * 2 + 2]);
        let Some(n) = n else { continue };
        let old_mv = &prev[n * 2..n * 2 + 2];
        let new_mv = &cur[n * 2..n * 2 + 2];
        // 分岐局面を再構築
        let (line, _) = kuroobi::learn::replay(None, cur).unwrap();
        let board = line
            .iter()
            .filter(|(_, m)| m.is_some())
            .nth(n)
            .expect("分岐手は棋譜内")
            .0;
        let side = if board.player() == Color::Black {
            "黒"
        } else {
            "白"
        };
        // 深い探索でこの局面の最善と、旧手・新手の値を測る
        let best = judge.eval_position(&board, 16);
        let mut val_of = |mv: &str| -> f32 {
            let p = Position::from_kifu(mv).unwrap();
            let mut c = board;
            c.make_move(p).unwrap();
            -judge.eval_position(&c, 15).value
        };
        let v_old = val_of(old_mv);
        let v_new = val_of(new_mv);
        let best_mv = best
            .pos
            .map(|p| p.to_kifu().to_lowercase())
            .unwrap_or_default();
        let loss_vs_old = v_old - v_new;
        let loss_vs_best = best.value - v_new;
        println!(
            "game {g}: {}手目 ({side}) {old_mv}→{new_mv}  深い評価: 旧 {v_old:+.1} / 新 {v_new:+.1} / 最善 {best_mv} {:+.1}  (旧比 {:+.1}, 最善比 {:+.1})",
            n + 1,
            best.value,
            -loss_vs_old,
            -loss_vs_best,
        );
        worst = worst.max(loss_vs_best);
        count += 1;
    }
    println!("分岐 {count} 箇所 / 最善からの最大損失 {worst:+.1} 石");
    assert!(count > 0, "分岐が観測できる");
}
