//! Kuroobi's GGS client.
//!
//! Connects to skatgame.net:5000 (Generic Game Server) and plays 8x8 on
//! the `/os` service (`KUROOBI_NO_RATED=1` pins unrated).
//!
//! Usage:
//!   ggs --play <opponent> [--games N]
//!       [--login name --pw pass | --credentials .ggs_credentials]
//!       [--depth N] [--solve-empties N] [--selective-band N] [--mpc]
//!       [--threads N] [--weights path] [--nnue path]
//!   ggs --console  (send stdin lines raw, print received lines)
//!   ggs --serve    (stdin "<64 cells> <X|O>" -> "= <coord>" bridge)
//!
//! Always set `KUROOBI_NO_RATED=1` for testing — a moving rating changes
//! every later measurement. `--console` can exercise everything the GUI
//! does: offers, accepts, watching, chat, listings, formula settings.

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
    /// Raw console mode: send stdin lines, print received lines — the
    /// hatch for exercising everything `/os` accepts.
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
    if args.play.is_none() && !args.serve && args.resume.is_none() && !args.console {
        return Err("--play <opponent>, --resume <.id>, --console or --serve is required".into());
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

    // Choose a move with the same policy as play (midgame / band /
    // solve), throttled by the clock: band off and depth -2 under 60s,
    // depth 6 under 20s, depth 4 under 8s. GGS soft timeout forfeits
    // the win, so the margins are generous.
    let pick = |board: &Board,
                search: &mut NnueSearch,
                solver: &mut Solver,
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
        /* Also return the value: GGS moves carry `move/eval/time`, and
        without it the opponent's screen shows none of our reading —
        debug games need both sides visible. */
        if board.empty_count() <= args.solve_empties {
            let r = solver.solve_with_eval(EndSolverMode::Perfect, board, Some(&evaluator));
            (r.best_move, Some(r.value as f32))
        } else if let Some(t) = selective_band(board.empty_count(), args.solve_empties, band) {
            let r = solver.solve_selective(board, Some(&evaluator), t);
            (r.best_move, Some(r.value as f32))
        } else {
            let (mv, v) = search.best_move_valued(board, depth as u32);
            /* Convert to disc scale before reporting: solved-in-search
            values are x1000, and the raw value would send +10000 to the
            opponent. The GUI applies stone_scale; this path didn't. */
            let v = if v.abs() >= 999.0 { v / 1000.0 } else { v };
            (mv, v.is_finite().then_some(v.clamp(-64.0, 64.0)))
        }
    };

    // Bridge mode: board in on stdin, move out.
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
            // Analysis: move and mover-view value; exact in the solve region.
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

    // Play mode. Credentials: --login/--pw or a name:pw file.
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

    // ============ Game session ============
    // Principle: never leave voluntarily while in a match.
    // - idle timeout applies only outside matches
    // - fatal-ERR exits apply only outside matches
    // - on disconnect, reconnect and auto-resume stored games
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
        /* Never print the password. The sent line used to be echoed,
        leaving it in plaintext in stdout and logs; the sender marks
        secrets (`send_secret`). */
        let pw_for_mask = pw.clone();
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

        /* --console: stdin lines are sent raw (no `/os` prefix added);
        the reader thread starts once and survives reconnects. */
        let stdin_rx = if args.console {
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
            Some(rx)
        } else {
            None
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
            /* Forward stdin lines (--console), but never before login
            completes — a line sent during the password prompt gets
            interpreted as the password (happened). */
            if logged_in {
                if let Some(rx) = &stdin_rx {
                    while let Ok(cmd) = rx.try_recv() {
                        send(&cmd);
                        last_activity = std::time::Instant::now();
                    }
                }
            }
            /* Idle exit only outside matches; in a match wait forever
            (opponent thinks, adjournment returns). --console never idles
            out — it is a prompt, and silently dropping the connection
            mid-check is worse than lingering. */
            if !in_match && !args.console && last_activity.elapsed().as_secs() > 900 {
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
                    /* Rated-play switch, `KUROOBI_NO_RATED=1` forbids
                    (same knob as the GUI). Always use it for self-play
                    debugging — a moving rating changes later
                    measurements. */
                    if std::env::var("KUROOBI_NO_RATED").is_ok() {
                        send("tell /os rated -");
                        eprintln!("### forced to unrated games (KUROOBI_NO_RATED)");
                    } else {
                        send("tell /os rated +");
                    }
                    send("tell /os open 1");
                    if !first_session {
                        // Reconnect: look for stored games and resume.
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
                                let t0 = std::time::Instant::now();
                                let (mv, val) =
                                    pick(&board, &mut search, &mut solver, my_clock_secs);
                                let m = match mv {
                                    Some(p) => coord(p),
                                    None => "pa".to_string(),
                                };
                                // move/eval/time; eval empty when absent.
                                let ev = val.map(|v| format!("{v:.2}")).unwrap_or_default();
                                let secs = t0.elapsed().as_secs_f32();
                                send(&format!("tell /os play {mid} {m}/{ev}/{secs:.2}"));
                            }
                        }
                    } else {
                        block.push(ln);
                    }
                    continue;
                }
                if ln.starts_with("/os: ERR") {
                    // Never leave during a match; only non-match ERRs
                    // (offer failures etc.) may exit.
                    /* --console never exits on errors. `not registered`
                    is not fatal — it only means unrated play (and some
                    listing commands refused). */
                    let fatal = !in_match
                        && !args.console
                        && (ln.contains("formula")
                            || ln.contains("not accepting")
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
                    // --console: stay after a game ends.
                    if !args.console && games_done >= args.games {
                        send("quit");
                        break 'session;
                    }
                }
            }

            if let Some(t0) = ready_at {
                if first_session && !in_match && asked_at.is_none() && t0.elapsed().as_secs() >= 4 {
                    // --console: never offer games on its own.
                    if args.console {
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
    ExitCode::SUCCESS
}
