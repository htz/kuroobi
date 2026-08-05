//! Kuroobi の GGS クライアント。
//!
//! skatgame.net:5000 (Generic Game Server) に接続し、/os (Othello Service)
//! で非レートの 8x8 を指す。プロトコルの要点はメモリ
//! ggs-server-alive-protocol-notes を参照。
//!
//! 使い方:
//!   ggs --play <相手> [--games N] [--login 名 --pw パス | --credentials .ggs_credentials]
//!       [--depth N] [--solve-empties N] [--selective-band N] [--mpc]
//!       [--threads N] [--weights path] [--nnue path]
//!   ggs --serve   (stdin で "<64面> <X|O>" を受け "= <座標>" を返すブリッジ)

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::evaluator::Evaluator;
use kuroobi::midgame::{selective_band, NnueSearch, SharedTt};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::{Board, Position};

struct Args {
    play: Option<String>,
    resume: Option<String>,
    serve: bool,
    login: Option<String>,
    pw: Option<String>,
    credentials: PathBuf,
    games: usize,
    time: String,
    gtype: String,
    depth: u8,
    solve_empties: u8,
    band: u8,
    mpc: bool,
    threads: usize,
    weights: PathBuf,
    nnue: PathBuf,
    solver_hash: u32,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        play: None,
        resume: None,
        serve: false,
        login: None,
        pw: None,
        credentials: PathBuf::from(".ggs_credentials"),
        games: 1,
        time: "30:00".into(),
        gtype: "8".into(),
        depth: 10,
        solve_empties: 20,
        band: 0,
        mpc: true,
        threads: 8,
        weights: PathBuf::from("weights/linear.bin"),
        nnue: PathBuf::from("weights/nnue-h16.bin"),
        solver_hash: 22,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} requires a value"));
        match arg.as_str() {
            "--play" => args.play = Some(value("--play")?),
            "--resume" => args.resume = Some(value("--resume")?),
            "--serve" => args.serve = true,
            "--login" => args.login = Some(value("--login")?),
            "--pw" => args.pw = Some(value("--pw")?),
            "--credentials" => args.credentials = PathBuf::from(value("--credentials")?),
            "--games" => {
                args.games = value("--games")?
                    .parse()
                    .map_err(|e| format!("--games: {e}"))?
            }
            "--time" => args.time = value("--time")?,
            "--type" => args.gtype = value("--type")?,
            "--depth" => {
                args.depth = value("--depth")?
                    .parse()
                    .map_err(|e| format!("--depth: {e}"))?
            }
            "--solve-empties" => {
                args.solve_empties = value("--solve-empties")?
                    .parse()
                    .map_err(|e| format!("--solve-empties: {e}"))?
            }
            "--selective-band" => {
                args.band = value("--selective-band")?
                    .parse()
                    .map_err(|e| format!("--selective-band: {e}"))?
            }
            "--no-mpc" => args.mpc = false,
            "--threads" => {
                args.threads = value("--threads")?
                    .parse()
                    .map_err(|e| format!("--threads: {e}"))?
            }
            "--weights" => args.weights = PathBuf::from(value("--weights")?),
            "--nnue" => args.nnue = PathBuf::from(value("--nnue")?),
            "--solver-hash" => {
                args.solver_hash = value("--solver-hash")?
                    .parse()
                    .map_err(|e| format!("--solver-hash: {e}"))?
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if args.play.is_none() && !args.serve && args.resume.is_none() {
        return Err("--play <opponent>, --resume <.id> or --serve is required".into());
    }
    Ok(args)
}

fn parse_obf(line: &str) -> Option<Board> {
    let s = line.trim();
    if s.len() < 66 {
        return None;
    }
    Board::from_string(&s[..66]).ok()
}

fn coord(p: Position) -> String {
    let i = p.index();
    format!("{}{}", (b'A' + i / 8) as char, i % 8 + 1)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut evaluator = Evaluator::new(EGAROUCID_PATTERNS);
    if let Err(e) = evaluator.load_weights(&args.weights) {
        eprintln!("failed to load {}: {e}", args.weights.display());
        return ExitCode::FAILURE;
    }
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    if let Err(e) = nn.load(&args.nnue) {
        eprintln!("failed to load nnue {}: {e}", args.nnue.display());
        return ExitCode::FAILURE;
    }
    nn.quantize();
    let nn: &'static Nnue = Box::leak(Box::new(nn));
    let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(24)));
    let mut search = NnueSearch::new(nn, tt);
    search.threads = args.threads;
    search.mpc = args.mpc;
    let mut solver = Solver::new(args.solver_hash);
    solver.set_nnue(nn, tt);
    solver.set_threads(args.threads);

    // 与えられた局面の着手を、対局路と同じ選択則 (中盤探索 / 選択帯 / 完全読み)
    // で決める。
    // 残り時間 (秒) に応じて絞る: 60 秒で帯オフ + 深さ -2、20 秒で深さ 6、
    // 8 秒で深さ 4。オセロのソフトタイムアウトは「切れたら勝ちが消える」なので
    // 保険は厚めに。
    let pick = |board: &Board,
                search: &mut NnueSearch,
                solver: &mut Solver,
                clock_secs: Option<u64>|
     -> Option<Position> {
        if board.movable() == 0 {
            return None;
        }
        let secs = clock_secs.unwrap_or(u64::MAX);
        let (depth, band) = if secs < 8 {
            (4, 0)
        } else if secs < 20 {
            (6, 0)
        } else if secs < 60 {
            (args.depth.saturating_sub(2).max(6), 0)
        } else {
            (args.depth, args.band)
        };
        if board.empty_count() <= args.solve_empties {
            solver
                .solve_with_eval(EndSolverMode::Perfect, board, Some(&evaluator))
                .best_move
        } else if let Some(t) = selective_band(board.empty_count(), args.solve_empties, band) {
            solver.solve_selective(board, Some(&evaluator), t).best_move
        } else {
            search.best_move(board, depth as u32)
        }
    };

    // ブリッジモード: stdin で盤面を受けて手を返すだけ。
    if args.serve {
        use std::io::{BufRead, Write};
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if line.trim() == "quit" {
                break;
            }
            let Some(board) = parse_obf(&line) else {
                println!("= ERR bad board");
                continue;
            };
            // 解析用: 手と評価値 (手番視点) を返す。完全読み域は厳密値。
            if board.empty_count() <= args.solve_empties {
                let r = solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(&evaluator));
                match r.best_move {
                    Some(p) => println!("= {} {}", coord(p), r.value),
                    None => println!("= pa {}", r.value),
                }
            } else {
                let (mv, v) = search.best_move_valued(&board, args.depth as u32);
                match mv {
                    Some(p) => println!("= {} {:.1}", coord(p), v),
                    None => println!("= pa {:.1}", v),
                }
            }
            std::io::stdout().flush().ok();
        }
        return ExitCode::SUCCESS;
    }

    // 対局モード。ログイン情報: --login/--pw か credentials ファイル (name:pw)。
    let (login, pw) = match (args.login.clone(), args.pw.clone()) {
        (Some(l), Some(p)) => (l, p),
        _ => match std::fs::read_to_string(&args.credentials) {
            Ok(s) => {
                let line = s.lines().next().unwrap_or("");
                match line.split_once(':') {
                    Some((l, p)) => (l.trim().to_string(), p.trim().to_string()),
                    None => {
                        eprintln!("bad credentials file {}", args.credentials.display());
                        return ExitCode::FAILURE;
                    }
                }
            }
            Err(e) => {
                eprintln!("need --login/--pw or {} ({e})", args.credentials.display());
                return ExitCode::FAILURE;
            }
        },
    };
    let opponent = args.play.clone().unwrap_or_default();

    use std::io::{Read, Write};

    // ============ 対局セッション ============
    // 原則: 対局中 (in_match) はいかなる理由でも自発的に離脱しない。
    // - idle タイムアウトは非対局時のみ
    // - 致命的 ERR による終了も非対局時のみ
    // - 接続断は再接続し、stored (中断) ゲームを自動再開する
    let mut games_done = 0usize;
    let mut first_session = true;

    'session: loop {
        let mut stream = loop {
            match std::net::TcpStream::connect(("skatgame.net", 5000)) {
                Ok(s) => break s,
                Err(e) => {
                    eprintln!("### connect failed: {e}; retry in 15s");
                    std::thread::sleep(std::time::Duration::from_secs(15));
                }
            }
        };
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(300)))
            .ok();
        let mut send = {
            let mut w = stream.try_clone().expect("clone stream");
            move |cmd: &str| {
                println!(">>> {cmd}");
                let _ = w.write_all(cmd.as_bytes()).and_then(|_| w.write_all(b"\n"));
            }
        };

        let mut raw = Vec::<u8>::new();
        let mut lines = std::collections::VecDeque::<String>::new();
        let mut block = Vec::<String>::new();
        let mut in_block = false;
        let mut logged_in = false;
        let mut my_color: Option<char> = None;
        let mut my_clock_secs: Option<u64> = None;
        let mut in_match = false;
        let mut asked_at: Option<std::time::Instant> = None;
        let mut ready_at: Option<std::time::Instant> = None;
        let mut awaiting_stored = false;
        let mut stored_ids: Vec<String> = Vec::new();
        let mut last_activity = std::time::Instant::now();
        let mut lost = false;

        loop {
            // 非対局時のみの idle 退出。対局中は無期限に待つ (相手の長考・
            // 中断復帰待ちを含む)。
            if !in_match && last_activity.elapsed().as_secs() > 900 {
                eprintln!("### idle timeout (not in match)");
                send("quit");
                break 'session;
            }
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk) {
                Ok(0) => {
                    lost = true;
                }
                Ok(n) => raw.extend_from_slice(&chunk[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => {
                    lost = true;
                }
            }
            if lost {
                eprintln!("### connection lost (in_match={in_match}); reconnecting");
                std::thread::sleep(std::time::Duration::from_secs(10));
                first_session = false;
                continue 'session;
            }
            while let Some(nl) = raw.iter().position(|&b| b == b'\n') {
                let mut line: Vec<u8> = raw.drain(..=nl).collect();
                while line.last() == Some(&b'\n') || line.last() == Some(&b'\r') {
                    line.pop();
                }
                let line = String::from_utf8_lossy(&line).into_owned();
                println!("{line}");
                lines.push_back(line);
            }
            if !logged_in {
                let tail = String::from_utf8_lossy(&raw).to_lowercase();
                let tail2 = lines
                    .iter()
                    .rev()
                    .take(3)
                    .map(|l| l.to_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ");
                if tail.contains("enter login") || tail2.contains("enter login") {
                    send(&login);
                    lines.clear();
                    raw.clear();
                } else if tail.contains("password") || tail2.contains("password") {
                    send(&pw);
                    lines.clear();
                    raw.clear();
                    logged_in = true;
                    ready_at = Some(std::time::Instant::now());
                    last_activity = std::time::Instant::now();
                    send("verbose -news -faq -help -ack");
                    send("tell /os client -");
                    send("tell /os trust +");
                    send("tell /os rated +");
                    send("tell /os open 1");
                    if !first_session {
                        // 再接続: 中断ゲームを探して自動再開する。
                        awaiting_stored = true;
                        send("tell /os stored");
                    }
                }
                continue;
            }

            while let Some(ln) = lines.pop_front() {
                if awaiting_stored {
                    // "|.82726   30 Jul 2026 ... kuroobi  opponent s8r16:l"
                    if let Some(rest) = ln.strip_prefix('|') {
                        let id = rest.split_whitespace().next().unwrap_or("");
                        if id.starts_with('.') && rest.contains(&login) {
                            stored_ids.push(id.to_string());
                        }
                    }
                    if ln == "READY" {
                        awaiting_stored = false;
                        if let Some(id) = stored_ids.first().cloned() {
                            eprintln!("### resuming stored {id}");
                            send(&format!("tell /os ask {id}"));
                            asked_at = Some(std::time::Instant::now());
                        }
                    }
                }
                if ln.starts_with("/os: update") || ln.starts_with("/os: join") {
                    last_activity = std::time::Instant::now();
                    in_block = true;
                    block.clear();
                    block.push(ln);
                    continue;
                }
                if in_block {
                    if ln == "READY" {
                        in_block = false;
                        let mut rows = Vec::<Vec<char>>::new();
                        let mut turn: Option<char> = None;
                        let mut mid = String::new();
                        if let Some(id) = block[0].split_whitespace().nth(2) {
                            mid = id.to_string();
                        }
                        for l in &block {
                            let b = l.strip_prefix('|').unwrap_or(l);
                            if b.starts_with(&format!("{login} ")) {
                                if let Some(open) = b.find('(') {
                                    if let Some(close) = b[open..].find(')') {
                                        let inner = &b[open + 1..open + close];
                                        if let Some(c) = inner.trim().chars().last() {
                                            if c == '*' || c == 'O' {
                                                my_color = Some(c);
                                                in_match = true;
                                            }
                                        }
                                        let after =
                                            b[open + close..].trim_start_matches(')').trim_start();
                                        let head: String = after
                                            .chars()
                                            .take_while(|c| c.is_ascii_digit() || *c == ':')
                                            .collect();
                                        let mut secs = 0u64;
                                        for part in head.split(':') {
                                            if let Ok(v) = part.parse::<u64>() {
                                                secs = secs * 60 + v;
                                            }
                                        }
                                        if !head.is_empty() {
                                            my_clock_secs = Some(secs);
                                        }
                                    }
                                }
                            }
                            let t = b.trim_start();
                            if let Some(rest) = t.strip_prefix(|c: char| c.is_ascii_digit()) {
                                let cells: Vec<char> = rest
                                    .split_whitespace()
                                    .take(8)
                                    .filter(|&w| {
                                        w.len() == 1
                                            && matches!(w.as_bytes()[0], b'-' | b'*' | b'O')
                                    })
                                    .map(|w| w.chars().next().unwrap())
                                    .collect();
                                if cells.len() == 8 {
                                    rows.push(cells);
                                }
                            }
                            if t.starts_with("* to move") {
                                turn = Some('*');
                            } else if t.starts_with("O to move") {
                                turn = Some('O');
                            }
                        }
                        if rows.len() == 8 && turn.is_some() && turn == my_color {
                            let mut sboard = String::with_capacity(66);
                            for r in &rows {
                                for &c in r {
                                    sboard.push(if c == '*' {
                                        'X'
                                    } else if c == 'O' {
                                        'O'
                                    } else {
                                        '-'
                                    });
                                }
                            }
                            sboard.push(' ');
                            sboard.push(if my_color == Some('*') { 'X' } else { 'O' });
                            if let Ok(board) = Board::from_string(&sboard) {
                                let m = match pick(&board, &mut search, &mut solver, my_clock_secs)
                                {
                                    Some(p) => coord(p),
                                    None => "pa".to_string(),
                                };
                                send(&format!("tell /os play {mid} {m}"));
                            }
                        }
                    } else {
                        block.push(ln);
                    }
                    continue;
                }
                if ln.starts_with("/os: ERR") {
                    // 対局中は何があっても離脱しない。非対局時のみ、申し込みが
                    // 受からない類の ERR で終了する。
                    let fatal = !in_match
                        && (ln.contains("formula")
                            || ln.contains("not accepting")
                            || ln.contains("not registered")
                            || ln.contains("variable mismatch")
                            || (ln.contains("not found")
                                && !opponent.is_empty()
                                && ln.contains(&opponent)));
                    if fatal {
                        eprintln!("### request rejected: {ln}");
                        send("quit");
                        break 'session;
                    }
                    eprintln!("### ignored: {ln}");
                    continue;
                }
                if ln.starts_with("/os: + match") && ln.contains(&login) {
                    in_match = true;
                    last_activity = std::time::Instant::now();
                }
                if ln.starts_with("/os: - match") && ln.contains(&login) {
                    in_match = false;
                    my_color = None;
                    my_clock_secs = None;
                    asked_at = None;
                    games_done += 1;
                    stored_ids.retain(|_| false);
                    println!("### game {games_done}/{} over: {ln}", args.games);
                    if games_done >= args.games {
                        send("quit");
                        break 'session;
                    }
                }
            }

            if let Some(t0) = ready_at {
                if first_session && !in_match && asked_at.is_none() && t0.elapsed().as_secs() >= 4 {
                    asked_at = Some(std::time::Instant::now());
                    if let Some(id) = &args.resume {
                        send(&format!("tell /os ask {id}"));
                    } else {
                        send(&format!(
                            "tell /os ask {} {} {opponent}",
                            args.gtype, args.time
                        ));
                    }
                }
            }
        }
    }
    ExitCode::SUCCESS
}
