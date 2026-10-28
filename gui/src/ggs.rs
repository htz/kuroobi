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

use crate::{append_line, now_secs};

pub enum Cmd {
    Connect {
        login: String,
        pw: String,
        /// `host:port`; empty means the public server.
        server: String,
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
    ReloadThreads,
    SetAutoPlay(bool),
    SetOpen(bool),
    SetWatchAnalysis(bool),
    SetLogChannel(String),
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

impl StandbyCfg {
    /// Without an opponent it never asks and only accepted whatever arrived, which the server-side formula already does.
    fn needing_opponent(mut self) -> StandbyCfg {
        self.opponent = self.opponent.trim().to_string();
        self.enabled &= !self.opponent.is_empty();
        self
    }
}

#[derive(Clone, Serialize, Default)]
pub struct LogLine {
    pub seq: u64,
    pub dir: &'static str, // "in" | "out" | "info"
    pub text: String,
}

#[derive(Clone, Serialize, Default)]
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

#[derive(Clone, Serialize, Default)]
pub struct FingerInfo {
    pub fields: Vec<(String, String)>,
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

#[derive(Clone, Serialize, Default)]
pub struct StoredView {
    pub id: String,
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
    /// When the request was first heard of (epoch ms), for "sent N min ago".
    pub since_ms: u64,
}

#[derive(Clone, Serialize, Default)]
pub struct EvalPoint {
    pub n: u32,
    pub mine: bool,
    pub eval: Option<f32>,
    pub secs: Option<f32>,
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
    pub seen: u64,
    /// Arrival of the last server update (epoch ms); the screen runs the clock from here.
    pub updated_ms: u64,
    /// When our search for this board began (epoch ms); our clock runs from here, not from the update.
    pub think_since_ms: u64,
    /// Our move is waiting for the other board's search; the server charges none of the wait.
    pub think_queued: bool,
    /// Squares the side to move can play, `file * 8 + rank`.
    pub legal: Vec<u32>,
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
    /// Every board of the match; `kifu` and `ggf` above hold only the first, which lost half of each synchro pair.
    #[serde(default)]
    pub boards: Vec<BoardRecord>,
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct BoardRecord {
    pub id: String,
    pub kifu: String,
    pub ggf: String,
}

#[derive(Clone, Serialize, Default)]
pub struct StandbyStats {
    pub games: usize,
    pub wins: usize,
    pub losses: usize,
    pub draws: usize,
    pub diff_sum: i32,
}

#[derive(Clone, Copy, Serialize)]
pub struct SearchStat {
    pub nodes: u64,
    pub secs: f64,
}

#[derive(Clone, Serialize, Default)]
pub struct Snapshot {
    pub conn: String, // disconnected | connecting | logging_in | online
    pub login: String,
    /// `host:port` of the current session.
    pub server: String,
    #[serde(skip)]
    pub my_rating: Option<f32>,
    pub my_ranks: Vec<RankRow>,
    pub log: VecDeque<LogLine>,
    pub users: Vec<UserRow>,
    pub ranking: Vec<UserRow>,
    pub fingers: HashMap<String, FingerInfo>,
    pub offers: Vec<Offer>,
    pub matches: Vec<MatchView>,
    /// Who is watching each match we play or watch, by match id (`.7`; both boards of a synchro
    /// pair share it), in name order.
    pub watchers: HashMap<String, Vec<String>>,
    pub ongoing: Vec<OngoingView>,
    /// Everyone logged in, or `None` until the server's `who` has answered once.
    pub online: Option<Vec<String>>,
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
    /// The server's `open` flag: whether anyone may send us a request at all.
    pub open: bool,
    pub watch_analysis: bool,
    /// Empty means the search log goes nowhere.
    pub log_channel: String,
    pub thinking: Option<String>,
    /// The last move kuroobi searched in a GGS game, for the status bar.
    pub last_search: Option<SearchStat>,
    pub fetched_ggf: Option<FetchedGgf>,
    pub td: TdView,
}

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct TourneyView {
    pub id: u32,
    /// announced | running | break | over | cancelled
    pub state: String,
    pub desc: String,
    pub director: String,
    pub clock: String,
    pub gtype: String,
    pub players: Vec<String>,
    pub n_players: usize,
    pub rounds_played: u32,
    pub rounds: u32,
    /// Unix seconds of the next start (tournament or round), when known.
    pub next_at: Option<u64>,
    pub joined: bool,
    pub winner: String,
    /// Latest raw `finger`, `rankings` and full-schedule replies for this tournament.
    pub info: String,
    pub rankings: String,
    pub schedule: String,
}

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct TdReply {
    pub cmd: String,
    pub text: String,
    pub err: bool,
    pub at: u64,
}

#[derive(Clone, Serialize, Default)]
pub struct TdView {
    pub tourneys: Vec<TourneyView>,
    pub reply: Option<TdReply>,
    /// Our matches the director started (`+ match ... T`), newest last.
    pub games: Vec<TdGame>,
}

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct TdGame {
    pub id: String,
    pub tid: u32,
    pub opp: String,
    /// Disc difference for us, summed over both boards of a synchro pair.
    pub diff: Option<i32>,
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
            watching: true,
            names: vec!["tamaki".into(), "momo-bot".into()],
            ratings: vec!["2011.4".into(), "2280.1".into()],
            gtype: "s8r14".into(),
            mine: false,
        },
        OngoingView {
            id: ".72.0".into(),
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
            since_ms: epoch_ms() - 120_000,
        },
        Offer {
            id: "2".into(),
            raw: "+ .2 tamaki 2011.4 8 10:00".into(),
            incoming: false,
            names: vec!["tamaki".into(), "nara".into()],
            gtype: "8".into(),
            time: "00:10:00".into(),
            rated: false,
            since_ms: epoch_ms() - 45_000,
        },
    ];
    s.stored = vec![StoredView {
        id: "3".into(),
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
            seq: 0,
            dir: "info",
            text: "connected (skatgame.net:5000)".into(),
        },
        LogLine {
            seq: 1,
            dir: "out",
            text: "tell /os play .71.0 F5".into(),
        },
        LogLine {
            seq: 2,
            dir: "in",
            text: "/os: match .71.0 update".into(),
        },
        LogLine {
            seq: 3,
            dir: "in",
            text: "/os: | 1 kuroobi 1842.3 11:04 vs saio 2245.8 12:30".into(),
        },
        LogLine {
            seq: 4,
            dir: "out",
            text: "tell /os look".into(),
        },
        LogLine {
            seq: 5,
            dir: "in",
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
    let finger = |accept: &str| FingerInfo {
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
    };
    s.fingers.insert(
        "kuroobi".into(),
        finger("rand&discs>=14&discs<=20&mt1>=120"),
    );
    s.fingers.insert("saio".into(), finger("rand&or>=1600"));
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
    pub threads: usize,
    pub use_book: bool,
    pub book_loaded: bool,
    pub learn: bool,
}

impl Default for EngineCfgView {
    fn default() -> Self {
        EngineCfgView {
            threads: 4,
            use_book: true,
            book_loaded: false,
            learn: true,
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
            gtype: "s8r14".into(),
            time: "00:05:00".into(),
            max_games: 0,
            interval_secs: 20,
        },
        auto_play: true,
        open: true,
        watch_analysis: true,
        results: load_history(),
        ..Default::default()
    }));
    let snap2 = snapshot.clone();
    std::thread::spawn(move || run(app, rx, snap2, local_stop, local_activity));
    Handle { tx, snapshot }
}

pub const DEFAULT_SERVER: &str = "skatgame.net:5000";
static SERVER: Mutex<String> = Mutex::new(String::new());

fn server() -> String {
    let s = SERVER.lock().unwrap();
    if s.is_empty() {
        DEFAULT_SERVER.to_string()
    } else {
        s.clone()
    }
}

/// Test games must not be learned into the shared book: only a server marked public counts.
fn local_server() -> bool {
    let addr = server();
    !saved_servers().iter().any(|s| s.addr == addr && s.public)
}

/// `ggs_games/`, holding the account store and one folder per account.
fn base_dir() -> PathBuf {
    for c in ["ggs_games", "../ggs_games", "../../ggs_games"] {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from(".")
}

fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The account whose history, chat and wire log are read and written.
static USER: Mutex<Option<(String, String)>> = Mutex::new(None);

fn user_dir(server: &str, login: &str) -> PathBuf {
    base_dir()
        .join("users")
        .join(safe_name(server))
        .join(safe_name(login))
}

fn set_user(server: &str, login: &str) {
    let dir = user_dir(server, login);
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
        adopt_legacy_data(server, login, &dir);
    }
    *USER.lock().unwrap() = Some((server.to_string(), login.to_string()));
}

/// Data written before per-account folders belongs to the public account whose chat file it kept.
fn adopt_legacy_data(server: &str, login: &str, dir: &std::path::Path) {
    if server != DEFAULT_SERVER {
        return;
    }
    let base = base_dir();
    let chat = base
        .join("chat")
        .join(format!("{}.jsonl", safe_name(login)));
    if !chat.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(dir.join("chat"));
    for f in [chat.clone(), chat.with_extension("seen")] {
        let _ = std::fs::rename(&f, dir.join("chat").join(f.file_name().unwrap()));
    }
    for f in ["history.jsonl", "ggs_wire.log", "ggs_wire.log.1"] {
        let _ = std::fs::rename(base.join(f), dir.join(f));
    }
}

fn data_dir() -> PathBuf {
    match &*USER.lock().unwrap() {
        Some((srv, login)) => user_dir(srv, login),
        None => base_dir(),
    }
}

fn history_path() -> PathBuf {
    data_dir().join("history.jsonl")
}

#[derive(Clone, Serialize, serde::Deserialize, PartialEq, Default)]
pub struct SavedAccount {
    pub server: String,
    pub login: String,
    /// Unix seconds of the last successful login.
    #[serde(default)]
    pub last: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
}

/// A server the user has named; `public` marks the real one, whose games teach the book.
#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct SavedServer {
    pub addr: String,
    pub name: String,
    #[serde(default)]
    pub public: bool,
}

#[derive(Serialize, serde::Deserialize, Default)]
struct AccountStore {
    #[serde(default)]
    servers: Vec<SavedServer>,
    accounts: Vec<SavedAccount>,
    /// `server|login` to log in as at startup; cleared by logging out.
    #[serde(default)]
    auto: Option<String>,
}

// The `.ggs_credentials` prefix keeps the file under the repo's credential ignore rule.
fn store_path() -> PathBuf {
    base_dir().join(".ggs_credentials.accounts.json")
}

fn key_of(server: &str, login: &str) -> String {
    format!("{server}|{login}")
}

fn load_store() -> AccountStore {
    if let Some(s) = std::fs::read_to_string(store_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
    {
        return s;
    }
    let s = import_old_credentials();
    write_store(&s);
    s
}

fn write_store(s: &AccountStore) {
    let Ok(text) = serde_json::to_string_pretty(s) else {
        return;
    };
    let p = store_path();
    if std::fs::write(&p, text).is_ok() {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
}

/// One-time pickup of the login the keychain or `.ggs_credentials` held before the store existed.
fn import_old_credentials() -> AccountStore {
    let run = |args: &[&str]| {
        std::process::Command::new("/usr/bin/security")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };
    let keychain = || -> Option<(String, String)> {
        let meta = run(&["find-generic-password", "-s", "kuroobi-ggs"])?;
        let login = meta.lines().find_map(|l| {
            l.trim()
                .strip_prefix("\"acct\"<blob>=\"")?
                .strip_suffix('"')
                .map(str::to_string)
        })?;
        let pw = run(&["find-generic-password", "-s", "kuroobi-ggs", "-w"])?;
        let pw = pw.trim_end_matches('\n').to_string();
        (!pw.is_empty() && login != "-").then_some((login, pw))
    };
    let mut s = AccountStore::default();
    if let Some((login, pw)) = keychain().or_else(crate::read_credentials) {
        s.auto = Some(key_of(DEFAULT_SERVER, &login));
        s.accounts.push(SavedAccount {
            server: DEFAULT_SERVER.to_string(),
            login,
            last: now_secs(),
            password: pw,
        });
    }
    s
}

/// Most recently used first, without passwords.
pub fn saved_accounts() -> Vec<SavedAccount> {
    let mut list: Vec<SavedAccount> = load_store()
        .accounts
        .into_iter()
        .map(|a| SavedAccount {
            password: String::new(),
            ..a
        })
        .collect();
    list.sort_by_key(|a| std::cmp::Reverse(a.last));
    list
}

pub fn account_password(server: &str, login: &str) -> Option<String> {
    load_store()
        .accounts
        .into_iter()
        .find(|a| a.server == server && a.login == login && !a.password.is_empty())
        .map(|a| a.password)
}

/// The account to log in as at startup: `(server, login, password)`.
pub fn auto_account() -> Option<(String, String, String)> {
    let s = load_store();
    let key = s.auto?;
    s.accounts
        .into_iter()
        .find(|a| key_of(&a.server, &a.login) == key && !a.password.is_empty())
        .map(|a| (a.server, a.login, a.password))
}

pub fn clear_auto() {
    let mut s = load_store();
    s.auto = None;
    write_store(&s);
}

fn remember_account(server: &str, login: &str, pw: &str) {
    let mut s = load_store();
    s.accounts
        .retain(|x| !(x.server == server && x.login == login));
    s.accounts.push(SavedAccount {
        server: server.to_string(),
        login: login.to_string(),
        last: now_secs(),
        password: pw.to_string(),
    });
    s.auto = Some(key_of(server, login));
    write_store(&s);
}

/// The named servers: the public one first, then every server an account was saved on.
pub fn saved_servers() -> Vec<SavedServer> {
    let s = load_store();
    let mut out = s.servers.clone();
    if !out.iter().any(|x| x.addr == DEFAULT_SERVER) {
        out.insert(
            0,
            SavedServer {
                addr: DEFAULT_SERVER.to_string(),
                name: "GGS".to_string(),
                public: true,
            },
        );
    }
    for a in &s.accounts {
        if !out.iter().any(|x| x.addr == a.server) {
            out.push(SavedServer {
                addr: a.server.clone(),
                name: a.server.clone(),
                public: false,
            });
        }
    }
    out
}

/// Adds or edits a server; when its address changes, its accounts move with it.
pub fn save_server(old: &str, name: &str, addr: &str, public: bool) -> Result<(), String> {
    let addr = addr.trim();
    if addr.is_empty() || !addr.contains(':') {
        return Err("err.server_address".to_string());
    }
    let mut s = load_store();
    let mut list = saved_servers();
    if addr != old && list.iter().any(|x| x.addr == addr) {
        return Err("err.server_exists".to_string());
    }
    let entry = SavedServer {
        addr: addr.to_string(),
        name: if name.trim().is_empty() {
            addr.to_string()
        } else {
            name.trim().to_string()
        },
        public,
    };
    match list.iter_mut().find(|x| !old.is_empty() && x.addr == old) {
        Some(x) => *x = entry,
        None => list.push(entry),
    }
    if !old.is_empty() && old != addr {
        for a in s.accounts.iter_mut().filter(|a| a.server == old) {
            a.server = addr.to_string();
        }
        if let Some(k) = s.auto.as_mut() {
            if let Some(login) = k.strip_prefix(&format!("{old}|")) {
                *k = key_of(addr, login);
            }
        }
    }
    s.servers = list;
    write_store(&s);
    Ok(())
}

/// Removes a server and the accounts saved on it.
pub fn forget_server(addr: &str) {
    let mut s = load_store();
    s.servers = saved_servers()
        .into_iter()
        .filter(|x| x.addr != addr)
        .collect();
    s.accounts.retain(|a| a.server != addr);
    if s.auto
        .as_deref()
        .is_some_and(|k| k.starts_with(&format!("{addr}|")))
    {
        s.auto = None;
    }
    write_store(&s);
}

/// The account to log in as at startup, as `(server, login)`.
pub fn auto_key() -> Option<(String, String)> {
    let k = load_store().auto?;
    let (server, login) = k.split_once('|')?;
    Some((server.to_string(), login.to_string()))
}

pub fn set_auto(server: &str, login: &str) {
    let mut s = load_store();
    s.auto = (!login.is_empty()).then(|| key_of(server, login));
    write_store(&s);
}

pub fn forget_account(server: &str, login: &str) {
    let mut s = load_store();
    s.accounts
        .retain(|x| !(x.server == server && x.login == login));
    if s.auto.as_deref() == Some(&key_of(server, login)) {
        s.auto = None;
    }
    write_store(&s);
}

/// The line is signed for the name written first; a synchro match scores the mean of two boards.
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
        append_line(&history_path(), &line);
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
    let dir = data_dir().join("chat");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{safe}.jsonl"))
}

const CHAT_KEEP: usize = 5000;
// Per-thread trimming can leave the file above the fixed floor, so the next trim waits for growth past it.
static CHAT_TRIMMED_LEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn load_chat(login: &str) -> Vec<ChatMsg> {
    let Ok(text) = std::fs::read_to_string(chat_path(login)) else {
        return Vec::new();
    };
    let out: Vec<ChatMsg> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<ChatMsg>(l).ok())
        .collect();
    keep_latest_per_thread(out, CHAT_KEEP)
}

fn settings_path() -> PathBuf {
    base_dir().join("ggs_settings.json")
}

/// A missing field falls back to its default instead of failing the read and wiping settings.
#[derive(Serialize, serde::Deserialize)]
#[serde(default)]
struct SavedSettings {
    auto_play: bool,
    open: bool,
    watch_analysis: bool,
    log_channel: String,
    use_book: bool,
    learn: bool,
}

impl Default for SavedSettings {
    fn default() -> Self {
        SavedSettings {
            auto_play: true,
            open: true,
            watch_analysis: true,
            log_channel: String::new(),
            use_book: true,
            learn: true,
        }
    }
}

fn save_settings(ctx: &Ctx) {
    let s = ctx.snap.lock().unwrap();
    let e = &s.engine;
    let v = SavedSettings {
        auto_play: s.auto_play,
        open: s.open,
        watch_analysis: s.watch_analysis,
        log_channel: s.log_channel.clone(),
        use_book: e.use_book,
        learn: e.learn,
    };
    drop(s);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(settings_path(), text);
    }
}

fn wire_path() -> PathBuf {
    data_dir().join("ggs_wire.log")
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
    append_line(&path, &line);
    if let Ok(meta) = std::fs::metadata(&path) {
        let floor = (CHAT_KEEP as u64) * 120 * 12 / 10;
        let last = CHAT_TRIMMED_LEN.load(std::sync::atomic::Ordering::Relaxed);
        if meta.len() > floor.max(last * 12 / 10) {
            let kept = load_chat(login);
            let body: String = kept
                .iter()
                .filter_map(|m| serde_json::to_string(m).ok())
                .map(|l| l + "\n")
                .collect();
            let tmp = path.with_extension("jsonl.tmp");
            if std::fs::write(&tmp, &body).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
                CHAT_TRIMMED_LEN.store(body.len() as u64, std::sync::atomic::Ordering::Relaxed);
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

fn other_session_pid(path: &std::path::Path) -> Option<i32> {
    let pid = std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()?;
    (pid != std::process::id() as i32 && unsafe { libc::kill(pid, 0) } == 0).then_some(pid)
}

pub fn session_locked_by_other() -> bool {
    other_session_pid(&session_lock_path()).is_some()
}

fn try_lock_session() -> Result<(), i32> {
    let path = session_lock_path();
    if let Some(pid) = other_session_pid(&path) {
        return Err(pid);
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

#[derive(Clone)]
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
    watch_hash: u64,
    last_played_hash: u64, // double-move protection
    seen: u64,
    updated_ms: u64,
    order: u64,
    over: bool,
    result: String,
}

fn is_pass(mv: &str) -> bool {
    mv.eq_ignore_ascii_case("pa") || mv.eq_ignore_ascii_case("pass")
}

impl MatchState {
    fn eval_series(&self) -> Vec<EvalPoint> {
        let Some(&last_n) = self.moves.keys().next_back() else {
            return Vec::new();
        };
        let mine_parity = match self.eval_parity {
            Some(p) => p,
            None => {
                // An observer sees the series from black's side.
                let mc = self.my_color.unwrap_or('*');
                if self.turn != '*' && self.turn != 'O' {
                    // Over (no side to move): count from who moved first. GGS numbers a pass
                    // like a move, so the sides alternate by move number.
                    match self.start_turn {
                        '*' | 'O' => {
                            if self.start_turn == mc {
                                1
                            } else {
                                0
                            }
                        }
                        _ => return Vec::new(),
                    }
                } else if self.turn != mc {
                    last_n % 2
                } else {
                    (last_n + 1) % 2
                }
            }
        };
        // GGS numbers a pass like a move; the series counts only the discs placed.
        (1..=last_n)
            .filter(|n| !self.moves.get(n).is_some_and(|m| is_pass(m)))
            .enumerate()
            .map(|(i, n)| EvalPoint {
                n: i as u32 + 1,
                mine: (n % 2) == mine_parity,
                eval: self.move_evals.get(&n).and_then(|&(ev, _)| ev),
                secs: self.move_evals.get(&n).and_then(|&(_, s)| s),
            })
            .collect()
    }

    fn snapshot(&self) -> MatchState {
        MatchState {
            told_turn: false,
            ..self.clone()
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
            watch_hash: 0,
            last_played_hash: 0,
            seen: 0,
            updated_ms: 0,
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
        self.ggf_with(id, result, false)
    }

    fn ggf_with(&self, id: &str, result: Option<&str>, evals: bool) -> String {
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
        for (n, mv) in &self.moves {
            let tag = if black { "B" } else { "W" };
            let mut m = if mv.eq_ignore_ascii_case("pa") || mv.eq_ignore_ascii_case("pass") {
                "PA".to_string()
            } else {
                mv.to_uppercase()
            };
            if let Some(&(Some(ev), secs)) = self.move_evals.get(n).filter(|_| evals) {
                m.push_str(&format!("/{ev:.2}"));
                if let Some(t) = secs {
                    m.push_str(&format!("/{t:.2}"));
                }
            }
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
        let secs = p
            .split(':')
            .try_fold(0u64, |acc, seg| Some(acc * 60 + seg.parse::<u64>().ok()?));
        if let Some(secs) = secs {
            out[i] = Some(if neg { 0 } else { secs });
        }
    }
    (out[0], out[1], out[2])
}

pub(crate) const LEARN_DEPTH: u32 = 18;

fn base_id(id: &str) -> &str {
    match id.rsplit_once('.') {
        Some((head, tail)) if tail.len() == 1 && head.contains('.') => head,
        _ => id,
    }
}

fn boards_of(matches: &HashMap<String, MatchState>, id: &str) -> Vec<String> {
    matches
        .keys()
        .filter(|k| k.as_str() == id || base_id(k) == id)
        .cloned()
        .collect()
}

fn drop_match(matches: &mut HashMap<String, MatchState>, id: &str) -> Vec<MatchState> {
    let keys = boards_of(matches, id);
    keys.iter().filter_map(|k| matches.remove(k)).collect()
}

/// A board's final disc difference from black's side, given the score from `first`'s side.
/// Black is unknown only before the board was seen; the score is then kept as written.
fn black_view(m: &MatchState, first: &str, v: f32) -> String {
    let first_is_white = m
        .players
        .iter()
        .any(|p| p.name == first && p.color == "white");
    format!("{:+.2}", if first_is_white { -v } else { v })
}

fn finish_match(
    matches: &mut HashMap<String, MatchState>,
    id: &str,
    first: &str,
    result: Option<f32>,
    ended: &str,
    left_by: &str,
    archive: &str,
) -> Vec<(String, MatchState)> {
    let mut out = Vec::new();
    for k in boards_of(matches, id) {
        if let Some(m) = matches.get_mut(&k) {
            m.over = true;
            m.turn = ' ';
            if !archive.is_empty() {
                m.archive = archive.to_string();
            }
            if m.result.is_empty() {
                if let Some(v) = result {
                    m.result = black_view(m, first, v);
                }
            }
            if !ended.is_empty() {
                m.ended = ended.to_string();
                m.left_by = left_by.to_string();
            }
            out.push((k, m.snapshot()));
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
    sent_at: Option<(Instant, Duration)>,
    stopped_at: Option<Instant>,
    progress: std::sync::Arc<kuroobi::engine::Progress>,
    threads: usize,
    sent_ms: u64,
    sent_empties: u8,
}

const WORKER_GIVE_UP: Duration = Duration::from_secs(10);

struct Pending {
    board: Board,
    clock: Duration,
    hash: u64,
    hint: Option<Position>,
    queued: Instant,
}

enum Job {
    Think {
        board: Board,
        clock: Duration,
        hint: Option<Position>,
    },
    NewGame,
    SetUseBook(bool),
    SetThreads(usize),
}

struct Report {
    nodes: u64,
    pv: Vec<Option<Position>>,
}

/// Edax's log shows eight plies.
const PV_LEN: usize = 8;

fn report(
    engine: &kuroobi::engine::Engine,
    board: &Board,
    mv: &kuroobi::engine::MoveEval,
    nodes: u64,
) -> Report {
    let pv = match mv.pos {
        Some(p) if !mv.from_book => engine.principal_variation(board, p, PV_LEN),
        _ => vec![mv.pos],
    };
    Report { nodes, pv }
}

enum Done {
    Moved(Box<kuroobi::engine::MoveEval>, Report),
}

struct Ctx {
    app: tauri::AppHandle,
    stop: Option<kuroobi::midgame::StopHandle>,
    snap: Arc<Mutex<Snapshot>>,
    engine: Option<Engine>,
    engine_cfg: EngineConfig,
    seq: u64,
    last_emit: Instant,
    last_sent: u64,
    /// `rating_update` arrives before the result line it belongs to.
    post_ratings: HashMap<String, f32>,
    dirty: bool,
    auto_watch: Vec<String>,
    rated_by_base: HashMap<String, bool>,
    /// Observers per match id: seeded once by `watch .id`, then kept by the server's notices.
    watchers: HashMap<String, std::collections::BTreeSet<String>>,
    /// Match ids whose observers have been asked for since we joined them.
    watch_asked: std::collections::HashSet<String>,
    learn_jobs: VecDeque<(String, kuroobi::learn::BackupJob, crate::LearnEntry)>,
    local_stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    local_activity: Arc<Mutex<crate::Activity>>,
    pending_watch: HashMap<String, Instant>,
    workers: Vec<EngineWorker>,
    /// A login asked for while a session was up; the session ends and this one starts.
    reconnect: Option<(String, String, String)>,
    /// Request ids we cancelled ourselves, so their removal is not taken as a refusal.
    cancelled: std::collections::HashSet<String>,
    /// The auto-match opponent took our request away this many times in a row without a game.
    ask_refusals: u32,
    /// A game with the auto-match opponent started since the last ask.
    ask_matched: bool,
    /// Auto match has just sent an ask and the server has not answered it yet.
    auto_asked: bool,
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
            self.workers[i].send(Job::NewGame);
            return Some(i);
        }
        if self.workers.len() < MAX_WORKERS {
            match EngineWorker::spawn(self.engine_cfg.clone()) {
                Ok(mut w) => {
                    w.mid = Some(mid.to_string());
                    self.workers.push(w);
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
            self.workers[i].send(Job::NewGame);
            return Some(i);
        }
        None
    }

    /// Loading a worker takes seconds the server charges to our first move, so it happens at login.
    fn prepare_workers(&mut self) {
        if let Err(e) = self.ensure_engine() {
            self.log("info", &format!("engine init failed: {e}"));
        }
        while self.workers.len() < MAX_WORKERS {
            match EngineWorker::spawn(self.engine_cfg.clone()) {
                Ok(w) => self.workers.push(w),
                Err(e) => {
                    self.log("info", &format!("cannot spawn search worker: {e}"));
                    return;
                }
            }
        }
    }

    fn total_threads(&self) -> usize {
        resolve_threads(self.engine_cfg.threads)
    }

    fn set_worker_threads(&mut self, i: usize, n: usize) {
        if self.workers[i].threads != n {
            self.workers[i].send(Job::SetThreads(n));
            self.workers[i].threads = n;
        }
    }

    fn thinking(&self) -> bool {
        self.workers.iter().any(|w| w.busy)
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
            }
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
                    Job::Think { board, clock, hint } => {
                        if let Some(h) = hint {
                            engine.hint_move(&board, h);
                        }
                        let nf0 = kuroobi::engine::NON_FINITE_VALUES
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let n0 = engine.nodes();
                        let mv = engine.play_clock(&board, clock);
                        let nodes = engine.nodes() - n0;
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
                        engine.progress().clear();
                        let r = report(&engine, &board, &mv, nodes);
                        if dtx.send(Done::Moved(Box::new(mv), r)).is_err() {
                            return;
                        }
                    }
                    Job::NewGame => engine.clear_tables(),
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
            sent_at: None,
            stopped_at: None,
            progress,
            threads: 0,
            sent_ms: 0,
            sent_empties: 0,
        })
    }

    fn send(&mut self, job: Job) {
        let counts = matches!(job, Job::Think { .. });
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
    fn log(&mut self, dir: &'static str, text: &str) {
        let p = std::env::var("KUROOBI_GGS_WIRE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| wire_path());
        roll_if_large(&p);
        append_line(&p, &format!("{dir} {text}"));
        let mut s = self.snap.lock().unwrap();
        let seq = s.log.back().map_or(0, |l| l.seq + 1);
        s.log.push_back(LogLine {
            seq,
            dir,
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
        let sent = serde_json::to_vec(&s).map_or(0, |b| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            b.hash(&mut h);
            h.finish()
        });
        if !force && sent == self.last_sent {
            self.last_emit = Instant::now();
            self.dirty = false;
            return;
        }
        self.last_sent = sent;
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
    rating_for: Option<String>,
    chat_block: Option<ChatMsg>,
    td_block: Vec<String>,
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
            rating_for: None,
            chat_block: None,
            td_block: Vec::new(),
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
    let res = resources();
    let ctx = Ctx {
        app,
        stop: None,
        snap,
        engine: None,
        engine_cfg: EngineConfig {
            threads: res.threads.unwrap_or_else(|| resolve_threads(0)),
            midgame_hash_bits: res.hash_mid_bits(),
            solver_hash_bits: res.hash_end_bits(),
            ..Default::default()
        },
        seq: 0,
        last_emit: Instant::now(),
        last_sent: 0,
        post_ratings: HashMap::new(),
        dirty: true,
        auto_watch: Vec::new(),
        rated_by_base: HashMap::new(),
        watchers: HashMap::new(),
        watch_asked: std::collections::HashSet::new(),
        learn_jobs: VecDeque::new(),
        local_stop,
        local_activity,
        pending_watch: HashMap::new(),
        reconnect: None,
        workers: Vec::new(),
        cancelled: std::collections::HashSet::new(),
        ask_refusals: 0,
        ask_matched: false,
        auto_asked: false,
    };
    ctx.snap.lock().unwrap().engine = EngineCfgView {
        threads: ctx.engine_cfg.threads,
        use_book: ctx.engine_cfg.use_book,
        book_loaded: res.book_path().exists(),
        ..Default::default()
    };
    ctx
}

fn restore_settings(ctx: &mut Ctx) {
    let Some(v) = load_settings() else { return };
    ctx.set_use_book(v.use_book);
    let mut s = ctx.snap.lock().unwrap();
    s.engine.learn = v.learn;
    s.auto_play = v.auto_play;
    s.open = v.open;
    s.watch_analysis = v.watch_analysis;
    if valid_log_channel(&v.log_channel) {
        s.log_channel = v.log_channel;
    }
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
        session(&mut ctx, &rx, &login, &pw);
        unlock_session();
        {
            let mut s = ctx.snap.lock().unwrap();
            s.conn = "disconnected".into();
            s.offers.clear();
            s.matches.clear();
            s.watchers.clear();
            s.thinking = None;
        }
        ctx.watchers.clear();
        ctx.watch_asked.clear();
        ctx.emit(true);
    }
}

fn wait_for_connect(ctx: &mut Ctx, rx: &Receiver<Cmd>) -> Option<(String, String)> {
    if let Some((login, pw, server)) = ctx.reconnect.take() {
        *SERVER.lock().unwrap() = server.trim().to_string();
        return Some((login, pw));
    }
    loop {
        match rx.recv_timeout(Duration::from_millis(300)) {
            Ok(Cmd::Connect { login, pw, server }) => {
                *SERVER.lock().unwrap() = server.trim().to_string();
                return Some((login, pw));
            }
            Ok(Cmd::ReloadThreads) => apply_threads(ctx),
            Ok(Cmd::SetStandby(cfg)) => ctx.snap.lock().unwrap().standby = cfg.needing_opponent(),
            Ok(Cmd::SetAutoPlay(b)) => {
                ctx.snap.lock().unwrap().auto_play = b;
                save_settings(ctx);
            }
            Ok(Cmd::SetOpen(b)) => {
                ctx.snap.lock().unwrap().open = b;
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
            Ok(Cmd::SetLogChannel(c)) if valid_log_channel(&c) => {
                ctx.snap.lock().unwrap().log_channel = c;
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

/// Sleeps before the next dial, but a disconnect or a login elsewhere ends the wait;
/// true when the session should end.
fn wait_redial(ctx: &mut Ctx, rx: &Receiver<Cmd>) -> bool {
    let until = Instant::now() + Duration::from_secs(15);
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left.min(Duration::from_millis(300))) {
            Ok(Cmd::Disconnect) => return true,
            Ok(Cmd::Connect { login, pw, server }) => {
                ctx.reconnect = Some((login, pw, server));
                return true;
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return true,
        }
    }
    false
}

fn dial(ctx: &mut Ctx) -> Option<(TcpStream, TcpStream)> {
    match TcpStream::connect(server()) {
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
            s.server = server();
            drop(s);
            set_user(&server(), login);
            let mut s = ctx.snap.lock().unwrap();
            s.results = load_history();
            // The account's folder is known only from here on.
            s.chat = keep_latest_per_thread(load_chat(login), CHAT_IN_MEMORY).into();
            s.chat_seen = load_chat_seen(login);
        }
        ctx.emit(true);
        let Some((mut stream, mut writer)) = dial(ctx) else {
            if wait_redial(ctx, rx) {
                return;
            }
            continue;
        };
        ctx.snap.lock().unwrap().conn = "logging_in".into();
        ctx.emit(true);

        let mut w = Wire::new();
        loop {
            // Stop at a quit so a Connect queued behind it reaches wait_for_connect.
            while !w.want_quit {
                let Ok(cmd) = rx.try_recv() else {
                    break;
                };
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
                if w.login_warned {
                    return;
                }
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

/// A server that has not asked for the login by now never will (a wrong port answers
/// nothing), and retrying forever left the window stuck on it.
const LOGIN_GIVE_UP: Duration = Duration::from_secs(20);

fn login_step(ctx: &mut Ctx, w: &mut Wire, login: &str, pw: &str, mut send: impl FnMut(String)) {
    if !w.login_warned && w.login_started.elapsed() > LOGIN_GIVE_UP {
        w.login_warned = true;
        ctx.log(
            "info",
            &format!(
                "login did not finish in {}s; giving up. Check that {} is a GGS \
                 server, and that no other process is connected with the same \
                 account (GGS rejects duplicate logins)",
                LOGIN_GIVE_UP.as_secs(),
                server()
            ),
        );
        ctx.notify(
            &crate::i18n::t("backend.notify.login_stalled_title"),
            &crate::i18n::t("backend.notify.login_stalled_body"),
        );
        ctx.emit(true);
        return;
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

fn ask_who(pending: &mut Vec<String>, mut send: impl FnMut(String)) {
    for t in ["8", "8r"] {
        pending.push(format!("who:{t}"));
        send(format!("tell /os who {t}"));
    }
    pending.push("online".into());
    send("who".into());
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
        // Observers coming and going on our games and watched games arrive as `+ name watch .id`;
        // that keeps the observer list live without asking again.
        "tell /os watch +",
        "chann + .chat",
    ] {
        say(ctx, send, c);
    }
    let chan = ctx.snap.lock().unwrap().log_channel.clone();
    if !chan.is_empty() {
        say(ctx, send, format!("chann + {chan}"));
    }
    let open = ctx.snap.lock().unwrap().open;
    say(ctx, send, format!("tell /os open {}", u8::from(open)));
    ask_who(&mut w.pending, |c| say(ctx, send, c));
    for t in ["8", "8r"] {
        w.pending.push(format!("rank:{t}:{login}"));
        say(ctx, send, format!("tell /os rank {t} {login}"));
    }
    for (key, cmd) in [
        ("stored_list", "tell /os stored"),
        ("match_list", "tell /os match"),
        ("history:", "tell /os history"),
    ] {
        w.pending.push(key.into());
        say(ctx, send, cmd);
    }
    ctx.prepare_workers();
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
        remember_account(&server(), login, pw);
    }

    if !w.td_block.is_empty() {
        if let Some(body) = ln.strip_prefix('|') {
            w.td_block.push(body.trim_end().to_string());
            return;
        }
        let text = std::mem::take(&mut w.td_block).join("\n");
        apply_td(ctx, login, &text, true);
    }
    if let Some(first) = ln.strip_prefix("/td: ") {
        w.td_block.push(first.trim_end().to_string());
        return;
    }
    let (done, was_chat) = take_chat_line(&mut w.chat_block, ln);
    for m in done {
        if m.from == "/td" {
            apply_td(ctx, login, &m.text, false);
        }
        deliver_chat(ctx, login, m);
    }
    if was_chat {
        return;
    }
    if let Some(id) = ln.strip_prefix("/os: rating_update ") {
        w.rating_for = Some(base_id(id.trim()).to_string());
        return;
    }
    if let Some(id) = w.rating_for.clone() {
        if ln == "READY" {
            w.rating_for = None;
        } else if let Some(after) = rating_after(ln, login) {
            record_post_rating(ctx, &id, after);
        }
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
                | ctx.pending_watch.remove(base_id(&mid)).is_some()
            {
                ctx.snap.lock().unwrap().notice.clear();
            }
        }
        handle_block(ctx, &w.block, login, &mut w.matches, &mut send);
        ask_watchers(ctx, w, &mut send);
        return;
    }

    if let Some((on, name, id)) = watch_notice(ln) {
        let set = ctx.watchers.entry(id.to_string()).or_default();
        if on {
            set.insert(name.to_string());
        } else {
            set.remove(name);
        }
        publish_watchers(ctx);
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
    // Both lists refresh every minute; anyone else is offline and an ask only draws an error.
    let present = s.users.iter().any(|u| u.name == sb.opponent)
        || s.ongoing.iter().any(|o| o.names.contains(&sb.opponent));
    drop(s);

    let more = sb.max_games == 0 || games < sb.max_games;
    if !sb.enabled || sb.opponent.is_empty() || !more {
        return;
    }
    w.next_ask_at = Some(Instant::now() + Duration::from_secs(sb.interval_secs.max(30)));
    if in_match || outgoing || !present {
        return;
    }
    ctx.log("info", &format!("auto match: asking {}", sb.opponent));
    ctx.ask_matched = false;
    ctx.auto_asked = true;
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
        // Players in a game drop out of `who`, so a one-off list never shows them free again.
        ask_who(&mut w.pending, |c| say(ctx, &mut send, c));
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

fn mark_watching(s: &mut Snapshot, id: &str, on: bool) {
    for o in s.ongoing.iter_mut() {
        if o.id == id {
            o.watching = on;
        }
    }
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
        Cmd::Connect { login, pw, server } => {
            if let Some(h) = &ctx.stop {
                h.stop();
            }
            send("quit".to_string());
            ctx.reconnect = Some((login, pw, server));
            quit = true;
        }
        Cmd::Disconnect => {
            if let Some(h) = &ctx.stop {
                h.stop();
            }
            send("quit".to_string());
            quit = true;
        }
        Cmd::Raw(c) => {
            if let Some(ids) = c.strip_prefix("tell /os cancel ") {
                ctx.cancelled
                    .extend(ids.split_whitespace().map(str::to_string));
            }
            send(c)
        }
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
        Cmd::Who => ask_who(pending, &mut send),
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
            mark_watching(&mut s, &id, true);
            drop(s);
            ctx.dirty = true;
        }
        Cmd::Unwatch(id) => {
            send(format!("tell /os watch - {id}"));
            // Notices stop with the watching: the list would go stale, and watching again asks anew.
            ctx.watchers.remove(base_id(&id));
            ctx.watch_asked.remove(base_id(&id));
            publish_watchers(ctx);
            let mut s = ctx.snap.lock().unwrap();
            mark_watching(&mut s, &id, false);
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
        Cmd::ReloadThreads => {
            apply_threads(ctx);
            ctx.dirty = true;
        }
        Cmd::SetAutoPlay(b) => {
            ctx.snap.lock().unwrap().auto_play = b;
            save_settings(ctx);
            ctx.dirty = true;
        }
        Cmd::SetOpen(b) => {
            ctx.snap.lock().unwrap().open = b;
            save_settings(ctx);
            send(format!("tell /os open {}", u8::from(b)));
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
        Cmd::SetLogChannel(new) if valid_log_channel(&new) => {
            let old = std::mem::replace(&mut ctx.snap.lock().unwrap().log_channel, new.clone());
            if old != new {
                if !old.is_empty() {
                    send(format!("chann - {old}"));
                }
                if !new.is_empty() {
                    send(format!("chann + {new}"));
                }
            }
            save_settings(ctx);
            ctx.dirty = true;
        }
        Cmd::SetLogChannel(_) => {}
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
            s.standby = cfg.needing_opponent();
            if !was && s.standby.enabled {
                s.standby_stats = Default::default();
                ctx.ask_refusals = 0;
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
                ctx.log("info", &format!("auto match: accepting pending offer {id}"));
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
            if mrest.split_whitespace().any(|t| t == login) {
                let opp = ctx.snap.lock().unwrap().standby.opponent.clone();
                if !opp.is_empty() && mrest.split_whitespace().any(|t| t == opp) {
                    ctx.ask_matched = true;
                    ctx.ask_refusals = 0;
                }
                // A game against someone we had an adjourned game with is that game resumed:
                // the stored list and the old adjourned boards are out of date now.
                let opp = match_line_parts(mrest)
                    .0
                    .into_iter()
                    .find(|n| n != login)
                    .unwrap_or_default();
                let resumed = ctx.snap.lock().unwrap().stored.iter().any(|x| x.opp == opp);
                if resumed {
                    pending.push("stored_list".into());
                    send("tell /os stored".to_string());
                    matches.retain(|_, m| !(m.over && m.ended == "adjourned" && m.opp_name == opp));
                    sync_matches(ctx, matches);
                }
            }
            if !id.is_empty() {
                if let Some(f) = mrest.split_whitespace().last() {
                    // Tournament games ("T") are always rated.
                    if f == "R" || f == "U" || f == "T" {
                        ctx.rated_by_base.insert(base_id(&id).to_string(), f != "U");
                    }
                    if f == "T" && mrest.contains(login) {
                        let mut snap = ctx.snap.lock().unwrap();
                        let tid = td_running(&snap.td);
                        let games = &mut snap.td.games;
                        let b = base_id(&id).to_string();
                        if !games.iter().any(|g| g.id == b) {
                            let opp = match_line_parts(mrest)
                                .0
                                .into_iter()
                                .find(|n| n != login)
                                .unwrap_or_default();
                            games.push(TdGame {
                                id: b,
                                tid,
                                opp,
                                diff: None,
                            });
                        }
                    }
                }
                let mine = mrest.contains(login);
                if mine {
                    ctx.notify(&crate::i18n::t("backend.notify.game_start_title"), mrest);
                }
                let mut s = ctx.snap.lock().unwrap();
                s.ongoing.retain(|o| o.id != id);
                if !mine {
                    let (names, ratings, gtype) = match_line_parts(mrest);
                    s.ongoing.push(OngoingView {
                        id,
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
            let incoming = s
                .offers
                .last()
                .map(|o| (o.incoming, o.id.clone(), o.names.join(" ")));
            drop(s);
            if let Some((true, id, who)) = incoming {
                if auto && !in_match {
                    ctx.log("info", &format!("auto match: auto-accepting {id}"));
                    send(format!("tell /os accept {id}"));
                } else {
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
                    // The score is from the first name in the brackets, which is not always black.
                    let first = rest
                        .split('(')
                        .nth(1)
                        .and_then(|t| t.split_whitespace().next())
                        .unwrap_or("");
                    m.result = black_view(m, first, v);
                    // One board of a synchro pair can end before the match does; its clocks stop here.
                    m.over = true;
                    m.turn = ' ';
                }
            }
        }
    } else if let Some(rest) = ln.strip_prefix("/os: - ") {
        let rest = rest.trim_start();
        if let Some(mrest) = rest.strip_prefix("match ") {
            let was_mine = mrest.contains(login);
            if was_mine {
                let id = mrest.split_whitespace().next().unwrap_or("");
                let mut s = ctx.snap.lock().unwrap();
                if let Some(g) = s.td.games.iter_mut().find(|g| g.id == base_id(id)) {
                    g.diff = diff_from_raw(mrest, &g.opp);
                }
            }
            {
                let id = mrest.split_whitespace().next().unwrap_or("").to_string();
                let mut s = ctx.snap.lock().unwrap();
                s.ongoing.retain(|o| o.id != id);
                drop(s);
                if !was_mine {
                    let (kind, who) = end_kind(mrest);
                    finish_match(matches, &id, "", None, kind, &who, "");
                }
            }
            sync_matches(ctx, matches);
            ctx.emit(true);
            handle_match_end(ctx, mrest, login, matches);
            // Both players just left the match list but are back in `who` only once it is asked again.
            ask_who(pending, &mut send);
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
            let gone = s.offers.iter().find(|o| o.id == id).cloned();
            s.offers.retain(|o| o.id != id);
            drop(s);
            ctx.dirty = true;
            if let Some(o) = gone {
                refused_ask(ctx, &o, login);
            }
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
                &format!("auto match: unattended, declining ({})", req.who),
            );
            send(format!("tell /os decline {}", req.who));
        }
    } else if ln.starts_with("/os: ERR") {
        ctx.log("info", &format!("server error: {ln}"));
        if ln.contains("not found") && ln.contains("match") {
            return;
        }
        let msg = ln.trim_start_matches("/os: ERR").trim();
        if std::mem::take(&mut ctx.auto_asked)
            && msg.contains("doesn't fit the opponent's formula")
            && stop_auto_match_unfit(ctx)
        {
            return;
        }
        ctx.snap.lock().unwrap().notice = if msg.is_empty() {
            "err.ggs_action_refused".into()
        } else {
            ggs_err_notice(msg)
        };
        ctx.dirty = true;
    }
}

/// Our auto-match request vanished with no game and no cancel from us: the opponent declined it
/// (or closed itself to requests). Asking again every minute only pesters them, so auto match
/// stops at the second time in a row; one alone could be the opponent starting a game elsewhere.
fn refused_ask(ctx: &mut Ctx, o: &Offer, login: &str) {
    const STOP_AFTER: u32 = 2;
    let cancelled = ctx.cancelled.remove(&o.id);
    let mut s = ctx.snap.lock().unwrap();
    let opp = s.standby.opponent.clone();
    if cancelled || ctx.ask_matched || !s.standby.enabled || !ask_to(o, login, &opp) {
        return;
    }
    ctx.ask_refusals += 1;
    let n = ctx.ask_refusals;
    if n >= STOP_AFTER {
        s.standby.enabled = false;
        s.notice = format!("err.auto_match_refused|name={opp}");
    }
    drop(s);
    ctx.log(
        "info",
        &format!(
            "auto match: {opp} took request {} away ({n} in a row)",
            o.id
        ),
    );
    if n >= STOP_AFTER {
        ctx.log("info", "auto match: stopped");
        ctx.notify(
            &crate::i18n::t("backend.notify.auto_match_refused_title"),
            &crate::i18n::tf("backend.notify.auto_match_refused_body", &[("name", &opp)]),
        );
    }
    ctx.dirty = true;
}

/// The auto-match opponent's formula turns the request away as written; asking again the same way
/// can never succeed, so auto match stops at the first one.
fn stop_auto_match_unfit(ctx: &mut Ctx) -> bool {
    let mut s = ctx.snap.lock().unwrap();
    if !s.standby.enabled {
        return false;
    }
    let opp = s.standby.opponent.clone();
    s.standby.enabled = false;
    s.notice = format!("err.auto_match_unfit|name={opp}");
    drop(s);
    ctx.log(
        "info",
        &format!("auto match: the request does not fit {opp}'s formula; stopped"),
    );
    ctx.notify(
        &crate::i18n::t("backend.notify.auto_match_refused_title"),
        &crate::i18n::tf("backend.notify.auto_match_unfit_body", &[("name", &opp)]),
    );
    ctx.dirty = true;
    true
}

/// Our own request to `opp` (not one sent to us, nor one to anybody).
fn ask_to(o: &Offer, login: &str, opp: &str) -> bool {
    !o.incoming
        && o.names.first().is_some_and(|n| n == login)
        && o.names.get(1).is_some_and(|n| n == opp)
}

/// A server refusal as a message key: the ones a person can cause from the screens get their own
/// words, anything else is shown as the server wrote it.
fn ggs_err_notice(msg: &str) -> String {
    let quoted = || msg.split('\'').nth(1).unwrap_or("").replace('|', "/");
    let key = match msg {
        m if m.contains("same parameters is already pending") => "dup_request",
        m if m.contains("doesn't fit the opponent's formula") => "formula",
        m if m.contains("not accepting new matches") => "not_accepting",
        m if m.contains("open value is too low") => "open_low",
        m if m.contains("cannot accept your own") => "own_request",
        m if m.contains("opponent is ignoring you") => "ignored",
        m if m.contains("you are ignoring the opponent") => "ignoring",
        m if m.contains("you have too many stored") => "stored_mine",
        m if m.contains("opponent has too many stored") => "stored_opp",
        m if m.contains("you are not registered") => "unregistered",
        m if m.contains("rated variable mismatch") => "rated_mismatch",
        m if m.starts_with("user '") && m.ends_with("not found.") => {
            return format!("err.ggs.no_user|name={}", quoted())
        }
        m if m.starts_with("request '") && m.ends_with("not found.") => "request_gone",
        _ => return format!("err.ggs_refused|msg={}", msg.replace('|', "/")),
    };
    format!("err.ggs.{key}")
}

fn ctx_send(writer: &mut TcpStream, cmd: &str) {
    let _ = writer
        .write_all(cmd.as_bytes())
        .and_then(|_| writer.write_all(b"\n"));
}

fn resolve_threads(n: usize) -> usize {
    if n == 0 {
        std::thread::available_parallelism()
            .map(|c| (c.get() / 2).max(1))
            .unwrap_or(4)
    } else {
        n
    }
}

fn apply_threads(ctx: &mut Ctx) {
    let n = resources().threads.unwrap_or_else(|| resolve_threads(0));
    ctx.engine_cfg.threads = n;
    if let Some(e) = ctx.engine.as_mut() {
        e.set_threads(n);
    }
    ctx.snap.lock().unwrap().engine.threads = n;
    for w in &mut ctx.workers {
        w.threads = 0;
    }
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
    forget_finished_on_join(matches, &block[0], &mid);
    let m = matches.entry(mid.clone()).or_insert_with(MatchState::new);
    m.seen += 1;
    m.updated_ms = epoch_ms();
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
            &format!("{mid}: in overtime; playing on with the extension as the clock"),
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
        // The search runs on this thread, so it would stall an own match.
        let playing = matches.values().any(|x| x.my_color.is_some() && !x.over);
        if watch_an && rows_ok && turn.is_some() && !playing {
            analyze_watch(ctx, &mid, matches);
        }
        return;
    }
    let my_turn = turn == m.my_color;
    if my_turn && !m.told_turn {
        m.told_turn = true;
        if !auto {
            let who = if m.opp_name.is_empty() {
                mid.clone()
            } else {
                m.opp_name.clone()
            };
            ctx.notify(&crate::i18n::t("backend.notify.your_turn_title"), &who);
        }
    } else if !my_turn {
        m.told_turn = false;
    }
    if !auto || !rows_ok || !my_turn {
        return;
    }
    think_and_play(ctx, &mid, matches, send);
}

/// Watched games are analysed between server lines, so the search stays short.
const WATCH_DEPTH: u32 = 14;
const WATCH_SOLVE: u8 = 20;

fn analyze_watch(ctx: &mut Ctx, mid: &str, matches: &mut HashMap<String, MatchState>) {
    let m = matches.get_mut(mid).unwrap();
    let Some(board) = board_of(m, m.turn) else {
        return;
    };
    let bh = board.black.wrapping_mul(31).wrapping_add(board.white) ^ (m.turn as u64);
    if bh == m.watch_hash {
        return;
    }
    m.watch_hash = bh;
    let black_turn = m.turn == '*';
    if ctx.ensure_engine().is_err() {
        return;
    }
    let engine = ctx.engine.as_mut().unwrap();
    engine.set_levels(WATCH_DEPTH, WATCH_SOLVE);
    let mv = engine.choose(&board);

    let m = matches.get_mut(mid).unwrap();
    let v = if mv.value.is_finite() { mv.value } else { 0.0 };
    m.watch_eval = Some(if black_turn { v } else { -v }); // to Black's view
    m.watch_best = mv.pos.map(coord);
    sync_matches(ctx, matches);
    ctx.emit(true);
}

fn legal_squares(m: &MatchState) -> Vec<u32> {
    if m.over {
        return Vec::new();
    }
    let Some(board) = board_of(m, m.turn) else {
        return Vec::new();
    };
    let bits = board.movable();
    (0..64u32).filter(|&i| bits >> i & 1 == 1).collect()
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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

/// GGS reuses match numbers; a join on a finished board is a new game.
fn forget_finished_on_join(matches: &mut HashMap<String, MatchState>, first: &str, mid: &str) {
    if first.starts_with("/os: join") && matches.get(mid).is_some_and(|m| m.over) {
        matches.remove(mid);
    }
}

fn apply_block(m: &mut MatchState, block: &[String], login: &str) -> (bool, Option<char>) {
    let mut rows: Vec<Vec<char>> = Vec::new();
    let mut boards: Vec<Vec<Vec<char>>> = Vec::new();
    let mut turns: Vec<char> = Vec::new();
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
                        rating: rating.clone(),
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
                            // The server shows 4:59 at join and 5:00 after an instant book move; only a jump into the extension is overtime.
                            let jumped = now <= g
                                && m.my_clock_secs
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
            turns.push('*');
        } else if t.starts_with("O to move") {
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
                if (2..=4).contains(&mv.len()) || mv.eq_ignore_ascii_case("pa") {
                    // GGS sends 0.00 as an empty field; no field at all means no evaluation.
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

    let turn = turns.last().copied();
    let last_n = m.moves.keys().next_back().copied();
    if let (Some(t), Some(mc), Some(n)) = (turn, m.my_color, last_n) {
        m.eval_parity = Some(if t != mc { n % 2 } else { (n + 1) % 2 });
        if t == mc {
            let (ev, sec) = m.move_evals.get(&n).copied().unwrap_or((None, None));
            m.opp_eval = ev;
            m.opp_secs_used = sec;
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

/// GGS shows 0:00 in overtime until the extension's own countdown appears.
fn my_clock_secs(m: &MatchState) -> u64 {
    let secs = m.my_clock_secs.unwrap_or(0);
    if m.in_overtime && secs == 0 {
        m.my_ext.unwrap_or(0)
    } else {
        secs
    }
}

fn game_over(board: &Board) -> bool {
    if board.movable() != 0 {
        return false;
    }
    let mut other = *board;
    other.pass();
    other.movable() == 0
}

fn play_hash(board: &Board, moves: usize) -> u64 {
    board
        .black
        .wrapping_mul(31)
        .wrapping_add(board.white)
        .wrapping_add((moves as u64).wrapping_mul(0x9e37_79b9))
}

fn move_text(pos: Option<Position>) -> String {
    match pos {
        Some(p) => coord(p),
        None => "pa".to_string(),
    }
}

fn move_source(mv: &kuroobi::engine::MoveEval) -> &'static str {
    if mv.from_book && mv.learned {
        "book (learned)"
    } else if mv.from_book {
        "book"
    } else {
        "search"
    }
}

/// GGS caps channel names at 8 characters including the dot; empty turns the log off.
pub fn valid_log_channel(chan: &str) -> bool {
    chan.is_empty()
        || chan.strip_prefix('.').is_some_and(|name| {
            (1..=7).contains(&name.len())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

struct Searched {
    empties: u8,
    threads: usize,
    nodes: u64,
    took: Duration,
    pv: Vec<Option<Position>>,
    black: bool,
}

fn count(n: f64, unit: &str) -> String {
    if n >= 1e9 {
        format!("{:.3} G{unit}", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} M{unit}", n / 1e6)
    } else {
        format!("{:.1} K{unit}", n / 1e3)
    }
}

fn confidence_pct(t: f32) -> u32 {
    // Abramowitz-Stegun 7.1.26 for erf(t / sqrt 2): the two-sided normal coverage.
    let x = f64::from(t) / std::f64::consts::SQRT_2;
    let k = 1.0 / (1.0 + 0.3275911 * x);
    let poly = k
        * (0.254829592
            + k * (-0.284496736 + k * (1.421413741 + k * (-1.453152027 + k * 1.061405429))));
    ((1.0 - poly * (-x * x).exp()) * 100.0).round() as u32
}

/// Edax's spectator log from the GGS guide: black's moves in capitals, white's in lower case.
fn search_log(
    chan: &str,
    login: &str,
    mid: &str,
    mv: &kuroobi::engine::MoveEval,
    s: &Searched,
) -> Vec<String> {
    let square = |p: Option<Position>, black: bool| {
        let t = p.map_or("PA".to_string(), coord);
        if black {
            t
        } else {
            t.to_lowercase()
        }
    };
    let mut lines = vec![
        "-----------------------------------------".to_string(),
        format!(
            "{login} plays {} in game {mid} using {} threads ;",
            square(mv.pos, s.black),
            s.threads
        ),
    ];
    let score = mv.value.round() as i32;
    if mv.from_book {
        lines.push(format!("score == {score:+} from book ;"));
    } else {
        let at = if mv.exact || mv.depth == 0 {
            format!("{}@100%", s.empties)
        } else {
            let t = mv.mpc_t.unwrap_or_else(kuroobi::midgame::mpc_t);
            format!("{}@{}%", mv.depth, confidence_pct(t))
        };
        let pv: Vec<String> =
            s.pv.iter()
                .scan(!s.black, |black, p| {
                    *black = !*black;
                    Some(square(*p, *black))
                })
                .collect();
        let secs = s.took.as_secs_f64();
        lines.push(format!(
            "score == {score:+} at {at} ; PV: {} ;",
            pv.join(" ")
        ));
        lines.push(format!(
            "nodes: {} ; time: search = {secs:.1}s, move = {secs:.1}s; speed: {}.",
            count(s.nodes as f64, "N"),
            count(s.nodes as f64 / secs.max(1e-3), "N/s"),
        ));
        lines.push(if mv.cut {
            format!("search out of time at {at}")
        } else {
            "search completed".to_string()
        });
    }
    lines
        .into_iter()
        .map(|l| format!("tell {chan} {l}"))
        .collect()
}

fn record_played(m: &mut MatchState, mv: &kuroobi::engine::MoveEval, hash: u64) {
    m.last_eval = Some(if mv.value.is_finite() { mv.value } else { 0.0 });
    m.last_eval_exact = mv.exact;
    m.last_from_book = mv.from_book;
    m.last_played_hash = hash;
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
    // The end line lags the last move; a pass sent here is refused as "wrong game!".
    if game_over(&board) {
        return;
    }
    let bh = play_hash(&board, m.moves.len());
    if bh == m.last_played_hash {
        return;
    }
    if ctx.workers.iter().any(|w| {
        w.mid.as_deref() == Some(mid)
            && ((w.busy && w.pending_hash == bh)
                || w.pending.as_ref().is_some_and(|p| p.hash == bh))
    }) {
        return;
    }
    if let Err(e) = ctx.ensure_engine() {
        ctx.log("info", &format!("engine init failed: {e}"));
        return;
    }
    let clock = Duration::from_secs(my_clock_secs(m));
    let black = m.my_color == Some('*');
    {
        let mut s = ctx.snap.lock().unwrap();
        s.thinking = Some(mid.to_string());
    }
    ctx.emit(true);

    if let Some(i) = ctx.worker_for(mid) {
        ctx.workers[i].pending = Some(Pending {
            board,
            clock,
            hash: bh,
            hint: mirror_hint(matches, mid),
            queued: Instant::now(),
        });
        pump_next(ctx);
        return;
    }
    let engine = ctx.engine.as_mut().unwrap();
    let n0 = engine.nodes();
    let began = std::time::Instant::now();
    let mv = engine.play_clock(&board, clock);
    let took = began.elapsed();
    let r = report(engine, &board, &mv, engine.nodes() - n0);

    let mstr = move_text(mv.pos);
    let arg = play_arg(&mstr, mv.value, Some(took));
    send(format!("tell /os play {mid} {arg}"));
    ctx.log("out", &format!("tell /os play {mid} {arg}"));
    let (chan, login) = {
        let mut s = ctx.snap.lock().unwrap();
        s.last_search = Some(SearchStat {
            nodes: r.nodes,
            secs: took.as_secs_f64(),
        });
        (s.log_channel.clone(), s.login.clone())
    };
    if !chan.is_empty() {
        for line in search_log(
            &chan,
            &login,
            mid,
            &mv,
            &Searched {
                empties: board.empty_count(),
                threads: ctx.total_threads(),
                nodes: r.nodes,
                took,
                pv: r.pv,
                black,
            },
        ) {
            send(line);
        }
    }
    ctx.log(
        "info",
        &format!(
            "{mid} {mstr}: {} {:+.2}{}{}",
            move_source(&mv),
            mv.value,
            if mv.exact { " (solved)" } else { "" },
            if mv.depth > 0 {
                format!(" depth {}", mv.depth)
            } else {
                String::new()
            }
        ),
    );

    record_played(matches.get_mut(mid).unwrap(), &mv, bh);
    {
        let mut s = ctx.snap.lock().unwrap();
        s.thinking = None;
    }
    sync_matches(ctx, matches);
    ctx.emit(true);
}

/// One board at a time on every thread: the server charges the reported time, so the waiting board loses none.
fn pump_next(ctx: &mut Ctx) {
    if ctx.thinking() {
        return;
    }
    let Some(i) = (0..ctx.workers.len())
        .filter(|&i| ctx.workers[i].pending.is_some())
        .min_by_key(|&i| ctx.workers[i].pending.as_ref().map(|p| p.queued))
    else {
        return;
    };
    let mut ponders = false;
    for w in &mut ctx.workers {
        if w.busy {
            w.stop.stop();
            ponders = true;
        }
    }
    if ponders {
        return;
    }
    let total = ctx.total_threads();
    ctx.set_worker_threads(i, total);
    let Some(p) = ctx.workers[i].pending.take() else {
        return;
    };
    ctx.workers[i].pending_hash = p.hash;
    // A search can never use more than the clock it was given.
    ctx.workers[i].sent_at = Some((Instant::now(), p.clock));
    ctx.workers[i].sent_ms = epoch_ms();
    ctx.workers[i].sent_empties = p.board.empty_count();
    ctx.workers[i].stop.reset();
    let empties = p.board.empty_count();
    let movable = p.board.movable_count();
    ctx.log(
        "info",
        &format!(
            "dispatch: empties {empties} moves {movable} clock {:.1}s",
            p.clock.as_secs_f32()
        ),
    );
    ctx.workers[i].send(Job::Think {
        board: p.board,
        clock: p.clock,
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
                    "stopped a search far past its limit ({:.1}s / limit {:.1}s)",
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
        let Done::Moved(mv, r) = done;
        if let Some(t) = took {
            ctx.log(
                "info",
                &format!(
                    "reply: {:.1}s{}{} {}{}",
                    t.as_secs_f32(),
                    if mv.cut { " (cut)" } else { "" },
                    if mv.depth > 0 {
                        format!(" depth {}", mv.depth)
                    } else {
                        String::new()
                    },
                    if mv.exact { "solve" } else { "search" },
                    mv.mpc_t
                        .map_or(String::new(), |t| format!(" @{}%", confidence_pct(t))),
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
        let now_hash = board_of(m, m.turn).map(|b| play_hash(&b, m.moves.len()));
        if now_hash != Some(bh) {
            ctx.log(
                "info",
                &format!("{mid}: searched position differs from the current one; not playing (will re-search)"),
            );
            continue;
        }
        let mstr = move_text(mv.pos);
        out.push(format!(
            "tell /os play {mid} {}",
            play_arg(&mstr, mv.value, took)
        ));
        record_played(m, &mv, bh);
        let (chan, login) = {
            let mut s = ctx.snap.lock().unwrap();
            if let Some(t) = took {
                s.last_search = Some(SearchStat {
                    nodes: r.nodes,
                    secs: t.as_secs_f64(),
                });
            }
            (s.log_channel.clone(), s.login.clone())
        };
        if !chan.is_empty() {
            let w = &ctx.workers[i];
            out.extend(search_log(
                &chan,
                &login,
                &mid,
                &mv,
                &Searched {
                    empties: w.sent_empties,
                    threads: w.threads,
                    nodes: r.nodes,
                    took: took.unwrap_or_default(),
                    pv: r.pv,
                    black: m.my_color == Some('*'),
                },
            ));
        }
        ctx.log(
            "info",
            &format!(
                "{mid} {mstr}: {} {:+.2}{}",
                move_source(&mv),
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

    pump_next(ctx);

    out
}

fn sync_matches(ctx: &mut Ctx, matches: &HashMap<String, MatchState>) {
    let mut view: Vec<MatchView> = matches
        .iter()
        .map(|(id, m)| MatchView {
            id: id.clone(),
            base: base_id(id).to_string(),
            rated: ctx.rated_by_base.get(base_id(id)).copied(),
            ended: m.ended.clone(),
            left_by: m.left_by.clone(),
            archive: m.archive.clone(),
            busy: String::new(),
            busy_depth: 0,
            busy_best: None,
            busy_eval: None,
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
            seen: m.seen,
            updated_ms: m.updated_ms,
            think_since_ms: 0,
            think_queued: false,
            legal: legal_squares(m),
            order: m.order,
        })
        .collect();
    view.sort_by_key(|v| std::cmp::Reverse(v.order));
    for v in &mut view {
        let Some(w) = ctx.workers.iter().find(|w| w.mid.as_deref() == Some(&v.id)) else {
            continue;
        };
        v.think_queued = w.pending.is_some();
        if w.sent_ms > v.updated_ms {
            v.think_since_ms = w.sent_ms;
        }
        if !w.busy {
            continue;
        }
        let (kind, depth, best, eval) = w.progress.snapshot();
        v.busy = match kind {
            kuroobi::engine::Progress::THINK => "think",
            kuroobi::engine::Progress::SOLVE => "solve",
            _ => "",
        }
        .into();
        v.busy_depth = depth;
        v.busy_best = best;
        v.busy_eval = eval;
    }
    ctx.snap.lock().unwrap().matches = view;
    ctx.dirty = true;
}

fn add_offer(ctx: &mut Ctx, rest: &str, login: &str) {
    let Some(mut offer) = parse_offer(rest, login) else {
        return;
    };
    let id = offer.id.clone();
    if !offer.incoming {
        // The server took our request, so a later formula error is not about auto match's ask.
        ctx.auto_asked = false;
    }
    let mut s = ctx.snap.lock().unwrap();
    offer.since_ms = s
        .offers
        .iter()
        .find(|o| o.id == id)
        .map_or_else(epoch_ms, |o| o.since_ms);
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
        since_ms: 0,
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

/// `|name old @ dev delta -> new @ dev`
fn rating_after(ln: &str, login: &str) -> Option<f32> {
    let mut t = ln.trim_start_matches('|').split_whitespace();
    if t.next()? != login {
        return None;
    }
    t.skip_while(|x| *x != "->").nth(1)?.parse().ok()
}

/// Only the newest row: GGS reuses match numbers.
fn record_post_rating(ctx: &mut Ctx, id: &str, after: f32) {
    let now = now_secs();
    let mut s = ctx.snap.lock().unwrap();
    match s.results.first_mut() {
        Some(r) if base_id(&r.id) == id && now.saturating_sub(r.at) < 60 => {
            r.my_rating = Some(after);
            drop(s);
            ctx.dirty = true;
        }
        _ => {
            drop(s);
            ctx.post_ratings.insert(id.to_string(), after);
        }
    }
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
    let re = score.map(|s| format!("{s:+.2}"));
    let mut dropped = finish_match(matches, &id, first_name, score, kind, &who, archive);
    dropped.sort_by_key(|(_, m)| m.seen);
    let m = dropped.first().map(|(_, m)| m);
    let mut boards: Vec<BoardRecord> = dropped
        .iter()
        .map(|(k, m)| BoardRecord {
            id: k.clone(),
            kifu: m.kifu(),
            ggf: m.ggf_with(k, Some(m.result.as_str()).filter(|r| !r.is_empty()), true),
        })
        .collect();
    boards.sort_by(|a, b| a.id.cmp(&b.id));
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
    let now = now_secs();
    let result = GameResult {
        id: id.clone(),
        base: base_id(&id).to_string(),
        ggf,
        archive: archive.to_string(),
        raw: rest.to_string(),
        my_diff,
        opp,
        kifu,
        seq: ctx.seq,
        my_rating: ctx.post_ratings.remove(base_id(&id)).or_else(|| {
            let pool = if toks
                .iter()
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
        }),
        at: now,
        boards,
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

    if !ctx.snap.lock().unwrap().engine.learn || local_server() {
        return;
    }
    for (_, lm) in &dropped {
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
                    at: now_secs(),
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
    let (_, job, _) = ctx.learn_jobs.front_mut().expect("checked non-empty");
    let step = ctx.engine.as_mut().unwrap().learn_step(job, LEARN_DEPTH);
    if matches!(step, Ok(None)) {
        return;
    }
    let (id, _, mut entry) = ctx.learn_jobs.pop_front().expect("checked non-empty");
    match step {
        Ok(Some(out)) => {
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
        Err(e) => ctx.log("info", &format!("learn failed ({id}): {e}")),
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

/// Names, their ratings in the same order, and the game type of a match line `.id r1 name1 r2 name2 type R|U ...`.
fn match_line_parts(line: &str) -> (Vec<String>, Vec<String>, String) {
    let t: Vec<&str> = line.split_whitespace().collect();
    let names = t
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
    // The id (`.23`) and the archive number parse as floats too.
    let ratings = t
        .iter()
        .filter(|x| !x.starts_with('.') && x.parse::<f32>().is_ok() && x.len() >= 3)
        .map(|x| x.to_string())
        .collect();
    let gtype = t
        .iter()
        .find(|x| x.starts_with("s8") || x.starts_with('8'))
        .map(|x| x.to_string())
        .unwrap_or_default();
    (names, ratings, gtype)
}

/// Everyone logged in to GGS, from the server's own `who`; `/os who` leaves out players who are in a game or not taking requests.
fn online_names(buf: &[String]) -> Vec<String> {
    buf.iter()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| n.starts_with(|c: char| c.is_ascii_alphabetic()))
        .map(str::to_string)
        .collect()
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
    } else if kind.starts_with("watchers:") {
        ln.starts_with("/os: watch")
    } else if kind == "online" {
        ln.starts_with(": who")
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

/// Returns the messages this line completes, and whether the line belonged to a chat.
fn take_chat_line(block: &mut Option<ChatMsg>, ln: &str) -> (Vec<ChatMsg>, bool) {
    if let Some(m) = block.as_mut() {
        if let Some(body) = ln.strip_prefix('|') {
            if !body.starts_with("---") {
                if !m.text.is_empty() {
                    m.text.push('\n');
                }
                m.text.push_str(body.trim_end());
            }
            return (Vec::new(), true);
        }
    }
    let done: Vec<ChatMsg> = block.take().into_iter().collect();
    let Some(msg) = parse_chat(ln) else {
        return (done, false);
    };
    // A message may continue on "|" lines (a /td announcement even starts with an empty header), so it waits for the next line.
    *block = Some(msg);
    (done, true)
}

fn apply_td(ctx: &mut Ctx, login: &str, text: &str, direct: bool) {
    let mut s = ctx.snap.lock().unwrap();
    td_apply(&mut s.td, login, text, direct, now_secs());
    drop(s);
    ctx.dirty = true;
}

/// The tournament we are playing in right now, which a new `T` match belongs to.
fn td_running(td: &TdView) -> u32 {
    td.tourneys
        .iter()
        .find(|t| t.joined && t.state == "running")
        .or_else(|| td.tourneys.iter().find(|t| t.joined && t.state != "over"))
        .map_or(0, |t| t.id)
}

/// `m:ss` as printed by the director (`TimeOut`).
fn td_secs(t: &str) -> Option<u64> {
    let (m, s) = t.trim().split_once(':')?;
    Some(m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?)
}

fn td_entry(td: &mut TdView, id: u32) -> &mut TourneyView {
    if let Some(i) = td.tourneys.iter().position(|t| t.id == id) {
        return &mut td.tourneys[i];
    }
    td.tourneys.push(TourneyView {
        id,
        ..Default::default()
    });
    td.tourneys.sort_by_key(|t| std::cmp::Reverse(t.id));
    let i = td.tourneys.iter().position(|t| t.id == id).unwrap();
    &mut td.tourneys[i]
}

/// The number right after `"tournament "` in `s`.
fn td_id_after(s: &str) -> Option<u32> {
    let rest = &s[s.find("tournament ")? + "tournament ".len()..];
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    n.parse().ok()
}

/// Folds one /td message (wording from GGS Service/TD/src) into the tournament list.
fn td_apply(td: &mut TdView, login: &str, text: &str, direct: bool, now: u64) {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("").trim();
    let rest: Vec<&str> = lines.collect();
    let (first, rest) = if first.is_empty() || first.starts_with("---") {
        let mut it = rest
            .iter()
            .copied()
            .filter(|l| !l.trim_start().starts_with("---"));
        (it.next().unwrap_or("").trim(), it.collect::<Vec<_>>())
    } else {
        (first, rest)
    };

    if direct {
        let (err, body) = match first.strip_prefix("ERR ") {
            Some(e) => (true, e.split_once(": ").map_or(e, |x| x.1)),
            None => (false, first),
        };
        let (cmd, head) = body.split_once(": ").unwrap_or(("", body));
        let mut full = vec![head];
        full.extend(rest.iter().copied());
        td.reply = Some(TdReply {
            cmd: cmd.to_string(),
            text: full.join("\n"),
            err,
            at: now,
        });
        if err {
            return;
        }
        match cmd {
            "tourneys" => {
                td_apply_list(td, &rest);
                return;
            }
            "finger" => {
                td_apply_finger(td, login, head, &rest);
                if let Ok(id) = head.trim().parse::<u32>() {
                    td_entry(td, id).info = rest.join("\n");
                }
                return;
            }
            "rankings" | "st" => {
                if let Some(id) = td_id_after(head) {
                    let full = td
                        .reply
                        .as_ref()
                        .map(|r| r.text.clone())
                        .unwrap_or_default();
                    let t = td_entry(td, id);
                    if cmd == "rankings" {
                        t.rankings = full;
                    } else {
                        t.schedule = full;
                    }
                }
                return;
            }
            _ => {}
        }
        if let Some(text) = body.split_once(": ").map(|x| x.1) {
            td_apply_event(td, login, text, &rest, now);
        }
        return;
    }
    td_apply_event(td, login, first, &rest, now);
}

fn td_apply_event(td: &mut TdView, login: &str, first: &str, rest: &[&str], now: u64) {
    let Some(id) = td_id_after(first) else {
        return;
    };
    // "join: X joins tournament N" and "withdraw: X withdrawn from tournament N".
    let event = first.split_once(": ").map_or(first, |x| x.1);
    let t = td_entry(td, id);
    if let Some(m) = first
        .strip_prefix(&format!("tournament {id} starting in "))
        .and_then(|r| r.strip_suffix(" minutes"))
    {
        t.state = "announced".into();
        if let Ok(m) = m.trim().parse::<u64>() {
            t.next_at = Some(now + m * 60);
        }
        if let Some(d) = rest
            .iter()
            .find_map(|l| l.trim().strip_prefix('(').and_then(|x| x.strip_suffix(')')))
        {
            t.desc = d.to_string();
        }
    } else if let Some(r) = first.strip_prefix(&format!("tournament {id} starts in ")) {
        t.state = "announced".into();
        t.next_at = td_secs(r).map(|x| now + x);
    } else if let Some(r) = first.strip_prefix(&format!("tournament {id} round ")) {
        if let Some((round, wait)) = r.split_once(" starts in ") {
            t.state = "break".into();
            if let Ok(n) = round.trim().parse::<u32>() {
                t.rounds_played = n.saturating_sub(1);
            }
            t.next_at = td_secs(wait).map(|x| now + x);
        }
    } else if let Some(r) = first.strip_prefix("starting round ") {
        t.state = "running".into();
        t.next_at = None;
        if let Some(n) = r
            .split_whitespace()
            .next()
            .and_then(|x| x.parse::<u32>().ok())
        {
            t.rounds_played = n.saturating_sub(1);
        }
    } else if let Some(r) = first.strip_prefix("ending round ") {
        t.state = "break".into();
        if let Some(n) = r
            .split_whitespace()
            .next()
            .and_then(|x| x.parse::<u32>().ok())
        {
            t.rounds_played = n;
        }
    } else if first.contains(" is over.") {
        t.state = "over".into();
        t.next_at = None;
        if t.rounds > 0 {
            t.rounds_played = t.rounds;
        }
    } else if first.contains(" is cancelled") {
        t.state = "cancelled".into();
        t.next_at = None;
    } else if let Some(w) = first.strip_prefix("Congratulations to ") {
        t.winner = w.split(',').next().unwrap_or("").trim().to_string();
        t.state = "over".into();
        t.next_at = None;
    } else if let Some((who, _)) = event.split_once(" joins tournament ") {
        let who = who.trim();
        if who == login {
            t.joined = true;
        }
        if !t.players.iter().any(|p| p == who) {
            t.players.push(who.to_string());
            t.n_players += 1;
        }
    } else if let Some((who, _)) = event.split_once(" withdrawn from tournament ") {
        let who = who.trim();
        if who == login {
            t.joined = false;
        }
        if t.players.iter().any(|p| p == who) {
            t.players.retain(|p| p != who);
            t.n_players = t.n_players.saturating_sub(1);
        }
    }
}

/// Rows of `tourneys`: `id director service #pl played/ rounds start clock type [Winner: X]`.
fn td_apply_list(td: &mut TdView, rows: &[&str]) {
    for row in rows {
        // `%3d/%3d` runs together once both reach three digits ("100/100").
        let mut joined = row.to_string();
        while joined.contains("/ ") {
            joined = joined.replace("/ ", "/");
        }
        let mut f: Vec<&str> = joined.split_whitespace().collect();
        let Some(id) = f.first().and_then(|x| x.parse::<u32>().ok()) else {
            continue;
        };
        if f.len() < 8 {
            continue;
        }
        let Some((played, rounds)) = f[4].split_once('/') else {
            continue;
        };
        f.splice(4..5, [played, rounds]);
        let t = td_entry(td, id);
        t.director = f[1].to_string();
        t.n_players = f[3].parse().unwrap_or(t.n_players);
        t.rounds_played = f[4].trim_end_matches('/').parse().unwrap_or(0);
        t.rounds = f[5].parse().unwrap_or(0);
        t.clock = f[7].to_string();
        t.gtype = f[8].to_string();
        if let Some(i) = f.iter().position(|x| *x == "Winner:") {
            t.winner = f.get(i + 1).unwrap_or(&"").to_string();
        }
        match f[6] {
            "now" => {
                t.state = "running".into();
                t.next_at = None;
            }
            "done" => {
                t.state = "over".into();
                t.next_at = None;
            }
            w => {
                if t.state.is_empty() || t.state == "over" {
                    t.state = if t.rounds_played == 0 {
                        "announced"
                    } else {
                        "break"
                    }
                    .into();
                }
                t.next_at = td_secs(w).map(|x| now_secs() + x);
            }
        }
    }
}

/// `finger N`: `key : value` lines after the id.
fn td_apply_finger(td: &mut TdView, login: &str, head: &str, rest: &[&str]) {
    let Some(id) = head.trim().parse::<u32>().ok() else {
        return;
    };
    let t = td_entry(td, id);
    for l in rest {
        let l = l.trim();
        if let Some((k, v)) = l.split_once(" : ") {
            let v = v.trim();
            match k.trim() {
                "director" => t.director = v.to_string(),
                "clock" => t.clock = v.to_string(),
                "type" => t.gtype = v.to_string(),
                "rounds" => {
                    if let Some((a, b)) = v.split_once('/') {
                        t.rounds_played = a.trim().parse().unwrap_or(t.rounds_played);
                        t.rounds = b.trim().parse().unwrap_or(t.rounds);
                    }
                }
                "players" => {
                    let mut it = v.split_whitespace();
                    t.n_players = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                    t.players = it
                        .filter(|p| *p != "(bye)")
                        .map(|p| p.trim_matches(|c| c == '(' || c == ')').to_string())
                        .collect();
                    t.joined = t.players.iter().any(|p| p == login);
                }
                _ => {}
            }
        } else if let Some(r) = l.strip_prefix("round ") {
            if let Some((_, w)) = r.split_once(" begins in ") {
                t.next_at = td_secs(w).map(|x| now_secs() + x);
                if t.rounds_played == 0 {
                    t.state = "announced".into();
                } else {
                    t.state = "break".into();
                }
            } else if r.contains("is being played") {
                t.state = "running".into();
                t.next_at = None;
            }
        } else if l == "tournament has finished" {
            t.state = "over".into();
            t.next_at = None;
        }
    }
}

fn deliver_chat(ctx: &mut Ctx, login: &str, msg: ChatMsg) {
    let chan = ctx.snap.lock().unwrap().log_channel.clone();
    if msg.chan != chan || msg.from != login {
        push_chat(ctx, login, msg);
    }
}

fn push_chat(ctx: &mut Ctx, login: &str, msg: ChatMsg) {
    append_chat(login, &msg);
    let mut s = ctx.snap.lock().unwrap();
    let thread = msg.thread.clone();
    s.chat.push_back(msg);
    if s.chat.iter().filter(|m| m.thread == thread).count() > CHAT_IN_MEMORY {
        if let Some(i) = s.chat.iter().position(|m| m.thread == thread) {
            s.chat.remove(i);
        }
    }
    drop(s);
    ctx.dirty = true;
}

/// Caps each thread separately so a busy analysis channel cannot push out conversations.
fn keep_latest_per_thread(msgs: Vec<ChatMsg>, n: usize) -> Vec<ChatMsg> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut keep: Vec<ChatMsg> = msgs
        .into_iter()
        .rev()
        .filter(|m| {
            let c = seen.entry(m.thread.clone()).or_default();
            *c += 1;
            *c <= n
        })
        .collect();
    keep.reverse();
    keep
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
        let dev = match toks[ri].split_once('@') {
            Some((_, rest)) if !rest.is_empty() => parse_dev_token(rest),
            Some(_) => toks.get(ri + 1).and_then(|t| parse_dev_token(t)),
            None => None,
        };
        if name == login {
            my_rating = Some(r);
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
            rating: Some(r),
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
        let (names, ratings, gtype) = match_line_parts(rest);
        list.push(OngoingView {
            id: id.to_string(),
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
    if std::env::var("KUROOBI_GGS_AUTOWATCH").as_deref() == Ok("auto") {
        ctx.auto_watch.extend(list.iter().map(|o| o.id.clone()));
    }
    s.ongoing = list;
    drop(s);
    ctx.dirty = true;
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
        // "1722.1@ 76.1=": a deviation under 100 is padded, so it lands in the next token.
        let (rating, dev) = match t[2].split_once('@') {
            Some((r, d)) => {
                let d = if d.is_empty() {
                    t.get(3).copied().unwrap_or("")
                } else {
                    d
                };
                (
                    r.parse::<f32>().unwrap_or(0.0),
                    d.trim_end_matches(['=', '+']).parse::<f32>().unwrap_or(0.0),
                )
            }
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
    for l in buf {
        let t = l.trim_end();
        if t.is_empty() || t == ":" {
            continue;
        }
        let t = t.strip_prefix(": ").unwrap_or(t);
        let lower = t.to_lowercase();
        if lower.starts_with("passw") {
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if !k.is_empty() && !k.contains(' ') || k.contains('(') {
                fields.push((k.to_string(), v.to_string()));
            }
        }
    }
    let mut s = ctx.snap.lock().unwrap();
    s.fingers.insert(name.to_string(), FingerInfo { fields });
    drop(s);
    ctx.dirty = true;
}

/// Asks once for the observers of each match that has appeared, and drops the ones that are gone.
fn ask_watchers(ctx: &mut Ctx, w: &mut Wire, send: &mut impl FnMut(String)) {
    let ids: std::collections::HashSet<String> =
        w.matches.keys().map(|k| base_id(k).to_string()).collect();
    let before = ctx.watchers.len();
    ctx.watchers.retain(|id, _| ids.contains(id));
    ctx.watch_asked.retain(|id| ids.contains(id));
    let mut ask: Vec<String> = ids
        .into_iter()
        .filter(|id| !ctx.watch_asked.contains(id))
        .collect();
    ask.sort();
    for id in ask {
        ctx.watch_asked.insert(id.clone());
        w.pending.push(format!("watchers:{id}"));
        say(ctx, send, format!("tell /os watch {id}"));
    }
    if ctx.watchers.len() != before {
        publish_watchers(ctx);
    }
}

fn publish_watchers(ctx: &mut Ctx) {
    let view: HashMap<String, Vec<String>> = ctx
        .watchers
        .iter()
        .map(|(id, names)| (id.clone(), names.iter().cloned().collect()))
        .collect();
    ctx.snap.lock().unwrap().watchers = view;
    ctx.dirty = true;
}

/// `/os:  + root watch .7` (someone began watching) or `/os:  - root watch .7` (stopped, or left).
fn watch_notice(ln: &str) -> Option<(bool, &str, &str)> {
    let mut t = ln.strip_prefix("/os:")?.split_whitespace();
    let on = match t.next()? {
        "+" => true,
        "-" => false,
        _ => return None,
    };
    let name = t.next()?;
    if t.next()? != "watch" {
        return None;
    }
    let id = t.next()?;
    (id.starts_with('.') && t.next().is_none()).then_some((on, name, id))
}

/// The answer to `watch .7 .9`: `/os: watch 2`, then `|.7        2 : root tdir` per match.
fn watch_list(buf: &[String]) -> Vec<(String, Vec<String>)> {
    buf.iter()
        .filter_map(|l| {
            let rest = l.strip_prefix('|')?;
            let (head, names) = rest.split_once(':')?;
            let id = head.split_whitespace().next()?;
            id.starts_with('.').then(|| {
                (
                    id.to_string(),
                    names.split_whitespace().map(str::to_string).collect(),
                )
            })
        })
        .collect()
}

fn finish_capture(ctx: &mut Ctx, kind: &str, buf: &[String], login: &str) {
    if kind == "online" {
        let names = online_names(buf);
        if !names.is_empty() {
            ctx.snap.lock().unwrap().online = Some(names);
            ctx.dirty = true;
        }
        return;
    }
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
    if kind.starts_with("watchers:") {
        for (id, names) in watch_list(buf) {
            ctx.watchers.insert(id, names.into_iter().collect());
        }
        publish_watchers(ctx);
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

    #[test]
    fn reads_watch_notices_and_lists() {
        assert_eq!(
            super::watch_notice("/os:  + root watch .7"),
            Some((true, "root", ".7"))
        );
        assert_eq!(
            super::watch_notice("/os:  - tdir watch .12"),
            Some((false, "tdir", ".12"))
        );
        assert_eq!(
            super::watch_notice("/os: + match .7 1667 regu 1690 bob s8r14 R"),
            None
        );
        assert_eq!(super::watch_notice("/os:  + root watch"), None);
        let buf: Vec<String> = ["/os: watch 2", "|.7        2 : root tdir", "|.9        0 :"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            super::watch_list(&buf),
            vec![
                (
                    ".7".to_string(),
                    vec!["root".to_string(), "tdir".to_string()]
                ),
                (".9".to_string(), vec![]),
            ]
        );
    }

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
    fn a_log_channel_is_a_dot_and_up_to_seven_name_characters() {
        for ok in ["", ".kuroobi", ".Rhapsod", ".a", ".my_bot1", ".x-y_z"] {
            assert!(valid_log_channel(ok), "{ok:?}");
        }
        for bad in [
            "kuroobi",
            ".",
            ".Rhapsody",
            " .kuroobi",
            ".kuro obi",
            "..kuroo",
            ".くろおび",
        ] {
            assert!(!valid_log_channel(bad), "{bad:?}");
        }
    }

    #[test]
    fn probcut_margins_read_as_their_confidence() {
        assert_eq!([1.5, 2.0, 2.5].map(confidence_pct), [87, 95, 99]);
    }

    #[test]
    fn the_search_log_follows_edax_s_layout() {
        let sq = |t: &str| Position::from_kifu(t).ok();
        let s = |empties, black| Searched {
            empties,
            threads: 8,
            nodes: 2_149_000_000,
            took: Duration::from_millis(4300),
            pv: vec![sq("f5"), sq("d6"), None, sq("c3")],
            black,
        };
        let mid = kuroobi::engine::MoveEval {
            pos: sq("f5"),
            value: 2.4,
            depth: 22,
            ..Default::default()
        };
        assert_eq!(
            search_log(".kuroobi", "kuroobi", ".1.0", &mid, &s(40, true)),
            [
                "tell .kuroobi -----------------------------------------",
                "tell .kuroobi kuroobi plays F5 in game .1.0 using 8 threads ;",
                "tell .kuroobi score == +2 at 22@73% ; PV: F5 d6 PA c3 ;",
                "tell .kuroobi nodes: 2.149 GN ; time: search = 4.3s, move = 4.3s; speed: 499.8 MN/s.",
                "tell .kuroobi search completed",
            ]
        );
        let sel = kuroobi::engine::MoveEval {
            pos: sq("f5"),
            value: -1.0,
            depth: 32,
            mpc_t: Some(2.5),
            cut: true,
            ..Default::default()
        };
        let lines = search_log(".kuroobi", "kuroobi", ".1.0", &sel, &s(32, false));
        assert_eq!(
            lines[1],
            "tell .kuroobi kuroobi plays f5 in game .1.0 using 8 threads ;"
        );
        assert_eq!(
            lines[2],
            "tell .kuroobi score == -1 at 32@99% ; PV: f5 D6 pa C3 ;"
        );
        assert_eq!(lines[4], "tell .kuroobi search out of time at 32@99%");
        let exact = kuroobi::engine::MoveEval {
            pos: sq("f5"),
            value: 4.0,
            exact: true,
            ..Default::default()
        };
        assert!(
            search_log(".kuroobi", "kuroobi", ".1.0", &exact, &s(26, true))[2]
                .starts_with("tell .kuroobi score == +4 at 26@100% ;")
        );
        let book = kuroobi::engine::MoveEval {
            pos: sq("f5"),
            value: 0.5,
            from_book: true,
            ..Default::default()
        };
        assert_eq!(
            search_log(".kuroobi", "kuroobi", ".1.0", &book, &s(50, true))[2..],
            ["tell .kuroobi score == +1 from book ;"]
        );
    }

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
    fn a_board_record_carries_each_move_s_eval_and_time() {
        let mut m = MatchState::new();
        m.moves = [
            (1u32, "f5".to_string()),
            (2, "d6".to_string()),
            (3, "pa".to_string()),
        ]
        .into();
        m.move_evals.insert(1, (Some(1.5), Some(2.25)));
        m.move_evals.insert(2, (Some(-2.0), None));

        let full = m.ggf_with(".7.1", Some("+4.00"), true);
        assert!(full.contains("B[F5/1.50/2.25]W[D6/-2.00]B[PA]"), "{full}");
        assert!(m.ggf(".7.1", None).contains("B[F5]W[D6]B[PA]"));
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
    fn the_legal_squares_are_the_side_to_moves() {
        let mut m = MatchState::new();
        m.cells = vec![0; 64];
        for (sq, c) in [(27, 2), (36, 2), (28, 1), (35, 1)] {
            m.cells[sq] = c;
        }
        m.turn = '*';
        assert_eq!(legal_squares(&m), vec![19, 26, 37, 44], "c4 d3 e6 f5");
        m.over = true;
        assert!(legal_squares(&m).is_empty());
    }

    #[test]
    fn the_eval_series_carries_the_time_each_move_took() {
        use std::collections::BTreeMap;
        let mut m = MatchState::new();
        m.moves = (1..=2u32)
            .map(|n| (n, "f5".to_string()))
            .collect::<BTreeMap<_, _>>();
        m.eval_parity = Some(1);
        m.move_evals.insert(1, (Some(1.5), Some(84.49)));
        m.move_evals.insert(2, (None, None));

        let series = m.eval_series();
        assert_eq!(series[0].secs, Some(84.49));
        assert_eq!(series[1].secs, None);
    }

    #[test]
    fn the_eval_series_leaves_passes_out() {
        let mut m = MatchState::new();
        m.moves = [(1u32, "f5"), (2, "pa"), (3, "d6")]
            .into_iter()
            .map(|(n, mv)| (n, mv.to_string()))
            .collect();
        m.eval_parity = Some(1);
        m.move_evals.insert(1, (Some(2.0), None));
        m.move_evals.insert(3, (Some(4.0), None));

        let series = m.eval_series();
        assert_eq!(
            series
                .iter()
                .map(|p| (p.n, p.mine, p.eval))
                .collect::<Vec<_>>(),
            vec![(1, true, Some(2.0)), (2, true, Some(4.0))],
            "a pass is not a move: two discs, two points, both mine"
        );
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
    fn a_finished_watched_board_keeps_its_eval_series() {
        let mut m = MatchState::new();
        for n in 1..=4u32 {
            m.moves.insert(n, "d3".into());
            m.move_evals.insert(n, (Some(n as f32), None));
        }
        m.start_turn = '*';
        m.turn = '*';
        let live: Vec<bool> = m.eval_series().iter().map(|p| p.mine).collect();
        m.turn = ' ';
        let over: Vec<bool> = m.eval_series().iter().map(|p| p.mine).collect();
        assert_eq!(
            live,
            [true, false, true, false],
            "black's moves are the observer's side"
        );
        assert_eq!(over, live, "the series survives the end of the game");
    }

    #[test]
    fn a_server_refusal_gets_words_when_it_is_known() {
        assert_eq!(
            ggs_err_notice("match with same parameters is already pending."),
            "err.ggs.dup_request"
        );
        assert_eq!(
            ggs_err_notice("user 'zed' not found."),
            "err.ggs.no_user|name=zed"
        );
        assert_eq!(
            ggs_err_notice("odd thing."),
            "err.ggs_refused|msg=odd thing."
        );
    }

    #[test]
    fn a_board_result_is_kept_from_blacks_side() {
        let mut m = MatchState::new();
        for (name, color) in [("bob", "black"), ("alice", "white")] {
            m.players.push(PlayerView {
                name: name.into(),
                rating: String::new(),
                clock: String::new(),
                color: color.into(),
                secs: None,
                ext: None,
            });
        }
        // GGS scores "( alice vs. bob ) +4.00" from alice's side; alice is white here.
        assert_eq!(black_view(&m, "alice", 4.0), "-4.00");
        assert_eq!(black_view(&m, "bob", 4.0), "+4.00");
        assert_eq!(black_view(&MatchState::new(), "alice", 4.0), "+4.00");
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

        let mut both = finish_match(&mut ms, ".4", "a", Some(-4.0), "finished", "", ".85929");
        both.sort_by(|a, b| a.0.cmp(&b.0));
        let ids: Vec<&str> = both.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            ids,
            [".4.0", ".4.1"],
            "a synchro match hands back both boards"
        );
        assert_eq!(ms[".4.0"].result, "+10.00");
        assert_eq!(ms[".4.1"].result, "-18.00");

        let mut ms2: HashMap<String, MatchState> = HashMap::new();
        ms2.insert(".9.0".into(), MatchState::new());
        finish_match(&mut ms2, ".9", "a", Some(2.0), "finished", "", "");
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
    fn auto_match_does_not_start_without_an_opponent() {
        let cfg = |opponent: &str| StandbyCfg {
            enabled: true,
            opponent: opponent.into(),
            ..Default::default()
        };
        assert!(!cfg("").needing_opponent().enabled);
        assert!(!cfg("  ").needing_opponent().enabled);
        assert!(cfg("iEdax").needing_opponent().enabled);
    }

    #[test]
    fn only_our_request_to_the_auto_match_opponent_counts_as_refused() {
        let line = "+  .50 2589.7 kuroobi  03:00//02:00    s8r14 R 2610.4 rho";
        let o = parse_offer(line.trim_start_matches("+ ").trim(), "kuroobi").unwrap();
        assert!(ask_to(&o, "kuroobi", "rho"));
        assert!(!ask_to(&o, "kuroobi", "piglet"));
        let theirs = parse_offer(
            ".51 2610.4 rho  03:00//02:00    s8r14 R 2589.7 kuroobi",
            "kuroobi",
        )
        .unwrap();
        assert!(!ask_to(&theirs, "kuroobi", "rho"));
        let open = parse_offer(
            ".52 2589.7 kuroobi  03:00//02:00    s8r14 R *      *",
            "kuroobi",
        )
        .unwrap();
        assert!(!ask_to(&open, "kuroobi", "rho"));
    }

    #[test]
    fn the_servers_who_lists_every_login_but_not_the_services() {
        let buf: Vec<String> = [
            ": who   17        idle      online         ip-addr hostname",
            "/os      +    00:00:12312.04:55:04",
            "HCyrano  +    00:02:29  1.20:22:29",
            "kuroobi  +    00:00:00    00:06:42 153.242.133.003  <=",
            "nyanyan3 -    00:00:12    06:02:18",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(online_names(&buf), ["HCyrano", "kuroobi", "nyanyan3"]);
    }

    #[test]
    fn a_match_line_pairs_each_name_with_its_rating_not_the_id() {
        let (names, ratings, gtype) =
            match_line_parts(".23 2571 Rhapsody 2415 piglet      s8r16  R 1");
        assert_eq!(names, ["Rhapsody", "piglet"]);
        assert_eq!(ratings, ["2571", "2415"]);
        assert_eq!(gtype, "s8r16");
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
        let mut m3 = MatchState::new();
        apply_block(&mut m3, &with_clock("04:59,0:0"), "kuroobi");
        apply_block(&mut m3, &with_clock("05:00,1:0"), "kuroobi");
        assert!(
            !m3.in_overtime,
            "a second back after a book move is not overtime"
        );
    }

    #[test]
    fn td_follows_a_tournament_from_announcement_to_end() {
        let mut td = TdView::default();
        let me = "kuroobi";
        td_apply(
            &mut td,
            me,
            "tournament 31 starting in 5 minutes\n(8x8 SynchroRand Reversi clock: 3:00//0:30, 10 rounds rrobin)\nto play: tell /td join 31\nfor tournament messages: chann + .tourney",
            false,
            1000,
        );
        let t = &td.tourneys[0];
        assert_eq!(
            (t.id, t.state.as_str(), t.next_at),
            (31, "announced", Some(1300))
        );
        assert_eq!(
            t.desc,
            "8x8 SynchroRand Reversi clock: 3:00//0:30, 10 rounds rrobin"
        );

        td_apply(&mut td, me, "join: egrcd joins tournament 31", false, 1005);
        td_apply(&mut td, me, "join: kuroobi joins tournament 31", true, 1010);
        assert!(td.tourneys[0].joined);
        assert_eq!(td.tourneys[0].n_players, 2);
        assert_eq!(td.reply.as_ref().unwrap().cmd, "join");

        td_apply(&mut td, me, "tournament 31 starts in  1:00", false, 1240);
        assert_eq!(td.tourneys[0].next_at, Some(1300));
        td_apply(
            &mut td,
            me,
            "starting round 1 of tournament 31",
            false,
            1300,
        );
        assert_eq!(td.tourneys[0].state, "running");
        td_apply(
            &mut td,
            me,
            "ending round 1 of tournament 31\nround   1:\n  1.1  kuroobi   egrcd    +2.0",
            false,
            1500,
        );
        let t = &td.tourneys[0];
        assert_eq!((t.state.as_str(), t.rounds_played), ("break", 1));
        td_apply(
            &mut td,
            me,
            "tournament 31 round 2 starts in  1:00",
            false,
            1500,
        );
        assert_eq!(td.tourneys[0].next_at, Some(1560));
        td_apply(
            &mut td,
            me,
            "Congratulations to kuroobi, winner of tournament 31",
            false,
            9000,
        );
        let t = &td.tourneys[0];
        assert_eq!((t.state.as_str(), t.winner.as_str()), ("over", "kuroobi"));
    }

    #[test]
    fn td_reads_list_finger_and_errors() {
        let mut td = TdView::default();
        td_apply(
            &mut td,
            "kuroobi",
            "tourneys: 2\n   id director  game #pl   round start             clock       type\n   27  HCyrano    os   7  10/ 10  done        3:00//0:30      s8r14   Winner: ymatioun\n   31  HCyrano    os   3   0/ 10  4:10        3:00//0:30      s8r14\n   29  HCyrano   /os   2   2/  2  done        1:00//0:30      s8r14   Winner: iEdax\n   11  HCyrano   /os   6 100/100  done        1:00//2:00      s8r14   Winner: ymatioun",
            true,
            100,
        );
        assert_eq!(td.tourneys.len(), 4);
        let t29 = td.tourneys.iter().find(|t| t.id == 29).unwrap();
        assert_eq!(
            (t29.rounds_played, t29.rounds, t29.state.as_str()),
            (2, 2, "over")
        );
        let t11 = td.tourneys.iter().find(|t| t.id == 11).unwrap();
        assert_eq!(
            (
                t11.state.as_str(),
                t11.rounds,
                t11.gtype.as_str(),
                t11.winner.as_str()
            ),
            ("over", 100, "s8r14", "ymatioun")
        );
        let t27 = td.tourneys.iter().find(|t| t.id == 27).unwrap();
        assert_eq!(
            (t27.state.as_str(), t27.winner.as_str(), t27.rounds),
            ("over", "ymatioun", 10)
        );
        let t31 = td.tourneys.iter().find(|t| t.id == 31).unwrap();
        assert_eq!(
            (t31.state.as_str(), t31.n_players, t31.clock.as_str()),
            ("announced", 3, "3:00//0:30")
        );

        td_apply(
            &mut td,
            "kuroobi",
            "finger: 31\nt kuroobi finger: \nid        : 31\njoining   : open\nrounds    : 0/10\nplayers   : 3 egrcd kuroobi (ymatioun) \nround 1 begins in  4:00",
            true,
            200,
        );
        let t31 = td.tourneys.iter().find(|t| t.id == 31).unwrap();
        assert!(t31.joined);
        assert_eq!(t31.players, ["egrcd", "kuroobi", "ymatioun"]);

        td_apply(
            &mut td,
            "kuroobi",
            "ERR 7: join: tournament id does not exist",
            true,
            300,
        );
        let r = td.reply.as_ref().unwrap();
        assert!(r.err);
        assert_eq!(
            (r.cmd.as_str(), r.text.as_str()),
            ("join", "tournament id does not exist")
        );
    }

    #[test]
    fn td_announcement_keeps_its_body() {
        let mut block = None;
        let lines = [
            ".chat /td: ",
            "|---------------- /td --------------",
            "|tournament 30 starting in 2 minutes",
            "|to play: tell /td join 30",
            "|---------------- /td --------------",
            "READY",
        ];
        let mut got = Vec::new();
        for ln in lines {
            got.extend(take_chat_line(&mut block, ln).0);
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].from, "/td");
        assert_eq!(
            got[0].text,
            "tournament 30 starting in 2 minutes\nto play: tell /td join 30"
        );
        assert!(block.is_none());
    }

    #[test]
    fn busy_channel_does_not_evict_other_threads() {
        let msg = |thread: &str, at: u64| ChatMsg {
            chan: thread.into(),
            from: "x".into(),
            text: "t".into(),
            at,
            thread: thread.into(),
        };
        let mut v = vec![msg(".chat", 0)];
        v.extend((1..=10).map(|i| msg(".Harmony", i)));
        let kept = keep_latest_per_thread(v, 3);
        assert_eq!(kept.iter().filter(|m| m.thread == ".chat").count(), 1);
        let harmony: Vec<u64> = kept
            .iter()
            .filter(|m| m.thread == ".Harmony")
            .map(|m| m.at)
            .collect();
        assert_eq!(harmony, [8, 9, 10]);
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

    #[test]
    fn a_full_board_is_not_a_move_to_play() {
        let full = Board::from_string(&format!("{} *", "*".repeat(64))).unwrap();
        assert!(game_over(&full));
        assert!(!game_over(&Board::new()), "the opening has moves");
    }

    #[test]
    fn a_reused_match_number_starts_from_a_clean_board() {
        let mut matches = HashMap::new();
        let mut old = MatchState::new();
        old.over = true;
        old.in_overtime = true;
        matches.insert(".10.0".to_string(), old);
        forget_finished_on_join(&mut matches, "/os: update .10.0 s8r16 K?", ".10.0");
        assert!(matches.contains_key(".10.0"), "an update is the same game");
        forget_finished_on_join(&mut matches, "/os: join .10.0 s8r16 K?", ".10.0");
        assert!(
            !matches.contains_key(".10.0"),
            "a join after the end is a new game"
        );

        let mut live = MatchState::new();
        live.in_overtime = true;
        matches.insert(".10.1".to_string(), live);
        forget_finished_on_join(&mut matches, "/os: join .10.1 s8r16 K?", ".10.1");
        assert!(
            matches.get(".10.1").is_some_and(|m| m.in_overtime),
            "a game still running keeps its state"
        );
    }

    #[test]
    fn a_result_takes_the_rating_the_server_announced_after_it() {
        let ln = "|kuroobi  2476.63 @  35.96    -5.90 -> 2470.74 @  35.80";
        assert_eq!(rating_after(ln, "kuroobi"), Some(2470.74));
        assert_eq!(rating_after(ln, "Rhapsody"), None);
        assert_eq!(rating_after("|kuroobi  2476.63 @  35.96", "kuroobi"), None);
    }

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
