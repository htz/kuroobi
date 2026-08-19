//! GTP server around [`kuroobi::engine::Engine`], so this build can be driven
//! as an external opponent by `roundrobin` (or any GTP driver).
//!
//! **なぜ要るか。** NNUE の accumulator 幅 `H` は**コンパイル時定数**なので、
//! H の違う 2 つのモデルを 1 プロセスに同居させられない。`nnue_arena` は
//! 1 プロセスで A と B を持つ作りなので、**H を変えた実験の採否を直接対戦で
//! 決められなかった**。共通の相手 (線形評価) への勝率で間接比較する手も
//! あるが、回りくどいうえに時間制で測れない。
//!
//! 別プロセスにすれば H でも特徴集合でも探索でも、何を変えたビルド同士でも
//! 戦わせられる。`roundrobin` は既に GTP を話す口を持っているので、
//! **こちらが GTP を話せばそれで足りる**。
//!
//! ```sh
//! cargo build --release --bin gtp                      # H=16 側
//! (cd ../wt-h64 && cargo build --release --bin gtp)    # H=64 側
//!
//! ./target/release/roundrobin --games 400 --time-ms 300 \
//!   --engine h16=kuroobi=./target/release/gtp \
//!   --engine h64=kuroobi=../wt-h64/target/release/gtp
//! ```
//!
//! **速度差を含めて測るなら `--time-ms` を使う。** 深さ固定は速度を無視する
//! ので、遅くて賢いモデルを過大評価する (H=64 は H=16 より 8% 遅い)。
//!
//! Egaroucid と同じ綴りのフラグ (`-gtp -l <深さ> -t <スレッド> -nobook -q`)
//! を受けるので、駆動側から見た扱いは既存の外部エンジンと変わらない。
//!
//! Usage:
//!   gtp [-gtp] [-l <depth>] [-t <threads>] [-nobook] [-q]
//!       [--solve-empties <n>] [--time-ms <n>] [--band <n>] [--no-mpc]
//!       [--weights <path>] [--nnue <path>] [--book <path>]

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Color, Position};

/// `a1`..`h8` を Position へ。`Position::index()` は file*8 + rank。
fn parse_vertex(s: &str) -> Option<Position> {
    let b = s.as_bytes();
    if b.len() < 2 {
        return None;
    }
    let file = b[0].to_ascii_lowercase().wrapping_sub(b'a');
    let rank = b[1].wrapping_sub(b'1');
    if file >= 8 || rank >= 8 {
        return None;
    }
    Position::from_file_rank(file, rank)
}

fn vertex_name(p: Position) -> String {
    let f = (b'a' + p.index() / 8) as char;
    let r = (b'1' + p.index() % 8) as char;
    format!("{f}{r}")
}

fn parse_color(s: &str) -> Option<Color> {
    match s.to_ascii_lowercase().as_str() {
        "b" | "black" => Some(Color::Black),
        "w" | "white" => Some(Color::White),
        _ => None,
    }
}

/// GTP は `= 本文` の後に**空行**を置いて 1 応答の終わりとする。
fn ok(id: &str, body: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "={id} {body}\n");
    let _ = out.flush();
}

fn err(id: &str, body: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "?{id} {body}\n");
    let _ = out.flush();
}

fn main() -> ExitCode {
    let mut cfg = EngineConfig {
        // 計測用なので既定は定石なし。定石が出ると評価の比較にならない。
        use_book: false,
        ..Default::default()
    };
    let mut time_ms: u64 = 0;
    let mut solve_set = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            // Egaroucid 綴り。駆動側を書き換えずに済ませるため受ける。
            "-gtp" | "-q" | "--quiet" => {}
            "-nobook" | "--no-book" => cfg.use_book = false,
            "-l" | "--level" | "--depth" => {
                cfg.depth = match it.next().and_then(|v| v.parse().ok()) {
                    Some(v) => v,
                    None => {
                        eprintln!("-l wants a depth");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "-t" | "--threads" => cfg.threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(1),
            "--solve-empties" => {
                cfg.solve_empties = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                solve_set = true;
            }
            "--band" => cfg.band = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--no-mpc" => cfg.mpc = false,
            "--time-ms" => time_ms = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--weights" => cfg.weights = PathBuf::from(it.next().unwrap_or_default()),
            "--nnue" => cfg.nnue = PathBuf::from(it.next().unwrap_or_default()),
            "--book" => {
                cfg.book = PathBuf::from(it.next().unwrap_or_default());
                cfg.use_book = true;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    /* 深さ N の全幅探索は空き N 以下をどのみち終局まで読む。読み切りの
    入り口を深さに合わせておかないと、深さだけ揃えたつもりで終盤の深さが
    engine ごとにずれる (Edax の level 表で踏んだのと同じ罠)。 */
    if !solve_set {
        cfg.solve_empties = cfg.depth.min(u8::MAX as u32) as u8;
    }

    let mut engine = match Engine::new(cfg) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut board = Board::new();
    /* **NPS を測るための口。** 探索の外 (起動・棋譜の再生・入出力) を数から
    外したいので、`genmove` の区間だけを足し込む。 */
    let mut tot_nodes = 0u64;
    let mut tot_secs = 0f64;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let first = parts.next().unwrap_or("");
        // GTP は先頭に任意の数値 id を置ける。付いていれば応答に写す。
        let (id, cmd) = match first.parse::<u64>() {
            Ok(_) => (first.to_string(), parts.next().unwrap_or("").to_string()),
            Err(_) => (String::new(), first.to_string()),
        };
        let args: Vec<&str> = parts.collect();

        match cmd.as_str() {
            "protocol_version" => ok(&id, "2"),
            "name" => ok(&id, "KUROOBI"),
            "version" => ok(&id, env!("CARGO_PKG_VERSION")),
            "list_commands" => ok(
                &id,
                "protocol_version\nname\nversion\nlist_commands\nknown_command\n\
                 boardsize\nclear_board\nkomi\nplay\ngenmove\nquit",
            ),
            "known_command" => {
                let known = matches!(
                    args.first().copied().unwrap_or(""),
                    "protocol_version"
                        | "name"
                        | "version"
                        | "list_commands"
                        | "known_command"
                        | "boardsize"
                        | "clear_board"
                        | "komi"
                        | "play"
                        | "genmove"
                        | "quit"
                );
                ok(&id, if known { "true" } else { "false" });
            }
            "boardsize" => {
                if args.first().copied() == Some("8") {
                    ok(&id, "");
                } else {
                    err(&id, "unacceptable size");
                }
            }
            "komi" => ok(&id, ""),
            "clear_board" => {
                board = Board::new();
                /* **1 局ごとに置換表を消す。** 温まった表が持ち越されると、
                同一重み同士の自己対戦ですら 42% のような偏った結果が出る。 */
                engine.clear_tables();
                ok(&id, "");
            }
            "play" => {
                let (Some(color), Some(v)) = (
                    args.first().copied().and_then(parse_color),
                    args.get(1).copied(),
                ) else {
                    err(&id, "syntax error");
                    continue;
                };
                /* 手番が合わなければパスを挟む。GTP は強制パスを明示しない
                ので色から復元するほかない。ただし**合法手がある側は飛ばさ
                ない** — 無条件にパスすると、駆動側の手番ずれを黙って呑んで
                盤面が静かに壊れる。 */
                if board.player() != color {
                    if board.movable() != 0 {
                        err(&id, "not your turn");
                        continue;
                    }
                    board.pass();
                }
                if v.eq_ignore_ascii_case("pass") {
                    board.pass();
                    ok(&id, "");
                    continue;
                }
                match parse_vertex(v) {
                    Some(pos) if board.movable() & (1u64 << pos.index()) != 0 => {
                        board.make_move_bits(pos);
                        ok(&id, "");
                    }
                    _ => err(&id, "illegal move"),
                }
            }
            "genmove" => {
                let Some(color) = args.first().copied().and_then(parse_color) else {
                    err(&id, "syntax error");
                    continue;
                };
                if board.player() != color {
                    if board.movable() != 0 {
                        err(&id, "not your turn");
                        continue;
                    }
                    board.pass();
                }
                if board.movable() == 0 {
                    board.pass();
                    ok(&id, "pass");
                    continue;
                }
                let deadline =
                    (time_ms > 0).then(|| Instant::now() + Duration::from_millis(time_ms));
                let n0 = engine.nodes();
                let t0 = Instant::now();
                let mv = engine.choose_within(&board, deadline);
                tot_nodes += engine.nodes() - n0;
                tot_secs += t0.elapsed().as_secs_f64();
                match mv.pos {
                    Some(pos) => {
                        board.make_move_bits(pos);
                        ok(&id, &vertex_name(pos));
                    }
                    None => {
                        board.pass();
                        ok(&id, "pass");
                    }
                }
            }
            "quit" => {
                ok(&id, "");
                report(tot_nodes, tot_secs);
                return ExitCode::SUCCESS;
            }
            other => err(&id, &format!("unknown command {other}")),
        }
    }
    report(tot_nodes, tot_secs);
    ExitCode::SUCCESS
}

/// 探索区間だけの合計を標準エラーへ。GTP の応答には混ぜない。
fn report(nodes: u64, secs: f64) {
    if nodes > 0 {
        let nps = nodes as f64 / secs.max(1e-9);
        eprintln!("stats nodes {nodes} secs {secs:.3} nps {nps:.0}");
    }
}
