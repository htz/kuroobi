//! **この機械の読切速度を測って `resources.conf` へ書く。**
//!
//! 持ち時間から読切の入り口を逆算する ([`kuroobi::timectl`]) には、見積った
//! ノード数を秒へ直す係数が要る。3 層 (基準ノード数 × 並列の割増 ÷ nps) の
//! うち **nps だけが機械依存**なので、ここで実測して控える。GUI の設定から
//! も同じことができる (こちらは CLI だけで対局する人と、測り直しの確認用)。
//!
//! **効果は棋力ではなく破綻の回避に出る。** 自己対局 (計 1400 局) では勝率
//! は動かず、終局時の残り時間が増えた — 30 秒の対局で最悪の局の残りが
//! 5.0 秒 (固定の階段) から 8.9 秒へ。
//!
//! ```sh
//! calibnps                      # 今の設定のスレッド数で測って保存
//! calibnps --threads 8          # スレッド数を指定して測る
//! calibnps --show               # 保存済みの値と、そこから出る入り口を見る
//! calibnps --no-save            # 測るだけ (書かない)
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::resources::Resources;

/// 設定ファイルの場所。**GUI と同じ場所を見る** — 別々だと、GUI で測った
/// 値が CLI に効かない。
fn resources_path() -> PathBuf {
    if let Ok(d) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(d).join("kuroobi").join("resources.conf");
    }
    let home = std::env::var("HOME").unwrap_or_default();
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(home).join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let base = PathBuf::from(home).join(".config");
    base.join("kuroobi").join("resources.conf")
}

/// 既定のスレッド数 (GUI の `auto_threads` と同じ「コア数の半分」)。
fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(1)
}

/// 較正した値から、持ち時間ごとの読切の入り口を並べる。**数字 1 個より
/// 「それで何が変わるか」のほうが読める。**
///
/// 空きの上限は 30 で見せる。設定の読切 (既定 18) で切ると表が全部同じ数に
/// なり、較正で何が変わるのかが読めない。
fn report(nps: f64, threads: usize) {
    println!("読切 {:.1}M ノード/秒 ({threads} スレッド)", nps / 1e6);
    println!("  持ち時間  読切に入る空き  その 1 回の見込み");
    for secs in [3u64, 10, 30, 60, 300, 900] {
        // 画面に出すのは序盤 (空き 60) の判断。対局が進むと残り手数が減り、
        // 1 手の予算が増えるので入り口はこれより深くなる
        let p = kuroobi::timectl::plan(
            kuroobi::timectl::Situation {
                clock_secs: Some(secs),
                empties: 60,
                nps: Some(nps),
                threads,
                ..Default::default()
            },
            kuroobi::timectl::Levels {
                depth: 22,
                solve: 30,
                band: 0,
            },
            kuroobi::timectl::Pace::Fast,
        );
        let t = kuroobi::timectl::solve_secs(p.solve, nps, threads);
        println!("  {secs:>6} 秒  {:>12}  {t:>14.1} 秒", p.solve);
    }
}

fn main() -> ExitCode {
    let mut threads: Option<usize> = None;
    let mut save = true;
    let mut show = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()),
            "--no-save" => save = false,
            "--show" => show = true,
            other => {
                eprintln!("unknown option {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let path = resources_path();
    let mut res = Resources::load(&path);
    let threads = threads.or(res.threads).unwrap_or_else(auto_threads);

    if show {
        if res.nps.is_empty() {
            println!("まだ較正していない ({} に nps が無い)", path.display());
            return ExitCode::SUCCESS;
        }
        println!("{}", path.display());
        for (t, nps) in &res.nps {
            report(*nps, *t);
            println!();
        }
        if res.nps_for(threads).is_none() {
            println!("(いま設定されているのは {threads} スレッド — その数では測っていない)");
        }
        return ExitCode::SUCCESS;
    }

    let cfg = EngineConfig {
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: res.book_path(),
        threads,
        ..Default::default()
    };
    let mut engine = match Engine::new(cfg) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("エンジンを用意できない: {e}");
            return ExitCode::FAILURE;
        }
    };
    let nps = engine.measure_solve_nps();
    if nps <= 0.0 {
        eprintln!("測れなかった (較正局面が解けていない)");
        return ExitCode::FAILURE;
    }
    report(nps, threads);

    if save {
        res.set_nps(threads, nps);
        if let Err(e) = res.save(&path) {
            eprintln!("保存できない {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("{} に保存した", path.display());
    }
    ExitCode::SUCCESS
}
