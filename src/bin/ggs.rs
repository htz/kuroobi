//! Kuroobi's GGS client.

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Position};

struct Args {
    play: Option<String>,
    accept: Option<String>,
    resume: Option<String>,
    serve: bool,
    login: Option<String>,
    pw: Option<String>,
    credentials: PathBuf,
    console: bool,
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
        accept: None,
        resume: None,
        serve: false,
        login: None,
        pw: None,
        credentials: PathBuf::from(".ggs_credentials"),
        console: false,
        games: 1,
        time: "30:00".into(),
        gtype: "8".into(),
        depth: 10,
        solve_empties: 20,
        band: 0,
        mpc: true,
        threads: 8,
        weights: PathBuf::from("weights/linear.bin"),
        nnue: PathBuf::from("weights/nnue.bin"),
        solver_hash: 22,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} requires a value"));
        match arg.as_str() {
            "--play" => args.play = Some(value("--play")?),
            "--accept" => args.accept = Some(value("--accept")?),
            "--resume" => args.resume = Some(value("--resume")?),
            "--serve" => args.serve = true,
            "--console" => args.console = true,
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
    if args.play.is_none()
        && args.accept.is_none()
        && !args.serve
        && args.resume.is_none()
        && !args.console
    {
        return Err(
            "--play <opponent>, --accept <opponent>, --resume <.id>, --console or --serve is required"
                .into(),
        );
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

enum SessionEnd {
    Stop,
    Retry,
}

#[allow(clippy::too_many_arguments)]
fn run_session(
    args: &Args,
    login: &str,
    pw: &str,
    opponent: &str,
    games_done: &mut usize,
    first_session: &mut bool,
    engine: &mut Engine,
    pick: impl Fn(&Board, &mut Engine, Option<u64>) -> (Option<Position>, Option<f32>),
) -> SessionEnd {
    use std::io::{Read, Write};
    let mut stream = dial();
    let pw_for_mask = pw.to_string();
    let mut send = {
        let mut w = stream.try_clone().expect("clone stream");
        move |cmd: &str| {
            if !pw_for_mask.is_empty() && cmd == pw_for_mask {
                println!(">>> ********");
            } else {
                println!(">>> {cmd}");
            }
            let _ = w.write_all(cmd.as_bytes()).and_then(|_| w.write_all(b"\n"));
        }
    };

    let stdin_rx = args.console.then(stdin_lines);

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
        if logged_in {
            if let Some(rx) = &stdin_rx {
                while let Ok(cmd) = rx.try_recv() {
                    send(&cmd);
                    last_activity = std::time::Instant::now();
                }
            }
        }
        if !in_match && !args.console && last_activity.elapsed().as_secs() > 900 {
            eprintln!("### idle timeout (not in match)");
            send("quit");
            return SessionEnd::Stop;
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
            *first_session = false;
            return SessionEnd::Retry;
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
                send(login);
                lines.clear();
                raw.clear();
            } else if tail.contains("password") || tail2.contains("password") {
                send(pw);
                lines.clear();
                raw.clear();
                logged_in = true;
                ready_at = Some(std::time::Instant::now());
                last_activity = std::time::Instant::now();
                for cmd in [
                    "verbose -news -faq -help -ack",
                    "tell /os client -",
                    "tell /os trust +",
                    rated_cmd(),
                    "tell /os open 1",
                ] {
                    send(cmd);
                }
                if !(*first_session) {
                    awaiting_stored = true;
                    send("tell /os stored");
                }
            }
            continue;
        }

        while let Some(ln) = lines.pop_front() {
            if awaiting_stored {
                if let Some(rest) = ln.strip_prefix('|') {
                    let id = rest.split_whitespace().next().unwrap_or("");
                    if id.starts_with('.') && rest.contains(login) {
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
                    let u = parse_update(&block, login);
                    if let Some(c) = u.color {
                        my_color = Some(c);
                        in_match = true;
                    }
                    if u.clock.is_some() {
                        my_clock_secs = u.clock;
                    }
                    if u.turn.is_some() && u.turn == my_color {
                        if let Some(board) = board_of(&u.rows, my_color) {
                            let t0 = std::time::Instant::now();
                            let (mv, val) = pick(&board, engine, my_clock_secs);
                            let m = match mv {
                                Some(p) => coord(p),
                                None => "pa".to_string(),
                            };
                            let ev = val.map(|v| format!("{v:.2}")).unwrap_or_default();
                            let secs = t0.elapsed().as_secs_f32();
                            send(&format!("tell /os play {} {m}/{ev}/{secs:.2}", u.mid));
                        }
                    }
                } else {
                    block.push(ln);
                }
                continue;
            }
            if ln.starts_with("/os: ERR") {
                let fatal = !in_match
                    && !args.console
                    && (ln.contains("formula")
                        || ln.contains("not accepting")
                        || ln.contains("variable mismatch")
                        || (ln.contains("not found")
                            && !opponent.is_empty()
                            && ln.contains(opponent)));
                if fatal {
                    eprintln!("### request rejected: {ln}");
                    send("quit");
                    return SessionEnd::Stop;
                }
                eprintln!("### ignored: {ln}");
                continue;
            }
            if let (Some(from), Some(id)) = (&args.accept, request_id(&ln)) {
                let mut words = ln.split_whitespace();
                if words.any(|w| w == from) && ln.split_whitespace().any(|w| w == login) {
                    send(&format!("tell /os accept {id}"));
                    last_activity = std::time::Instant::now();
                }
            }
            if ln.starts_with("/os: + match") && ln.contains(login) {
                in_match = true;
                last_activity = std::time::Instant::now();
            }
            if ln.starts_with("/os: - match") && ln.contains(login) {
                in_match = false;
                my_color = None;
                my_clock_secs = None;
                asked_at = None;
                *games_done += 1;
                stored_ids.retain(|_| false);
                println!("### game {}/{} over: {ln}", *games_done, args.games);
                if !args.console && (*games_done) >= args.games {
                    send("quit");
                    return SessionEnd::Stop;
                }
            }
        }

        if let Some(t0) = ready_at {
            if (*first_session) && !in_match && asked_at.is_none() && t0.elapsed().as_secs() >= 4 {
                if args.console || args.accept.is_some() {
                    continue;
                }
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

/// A request is `/os: +  .id ...` (two spaces after `+`); `+ match` starts a game instead.
fn request_id(ln: &str) -> Option<&str> {
    let rest = ln.strip_prefix("/os: +")?;
    let id = rest.split_whitespace().next()?;
    id.starts_with('.').then_some(id)
}

fn dial() -> std::net::TcpStream {
    let stream = loop {
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
    stream
}

fn stdin_lines() -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let t = line.trim_end_matches(['\r', '\n']).to_string();
                    if !t.is_empty() && tx.send(t).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

fn rated_cmd() -> &'static str {
    if std::env::var("KUROOBI_NO_RATED").is_ok() {
        eprintln!("### forced to unrated games (KUROOBI_NO_RATED)");
        return "tell /os rated -";
    }
    "tell /os rated +"
}

struct Update {
    mid: String,
    rows: Vec<Vec<char>>,
    turn: Option<char>,
    color: Option<char>,
    clock: Option<u64>,
}

fn parse_update(block: &[String], login: &str) -> Update {
    let mut u = Update {
        mid: block[0]
            .split_whitespace()
            .nth(2)
            .unwrap_or_default()
            .to_string(),
        rows: Vec::new(),
        turn: None,
        color: None,
        clock: None,
    };
    for l in block {
        let b = l.strip_prefix('|').unwrap_or(l);
        if b.starts_with(&format!("{login} ")) {
            read_seat(b, &mut u);
        }
        let t = b.trim_start();
        if let Some(rest) = t.strip_prefix(|c: char| c.is_ascii_digit()) {
            let cells: Vec<char> = rest
                .split_whitespace()
                .take(8)
                .filter(|&w| w.len() == 1 && matches!(w.as_bytes()[0], b'-' | b'*' | b'O'))
                .map(|w| w.chars().next().unwrap())
                .collect();
            if cells.len() == 8 {
                u.rows.push(cells);
            }
        }
        if t.starts_with("* to move") {
            u.turn = Some('*');
        } else if t.starts_with("O to move") {
            u.turn = Some('O');
        }
    }
    u
}

/// `kuroobi (1720.0 *) 05:00,0:0//02:00,0:0` -- our colour and clock.
fn read_seat(b: &str, u: &mut Update) {
    let Some(open) = b.find('(') else { return };
    let Some(close) = b[open..].find(')') else {
        return;
    };
    let inner = &b[open + 1..open + close];
    if let Some(c @ ('*' | 'O')) = inner.trim().chars().last() {
        u.color = Some(c);
    }
    let after = b[open + close..].trim_start_matches(')').trim_start();
    let head: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    if head.is_empty() {
        return;
    }
    let mut secs = 0u64;
    for part in head.split(':') {
        if let Ok(v) = part.parse::<u64>() {
            secs = secs * 60 + v;
        }
    }
    u.clock = Some(secs);
}

fn board_of(rows: &[Vec<char>], color: Option<char>) -> Option<Board> {
    if rows.len() != 8 {
        return None;
    }
    let mut s = String::with_capacity(66);
    for r in rows {
        for &c in r {
            s.push(if c == '*' {
                'X'
            } else if c == 'O' {
                'O'
            } else {
                '-'
            });
        }
    }
    s.push(' ');
    s.push(if color == Some('*') { 'X' } else { 'O' });
    Board::from_string(&s).ok()
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let config = EngineConfig {
        depth: args.depth as u32,
        solve_empties: args.solve_empties,
        band: args.band,
        threads: args.threads,
        mpc: args.mpc,
        solver_hash_bits: args.solver_hash,
        weights: args.weights.clone(),
        nnue: args.nnue.clone(),
        use_book: false,
        ..Default::default()
    };
    let mut engine = match Engine::new(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to load the engine: {e}");
            return ExitCode::FAILURE;
        }
    };

    let pick = |board: &Board,
                engine: &mut Engine,
                clock_secs: Option<u64>|
     -> (Option<Position>, Option<f32>) {
        if board.movable() == 0 {
            return (None, None);
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
        engine.set_levels(depth as u32, args.solve_empties, band);
        let mv = engine.choose(board);
        (
            mv.pos,
            mv.value.is_finite().then(|| mv.value.clamp(-64.0, 64.0)),
        )
    };

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
            let mv = engine.choose(&board);
            match mv.pos {
                Some(p) => println!("= {} {:.1}", coord(p), mv.value),
                None => println!("= pa {:.1}", mv.value),
            }
            std::io::stdout().flush().ok();
        }
        return ExitCode::SUCCESS;
    }

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

    let mut games_done = 0usize;
    let mut first_session = true;

    loop {
        match run_session(
            &args,
            &login,
            &pw,
            &opponent,
            &mut games_done,
            &mut first_session,
            &mut engine,
            pick,
        ) {
            SessionEnd::Stop => break,
            SessionEnd::Retry => continue,
        }
    }
    ExitCode::SUCCESS
}
