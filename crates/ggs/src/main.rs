//! Kuroobi's GGS client from the command line: the GUI's client, driven by arguments and stdin.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ggs::client::{self, Cmd, LogLine, Options, Snapshot};
use ggs::coord;
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::Board;

struct Args {
    play: Option<String>,
    accept: Option<String>,
    resume: Option<String>,
    serve: bool,
    fixed_level: bool,
    server: String,
    login: Option<String>,
    pw: Option<String>,
    credentials: PathBuf,
    console: bool,
    games: usize,
    time: String,
    gtype: String,
    depth: u8,
    solve_empties: u8,
    mpc: bool,
    threads: usize,
    weights: PathBuf,
    nnue: PathBuf,
    solver_hash: u32,
    midgame_hash: u32,
    ponder: bool,
}

fn number<T: std::str::FromStr>(name: &str, v: String) -> Result<T, String>
where
    T::Err: std::fmt::Display,
{
    v.parse().map_err(|e| format!("{name}: {e}"))
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        play: None,
        accept: None,
        resume: None,
        serve: false,
        fixed_level: false,
        server: "skatgame.net:5000".into(),
        login: None,
        pw: None,
        credentials: PathBuf::from(".ggs_credentials"),
        console: false,
        games: 1,
        time: "30:00".into(),
        gtype: "8".into(),
        depth: 10,
        solve_empties: 20,
        mpc: true,
        threads: 8,
        weights: PathBuf::from("weights/linear.bin"),
        nnue: PathBuf::from("weights/nnue.bin"),
        solver_hash: 22,
        midgame_hash: EngineConfig::default().midgame_hash_bits,
        ponder: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} requires a value"));
        match arg.as_str() {
            "--play" => args.play = Some(value("--play")?),
            "--accept" => args.accept = Some(value("--accept")?),
            "--resume" => args.resume = Some(value("--resume")?),
            "--serve" => args.serve = true,
            "--fixed-level" => args.fixed_level = true,
            "--server" => args.server = value("--server")?,
            "--console" => args.console = true,
            "--login" => args.login = Some(value("--login")?),
            "--pw" => args.pw = Some(value("--pw")?),
            "--credentials" => args.credentials = PathBuf::from(value("--credentials")?),
            "--games" => args.games = number("--games", value("--games")?)?,
            "--time" => args.time = value("--time")?,
            "--type" => args.gtype = value("--type")?,
            "--depth" => args.depth = number("--depth", value("--depth")?)?,
            "--solve-empties" => {
                args.solve_empties = number("--solve-empties", value("--solve-empties")?)?
            }
            "--no-mpc" => args.mpc = false,
            "--threads" => args.threads = number("--threads", value("--threads")?)?,
            "--weights" => args.weights = PathBuf::from(value("--weights")?),
            "--nnue" => args.nnue = PathBuf::from(value("--nnue")?),
            "--solver-hash" => args.solver_hash = number("--solver-hash", value("--solver-hash")?)?,
            "--midgame-hash" => {
                args.midgame_hash = number("--midgame-hash", value("--midgame-hash")?)?
            }
            "--no-ponder" => args.ponder = false,
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

/// What the client publishes, cut down to what the command line acts on.
struct Seen {
    conn: String,
    log: Vec<LogLine>,
    incoming: Vec<(String, Vec<String>)>,
}

struct CliHost {
    tx: Mutex<Sender<Seen>>,
    last_seq: Mutex<Option<u64>>,
}

impl ggs::Host for CliHost {
    fn publish(&self, s: &Snapshot) {
        let mut last = self.last_seq.lock().unwrap();
        let log: Vec<LogLine> = s
            .log
            .iter()
            .filter(|l| last.is_none_or(|n| l.seq > n))
            .cloned()
            .collect();
        if let Some(l) = log.last() {
            *last = Some(l.seq);
        }
        let incoming = s
            .offers
            .iter()
            .filter(|o| o.incoming)
            .map(|o| (o.id.clone(), o.names.clone()))
            .collect();
        let _ = self.tx.lock().unwrap().send(Seen {
            conn: s.conn.clone(),
            log,
            incoming,
        });
    }
    fn notify(&self, title: &str, body: &str) {
        eprintln!("### {title}: {body}");
    }
    fn local_search_running(&self) -> bool {
        false
    }
}

fn stdin_lines() -> Receiver<String> {
    let (tx, rx) = mpsc::channel::<String>();
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

fn credentials(args: &Args) -> Result<(String, String), String> {
    if let (Some(l), Some(p)) = (args.login.clone(), args.pw.clone()) {
        return Ok((l, p));
    }
    let s = std::fs::read_to_string(&args.credentials)
        .map_err(|e| format!("need --login/--pw or {} ({e})", args.credentials.display()))?;
    let line = s.lines().next().unwrap_or("");
    line.split_once(':')
        .map(|(l, p)| (l.trim().to_string(), p.trim().to_string()))
        .ok_or_else(|| format!("bad credentials file {}", args.credentials.display()))
}

/// A refusal that leaves nothing to wait for while no game runs.
fn fatal_refusal(ln: &str, opponent: &str) -> bool {
    ln.starts_with("/os: ERR")
        && (ln.contains("formula")
            || ln.contains("not accepting")
            || ln.contains("variable mismatch")
            || (ln.contains("not found") && !opponent.is_empty() && ln.contains(opponent)))
}

fn play(args: &Args, config: EngineConfig) -> ExitCode {
    let (login, pw) = match credentials(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    ggs::i18n::set(ggs::i18n::from_locale(
        include_str!("../../gui/src-ui/locales/en.yaml"),
        "backend.",
    ));
    let (tx, rx) = mpsc::channel::<Seen>();
    let host = Arc::new(CliHost {
        tx: Mutex::new(tx),
        last_seq: Mutex::new(None),
    });
    let handle = client::spawn_with(
        host,
        Arc::new(Mutex::new(None)),
        Options {
            persist: false,
            engine: Some(config),
            fixed_level: args.fixed_level,
            use_book: false,
            learn: false,
        },
    );
    let send = |cmd: Cmd| {
        let _ = handle.tx.send(cmd);
    };
    send(Cmd::SetPonder(args.ponder));
    send(Cmd::Connect {
        login: login.clone(),
        pw,
        server: args.server.clone(),
    });

    let opponent = args.play.clone().unwrap_or_default();
    let stdin_rx = args.console.then(stdin_lines);
    let mut online_at: Option<Instant> = None;
    let mut in_match = false;
    let mut asked = false;
    let mut games_done = 0usize;
    let mut accepted = HashSet::<String>::new();
    let mut last_activity = Instant::now();
    let quit = |send: &dyn Fn(Cmd)| {
        send(Cmd::Disconnect);
        std::thread::sleep(Duration::from_millis(500));
        ExitCode::SUCCESS
    };

    loop {
        let seen = rx.recv_timeout(Duration::from_millis(300)).ok();
        if let Some(seen) = &seen {
            for l in &seen.log {
                match l.dir {
                    "out" => println!(">>> {}", l.text),
                    "info" => println!("### {}", l.text),
                    _ => println!("{}", l.text),
                }
                let ln = l.text.as_str();
                if l.dir != "in" {
                    continue;
                }
                if ln.starts_with("/os: update") || ln.starts_with("/os: join") {
                    last_activity = Instant::now();
                }
                if ln.starts_with("/os: + match") && ln.contains(&login) {
                    in_match = true;
                    last_activity = Instant::now();
                }
                if ln.starts_with("/os: - match") && ln.contains(&login) {
                    in_match = false;
                    asked = false;
                    games_done += 1;
                    println!("### game {games_done}/{} over: {ln}", args.games);
                    if !args.console && games_done >= args.games {
                        return quit(&send);
                    }
                }
                if !in_match && !args.console && fatal_refusal(ln, &opponent) {
                    eprintln!("### request rejected: {ln}");
                    return quit(&send);
                }
            }
            if seen.conn == "online" {
                online_at.get_or_insert_with(Instant::now);
            }
            if let Some(from) = &args.accept {
                for (id, names) in &seen.incoming {
                    let named = |n: &str| names.iter().any(|x| x == n);
                    if (from == "*" || named(from)) && named(&login) && accepted.insert(id.clone())
                    {
                        send(Cmd::Accept(id.clone()));
                        last_activity = Instant::now();
                    }
                }
            }
        }
        if let Some(rx) = &stdin_rx {
            while let Ok(line) = rx.try_recv() {
                send(Cmd::Raw(line));
                last_activity = Instant::now();
            }
        }
        if !in_match && !args.console && last_activity.elapsed() > Duration::from_secs(900) {
            eprintln!("### idle timeout (not in match)");
            return quit(&send);
        }
        let ready = online_at.is_some_and(|t| t.elapsed() >= Duration::from_secs(4));
        if ready && !in_match && !asked && !args.console && args.accept.is_none() {
            asked = true;
            last_activity = Instant::now();
            send(match &args.resume {
                Some(id) => Cmd::ResumeStored(id.clone()),
                None => Cmd::Ask {
                    gtype: args.gtype.clone(),
                    time: args.time.clone(),
                    opponent: opponent.clone(),
                    rated: true,
                },
            });
        }
    }
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
        threads: args.threads,
        mpc: args.mpc,
        solver_hash_bits: args.solver_hash,
        midgame_hash_bits: args.midgame_hash,
        weights: args.weights.clone(),
        nnue: args.nnue.clone(),
        use_book: false,
        ..Default::default()
    };
    if !args.serve {
        return play(&args, config);
    }

    let mut engine = match Engine::new(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to load the engine: {e}");
            return ExitCode::FAILURE;
        }
    };
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
    ExitCode::SUCCESS
}
