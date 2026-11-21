//! Several bots at once, for the GUI: their tokens and settings saved, one copy of the weights.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kuroobi::engine::{EngineAssets, EngineConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::{Api, Reply};
use crate::{engine_config, Bot, BotView, Challenge, Control, Options};

pub const BASE: &str = "https://othelloclub.com";

/// The server; `KUROOBI_FS_BASE` points at the local stand-in (`tools/fake_server.py`).
fn base() -> String {
    std::env::var("KUROOBI_FS_BASE").unwrap_or_else(|_| BASE.into())
}

/// The site's own default for matches at once.
const DEFAULT_MAX_GAMES: usize = 2;

pub trait Host: Send + Sync + 'static {
    fn publish(&self, snapshot: &Snapshot);
    fn notify(&self, title: &str, body: &str);
}

/// A bot as saved on this machine. The token never leaves this file.
#[derive(Clone, Serialize, Deserialize)]
pub struct Saved {
    pub id: i64,
    pub name: String,
    pub token: String,
    /// Threads for each leg's engine, as the site counts them.
    pub threads: usize,
    /// Matches at once taken here; `None` is the site's setting.
    pub cap: Option<usize>,
    pub book: bool,
    pub accept: bool,
    /// Depth and solve every move is played at; `None` plays by the clock.
    #[serde(default)]
    pub level: Option<[u32; 2]>,
    /// Connected when the app last ran, so it connects at the next launch.
    pub connect: bool,
    #[serde(default)]
    pub max_games: Option<usize>,
    /// `threads` is per leg; settings saved before were per match, twice as many.
    #[serde(default)]
    pub per_leg: bool,
    /// The bot's picture on the site, kept so a bot that is not connected still shows it.
    #[serde(default)]
    pub photo: Option<String>,
}

pub enum Cmd {
    /// Saves a bot after the site has named its token; it connects only when asked.
    Add {
        token: String,
        cap: Option<usize>,
        threads: Option<usize>,
        level: Option<[u32; 2]>,
    },
    Remove(i64),
    Connect(i64),
    /// Takes no new match and stops once the running ones are over.
    Disconnect(i64),
    /// The bots that were connected when the app last ran.
    ConnectSaved,
    DisconnectAll,
    Set {
        id: i64,
        cap: Option<Option<usize>>,
        threads: Option<usize>,
        book: Option<bool>,
        accept: Option<bool>,
        level: Option<Option<[u32; 2]>>,
    },
    Challenge {
        id: i64,
        target: i64,
    },
    ChangeToken {
        id: i64,
        token: String,
    },
    /// Forgets the add form's or the token dialog's outcome.
    ClearPending,
    /// Reads a disconnected bot's settings on the site again (a connected one has them from
    /// its stream).
    Refresh(i64),
}

/// The add form or the token dialog: waiting for the site, or why it failed (an `err.*` key).
#[derive(Clone, Default, Serialize)]
pub struct Pending {
    pub busy: bool,
    pub error: String,
    /// The bot added or changed, once done.
    pub done: Option<i64>,
}

#[derive(Clone, Serialize)]
pub struct BotRow {
    pub id: i64,
    pub name: String,
    pub photo: Option<String>,
    pub threads: usize,
    pub cap: Option<usize>,
    pub max_games: Option<usize>,
    pub book: bool,
    pub accept: bool,
    pub level: Option<[u32; 2]>,
    /// off | connecting | online | draining | retrying | refused
    pub conn: String,
    pub view: Option<BotView>,
}

#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    /// Threads this machine runs at once.
    pub cores: usize,
    pub bots: Vec<BotRow>,
    pub adding: Option<Pending>,
    pub token_change: Option<Pending>,
    /// Some bot is connected or connecting.
    pub connected: bool,
    /// Some bot has a match in progress.
    pub playing: bool,
}

pub struct Handle {
    pub tx: Sender<Cmd>,
    pub snapshot: Arc<Mutex<Snapshot>>,
}

pub fn dir() -> PathBuf {
    ggs::env::config_dir().join("flipsight")
}

fn store_path() -> PathBuf {
    dir().join("bots.json")
}

/// Where a bot's moves and results are kept.
pub fn records_dir(id: i64) -> PathBuf {
    dir().join(id.to_string())
}

pub fn load_saved() -> Vec<Saved> {
    let mut bots: Vec<Saved> = std::fs::read_to_string(store_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    for b in bots.iter_mut().filter(|b| !b.per_leg) {
        b.threads = (b.threads / 2).max(1);
        b.per_leg = true;
    }
    bots
}

fn save(bots: &[Saved]) {
    let _ = std::fs::create_dir_all(dir());
    let path = store_path();
    if let Ok(text) = serde_json::to_string_pretty(bots) {
        if std::fs::write(&path, text).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
}

/// Some saved bot connects at the next launch.
pub fn connects_at_launch() -> bool {
    load_saved().iter().any(|b| b.connect)
}

/// A bot's finished matches, oldest first.
pub fn results(id: i64) -> Vec<Value> {
    std::fs::read_to_string(records_dir(id).join("results.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .map(with_boards)
        .collect()
}

/// Adds each leg's final board (`cells`, `last`), replayed from its moves, to records written
/// before the board itself was kept.
fn with_boards(mut r: Value) -> Value {
    if let Some(legs) = r.get_mut("legs").and_then(Value::as_array_mut) {
        for leg in legs {
            if leg.get("cells").is_some_and(|c| !c.is_null()) {
                continue;
            }
            let Some(moves) = leg.get("moves").and_then(Value::as_str) else {
                continue;
            };
            if let Some((cells, last)) = crate::final_board(moves) {
                leg["cells"] = json!(cells);
                leg["last"] = json!(last);
            }
        }
    }
    r
}

fn level_of(l: Option<[u32; 2]>) -> Option<(u32, u8)> {
    l.map(|[depth, solve]| (depth.max(1), solve.min(64) as u8))
}

pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

struct Running {
    stop: Arc<AtomicBool>,
    drain: Arc<AtomicBool>,
    ctl: Arc<Mutex<Control>>,
    view: Arc<Mutex<BotView>>,
    done: Arc<AtomicBool>,
    told_refused: bool,
}

enum Check {
    Add {
        token: String,
        cap: Option<usize>,
        threads: Option<usize>,
        level: Option<[u32; 2]>,
    },
    Token {
        id: i64,
        token: String,
    },
    Refresh(i64),
}

/// Who a token belongs to, as the site says.
struct Who {
    id: i64,
    name: String,
    max_games: Option<usize>,
}

type Checked = (Check, Result<Who, String>);

struct Manager {
    host: Arc<dyn Host>,
    saved: Vec<Saved>,
    running: HashMap<i64, Running>,
    assets: Option<(EngineConfig, EngineAssets)>,
    snap: Arc<Mutex<Snapshot>>,
    /// What the site said about a token, from a thread so the screen keeps moving.
    checks: Sender<Checked>,
    checked: Receiver<Checked>,
}

fn who(token: &str) -> Result<Who, String> {
    match Api::new(&base(), token.to_string()).first_state() {
        Reply::Ok(v) => {
            let id = v["bot"]["id"].as_i64().ok_or("err.fs_unreachable")?;
            let entry = v["bots"]
                .as_array()
                .and_then(|bs| bs.iter().find(|b| b["id"].as_i64() == Some(id)).cloned());
            let name = v["bot"]["name"]
                .as_str()
                .or_else(|| entry.as_ref().and_then(|b| b["name"].as_str()))
                .unwrap_or("")
                .to_string();
            let max_games = v["bot"]["max_games"]
                .as_u64()
                .or_else(|| entry.as_ref().and_then(|b| b["max_games"].as_u64()))
                .map(|n| n as usize);
            Ok(Who {
                id,
                name,
                max_games,
            })
        }
        Reply::Refused { status: 401, .. } | Reply::Refused { status: 403, .. } => {
            Err("err.fs_token_refused".into())
        }
        _ => Err("err.fs_unreachable".into()),
    }
}

/// FlipSight's limit on one engine's threads.
pub const MAX_THREADS: usize = 4;

/// Matches at once this bot takes.
fn matches_of(b: &Saved) -> usize {
    b.cap.or(b.max_games).unwrap_or(DEFAULT_MAX_GAMES).max(1)
}

/// A match's two legs think at once, each with the bot's threads.
fn used_by(b: &Saved) -> usize {
    b.threads * 2 * matches_of(b)
}

fn failed(e: String) -> Option<Pending> {
    Some(Pending {
        busy: false,
        error: e,
        done: None,
    })
}

fn done(id: i64) -> Option<Pending> {
    Some(Pending {
        busy: false,
        error: String::new(),
        done: Some(id),
    })
}

fn busy() -> Option<Pending> {
    Some(Pending {
        busy: true,
        ..Default::default()
    })
}

impl Manager {
    fn used_except(&self, id: i64) -> usize {
        self.saved.iter().filter(|b| b.id != id).map(used_by).sum()
    }

    fn find(&mut self, id: i64) -> Option<&mut Saved> {
        self.saved.iter_mut().find(|b| b.id == id)
    }

    fn assets(&mut self) -> Result<(EngineConfig, EngineAssets), String> {
        if let Some(a) = &self.assets {
            return Ok(a.clone());
        }
        let cfg = engine_config(1, false);
        let assets = EngineAssets::load(&cfg)?;
        self.assets = Some((cfg.clone(), assets.clone()));
        Ok((cfg, assets))
    }

    fn connect(&mut self, id: i64) {
        if let Some(r) = self.running.get(&id) {
            if !r.done.load(Ordering::Relaxed) && !r.stop.load(Ordering::Relaxed) {
                r.drain.store(false, Ordering::Relaxed);
                r.view.lock().unwrap().conn = "online".into();
                if let Some(b) = self.find(id) {
                    b.connect = true;
                }
                save(&self.saved);
                return;
            }
        }
        let Some(b) = self.saved.iter().find(|b| b.id == id).cloned() else {
            return;
        };
        let (mut cfg, assets) = match self.assets() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("flipsight: weights not loaded: {e}");
                return;
            }
        };
        cfg.threads = b.threads;
        cfg.use_book = b.book;
        let out = records_dir(id);
        let o = Options {
            base: base(),
            token: b.token.clone(),
            threads: b.threads,
            weights: None,
            use_book: b.book,
            margin: 2.0,
            lag: 0.4,
            practice: false,
            pause: String::new(),
            pause_log: out.join("paused.txt"),
            ggs_wire: PathBuf::new(),
            out,
            challenge: Challenge::None,
            until_stable: None,
        };
        let mut bot = match Bot::with_assets(o, cfg, assets) {
            Ok(bot) => bot,
            Err(e) => {
                eprintln!("flipsight: bot {id} not started: {e}");
                return;
            }
        };
        let ctl = bot.control();
        {
            let mut c = ctl.lock().unwrap();
            c.accept = b.accept;
            c.cap = b.cap;
            c.level = level_of(b.level);
        }
        let view = bot.view();
        {
            let mut v = view.lock().unwrap();
            v.id = id;
            v.name = b.name.clone();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let drain = bot.drain_flag();
        let finished = Arc::new(AtomicBool::new(false));
        {
            let stop = stop.clone();
            let finished = finished.clone();
            std::thread::spawn(move || {
                let _ = bot.run(stop);
                finished.store(true, Ordering::Relaxed);
            });
        }
        self.running.insert(
            id,
            Running {
                stop,
                drain,
                ctl,
                view,
                done: finished,
                told_refused: false,
            },
        );
        if let Some(b) = self.find(id) {
            b.connect = true;
        }
        save(&self.saved);
    }

    /// Stops at once with no match in progress; otherwise after the matches are over.
    fn disconnect(&mut self, id: i64) {
        if let Some(r) = self.running.get(&id) {
            let mut v = r.view.lock().unwrap();
            if v.matches.is_empty() {
                r.stop.store(true, Ordering::Relaxed);
            } else {
                r.drain.store(true, Ordering::Relaxed);
            }
            if !r.done.load(Ordering::Relaxed) {
                v.conn = "draining".into();
            }
        }
        if let Some(b) = self.find(id) {
            b.connect = false;
        }
        save(&self.saved);
    }

    fn check(&self, check: Check, token: String) {
        let tx = self.checks.clone();
        std::thread::spawn(move || {
            let r = who(&token);
            let _ = tx.send((check, r));
        });
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Add {
                token,
                cap,
                threads,
                level,
            } => {
                self.snap.lock().unwrap().adding = busy();
                let check = Check::Add {
                    token: token.clone(),
                    cap,
                    threads,
                    level,
                };
                self.check(check, token);
            }
            Cmd::ChangeToken { id, token } => {
                self.snap.lock().unwrap().token_change = busy();
                let check = Check::Token {
                    id,
                    token: token.clone(),
                };
                self.check(check, token);
            }
            Cmd::Refresh(id) => {
                let live = self.running.get(&id);
                if live.is_some_and(|r| !r.done.load(Ordering::Relaxed)) {
                    return;
                }
                if let Some(token) = self.find(id).map(|b| b.token.clone()) {
                    self.check(Check::Refresh(id), token);
                }
            }
            Cmd::ClearPending => {
                let mut s = self.snap.lock().unwrap();
                s.adding = None;
                s.token_change = None;
            }
            Cmd::Remove(id) => {
                if let Some(r) = self.running.remove(&id) {
                    r.stop.store(true, Ordering::Relaxed);
                }
                self.saved.retain(|b| b.id != id);
                save(&self.saved);
            }
            Cmd::Connect(id) => self.connect(id),
            Cmd::Disconnect(id) => self.disconnect(id),
            Cmd::ConnectSaved => {
                let ids: Vec<i64> = self
                    .saved
                    .iter()
                    .filter(|b| b.connect)
                    .map(|b| b.id)
                    .collect();
                for id in ids {
                    self.connect(id);
                }
            }
            Cmd::DisconnectAll => {
                let ids: Vec<i64> = self.running.keys().copied().collect();
                for id in ids {
                    self.disconnect(id);
                }
            }
            Cmd::Set {
                id,
                cap,
                threads,
                book,
                accept,
                level,
            } => self.set(id, cap, threads, book, accept, level),
            Cmd::Challenge { id, target } => {
                if let Some(r) = self.running.get(&id) {
                    r.ctl.lock().unwrap().challenge_now = Some(target);
                }
            }
        }
    }

    fn set(
        &mut self,
        id: i64,
        cap: Option<Option<usize>>,
        threads: Option<usize>,
        book: Option<bool>,
        accept: Option<bool>,
        level: Option<Option<[u32; 2]>>,
    ) {
        let Some(b) = self.find(id) else { return };
        let mut next = b.clone();
        if let Some(c) = cap {
            next.cap = c;
        }
        if let Some(t) = threads {
            next.threads = t.clamp(1, MAX_THREADS);
        }
        if let Some(x) = book {
            next.book = x;
        }
        if let Some(x) = accept {
            next.accept = x;
        }
        if let Some(x) = level {
            next.level = x;
        }
        *b = next.clone();
        if let Some(r) = self.running.get(&id) {
            let mut c = r.ctl.lock().unwrap();
            c.cap = next.cap;
            c.threads = next.threads;
            c.use_book = next.book;
            c.accept = next.accept;
            c.level = level_of(next.level);
        }
        save(&self.saved);
    }

    fn checked(&mut self, check: Check, r: Result<Who, String>) {
        match check {
            Check::Add {
                token,
                cap,
                threads,
                level,
            } => {
                let who = match r {
                    Ok(w) => w,
                    Err(e) => {
                        self.snap.lock().unwrap().adding = failed(e);
                        return;
                    }
                };
                if let Some(b) = self.saved.iter().find(|b| b.id == who.id) {
                    let e = format!("err.fs_already_added|name={}", b.name);
                    self.snap.lock().unwrap().adding = failed(e);
                    return;
                }
                let free = cores().saturating_sub(self.used_except(who.id));
                let mut b = Saved {
                    id: who.id,
                    name: who.name,
                    token,
                    threads: 1,
                    cap,
                    book: false,
                    accept: true,
                    level,
                    connect: false,
                    max_games: who.max_games,
                    per_leg: true,
                    photo: None,
                };
                let m = matches_of(&b);
                b.threads = threads.unwrap_or(free / (2 * m)).clamp(1, MAX_THREADS);
                let id = b.id;
                self.saved.push(b);
                save(&self.saved);
                self.snap.lock().unwrap().adding = done(id);
            }
            Check::Refresh(id) => {
                let Some(w) = r.ok().filter(|w| w.id == id) else {
                    return;
                };
                if let Some(b) = self.find(id) {
                    b.max_games = w.max_games.or(b.max_games);
                    if !w.name.is_empty() {
                        b.name = w.name;
                    }
                }
                save(&self.saved);
            }
            Check::Token { id, token } => {
                let w = match r {
                    Ok(w) if w.id != id => Err(format!("err.fs_other_bot|name={}", w.name)),
                    other => other,
                };
                let w = match w {
                    Ok(w) => w,
                    Err(e) => {
                        self.snap.lock().unwrap().token_change = failed(e);
                        return;
                    }
                };
                let was_on = self.running.contains_key(&id);
                if let Some(r) = self.running.remove(&id) {
                    r.stop.store(true, Ordering::Relaxed);
                }
                let mut wanted = was_on;
                if let Some(b) = self.find(id) {
                    b.token = token;
                    b.name = w.name;
                    b.max_games = w.max_games.or(b.max_games);
                    wanted |= b.connect;
                }
                save(&self.saved);
                self.snap.lock().unwrap().token_change = done(id);
                if wanted {
                    self.connect(id);
                }
            }
        }
    }

    fn publish(&mut self) {
        let mut rows = Vec::new();
        let mut connected = false;
        let mut playing = false;
        let mut learned = false;
        for b in self.saved.iter_mut() {
            let r = self.running.get_mut(&b.id);
            let view = r.as_ref().map(|r| r.view.lock().unwrap().clone());
            let over = r.as_ref().is_some_and(|r| r.done.load(Ordering::Relaxed));
            let conn = match (&view, over) {
                (Some(v), true) if v.conn == "refused" => "refused".to_string(),
                (Some(_), true) | (None, _) => "off".to_string(),
                (Some(v), false) => v.conn.clone(),
            };
            // A stopped bot's view keeps what it last saw; the site may have changed since.
            if let Some(v) = view.as_ref().filter(|_| !over) {
                if !v.name.is_empty() && v.name != b.name {
                    b.name = v.name.clone();
                    learned = true;
                }
                if v.max_games.is_some() && v.max_games != b.max_games {
                    b.max_games = v.max_games;
                    learned = true;
                }
                if v.photo.is_some() && v.photo != b.photo {
                    b.photo = v.photo.clone();
                    learned = true;
                }
                playing |= !v.matches.is_empty();
            }
            if let Some(r) = r {
                if conn == "refused" && !r.told_refused {
                    r.told_refused = true;
                    self.host.notify(
                        &ggs::i18n::t("backend.notify.fs_refused_title"),
                        &ggs::i18n::tf("backend.notify.fs_refused_body", &[("name", &b.name)]),
                    );
                }
            }
            connected |= matches!(
                conn.as_str(),
                "connecting" | "online" | "draining" | "retrying"
            );
            rows.push(BotRow {
                id: b.id,
                name: b.name.clone(),
                photo: b.photo.clone(),
                threads: b.threads,
                cap: b.cap,
                max_games: b.max_games,
                book: b.book,
                accept: b.accept,
                level: b.level,
                conn,
                view,
            });
        }
        if learned {
            save(&self.saved);
        }
        let snap = {
            let mut s = self.snap.lock().unwrap();
            s.cores = cores();
            s.bots = rows;
            s.connected = connected;
            s.playing = playing;
            s.clone()
        };
        self.host.publish(&snap);
    }
}

pub fn spawn(host: Arc<dyn Host>) -> Handle {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let snap = Arc::new(Mutex::new(Snapshot {
        cores: cores(),
        ..Default::default()
    }));
    let (checks, checked) = mpsc::channel();
    let mut m = Manager {
        host,
        saved: load_saved(),
        running: HashMap::new(),
        assets: None,
        snap: snap.clone(),
        checks,
        checked,
    };
    std::thread::spawn(move || loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(cmd) => m.handle(cmd),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        while let Ok((check, r)) = m.checked.try_recv() {
            m.checked(check, r);
        }
        m.publish();
    });
    Handle { tx, snapshot: snap }
}
