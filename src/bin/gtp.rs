//! GTP server around [`kuroobi::engine::Engine`], so this build can be driven as an external opponent by `roundrobin` (or any GTP driver).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Color, Position};

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

fn parse_args() -> Result<(EngineConfig, u64), ExitCode> {
    let mut cfg = EngineConfig {
        use_book: false,
        ..Default::default()
    };
    let mut time_ms: u64 = 0;
    let mut solve_set = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-gtp" | "-q" | "--quiet" => {}
            "-nobook" | "--no-book" => cfg.use_book = false,
            "-l" | "--level" | "--depth" => {
                cfg.depth = match it.next().and_then(|v| v.parse().ok()) {
                    Some(v) => v,
                    None => {
                        eprintln!("-l wants a depth");
                        return Err(ExitCode::FAILURE);
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
            "--head-f32" => cfg.head_f32 = true,
            "--act-units" => {
                cfg.act_units = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(cfg.act_units)
            }
            "--time-ms" => time_ms = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--weights" => cfg.weights = PathBuf::from(it.next().unwrap_or_default()),
            "--nnue" => cfg.nnue = PathBuf::from(it.next().unwrap_or_default()),
            "--nnue-base" => cfg.nnue_base = PathBuf::from(it.next().unwrap_or_default()),
            "--patterns" => {
                let name = it.next().unwrap_or_else(|| "linear".to_string());
                cfg.nnue_patterns = match kuroobi::pattern::resolve(&name, None) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
            "--patterns-file" => {
                let path = PathBuf::from(it.next().unwrap_or_default());
                cfg.nnue_patterns = match kuroobi::pattern::resolve("", Some(&path)) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
            "--book" => {
                cfg.book = PathBuf::from(it.next().unwrap_or_default());
                cfg.use_book = true;
            }
            other => {
                eprintln!("unknown argument {other}");
                return Err(ExitCode::FAILURE);
            }
        }
    }
    if !solve_set {
        cfg.solve_empties = cfg.depth.min(u8::MAX as u32) as u8;
    }
    Ok((cfg, time_ms))
}

fn main() -> ExitCode {
    let (cfg, time_ms) = match parse_args() {
        Ok(v) => v,
        Err(code) => return code,
    };

    let mut engine = match Engine::new(cfg) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut board = Board::new();
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

fn report(nodes: u64, secs: f64) {
    if nodes > 0 {
        let nps = nodes as f64 / secs.max(1e-9);
        eprintln!("stats nodes {nodes} secs {secs:.3} nps {nps:.0}");
    }
}
