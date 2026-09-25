//! GGS session thread (Generic Game Server, skatgame.net:5000).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Emitter;

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Position};

pub enum Cmd {
    Connect {
        login: String,
        pw: String,
    },
    Disconnect,
    CloseMatch(String),
    Raw(String),
    Ask {
        gtype: String,
        time: String,
        opponent: String,
        rated: bool,
    },
    Accept(String),
    Decline(String),
    Finger(String),
    Who,
    Top {
        gtype: String,
        n: u32,
    },
    Rank {
        gtype: String,
        name: String,
    },
    Watch(String),
    Look(String),
    Unwatch(String),
    Chat {
        target: String,
        text: String,
    },
    SetEngine {
        depth: u32,
        solve: u8,
        band: u8,
        ponder: bool,
    },
    ReloadThreads,
    SetPacing {
        pace: String,
        max_move_secs: u64,
        reserve_secs: u64,
        budget_use: f64,
    },
    SetAutoPlay(bool),
    SetWatchAnalysis(bool),
    SetUseBook(bool),
    SetLearn(bool),
    #[allow(clippy::enum_variant_names)]
    MatchCmd {
        id: String,
        verb: String,
        arg: String,
    },
    SetFormula {
        kind: String,
        expr: String,
    },
    ListStored,
    ListMatches,
    ResumeStored(String),
    History(String),
    ChatSeen(u64),
    SetStandby(StandbyCfg),
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct StandbyCfg {
    pub enabled: bool,
    pub auto_accept: bool,
    pub rated: bool,
    pub opponent: String,
    pub gtype: String,
    pub time: String,
    pub max_games: usize,
    pub interval_secs: u64,
}

#[derive(Clone, Serialize, Default)]
pub struct LogLine {
    pub dir: String, // "in" | "out" | "info"
    pub text: String,
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct RankRow {
    pub gtype: String,
    pub name: String,
    pub rating: f32,
    pub dev: f32,
    pub rank: u32,
    pub wins: u64,
    pub draws: u64,
    pub losses: u64,
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct FingerInfo {
    pub name: String,
    pub fields: Vec<(String, String)>,
    pub raw: Vec<String>,
}

#[derive(Clone, Serialize, Default)]
pub struct UserRow {
    pub name: String,
    pub rating: Option<f32>,
    pub dev: Option<f32>,
    pub rating_r: Option<f32>,
    pub dev_r: Option<f32>,
    pub open: Option<char>,
    pub raw: String,
}

const BARE_CMDS: [&str; 2] = ["verbose", "chann"];

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct ChatMsg {
    pub chan: String,
    pub from: String,
    pub text: String,
    #[serde(default)]
    pub at: u64,
    #[serde(default)]
    pub thread: String,
}

pub fn no_rated() -> bool {
    std::env::var("KUROOBI_NO_RATED").is_ok()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Serialize, Default)]
pub struct StoredView {
    pub id: String,
    pub raw: String,
    pub opp: String,
    pub gtype: String,
}

#[derive(Clone, Serialize, Default)]
pub struct HistoryRow {
    pub id: String,
    pub at: String,
    pub black: String,
    pub black_rating: String,
    pub white: String,
    pub white_rating: String,
    pub score: String,
    pub gtype: String,
}

#[derive(Clone, Serialize, Default)]
pub struct OngoingView {
    pub id: String,
    pub raw: String,
    pub watching: bool,
    pub names: Vec<String>,
    pub ratings: Vec<String>,
    pub gtype: String,
    pub mine: bool,
}

#[derive(Clone, Serialize, Default)]
pub struct PlayerView {
    pub name: String,
    pub rating: String,
    pub clock: String,
    pub color: String, // "black" | "white"
    pub secs: Option<u64>,
    pub ext: Option<u64>,
}

#[derive(Clone, Serialize, Default)]
pub struct Offer {
    pub id: String,
    pub raw: String,
    pub incoming: bool,
    pub names: Vec<String>,
    pub gtype: String,
    pub time: String,
    pub rated: bool,
}

#[derive(Clone, Serialize, Default)]
pub struct EvalPoint {
    pub n: u32,
    pub mine: bool,
    pub eval: Option<f32>,
}

#[derive(Clone, Serialize, Default)]
pub struct MatchView {
    pub id: String,
    pub base: String,
    pub rated: Option<bool>,
    pub over: bool,
    pub ended: String,
    pub left_by: String,
    pub result: String,
    pub archive: String,
    pub busy: String,
    pub busy_depth: u32,
    pub busy_best: Option<u32>,
    pub busy_eval: Option<f32>,
    pub busy_predict: Option<u32>,
    pub cells: Vec<u8>, // 0 empty, 1 black (*), 2 white (O)
    pub turn: String,   // "black" | "white" | ""
    pub my_color: String,
    pub opp_name: String,
    pub opp_rating: String,
    pub opp_clock: String,
    pub my_clock: String,
    pub my_secs: Option<u64>,
    pub opp_secs: Option<u64>,
    pub my_ext: Option<u64>,
    pub opp_ext: Option<u64>,
    pub in_overtime: bool,
    pub players: Vec<PlayerView>,
    pub gtype: String,
    pub moves: Vec<String>,
    pub ggf: String,
    pub last_eval: Option<f32>,
    pub last_eval_exact: bool,
    pub opp_eval: Option<f32>,
    pub opp_secs_used: Option<f32>,
    pub eval_series: Vec<EvalPoint>,
    pub last_from_book: bool,
    pub watch_eval: Option<f32>,
    pub watch_best: Option<String>,
    pub watch_exact: bool,
    pub seen: u64,
    pub order: u64,
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct GameResult {
    pub id: String,
    pub base: String,
    pub raw: String,
    pub my_diff: Option<i32>,
    pub opp: String,
    pub kifu: String,
    #[serde(default)]
    pub ggf: String,
    #[serde(default)]
    pub archive: String,
    pub seq: u64,
    #[serde(default)]
    pub my_rating: Option<f32>,
    #[serde(default)]
    pub at: u64,
}

#[derive(Clone, Serialize, Default)]
pub struct StandbyStats {
    pub games: usize,
    pub wins: usize,
    pub losses: usize,
    pub draws: usize,
    pub diff_sum: i32,
}

#[derive(Clone, Serialize, Default)]
pub struct Snapshot {
    pub conn: String, // disconnected | connecting | logging_in | online
    pub login: String,
    #[serde(skip)]
    pub my_rating: Option<f32>,
    pub my_ranks: Vec<RankRow>,
    pub log: VecDeque<LogLine>,
    pub users: Vec<UserRow>,
    pub ranking: Vec<UserRow>,
    pub fingers: HashMap<String, FingerInfo>,
    pub offers: Vec<Offer>,
    pub matches: Vec<MatchView>,
    pub ongoing: Vec<OngoingView>,
    pub notice: String,
    pub stored: Vec<StoredView>,
    pub history: HashMap<String, Vec<HistoryRow>>,
    pub chat: VecDeque<ChatMsg>,
    pub chat_seen: u64,
    pub results: Vec<GameResult>,
    pub standby: StandbyCfg,
    pub standby_stats: StandbyStats,
    pub engine: EngineCfgView,
    pub auto_play: bool,
    pub watch_analysis: bool,
    pub thinking: Option<String>,
    pub fetched_ggf: Option<FetchedGgf>,
}

pub fn demo_snapshot() -> Snapshot {
    let mut s = Snapshot {
        conn: "online".into(),
        login: "kuroobi".into(),
        ..Default::default()
    };
    s.my_ranks = vec![
        RankRow {
            gtype: "8".into(),
            name: "kuroobi".into(),
            rating: 1842.3,
            dev: 34.0,
            rank: 12,
            wins: 128,
            losses: 74,
            draws: 6,
        },
        RankRow {
            gtype: "8r16".into(),
            name: "kuroobi".into(),
            rating: 1795.0,
            dev: 51.0,
            rank: 27,
            wins: 41,
            losses: 38,
            draws: 2,
        },
    ];
    let mut users: Vec<UserRow> = vec![
        ("saio", 2245.8, 34.0),
        ("tamaki", 2011.4, 46.0),
        ("momo-bot", 2280.1, 22.0),
        ("nara", 1688.2, 91.0),
        ("kei", 1488.9, 216.0),
        ("newbie", 1200.0, 350.0),
    ]
    .into_iter()
    .map(|(n, r, d)| UserRow {
        name: n.into(),
        rating: Some(r),
        dev: Some(d),
        rating_r: Some(r - 30.0),
        dev_r: Some(d + 8.0),
        open: Some(if r > 1700.0 { '+' } else { '-' }),
        raw: format!("{n} {r}@{d}"),
    })
    .collect();
    for i in 0..26 {
        let r = 1900.0 - i as f32 * 21.0;
        users.push(UserRow {
            name: format!("player{:02}", i + 1),
            rating: Some(r),
            dev: Some(40.0 + i as f32),
            rating_r: Some(r - 30.0),
            dev_r: Some(48.0 + i as f32),
            open: Some(if i % 3 == 0 { '-' } else { '+' }),
            raw: format!("player{:02} {r}@40", i + 1),
        });
    }
    s.users = users;
    s.ranking = s.users.clone();
    s.ongoing = vec![
        OngoingView {
            id: ".71.0".into(),
            raw: "tamaki vs momo-bot".into(),
            watching: true,
            names: vec!["tamaki".into(), "momo-bot".into()],
            ratings: vec!["2011.4".into(), "2280.1".into()],
            gtype: "s8r14".into(),
            mine: false,
        },
        OngoingView {
            id: ".72.0".into(),
            raw: "nara vs kei".into(),
            watching: false,
            names: vec!["nara".into(), "kei".into()],
            ratings: vec!["1688.2".into(), "1488.9".into()],
            gtype: "8".into(),
            mine: false,
        },
    ];
    s.offers = vec![
        Offer {
            id: "1".into(),
            raw: "+ .1 saio 2245.8 8r16 15:00 R".into(),
            incoming: true,
            names: vec!["saio".into(), "kuroobi".into()],
            gtype: "s8r16".into(),
            time: "00:15:00".into(),
            rated: true,
        },
        Offer {
            id: "2".into(),
            raw: "+ .2 tamaki 2011.4 8 10:00".into(),
            incoming: false,
            names: vec!["tamaki".into(), "nara".into()],
            gtype: "8".into(),
            time: "00:10:00".into(),
            rated: false,
        },
    ];
    s.stored = vec![StoredView {
        id: "3".into(),
        raw: "tamaki".into(),
        opp: "tamaki".into(),
        gtype: "s8r16".into(),
    }];
    let cells = |mv: &[(usize, u8)]| {
        let mut c = vec![0u8; 64];
        c[27] = 2;
        c[28] = 1;
        c[35] = 1;
        c[36] = 2;
        for &(i, v) in mv {
            c[i] = v;
        }
        c
    };
    let face = |id: &str, my: &str, turn: &str, mine: u64, opp: u64| MatchView {
        rated: Some(false),
        id: id.into(),
        base: ".71".into(),
        over: false,
        cells: cells(&[(20, 1), (29, 1), (34, 2), (37, 1), (43, 2), (44, 1)]),
        turn: turn.into(),
        my_color: my.into(),
        opp_name: "saio".into(),
        opp_rating: "2245.8".into(),
        my_clock: format!("{}:{:02}", mine / 60, mine % 60),
        opp_clock: format!("{}:{:02}", opp / 60, opp % 60),
        my_secs: Some(mine),
        opp_secs: Some(opp),
        gtype: "s8r16".into(),
        moves: vec!["f5".into(), "d6".into(), "c3".into(), "d3".into()],
        last_eval: Some(2.5),
        last_from_book: true,
        ..Default::default()
    };
    s.matches = vec![
        face(".71.0", "black", "black", 664, 750),
        face(".71.1", "white", "white", 672, 746),
    ];
    s.chat = vec![
        ChatMsg {
            chan: ".chat".into(),
            from: "demo-bob".into(),
            text: "anyone up for a game?".into(),
            at: 1_754_000_000,
            thread: ".chat".into(),
        },
        ChatMsg {
            chan: ".chat".into(),
            from: "kuroobi".into(),
            text: "sure, 15 min?".into(),
            at: 1_754_000_060,
            thread: ".chat".into(),
        },
        ChatMsg {
            chan: ".chat".into(),
            from: "saio".into(),
            text: "good luck both".into(),
            at: 1_754_002_800,
            thread: ".chat".into(),
        },
    ]
    .into();
    s.results = vec![
        (".70", "saio", 6i32, 1_754_003_000u64, 1842.3, "s8r16"),
        (".69", "tamaki", -4, 1_753_990_000, 1836.1, "8"),
        (".68", "nobu", 12, 1_753_900_000, 1840.0, "s8r16"),
        (".67", "kei", 18, 1_753_820_000, 1828.4, "8r16"),
        (".66", "momo-bot", -18, 1_753_740_000, 1812.9, "s8"),
        (".65", "nara", 0, 1_753_650_000, 1825.5, "8"),
    ]
    .into_iter()
    .map(|(id, opp, d, at, rate, gt)| GameResult {
        id: id.into(),
        base: format!("{gt}{id}"),
        raw: format!("{id} {opp} {d:+}"),
        my_diff: Some(d),
        opp: opp.into(),
        at,
        my_rating: Some(rate),
        ..Default::default()
    })
    .collect();
    s.log = vec![
        LogLine {
            dir: "info".into(),
            text: "connected (skatgame.net:5000)".into(),
        },
        LogLine {
            dir: "out".into(),
            text: "tell /os play .71.0 F5".into(),
        },
        LogLine {
            dir: "in".into(),
            text: "/os: match .71.0 update".into(),
        },
        LogLine {
            dir: "in".into(),
            text: "/os: | 1 kuroobi 1842.3 11:04 vs saio 2245.8 12:30".into(),
        },
        LogLine {
            dir: "out".into(),
            text: "tell /os look".into(),
        },
        LogLine {
            dir: "in".into(),
            text: "/os: 12 waiting requests".into(),
        },
    ]
    .into();
    s.standby_stats = StandbyStats {
        games: 12,
        wins: 7,
        losses: 4,
        draws: 1,
        diff_sum: 38,
    };
    let finger = |name: &str, accept: &str| FingerInfo {
        name: name.into(),
        fields: vec![
            ("open".into(), "1".into()),
            ("rated".into(), "+".into()),
            ("accept".into(), accept.into()),
            ("decline".into(), "rated&or>2400".into()),
            ("play".into(), "0".into()),
            ("stored (+)".into(), "0".into()),
            ("info".into(), "demo data for screen checks".into()),
            ("since".into(), "2026-01-15".into()),
        ],
        raw: vec![format!("{name} 1842.3@34.0")],
    };
    s.fingers.insert(
        "kuroobi".into(),
        finger("kuroobi", "rand&discs>=14&discs<=20&mt1>=120"),
    );
    s.fingers
        .insert("saio".into(), finger("saio", "rand&or>=1600"));
    s
}

#[derive(Clone, Serialize)]
pub struct FetchedGgf {
    pub id: String,
    pub ggf: String,
    pub parts: Vec<String>,
    pub error: String,
}

#[derive(Clone, Serialize)]
pub struct EngineCfgView {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
    pub threads: usize,
    pub ready: bool,
    pub use_book: bool,
    pub book_loaded: bool,
    pub learn: bool,
    pub ponder: bool,
    pub pace: String,
    pub max_move_secs: u64,
    pub reserve_secs: u64,
    pub budget_use: f64,
}

impl Default for EngineCfgView {
    fn default() -> Self {
        EngineCfgView {
            depth: 22,
            solve: 26,
            band: 6,
            threads: 4,
            ready: false,
            use_book: true,
            book_loaded: false,
            learn: true,
            ponder: true,
            pace: "fast".into(),
            max_move_secs: 0,
            reserve_secs: 20,
            budget_use: 2.5,
        }
    }
}

pub struct Handle {
    pub tx: Sender<Cmd>,
    pub snapshot: Arc<Mutex<Snapshot>>,
}

pub fn spawn(
    app: tauri::AppHandle,
    local_stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    local_activity: Arc<Mutex<crate::Activity>>,
) -> Handle {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let snapshot = Arc::new(Mutex::new(Snapshot {
        conn: "disconnected".into(),
        standby: StandbyCfg {
            enabled: false,
            auto_accept: true,
            rated: true,
            opponent: String::new(),
            gtype: "s8r16".into(),
            time: "00:15:00".into(),
            max_games: 0,
            interval_secs: 20,
        },
        auto_play: true,
        watch_analysis: true,
        results: load_history(),
        ..Default::default()
    }));
    let snap2 = snapshot.clone();
    std::thread::spawn(move || run(app, rx, snap2, local_stop, local_activity));
    Handle { tx, snapshot }
}

fn history_path() -> PathBuf {
    for c in ["ggs_games", "../ggs_games", "../../ggs_games"] {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return p.join("history.jsonl");
        }
    }
    PathBuf::from("ggs_history.jsonl")
}

/// The margin a stored result really carries, read back from the server's own
/// line rather than from the number beside it.
///
/// `- match .63 2696 Rhapsody 2358 kuroobi s8r14 R +29.00` is +29 *for the name
/// written first*, so that row is a 29-disc loss; older builds filed it as a
/// win, and a stretch of August reads as nine wins that the rating says were
/// defeats -- it fell 2468 to 2324 across them. A synchro pair scores as the
/// mean of its boards, so the margin for the match is twice the line.
fn diff_from_raw(raw: &str, opp: &str) -> Option<i32> {
    if opp.is_empty() {
        return None;
    }
    let toks: Vec<&str> = raw.split_whitespace().collect();
    let score = toks.iter().find_map(|t| {
        t.starts_with(['+', '-'])
            .then(|| t.parse::<f32>().ok())
            .flatten()
    })?;
    let first = toks
        .iter()
        .skip(1)
        .find(|t| t.len() >= 2 && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))?;
    let synchro = toks.iter().any(|t| t.starts_with("s8"));
    let v = (if synchro { score * 2.0 } else { score }).round() as i32;
    Some(if *first == opp { -v } else { v })
}

fn load_history() -> Vec<GameResult> {
    let Ok(text) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    let mut out: Vec<GameResult> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<GameResult>(l).ok())
        .collect();
    for r in &mut out {
        if let Some(d) = diff_from_raw(&r.raw, &r.opp) {
            r.my_diff = Some(d);
        }
    }
    out.reverse();
    out.truncate(500);
    out
}

fn append_history(r: &GameResult) {
    if let Ok(line) = serde_json::to_string(r) {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(history_path())
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn chat_path(login: &str) -> PathBuf {
    let safe: String = login
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = history_path()
        .parent()
        .unwrap_or(&PathBuf::from("."))
        .join("chat");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{safe}.jsonl"))
}

const CHAT_KEEP: usize = 5000;

fn load_chat(login: &str) -> Vec<ChatMsg> {
    let Ok(text) = std::fs::read_to_string(chat_path(login)) else {
        return Vec::new();
    };
    let mut out: Vec<ChatMsg> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<ChatMsg>(l).ok())
        .collect();
    if out.len() > CHAT_KEEP {
        out.drain(..out.len() - CHAT_KEEP);
    }
    out
}

fn settings_path() -> PathBuf {
    history_path()
        .parent()
        .unwrap_or(&PathBuf::from("."))
        .join("ggs_settings.json")
}

/// A field the file does not carry falls back to the value a fresh install
/// uses. Without this, one missing field made the whole read fail, the defaults
/// took over, and the next save overwrote what the user had set -- which is how
/// a `budget_use` of 5.8 became 2.5.
#[derive(Serialize, serde::Deserialize)]
#[serde(default)]
struct SavedSettings {
    depth: u32,
    solve: u8,
    band: u8,
    ponder: bool,
    pace: String,
    max_move_secs: u64,
    reserve_secs: u64,
    budget_use: f64,
    auto_play: bool,
    watch_analysis: bool,
    use_book: bool,
    learn: bool,
}

impl Default for SavedSettings {
    fn default() -> Self {
        SavedSettings {
            depth: 22,
            solve: 26,
            band: 6,
            ponder: true,
            pace: "fast".into(),
            max_move_secs: 0,
            reserve_secs: 20,
            budget_use: 2.5,
            auto_play: false,
            watch_analysis: false,
            use_book: true,
            learn: true,
        }
    }
}

fn save_settings(ctx: &Ctx) {
    let s = ctx.snap.lock().unwrap();
    let e = &s.engine;
    let v = SavedSettings {
        depth: e.depth,
        solve: e.solve,
        band: e.band,
        ponder: e.ponder,
        pace: e.pace.clone(),
        max_move_secs: e.max_move_secs,
        reserve_secs: e.reserve_secs,
        budget_use: e.budget_use,
        auto_play: s.auto_play,
        watch_analysis: s.watch_analysis,
        use_book: e.use_book,
        learn: e.learn,
    };
    drop(s);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(settings_path(), text);
    }
}

fn wire_path() -> PathBuf {
    history_path()
        .parent()
        .unwrap_or(&PathBuf::from("."))
        .join("ggs_wire.log")
}

fn roll_if_large(p: &std::path::Path) {
    const CAP: u64 = 8 * 1024 * 1024;
    if std::fs::metadata(p).is_ok_and(|m| m.len() > CAP) {
        let _ = std::fs::rename(p, p.with_extension("log.1"));
    }
}

fn load_settings() -> Option<SavedSettings> {
    let text = std::fs::read_to_string(settings_path()).ok()?;
    serde_json::from_str(&text).ok()
}

static MATCH_ORDER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn chat_seen_path(login: &str) -> PathBuf {
    chat_path(login).with_extension("seen")
}

fn load_chat_seen(login: &str) -> u64 {
    std::fs::read_to_string(chat_seen_path(login))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or_else(now_secs)
}

fn save_chat_seen(login: &str, at: u64) {
    if login.is_empty() {
        return;
    }
    let path = chat_seen_path(login);
    let cur = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if at > cur {
        let _ = std::fs::write(&path, at.to_string());
    }
}

fn append_chat(login: &str, m: &ChatMsg) {
    if login.is_empty() {
        return;
    }
    let path = chat_path(login);
    let Ok(line) = serde_json::to_string(m) else {
        return;
    };
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{line}");
    }
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > (CHAT_KEEP as u64) * 120 * 12 / 10 {
            let kept = load_chat(login);
            let body: String = kept
                .iter()
                .filter_map(|m| serde_json::to_string(m).ok())
                .map(|l| l + "\n")
                .collect();
            let tmp = path.with_extension("jsonl.tmp");
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
}

fn resources() -> kuroobi::resources::Resources {
    crate::resources()
}

fn session_lock_path() -> PathBuf {
    if let Ok(p) = std::env::var("KUROOBI_SESSION_LOCK") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::temp_dir().join("kuroobi_ggs.pid")
}

pub fn session_locked_by_other() -> bool {
    let Ok(s) = std::fs::read_to_string(session_lock_path()) else {
        return false;
    };
    match s.trim().parse::<i32>() {
        Ok(pid) => pid != std::process::id() as i32 && unsafe { libc::kill(pid, 0) } == 0,
        Err(_) => false,
    }
}

fn try_lock_session() -> Result<(), i32> {
    let path = session_lock_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(pid) = s.trim().parse::<i32>() {
            if pid != std::process::id() as i32 && unsafe { libc::kill(pid, 0) } == 0 {
                return Err(pid);
            }
        }
    }
    let _ = std::fs::write(&path, std::process::id().to_string());
    Ok(())
}

fn unlock_session() {
    let path = session_lock_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if s.trim() == std::process::id().to_string() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

struct MatchState {
    cells: Vec<u8>,
    start_cells: Vec<u8>,
    start_turn: char,
    gtype: String,
    turn: char, // '*' / 'O' / ' '
    my_color: Option<char>,
    told_turn: bool,
    my_clock: String,
    my_clock_secs: Option<u64>,
    my_ext: Option<u64>,
    in_overtime: bool,
    ended: String,
    archive: String,
    left_by: String,
    opp_name: String,
    opp_rating: String,
    opp_clock: String,
    opp_secs: Option<u64>,
    opp_ext: Option<u64>,
    players: Vec<PlayerView>,
    moves: std::collections::BTreeMap<u32, String>,
    last_eval: Option<f32>,
    last_eval_exact: bool,
    last_from_book: bool,
    move_evals: std::collections::BTreeMap<u32, (Option<f32>, Option<f32>)>,
    opp_eval: Option<f32>,
    opp_secs_used: Option<f32>,
    eval_parity: Option<u32>,
    watch_eval: Option<f32>,
    watch_best: Option<String>,
    watch_exact: bool,
    watch_hash: u64,
    last_played_hash: u64, // double-move protection
    seen: u64,
    order: u64,
    over: bool,
    result: String,
}

impl MatchState {
    fn eval_series(&self) -> Vec<EvalPoint> {
        let Some(&last_n) = self.moves.keys().next_back() else {
            return Vec::new();
        };
        let mine_parity = match self.eval_parity {
            Some(p) => p,
            None => {
                let Some(mc) = self.my_color else {
                    return Vec::new();
                };
                if self.turn != '*' && self.turn != 'O' {
                    return Vec::new();
                }
                if self.turn != mc {
                    last_n % 2
                } else {
                    (last_n + 1) % 2
                }
            }
        };
        (1..=last_n)
            .map(|n| EvalPoint {
                n,
                mine: (n % 2) == mine_parity,
                eval: self.move_evals.get(&n).and_then(|&(ev, _)| ev),
            })
            .collect()
    }

    fn snapshot(&self) -> MatchState {
        MatchState {
            cells: self.cells.clone(),
            start_cells: self.start_cells.clone(),
            start_turn: self.start_turn,
            gtype: self.gtype.clone(),
            turn: self.turn,
            my_color: self.my_color,
            told_turn: false,
            my_clock: self.my_clock.clone(),
            my_clock_secs: self.my_clock_secs,
            my_ext: self.my_ext,
            in_overtime: self.in_overtime,
            opp_name: self.opp_name.clone(),
            opp_rating: self.opp_rating.clone(),
            opp_clock: self.opp_clock.clone(),
            opp_secs: self.opp_secs,
            opp_ext: self.opp_ext,
            players: self.players.clone(),
            moves: self.moves.clone(),
            last_eval: self.last_eval,
            last_eval_exact: self.last_eval_exact,
            last_from_book: self.last_from_book,
            move_evals: self.move_evals.clone(),
            opp_eval: self.opp_eval,
            opp_secs_used: self.opp_secs_used,
            eval_parity: self.eval_parity,
            watch_eval: self.watch_eval,
            watch_best: self.watch_best.clone(),
            watch_exact: self.watch_exact,
            watch_hash: self.watch_hash,
            last_played_hash: self.last_played_hash,
            seen: self.seen,
            order: self.order,
            over: self.over,
            ended: self.ended.clone(),
            archive: self.archive.clone(),
            left_by: self.left_by.clone(),
            result: self.result.clone(),
        }
    }

    fn new() -> Self {
        MatchState {
            cells: vec![0; 64],
            start_cells: Vec::new(),
            start_turn: ' ',
            gtype: String::new(),
            turn: ' ',
            my_color: None,
            told_turn: false,
            my_clock: String::new(),
            my_clock_secs: None,
            my_ext: None,
            in_overtime: false,
            ended: String::new(),
            archive: String::new(),
            left_by: String::new(),
            opp_name: String::new(),
            opp_rating: String::new(),
            opp_clock: String::new(),
            opp_secs: None,
            opp_ext: None,
            players: Vec::new(),
            moves: Default::default(),
            last_eval: None,
            last_eval_exact: false,
            last_from_book: false,
            move_evals: Default::default(),
            opp_eval: None,
            opp_secs_used: None,
            eval_parity: None,
            watch_eval: None,
            watch_best: None,
            watch_exact: false,
            watch_hash: 0,
            last_played_hash: 0,
            seen: 0,
            order: MATCH_ORDER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            over: false,
            result: String::new(),
        }
    }
    fn start_string(&self) -> String {
        if self.start_cells.len() != 64 {
            return String::new();
        }
        let mut out = String::with_capacity(66);
        for i in 0..64 {
            let (file, rank) = (i % 8, i / 8);
            out.push(match self.start_cells[file * 8 + rank] {
                1 => 'X',
                2 => 'O',
                _ => '-',
            });
        }
        out.push(' ');
        out.push(if self.start_turn == 'O' { 'O' } else { 'X' });
        out
    }

    fn ggf(&self, id: &str, result: Option<&str>) -> String {
        let board = match self.start_string() {
            s if s.len() == 66 => s.replace('X', "*"),
            _ => format!(
                "{}{}{}{}{} *",
                "-".repeat(27),
                "O*",
                "-".repeat(6),
                "*O",
                "-".repeat(27),
            ),
        };
        let mut out = String::from("(;GM[Othello]PC[GGS/os]");
        if !id.is_empty() {
            out.push_str(&format!("ID[{id}]"));
        }
        let find = |c: &str| self.players.iter().find(|p| p.color == c);
        if let Some(b) = find("black") {
            out.push_str(&format!("PB[{}]RB[{}]", b.name, b.rating));
        }
        if let Some(w) = find("white") {
            out.push_str(&format!("PW[{}]RW[{}]", w.name, w.rating));
        }
        if let Some(r) = result {
            out.push_str(&format!("RE[{r}]"));
        }
        out.push_str(&format!("BO[8 {board}]"));
        let mut black = !board.ends_with(" O");
        for mv in self.moves.values() {
            let tag = if black { "B" } else { "W" };
            let m = if mv.eq_ignore_ascii_case("pa") || mv.eq_ignore_ascii_case("pass") {
                "PA".to_string()
            } else {
                mv.to_uppercase()
            };
            out.push_str(&format!("{tag}[{m}]"));
            black = !black;
        }
        out.push_str(";)");
        out
    }

    fn kifu(&self) -> String {
        self.moves
            .values()
            .filter(|m| !m.eq_ignore_ascii_case("pa") && !m.eq_ignore_ascii_case("pass"))
            .map(|m| m.to_lowercase())
            .collect()
    }
}

fn my_stone_diff(score: f32, first_name: &str, login: &str, synchro: bool) -> i32 {
    let v = (if synchro { score * 2.0 } else { score }).round() as i32;
    if first_name == login {
        v
    } else {
        -v
    }
}

fn mirror_hint(matches: &HashMap<String, MatchState>, mid: &str) -> Option<Position> {
    let (base, side) = mid.rsplit_once('.')?;
    let other = format!("{base}.{}", if side == "0" { "1" } else { "0" });
    let me = matches.get(mid)?;
    let you = matches.get(&other)?;
    let n = me.moves.keys().copied().max().unwrap_or(0) + 1;
    if (1..n).any(|i| me.moves.get(&i) != you.moves.get(&i)) {
        return None;
    }
    let mv = you.moves.get(&n)?;
    let b = mv.as_bytes();
    if b.len() != 2 {
        return None;
    }
    let file = b[0].to_ascii_lowercase().wrapping_sub(b'a');
    let rank = b[1].wrapping_sub(b'1');
    Position::from_file_rank(file, rank)
}

struct Request {
    verb: &'static str,
    id: String,
    who: String,
}

fn parse_request(ln: &str, login: &str) -> Option<Request> {
    let rest = ln.strip_prefix("/os: ")?;
    let verb = ["undo", "abort"]
        .into_iter()
        .find(|v| rest.starts_with(&format!("{v} ")))?;
    let mut it = rest[verb.len() + 1..].split_whitespace();
    let id = it.next()?;
    let who = it.next()?;
    if it.next() != Some("is") || who == login || !id.starts_with('.') {
        return None;
    }
    Some(Request {
        verb,
        id: id.to_string(),
        who: who.to_string(),
    })
}

fn parse_clock(s: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    let mut out = [None, None, None];
    for (i, part) in s.split('/').take(3).enumerate() {
        let p = part.trim().split(',').next().unwrap_or("").trim();
        let (p, neg) = match p.strip_prefix('-') {
            Some(rest) => (rest.trim(), true),
            None => (p, false),
        };
        if p.is_empty()
            || !p
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
        {
            continue;
        }
        let mut secs = 0u64;
        let mut ok = true;
        for seg in p.split(':') {
            match seg.parse::<u64>() {
                Ok(v) => secs = secs * 60 + v,
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            out[i] = Some(if neg { 0 } else { secs });
        }
    }
    (out[0], out[1], out[2])
}

pub(crate) const LEARN_DEPTH: u32 = 18;

fn base_id(id: &str) -> String {
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() >= 3 && parts.last().map(|s| s.len() == 1) == Some(true) {
        parts[..parts.len() - 1].join(".")
    } else {
        id.to_string()
    }
}

fn drop_match(matches: &mut HashMap<String, MatchState>, id: &str) -> Vec<MatchState> {
    let keys: Vec<String> = matches
        .keys()
        .filter(|k| k.as_str() == id || base_id(k) == id)
        .cloned()
        .collect();
    keys.iter().filter_map(|k| matches.remove(k)).collect()
}

fn finish_match(
    matches: &mut HashMap<String, MatchState>,
    id: &str,
    result: &str,
    ended: &str,
    left_by: &str,
    archive: &str,
) -> Vec<MatchState> {
    let keys: Vec<String> = matches
        .keys()
        .filter(|k| k.as_str() == id || base_id(k) == id)
        .cloned()
        .collect();
    let mut out = Vec::new();
    for k in keys {
        if let Some(m) = matches.get_mut(&k) {
            m.over = true;
            m.turn = ' ';
            if !archive.is_empty() {
                m.archive = archive.to_string();
            }
            if !result.is_empty() && m.result.is_empty() {
                m.result = result.to_string();
            }
            if !ended.is_empty() {
                m.ended = ended.to_string();
                m.left_by = left_by.to_string();
            }
            out.push(m.snapshot());
        }
    }
    out
}

fn play_arg(mstr: &str, value: f32, took: Option<std::time::Duration>) -> String {
    if mstr == "pa" {
        return mstr.to_string();
    }
    let v = if value.is_finite() { value } else { 0.0 };
    match took {
        Some(t) => format!("{mstr}/{v:.2}/{:.2}", t.as_secs_f64()),
        None => format!("{mstr}/{v:.2}"),
    }
}

fn coord(p: Position) -> String {
    let f = (b'A' + p.index() / 8) as char;
    let r = (b'1' + p.index() % 8) as char;
    format!("{f}{r}")
}

struct EngineWorker {
    tx: std::sync::mpsc::Sender<Job>,
    rx: std::sync::mpsc::Receiver<Done>,
    stop: kuroobi::midgame::StopHandle,
    busy: bool,
    mid: Option<String>,
    pending_hash: u64,
    pending: Option<Pending>,
    pondering: bool,
    sent_at: Option<(Instant, Duration)>,
    stopped_at: Option<Instant>,
    progress: std::sync::Arc<kuroobi::engine::Progress>,
}

const WORKER_GIVE_UP: Duration = Duration::from_secs(10);

struct Pending {
    board: Board,
    levels: (u32, u8, u8),
    cap: Option<Duration>,
    hash: u64,
    hint: Option<Position>,
}

enum Job {
    Think {
        board: Board,
        levels: (u32, u8, u8),
        cap: Option<Duration>,
        hint: Option<Position>,
    },
    Ponder {
        board: Board,
        slice: Duration,
    },
    SetUseBook(bool),
    SetThreads(usize),
}

enum Done {
    Moved(Box<kuroobi::engine::MoveEval>),
    Pondered,
}

struct Ctx {
    app: tauri::AppHandle,
    stop: Option<kuroobi::midgame::StopHandle>,
    snap: Arc<Mutex<Snapshot>>,
    engine: Option<Engine>,
    engine_cfg: EngineConfig,
    seq: u64,
    last_emit: Instant,
    dirty: bool,
    auto_watch: Vec<String>,
    rated_by_base: HashMap<String, bool>,
    learn_jobs: VecDeque<(String, kuroobi::learn::BackupJob, crate::LearnEntry)>,
    local_stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    local_activity: Arc<Mutex<crate::Activity>>,
    pending_watch: HashMap<String, Instant>,
    engine_cfg_pace: String,
    engine_cfg_max_move: u64,
    engine_cfg_reserve: u64,
    engine_cfg_budget_use: f64,
    engine_cfg_ponder: bool,
    ponder_at: Option<String>,
    workers: Vec<EngineWorker>,
    worker_threads: usize,
}

const MAX_WORKERS: usize = 2;

impl Ctx {
    fn worker_for(&mut self, mid: &str) -> Option<usize> {
        if let Some(i) = self
            .workers
            .iter()
            .position(|w| w.mid.as_deref() == Some(mid))
        {
            return Some(i);
        }
        if let Some(i) = self.workers.iter().position(|w| w.mid.is_none()) {
            self.workers[i].mid = Some(mid.to_string());
            return Some(i);
        }
        if self.workers.len() < MAX_WORKERS {
            match EngineWorker::spawn(self.engine_cfg.clone()) {
                Ok(mut w) => {
                    w.mid = Some(mid.to_string());
                    self.workers.push(w);
                    self.share_threads();
                    return Some(self.workers.len() - 1);
                }
                Err(e) => {
                    self.log("info", &format!("cannot spawn search worker: {e}"));
                    return None;
                }
            }
        }
        if let Some(i) = self
            .workers
            .iter()
            .position(|w| !w.busy && w.pending.is_none())
        {
            self.workers[i].mid = Some(mid.to_string());
            return Some(i);
        }
        None
    }

    fn share_threads(&mut self) {
        let total = resolve_threads(self.engine_cfg.threads);
        let active = self
            .workers
            .iter()
            .filter(|w| w.mid.is_some())
            .count()
            .max(1);
        let each = (total / active).max(1);
        if each == self.worker_threads {
            return;
        }
        for w in &mut self.workers {
            w.send(Job::SetThreads(each));
        }
        self.worker_threads = each;
    }

    fn set_use_book(&mut self, b: bool) {
        self.engine_cfg.use_book = b;
        if let Some(e) = self.engine.as_mut() {
            e.set_use_book(b);
        }
        for w in &mut self.workers {
            w.send(Job::SetUseBook(b));
        }
        self.snap.lock().unwrap().engine.use_book = b;
    }

    fn release_worker(&mut self, mid: &str) {
        let mut hit = false;
        for w in &mut self.workers {
            if w.mid.as_deref() == Some(mid) {
                w.mid = None;
                w.pending = None;
                if w.busy {
                    w.stop.stop();
                }
                w.sent_at = None;
                w.stopped_at = None;
                w.pending_hash = 0;
                hit = true;
            }
        }
        if hit {
            self.share_threads();
        }
    }
}

impl EngineWorker {
    fn spawn(mut cfg: EngineConfig) -> Result<EngineWorker, String> {
        let res = resources();
        cfg.threads = resolve_threads(cfg.threads);
        cfg.weights = res.weights_path();
        cfg.nnue = res.nnue_path();
        cfg.book = res.book_path();
        cfg.midgame_hash_bits = res.hash_mid_bits();
        cfg.solver_hash_bits = res.hash_end_bits();
        let engine = Engine::new(cfg)?;
        let stop = engine.stop_handle();
        let progress = engine.progress();
        let (jtx, jrx) = std::sync::mpsc::channel::<Job>();
        let (dtx, drx) = std::sync::mpsc::channel::<Done>();
        std::thread::spawn(move || {
            let mut engine = engine;
            while let Ok(job) = jrx.recv() {
                match job {
                    Job::Think {
                        board,
                        levels,
                        cap,
                        hint,
                    } => {
                        let base = {
                            let c = engine.config();
                            (c.depth, c.solve_empties, c.band)
                        };
                        engine.set_levels(levels.0, levels.1, levels.2);
                        if let Some(h) = hint {
                            engine.hint_move(&board, h);
                        }
                        let dl = cap.map(|c| Instant::now() + c);
                        let nf0 = kuroobi::engine::NON_FINITE_VALUES
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let mv = engine.choose_within(&board, dl);
                        let nf1 = kuroobi::engine::NON_FINITE_VALUES
                            .load(std::sync::atomic::Ordering::Relaxed);
                        if nf1 != nf0 {
                            eprintln!(
                                "!! search returned a non-finite value (empties {} depth {} cut {})",
                                board.empty_count(),
                                mv.depth,
                                mv.cut
                            );
                        }
                        engine.set_levels(base.0, base.1, base.2);
                        engine.progress().clear();
                        if dtx.send(Done::Moved(Box::new(mv))).is_err() {
                            return;
                        }
                    }
                    Job::Ponder { board, slice } => {
                        engine.ponder(&board, Instant::now() + slice);
                        engine.progress().clear();
                        if dtx.send(Done::Pondered).is_err() {
                            return;
                        }
                    }
                    Job::SetUseBook(b) => engine.set_use_book(b),
                    Job::SetThreads(n) => engine.set_threads(n),
                }
            }
        });
        Ok(EngineWorker {
            tx: jtx,
            rx: drx,
            stop,
            busy: false,
            mid: None,
            pending_hash: 0,
            pending: None,
            pondering: false,
            sent_at: None,
            stopped_at: None,
            progress,
        })
    }

    fn send(&mut self, job: Job) {
        let counts = matches!(job, Job::Think { .. } | Job::Ponder { .. });
        if self.tx.send(job).is_ok() && counts {
            self.busy = true;
        }
    }
}

impl Ctx {
    fn notify(&self, title: &str, body: &str) {
        use tauri_plugin_notification::NotificationExt;
        let _ = self
            .app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }
    fn log(&mut self, dir: &str, text: &str) {
        {
            use std::io::Write as _;
            let p = std::env::var("KUROOBI_GGS_WIRE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| wire_path());
            roll_if_large(&p);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&p)
            {
                let _ = writeln!(f, "{dir} {text}");
            }
        }
        let mut s = self.snap.lock().unwrap();
        s.log.push_back(LogLine {
            dir: dir.into(),
            text: text.into(),
        });
        while s.log.len() > 600 {
            s.log.pop_front();
        }
        drop(s);
        self.dirty = true;
    }
    fn emit(&mut self, force: bool) {
        if !self.dirty && !force {
            return;
        }
        if !force && self.last_emit.elapsed() < Duration::from_millis(120) {
            return;
        }
        let s = self.snap.lock().unwrap().clone();
        let _emit_ok = self.app.emit("ggs", &s).is_ok();
        #[cfg(debug_assertions)]
        {
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/ggs_session_state.log")
            {
                let detail = s
                    .matches
                    .iter()
                    .map(|m| {
                        format!(
                            " [{} {}discs start {}discs turn={} me={} eval={}]",
                            m.id,
                            m.cells.iter().filter(|&&c| c != 0).count(),
                            m.ggf
                                .split_once("BO[8 ")
                                .map(|(_, r)| { r.chars().take(64).filter(|c| *c != '-').count() })
                                .unwrap_or(0),
                            if m.turn.is_empty() { "-" } else { &m.turn },
                            if m.my_color.is_empty() {
                                "observe"
                            } else {
                                &m.my_color
                            },
                            m.watch_eval
                                .map(|v| format!("{v:+.1}"))
                                .unwrap_or_else(|| "-".into()),
                        )
                    })
                    .collect::<String>();
                let fetched = match &s.fetched_ggf {
                    Some(g) if !g.ggf.is_empty() => {
                        format!(" fetched={} {}chars", g.id, g.ggf.len())
                    }
                    Some(g) => format!(" fetched={} error:{}", g.id, g.error),
                    None => String::new(),
                };
                let _ = writeln!(
                    f,
                    "conn={} login={} matches={} offers={} log={} emit={}{}{fetched}",
                    s.conn,
                    s.login,
                    s.matches.len(),
                    s.offers.len(),
                    s.log.len(),
                    if _emit_ok { "ok" } else { "ERR" },
                    detail
                );
            }
        }
        self.last_emit = Instant::now();
        self.dirty = false;
    }
    fn ensure_engine(&mut self) -> Result<(), String> {
        if self.engine.is_none() {
            let res = resources();
            let mut cfg = self.engine_cfg.clone();
            cfg.threads = resolve_threads(cfg.threads);
            cfg.weights = res.weights_path();
            cfg.nnue = res.nnue_path();
            cfg.book = res.book_path();
            let engine = Engine::new(cfg)?;
            self.stop = Some(engine.stop_handle());
            let loaded = engine.has_book();
            self.engine = Some(engine);
            {
                let mut s = self.snap.lock().unwrap();
                s.engine.ready = true;
                s.engine.book_loaded = loaded;
            }
            self.dirty = true;
        }
        Ok(())
    }
}

struct Wire {
    raw: Vec<u8>,
    lines: VecDeque<String>,
    logged_in: bool,
    login_started: Instant,
    login_warned: bool,
    lost: bool,
    in_block: bool,
    block: Vec<String>,
    matches: HashMap<String, MatchState>,
    pending: Vec<String>,
    next_match_at: Instant,
    capture: Option<(String, Vec<String>)>,
    next_ask_at: Option<Instant>,
    want_quit: bool,
    had_own_match: bool,
}

impl Wire {
    fn new() -> Self {
        Wire {
            raw: Vec::new(),
            lines: VecDeque::new(),
            logged_in: false,
            login_started: Instant::now(),
            login_warned: false,
            lost: false,
            in_block: false,
            block: Vec::new(),
            matches: HashMap::new(),
            pending: Vec::new(),
            next_match_at: Instant::now() + Duration::from_secs(60),
            capture: None,
            next_ask_at: None,
            want_quit: false,
            had_own_match: false,
        }
    }
}

fn say(ctx: &mut Ctx, send: &mut impl FnMut(String), cmd: impl Into<String>) {
    let c: String = cmd.into();
    ctx.log("out", &c);
    send(c);
}

fn new_ctx(
    app: tauri::AppHandle,
    snap: Arc<Mutex<Snapshot>>,
    local_stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    local_activity: Arc<Mutex<crate::Activity>>,
) -> Ctx {
    let ctx = Ctx {
        app,
        stop: None,
        snap,
        engine: None,
        engine_cfg: EngineConfig {
            depth: 22,
            solve_empties: 26,
            band: 6,
            threads: resources().threads.unwrap_or_else(|| resolve_threads(0)),
            midgame_hash_bits: resources().hash_mid_bits(),
            solver_hash_bits: resources().hash_end_bits(),
            ..Default::default()
        },
        seq: 0,
        last_emit: Instant::now(),
        dirty: true,
        auto_watch: Vec::new(),
        rated_by_base: HashMap::new(),
        learn_jobs: VecDeque::new(),
        local_stop,
        local_activity,
        pending_watch: HashMap::new(),
        engine_cfg_pace: "fast".into(),
        engine_cfg_max_move: 0,
        engine_cfg_reserve: 20,
        engine_cfg_budget_use: 2.5,
        engine_cfg_ponder: true,
        ponder_at: None,
        workers: Vec::new(),
        worker_threads: resolve_threads(0),
    };
    ctx.snap.lock().unwrap().engine = EngineCfgView {
        budget_use: 2.5,
        depth: 22,
        solve: 26,
        band: 6,
        threads: ctx.engine_cfg.threads,
        ready: false,
        use_book: ctx.engine_cfg.use_book,
        book_loaded: resources().book_path().exists(),
        learn: true,
        ponder: ctx.engine_cfg_ponder,
        pace: ctx.engine_cfg_pace.clone(),
        max_move_secs: ctx.engine_cfg_max_move,
        reserve_secs: ctx.engine_cfg_reserve,
    };
    ctx
}

fn restore_settings(ctx: &mut Ctx) {
    let Some(v) = load_settings() else { return };
    apply_engine_cfg(ctx, v.depth, v.solve, v.band);
    ctx.engine_cfg_ponder = v.ponder;
    ctx.engine_cfg_pace = v.pace.clone();
    ctx.engine_cfg_max_move = v.max_move_secs;
    ctx.engine_cfg_reserve = v.reserve_secs;
    apply_pacing(
        ctx,
        v.pace.clone(),
        v.max_move_secs,
        v.reserve_secs,
        v.budget_use,
    );
    let mut s = ctx.snap.lock().unwrap();
    s.engine.depth = v.depth;
    s.engine.solve = v.solve;
    s.engine.band = v.band;
    s.engine.ponder = v.ponder;
    s.engine.pace = v.pace;
    s.engine.max_move_secs = v.max_move_secs;
    s.engine.reserve_secs = v.reserve_secs;
    s.engine.budget_use = v.budget_use;
    s.engine.use_book = v.use_book;
    s.engine.learn = v.learn;
    s.auto_play = v.auto_play;
    s.watch_analysis = v.watch_analysis;
}

pub fn run(
    app: tauri::AppHandle,
    rx: Receiver<Cmd>,
    snap: Arc<Mutex<Snapshot>>,
    local_stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    local_activity: Arc<Mutex<crate::Activity>>,
) {
    let mut ctx = new_ctx(app, snap, local_stop, local_activity);
    restore_settings(&mut ctx);

    loop {
        ctx.emit(true);
        let Some((login, pw)) = wait_for_connect(&mut ctx, &rx) else {
            return;
        };
        if !claim_session(&mut ctx) {
            continue;
        }
        restore_chat(&mut ctx, &login);
        session(&mut ctx, &rx, &login, &pw);
        unlock_session();
        {
            let mut s = ctx.snap.lock().unwrap();
            s.conn = "disconnected".into();
            s.offers.clear();
            s.matches.clear();
            s.thinking = None;
        }
        ctx.emit(true);
    }
}

fn wait_for_connect(ctx: &mut Ctx, rx: &Receiver<Cmd>) -> Option<(String, String)> {
    loop {
        match rx.recv_timeout(Duration::from_millis(300)) {
            Ok(Cmd::Connect { login, pw }) => return Some((login, pw)),
            Ok(Cmd::SetEngine {
                depth,
                solve,
                band,
                ponder,
            }) => {
                apply_engine_cfg(ctx, depth, solve, band);
                ctx.engine_cfg_ponder = ponder;
                ctx.snap.lock().unwrap().engine.ponder = ponder;
                save_settings(ctx);
            }
            Ok(Cmd::ReloadThreads) => apply_threads(ctx),
            Ok(Cmd::SetStandby(cfg)) => ctx.snap.lock().unwrap().standby = cfg,
            Ok(Cmd::SetPacing {
                pace,
                max_move_secs,
                reserve_secs,
                budget_use,
            }) => {
                apply_pacing(ctx, pace, max_move_secs, reserve_secs, budget_use);
                save_settings(ctx);
            }
            Ok(Cmd::SetAutoPlay(b)) => {
                ctx.snap.lock().unwrap().auto_play = b;
                save_settings(ctx);
            }
            Ok(Cmd::SetUseBook(b)) => {
                ctx.set_use_book(b);
                save_settings(ctx);
            }
            Ok(Cmd::SetWatchAnalysis(b)) => {
                ctx.snap.lock().unwrap().watch_analysis = b;
                save_settings(ctx);
            }
            Ok(Cmd::SetLearn(b)) => {
                ctx.snap.lock().unwrap().engine.learn = b;
                save_settings(ctx);
            }
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                learn_tick(ctx);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
        ctx.emit(true);
    }
}

fn claim_session(ctx: &mut Ctx) -> bool {
    let Err(pid) = try_lock_session() else {
        return true;
    };
    ctx.log(
        "info",
        &format!(
            "another window (PID {pid}) is connected to GGS; \
             use it, or disconnect there first"
        ),
    );
    ctx.notify(
        &crate::i18n::t("backend.notify.connect_blocked_title"),
        &crate::i18n::t("backend.notify.connect_blocked_body"),
    );
    ctx.snap.lock().unwrap().conn = "disconnected".into();
    ctx.emit(true);
    false
}

fn restore_chat(ctx: &mut Ctx, login: &str) {
    let mut s = ctx.snap.lock().unwrap();
    if s.chat.is_empty() {
        let mut past = load_chat(login);
        if past.len() > 300 {
            past.drain(..past.len() - 300);
        }
        s.chat.extend(past);
    }
    s.chat_seen = load_chat_seen(login);
}

fn dial(ctx: &mut Ctx) -> Option<(TcpStream, TcpStream)> {
    match TcpStream::connect(("skatgame.net", 5000)) {
        Ok(stream) => {
            stream
                .set_read_timeout(Some(Duration::from_millis(250)))
                .ok();
            let writer = stream.try_clone().expect("clone");
            Some((stream, writer))
        }
        Err(e) => {
            ctx.log("info", &format!("connect failed: {e} — retrying in 15s"));
            ctx.emit(true);
            std::thread::sleep(Duration::from_secs(15));
            None
        }
    }
}

fn session(ctx: &mut Ctx, rx: &Receiver<Cmd>, login: &str, pw: &str) {
    let mut login_fails = 0u32;
    let mut cred_saved = false;
    loop {
        {
            let mut s = ctx.snap.lock().unwrap();
            s.conn = "connecting".into();
            s.login = login.to_string();
        }
        ctx.emit(true);
        let Some((mut stream, mut writer)) = dial(ctx) else {
            continue;
        };
        ctx.snap.lock().unwrap().conn = "logging_in".into();
        ctx.emit(true);

        let mut w = Wire::new();
        loop {
            while let Ok(cmd) = rx.try_recv() {
                if handle_cmd(
                    ctx,
                    cmd,
                    login,
                    &mut w.matches,
                    &mut w.pending,
                    &mut w.next_ask_at,
                    |c| ctx_send(&mut writer, &c),
                ) {
                    w.want_quit = true;
                }
            }
            if w.want_quit {
                return;
            }

            read_wire(&mut stream, &mut w);
            if w.lost {
                if on_lost(ctx, &mut login_fails, w.logged_in) {
                    return;
                }
                break;
            }
            if !w.logged_in {
                login_step(ctx, &mut w, login, pw, |c| ctx_send(&mut writer, &c));
                continue;
            }
            while let Some(ln) = w.lines.pop_front() {
                dispatch_line(ctx, &mut w, &ln, login, pw, &mut cred_saved, |c| {
                    ctx_send(&mut writer, &c)
                });
            }
            standby_tick(ctx, &mut w, |c| ctx_send(&mut writer, &c));
            service(ctx, &mut w, |c| ctx_send(&mut writer, &c));
            ctx.emit(false);
        }
    }
}

fn read_wire(stream: &mut TcpStream, w: &mut Wire) {
    let mut chunk = [0u8; 8192];
    match stream.read(&mut chunk) {
        Ok(0) => w.lost = true,
        Ok(n) => w.raw.extend_from_slice(&chunk[..n]),
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => {}
        Err(_) => w.lost = true,
    }
    if w.lost {
        return;
    }
    while let Some(nl) = w.raw.iter().position(|&b| b == b'\n') {
        let mut line: Vec<u8> = w.raw.drain(..=nl).collect();
        while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
            line.pop();
        }
        w.lines
            .push_back(String::from_utf8_lossy(&line).into_owned());
    }
}

/// True to give up on the account (reconnect is then a user action).
fn on_lost(ctx: &mut Ctx, login_fails: &mut u32, logged_in: bool) -> bool {
    if !logged_in {
        *login_fails += 1;
        ctx.log(
            "info",
            "dropped by the server while logging in; check whether \
             another process is connected with the same account \
             (GGS rejects duplicate logins)",
        );
        ctx.notify(
            &crate::i18n::t("backend.notify.login_failed_title"),
            &crate::i18n::t("backend.notify.duplicate_login_body"),
        );
        if *login_fails >= 2 {
            ctx.snap.lock().unwrap().conn = "disconnected".into();
            ctx.emit(true);
            return true;
        }
        ctx.emit(true);
        std::thread::sleep(Duration::from_secs(3));
        return false;
    }
    ctx.notify(
        &crate::i18n::t("backend.notify.disconnected_title"),
        &crate::i18n::t("backend.notify.disconnected_body"),
    );
    ctx.log(
        "info",
        "disconnected — reconnecting in 10s and resuming games",
    );
    ctx.emit(true);
    std::thread::sleep(Duration::from_secs(10));
    false
}

fn login_step(ctx: &mut Ctx, w: &mut Wire, login: &str, pw: &str, mut send: impl FnMut(String)) {
    if !w.login_warned && w.login_started.elapsed() > Duration::from_secs(20) {
        w.login_warned = true;
        ctx.log(
            "info",
            "login is not progressing; check whether another process \
             is connected with the same account (GGS rejects \
             duplicate logins)",
        );
        ctx.notify(
            &crate::i18n::t("backend.notify.login_stalled_title"),
            &crate::i18n::t("backend.notify.duplicate_login_body"),
        );
        ctx.emit(true);
    }
    let tail = String::from_utf8_lossy(&w.raw).to_lowercase();
    let tail2 = w
        .lines
        .iter()
        .rev()
        .take(3)
        .map(|l| l.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    while let Some(l) = w.lines.pop_front() {
        ctx.log("in", &l);
    }
    if tail.contains("enter login") || tail2.contains("enter login") {
        say(ctx, &mut send, login);
        w.raw.clear();
    } else if tail.contains("password") || tail2.contains("password") {
        ctx.log("out", "********");
        send(pw.to_string());
        w.raw.clear();
        w.logged_in = true;
        ctx.snap.lock().unwrap().conn = "online".into();
        greet(ctx, w, login, &mut send);
        ctx.emit(true);
    }
    ctx.emit(false);
}

fn greet(ctx: &mut Ctx, w: &mut Wire, login: &str, send: &mut impl FnMut(String)) {
    for c in [
        "verbose -news -faq -help -ack",
        "tell /os client -",
        "tell /os trust +",
        if no_rated() {
            "tell /os rated -"
        } else {
            "tell /os rated +"
        },
        "tell /os notify +",
        "tell /os open 1",
        "chann + .chat",
    ] {
        say(ctx, send, c);
    }
    for t in ["8", "8r"] {
        w.pending.push(format!("who:{t}"));
        say(ctx, send, format!("tell /os who {t}"));
    }
    for t in ["8", "8r"] {
        w.pending.push(format!("rank:{t}:{login}"));
        say(ctx, send, format!("tell /os rank {t} {login}"));
    }
    for (key, cmd) in [
        ("stored_list", "tell /os stored"),
        ("match_list", "tell /os match"),
        ("history:", "tell /os history"),
        ("stored_list", "tell /os stored"),
    ] {
        w.pending.push(key.into());
        say(ctx, send, cmd);
    }
}

fn dispatch_line(
    ctx: &mut Ctx,
    w: &mut Wire,
    ln: &str,
    login: &str,
    pw: &str,
    cred_saved: &mut bool,
    mut send: impl FnMut(String),
) {
    ctx.log("in", ln);

    if !*cred_saved && (ln == "READY" || ln.starts_with("/os")) {
        *cred_saved = true;
        crate::keychain::save(login, pw);
    }

    if let Some(msg) = parse_chat(ln) {
        push_chat(ctx, login, msg);
        return;
    }
    if ln.starts_with("/os: update") || ln.starts_with("/os: join") {
        w.in_block = true;
        w.block.clear();
        w.block.push(ln.to_string());
        return;
    }
    if w.in_block {
        if ln != "READY" {
            w.block.push(ln.to_string());
            return;
        }
        w.in_block = false;
        if let Some(mid) = w
            .block
            .first()
            .and_then(|l| l.split_whitespace().nth(2))
            .map(str::to_string)
        {
            if ctx.pending_watch.remove(&mid).is_some()
                | ctx.pending_watch.remove(&base_id(&mid)).is_some()
            {
                ctx.snap.lock().unwrap().notice.clear();
            }
        }
        handle_block(ctx, &w.block, login, &mut w.matches, &mut send);
        return;
    }

    for id in std::mem::take(&mut ctx.auto_watch) {
        say(ctx, &mut send, format!("tell /os watch + {id}"));
    }

    if let Some((kind, buf)) = w.capture.as_mut() {
        if ln == "READY" {
            let (kind, buf) = (std::mem::take(kind), std::mem::take(buf));
            w.capture = None;
            finish_capture(ctx, &kind, &buf, login);
        } else {
            buf.push(ln.to_string());
        }
    } else if let Some(pos) = w.pending.iter().position(|k| capture_header_matches(k, ln)) {
        let kind = w.pending.remove(pos);
        w.capture = Some((kind, vec![ln.to_string()]));
    }

    handle_os_line(
        ctx,
        ln,
        login,
        &mut w.matches,
        &mut w.pending,
        &mut w.next_ask_at,
        &mut send,
    );
}

fn standby_tick(ctx: &mut Ctx, w: &mut Wire, mut send: impl FnMut(String)) {
    match w.next_ask_at {
        Some(t) if Instant::now() >= t => w.next_ask_at = None,
        _ => return,
    }
    let s = ctx.snap.lock().unwrap();
    let sb = s.standby.clone();
    let in_match = s.matches.iter().any(|m| !m.over);
    let games = s.standby_stats.games;
    let outgoing = s.offers.iter().any(|o| !o.incoming);
    drop(s);

    let more = sb.max_games == 0 || games < sb.max_games;
    if !sb.enabled || sb.opponent.is_empty() || !more {
        return;
    }
    w.next_ask_at = Some(Instant::now() + Duration::from_secs(sb.interval_secs.max(30)));
    if in_match || outgoing {
        return;
    }
    ctx.log("info", &format!("waiting mode: asking {}", sb.opponent));
    let rated = if sb.rated && !no_rated() { "+" } else { "-" };
    say(ctx, &mut send, format!("tell /os rated {rated}"));
    say(
        ctx,
        &mut send,
        format!("tell /os ask {} {} {}", sb.gtype, sb.time, sb.opponent),
    );
}

fn service(ctx: &mut Ctx, w: &mut Wire, mut send: impl FnMut(String)) {
    if Instant::now() >= w.next_match_at {
        w.next_match_at = Instant::now() + Duration::from_secs(60);
        w.pending.push("match_list".into());
        say(ctx, &mut send, "tell /os match");
    }

    let done: Vec<String> = ctx
        .workers
        .iter()
        .filter_map(|wk| wk.mid.clone())
        .filter(|mid| w.matches.get(mid).is_none_or(|m| m.over))
        .collect();
    for mid in done {
        ctx.release_worker(&mid);
    }

    let stalled: Vec<String> = w
        .matches
        .iter()
        .filter(|(_, m)| !m.over && m.my_color.is_some() && Some(m.turn) == m.my_color)
        .map(|(k, _)| k.clone())
        .collect();
    let mut out: Vec<String> = Vec::new();
    for mid in stalled {
        think_and_play(ctx, &mid, &mut w.matches, |s| out.push(s));
    }
    out.extend(collect_workers(ctx, &mut w.matches));
    for line in out {
        say(ctx, &mut send, line);
    }
    sync_matches(ctx, &w.matches);

    if ctx.workers.is_empty() {
        ponder_slice(ctx, &w.matches);
    }
    expire_watches(ctx);

    let own_match = w.matches.values().any(|m| m.my_color.is_some());
    if own_match && !w.had_own_match && ctx.local_activity.lock().unwrap().local.is_some() {
        if let Some(h) = ctx.local_stop.lock().unwrap().as_ref() {
            h.stop();
        }
        ctx.notify(
            &crate::i18n::t("backend.notify.game_start_title"),
            &crate::i18n::t("backend.notify.local_search_stopped_body"),
        );
        ctx.log("info", "GGS game started — stopped the local search");
    }
    w.had_own_match = own_match;

    if !own_match {
        learn_tick(ctx);
    }
}

fn expire_watches(ctx: &mut Ctx) {
    let now = Instant::now();
    let stale: Vec<String> = ctx
        .pending_watch
        .iter()
        .filter(|(_, &due)| now >= due)
        .map(|(id, _)| id.clone())
        .collect();
    if stale.is_empty() {
        return;
    }
    for id in &stale {
        ctx.pending_watch.remove(id);
        ctx.log("info", &format!("cannot observe {id} (the game has ended)"));
    }
    let mut s = ctx.snap.lock().unwrap();
    s.ongoing.retain(|o| !stale.contains(&o.id));
    s.notice = format!("err.observe_failed|ids={}", stale.join(" / "));
    drop(s);
    ctx.emit(true);
}

fn handle_cmd(
    ctx: &mut Ctx,
    cmd: Cmd,
    login: &str,
    matches: &mut HashMap<String, MatchState>,
    pending: &mut Vec<String>,
    next_ask_at: &mut Option<Instant>,
    mut send: impl FnMut(String),
) -> bool {
    let mut quit = false;
    match cmd {
        Cmd::Connect { .. } => {}
        Cmd::Disconnect => {
            if let Some(h) = &ctx.stop {
                h.stop(); // abort any running think too
            }
            send("quit".to_string());
            quit = true;
        }
        Cmd::Raw(c) => send(c),
        Cmd::ChatSeen(at) => {
            save_chat_seen(login, at);
            let mut s = ctx.snap.lock().unwrap();
            if at > s.chat_seen {
                s.chat_seen = at;
            }
            ctx.dirty = true;
        }
        Cmd::Ask {
            gtype,
            time,
            opponent,
            rated,
        } => {
            send(format!(
                "tell /os rated {}",
                if rated && !no_rated() { "+" } else { "-" }
            ));
            let cmd = if opponent.is_empty() {
                format!("tell /os ask {gtype} {time}")
            } else {
                format!("tell /os ask {gtype} {time} {opponent}")
            };
            send(cmd)
        }
        Cmd::Accept(id) => send(format!("tell /os accept {id}")),
        Cmd::Decline(id) => send(format!("tell /os decline {id}")),
        Cmd::Finger(name) => {
            pending.push(format!("finger:{name}"));
            send(format!("finger {name}"));
            pending.push(format!("osfinger:{name}"));
            send(format!("tell /os finger {name}"));
        }
        Cmd::Who => {
            for t in ["8", "8r"] {
                pending.push(format!("who:{t}"));
                send(format!("tell /os who {t}"));
            }
        }
        Cmd::Top { gtype, n } => {
            pending.push("top".into());
            send(format!("tell /os top {gtype} {n}"));
        }
        Cmd::Rank { gtype, name } => {
            pending.push(format!("rank:{gtype}:{name}"));
            send(format!("tell /os rank {gtype} {name}"));
        }
        Cmd::Look(id) => {
            pending.push(format!("look:{id}"));
            send(format!("tell /os look {id}"));
        }
        Cmd::CloseMatch(id) => {
            let keys: Vec<String> = matches
                .iter()
                .filter(|(k, m)| m.over && (k.as_str() == id || base_id(k) == id))
                .map(|(k, _)| k.clone())
                .collect();
            for k in keys {
                matches.remove(&k);
                ctx.release_worker(&k);
            }
            sync_matches(ctx, matches);
            ctx.emit(true);
        }
        Cmd::Watch(id) => {
            send(format!("tell /os watch + {id}"));
            ctx.pending_watch
                .insert(id.clone(), Instant::now() + Duration::from_secs(6));
            let mut s = ctx.snap.lock().unwrap();
            for o in s.ongoing.iter_mut() {
                if o.id == id {
                    o.watching = true;
                }
            }
            drop(s);
            ctx.dirty = true;
        }
        Cmd::Unwatch(id) => {
            send(format!("tell /os watch - {id}"));
            let mut s = ctx.snap.lock().unwrap();
            for o in s.ongoing.iter_mut() {
                if o.id == id {
                    o.watching = false;
                }
            }
            s.matches.retain(|m| m.id != id && base_id(&m.id) != id);
            drop(s);
            drop_match(matches, &id);
            ctx.release_worker(&id);
            ctx.dirty = true;
        }
        Cmd::Chat { target, text } => {
            send(format!("tell {target} {text}"));
            let chan = if target.starts_with('.') {
                target.clone()
            } else {
                format!("→{target}")
            };
            push_chat(
                ctx,
                login,
                ChatMsg {
                    chan,
                    from: login.to_string(),
                    text,
                    at: now_secs(),
                    thread: target,
                },
            );
        }
        Cmd::SetEngine {
            depth,
            solve,
            band,
            ponder,
        } => {
            apply_engine_cfg(ctx, depth, solve, band);
            ctx.engine_cfg_ponder = ponder;
            ctx.snap.lock().unwrap().engine.ponder = ponder;
            save_settings(ctx);
            ctx.dirty = true;
        }
        Cmd::ReloadThreads => {
            apply_threads(ctx);
            ctx.dirty = true;
        }
        Cmd::SetPacing {
            pace,
            max_move_secs,
            reserve_secs,
            budget_use,
        } => {
            apply_pacing(ctx, pace, max_move_secs, reserve_secs, budget_use);
            save_settings(ctx);
        }
        Cmd::SetAutoPlay(b) => {
            ctx.snap.lock().unwrap().auto_play = b;
            save_settings(ctx);
            ctx.dirty = true;
        }
        Cmd::SetUseBook(b) => {
            ctx.set_use_book(b);
            save_settings(ctx);
            ctx.dirty = true;
        }
        Cmd::SetWatchAnalysis(b) => {
            ctx.snap.lock().unwrap().watch_analysis = b;
            save_settings(ctx);
            ctx.dirty = true;
        }
        Cmd::SetLearn(b) => {
            ctx.snap.lock().unwrap().engine.learn = b;
            save_settings(ctx);
            ctx.dirty = true;
        }
        Cmd::ListStored => {
            pending.push("stored_list".into());
            send("tell /os stored".to_string());
        }
        Cmd::ListMatches => {
            pending.push("match_list".into());
            send("tell /os match".to_string());
        }
        Cmd::ResumeStored(id) => {
            ctx.log("info", &format!("resuming adjourned game {id}"));
            send(format!("tell /os ask {id}"));
        }
        Cmd::History(name) => {
            pending.push(format!("history:{name}"));
            if name.is_empty() {
                send("tell /os history".to_string());
            } else {
                send(format!("tell /os history {name}"));
            }
        }
        Cmd::SetFormula { kind, expr } => {
            send(format!("tell /os {kind} {expr}"));
            ctx.log("info", &format!("set {kind}: {expr}"));
        }
        Cmd::MatchCmd { id, verb, arg } => {
            if arg.is_empty() {
                send(format!("tell /os {verb} {id}"));
            } else {
                send(format!("tell /os {verb} {id} {arg}"));
            }
        }
        Cmd::SetStandby(cfg) => {
            let mut s = ctx.snap.lock().unwrap();
            let was = s.standby.enabled;
            s.standby = cfg;
            if !was && s.standby.enabled {
                s.standby_stats = Default::default();
            }
            let take = if s.standby.enabled && s.standby.auto_accept {
                let busy = s.matches.iter().any(|m| !m.over);
                if busy {
                    None
                } else {
                    s.offers.iter().find(|o| o.incoming).map(|o| o.id.clone())
                }
            } else {
                None
            };
            drop(s);
            if let Some(id) = take {
                ctx.log(
                    "info",
                    &format!("waiting mode: accepting pending offer {id}"),
                );
                send(format!("tell /os accept {id}"));
            }
            *next_ask_at = Some(Instant::now() + Duration::from_secs(3));
            ctx.dirty = true;
        }
    }
    quit
}

fn handle_os_line(
    ctx: &mut Ctx,
    ln: &str,
    login: &str,
    matches: &mut HashMap<String, MatchState>,
    pending: &mut Vec<String>,
    next_ask_at: &mut Option<Instant>,
    mut send: impl FnMut(String),
) {
    if let Some(rest) = ln.strip_prefix("/os: + ") {
        let rest = rest.trim_start();
        if let Some(mrest) = rest.strip_prefix("match ") {
            let id = mrest.split_whitespace().next().unwrap_or("").to_string();
            if !id.is_empty() {
                if let Some(f) = mrest.split_whitespace().last() {
                    if f == "R" || f == "U" {
                        ctx.rated_by_base.insert(base_id(&id), f == "R");
                    }
                }
                let mine = mrest.contains(login);
                if mine {
                    ctx.notify(&crate::i18n::t("backend.notify.game_start_title"), mrest);
                }
                let mut s = ctx.snap.lock().unwrap();
                s.ongoing.retain(|o| o.id != id);
                if !mine {
                    let t: Vec<&str> = mrest.split_whitespace().collect();
                    let names: Vec<String> = t
                        .iter()
                        .filter(|x| {
                            x.len() >= 2
                                && x.chars().next().map(|c| c.is_ascii_alphabetic()) == Some(true)
                                && !x.starts_with("s8")
                                && **x != "R"
                                && **x != "U"
                        })
                        .map(|x| x.to_string())
                        .collect();
                    let gtype = t
                        .iter()
                        .find(|x| x.starts_with("s8") || x.starts_with('8'))
                        .map(|x| x.to_string())
                        .unwrap_or_default();
                    let ratings: Vec<String> = t
                        .iter()
                        .filter(|x| x.parse::<f32>().is_ok() && x.len() >= 3)
                        .map(|x| x.to_string())
                        .collect();
                    s.ongoing.push(OngoingView {
                        id,
                        raw: mrest.to_string(),
                        watching: false,
                        names,
                        ratings,
                        gtype,
                        mine: false,
                    });
                }
                drop(s);
            }
            ctx.dirty = true;
        } else if rest.starts_with('.') {
            add_offer(ctx, rest, login);
            let s = ctx.snap.lock().unwrap();
            let auto = s.standby.enabled && s.standby.auto_accept;
            let in_match = s.matches.iter().any(|m| !m.over);
            let incoming = s.offers.last().map(|o| (o.incoming, o.id.clone()));
            drop(s);
            if let Some((true, id)) = incoming {
                if auto && !in_match {
                    ctx.log("info", &format!("waiting mode: auto-accepting {id}"));
                    send(format!("tell /os accept {id}"));
                } else {
                    let who = ctx
                        .snap
                        .lock()
                        .unwrap()
                        .offers
                        .last()
                        .map(|o| o.names.join(" "))
                        .unwrap_or_default();
                    ctx.notify(
                        &crate::i18n::t("backend.notify.match_request_title"),
                        &format!("{who} ({id})"),
                    );
                }
            }
        }
    } else if let Some(rest) = ln.strip_prefix("/os: end ") {
        let mut it = rest.split_whitespace();
        if let (Some(id), Some(score)) = (it.next(), rest.rsplit(' ').next()) {
            if let Ok(v) = score.trim().parse::<f32>() {
                if let Some(m) = matches.get_mut(id) {
                    let mine = rest
                        .split('(')
                        .nth(1)
                        .and_then(|t| t.split_whitespace().next())
                        .is_some_and(|first| first == login);
                    m.result = format!("{:+.2}", if mine { v } else { -v });
                }
            }
        }
    } else if let Some(rest) = ln.strip_prefix("/os: - ") {
        let rest = rest.trim_start();
        if let Some(mrest) = rest.strip_prefix("match ") {
            let was_mine = mrest.contains(login);
            {
                let id = mrest.split_whitespace().next().unwrap_or("").to_string();
                let mut s = ctx.snap.lock().unwrap();
                s.ongoing.retain(|o| o.id != id);
                drop(s);
                if !was_mine {
                    let (kind, who) = end_kind(mrest);
                    finish_match(matches, &id, "", kind, &who, "");
                }
            }
            sync_matches(ctx, matches);
            ctx.emit(true);
            handle_match_end(ctx, mrest, login, matches);
            if was_mine {
                for t in ["8", "8r"] {
                    pending.push(format!("who:{t}"));
                    send(format!("tell /os who {t}"));
                }
            }
            let s = ctx.snap.lock().unwrap();
            let sb = s.standby.clone();
            let games = s.standby_stats.games;
            drop(s);
            if sb.enabled && (sb.max_games == 0 || games < sb.max_games) && !sb.opponent.is_empty()
            {
                *next_ask_at = Some(Instant::now() + Duration::from_secs(sb.interval_secs.max(5)));
            }
        } else if rest.starts_with('.') {
            let id = rest.split_whitespace().next().unwrap_or("").to_string();
            let mut s = ctx.snap.lock().unwrap();
            s.offers.retain(|o| o.id != id);
            drop(s);
            ctx.dirty = true;
        }
    } else if let Some(req) = parse_request(ln, login) {
        let unattended = {
            let s = ctx.snap.lock().unwrap();
            s.standby.enabled
        };
        let undo = req.verb == "undo";
        ctx.log(
            "info",
            &format!(
                "{} requests {} ({})",
                req.who,
                if undo { "undo" } else { "abort" },
                req.id
            ),
        );
        ctx.notify(
            &crate::i18n::t(if undo {
                "backend.notify.undo_request_title"
            } else {
                "backend.notify.abort_request_title"
            }),
            &format!("{} ({})", req.who, req.id),
        );
        if unattended {
            ctx.log(
                "info",
                &format!("waiting mode: unattended, declining ({})", req.who),
            );
            send(format!("tell /os decline {}", req.who));
        }
    } else if ln.starts_with("/os: ERR") {
        ctx.log("info", &format!("server error: {ln}"));
        if ln.contains("not found") && ln.contains("match") {
            return;
        }
        let msg = ln.trim_start_matches("/os: ERR").trim();
        ctx.snap.lock().unwrap().notice = if msg.is_empty() {
            "err.ggs_action_refused".into()
        } else {
            format!("GGS: {msg}")
        };
        ctx.dirty = true;
    }
}

fn ctx_send(writer: &mut TcpStream, cmd: &str) {
    let _ = writer
        .write_all(cmd.as_bytes())
        .and_then(|_| writer.write_all(b"\n"));
}

fn apply_pacing(
    ctx: &mut Ctx,
    pace: String,
    max_move_secs: u64,
    reserve_secs: u64,
    budget_use: f64,
) {
    let budget_use = if budget_use.is_finite() && budget_use > 0.0 {
        budget_use
    } else {
        2.5
    };
    ctx.engine_cfg_pace = pace.clone();
    ctx.engine_cfg_max_move = max_move_secs;
    ctx.engine_cfg_reserve = reserve_secs;
    ctx.engine_cfg_budget_use = budget_use;
    let mut s = ctx.snap.lock().unwrap();
    s.engine.pace = pace;
    s.engine.max_move_secs = max_move_secs;
    s.engine.reserve_secs = reserve_secs;
    s.engine.budget_use = budget_use;
    drop(s);
    ctx.dirty = true;
}

pub fn resolve_threads(n: usize) -> usize {
    if n == 0 {
        std::thread::available_parallelism()
            .map(|c| (c.get() / 2).max(1))
            .unwrap_or(4)
    } else {
        n
    }
}

fn apply_engine_cfg(ctx: &mut Ctx, depth: u32, solve: u8, band: u8) {
    ctx.engine_cfg.depth = depth;
    ctx.engine_cfg.solve_empties = solve;
    ctx.engine_cfg.band = band;
    if let Some(e) = ctx.engine.as_mut() {
        e.set_levels(depth, solve, band);
    }
    let mut s = ctx.snap.lock().unwrap();
    s.engine.depth = depth;
    s.engine.solve = solve;
    s.engine.band = band;
    drop(s);
    ctx.dirty = true;
}

fn apply_threads(ctx: &mut Ctx) {
    let n = resources().threads.unwrap_or_else(|| resolve_threads(0));
    ctx.engine_cfg.threads = n;
    if let Some(e) = ctx.engine.as_mut() {
        e.set_threads(n);
    }
    ctx.snap.lock().unwrap().engine.threads = n;
    ctx.share_threads();
}

fn handle_block(
    ctx: &mut Ctx,
    block: &[String],
    login: &str,
    matches: &mut HashMap<String, MatchState>,
    send: impl FnMut(String),
) {
    let mid = block[0].split_whitespace().nth(2).unwrap_or("").to_string();
    if mid.is_empty() {
        return;
    }
    let m = matches.entry(mid.clone()).or_insert_with(MatchState::new);
    m.seen += 1;
    if let Some(t) = block[0].split_whitespace().nth(3) {
        if t.starts_with('8') || t.starts_with("s8") {
            m.gtype = t.to_string();
        }
    }
    let was_overtime = m.in_overtime;
    let (rows_ok, turn) = apply_block(m, block, login);
    if m.in_overtime && !was_overtime {
        ctx.log(
            "info",
            &format!(
                "{mid}: in overtime. **this game is a decided loss on time** \
                 (the result is capped at a minimal loss); playing out fast \
                 from here to avoid a wipeout"
            ),
        );
    }

    sync_matches(ctx, matches);
    ctx.emit(true);

    let (auto, watch_an) = {
        let s = ctx.snap.lock().unwrap();
        (s.auto_play, s.watch_analysis)
    };
    let m = matches.get_mut(&mid).unwrap();
    if m.my_color.is_none() {
        if watch_an && rows_ok && turn.is_some() {
            analyze_watch(ctx, &mid, matches);
        }
        return;
    }
    if turn.is_some() && turn == m.my_color && !m.told_turn {
        m.told_turn = true;
        if !auto {
            let who = if m.opp_name.is_empty() {
                mid.clone()
            } else {
                m.opp_name.clone()
            };
            ctx.notify(&crate::i18n::t("backend.notify.your_turn_title"), &who);
        }
    } else if turn != m.my_color {
        let m = matches.get_mut(&mid).unwrap();
        m.told_turn = false;
    }
    ctx.ponder_at = if turn.is_some() && turn != matches[&mid].my_color {
        Some(mid.clone())
    } else {
        None
    };
    let m = matches.get_mut(&mid).unwrap();
    if !auto || !rows_ok || turn.is_none() || turn != m.my_color {
        return;
    }
    think_and_play(ctx, &mid, matches, send);
}

fn ponder_slice(ctx: &mut Ctx, matches: &HashMap<String, MatchState>) {
    const SLICE: Duration = Duration::from_millis(200);
    if !ctx.engine_cfg_ponder {
        return;
    }
    let Some(mid) = ctx.ponder_at.clone() else {
        return;
    };
    let Some(m) = matches.get(&mid) else {
        ctx.ponder_at = None;
        return;
    };
    let Some(board) = board_of(m, m.turn) else {
        return;
    };
    if Some(m.turn) == m.my_color {
        return; // it became our turn again
    }
    if ctx.engine.is_none() {
        return;
    }
    let engine = ctx.engine.as_mut().unwrap();
    engine.ponder(&board, Instant::now() + SLICE);
}

fn analyze_watch(ctx: &mut Ctx, mid: &str, matches: &mut HashMap<String, MatchState>) {
    let m = matches.get_mut(mid).unwrap();
    let Some(board) = board_of(m, m.turn) else {
        return;
    };
    let bh = board.black.wrapping_mul(31).wrapping_add(board.white) ^ (m.turn as u64);
    if bh == m.watch_hash {
        return; // never re-analyze the same position
    }
    m.watch_hash = bh;
    let black_turn = m.turn == '*';
    if ctx.ensure_engine().is_err() {
        return;
    }
    let base = (
        ctx.engine_cfg.depth,
        ctx.engine_cfg.solve_empties,
        ctx.engine_cfg.band,
    );
    let engine = ctx.engine.as_mut().unwrap();
    engine.set_levels(base.0.min(14), base.1.min(20), 0);
    let mv = engine.choose(&board);
    engine.set_levels(base.0, base.1, base.2);

    let m = matches.get_mut(mid).unwrap();
    let v = if mv.value.is_finite() { mv.value } else { 0.0 };
    m.watch_eval = Some(if black_turn { v } else { -v }); // to Black's view
    m.watch_best = mv.pos.map(coord);
    m.watch_exact = mv.exact;
    sync_matches(ctx, matches);
    ctx.emit(true);
}

fn board_of(m: &MatchState, turn: char) -> Option<Board> {
    if turn != '*' && turn != 'O' {
        return None;
    }
    let mut sboard = String::with_capacity(66);
    for r in 0..8 {
        for f in 0..8 {
            sboard.push(match m.cells[f * 8 + r] {
                1 => 'X',
                2 => 'O',
                _ => '-',
            });
        }
    }
    sboard.push(' ');
    sboard.push(if turn == '*' { 'X' } else { 'O' });
    Board::from_string(&sboard).ok()
}

fn apply_block(m: &mut MatchState, block: &[String], login: &str) -> (bool, Option<char>) {
    let mut rows: Vec<Vec<char>> = Vec::new();
    let mut boards: Vec<Vec<Vec<char>>> = Vec::new();
    let mut turns: Vec<char> = Vec::new();
    let mut turn: Option<char> = None;
    let moves_before = m.moves.len();
    m.players.clear();
    for l in block {
        let b = l.strip_prefix('|').unwrap_or(l);
        if let Some(open) = b.find('(') {
            let name = b[..open].trim();
            if !name.is_empty() && !name.contains(' ') && b[open..].contains(')') {
                let close = open + b[open..].find(')').unwrap();
                let inner = b[open + 1..close].trim();
                let color = inner.chars().last().unwrap_or(' ');
                if color == '*' || color == 'O' {
                    let rating = inner.trim_end_matches(['*', 'O']).trim().to_string();
                    let clock = b[close + 1..].trim().to_string();
                    let (main, inc, ext) = parse_clock(&clock);
                    m.players.retain(|p| p.name != name);
                    m.players.push(PlayerView {
                        name: name.to_string(),
                        rating: inner.trim_end_matches(['*', 'O']).trim().to_string(),
                        clock: clock.clone(),
                        color: if color == '*' {
                            "black".into()
                        } else {
                            "white".into()
                        },
                        secs: main,
                        ext,
                    });
                    if name == login {
                        m.my_color = Some(color);
                        m.my_clock = clock.clone();
                        if let (Some(now), Some(g)) = (main, ext) {
                            let bump = inc.unwrap_or(0);
                            let jumped = m
                                .my_clock_secs
                                .is_some_and(|prev| now > prev.saturating_add(bump));
                            let played = m.moves.len() >= 2;
                            if g > 0 && (jumped || (now == 0 && played)) {
                                m.in_overtime = true;
                            }
                        }
                        if !m.over {
                            m.my_clock_secs = main;
                            m.my_ext = ext;
                        }
                    } else {
                        m.opp_name = name.to_string();
                        m.opp_rating = rating;
                        if !m.over {
                            m.opp_clock = clock;
                            m.opp_secs = main;
                            m.opp_ext = ext;
                        }
                    }
                }
            }
        }
        let t = b.trim_start();
        if t.chars().next().map(|c| c.is_ascii_digit()) == Some(true) {
            let rest = &t[1..];
            let cells: Vec<char> = rest
                .split_whitespace()
                .take(8)
                .filter(|&w| w.len() == 1 && matches!(w.as_bytes()[0], b'-' | b'*' | b'O'))
                .map(|w| w.chars().next().unwrap())
                .collect();
            if cells.len() == 8 {
                rows.push(cells);
                if rows.len() == 8 {
                    boards.push(std::mem::take(&mut rows));
                }
            }
        }
        if t.starts_with("* to move") {
            turn = Some('*');
            turns.push('*');
        } else if t.starts_with("O to move") {
            turn = Some('O');
            turns.push('O');
        }
        if let Some(colon) = t.find(':') {
            let (num, rest) = t.split_at(colon);
            if let Some(n) = num.trim().parse::<u32>().ok().filter(|n| *n > 0) {
                let body = rest[1..].trim();
                let mut parts = body.split('/');
                let mv = parts
                    .next()
                    .unwrap_or("")
                    .split(' ')
                    .next()
                    .unwrap_or("")
                    .to_string();
                if ((2..=4).contains(&mv.len()) || mv.eq_ignore_ascii_case("pa")) && !mv.is_empty()
                {
                    // The server returns an evaluation of 0.00 as an empty
                    // field -- 412 zeros went out over one session and not one
                    // came back with its value -- so an empty field that is
                    // there at all reads as 0. A move with no field (`F1`, no
                    // slashes) reported nothing and stays unknown; without the
                    // distinction every settled endgame drops out of the graph.
                    let ev = parts.next().and_then(|x| match x.trim() {
                        "" => Some(0.0),
                        v => v.parse::<f32>().ok(),
                    });
                    let sec = parts.next().and_then(|x| x.trim().parse::<f32>().ok());
                    m.moves.insert(n, mv);
                    let slot = m.move_evals.entry(n).or_insert((None, None));
                    if ev.is_some() {
                        slot.0 = ev;
                    }
                    if sec.is_some() {
                        slot.1 = sec;
                    }
                }
            }
        }
    }

    if let (Some(t), Some(mc)) = (turn, m.my_color) {
        if let Some((&n, _)) = m.moves.iter().next_back() {
            m.eval_parity = Some(if t != mc { n % 2 } else { (n + 1) % 2 });
        }
        if t == mc {
            if let Some((n, _)) = m.moves.iter().next_back() {
                let (ev, sec) = m.move_evals.get(n).copied().unwrap_or((None, None));
                m.opp_eval = ev;
                m.opp_secs_used = sec;
            }
        }
    }

    let to_cells = |rows: &Vec<Vec<char>>| -> Vec<u8> {
        let mut cells = vec![0u8; 64];
        for (r, row) in rows.iter().enumerate() {
            for (f, &c) in row.iter().enumerate() {
                cells[f * 8 + r] = match c {
                    '*' => 1,
                    'O' => 2,
                    _ => 0,
                };
            }
        }
        cells
    };
    if let Some(last) = boards.last() {
        m.cells = to_cells(last);
    }
    if boards.len() >= 2 && m.start_cells.is_empty() {
        m.start_cells = to_cells(&boards[0]);
        m.start_turn = turns.first().copied().unwrap_or('*');
    } else if m.start_cells.is_empty() && moves_before == 0 && m.moves.is_empty() {
        if let Some(last) = boards.last() {
            m.start_cells = to_cells(last);
            m.start_turn = turn.unwrap_or('*');
        }
    }
    m.turn = turn.unwrap_or(' ');
    (!boards.is_empty(), turn)
}

fn time_budget(
    mut s: kuroobi::timectl::Situation,
    base: (u32, u8, u8),
    pace: &str,
) -> (u32, u8, u8, Option<Duration>) {
    s.nps = resources().nps_for(s.threads);
    let p = kuroobi::timectl::plan(
        s,
        kuroobi::timectl::Levels {
            depth: base.0,
            solve: base.1,
            band: base.2,
            auto_band: true,
        },
        kuroobi::timectl::Pace::parse(pace),
    );
    (p.depth, p.solve, p.band, p.cap)
}

fn think_and_play(
    ctx: &mut Ctx,
    mid: &str,
    matches: &mut HashMap<String, MatchState>,
    mut send: impl FnMut(String),
) {
    let m = matches.get_mut(mid).unwrap();
    let Some(board) = board_of(m, m.my_color.unwrap_or(' ')) else {
        ctx.log("info", "failed to parse the board");
        return;
    };
    let bh = board
        .black
        .wrapping_mul(31)
        .wrapping_add(board.white)
        .wrapping_add((m.moves.len() as u64).wrapping_mul(0x9e37_79b9));
    if bh == m.last_played_hash {
        return;
    }
    if ctx.workers.iter().any(|w| {
        w.mid.as_deref() == Some(mid)
            && ((w.busy && !w.pondering && w.pending_hash == bh)
                || w.pending.as_ref().is_some_and(|p| p.hash == bh))
    }) {
        return;
    }
    if let Err(e) = ctx.ensure_engine() {
        ctx.log("info", &format!("engine init failed: {e}"));
        return;
    }
    let clock_secs = m.my_clock_secs;
    let grace = m.my_ext.unwrap_or(0);
    let empties = board.empty_count();
    {
        let mut s = ctx.snap.lock().unwrap();
        s.thinking = Some(mid.to_string());
    }
    ctx.emit(true);

    let base = (
        ctx.engine_cfg.depth,
        ctx.engine_cfg.solve_empties,
        ctx.engine_cfg.band,
    );
    let (d, solve, band, cap) = time_budget(
        kuroobi::timectl::Situation {
            clock_secs,
            in_overtime: m.in_overtime,
            grace_secs: grace,
            empties,
            max_move_secs: ctx.engine_cfg_max_move,
            reserve_secs: ctx.engine_cfg_reserve,
            budget_use: ctx.engine_cfg_budget_use,
            threads: ctx.worker_threads,
            ..Default::default()
        },
        base,
        &ctx.engine_cfg_pace,
    );
    if let Some(i) = ctx.worker_for(mid) {
        ctx.workers[i].pending = Some(Pending {
            board,
            levels: (d, solve, band),
            cap,
            hash: bh,
            hint: mirror_hint(matches, mid),
        });
        if ctx.workers[i].pondering {
            ctx.workers[i].stop.stop();
        }
        pump_worker(ctx, i);
        return;
    }
    if let Err(e) = ctx.ensure_engine() {
        ctx.log("info", &format!("engine init failed: {e}"));
        return;
    }
    let engine = ctx.engine.as_mut().unwrap();
    engine.set_levels(d, solve, band);
    let deadline = cap.map(|c| std::time::Instant::now() + c);
    let began = std::time::Instant::now();
    let mv = engine.choose_within(&board, deadline);
    let took = began.elapsed();
    engine.set_levels(base.0, base.1, base.2);

    let mstr = match mv.pos {
        Some(p) => coord(p),
        None => "pa".to_string(),
    };
    let arg = play_arg(&mstr, mv.value, Some(took));
    send(format!("tell /os play {mid} {arg}"));
    ctx.log("out", &format!("tell /os play {mid} {arg}"));
    ctx.log(
        "info",
        &format!(
            "{mid} {mstr}: {} {:+.2}{}{}",
            if mv.from_book && mv.learned {
                "book (learned)"
            } else if mv.from_book {
                "book"
            } else {
                "search"
            },
            mv.value,
            if mv.exact { " (solved)" } else { "" },
            if mv.depth > 0 {
                format!(" depth {}", mv.depth)
            } else {
                String::new()
            }
        ),
    );

    let m = matches.get_mut(mid).unwrap();
    m.last_eval = Some(if mv.value.is_finite() { mv.value } else { 0.0 });
    m.last_eval_exact = mv.exact;
    m.last_from_book = mv.from_book;
    m.last_played_hash = bh;
    {
        let mut s = ctx.snap.lock().unwrap();
        s.thinking = None;
    }
    sync_matches(ctx, matches);
    ctx.emit(true);
}

fn pump_worker(ctx: &mut Ctx, i: usize) {
    if ctx.workers[i].busy || ctx.workers[i].pending.is_none() {
        return;
    }
    let Some(p) = ctx.workers[i].pending.take() else {
        return;
    };
    ctx.workers[i].pending_hash = p.hash;
    ctx.workers[i].pondering = false;
    ctx.workers[i].sent_at = Some((Instant::now(), p.cap.unwrap_or(Duration::from_secs(60))));
    ctx.workers[i].stop.reset();
    let empties = p.board.empty_count();
    let movable = p.board.movable_count();
    ctx.log(
        "info",
        &format!(
            "dispatch: empties {empties} moves {movable} deadline {:.1}s (depth {} solve {} band {})",
            p.cap.map_or(-1.0, |c| c.as_secs_f32()),
            p.levels.0,
            p.levels.1,
            p.levels.2,
        ),
    );
    ctx.workers[i].send(Job::Think {
        board: p.board,
        levels: p.levels,
        cap: p.cap,
        hint: p.hint,
    });
}

fn collect_workers(ctx: &mut Ctx, matches: &mut HashMap<String, MatchState>) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..ctx.workers.len() {
        if !ctx.workers[i].busy {
            continue;
        }
        let Some((at, cap)) = ctx.workers[i].sent_at else {
            continue;
        };
        if at.elapsed() > cap.mul_f32(3.0) + Duration::from_secs(2) {
            ctx.workers[i].stop.stop();
            ctx.workers[i].stopped_at.get_or_insert_with(Instant::now);
            ctx.workers[i].sent_at = None;
            ctx.log(
                "info",
                &format!(
                    "stopped a search far past its deadline ({:.1}s / deadline {:.1}s)",
                    at.elapsed().as_secs_f32(),
                    cap.as_secs_f32()
                ),
            );
        }
    }
    for i in 0..ctx.workers.len() {
        let Some(since) = ctx.workers[i].stopped_at else {
            continue;
        };
        if !ctx.workers[i].busy {
            ctx.workers[i].stopped_at = None;
            continue;
        }
        if since.elapsed() < WORKER_GIVE_UP {
            continue;
        }
        let cfg = ctx.engine_cfg.clone();
        match EngineWorker::spawn(cfg) {
            Ok(w) => {
                ctx.workers[i] = w;
                ctx.log(
                    "info",
                    "abandoned an unstoppable search and rebuilt the worker",
                );
            }
            Err(e) => ctx.log("info", &format!("worker rebuild failed: {e}")),
        }
    }
    for i in 0..ctx.workers.len() {
        let done = ctx.workers[i].rx.try_recv().ok();
        let Some(done) = done else { continue };
        ctx.workers[i].busy = false;
        let took = ctx.workers[i].sent_at.map(|(t, _)| t.elapsed());
        ctx.workers[i].sent_at = None;
        pump_worker(ctx, i);
        let Done::Moved(mv) = done else { continue };
        if let Some(t) = took {
            ctx.log(
                "info",
                &format!(
                    "reply: {:.1}s{}{} {}",
                    t.as_secs_f32(),
                    if mv.cut { " (cut)" } else { "" },
                    if mv.depth > 0 {
                        format!(" depth {}", mv.depth)
                    } else {
                        String::new()
                    },
                    if mv.exact { "solve" } else { "search" }
                ),
            );
        }
        let Some(mid) = ctx.workers[i].mid.clone() else {
            continue;
        };
        let bh = ctx.workers[i].pending_hash;
        let Some(m) = matches.get_mut(&mid) else {
            continue;
        };
        if m.over || m.my_color.is_none() || Some(m.turn) != m.my_color {
            continue;
        }
        let now_hash = board_of(m, m.turn).map(|b| {
            b.black
                .wrapping_mul(31)
                .wrapping_add(b.white)
                .wrapping_add((m.moves.len() as u64).wrapping_mul(0x9e37_79b9))
        });
        if now_hash != Some(bh) {
            ctx.log(
                "info",
                &format!("{mid}: searched position differs from the current one; not playing (will re-search)"),
            );
            continue;
        }
        let mstr = match mv.pos {
            Some(p) => coord(p),
            None => "pa".to_string(),
        };
        out.push(format!(
            "tell /os play {mid} {}",
            play_arg(&mstr, mv.value, took)
        ));
        m.last_eval = Some(if mv.value.is_finite() { mv.value } else { 0.0 });
        m.last_eval_exact = mv.exact;
        m.last_from_book = mv.from_book;
        m.last_played_hash = bh;
        ctx.log(
            "info",
            &format!(
                "{mid} {mstr}: {} {:+.2}{}",
                if mv.from_book && mv.learned {
                    "book (learned)"
                } else if mv.from_book {
                    "book"
                } else {
                    "search"
                },
                mv.value,
                if mv.exact { " (solved)" } else { "" }
            ),
        );
        {
            let mut s = ctx.snap.lock().unwrap();
            if s.thinking.as_deref() == Some(mid.as_str()) {
                s.thinking = None;
            }
        }
        ctx.dirty = true;
    }

    if ctx.engine_cfg_ponder {
        const SLICE: Duration = Duration::from_secs(5);
        for i in 0..ctx.workers.len() {
            if ctx.workers[i].busy {
                continue;
            }
            let Some(mid) = ctx.workers[i].mid.clone() else {
                continue;
            };
            let Some(m) = matches.get(&mid) else { continue };
            if m.over {
                continue;
            }
            if m.my_color.is_none() || Some(m.turn) == m.my_color {
                continue;
            }
            let Some(board) = board_of(m, m.turn) else {
                continue;
            };
            if ctx.workers[i].pending.is_some() {
                continue;
            }
            ctx.workers[i].pondering = true;
            ctx.workers[i].send(Job::Ponder {
                board,
                slice: SLICE,
            });
        }
    }
    out
}

fn sync_matches(ctx: &mut Ctx, matches: &HashMap<String, MatchState>) {
    let mut view: Vec<MatchView> = matches
        .iter()
        .map(|(id, m)| MatchView {
            id: id.clone(),
            base: base_id(id),
            rated: ctx.rated_by_base.get(&base_id(id)).copied(),
            ended: m.ended.clone(),
            left_by: m.left_by.clone(),
            archive: m.archive.clone(),
            busy: String::new(),
            busy_depth: 0,
            busy_best: None,
            busy_eval: None,
            busy_predict: None,
            cells: m.cells.clone(),
            turn: match m.turn {
                '*' => "black".into(),
                'O' => "white".into(),
                _ => "".into(),
            },
            my_color: match m.my_color {
                Some('*') => "black".into(),
                Some('O') => "white".into(),
                _ => "".into(),
            },
            opp_name: m.opp_name.clone(),
            opp_rating: m.opp_rating.clone(),
            opp_clock: m.opp_clock.clone(),
            my_clock: m.my_clock.clone(),
            my_secs: m.my_clock_secs,
            opp_secs: m.opp_secs,
            my_ext: m.my_ext,
            opp_ext: m.opp_ext,
            in_overtime: m.in_overtime,
            players: m.players.clone(),
            gtype: m.gtype.clone(),
            ggf: m.ggf(id, None),
            moves: m.moves.values().cloned().collect(),
            over: m.over,
            result: m.result.clone(),
            last_eval: m.last_eval,
            last_eval_exact: m.last_eval_exact,
            opp_eval: m.opp_eval,
            opp_secs_used: m.opp_secs_used,
            eval_series: m.eval_series(),
            last_from_book: m.last_from_book,
            watch_eval: m.watch_eval,
            watch_best: m.watch_best.clone(),
            watch_exact: m.watch_exact,
            seen: m.seen,
            order: m.order,
        })
        .collect();
    view.sort_by_key(|v| std::cmp::Reverse(v.order));
    for v in &mut view {
        let Some(w) = ctx.workers.iter().find(|w| w.mid.as_deref() == Some(&v.id)) else {
            continue;
        };
        if !w.busy {
            continue;
        }
        let (kind, depth, best, eval, predict) = w.progress.snapshot();
        v.busy = match kind {
            kuroobi::engine::Progress::THINK => "think",
            kuroobi::engine::Progress::PONDER => "ponder",
            kuroobi::engine::Progress::SOLVE => "solve",
            kuroobi::engine::Progress::SELECT => "select",
            _ => "",
        }
        .into();
        v.busy_depth = depth;
        v.busy_best = best;
        v.busy_eval = eval;
        v.busy_predict = predict;
    }
    ctx.snap.lock().unwrap().matches = view;
    ctx.dirty = true;
}

fn add_offer(ctx: &mut Ctx, rest: &str, login: &str) {
    let Some(offer) = parse_offer(rest, login) else {
        return;
    };
    let id = offer.id.clone();
    let mut s = ctx.snap.lock().unwrap();
    s.offers.retain(|o| o.id != id);
    s.offers.push(offer);
    drop(s);
    ctx.dirty = true;
}

fn parse_offer(rest: &str, login: &str) -> Option<Offer> {
    let id = rest.split_whitespace().next().unwrap_or("").to_string();
    if id.is_empty() {
        return None;
    }
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let mut names = Vec::new();
    let mut gtype = String::new();
    let mut time = String::new();
    let mut rated = false;
    for t in &toks[1..] {
        if t.contains(':') && t.chars().next().map(|c| c.is_ascii_digit()) == Some(true) {
            time = t.to_string();
        } else if t.starts_with("s8") || *t == "8" || t.starts_with("8r") {
            gtype = t.to_string();
        } else if *t == "R" {
            rated = true;
        } else if *t == "U" {
            rated = false;
        } else if t.len() >= 2
            && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && t.chars().next().map(|c| c.is_ascii_alphabetic()) == Some(true)
        {
            names.push(t.to_string());
        }
    }
    let incoming =
        names.iter().any(|n| n == login) && names.first().map(|n| n.as_str()) != Some(login);
    Some(Offer {
        id,
        raw: rest.to_string(),
        incoming,
        names,
        gtype,
        time,
        rated,
    })
}

fn end_kind(rest: &str) -> (&'static str, String) {
    if rest.contains(" aborted") {
        return ("aborted", String::new());
    }
    let toks: Vec<&str> = rest.split_whitespace().collect();
    if let Some(i) = toks.iter().position(|t| *t == "left") {
        let who = i
            .checked_sub(1)
            .map(|j| toks[j].to_string())
            .unwrap_or_default();
        return ("adjourned", who);
    }
    ("finished", String::new())
}

fn re_text(score: Option<f32>) -> Option<String> {
    score.map(|s| format!("{s:+.2}"))
}

fn handle_match_end(
    ctx: &mut Ctx,
    rest: &str,
    login: &str,
    matches: &mut HashMap<String, MatchState>,
) {
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let id = toks.first().copied().unwrap_or("").to_string();
    if !rest.contains(login) {
        return;
    }
    let score: Option<f32> = toks.iter().find_map(|t| {
        (t.starts_with(['+', '-']))
            .then(|| t.parse::<f32>().ok())
            .flatten()
    });
    let first_name = toks
        .iter()
        .skip(1)
        .find(|t| t.len() >= 2 && t.chars().next().map(|c| c.is_ascii_alphabetic()) == Some(true))
        .copied()
        .unwrap_or("");
    let (kind, who) = end_kind(rest);
    let archive = toks
        .last()
        .filter(|t| t.starts_with('.') && t.len() > 1 && **t != id)
        .copied()
        .unwrap_or("");
    let mut dropped = finish_match(
        matches,
        &id,
        re_text(score).as_deref().unwrap_or(""),
        kind,
        &who,
        archive,
    );
    dropped.sort_by_key(|m| m.seen);
    let m = dropped.first();
    let re = score.map(|s| format!("{s:+.2}"));
    let (kifu, ggf, opp) = match &m {
        Some(m) => (m.kifu(), m.ggf(&id, re.as_deref()), m.opp_name.clone()),
        None => (String::new(), String::new(), String::new()),
    };
    let synchro = toks.iter().any(|t| t.starts_with("s8"));
    let my_diff = score.map(|s| my_stone_diff(s, first_name, login, synchro));
    let opp_for_note = if opp.is_empty() {
        "?".to_string()
    } else {
        opp.clone()
    };
    ctx.seq += 1;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let result = GameResult {
        id: id.clone(),
        base: base_id(&id),
        ggf,
        archive: toks
            .last()
            .filter(|t| t.starts_with('.') && t.len() > 1 && **t != id)
            .map(|t| t.to_string())
            .unwrap_or_default(),
        raw: rest.to_string(),
        my_diff,
        opp,
        kifu,
        seq: ctx.seq,
        my_rating: {
            let pool = if rest
                .split_whitespace()
                .any(|t| t.starts_with("s8r") || t.starts_with("8r"))
            {
                "8r"
            } else {
                "8"
            };
            let s = ctx.snap.lock().unwrap();
            s.my_ranks
                .iter()
                .find(|r| r.gtype == pool)
                .map(|r| r.rating)
                .or(s.my_rating)
        },
        at: now,
    };
    append_history(&result);
    let mut s = ctx.snap.lock().unwrap();
    s.results.insert(0, result);
    s.results.truncate(200);
    s.thinking = None;
    s.standby_stats.games += 1;
    if let Some(d) = my_diff {
        s.standby_stats.diff_sum += d;
        match d.cmp(&0) {
            std::cmp::Ordering::Greater => s.standby_stats.wins += 1,
            std::cmp::Ordering::Less => s.standby_stats.losses += 1,
            std::cmp::Ordering::Equal => s.standby_stats.draws += 1,
        }
    }
    drop(s);
    sync_matches(ctx, matches);
    let msg = match my_diff {
        Some(d) if d > 0 => crate::i18n::tf(
            "backend.notify.result_win",
            &[("diff", &format!("+{d}")), ("opp", &opp_for_note)],
        ),
        Some(d) if d < 0 => crate::i18n::tf(
            "backend.notify.result_loss",
            &[("diff", &d.to_string()), ("opp", &opp_for_note)],
        ),
        Some(_) => crate::i18n::tf("backend.notify.result_draw", &[("opp", &opp_for_note)]),
        None => rest.to_string(),
    };
    ctx.notify(&crate::i18n::t("backend.notify.game_over_title"), &msg);
    ctx.log("info", &format!("game over: {rest}"));
    ctx.emit(true);

    if !ctx.snap.lock().unwrap().engine.learn {
        return;
    }
    for lm in &dropped {
        if lm.my_color.is_none() {
            continue; // watched games are not imported (not our choices)
        }
        let kifu = lm.kifu();
        if kifu.is_empty() {
            continue;
        }
        if ctx.ensure_engine().is_err() {
            break;
        }
        let start = lm.start_string();
        let start_opt = (!start.is_empty()).then_some(start.as_str());
        match ctx
            .engine
            .as_ref()
            .unwrap()
            .learn_start(start_opt, &kifu, LEARN_DEPTH)
        {
            Ok(job) => {
                ctx.log(
                    "info",
                    &format!("learn: queued {id} ({} positions)", job.remaining()),
                );
                let (black, white) = match kuroobi::learn::replay(start_opt, &kifu) {
                    Ok((_, fin)) => (fin.black.count_ones() as u8, fin.white.count_ones() as u8),
                    Err(_) => (0, 0),
                };
                let entry = crate::LearnEntry {
                    at: crate::now_secs(),
                    kifu: kifu.clone(),
                    black,
                    white,
                    positions: job.remaining() as u32,
                    start: start.clone(),
                    changes: Vec::new(),
                    opponent: lm.opp_name.clone(),
                    my_color: match lm.my_color {
                        Some('*') => "b".into(),
                        Some('O') => "w".into(),
                        _ => String::new(),
                    },
                };
                ctx.learn_jobs.push_back((id.clone(), job, entry));
            }
            Err(e) => ctx.log("info", &format!("learn setup failed ({id}): {e}")),
        }
    }
    ctx.emit(true);
}

fn learn_tick(ctx: &mut Ctx) {
    if ctx.learn_jobs.is_empty() || ctx.ensure_engine().is_err() {
        return;
    }
    let (id, job, entry) = ctx.learn_jobs.front_mut().expect("checked non-empty");
    let id = id.clone();
    let entry = entry.clone();
    match ctx.engine.as_mut().unwrap().learn_step(job, LEARN_DEPTH) {
        Ok(Some(out)) => {
            ctx.learn_jobs.pop_front();
            let mut entry = entry;
            entry.changes = out.changes.iter().map(crate::LearnChange::of).collect();
            crate::learn_log_append(&entry);
            ctx.log(
                "info",
                &format!(
                    "learn: imported {id} ({} values updated, {} positions added, {} left)",
                    out.updated,
                    out.added,
                    ctx.learn_jobs.len()
                ),
            );
        }
        Ok(None) => {}
        Err(e) => {
            ctx.learn_jobs.pop_front();
            ctx.log("info", &format!("learn failed ({id}): {e}"));
        }
    }
}

fn is_month(t: &str) -> bool {
    matches!(
        t,
        "Jan"
            | "Feb"
            | "Mar"
            | "Apr"
            | "May"
            | "Jun"
            | "Jul"
            | "Aug"
            | "Sep"
            | "Oct"
            | "Nov"
            | "Dec"
    )
}

fn capture_header_matches(kind: &str, ln: &str) -> bool {
    if kind.starts_with("who") {
        ln.starts_with("/os: who")
    } else if kind == "top" {
        ln.starts_with("/os: top")
    } else if kind == "stored_list" {
        ln.starts_with("/os: stored")
    } else if kind.starts_with("history:") {
        ln.starts_with("/os: history")
    } else if kind.starts_with("rank:") {
        ln.starts_with("/os: rank")
    } else if kind.starts_with("look:") {
        ln.starts_with("/os: look")
    } else if kind == "match_list" {
        ln.starts_with("/os: match")
    } else if kind.starts_with("osfinger:") {
        ln.starts_with("/os: finger")
    } else if kind.starts_with("finger:") {
        ln.starts_with(": finger") || ln.trim_start().starts_with("login")
    } else {
        false
    }
}

fn parse_dev_token(t: &str) -> Option<f32> {
    let head: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    head.parse::<f32>()
        .ok()
        .filter(|v| (0.0..1000.0).contains(v))
}

fn parse_rating_token(t: &str) -> Option<f32> {
    let head = t.split('@').next()?;
    head.parse::<f32>()
        .ok()
        .filter(|v| (100.0..4000.0).contains(v))
}

const CHAT_IN_MEMORY: usize = 300;

fn parse_chat(ln: &str) -> Option<ChatMsg> {
    let at = now_secs();
    if let Some(rest) = ln.strip_prefix('.') {
        let (head, text) = rest.split_once(": ")?;
        let mut it = head.split_whitespace();
        if let (Some(chan), Some(from), None) = (it.next(), it.next(), it.next()) {
            return Some(ChatMsg {
                chan: format!(".{chan}"),
                from: from.to_string(),
                text: text.to_string(),
                at,
                thread: format!(".{chan}"),
            });
        }
        return None;
    }
    if ln.starts_with(['/', '|', ':', ' ']) || ln == "READY" {
        return None;
    }
    let (name, text) = ln.split_once(": ")?;
    let named = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && !BARE_CMDS.contains(&name);
    named.then(|| ChatMsg {
        chan: String::new(),
        from: name.to_string(),
        text: text.to_string(),
        at,
        thread: name.to_string(),
    })
}

fn push_chat(ctx: &mut Ctx, login: &str, msg: ChatMsg) {
    append_chat(login, &msg);
    let mut s = ctx.snap.lock().unwrap();
    s.chat.push_back(msg);
    while s.chat.len() > CHAT_IN_MEMORY {
        s.chat.pop_front();
    }
    drop(s);
    ctx.dirty = true;
}
fn capture_who(ctx: &mut Ctx, kind: &str, buf: &[String], login: &str) {
    let mut users = Vec::new();
    let mut my_rating = None;
    for l in buf {
        let b = l.strip_prefix("/os: ").unwrap_or(l);
        let b = b.strip_prefix('|').unwrap_or(b);
        let mut toks: Vec<&str> = b.split_whitespace().collect();
        if kind == "top" {
            if toks.first().map(|t| t.chars().all(|c| c.is_ascii_digit())) == Some(true) {
                toks.remove(0);
            } else {
                continue;
            }
        }
        let Some(&name) = toks.first() else { continue };
        if name.is_empty()
            || !name.chars().next().unwrap().is_ascii_alphabetic()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+')
        {
            continue;
        }
        let at = toks
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(i, t)| parse_rating_token(t).map(|v| (i, v)));
        let Some((ri, r)) = at else { continue };
        let rating = Some(r);
        let dev = match toks[ri].split_once('@') {
            Some((_, rest)) if !rest.is_empty() => parse_dev_token(rest),
            Some(_) => toks.get(ri + 1).and_then(|t| parse_dev_token(t)),
            None => None,
        };
        if name == login {
            my_rating = rating;
        }
        let open = if kind.starts_with("who") {
            toks.get(1)
                .filter(|t| matches!(*t, &"+" | &"-" | &"x"))
                .and_then(|t| t.chars().next())
        } else {
            None
        };
        users.push(UserRow {
            name: name.to_string(),
            rating,
            dev,
            rating_r: None,
            dev_r: None,
            open,
            raw: b.to_string(),
        });
    }
    users.sort_by(|a, b| {
        b.rating
            .unwrap_or(0.0)
            .partial_cmp(&a.rating.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut s = ctx.snap.lock().unwrap();
    if !users.is_empty() {
        match kind {
            "who:8r" => {
                for u in &users {
                    if let Some(t) = s.users.iter_mut().find(|x| x.name == u.name) {
                        t.rating_r = u.rating;
                        t.dev_r = u.dev;
                    }
                }
            }
            k if k.starts_with("who") => s.users = users,
            _ => s.ranking = users,
        }
    }
    if let Some(r) = my_rating {
        s.my_rating = Some(r);
        if let Some(latest) = s.results.first_mut() {
            if latest.my_rating.is_none() {
                latest.my_rating = Some(r);
            }
        }
    }
    drop(s);
    ctx.dirty = true;
}

fn capture_match_list(ctx: &mut Ctx, buf: &[String], login: &str) {
    let mut list = Vec::new();
    for l in buf {
        let Some(rest) = l.strip_prefix('|') else {
            continue;
        };
        let t: Vec<&str> = rest.split_whitespace().collect();
        let Some(&id) = t.first() else { continue };
        if !id.starts_with('.') {
            continue;
        }
        let names: Vec<String> = t
            .iter()
            .filter(|x| {
                x.len() >= 2
                    && x.chars().next().map(|c| c.is_ascii_alphabetic()) == Some(true)
                    && !x.starts_with("s8")
                    && **x != "R"
                    && **x != "U"
            })
            .map(|x| x.to_string())
            .collect();
        let gtype = t
            .iter()
            .find(|x| x.starts_with("s8") || x.starts_with('8'))
            .map(|x| x.to_string())
            .unwrap_or_default();
        let ratings: Vec<String> = t
            .iter()
            .filter(|x| x.parse::<f32>().is_ok() && x.len() >= 3)
            .map(|x| x.to_string())
            .collect();
        list.push(OngoingView {
            id: id.to_string(),
            raw: rest.trim().to_string(),
            watching: false,
            names,
            ratings,
            gtype,
            mine: rest.contains(login),
        });
    }
    let mut s = ctx.snap.lock().unwrap();
    for o in list.iter_mut() {
        if s.ongoing.iter().any(|x| x.id == o.id && x.watching) {
            o.watching = true;
        }
    }
    let ids: Vec<String> = list.iter().map(|o| o.id.clone()).collect();
    s.ongoing = list;
    drop(s);
    ctx.dirty = true;
    if std::env::var("KUROOBI_GGS_AUTOWATCH").as_deref() == Ok("auto") {
        for id in ids {
            ctx.auto_watch.push(id);
        }
    }
}

fn capture_stored_list(ctx: &mut Ctx, buf: &[String], login: &str) {
    let mut list = Vec::new();
    for l in buf {
        let Some(rest) = l.strip_prefix('|') else {
            continue;
        };
        let toks: Vec<&str> = rest.split_whitespace().collect();
        let Some(&id) = toks.first() else { continue };
        if !id.starts_with('.') {
            continue;
        }
        let names: Vec<&str> = toks
            .iter()
            .filter(|t| {
                t.len() >= 2
                    && t.chars().next().map(|c| c.is_ascii_alphabetic()) == Some(true)
                    && !t.contains(':')
            })
            .copied()
            .collect();
        let opp = names
            .iter()
            .find(|n| **n != login && !is_month(n))
            .map(|s| s.to_string())
            .unwrap_or_default();
        let gtype = toks
            .iter()
            .rev()
            .find(|t| t.starts_with("s8") || t.starts_with('8'))
            .map(|s| s.to_string())
            .unwrap_or_default();
        list.push(StoredView {
            id: id.to_string(),
            raw: rest.to_string(),
            opp,
            gtype,
        });
    }
    let mut s = ctx.snap.lock().unwrap();
    s.stored = list;
    drop(s);
    ctx.dirty = true;
}

fn capture_history(ctx: &mut Ctx, buf: &[String], login: &str, target: &str) {
    let mut rows = Vec::new();
    for l in buf {
        let Some(rest) = l.strip_prefix('|') else {
            continue;
        };
        let t: Vec<&str> = rest.split_whitespace().collect();
        if t.len() < 11 || !t[0].starts_with('.') {
            continue;
        }
        rows.push(HistoryRow {
            id: t[0].to_string(),
            at: format!("{} {} {} {}", t[1], t[2], t[3], t[4]),
            black_rating: t[5].to_string(),
            black: t[6].to_string(),
            white_rating: t[7].to_string(),
            white: t[8].to_string(),
            score: t[9].to_string(),
            gtype: t[10].to_string(),
        });
    }
    rows.reverse(); // newest first
    let key = if target.is_empty() {
        login.to_string()
    } else {
        target.to_string()
    };
    let mut s = ctx.snap.lock().unwrap();
    s.history.insert(key, rows);
    drop(s);
    ctx.dirty = true;
}

fn capture_rank(ctx: &mut Ctx, buf: &[String], login: &str, rest: &str) {
    let (gtype, target) = rest.split_once(':').unwrap_or((rest, ""));
    for l in buf {
        let b = l.strip_prefix("/os: ").unwrap_or(l);
        let b = b.strip_prefix('|').unwrap_or(b);
        let t: Vec<&str> = b.split_whitespace().collect();
        if t.len() < 3 {
            continue;
        }
        let Ok(rank) = t[0].parse::<u32>() else {
            continue;
        };
        if t[1] != target {
            continue;
        }
        let (rating, dev) = match t[2].split_once('@') {
            Some((r, d)) => (
                r.parse::<f32>().unwrap_or(0.0),
                d.trim_end_matches(['=', '+']).parse::<f32>().unwrap_or(0.0),
            ),
            None => continue,
        };
        let nums: Vec<u64> = t
            .iter()
            .rev()
            .filter_map(|x| x.parse::<u64>().ok())
            .take(3)
            .collect();
        let (wins, draws, losses) = match nums.len() {
            3 => (nums[2], nums[1], nums[0]),
            _ => (0, 0, 0),
        };
        let row = RankRow {
            gtype: gtype.to_string(),
            name: target.to_string(),
            rating,
            dev,
            rank,
            wins,
            draws,
            losses,
        };
        let mut s = ctx.snap.lock().unwrap();
        if target == login {
            s.my_ranks.retain(|r| r.gtype != row.gtype);
            s.my_ranks.push(row);
            s.my_ranks.sort_by(|a, b| a.gtype.cmp(&b.gtype));
        }
        drop(s);
        ctx.dirty = true;
    }
}

fn capture_osfinger(ctx: &mut Ctx, buf: &[String], name: &str) {
    let mut add: Vec<(String, String)> = Vec::new();
    for l in buf {
        if l.starts_with("/os:") {
            continue;
        }
        let t = l.trim_end().trim_start_matches('|');
        let Some((k, v)) = t.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || (k.contains(' ') && !k.contains('(')) {
            continue;
        }
        if k.to_lowercase().starts_with("passw") {
            continue;
        }
        add.push((k.to_string(), v.to_string()));
    }
    let mut s = ctx.snap.lock().unwrap();
    if let Some(f) = s.fingers.get_mut(name) {
        for (k, v) in add {
            if let Some(slot) = f.fields.iter_mut().find(|(kk, _)| *kk == k) {
                slot.1 = v;
            } else {
                f.fields.push((k, v));
            }
        }
    }
    drop(s);
    ctx.dirty = true;
}

fn capture_finger(ctx: &mut Ctx, buf: &[String], name: &str) {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut raw: Vec<String> = Vec::new();
    for l in buf {
        let t = l.trim_end();
        if t.is_empty() || t == ":" {
            continue;
        }
        let t = t.strip_prefix(": ").unwrap_or(t);
        let lower = t.to_lowercase();
        if lower.starts_with("passw") || lower.starts_with("password") {
            continue;
        }
        raw.push(t.to_string());
        if let Some((k, v)) = t.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if !k.is_empty() && !k.contains(' ') || k.contains('(') {
                fields.push((k.to_string(), v.to_string()));
            }
        }
    }
    let mut s = ctx.snap.lock().unwrap();
    s.fingers.insert(
        name.to_string(),
        FingerInfo {
            name: name.to_string(),
            fields,
            raw,
        },
    );
    drop(s);
    ctx.dirty = true;
}

fn finish_capture(ctx: &mut Ctx, kind: &str, buf: &[String], login: &str) {
    if let Some(id) = kind.strip_prefix("look:") {
        let joined: String = buf.iter().map(|l| l.trim()).collect::<Vec<_>>().join("");
        let mut parts: Vec<String> = Vec::new();
        let mut rest = joined.as_str();
        while let Some(i) = rest.find("(;") {
            let after = &rest[i..];
            let Some(end) = after.find(";)") else { break };
            let one = &after[..end + 2];
            if one.contains("GM[Othello]") {
                parts.push(one.to_string());
            }
            rest = &after[end + 2..];
        }
        let ggf = parts.first().cloned();
        let mut s = ctx.snap.lock().unwrap();
        match ggf {
            Some(g) => {
                s.fetched_ggf = Some(FetchedGgf {
                    id: id.to_string(),
                    ggf: g,
                    parts,
                    error: String::new(),
                });
            }
            None => {
                let err = buf
                    .iter()
                    .find(|l| l.contains("ERR"))
                    .cloned()
                    .unwrap_or_default();
                s.fetched_ggf = Some(FetchedGgf {
                    id: id.to_string(),
                    ggf: String::new(),
                    parts: Vec::new(),
                    error: if err.is_empty() {
                        "err.record_not_found".into()
                    } else {
                        err
                    },
                });
            }
        }
        drop(s);
        ctx.dirty = true;
        return;
    }
    if kind.starts_with("who") || kind == "top" {
        capture_who(ctx, kind, buf, login);
    } else if kind == "match_list" {
        capture_match_list(ctx, buf, login);
    } else if kind == "stored_list" {
        capture_stored_list(ctx, buf, login);
    } else if let Some(target) = kind.strip_prefix("history:") {
        capture_history(ctx, buf, login, target);
    } else if let Some(rest) = kind.strip_prefix("rank:") {
        capture_rank(ctx, buf, login, rest);
    } else if let Some(name) = kind.strip_prefix("osfinger:") {
        capture_osfinger(ctx, buf, name);
    } else if let Some(name) = kind.strip_prefix("finger:") {
        capture_finger(ctx, buf, name);
    }
}

#[cfg(test)]
mod tests {

    const WATCH_JOIN_BLOCK: &[&str] = &[
        "/os: join .45.0 s8r14 K?",
        "|24 move(s)",
        "|nyanyan  (2658.9 *) 01:00//00:30",
        "|egrcd    (2585.8 O) 01:00//00:30",
        "|",
        "|   A B C D E F G H",
        "| 1 - - - - - - - - 1",
        "| 2 - - O - - - - - 2",
        "| 3 - - * O * - - - 3",
        "| 4 - - - * O O - - 4",
        "| 5 - - * * * O - - 5",
        "| 6 - - O O - - O - 6",
        "| 7 - - - - - - - - 7",
        "| 8 - - - - - - - - 8",
        "|   A B C D E F G H",
        "|",
        "|* to move",
        "|  1: F3/8.00/3.71",
        "|  2: d2/-9.00/9.08",
        "|  3: F2/8.00/4.91",
        "|  4: b3/-10.00/5.23",
        "|  5: G4/8.00/4.91",
        "|  6: f6/-9.00/4.32",
        "|  7: C4/8.00/4.89",
        "|  8: e2/-9.00/5.23",
        "|  9: G5/8.00/4.88",
        "| 10: e6/-9.00/1.67",
        "| 11: B4/8.00/4.25",
        "| 12: h3/-8.00/1.60",
        "| 13: H4/8.00/5.72",
        "| 14: b6/-7.00/1.57",
        "| 15: H2/8.00/4.00",
        "| 16: h6/-8.00/10.15",
        "| 17: D7/10.00/4.00",
        "| 18: f7/-8.00/1.15",
        "| 19: C1/10.00/4.00",
        "| 20: a4/-7.00/1.09",
        "| 21: A3/12.00/3.27",
        "| 22: a2/-10.00/0.96",
        "| 23: E7/12.00/2.31",
        "| 24: d1/-12.00/1.10",
        "|nyanyan  (2658.9 *) 00:07,12:0//00:30,12:0",
        "|egrcd    (2585.8 O) 00:16,12:0//00:30,12:0",
        "|",
        "|   A B C D E F G H",
        "| 1 - - * O - - - - 1",
        "| 2 O - O O O * - * 2",
        "| 3 O O * O O * - * 3",
        "| 4 O O O O O O * * 4",
        "| 5 - - O O O * * - 5",
        "| 6 - O O O O * O O 6",
        "| 7 - - - * * O - - 7",
        "| 8 - - - - - - - - 8",
        "|   A B C D E F G H",
        "|",
        "|* to move",
    ];

    use super::*;

    #[test]
    fn clock_real_format() {
        let (main, inc, ext) = parse_clock("14:59,0:0//02:00,0:0");
        assert_eq!(main, Some(14 * 60 + 59));
        assert_eq!(inc, None);
        assert_eq!(ext, Some(120));
        let (main, _, ext) = parse_clock("15:00//02:00");
        assert_eq!(main, Some(900));
        assert_eq!(ext, Some(120));
        let (main, _, ext) = parse_clock("00:07");
        assert_eq!(main, Some(7));
        assert_eq!(ext, None);
    }

    #[test]
    fn the_eval_series_keeps_the_moves_nobody_reported() {
        use std::collections::BTreeMap;
        let mut m = MatchState::new();
        m.moves = (1..=6u32)
            .map(|n| (n, "f5".to_string()))
            .collect::<BTreeMap<_, _>>();
        m.eval_parity = Some(1);
        m.move_evals.insert(1, (Some(1.5), None));
        m.move_evals.insert(2, (Some(-2.0), None));

        let series = m.eval_series();
        assert_eq!(series.len(), 6, "one point per move, reported or not");
        assert_eq!(series[0].eval, Some(1.5));
        assert_eq!(series[1].eval, Some(-2.0));
        assert!(
            series[2..].iter().all(|p| p.eval.is_none()),
            "unreported moves stay in the series with no value"
        );
        assert_eq!(series[0].n, 1, "numbering is GGS's, not the index");
        assert!(series[0].mine, "odd moves are ours at this parity");
    }

    #[test]
    fn the_eval_series_spans_every_ply_even_the_missing_ones() {
        use std::collections::BTreeMap;
        let mut m = MatchState::new();
        m.moves = [1u32, 2, 5, 6]
            .into_iter()
            .map(|n| (n, "f5".to_string()))
            .collect::<BTreeMap<_, _>>();
        m.eval_parity = Some(1);
        m.move_evals.insert(1, (Some(1.5), None));
        m.move_evals.insert(6, (Some(-2.0), None));

        let series = m.eval_series();
        assert_eq!(series.len(), 6, "the axis spans plies 1..6, holes included");
        assert_eq!(
            series.iter().map(|p| p.n).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
        assert_eq!(series[0].eval, Some(1.5));
        assert_eq!(
            series[5].eval,
            Some(-2.0),
            "the last ply keeps its own value"
        );
        assert!(
            series[2].eval.is_none(),
            "a missing move leaves a gap, not a shift"
        );
    }

    #[test]
    fn a_per_board_result_survives_the_match_line() {
        let mut ms: HashMap<String, MatchState> = HashMap::new();
        let mut a = MatchState::new();
        a.result = "+10.00".into();
        ms.insert(".4.0".into(), a);
        let mut b = MatchState::new();
        b.result = "-18.00".into();
        ms.insert(".4.1".into(), b);

        finish_match(&mut ms, ".4", "-4.00", "finished", "", ".85929");
        assert_eq!(ms[".4.0"].result, "+10.00");
        assert_eq!(ms[".4.1"].result, "-18.00");

        let mut ms2: HashMap<String, MatchState> = HashMap::new();
        ms2.insert(".9.0".into(), MatchState::new());
        finish_match(&mut ms2, ".9", "+2.00", "finished", "", "");
        assert_eq!(ms2[".9.0"].result, "+2.00");
    }

    #[test]
    fn a_mirror_hint_is_only_taken_while_the_boards_agree() {
        use std::collections::BTreeMap;
        let mk = |mv: &[(&str, &str)]| {
            let mut m = MatchState::new();
            m.moves = mv
                .iter()
                .enumerate()
                .map(|(i, (_, s))| (i as u32 + 1, s.to_string()))
                .collect::<BTreeMap<_, _>>();
            m
        };
        let mut ms = HashMap::new();
        ms.insert(".9.0".to_string(), mk(&[("", "e2"), ("", "g4")]));
        ms.insert(
            ".9.1".to_string(),
            mk(&[("", "e2"), ("", "g4"), ("", "g5")]),
        );
        let h = mirror_hint(&ms, ".9.0").expect("borrows the hint");
        assert_eq!(coord(h), "G5");

        assert!(mirror_hint(&ms, ".9.1").is_none());

        ms.insert(
            ".9.1".to_string(),
            mk(&[("", "e2"), ("", "h4"), ("", "g5")]),
        );
        assert!(
            mirror_hint(&ms, ".9.0").is_none(),
            "borrowed after the boards diverged"
        );

        ms.remove(".9.1");
        assert!(mirror_hint(&ms, ".9.0").is_none());
    }

    #[test]
    fn the_stone_diff_follows_the_first_name() {
        assert_eq!(my_stone_diff(2.0, "kuroobi", "kuroobi", false), 2);
        assert_eq!(my_stone_diff(-10.0, "kuroobi", "kuroobi", false), -10);
        assert_eq!(my_stone_diff(-5.0, "kuroobi", "kuroobi", false), -5);
        assert_eq!(my_stone_diff(-7.0, "kuroobi", "kuroobi", false), -7);
        assert_eq!(my_stone_diff(-6.0, "htz", "kuroobi", false), 6);
        assert_eq!(my_stone_diff(2.0, "htz", "kuroobi", false), -2);
        assert_eq!(my_stone_diff(0.0, "kuroobi", "kuroobi", false), 0);
        assert_eq!(my_stone_diff(0.0, "htz", "kuroobi", false), 0);
    }

    #[test]
    fn a_synchro_margin_is_the_sum_of_both_boards() {
        assert_eq!(my_stone_diff(-10.0, "kuroobi", "kuroobi", true), -20);
        assert_eq!(my_stone_diff(16.0, "kuroobi", "kuroobi", true), 32);
        assert_eq!(my_stone_diff(7.0, "kuroobi", "kuroobi", true), 14);
        assert_eq!(my_stone_diff(-1.0, "piglet", "kuroobi", true), 2);
        assert_eq!(my_stone_diff(0.5, "kuroobi", "kuroobi", true), 1);
    }

    #[test]
    fn request_real_format() {
        let r = parse_request("/os: undo .24 htz is asking", "kuroobi").unwrap();
        assert_eq!(
            (r.verb, r.id.as_str(), r.who.as_str()),
            ("undo", ".24", "htz")
        );
        let r = parse_request("/os: abort .24 htz is asking", "kuroobi").unwrap();
        assert_eq!(r.verb, "abort");
        assert!(parse_request("/os: undo .24 kuroobi is asking", "kuroobi").is_none());
        assert!(parse_request("/os: update .24 8 K?", "kuroobi").is_none());
        assert!(parse_request("/os: undo .24 htz declined", "kuroobi").is_none());
        assert!(parse_request("/os: - match .24 1720 htz", "kuroobi").is_none());
    }

    #[test]
    fn a_negative_clock_reads_as_zero() {
        let (main, _, ext) = parse_clock("-0:03,0:0//02:00,0:0");
        assert_eq!(main, Some(0));
        assert_eq!(ext, Some(120));
    }

    #[test]
    fn a_jumping_clock_means_overtime() {
        let board: Vec<String> = [
            "|   A B C D E F G H",
            "| 1 - - - - - - - - 1 ",
            "| 2 - - - - - - - - 2 ",
            "| 3 - - - - - - - - 3 ",
            "| 4 - - - O * - - - 4 ",
            "| 5 - - - * O - - - 5 ",
            "| 6 - - - - - - - - 6 ",
            "| 7 - - - - - - - - 7 ",
            "| 8 - - - - - - - - 8 ",
            "|   A B C D E F G H",
            "|* to move",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let with_clock = |secs: &str| {
            let mut b = vec![
                format!("|kuroobi  (1720.0 *) {secs}//02:00,0:0"),
                "|  1: F5/1.00/0.00".to_string(),
                "|  2: D6/1.00/0.00".to_string(),
            ];
            b.extend(board.iter().cloned());
            b
        };

        let mut m = MatchState::new();
        apply_block(&mut m, &with_clock("00:04,0:0"), "kuroobi");
        assert!(!m.in_overtime, "still in main time");
        apply_block(&mut m, &with_clock("02:00,0:0"), "kuroobi");
        assert!(m.in_overtime, "missed the jump");
        let mut m2 = MatchState::new();
        apply_block(&mut m2, &with_clock("00:03,0:0"), "kuroobi");
        assert!(!m2.in_overtime);
        apply_block(&mut m2, &with_clock("00:00,0:0"), "kuroobi");
        assert!(m2.in_overtime, "missed the clock stuck at 0");
        apply_block(&mut m, &with_clock("01:12,0:0"), "kuroobi");
        assert!(m.in_overtime, "cleared the overtime flag");
    }

    #[test]
    fn chat_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("kuroobi-chat-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let _ = std::fs::create_dir_all("ggs_games");
        let m = ChatMsg {
            chan: ".Harmony".into(),
            from: "Harmony".into(),
            text: "hi".into(),
            at: 42,
            thread: ".Harmony".into(),
        };
        append_chat("kuroobi", &m);
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(chat_path("kuroobi"))
                .unwrap();
            let _ = writeln!(f, "{{\"chan\": truncated line");
        }
        let back = load_chat("kuroobi");
        std::env::set_current_dir(prev).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back.len(), 1, "cannot read the file back");
        assert_eq!(back[0].text, "hi");
        assert_eq!(back[0].thread, ".Harmony");
    }

    #[test]
    fn the_zero_move_marker_is_not_a_move() {
        let join: Vec<String> = [
            "|0 move(s)",
            "|  0: PASS",
            "|kuroobi  (2300.0 *) 15:00,0:0//02:00,0:0",
            "|Rhapsody (2700.0 O) 15:00,0:0//02:00,0:0",
            "|   A B C D E F G H",
            "| 1 - - - - - - - - 1 ",
            "| 2 - - - - - - - - 2 ",
            "| 3 - - - * - - - - 3 ",
            "| 4 - - * * * - - - 4 ",
            "| 5 - - - * O - - - 5 ",
            "| 6 - - - - - - - - 6 ",
            "| 7 - - - - - - - - 7 ",
            "| 8 - - - - - - - - 8 ",
            "|   A B C D E F G H",
            "|* to move",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut m = MatchState::new();
        apply_block(&mut m, &join, "kuroobi");
        assert!(m.moves.is_empty(), "move 0 was recorded as a played move");
        let ggf = m.ggf(".1", None);
        let bo = ggf.split_once("BO[8 ").expect("BO tag present").1;
        let discs = bo.chars().take(64).filter(|c| *c != '-').count();
        assert_eq!(
            discs, 6,
            "the post-draw board is not the start position: {bo}"
        );
        assert!(!ggf.contains("B[PA]"), "the record begins with a pass");
    }

    /// A stored margin is only as good as the name order it was read with.
    /// These are real lines: `.63` went down 29 discs and was filed as a
    /// 29-disc win, which turned a losing August into a winning one.
    #[test]
    fn a_stored_margin_follows_the_name_written_first() {
        assert_eq!(
            diff_from_raw(
                ".63 2696 Rhapsody 2358 kuroobi s8r14 R +29.00  .83993",
                "Rhapsody"
            ),
            Some(-58),
            "the score belongs to the name written first, and a pair doubles it"
        );
        assert_eq!(
            diff_from_raw(
                ".21 2469 kuroobi 2591 Rhapsody s8r16 R +1.00  .86021",
                "Rhapsody"
            ),
            Some(2),
            "written first ourselves, the sign stands"
        );
        assert_eq!(
            diff_from_raw(".9 2400 kuroobi 2400 saio 8 R -3.00", "saio"),
            Some(-3),
            "a lone board is not doubled"
        );
        assert_eq!(
            diff_from_raw(".7 2400 kuroobi 2400 saio s8r16", "saio"),
            None,
            "an adjourned match has no margin to show"
        );
    }

    /// The server hands back an evaluation of 0.00 as an empty field, so
    /// reading the field as "no value" lost every settled endgame from the
    /// graph -- the line simply stopped where the game became even.
    #[test]
    fn an_empty_evaluation_field_is_a_zero() {
        let block: Vec<String> = [
            "|  1: F5/1.50/12.00",
            "|  2: D6//8.00",
            "|  3: C4",
            "|* to move",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut m = MatchState::new();
        apply_block(&mut m, &block, "kuroobi");
        assert_eq!(m.move_evals.get(&1).map(|e| e.0), Some(Some(1.5)));
        assert_eq!(
            m.move_evals.get(&2).map(|e| e.0),
            Some(Some(0.0)),
            "an empty field that is present means 0.00"
        );
        assert_eq!(
            m.move_evals.get(&3).map(|e| e.0),
            Some(None),
            "a move with no field at all reported nothing"
        );
        assert_eq!(
            m.move_evals.get(&2).map(|e| e.1),
            Some(Some(8.0)),
            "the seconds after an empty evaluation still land"
        );
    }

    #[test]
    fn a_finished_clock_is_frozen() {
        let with_clock = |secs: &str| {
            vec![
                format!("|kuroobi  (1720.0 *) {secs}//02:00,0:0"),
                "|  1: F5/1.00/0.00".to_string(),
                "|* to move".to_string(),
            ]
        };
        let mut m = MatchState::new();
        apply_block(&mut m, &with_clock("05:00,0:0"), "kuroobi");
        assert_eq!(m.my_clock_secs, Some(300));
        m.over = true;
        apply_block(&mut m, &with_clock("02:09,0:0"), "kuroobi");
        assert_eq!(
            m.my_clock_secs,
            Some(300),
            "the clock moved after the game ended"
        );
    }

    #[test]
    fn a_dealt_opening_is_kept_as_the_start() {
        let dealt: Vec<String> = [
            "|kuroobi  (2300.0 *) 15:00,0:0//02:00,0:0",
            "|Rhapsody (2700.0 O) 15:00,0:0//02:00,0:0",
            "|   A B C D E F G H",
            "| 1 - - - - - - - - 1 ",
            "| 2 - - - - - - - - 2 ",
            "| 3 - - - * - - - - 3 ",
            "| 4 - - * * * - - - 4 ",
            "| 5 - - - * O - - - 5 ",
            "| 6 - - - - - - - - 6 ",
            "| 7 - - - - - - - - 7 ",
            "| 8 - - - - - - - - 8 ",
            "|   A B C D E F G H",
            "|O to move",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut m = MatchState::new();
        apply_block(&mut m, &dealt, "kuroobi");
        let ggf = m.ggf(".1", None);
        let bo = ggf.split_once("BO[8 ").expect("BO tag present").1;
        let discs = bo.chars().take(64).filter(|c| *c != '-').count();
        assert_eq!(
            discs, 6,
            "the 6-disc post-draw board is not the start position: {bo}"
        );
        assert!(
            bo[..66].ends_with(" O"),
            "the side to move is not White: {bo}"
        );
    }

    #[test]
    fn a_board_with_moves_is_not_the_start() {
        let mid: Vec<String> = [
            "|kuroobi  (2300.0 *) 15:00,0:0//02:00,0:0",
            "|  1: F5/1.00/0.00",
            "|   A B C D E F G H",
            "| 1 - - - - - - - - 1 ",
            "| 2 - - - - - - - - 2 ",
            "| 3 - - - - - - - - 3 ",
            "| 4 - - - O * - - - 4 ",
            "| 5 - - - * * * - - 5 ",
            "| 6 - - - - - - - - 6 ",
            "| 7 - - - - - - - - 7 ",
            "| 8 - - - - - - - - 8 ",
            "|   A B C D E F G H",
            "|O to move",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut m = MatchState::new();
        apply_block(&mut m, &mid, "kuroobi");
        let ggf = m.ggf(".1", None);
        let bo = ggf.split_once("BO[8 ").expect("BO tag present").1;
        let discs = bo.chars().take(64).filter(|c| *c != '-').count();
        assert_eq!(discs, 4, "a mid-game board became the start position: {bo}");
    }

    #[test]
    fn a_small_jump_is_still_overtime() {
        let with_clock = |secs: &str| {
            vec![
                format!("|kuroobi  (1720.0 *) {secs}//02:00,0:0"),
                "|  1: F5/1.00/0.00".to_string(),
                "|  2: D6/1.00/0.00".to_string(),
                "|* to move".to_string(),
            ]
        };
        let mut m = MatchState::new();
        apply_block(&mut m, &with_clock("00:50,0:0"), "kuroobi");
        assert!(!m.in_overtime, "still in main time");
        apply_block(&mut m, &with_clock("01:30,0:0"), "kuroobi");
        assert!(m.in_overtime, "missed the 40-second jump");
    }

    #[test]
    fn an_increment_is_not_overtime() {
        let with_clock2 = |secs: &str, inc: &str| {
            vec![
                format!("|kuroobi  (1720.0 *) {secs}/{inc}/02:00,0:0"),
                "|  1: F5/1.00/0.00".to_string(),
                "|  2: D6/1.00/0.00".to_string(),
                "|* to move".to_string(),
            ]
        };
        let mut m = MatchState::new();
        apply_block(&mut m, &with_clock2("05:00,0:0", "0:20"), "kuroobi");
        apply_block(&mut m, &with_clock2("05:15,0:0", "0:20"), "kuroobi");
        assert!(!m.in_overtime, "mistook an increment for overtime");
    }

    #[test]
    fn a_zero_before_any_move_is_not_overtime() {
        let mut m = MatchState::new();
        let b = vec![
            "|kuroobi  (1720.0 *) 00:00,0:0//02:00,0:0".to_string(),
            "|* to move".to_string(),
        ];
        apply_block(&mut m, &b, "kuroobi");
        assert!(!m.in_overtime, "set before any move was played");
    }

    #[test]
    fn a_falling_clock_is_not_overtime() {
        let mut m = MatchState::new();
        for secs in ["15:00,0:0", "14:31,0:0", "13:02,0:0", "00:41,0:0"] {
            let b = vec![
                format!("|kuroobi  (1720.0 *) {secs}//02:00,0:0"),
                "|* to move".to_string(),
            ];
            apply_block(&mut m, &b, "kuroobi");
            assert!(!m.in_overtime, "{secs} was treated as overtime");
        }
    }

    #[test]
    fn offer_real_format() {
        let o = parse_offer(
            ".25 1720.0 kuroobi  15:00//02:00        8 R 1438.6 fly",
            "kuroobi",
        )
        .unwrap();
        assert_eq!(o.id, ".25");
        assert!(!o.incoming);
        assert_eq!(o.names, vec!["kuroobi", "fly"]);
        assert_eq!(o.gtype, "8");
        assert!(o.rated);
        assert_eq!(o.time, "15:00//02:00");
        let o = parse_offer(
            ".31 1438.6 fly  15:00//02:00  s8r16 R 1720.0 kuroobi",
            "kuroobi",
        )
        .unwrap();
        assert!(o.incoming);
        assert_eq!(o.gtype, "s8r16");
    }

    #[test]
    fn block_real_format() {
        let block: Vec<String> = [
            "/os: join .13 8 K?",
            "|0 move(s)",
            "|  0: PASS",
            "|  1: E6",
            "|  2: f4/-25.99/0.20",
            "|kuroobi  (1720.0 *) 14:59,0:0//02:00,0:0",
            "|fly      (1438.6 O) 15:00,0:0//02:00,0:0",
            "|",
            "|   A B C D E F G H",
            "| 1 - - - - - - - - 1 ",
            "| 2 - - - - - - - - 2 ",
            "| 3 - - - - - - - - 3 ",
            "| 4 - - - O * - - - 4 ",
            "| 5 - - - * O - - - 5 ",
            "| 6 - - - - - - - - 6 ",
            "| 7 - - - - - - - - 7 ",
            "| 8 - - - - - - - - 8 ",
            "|   A B C D E F G H",
            "|* to move",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut m = MatchState::new();
        let (rows_ok, turn) = apply_block(&mut m, &block, "kuroobi");
        assert!(rows_ok);
        assert_eq!(turn, Some('*'));
        assert_eq!(m.my_color, Some('*'));
        assert_eq!(m.my_clock_secs, Some(899));
        assert_eq!(m.my_ext, Some(120));
        assert_eq!(m.opp_name, "fly");
        assert_eq!(m.opp_rating, "1438.6");
        assert_eq!(m.opp_secs, Some(900));
        let disc = |f: usize, r: usize| m.cells[f * 8 + r];
        assert_eq!(disc(3, 3), 2); // D4 = O
        assert_eq!(disc(4, 3), 1); // E4 = *
        assert_eq!(disc(3, 4), 1); // D5 = *
        assert_eq!(disc(4, 4), 2); // E5 = O
        assert_eq!(m.cells.iter().filter(|&&c| c != 0).count(), 4);
        assert!(
            !m.moves.contains_key(&0),
            "move 0 was recorded as a played move"
        );
        assert_eq!(m.moves.get(&1).map(|s| s.as_str()), Some("E6"));
        assert_eq!(m.moves.get(&2).map(|s| s.as_str()), Some("f4"));
        assert_eq!(m.kifu(), "e6f4");
    }

    #[test]
    fn watch_join_block_two_boards() {
        let block: Vec<String> = WATCH_JOIN_BLOCK.iter().map(|s| s.to_string()).collect();
        let mut m = MatchState::new();
        let (rows_ok, turn) = apply_block(&mut m, &block, "kuroobi");
        assert!(rows_ok, "parses even with two boards");
        assert_eq!(turn, Some('*'));
        assert_eq!(m.my_color, None);
        assert_eq!(m.players.len(), 2);
        assert_eq!(m.players[0].name, "nyanyan");
        assert_eq!(m.players[0].color, "black");
        assert_eq!(m.players[1].name, "egrcd");
        assert_eq!(m.players[1].color, "white");
        assert_eq!(m.moves.len(), 24);
        let stones = m.cells.iter().filter(|&&c| c != 0).count();
        assert_eq!(stones, 38, "disc count of the current position");

        assert_eq!(m.start_cells.iter().filter(|&&c| c != 0).count(), 14);
        let start = m.start_string();
        assert_eq!(start.len(), 66, "64 squares + space + side to move");
        assert!(
            start.ends_with(" X"),
            "Black is to move in the start position"
        );
        let board = kuroobi::Board::from_string(&start).expect("parses as a board string");
        assert_eq!((board.black | board.white).count_ones(), 14);
    }

    #[test]
    fn ggf_round_trips_a_drawn_opening() {
        let block: Vec<String> = WATCH_JOIN_BLOCK.iter().map(|s| s.to_string()).collect();
        let mut m = MatchState::new();
        apply_block(&mut m, &block, "kuroobi");

        let ggf = m.ggf(".45.0", Some("+4.00"));
        assert!(ggf.starts_with("(;GM[Othello]"));
        assert!(ggf.ends_with(";)"));
        assert!(ggf.contains("PB[nyanyan]"), "Black is nyanyan");
        assert!(ggf.contains("PW[egrcd]"), "White is egrcd");
        assert!(ggf.contains("RE[+4.00]"));
        let bo = ggf
            .split_once("BO[8 ")
            .unwrap()
            .1
            .split_once(']')
            .unwrap()
            .0;
        assert_eq!(bo.len(), 66);
        assert_eq!(
            bo.chars().filter(|c| *c != '-' && *c != ' ').count(),
            14 + 1
        );
        let moves_part = ggf.split_once("BO[").unwrap().1;
        assert_eq!(
            moves_part.matches("B[").count() + moves_part.matches("W[").count(),
            24
        );
        assert!(ggf.contains("B[F3]"), "move 1 is Black F3");
        assert!(ggf.contains("W[D2]"), "move 2 is White D2");

        let start = bo.replace('*', "X");
        let board = kuroobi::Board::from_string(&start).expect("BO parses");
        assert_eq!((board.black | board.white).count_ones(), 14);

        let kifu: String = m.kifu();
        let replayed =
            kuroobi::game::Reversi::from_kifu_with_start(&start, &kifu).expect("replays");
        for (i, &want) in m.cells.iter().enumerate() {
            let bit = 1u64 << i;
            let got = if replayed.board.black & bit != 0 {
                1
            } else if replayed.board.white & bit != 0 {
                2
            } else {
                0
            };
            assert_eq!(got, want, "square {i} does not match");
        }
    }

    #[test]
    fn watch_synchro_keeps_two_boards_apart() {
        let mk = |id: &str, row6: &str| -> Vec<String> {
            [
                &format!("/os: update {id} s8r14 K?"),
                "| 25: B5/12.00/0.02",
                "|nyanyan  (2658.9 *) 00:09,13:0//00:30,13:0",
                "|egrcd    (2585.8 O) 00:16,12:0//00:30,12:0",
                "|",
                "|   A B C D E F G H",
                "| 1 - - - - - - - - 1 ",
                "| 2 - - - - - - - - 2 ",
                "| 3 - - - - - - - - 3 ",
                "| 4 - - - O * - - - 4 ",
                "| 5 - - - * O - - - 5 ",
                row6,
                "| 7 - - - - - - - - 7 ",
                "| 8 - - - - - - - - 8 ",
                "|   A B C D E F G H",
                "|O to move",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect()
        };
        let mut matches: HashMap<String, MatchState> = HashMap::new();
        for (id, row6) in [
            (".56.0", "| 6 - - * - - - - - 6 "),
            (".56.1", "| 6 - - - - * - - - 6 "),
        ] {
            let block = mk(id, row6);
            let mid = block[0].split_whitespace().nth(2).unwrap().to_string();
            let m = matches.entry(mid).or_insert_with(MatchState::new);
            apply_block(m, &block, "kuroobi");
        }
        assert_eq!(matches.len(), 2, "the two games are stored separately");
        assert_eq!(matches[".56.0"].cells[2 * 8 + 5], 1);
        assert_eq!(matches[".56.0"].cells[4 * 8 + 5], 0);
        assert_eq!(matches[".56.1"].cells[2 * 8 + 5], 0);
        assert_eq!(matches[".56.1"].cells[4 * 8 + 5], 1);
        assert!(matches.values().all(|m| m.my_color.is_none()));
        assert_eq!(base_id(".56.0"), ".56");
        assert_eq!(base_id(".56.1"), ".56");
    }

    #[test]
    fn synchro_end_clears_both_boards() {
        let mut matches: HashMap<String, MatchState> = HashMap::new();
        matches.insert(".4.0".into(), MatchState::new());
        matches.insert(".4.1".into(), MatchState::new());
        matches.insert(".9".into(), MatchState::new());
        let dropped = drop_match(&mut matches, ".4");
        assert_eq!(dropped.len(), 2, "both synchro games are collected");
        assert!(!matches.contains_key(".4.0"));
        assert!(!matches.contains_key(".4.1"));
        assert!(matches.contains_key(".9"), "unrelated games are kept");

        let dropped = drop_match(&mut matches, ".9");
        assert_eq!(dropped.len(), 1);
        assert!(matches.is_empty());
    }

    #[test]
    fn drop_match_does_not_touch_similar_ids() {
        let mut matches: HashMap<String, MatchState> = HashMap::new();
        for id in [".4", ".40", ".4.0", ".44.1"] {
            matches.insert(id.into(), MatchState::new());
        }
        drop_match(&mut matches, ".4");
        assert!(!matches.contains_key(".4"));
        assert!(!matches.contains_key(".4.0"));
        assert!(matches.contains_key(".40"));
        assert!(matches.contains_key(".44.1"));
    }

    #[test]
    fn base_id_synchro() {
        assert_eq!(base_id(".82726.1"), ".82726");
        assert_eq!(base_id(".82726.2"), ".82726");
        assert_eq!(base_id(".13"), ".13");
    }
}

#[cfg(test)]
mod who_tests {
    use super::*;

    #[test]
    fn rating_token() {
        assert_eq!(parse_rating_token("1720.0@350.0"), Some(1720.0));
        assert_eq!(parse_rating_token("1938.0@"), Some(1938.0));
        assert_eq!(parse_rating_token("+33.6"), None); // a delta is out of range
        assert_eq!(parse_rating_token("71.7="), None);
    }
}

#[cfg(test)]
mod budget_tests {
    use super::time_budget;
    use std::time::Duration;

    const BASE: (u32, u8, u8) = (22, 26, 6);

    fn cap(secs: Option<u64>, empties: u8, pace: &str) -> Option<Duration> {
        time_budget(
            kuroobi::timectl::Situation {
                clock_secs: secs,
                grace_secs: 120,
                empties,
                ..Default::default()
            },
            BASE,
            pace,
        )
        .3
    }

    #[test]
    fn depth_mode_ignores_the_clock() {
        let (d, solve, band, c) = time_budget(
            kuroobi::timectl::Situation {
                clock_secs: Some(30),
                grace_secs: 120,
                empties: 40,
                ..Default::default()
            },
            BASE,
            "depth",
        );
        assert_eq!((d, solve, band), BASE);
        assert!(c.is_none(), "no deadline");
    }

    #[test]
    fn the_budget_shrinks_with_the_clock() {
        let a = cap(Some(900), 40, "fast").unwrap();
        let b = cap(Some(300), 40, "fast").unwrap();
        let c = cap(Some(60), 40, "fast").unwrap();
        assert!(
            a > b && b > c,
            "less time left means a shorter move: {a:?} > {b:?} > {c:?}"
        );
    }

    #[test]
    fn dropped_paces_fall_back_to_the_default() {
        let fast = cap(Some(900), 50, "fast").unwrap();
        for p in ["slow", "even", ""] {
            assert_eq!(cap(Some(900), 50, p).unwrap(), fast, "{p:?}");
        }
        let even = cap(Some(900), 50, "tail:1.0").unwrap();
        assert!(even > fast, "even split {even:?} > default {fast:?}");
    }

    #[test]
    fn out_of_main_time_plays_fast() {
        let c = cap(Some(0), 30, "fast").unwrap();
        assert!(c <= Duration::from_secs(1), "{c:?}");
    }

    #[test]
    fn the_endgame_keeps_a_reserve() {
        let (_, _, _, c) = time_budget(
            kuroobi::timectl::Situation {
                clock_secs: Some(100),
                empties: 40,
                ..Default::default()
            },
            BASE,
            "tail:1.0",
        );
        let b = c.unwrap().as_secs_f64();
        assert!(
            b <= 80.0,
            "the per-move deadline exceeded the share available (80s): {b:.1}"
        );
        assert!(b > 0.0);
    }
}

#[cfg(test)]
mod play_arg_tests {
    use super::play_arg;
    use std::time::Duration;

    #[test]
    fn a_move_carries_its_score_and_seconds() {
        let s = play_arg("D8", 6.0, Some(Duration::from_millis(17_390)));
        assert_eq!(s, "D8/6.00/17.39");
    }

    #[test]
    fn a_negative_score_keeps_its_sign() {
        let s = play_arg("F5", -3.5, Some(Duration::from_millis(1_200)));
        assert_eq!(s, "F5/-3.50/1.20");
    }

    #[test]
    fn a_pass_goes_bare() {
        assert_eq!(play_arg("pa", 12.0, Some(Duration::from_secs(3))), "pa");
    }

    #[test]
    fn a_non_finite_score_becomes_zero() {
        let s = play_arg("A1", f32::INFINITY, Some(Duration::from_secs(1)));
        assert_eq!(s, "A1/0.00/1.00");
    }

    #[test]
    fn an_unmeasured_move_omits_the_time() {
        assert_eq!(play_arg("C4", 2.0, None), "C4/2.00");
    }
}
