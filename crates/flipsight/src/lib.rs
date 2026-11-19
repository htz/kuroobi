//! A FlipSight bot: accepts standard 8x8 matches and plays every leg with its own engine worker.

pub mod api;
pub mod manager;
mod pause;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kuroobi::engine::{EngineAssets, EngineConfig};
use kuroobi::{Board, Position};
use play::{Done, EngineWorker, Job};
use serde::Serialize;
use serde_json::{json, Map, Value};

use api::{Api, Leg, Match, Reply};

pub struct Options {
    pub base: String,
    pub token: String,
    /// Threads for each leg's engine, as the site counts them; a match's two legs use twice that.
    pub threads: usize,
    /// Folder holding `linear.bin`, `nnue.bin` and `book.txt`; `None` keeps `resources.conf`'s.
    pub weights: Option<PathBuf>,
    pub use_book: bool,
    /// Seconds kept back from the clock on every move.
    pub margin: f64,
    /// First guess of the seconds a move loses outside the engine (polling, network); then measured.
    pub lag: f64,
    pub practice: bool,
    /// Process names (`pgrep -x` pattern) of background jobs stopped while a match runs; empty leaves them.
    pub pause: String,
    pub pause_log: PathBuf,
    /// A GGS wire log; while it shows a live match the paused jobs stay paused.
    pub ggs_wire: PathBuf,
    pub out: PathBuf,
    pub challenge: Challenge,
    /// `(n, points)`: stop (as on a first Ctrl-C) once the last `n` rating changes moved the
    /// rating by no more than `points` in all.
    pub until_stable: Option<(usize, f64)>,
}

/// Whom to challenge, one match at a time, between the server's own matches.
#[derive(Clone, Default, PartialEq)]
pub enum Challenge {
    #[default]
    None,
    /// The site's built-in bots, weakest first.
    Builtin,
    Ids(Vec<i64>),
    /// Every online bot rated above us, weakest first, over and over.
    Stronger,
}

impl Challenge {
    /// The name the screen and the saved settings use.
    pub fn key(&self) -> &'static str {
        match self {
            Challenge::None => "none",
            Challenge::Builtin => "builtin",
            Challenge::Stronger => "stronger",
            Challenge::Ids(_) => "ids",
        }
    }

    pub fn from_key(k: &str) -> Challenge {
        match k {
            "builtin" => Challenge::Builtin,
            "stronger" => Challenge::Stronger,
            _ => Challenge::None,
        }
    }
}

/// What may change while the bot runs; read at every poll.
#[derive(Clone)]
pub struct Control {
    /// Take challenges at all.
    pub accept: bool,
    pub challenge: Challenge,
    /// Matches at once this side takes; `None` is the bot's own setting on the site.
    pub cap: Option<usize>,
    /// Threads for each leg's engine, as the site counts them.
    pub threads: usize,
    pub use_book: bool,
    /// Depth and solve to play every move at; `None` plays by the clock. A leg keeps the
    /// setting it started with.
    pub level: Option<(u32, u8)>,
    /// One bot to challenge now, taken once sent.
    pub challenge_now: Option<i64>,
}

/// What the bot shows of itself, refreshed at every poll.
#[derive(Clone, Default, Serialize)]
pub struct BotView {
    pub id: i64,
    pub name: String,
    pub photo: Option<String>,
    pub rating: Option<f64>,
    /// The site's matches at once (`max_games`).
    pub max_games: Option<usize>,
    /// connecting | online | draining (finishing its matches, then stopping) | retrying | refused | stopped
    pub conn: String,
    pub matches: Vec<LiveMatch>,
    pub outstanding: Option<Outstanding>,
    /// Bots that turned a challenge down and are left alone for now.
    pub cooling: Vec<i64>,
    pub bots: Vec<OtherBot>,
    /// Moves on every time a match is written to the results.
    pub finished: u64,
}

#[derive(Clone, Default, Serialize)]
pub struct Outstanding {
    pub target: i64,
    pub name: String,
    pub since_ms: i64,
}

#[derive(Clone, Default, Serialize)]
pub struct OtherBot {
    pub id: i64,
    pub name: String,
    pub rating: Option<f64>,
    pub online: bool,
    pub playing: bool,
    pub builtin: bool,
    /// The bot's picture on the site, as a full address.
    pub photo: Option<String>,
}

#[derive(Clone, Default, Serialize)]
pub struct LiveMatch {
    pub id: i64,
    pub opponent: i64,
    pub opponent_name: String,
    pub opponent_rating: Option<f64>,
    pub legs: Vec<LiveLeg>,
}

#[derive(Clone, Default, Serialize)]
pub struct LiveLeg {
    /// a1 = 0, b1 = 1, ...; 1 black, 2 white.
    pub cells: Vec<u8>,
    pub turn: u8,
    pub my_color: u8,
    /// Black's and white's time left as of this poll.
    pub clocks_ms: Vec<f64>,
    pub discs: Vec<i64>,
    pub last: Option<i64>,
    pub ply: usize,
    pub active: bool,
    pub thinking: bool,
    /// Our moves: (move number, value, seconds).
    pub evals: Vec<(usize, f32, f32)>,
    /// The opponent's moves: (move number, seconds), from its clock.
    pub opp_secs: Vec<(usize, f32)>,
    /// The moves so far (`f5d6...`), when known from the first.
    pub moves: String,
}

/// One leg as followed poll by poll, to keep its moves.
#[derive(Default)]
struct LegHist {
    cells: Vec<u8>,
    turn: u8,
    moves: Vec<i64>,
    /// Seen from the first move on, so `moves` is the whole game.
    complete: bool,
    /// Our moves: (move number, value, seconds).
    evals: Vec<(usize, f32, f32)>,
    /// The opponent's moves: (move number, seconds).
    opp_secs: Vec<(usize, f32)>,
    /// The opponent's clock when its turn began.
    opp_clock: Option<f64>,
}

/// A bot's `photo` is a path on the site; anything else (another host) is not shown.
fn photo_url(base: &str, photo: &Value) -> Option<String> {
    let p = photo.as_str()?;
    (p.starts_with('/') && !p.starts_with("//"))
        .then(|| format!("{}{p}", base.trim_end_matches('/')))
}

fn cells_of_board(b: &Board) -> Vec<u8> {
    format!("{b}")
        .chars()
        .take(64)
        .map(|c| match c {
            'X' => 1,
            'O' => 2,
            _ => 0,
        })
        .collect()
}

/// The board a leg ended on and its last square, from its moves played from the start position;
/// none when the moves stop short of the end.
pub fn final_board(moves: &str) -> Option<(Vec<u8>, Option<usize>)> {
    let mut b = Board::new();
    let mut last = None;
    for m in moves.as_bytes().chunks(2) {
        let sq = std::str::from_utf8(m).ok()?;
        let pos = Position::from_kifu(sq).ok()?;
        if b.make_move(pos).is_err() {
            b.pass();
            b.make_move(pos).ok()?;
        }
        last = Some(index_of(pos) as usize);
    }
    b.is_game_over().then(|| (cells_of_board(&b), last))
}

/// The moves that turned one board into the next: the newly filled squares, in an order that
/// replays to it (a side with no move at a square passes). Polls can miss several moves, most
/// often at the end of a game.
fn infer_moves(prev: &[u8], turn: u8, now: &[u8]) -> Option<Vec<usize>> {
    let new: Vec<usize> = (0..64).filter(|&i| prev[i] == 0 && now[i] != 0).collect();
    if new.len() > MAX_INFERRED {
        return None;
    }
    let leg = Leg {
        board: prev.to_vec(),
        turn,
        ..Default::default()
    };
    let mut seq = Vec::with_capacity(new.len());
    find_order(board_of(&leg)?, &new, &mut seq, now).then_some(seq)
}

/// Squares filled between two polls that are still searched for an order.
const MAX_INFERRED: usize = 12;

fn find_order(b: Board, left: &[usize], seq: &mut Vec<usize>, now: &[u8]) -> bool {
    if left.is_empty() {
        return cells_of_board(&b) == now;
    }
    for (k, &sq) in left.iter().enumerate() {
        let Ok(pos) = Position::from_kifu(&coord(sq as i64)) else {
            return false;
        };
        let mut next = b;
        if next.make_move(pos).is_err() {
            next.pass();
            if next.make_move(pos).is_err() {
                continue;
            }
        }
        let rest: Vec<usize> = left
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != k)
            .map(|(_, &q)| q)
            .collect();
        seq.push(sq);
        if find_order(next, &rest, seq, now) {
            return true;
        }
        seq.pop();
    }
    false
}

/// The engine settings a bot runs with, from the shared resources.
pub fn engine_config(threads: usize, use_book: bool) -> EngineConfig {
    let res = ggs::env::resources();
    EngineConfig {
        threads,
        midgame_hash_bits: res.hash_mid_bits(),
        solver_hash_bits: res.hash_end_bits(),
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: res.book_path(),
        use_book,
        ..Default::default()
    }
}

/// The engines start before the first match: loading one costs seconds of the first move.
const WARM_WORKERS: usize = 2;

/// The server lets a challenge stand for 120 s.
const CHALLENGE_WAIT: Duration = Duration::from_secs(125);
const COOL_DOWN: Duration = Duration::from_secs(600);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Local wall-clock time, `HH:MM:SS`.
fn clock_text() -> String {
    let t = (now_ms() / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn log(msg: &str) {
    println!("{} {msg}", clock_text());
}

/// The site writes ratings as strings ("2463.4217") or numbers.
fn rating_of(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse().ok())
}

fn coord(i: i64) -> String {
    if i < 0 {
        return "pass".into();
    }
    format!("{}{}", (b'a' + (i % 8) as u8) as char, i / 8 + 1)
}

fn index_of(p: Position) -> i64 {
    let k = p.to_kifu();
    let b = k.as_bytes();
    i64::from(b[1] - b'1') * 8 + i64::from(b[0] - b'a')
}

/// The server's board (index a1 = 0, b1 = 1, ...; 1 black, 2 white) with `turn` to move.
fn board_of(leg: &Leg) -> Option<Board> {
    if leg.board.len() != 64 {
        return None;
    }
    let mut s: String = leg
        .board
        .iter()
        .map(|c| match c {
            1 => 'X',
            2 => 'O',
            _ => '-',
        })
        .collect();
    s.push(if leg.turn == 1 { 'X' } else { 'O' });
    Board::from_string(&s).ok()
}

enum Asked {
    Yes,
    Busy,
    Refused,
    Failed,
}

/// A search sent for one leg at one revision.
struct Sent {
    key: (i64, usize),
    revision: i64,
    color: u8,
    match_id: i64,
    human: Option<(Option<i64>, Option<String>)>,
    at: Instant,
    left: f64,
    budget: f64,
    empties: u32,
    threads: usize,
}

struct Slot {
    worker: EngineWorker,
    sent: Option<Sent>,
}

pub struct Bot {
    o: Options,
    api: Api,
    me: i64,
    engine_cfg: EngineConfig,
    assets: EngineAssets,
    slots: Vec<Slot>,
    /// Which slot serves which (match, leg).
    owner: HashMap<(i64, usize), usize>,
    answered: HashSet<(i64, usize, i64)>,
    finished: HashSet<i64>,
    lag: f64,
    /// A challenge was just accepted: poll again at once for the started match.
    answered_any_pending: bool,
    /// Bots still to challenge, once the list is known.
    to_challenge: Option<std::collections::VecDeque<(i64, String)>>,
    /// Set from outside: take no new match, and stop once the running ones are over.
    drain: Arc<AtomicBool>,
    draining_logged: bool,
    /// Our rating after each change, oldest first.
    ratings: Vec<f64>,
    last_challenge: Option<Instant>,
    /// Our challenge waiting for an answer: match id, target id, when sent.
    outstanding: Option<(i64, i64, Instant)>,
    /// Targets that turned a challenge down, and until when they are left alone.
    cooling: HashMap<i64, Instant>,
    pauser: pause::Pauser,
    moves: std::fs::File,
    results: std::fs::File,
    ctl: Arc<Mutex<Control>>,
    view: Arc<Mutex<BotView>>,
    hist: HashMap<(i64, usize), LegHist>,
    challenge_key: String,
    max_games: Option<usize>,
    /// Every standard bot in the last poll.
    names: HashMap<i64, OtherBot>,
    cpu: Option<String>,
}

impl Bot {
    pub fn new(o: Options) -> Result<Bot, String> {
        let mut engine_cfg = engine_config(o.threads, o.use_book);
        if let Some(d) = &o.weights {
            engine_cfg.weights = d.join("linear.bin");
            engine_cfg.nnue = d.join("nnue.bin");
            engine_cfg.book = d.join("book.txt");
        }
        let assets = EngineAssets::load(&engine_cfg)?;
        Bot::with_assets(o, engine_cfg, assets)
    }

    /// As [`Bot::new`], on weights already loaded: several bots share one copy.
    pub fn with_assets(
        o: Options,
        engine_cfg: EngineConfig,
        assets: EngineAssets,
    ) -> Result<Bot, String> {
        std::fs::create_dir_all(&o.out).map_err(|e| e.to_string())?;
        let open = |name: &str| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(o.out.join(name))
                .map_err(|e| e.to_string())
        };
        let moves = open("moves.jsonl")?;
        let results = open("results.jsonl")?;
        // The server keeps listing finished matches; those already written are not written again.
        let finished: HashSet<i64> = std::fs::read_to_string(o.out.join("results.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok()?["match"].as_i64())
            .collect();
        let began = Instant::now();
        let mut slots = Vec::new();
        for _ in 0..WARM_WORKERS {
            slots.push(Slot {
                worker: EngineWorker::spawn_with(assets.clone(), engine_cfg.clone(), false)?,
                sent: None,
            });
        }
        log(&format!(
            "{WARM_WORKERS} engines ready in {:.1}s",
            began.elapsed().as_secs_f64()
        ));
        let ctl = Control {
            accept: true,
            challenge: o.challenge.clone(),
            cap: None,
            threads: o.threads,
            use_book: o.use_book,
            level: None,
            challenge_now: None,
        };
        let cpu = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        Ok(Bot {
            ctl: Arc::new(Mutex::new(ctl)),
            view: Arc::new(Mutex::new(BotView {
                conn: "connecting".into(),
                ..Default::default()
            })),
            hist: HashMap::new(),
            challenge_key: String::new(),
            max_games: None,
            names: HashMap::new(),
            cpu,
            api: Api::new(&o.base, o.token.clone()),
            pauser: pause::Pauser::new(&o.pause, o.pause_log.clone(), o.ggs_wire.clone()),
            lag: o.lag,
            answered_any_pending: false,
            to_challenge: None,
            drain: Arc::new(AtomicBool::new(false)),
            draining_logged: false,
            ratings: Vec::new(),
            last_challenge: None,
            outstanding: None,
            cooling: HashMap::new(),
            o,
            me: 0,
            engine_cfg,
            assets,
            slots,
            owner: HashMap::new(),
            answered: HashSet::new(),
            finished,
            moves,
            results,
        })
    }

    /// The switches the bot reads at every poll.
    pub fn control(&self) -> Arc<Mutex<Control>> {
        self.ctl.clone()
    }

    /// What the bot shows of itself.
    pub fn view(&self) -> Arc<Mutex<BotView>> {
        self.view.clone()
    }

    fn set_conn(&self, conn: &str) {
        self.view.lock().unwrap().conn = conn.into();
    }

    fn environment(&self) -> Value {
        json!({
            "cpu": self.cpu,
            "engine": "KUROOBI",
            "os": std::env::consts::OS,
            "engine_threads": self.ctl.lock().unwrap().threads,
            "practice": self.o.practice,
        })
    }

    /// The flag that, once set, makes the bot finish its matches and stop.
    pub fn drain_flag(&self) -> Arc<AtomicBool> {
        self.drain.clone()
    }

    pub fn run(&mut self, stop: Arc<AtomicBool>) -> Result<(), String> {
        let (wake_tx, wake) = mpsc::channel::<()>();
        {
            let stop = stop.clone();
            let api = Api::new(&self.o.base, self.o.token.clone());
            std::thread::spawn(move || {
                let r = api.stream(&stop, |ev| match ev["type"].as_str() {
                    Some("hello") => {
                        log(&format!("stream connected, bot id {}", ev["bot_id"]));
                        let _ = wake_tx.send(());
                    }
                    Some("match") => {
                        let _ = wake_tx.send(());
                    }
                    _ => {}
                });
                if let Err(e) = r {
                    log(&format!("stream stopped: {e}"));
                }
            });
        }
        let mut last_poll: Option<Instant> = None;
        let mut busy = false;
        let mut want_poll = true;
        while !stop.load(Ordering::Relaxed) {
            if wake.recv_timeout(Duration::from_millis(30)).is_ok() {
                want_poll = true;
            }
            self.collect();
            let since = last_poll.map_or(Duration::MAX, |t| t.elapsed());
            let every = Duration::from_secs(if busy { 1 } else { 5 });
            if since < Duration::from_secs(1) || !(want_poll || since >= every) {
                continue;
            }
            want_poll = false;
            last_poll = Some(Instant::now());
            let mut body = Map::new();
            body.insert("environment".into(), self.environment());
            match self.api.call("agent_poll", body) {
                Reply::Ok(v) => {
                    let recv = Instant::now();
                    let leaving =
                        stop.load(Ordering::Relaxed) || self.drain.load(Ordering::Relaxed);
                    self.set_conn(if leaving { "draining" } else { "online" });
                    busy = self.handle(&v, recv);
                    if self.drain.load(Ordering::Relaxed) && !busy && !self.has_pending(&v) {
                        log("no match running; stopped");
                        break;
                    }
                    if self.answered_any_pending {
                        self.answered_any_pending = false;
                        want_poll = true;
                    }
                }
                Reply::Refused { status: 401, .. } => {
                    self.set_conn("refused");
                    self.pauser.hold(false);
                    return Err("invalid bot credential; check FS_BOT_TOKEN".into());
                }
                Reply::Refused { status, error } => {
                    log(&format!("poll refused ({status}): {error}"))
                }
                Reply::Failed(e) => {
                    self.set_conn("retrying");
                    log(&format!("poll failed: {e}"))
                }
            }
        }
        self.pauser.hold(false);
        self.set_conn("stopped");
        Ok(())
    }
}

impl Bot {
    /// Acts on one poll; returns whether a match of ours is running.
    fn handle(&mut self, v: &Value, recv: Instant) -> bool {
        let ctl = self.ctl.lock().unwrap().clone();
        if ctl.use_book != self.engine_cfg.use_book {
            self.engine_cfg.use_book = ctl.use_book;
            for slot in &mut self.slots {
                slot.worker.send(Job::SetUseBook(ctl.use_book));
            }
        }
        self.me = v["bot"]["id"].as_i64().unwrap_or(self.me);
        if let Some(n) = v["bot"]["max_games"].as_u64() {
            self.max_games = Some(n as usize);
        }
        self.learn_bots(v);
        self.track_rating(v);
        let server_now = v["server_now_ms"].as_i64().unwrap_or_else(now_ms);
        let matches: Vec<Match> = v["matches"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| serde_json::from_value(m.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let mine: Vec<&Match> = matches
            .iter()
            .filter(|m| m.bot_a == self.me || m.bot_b == self.me || m.id < 0)
            .collect();
        self.settle_challenge(&matches);
        let running = mine.iter().filter(|m| m.status == "running").count();
        let mut taking = running;
        for m in &mine {
            match m.status.as_str() {
                "pending" if m.bot_b == self.me || m.id < 0 => {
                    if self.answer(m, &ctl, taking) {
                        taking += 1;
                    }
                }
                "running" => {
                    if let Some(s) = &m.state {
                        self.track_legs(m, s);
                        self.play(m, s, server_now, recv, &ctl);
                    }
                }
                "finished" | "aborted" if !self.finished.contains(&m.id) => {
                    // the last moves came after the last poll of the running match
                    if let Some(s) = &m.state {
                        self.track_legs(m, s);
                    }
                    self.report(m);
                }
                _ => {}
            }
        }
        let live: HashSet<i64> = mine
            .iter()
            .filter(|m| m.status == "running")
            .map(|m| m.id)
            .collect();
        self.release(&live);
        self.pauser.hold(running > 0);
        let waiting = mine.iter().any(|m| m.status == "pending");
        if self.drain.load(Ordering::Relaxed) {
            if !self.draining_logged {
                self.draining_logged = true;
                log(&format!("stopping: taking no new match; {running} running"));
            }
        } else if let Some(target) = ctl.challenge_now.filter(|_| self.outstanding.is_none()) {
            self.ctl.lock().unwrap().challenge_now = None;
            // Our own challenge counts against the matches at once as one taken would.
            if taking >= self.cap(&ctl) {
                log("challenge not sent: the matches at once are full");
                self.publish_view(&mine, server_now, recv);
                return running > 0;
            }
            let name = self
                .names
                .get(&target)
                .map(|b| b.name.clone())
                .unwrap_or_default();
            self.send_challenge(target, &name);
        } else if running == 0 && !waiting && self.cap(&ctl) > 0 {
            if ctl.challenge.key() != self.challenge_key {
                self.challenge_key = ctl.challenge.key().into();
                self.to_challenge = None;
            }
            self.challenge_next(v, &ctl.challenge);
        }
        self.publish_view(&mine, server_now, recv);
        running > 0
    }

    /// Names, ratings and states of the standard bots in the poll, and our matches at once.
    fn learn_bots(&mut self, v: &Value) {
        let Some(bots) = v["bots"].as_array() else {
            return;
        };
        self.names.clear();
        for b in bots {
            let Some(id) = b["id"].as_i64() else { continue };
            if id == self.me {
                if let Some(n) = b["max_games"].as_u64() {
                    self.max_games = Some(n as usize);
                }
            }
            let std = b["supported_games"]
                .as_array()
                .is_none_or(|g| g.iter().any(|x| x == "std"));
            if !std || b["deleted"] == Value::Bool(true) || b["disabled"] == Value::Bool(true) {
                continue;
            }
            self.names.insert(
                id,
                OtherBot {
                    id,
                    name: b["name"].as_str().unwrap_or("").to_string(),
                    rating: b["rating"]["rating"].as_f64(),
                    online: b["online"] == Value::Bool(true),
                    playing: b["games_now"].as_u64().is_some_and(|n| n > 0),
                    builtin: b["is_builtin"] == Value::Bool(true),
                    photo: photo_url(&self.o.base, &b["photo"]),
                },
            );
        }
    }

    /// Follows each leg poll by poll, keeping its moves.
    fn track_legs(&mut self, m: &Match, s: &api::State) {
        let part = if m.bot_a == self.me { 0 } else { 1 };
        for (i, leg) in s.legs.iter().enumerate() {
            if leg.board.len() != 64 {
                continue;
            }
            let h = self.hist.entry((m.id, i)).or_insert_with(|| LegHist {
                cells: leg.board.clone(),
                turn: leg.turn,
                complete: leg.board.iter().filter(|&&c| c != 0).count() == 4,
                ..Default::default()
            });
            // The opponent's thinking time is what its clock lost: the clock of the side to move
            // stands as of its turn's start until it moves.
            let opp = if usize::from(leg.black_participant) == part {
                2
            } else {
                1
            };
            if let Some(&c) = leg.clocks_ms.get(usize::from(opp - 1)) {
                if leg.status == "active" && leg.turn == opp {
                    h.opp_clock.get_or_insert(c);
                } else if let Some(start) = h.opp_clock.take() {
                    let ply = leg
                        .board
                        .iter()
                        .filter(|&&c| c != 0)
                        .count()
                        .saturating_sub(4);
                    h.opp_secs
                        .push((ply, ((start - c) / 1000.0).max(0.0) as f32));
                }
            }
            // The site's own record of the moves, when it sends one; else they are read off the boards.
            if !leg.history.is_empty() || !s.opening_moves.is_empty() {
                h.moves = s
                    .opening_moves
                    .iter()
                    .copied()
                    .chain(leg.history.iter().map(|p| p.mv))
                    .filter(|&q| q >= 0)
                    .collect();
                h.complete = true;
            } else if h.cells != leg.board {
                match infer_moves(&h.cells, h.turn, &leg.board) {
                    Some(ms) => h.moves.extend(ms.into_iter().map(|q| q as i64)),
                    None => h.complete = false,
                }
            }
            h.cells = leg.board.clone();
            h.turn = leg.turn;
        }
    }

    fn publish_view(&mut self, mine: &[&Match], server_now: i64, recv: Instant) {
        let thinking: HashSet<(i64, usize)> = self
            .slots
            .iter()
            .filter(|s| s.worker.busy)
            .filter_map(|s| s.sent.as_ref().map(|x| x.key))
            .collect();
        let waited = recv.elapsed().as_secs_f64() * 1000.0;
        let matches: Vec<LiveMatch> = mine
            .iter()
            .filter(|m| m.status == "running")
            .filter_map(|m| {
                let s = m.state.as_ref()?;
                let part = if m.bot_a == self.me { 0 } else { 1 };
                let opp = if part == 0 { m.bot_b } else { m.bot_a };
                let legs = s
                    .legs
                    .iter()
                    .enumerate()
                    .map(|(i, leg)| {
                        let h = self.hist.get(&(m.id, i));
                        let mut clocks = leg.clocks_ms.clone();
                        if leg.status == "active" {
                            let at = usize::from(leg.turn.max(1) - 1);
                            if let Some(c) = clocks.get_mut(at) {
                                let gone = (server_now - leg.turn_started_ms) as f64 + waited;
                                *c = (*c - gone).max(0.0);
                            }
                        }
                        LiveLeg {
                            cells: leg.board.clone(),
                            turn: leg.turn,
                            my_color: if usize::from(leg.black_participant) == part {
                                1
                            } else {
                                2
                            },
                            clocks_ms: clocks,
                            discs: leg.discs.clone(),
                            last: h.and_then(|h| h.moves.last().copied()),
                            ply: leg
                                .board
                                .iter()
                                .filter(|&&c| c != 0)
                                .count()
                                .saturating_sub(4),
                            active: leg.status == "active",
                            thinking: thinking.contains(&(m.id, i)),
                            evals: h.map(|h| h.evals.clone()).unwrap_or_default(),
                            opp_secs: h.map(|h| h.opp_secs.clone()).unwrap_or_default(),
                            moves: h
                                .filter(|h| h.complete)
                                .map(|h| h.moves.iter().map(|&q| coord(q)).collect())
                                .unwrap_or_default(),
                        }
                    })
                    .collect();
                let o = self.names.get(&opp);
                Some(LiveMatch {
                    id: m.id,
                    opponent: opp,
                    opponent_name: o.map(|b| b.name.clone()).unwrap_or_default(),
                    opponent_rating: o.and_then(|b| b.rating),
                    legs,
                })
            })
            .collect();
        let me = self.names.get(&self.me).cloned();
        let outstanding = self.outstanding.map(|(_, target, at)| Outstanding {
            target,
            name: self
                .names
                .get(&target)
                .map(|b| b.name.clone())
                .unwrap_or_default(),
            since_ms: now_ms() - at.elapsed().as_millis() as i64,
        });
        let mut bots: Vec<OtherBot> = self.names.values().cloned().collect();
        bots.sort_by(|a, b| b.rating.unwrap_or(0.0).total_cmp(&a.rating.unwrap_or(0.0)));
        let mut v = self.view.lock().unwrap();
        v.id = self.me;
        if let Some(me) = me {
            v.name = me.name;
            v.photo = me.photo;
        }
        v.rating = self.ratings.last().copied();
        v.max_games = self.max_games;
        v.matches = matches;
        v.outstanding = outstanding;
        v.cooling = self.cooling.keys().copied().collect();
        v.bots = bots;
    }

    fn track_rating(&mut self, v: &Value) {
        let Some(r) = v["bot"]["rating"]["rating"].as_f64() else {
            return;
        };
        if self.ratings.last() == Some(&r) {
            return;
        }
        match self.ratings.last() {
            Some(&prev) => log(&format!("rating {r:.1} ({:+.1})", r - prev)),
            None => log(&format!("rating {r:.1}")),
        }
        self.ratings.push(r);
        let Some((n, points)) = self.o.until_stable else {
            return;
        };
        if self.ratings.len() > n && !self.drain.load(Ordering::Relaxed) {
            let moved = r - self.ratings[self.ratings.len() - 1 - n];
            if moved.abs() <= points {
                log(&format!(
                    "rating settled: {moved:+.1} over the last {n} changes; finishing up"
                ));
                self.drain.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Clears our challenge once answered; one not taken leaves that bot alone for a while.
    fn settle_challenge(&mut self, matches: &[Match]) {
        let Some((id, target, at)) = self.outstanding else {
            return;
        };
        let status = matches
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.status.as_str());
        let taken = matches!(status, Some("running") | Some("finished") | Some("aborted"));
        let waiting = status == Some("pending") && at.elapsed() < CHALLENGE_WAIT;
        if taken {
            self.outstanding = None;
        } else if !waiting && (status.is_some() || at.elapsed() >= CHALLENGE_WAIT) {
            log(&format!(
                "bot {target} did not take match {id} ({}); left alone for {} min",
                status.unwrap_or("gone"),
                COOL_DOWN.as_secs() / 60
            ));
            self.cooling.insert(target, Instant::now() + COOL_DOWN);
            self.outstanding = None;
        }
    }

    fn challenge_next(&mut self, v: &Value, challenge: &Challenge) {
        if self.outstanding.is_some() {
            return;
        }
        self.cooling.retain(|_, until| *until > Instant::now());
        let stronger = matches!(challenge, Challenge::Stronger);
        if stronger && self.to_challenge.as_ref().is_some_and(|q| q.is_empty()) {
            self.to_challenge = None;
        }
        if self.to_challenge.is_none() {
            let bots = v["bots"].as_array().cloned().unwrap_or_default();
            let std_ok = |b: &Value| {
                b["supported_games"]
                    .as_array()
                    .is_some_and(|g| g.iter().any(|x| x == "std"))
                    && b["deleted"] != Value::Bool(true)
                    && b["disabled"] != Value::Bool(true)
            };
            let name = |b: &Value| b["name"].as_str().unwrap_or("").to_string();
            let mine = v["bot"]["rating"]["rating"].as_f64().unwrap_or(0.0);
            let list: Vec<(i64, String)> = match challenge {
                Challenge::None => Vec::new(),
                Challenge::Stronger => {
                    let r = |x: &Value| x["rating"]["rating"].as_f64().unwrap_or(0.0);
                    let mut l: Vec<&Value> = bots
                        .iter()
                        .filter(|b| {
                            std_ok(b)
                                && b["online"] == Value::Bool(true)
                                && b["id"].as_i64() != Some(self.me)
                                && r(b) > mine
                        })
                        .collect();
                    l.sort_by(|a, b| r(a).total_cmp(&r(b)));
                    l.into_iter()
                        .filter_map(|b| Some((b["id"].as_i64()?, name(b))))
                        .collect()
                }
                Challenge::Builtin => {
                    let mut l: Vec<&Value> = bots
                        .iter()
                        .filter(|b| b["is_builtin"] == Value::Bool(true) && std_ok(b))
                        .collect();
                    l.sort_by(|a, b| {
                        let r = |x: &Value| x["rating"]["rating"].as_f64().unwrap_or(0.0);
                        r(a).total_cmp(&r(b))
                    });
                    l.into_iter()
                        .filter_map(|b| Some((b["id"].as_i64()?, name(b))))
                        .collect()
                }
                Challenge::Ids(ids) => ids
                    .iter()
                    .map(|&id| {
                        let n = bots
                            .iter()
                            .find(|b| b["id"].as_i64() == Some(id))
                            .map(name)
                            .unwrap_or_default();
                        (id, n)
                    })
                    .collect(),
            };
            if !list.is_empty() {
                let names: Vec<String> = list.iter().map(|(i, n)| format!("{n} ({i})")).collect();
                log(&format!("to challenge: {}", names.join(", ")));
            }
            self.to_challenge = Some(list.into());
        }
        if self
            .last_challenge
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
        {
            return;
        }
        let Some(queue) = self.to_challenge.as_mut() else {
            return;
        };
        // A bot that is not online now is kept for later.
        let online = |id: i64| {
            v["bots"].as_array().is_some_and(|bots| {
                bots.iter()
                    .any(|b| b["id"].as_i64() == Some(id) && b["online"] == Value::Bool(true))
            })
        };
        let cooling = &self.cooling;
        let Some(at) = queue
            .iter()
            .position(|&(id, _)| online(id) && !cooling.contains_key(&id))
        else {
            return;
        };
        let Some((id, name)) = queue.remove(at) else {
            return;
        };
        let last = queue.is_empty();
        match self.send_challenge(id, &name) {
            Asked::Yes if last && !stronger => log("every bot on the list has been challenged"),
            Asked::Busy => {
                if let Some(q) = self.to_challenge.as_mut() {
                    q.push_back((id, name));
                }
            }
            Asked::Failed => {
                if let Some(q) = self.to_challenge.as_mut() {
                    q.push_front((id, name));
                }
            }
            _ => {}
        }
    }

    fn send_challenge(&mut self, id: i64, name: &str) -> Asked {
        self.last_challenge = Some(Instant::now());
        let mut body = Map::new();
        body.insert("target_id".into(), json!(id));
        body.insert("game_mode".into(), json!("std"));
        match self.api.call("agent_challenge", body) {
            Reply::Ok(r) => {
                if let Some(mid) = r["match_id"].as_i64() {
                    self.outstanding = Some((mid, id, Instant::now()));
                }
                log(&format!(
                    "challenged {name} ({id}): match {}",
                    r["match_id"]
                ));
                self.answered_any_pending = true;
                Asked::Yes
            }
            Reply::Refused { status: 409, error }
                if error.contains("busy") || error.contains("active") =>
            {
                log(&format!("{name} ({id}) is busy; later"));
                Asked::Busy
            }
            Reply::Refused { error, .. } => {
                log(&format!("challenge {name} ({id}): {error}"));
                Asked::Refused
            }
            Reply::Failed(e) => {
                log(&format!("challenge {name} ({id}) not sent: {e}"));
                Asked::Failed
            }
        }
    }

    fn has_pending(&self, v: &Value) -> bool {
        v["matches"].as_array().is_some_and(|ms| {
            ms.iter().any(|m| {
                m["status"] == "pending"
                    && (m["bot_a"].as_i64() == Some(self.me)
                        || m["bot_b"].as_i64() == Some(self.me))
            })
        })
    }

    /// Matches at once: ours here, else the site's.
    fn cap(&self, ctl: &Control) -> usize {
        ctl.cap.or(self.max_games).unwrap_or(usize::MAX)
    }

    /// Takes or declines a challenge; `taking` is the matches already running or just taken.
    fn answer(&mut self, m: &Match, ctl: &Control, taking: usize) -> bool {
        let mut body = Map::new();
        body.insert("match_id".into(), json!(m.id));
        // A challenge of ours still waiting for its answer holds a place, so both cannot start.
        let held = taking + usize::from(self.outstanding.is_some());
        let take = (m.game_mode.is_empty() || m.game_mode == "std")
            && !self.drain.load(Ordering::Relaxed)
            && ctl.accept
            && held < self.cap(ctl);
        let action = if take { "agent_accept" } else { "agent_reject" };
        match self.api.call(action, body) {
            Reply::Ok(_) if take => {
                log(&format!("accepted match {}", m.id));
                self.answered_any_pending = true;
                return true;
            }
            Reply::Ok(_) => log(&format!("declined match {} ({})", m.id, m.game_mode)),
            Reply::Refused { error, .. } => log(&format!("{action} {}: {error}", m.id)),
            Reply::Failed(e) => log(&format!("{action} {}: {e}", m.id)),
        }
        false
    }

    /// Starts a search for each leg to move, each on its own engine with the bot's threads.
    fn play(&mut self, m: &Match, s: &api::State, server_now: i64, recv: Instant, ctl: &Control) {
        if s.board_size != 8 || !(s.game_mode.is_empty() || s.game_mode == "std") {
            return;
        }
        let part = if m.bot_a == self.me { 0 } else { 1 };
        let threads = ctl.threads.max(1);
        for (i, leg) in s.legs.iter().enumerate() {
            let color = if leg.black_participant == part { 1 } else { 2 };
            if leg.status != "active" || leg.turn != color {
                continue;
            }
            let key = (m.id, i);
            if self.answered.contains(&(m.id, i, leg.revision)) {
                continue;
            }
            if self
                .owner
                .get(&key)
                .and_then(|&k| self.slots[k].sent.as_ref())
                .is_some_and(|x| x.revision == leg.revision)
            {
                continue;
            }
            let human = (m.id < 0).then(|| (m.round_no, m.room_created_at.clone()));
            if leg.legal_moves.is_empty() {
                if human.is_none() {
                    self.send_move(m.id, i, leg.revision, -1, None);
                }
                continue;
            }
            let Some(board) = board_of(leg) else {
                log(&format!("match {} leg {i}: unreadable board", m.id));
                continue;
            };
            let waited =
                (server_now - leg.turn_started_ms) as f64 / 1000.0 + recv.elapsed().as_secs_f64();
            let left = leg
                .clocks_ms
                .get(usize::from(color - 1))
                .copied()
                .unwrap_or(0.0)
                / 1000.0
                - waited;
            self.lag = 0.8 * self.lag + 0.2 * (waited + 0.2);
            let mine_left = f64::from(board.empty_count().div_ceil(2));
            let budget =
                (left - self.o.margin - 1.3 * self.lag * (mine_left - 1.0).max(0.0)).max(0.05);
            let Some(k) = self.slot_for(key) else {
                log("no engine free for a leg");
                continue;
            };
            let slot = &mut self.slots[k];
            if slot.worker.threads != threads {
                slot.worker.send(Job::SetThreads(threads));
                slot.worker.threads = threads;
            }
            slot.worker.stop.reset();
            slot.worker.send(Job::Think {
                board,
                clock: Duration::from_secs_f64(budget),
                hint: None,
            });
            slot.sent = Some(Sent {
                key,
                revision: leg.revision,
                color,
                match_id: m.id,
                human,
                at: Instant::now(),
                left,
                budget,
                empties: u32::from(board.empty_count()),
                threads,
            });
        }
    }

    fn slot_for(&mut self, key: (i64, usize)) -> Option<usize> {
        if let Some(&k) = self.owner.get(&key) {
            return (!self.slots[k].worker.busy).then_some(k);
        }
        let taken: HashSet<usize> = self.owner.values().copied().collect();
        let k = match (0..self.slots.len()).find(|k| !taken.contains(k)) {
            Some(k) => k,
            None => {
                let w =
                    EngineWorker::spawn_with(self.assets.clone(), self.engine_cfg.clone(), false)
                        .ok()?;
                self.slots.push(Slot {
                    worker: w,
                    sent: None,
                });
                self.slots.len() - 1
            }
        };
        let level = self.ctl.lock().unwrap().level;
        self.slots[k].worker.send(Job::SetLevel(level));
        self.slots[k].worker.send(Job::NewGame);
        self.owner.insert(key, k);
        Some(k)
    }

    /// Sends the moves the engines have found.
    fn collect(&mut self) {
        for k in 0..self.slots.len() {
            let slot = &mut self.slots[k];
            if let Some(sent) = &slot.sent {
                if slot.worker.busy && sent.at.elapsed().as_secs_f64() > sent.budget * 2.0 + 3.0 {
                    slot.worker.stop.stop();
                }
            }
            let Ok(Done(mv, report)) = slot.worker.rx.try_recv() else {
                continue;
            };
            slot.worker.busy = false;
            let Some(sent) = slot.sent.take() else {
                continue;
            };
            let secs = sent.at.elapsed().as_secs_f64();
            let idx = mv.pos.map_or(-1, index_of);
            let rec = json!({
                "t": now_ms(), "match": sent.match_id, "leg": sent.key.1, "color": sent.color,
                "empties": sent.empties, "move": coord(idx), "value": mv.value, "depth": mv.depth,
                "exact": mv.exact, "cut": mv.cut, "book": mv.from_book, "nodes": report.nodes,
                "threads": sent.threads, "secs": (secs * 1000.0).round() / 1000.0,
                "left": (sent.left * 1000.0).round() / 1000.0,
                "budget": (sent.budget * 1000.0).round() / 1000.0,
                "lag": (self.lag * 1000.0).round() / 1000.0,
            });
            let _ = writeln!(self.moves, "{rec}");
            if let Some(h) = self.hist.get_mut(&sent.key) {
                h.evals
                    .push((61 - sent.empties as usize, mv.value, secs as f32));
            }
            self.send_move(sent.match_id, sent.key.1, sent.revision, idx, sent.human);
        }
    }

    fn send_move(
        &mut self,
        match_id: i64,
        leg: usize,
        revision: i64,
        mv: i64,
        human: Option<(Option<i64>, Option<String>)>,
    ) {
        let mut body = Map::new();
        body.insert("match_id".into(), json!(match_id));
        body.insert("leg".into(), json!(leg));
        body.insert("revision".into(), json!(revision));
        body.insert("move".into(), json!(mv));
        if let Some((round, created)) = human {
            body.insert("round_no".into(), json!(round));
            body.insert("room_created_at".into(), json!(created));
        }
        for attempt in 0..3 {
            match self.api.call("agent_move", body.clone()) {
                Reply::Ok(_) => {
                    self.answered.insert((match_id, leg, revision));
                    return;
                }
                Reply::Refused { status, error } if status == 429 || status == 503 => {
                    log(&format!("move {} retried ({status}): {error}", coord(mv)));
                    std::thread::sleep(Duration::from_millis(300 * (attempt + 1)));
                }
                Reply::Refused { error, .. } => {
                    log(&format!(
                        "move {} refused in match {match_id} leg {leg}: {error}",
                        coord(mv)
                    ));
                    return;
                }
                Reply::Failed(e) => {
                    log(&format!("move {} not sent: {e}", coord(mv)));
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    fn report(&mut self, m: &Match) {
        self.finished.insert(m.id);
        let Some(s) = &m.state else { return };
        let part = if m.bot_a == self.me { 0 } else { 1 };
        let (mine, theirs) = (
            s.totals.get(part).copied().unwrap_or(0),
            s.totals.get(1 - part).copied().unwrap_or(0),
        );
        let result = match (m.status.as_str(), s.winner) {
            ("aborted", _) => "aborted",
            (_, None) => "draw",
            (_, Some(w)) if w == part as i64 => "win",
            _ => "loss",
        };
        let opp = if part == 0 { m.bot_b } else { m.bot_a };
        let legs: Vec<Value> = s
            .legs
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let h = self.hist.remove(&(m.id, i));
                let moves = h
                    .as_ref()
                    .filter(|h| h.complete)
                    .map(|h| h.moves.iter().map(|&q| coord(q)).collect::<String>());
                let last = h.as_ref().and_then(|h| h.moves.last().copied());
                json!({"black": usize::from(l.black_participant) == part, "discs": l.discs,
                       "result_discs": l.result_discs, "status": l.status, "moves": moves,
                       "cells": (l.board.len() == 64).then_some(&l.board), "last": last,
                       "evals": h.as_ref().map(|h| h.evals.clone()).unwrap_or_default(),
                       "opp_secs": h.map(|h| h.opp_secs).unwrap_or_default()})
            })
            .collect();
        let o = self.names.get(&opp);
        let rec = json!({
            "t": now_ms(), "match": m.id, "opponent": opp, "status": m.status, "result": result,
            "opponent_name": o.map(|b| b.name.clone()), "opponent_rating": o.and_then(|b| b.rating),
            "opponent_photo": o.and_then(|b| b.photo.clone()),
            "my_rating": m.rating_after.get(part).and_then(rating_of),
            "my_rating_before": m.rating_before.get(part).and_then(rating_of),
            "mine": mine, "theirs": theirs, "diff": mine - theirs, "finish_reason": s.finish_reason,
            "legs": legs, "rating_before": m.rating_before, "rating_after": m.rating_after,
        });
        let _ = writeln!(self.results, "{rec}");
        self.view.lock().unwrap().finished += 1;
        log(&format!(
            "match {} over vs bot {opp}: {result} {mine}-{theirs} ({})",
            m.id,
            s.finish_reason.as_deref().unwrap_or("-")
        ));
    }

    /// Frees the engines of matches no longer running.
    fn release(&mut self, live: &HashSet<i64>) {
        self.owner.retain(|key, k| {
            let keep = live.contains(&key.0);
            if !keep {
                let slot = &mut self.slots[*k];
                if slot.worker.busy {
                    slot.worker.stop.stop();
                }
                slot.sent = None;
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(moves: &[&str]) -> Vec<u8> {
        let mut b = Board::new();
        for m in moves {
            b.make_move(Position::from_kifu(m).unwrap()).unwrap();
        }
        cells_of_board(&b)
    }

    fn sq(m: &str) -> usize {
        index_of(Position::from_kifu(m).unwrap()) as usize
    }

    #[test]
    fn moves_are_read_back_from_the_boards() {
        let start = after(&[]);
        assert_eq!(
            infer_moves(&start, 1, &after(&["f5"])),
            Some(vec![sq("f5")])
        );
        assert_eq!(
            infer_moves(&start, 1, &after(&["f5", "d6"])),
            Some(vec![sq("f5"), sq("d6")])
        );
        let mid = after(&["f5", "d6"]);
        assert_eq!(
            infer_moves(&mid, 1, &after(&["f5", "d6", "c3", "d3", "c4"])),
            Some(vec![sq("c3"), sq("d3"), sq("c4")])
        );
        assert_eq!(
            infer_moves(
                &mid,
                1,
                &after(&["f5", "d6", "c3", "d3", "c4", "f4", "f6", "f3"])
            ),
            Some(vec![
                sq("c3"),
                sq("d3"),
                sq("c4"),
                sq("f4"),
                sq("f6"),
                sq("f3")
            ])
        );
        let mut odd = after(&["f5"]);
        odd[sq("a1")] = 1;
        assert_eq!(infer_moves(&after(&[]), 1, &odd), None);
        assert_eq!(infer_moves(&mid, 1, &mid), Some(vec![]));
    }

    #[test]
    fn a_finished_leg_is_replayed_from_its_moves() {
        let game = "d3c5f6e3b5g7e6e7f7f8f5f4g6c4d6c3g3b6h8g4b4c6g8g5e8a6f3c7h5h4\
                    h3a5b8g2f2d7h1h2e2b7a8a7d8h6h7c8d2d1b3a4e1c1a3c2b1b2a1a2f1g1";
        let (cells, last) = final_board(game).unwrap();
        let count = |c| cells.iter().filter(|&&x| x == c).count();
        assert_eq!((count(1), count(2), last), (45, 19, Some(sq("g1"))));
        assert_eq!(final_board(&game[..game.len() - 2]), None, "not over yet");
        assert_eq!(final_board("f5f5"), None);
    }
}
