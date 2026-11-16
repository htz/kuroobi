//! A FlipSight bot: accepts standard 8x8 matches and plays every leg with its own engine worker.

pub mod api;
mod pause;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kuroobi::engine::{EngineAssets, EngineConfig};
use kuroobi::{Board, Position};
use play::{Done, EngineWorker, Job};
use serde_json::{json, Map, Value};

use api::{Api, Leg, Match, Reply};

pub struct Options {
    pub base: String,
    pub token: String,
    /// Threads for one leg alone; two legs thinking at once get half each.
    pub threads: usize,
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
#[derive(Clone, Default)]
pub enum Challenge {
    #[default]
    None,
    /// The site's built-in bots, weakest first.
    Builtin,
    Ids(Vec<i64>),
    /// Every online bot rated above us, weakest first, over and over.
    Stronger,
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
}

impl Bot {
    pub fn new(o: Options) -> Result<Bot, String> {
        let res = ggs::env::resources();
        let engine_cfg = EngineConfig {
            threads: o.threads,
            midgame_hash_bits: res.hash_mid_bits(),
            solver_hash_bits: res.hash_end_bits(),
            weights: res.weights_path(),
            nnue: res.nnue_path(),
            book: res.book_path(),
            use_book: o.use_book,
            ..Default::default()
        };
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
        let assets = EngineAssets::load(&engine_cfg)?;
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
        Ok(Bot {
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

    fn environment(&self) -> Value {
        let cpu = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        json!({
            "cpu": cpu,
            "engine": "KUROOBI",
            "os": std::env::consts::OS,
            "engine_threads": self.o.threads,
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
        let env = self.environment();
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
            body.insert("environment".into(), env.clone());
            match self.api.call("agent_poll", body) {
                Reply::Ok(v) => {
                    let recv = Instant::now();
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
                    return Err("invalid bot credential; check FS_BOT_TOKEN".into())
                }
                Reply::Refused { status, error } => {
                    log(&format!("poll refused ({status}): {error}"))
                }
                Reply::Failed(e) => log(&format!("poll failed: {e}")),
            }
        }
        self.pauser.hold(false);
        Ok(())
    }
}

impl Bot {
    /// Acts on one poll; returns whether a match of ours is running.
    fn handle(&mut self, v: &Value, recv: Instant) -> bool {
        self.me = v["bot"]["id"].as_i64().unwrap_or(self.me);
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
        let mut running = 0;
        for m in &mine {
            match m.status.as_str() {
                "pending" if m.bot_b == self.me || m.id < 0 => self.answer(m),
                "running" => {
                    running += 1;
                    if let Some(s) = &m.state {
                        self.play(m, s, server_now, recv);
                    }
                }
                "finished" | "aborted" if !self.finished.contains(&m.id) => self.report(m),
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
        } else if running == 0 && !waiting {
            self.challenge_next(v);
        }
        running > 0
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

    fn challenge_next(&mut self, v: &Value) {
        if self.outstanding.is_some() {
            return;
        }
        self.cooling.retain(|_, until| *until > Instant::now());
        let stronger = matches!(self.o.challenge, Challenge::Stronger);
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
            let list: Vec<(i64, String)> = match &self.o.challenge {
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
                if queue.is_empty() && !stronger {
                    log("every bot on the list has been challenged");
                }
            }
            Reply::Refused { status: 409, error }
                if error.contains("busy") || error.contains("active") =>
            {
                log(&format!("{name} ({id}) is busy; later"));
                queue.push_back((id, name));
            }
            Reply::Refused { error, .. } => log(&format!("challenge {name} ({id}): {error}")),
            Reply::Failed(e) => {
                log(&format!("challenge {name} ({id}) not sent: {e}"));
                queue.push_front((id, name));
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

    fn answer(&mut self, m: &Match) {
        let mut body = Map::new();
        body.insert("match_id".into(), json!(m.id));
        let std =
            (m.game_mode.is_empty() || m.game_mode == "std") && !self.drain.load(Ordering::Relaxed);
        let action = if std { "agent_accept" } else { "agent_reject" };
        match self.api.call(action, body) {
            Reply::Ok(_) if std => {
                log(&format!("accepted match {}", m.id));
                self.answered_any_pending = true;
            }
            Reply::Ok(_) => log(&format!("declined match {} ({})", m.id, m.game_mode)),
            Reply::Refused { error, .. } => log(&format!("{action} {}: {error}", m.id)),
            Reply::Failed(e) => log(&format!("{action} {}: {e}", m.id)),
        }
    }

    fn play(&mut self, m: &Match, s: &api::State, server_now: i64, recv: Instant) {
        if s.board_size != 8 || !(s.game_mode.is_empty() || s.game_mode == "std") {
            return;
        }
        let part = if m.bot_a == self.me { 0 } else { 1 };
        let legs = s.legs.len();
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
            let threads = if legs == 2 {
                (self.o.threads / 2).max(1)
            } else {
                self.o.threads
            };
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
            .map(|l| {
                json!({"black": usize::from(l.black_participant) == part, "discs": l.discs,
                       "result_discs": l.result_discs, "status": l.status})
            })
            .collect();
        let rec = json!({
            "t": now_ms(), "match": m.id, "opponent": opp, "status": m.status, "result": result,
            "mine": mine, "theirs": theirs, "diff": mine - theirs, "finish_reason": s.finish_reason,
            "legs": legs, "rating_before": m.rating_before, "rating_after": m.rating_after,
        });
        let _ = writeln!(self.results, "{rec}");
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
