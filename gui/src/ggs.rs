//! GGS session thread (Generic Game Server, skatgame.net:5000).

use crate::formula;
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

use crate::{append_line, now_secs, resources};

mod chat;
mod demo;
mod replies;
mod results;
mod store;
mod td;
mod unfit;

pub use demo::demo_snapshot;
pub use store::{
    account_password, auto_account, auto_key, clear_auto, forget_account, forget_server,
    save_server, saved_accounts, saved_servers, session_locked_by_other, set_auto, SavedAccount,
    SavedServer,
};
pub use unfit::{unfit_view, UnfitView};

use chat::*;
use replies::*;
use results::*;
use store::*;
use td::*;
use unfit::*;

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

fn no_rated() -> bool {
    std::env::var("KUROOBI_NO_RATED").is_ok()
}

fn rated_cmd(rated: bool) -> &'static str {
    if rated && !no_rated() {
        "tell /os rated +"
    } else {
        "tell /os rated -"
    }
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
    pub in_extension: bool,
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
    /// The server's refusal of the last aform/dform we sent ("dform|O2.e{REAL}!={INT}"), else empty.
    pub formula_refused: String,
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

static MATCH_ORDER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    in_extension: bool,
    /// Who used up the main time first (`timeout`) and who used up the extension too (`fatal-timeout`).
    over_time_by: String,
    out_of_time_by: String,
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
                    // Over: count from who moved first; GGS numbers a pass like a move.
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
            in_extension: false,
            over_time_by: String::new(),
            out_of_time_by: String::new(),
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
        let side = if self.start_turn == 'O' { 'O' } else { 'X' };
        format!("{} {side}", cells_text(&self.start_cells))
    }

    fn ggf(&self, id: &str, result: Option<&str>, evals: bool) -> String {
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
            let mut m = if is_pass(mv) {
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
            .filter(|m| !is_pass(m))
            .map(|m| m.to_lowercase())
            .collect()
    }
}

fn my_stone_diff(score: f32, first_name: &str, login: &str) -> i32 {
    let v = score.round() as i32;
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
    Position::from_kifu(&you.moves.get(&n)?.to_ascii_lowercase()).ok()
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
    p.to_kifu().to_uppercase()
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

struct Done(Box<kuroobi::engine::MoveEval>, Report);

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
    /// The auto-match opponent whose finger was fetched to check the request; fetched once only.
    auto_fingered: String,
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
    fn spawn(cfg: EngineConfig) -> Result<EngineWorker, String> {
        let res = resources();
        let mut cfg = with_resources(cfg, &res);
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
                        if dtx.send(Done(Box::new(mv), r)).is_err() {
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
        let _ = self.app.emit("ggs", &s);
        self.last_emit = Instant::now();
        self.dirty = false;
    }
    fn ensure_engine(&mut self) -> Result<(), String> {
        if self.engine.is_none() {
            let engine = Engine::new(with_resources(self.engine_cfg.clone(), &resources()))?;
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
            threads: res.threads.unwrap_or_else(crate::auto_threads),
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
        auto_fingered: String::new(),
    };
    ctx.snap.lock().unwrap().engine = EngineCfgView {
        threads: ctx.engine_cfg.threads,
        use_book: ctx.engine_cfg.use_book,
        book_loaded: res.book_path().exists(),
        ..Default::default()
    };
    ctx
}

fn run(
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
            Ok(Cmd::SetLogChannel(c)) if valid_log_channel(&c) => {
                ctx.snap.lock().unwrap().log_channel = c;
                save_settings(ctx);
            }
            Ok(cmd) => {
                if !set_pref(ctx, &cmd) {
                    continue;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                learn_tick(ctx);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
        ctx.emit(true);
    }
}

/// A setting kept in `ggs_settings.json`; false for any other command.
fn set_pref(ctx: &mut Ctx, cmd: &Cmd) -> bool {
    match *cmd {
        Cmd::SetAutoPlay(b) => ctx.snap.lock().unwrap().auto_play = b,
        Cmd::SetOpen(b) => ctx.snap.lock().unwrap().open = b,
        Cmd::SetUseBook(b) => ctx.set_use_book(b),
        Cmd::SetWatchAnalysis(b) => ctx.snap.lock().unwrap().watch_analysis = b,
        Cmd::SetLearn(b) => ctx.snap.lock().unwrap().engine.learn = b,
        _ => return false,
    }
    save_settings(ctx);
    true
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
                    |c| write_line(&mut writer, &c),
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
                login_step(ctx, &mut w, login, pw, |c| write_line(&mut writer, &c));
                if w.login_warned {
                    return;
                }
                continue;
            }
            while let Some(ln) = w.lines.pop_front() {
                dispatch_line(ctx, &mut w, &ln, login, pw, &mut cred_saved, |c| {
                    write_line(&mut writer, &c)
                });
            }
            standby_tick(ctx, &mut w, |c| write_line(&mut writer, &c));
            service(ctx, &mut w, |c| write_line(&mut writer, &c));
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

/// Sends a command whose answer is captured under `key` (see `capture_header_matches`).
fn query(
    pending: &mut Vec<String>,
    key: impl Into<String>,
    cmd: impl Into<String>,
    mut send: impl FnMut(String),
) {
    pending.push(key.into());
    send(cmd.into());
}

fn ask_who(pending: &mut Vec<String>, mut send: impl FnMut(String)) {
    for t in ["8", "8r"] {
        query(
            pending,
            format!("who:{t}"),
            format!("tell /os who {t}"),
            &mut send,
        );
    }
    query(pending, "online", "who", send);
}

/// Both fingers: the server's (`decline` formula and the like) and the Othello service's.
fn ask_finger(pending: &mut Vec<String>, name: &str, mut send: impl FnMut(String)) {
    query(
        pending,
        format!("finger:{name}"),
        format!("finger {name}"),
        &mut send,
    );
    query(
        pending,
        format!("osfinger:{name}"),
        format!("tell /os finger {name}"),
        send,
    );
}

fn greet(ctx: &mut Ctx, w: &mut Wire, login: &str, send: &mut impl FnMut(String)) {
    for c in [
        "verbose -news -faq -help -ack",
        "tell /os client -",
        "tell /os trust +",
        rated_cmd(true),
        "tell /os notify +",
        // Observers coming and going then arrive as `+ name watch .id`, keeping the lists live.
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
        query(
            &mut w.pending,
            format!("rank:{t}:{login}"),
            format!("tell /os rank {t} {login}"),
            |c| say(ctx, send, c),
        );
    }
    for (key, cmd) in [
        ("stored_list", "tell /os stored"),
        ("match_list", "tell /os match"),
        ("history:", "tell /os history"),
    ] {
        query(&mut w.pending, key, cmd, |c| say(ctx, send, c));
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
    // The opponent's decline formula is checked here first, so a request it would refuse is not sent.
    let (read, unfit) = {
        let s = ctx.snap.lock().unwrap();
        (
            s.fingers
                .get(&sb.opponent)
                .is_some_and(|f| f.fields.iter().any(|(k, _)| k == "decline")),
            unfit_ask(&s, &sb.gtype, &sb.time, &sb.opponent, sb.rated),
        )
    };
    if !read && ctx.auto_fingered != sb.opponent {
        ctx.auto_fingered = sb.opponent.clone();
        ask_finger(&mut w.pending, &sb.opponent, |c| say(ctx, &mut send, c));
        w.next_ask_at = Some(Instant::now() + Duration::from_secs(2));
        return;
    }
    if let Some(o) = unfit {
        stop_auto_match_unfit(ctx, Some(&o));
        return;
    }
    ctx.log("info", &format!("auto match: asking {}", sb.opponent));
    ctx.ask_matched = false;
    ctx.auto_asked = true;
    say(ctx, &mut send, rated_cmd(sb.rated));
    say(
        ctx,
        &mut send,
        format!("tell /os ask {} {} {}", sb.gtype, sb.time, sb.opponent),
    );
}

fn service(ctx: &mut Ctx, w: &mut Wire, mut send: impl FnMut(String)) {
    if Instant::now() >= w.next_match_at {
        w.next_match_at = Instant::now() + Duration::from_secs(60);
        query(&mut w.pending, "match_list", "tell /os match", |c| {
            say(ctx, &mut send, c)
        });
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
            send(rated_cmd(rated).to_string());
            let cmd = if opponent.is_empty() {
                format!("tell /os ask {gtype} {time}")
            } else {
                format!("tell /os ask {gtype} {time} {opponent}")
            };
            send(cmd)
        }
        Cmd::Accept(id) => send(format!("tell /os accept {id}")),
        Cmd::Decline(id) => send(format!("tell /os decline {id}")),
        Cmd::Finger(name) => ask_finger(pending, &name, send),
        Cmd::Who => ask_who(pending, send),
        Cmd::Top { gtype, n } => query(pending, "top", format!("tell /os top {gtype} {n}"), send),
        Cmd::Rank { gtype, name } => query(
            pending,
            format!("rank:{gtype}:{name}"),
            format!("tell /os rank {gtype} {name}"),
            send,
        ),
        Cmd::Look(id) => query(
            pending,
            format!("look:{id}"),
            format!("tell /os look {id}"),
            send,
        ),
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
        Cmd::SetAutoPlay(_)
        | Cmd::SetOpen(_)
        | Cmd::SetUseBook(_)
        | Cmd::SetWatchAnalysis(_)
        | Cmd::SetLearn(_) => {
            set_pref(ctx, &cmd);
            if let Cmd::SetOpen(b) = cmd {
                send(format!("tell /os open {}", u8::from(b)));
            }
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
        Cmd::ListStored => query(pending, "stored_list", "tell /os stored", send),
        Cmd::ListMatches => query(pending, "match_list", "tell /os match", send),
        Cmd::ResumeStored(id) => {
            ctx.log("info", &format!("resuming adjourned game {id}"));
            send(format!("tell /os ask {id}"));
        }
        Cmd::History(name) => {
            let cmd = if name.is_empty() {
                "tell /os history".to_string()
            } else {
                format!("tell /os history {name}")
            };
            query(pending, format!("history:{name}"), cmd, send);
        }
        Cmd::SetFormula { kind, expr } => {
            ctx.snap.lock().unwrap().formula_refused.clear();
            ctx.dirty = true;
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
            let opponent = || {
                match_line_parts(mrest)
                    .0
                    .into_iter()
                    .find(|n| n != login)
                    .unwrap_or_default()
            };
            if mrest.split_whitespace().any(|t| t == login) {
                let opp = ctx.snap.lock().unwrap().standby.opponent.clone();
                if !opp.is_empty() && mrest.split_whitespace().any(|t| t == opp) {
                    ctx.ask_matched = true;
                    ctx.ask_refusals = 0;
                }
                // A game with someone we had adjourned with resumes it: the stored list and old boards are stale.
                let opp = opponent();
                let resumed = ctx.snap.lock().unwrap().stored.iter().any(|x| x.opp == opp);
                if resumed {
                    query(pending, "stored_list", "tell /os stored", &mut send);
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
                            games.push(TdGame {
                                id: b,
                                tid,
                                opp: opponent(),
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
    } else if let Some((id, name, fatal)) = timeout_line(ln) {
        if let Some(m) = matches.get_mut(id) {
            if fatal {
                m.out_of_time_by = name.to_string();
            } else if m.over_time_by.is_empty() {
                m.over_time_by = name.to_string();
            }
        }
    } else if let Some(rest) = ln.strip_prefix("/os: end ") {
        let (id, score, first) = end_line(rest);
        if let (Some(v), Some(m)) = (score, matches.get_mut(id)) {
            m.result = black_view(m, first, v);
            // One board of a synchro pair can end before the match does; its clocks stop here.
            m.over = true;
            m.turn = ' ';
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
    } else if let Some((kind, why)) = formula_refusal(ln) {
        ctx.log("info", &format!("server error: {ln}"));
        ctx.snap.lock().unwrap().formula_refused = format!("{kind}|{why}");
        ctx.dirty = true;
    } else if ln.starts_with("/os: ERR") {
        ctx.log("info", &format!("server error: {ln}"));
        if ln.contains("not found") && ln.contains("match") {
            return;
        }
        let msg = ln.trim_start_matches("/os: ERR").trim();
        if std::mem::take(&mut ctx.auto_asked)
            && msg.contains("doesn't fit the opponent's formula")
            && stop_auto_match_unfit(ctx, None)
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
/// can never succeed, so auto match stops at the first one. `why` is the outline found before
/// sending; `None` when it was the server that refused.
fn stop_auto_match_unfit(ctx: &mut Ctx, why: Option<&formula::Outline>) -> bool {
    let mut s = ctx.snap.lock().unwrap();
    if !s.standby.enabled {
        return false;
    }
    let opp = s.standby.opponent.clone();
    s.standby.enabled = false;
    s.notice = match why {
        None => format!("err.auto_match_unfit|name={opp}"),
        Some(o) => format!(
            "err.auto_match_unfit_why|name={opp}|why={}",
            hit_words(o, &opp).replace('|', "/")
        ),
    };
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
/// A formula the server would not take: `/os: dform ERR O2.e{REAL}!={INT}""` (not `/os: ERR`).
fn formula_refusal(ln: &str) -> Option<(&str, &str)> {
    ["aform", "dform"].into_iter().find_map(|kind| {
        let rest = ln.strip_prefix("/os: ")?.strip_prefix(kind)?;
        Some((kind, rest.trim_start().strip_prefix("ERR")?.trim()))
    })
}

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

fn write_line(writer: &mut TcpStream, cmd: &str) {
    let _ = writer
        .write_all(cmd.as_bytes())
        .and_then(|_| writer.write_all(b"\n"));
}

/// The thread count resolved and the files the resource settings point at.
fn with_resources(mut cfg: EngineConfig, res: &kuroobi::resources::Resources) -> EngineConfig {
    cfg.threads = resolve_threads(cfg.threads);
    cfg.weights = res.weights_path();
    cfg.nnue = res.nnue_path();
    cfg.book = res.book_path();
    cfg
}

fn resolve_threads(n: usize) -> usize {
    if n == 0 {
        crate::auto_threads()
    } else {
        n
    }
}

fn apply_threads(ctx: &mut Ctx) {
    let n = resources().threads.unwrap_or_else(crate::auto_threads);
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
    let was_in_extension = m.in_extension;
    let (rows_ok, turn) = apply_block(m, block, login);
    if m.in_extension && !was_in_extension {
        ctx.log(
            "info",
            &format!("{mid}: in the extension; playing on with the extension as the clock"),
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

/// `cells` (`file * 8 + rank`) as a board string runs them: a1, b1 … h8.
fn cells_text(cells: &[u8]) -> String {
    (0..64)
        .map(|n| match cells[(n % 8) * 8 + n / 8] {
            1 => 'X',
            2 => 'O',
            _ => '-',
        })
        .collect()
}

fn board_of(m: &MatchState, turn: char) -> Option<Board> {
    if turn != '*' && turn != 'O' {
        return None;
    }
    Board::from_string(&format!("{} {turn}", cells_text(&m.cells))).ok()
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
                            // The server shows 4:59 at join and 5:00 after an instant book move; only a jump into the extension means it began.
                            let jumped = now <= g
                                && m.my_clock_secs
                                    .is_some_and(|prev| now > prev.saturating_add(bump));
                            let played = m.moves.len() >= 2;
                            if g > 0 && (jumped || (now == 0 && played)) {
                                m.in_extension = true;
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

/// GGS shows 0:00 in the extension until the extension's own countdown appears.
fn my_clock_secs(m: &MatchState) -> u64 {
    let secs = m.my_clock_secs.unwrap_or(0);
    if m.in_extension && secs == 0 {
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
        let Done(mv, r) = done;
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
            turn: color_word(Some(m.turn)).into(),
            my_color: color_word(m.my_color).into(),
            opp_name: m.opp_name.clone(),
            opp_rating: m.opp_rating.clone(),
            opp_clock: m.opp_clock.clone(),
            my_clock: m.my_clock.clone(),
            my_secs: m.my_clock_secs,
            opp_secs: m.opp_secs,
            my_ext: m.my_ext,
            opp_ext: m.opp_ext,
            in_extension: m.in_extension,
            players: m.players.clone(),
            gtype: m.gtype.clone(),
            ggf: m.ggf(id, None, false),
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

fn color_word(c: Option<char>) -> &'static str {
    match c {
        Some('*') => "black",
        Some('O') => "white",
        _ => "",
    }
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
    let (score, first_name) = match_score(rest);
    let first_name = first_name.unwrap_or("");
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
    let mut boards: Vec<BoardRecord> = dropped.iter().map(|(k, m)| board_record(k, m)).collect();
    boards.sort_by(|a, b| a.id.cmp(&b.id));
    let (kifu, ggf, opp) = match &m {
        Some(m) => (
            m.kifu(),
            m.ggf(&id, re.as_deref(), false),
            m.opp_name.clone(),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    let my_diff = score.map(|s| my_stone_diff(s, first_name, login));
    let (ending, ending_by) = match_ending(kind, &who, &dropped, my_diff, login, &opp);
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
        ending,
        ending_by,
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

fn apply_td(ctx: &mut Ctx, login: &str, text: &str, direct: bool) {
    let mut s = ctx.snap.lock().unwrap();
    td_apply(&mut s.td, login, text, direct, now_secs());
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
        query(
            &mut w.pending,
            format!("watchers:{id}"),
            format!("tell /os watch {id}"),
            |c| say(ctx, send, c),
        );
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

        let full = m.ggf(".7.1", Some("+4.00"), true);
        assert!(full.contains("B[F5/1.50/2.25]W[D6/-2.00]B[PA]"), "{full}");
        assert!(m.ggf(".7.1", None, false).contains("B[F5]W[D6]B[PA]"));
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
    fn a_refused_formula_is_told_apart_from_other_errors() {
        assert_eq!(
            formula_refusal("/os: dform ERR O2.e{REAL}!={INT}\"\""),
            Some(("dform", "O2.e{REAL}!={INT}\"\""))
        );
        assert_eq!(
            formula_refusal("/os: aform ERR E5.?: *"),
            Some(("aform", "E5.?: *"))
        );
        assert_eq!(formula_refusal("/os: ERR user 'zed' not found."), None);
        assert_eq!(formula_refusal("/os: dform rated"), None);
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
        assert_eq!(my_stone_diff(2.0, "kuroobi", "kuroobi"), 2);
        assert_eq!(my_stone_diff(-10.0, "kuroobi", "kuroobi"), -10);
        assert_eq!(my_stone_diff(-5.0, "kuroobi", "kuroobi"), -5);
        assert_eq!(my_stone_diff(-7.0, "kuroobi", "kuroobi"), -7);
        assert_eq!(my_stone_diff(-6.0, "htz", "kuroobi"), 6);
        assert_eq!(my_stone_diff(2.0, "htz", "kuroobi"), -2);
        assert_eq!(my_stone_diff(0.0, "kuroobi", "kuroobi"), 0);
        assert_eq!(my_stone_diff(0.0, "htz", "kuroobi"), 0);
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
    fn a_match_line_pairs_each_name_with_its_rating_not_the_id() {
        let (names, ratings, gtype) =
            match_line_parts(".23 2571 Rhapsody 2415 piglet      s8r16  R 1");
        assert_eq!(names, ["Rhapsody", "piglet"]);
        assert_eq!(ratings, ["2571", "2415"]);
        assert_eq!(gtype, "s8r16");
    }

    #[test]
    fn a_jumping_clock_means_the_extension() {
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
        assert!(!m.in_extension, "still in main time");
        apply_block(&mut m, &with_clock("02:00,0:0"), "kuroobi");
        assert!(m.in_extension, "missed the jump");
        let mut m2 = MatchState::new();
        apply_block(&mut m2, &with_clock("00:03,0:0"), "kuroobi");
        assert!(!m2.in_extension);
        apply_block(&mut m2, &with_clock("00:00,0:0"), "kuroobi");
        assert!(m2.in_extension, "missed the clock stuck at 0");
        apply_block(&mut m, &with_clock("01:12,0:0"), "kuroobi");
        assert!(m.in_extension, "cleared the extension flag");
        let mut m3 = MatchState::new();
        apply_block(&mut m3, &with_clock("04:59,0:0"), "kuroobi");
        apply_block(&mut m3, &with_clock("05:00,1:0"), "kuroobi");
        assert!(
            !m3.in_extension,
            "a second back after a book move is not the extension"
        );
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
        let ggf = m.ggf(".1", None, false);
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
            Some(-29),
            "the score belongs to the name written first; a pair's is the mean of its boards"
        );
        assert_eq!(
            diff_from_raw(
                ".21 2469 kuroobi 2591 Rhapsody s8r16 R +1.00  .86021",
                "Rhapsody"
            ),
            Some(1),
            "written first ourselves, the sign stands"
        );
        assert_eq!(
            diff_from_raw(".9 2400 kuroobi 2400 saio 8 R -3.00", "saio"),
            Some(-3),
            "a lone board's is its own"
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
        old.in_extension = true;
        matches.insert(".10.0".to_string(), old);
        forget_finished_on_join(&mut matches, "/os: update .10.0 s8r16 K?", ".10.0");
        assert!(matches.contains_key(".10.0"), "an update is the same game");
        forget_finished_on_join(&mut matches, "/os: join .10.0 s8r16 K?", ".10.0");
        assert!(
            !matches.contains_key(".10.0"),
            "a join after the end is a new game"
        );

        let mut live = MatchState::new();
        live.in_extension = true;
        matches.insert(".10.1".to_string(), live);
        forget_finished_on_join(&mut matches, "/os: join .10.1 s8r16 K?", ".10.1");
        assert!(
            matches.get(".10.1").is_some_and(|m| m.in_extension),
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
        let ggf = m.ggf(".1", None, false);
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
        let ggf = m.ggf(".1", None, false);
        let bo = ggf.split_once("BO[8 ").expect("BO tag present").1;
        let discs = bo.chars().take(64).filter(|c| *c != '-').count();
        assert_eq!(discs, 4, "a mid-game board became the start position: {bo}");
    }

    #[test]
    fn a_small_jump_is_still_the_extension() {
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
        assert!(!m.in_extension, "still in main time");
        apply_block(&mut m, &with_clock("01:30,0:0"), "kuroobi");
        assert!(m.in_extension, "missed the 40-second jump");
    }

    #[test]
    fn an_increment_is_not_the_extension() {
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
        assert!(!m.in_extension, "mistook an increment for the extension");
    }

    #[test]
    fn a_zero_before_any_move_is_not_the_extension() {
        let mut m = MatchState::new();
        let b = vec![
            "|kuroobi  (1720.0 *) 00:00,0:0//02:00,0:0".to_string(),
            "|* to move".to_string(),
        ];
        apply_block(&mut m, &b, "kuroobi");
        assert!(!m.in_extension, "set before any move was played");
    }

    #[test]
    fn a_falling_clock_is_not_the_extension() {
        let mut m = MatchState::new();
        for secs in ["15:00,0:0", "14:31,0:0", "13:02,0:0", "00:41,0:0"] {
            let b = vec![
                format!("|kuroobi  (1720.0 *) {secs}//02:00,0:0"),
                "|* to move".to_string(),
            ];
            apply_block(&mut m, &b, "kuroobi");
            assert!(!m.in_extension, "{secs} was treated as the extension");
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

        let ggf = m.ggf(".45.0", Some("+4.00"), false);
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
