//! NNUE 重みを 8 対称で平均し、評価を対称不変にする。
//!
//! パターンのマスクは並び順が対称変換で変わるため、同一配置でも別インデックス
//! を引いて評価がずれる (実測 0.1-0.8 石)。軌道ごとに平均して根治する。
//!
//! 使い方: nnue_symmetrize <in.bin> <out.bin> [--val <file>]
//! --val を渡すと対称化前後の検証 MSE を測って表示する (品質確認)。

use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::trainer::load_examples_binary;

fn mse(nn: &Nnue, ex: &[kuroobi::trainer::Example]) -> f64 {
    let mut sum = 0.0f64;
    for e in ex {
        let d = nn.eval(&e.board()) as f64 - e.score as f64;
        sum += d * d;
    }
    sum / ex.len().max(1) as f64
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: nnue_symmetrize <in.bin> <out.bin> [--val <file>] [--limit <n>]");
        return std::process::ExitCode::FAILURE;
    }
    let inp = std::path::PathBuf::from(&args[0]);
    let out = std::path::PathBuf::from(&args[1]);
    let mut val: Option<std::path::PathBuf> = None;
    let mut limit = 300_000usize;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--val" => {
                i += 1;
                val = args.get(i).map(std::path::PathBuf::from);
            }
            "--limit" => {
                i += 1;
                limit = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(limit);
            }
            _ => {}
        }
        i += 1;
    }

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    if let Err(e) = nn.load(&inp) {
        eprintln!("load {}: {e}", inp.display());
        return std::process::ExitCode::FAILURE;
    }

    let examples = val.as_ref().and_then(|p| {
        let mut v = load_examples_binary(p).ok()?;
        v.truncate(limit);
        Some(v)
    });
    if let Some(ex) = &examples {
        println!("検証 {} 局面: 対称化前 MSE = {:.4}", ex.len(), mse(&nn, ex));
    }

    nn.symmetrize();

    if let Some(ex) = &examples {
        println!("                対称化後 MSE = {:.4}", mse(&nn, ex));
    }
    if let Err(e) = nn.save(&out) {
        eprintln!("save {}: {e}", out.display());
        return std::process::ExitCode::FAILURE;
    }
    println!("保存: {}", out.display());
    std::process::ExitCode::SUCCESS
}
