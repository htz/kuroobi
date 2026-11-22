//! FlipSight bot: `FS_BOT_TOKEN=... kuroobi-flipsight [options]`, run from the repository root or
//! given `--weights DIR`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use flipsight::{Bot, Challenge, Options};

const USAGE: &str =
    "usage: FS_BOT_TOKEN=... kuroobi-flipsight [--base URL] [--weights DIR] [--threads N] [--book] \
[--margin SECS] [--lag SECS] [--practice] [--ponder] [--learn] [--pause NAMES] [--pause-log FILE] [--out DIR] \
[--challenge builtin|stronger|ID,ID...] [--until-stable N:POINTS]";

fn main() -> ExitCode {
    let Ok(token) = std::env::var("FS_BOT_TOKEN") else {
        eprintln!("FS_BOT_TOKEN is not set\n{USAGE}");
        return ExitCode::FAILURE;
    };
    let mut o = Options {
        base: "https://othelloclub.com".into(),
        token,
        // Per leg: a match's two legs think at once on half the configured threads.
        threads: (ggs::env::resources()
            .threads
            .unwrap_or_else(ggs::env::auto_threads)
            / 2)
        .max(1),
        weights: None,
        use_book: false,
        margin: 2.0,
        lag: 0.4,
        practice: false,
        ponder: false,
        learn: false,
        pause: String::new(),
        pause_log: PathBuf::new(),
        ggs_wire: PathBuf::from("ggs_games/users/skatgame.net_5000/kuroobi/ggs_wire.log"),
        out: PathBuf::from("flipsight_games"),
        challenge: Challenge::None,
        until_stable: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_default();
        match a.as_str() {
            "--base" => o.base = val(),
            "--weights" => o.weights = Some(PathBuf::from(val())),
            "--threads" => o.threads = val().parse().unwrap_or(o.threads),
            "--book" => o.use_book = true,
            "--margin" => o.margin = val().parse().unwrap_or(o.margin),
            "--lag" => o.lag = val().parse().unwrap_or(o.lag),
            "--practice" => o.practice = true,
            "--ponder" => o.ponder = true,
            "--learn" => o.learn = true,
            "--pause" => o.pause = val(),
            "--pause-log" => o.pause_log = PathBuf::from(val()),
            "--until-stable" => {
                let v = val();
                let (n, p) = v.split_once(':').unwrap_or((&v, "10"));
                o.until_stable = n.parse().ok().zip(p.parse().ok());
            }
            "--challenge" => {
                let v = val();
                o.challenge = if v == "builtin" {
                    Challenge::Builtin
                } else if v == "stronger" {
                    Challenge::Stronger
                } else {
                    Challenge::Ids(v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                }
            }
            "--out" => o.out = PathBuf::from(val()),
            _ => {
                eprintln!("unknown option {a}\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    if o.pause_log.as_os_str().is_empty() {
        o.pause_log = o.out.join("paused.txt");
    }
    let mut bot = match Bot::new(o) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    // The first Ctrl-C (or SIGTERM) lets the running matches finish; the second stops at once.
    static SIGNALS: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn on_signal(_: libc::c_int) {
        SIGNALS.fetch_add(1, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    {
        let (drain, stop) = (bot.drain_flag(), stop.clone());
        std::thread::spawn(move || loop {
            match SIGNALS.load(Ordering::SeqCst) {
                0 => {}
                1 => drain.store(true, Ordering::SeqCst),
                _ => {
                    stop.store(true, Ordering::SeqCst);
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        });
    }
    let r = bot.run(stop);
    if let Err(e) = r {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
