//! GGS session thread (Generic Game Server, skatgame.net:5000).
//!
//! One thread owns the TCP connection, protocol parsing, engine
//! thinking and standby mode, streaming state snapshots to the frontend
//! via the "ggs" Tauri event. Derived from the CLI client
//! (src/bin/ggs.rs) as a resident UI: never leaves voluntarily during a
//! match, reconnects on disconnect. Adjourned games are NOT auto-resumed
//! — GGS freezes their clocks, so auto-resume would indulge opponents
//! who disconnect to analyze; they are listed and a human decides.

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

// ============================ External commands ============================

pub enum Cmd {
    Connect {
        login: String,
        pw: String,
    },
    Disconnect,
    /// Close a finished game from the list (never an ongoing one).
    CloseMatch(String),
    Raw(String),
    Ask {
        gtype: String,
        time: String,
        opponent: String,
        /// Whether the game is rated. On `/os` this is per-account
        /// state, so `/os rated +|-` is sent right before every offer;
        /// otherwise the previous offer's setting lingers.
        rated: bool,
    },
    Accept(String),
    Decline(String),
    Finger(String),
    /// Who list. Always fetches both pools (8 and random 8r) since
    /// both ratings are displayed.
    Who,
    Top {
        gtype: String,
        n: u32,
    },
    /// Fetch rank/rating in a given pool.
    Rank {
        gtype: String,
        name: String,
    },
    Watch(String),
    /// Fetch a finished game's record (GGF).
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
        /// Whether to ponder during the opponent's turn.
        ponder: bool,
    },
    /// The global thread count changed. There is no GGS-specific
    /// setting — separate ones would need two calibrations, with one
    /// silently missing at game time.
    ReloadThreads,
    /// Time-usage settings (pacing, per-move cap, reserve).
    SetPacing {
        pace: String,
        max_move_secs: u64,
        reserve_secs: u64,
        /// Clock aggressiveness (`timectl::Situation::budget_use`).
        budget_use: f64,
    },
    SetAutoPlay(bool),
    SetWatchAnalysis(bool),
    /// Whether to use the book (off for study/verification).
    SetUseBook(bool),
    /// Whether finished games feed book learning.
    SetLearn(bool),
    /// Match commands: undo / abort / resign / in-match tell. Named
    /// after the GGS verbs, so the enum-name overlap is accepted.
    #[allow(clippy::enum_variant_names)]
    MatchCmd {
        id: String,
        verb: String,
        arg: String,
    },
    /// Set server-side auto-accept (aform) / auto-decline (dform) formulas.
    SetFormula {
        kind: String,
        expr: String,
    },
    /// Refresh the adjourned-games list.
    ListStored,
    /// Refresh all ongoing games (the lobby's in-progress list).
    ListMatches,
    /// Resume an adjourned game (ask <.stored>).
    ResumeStored(String),
    /// Game history (ours or an opponent's).
    History(String),
    /// Advance the chat read marker (read up to this time). Persisted,
    /// so restarts do not resurrect unread counts.
    ChatSeen(u64),
    SetStandby(StandbyCfg),
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct StandbyCfg {
    pub enabled: bool,
    pub auto_accept: bool,
    /// Whether standby-mode offers are rated.
    pub rated: bool,
    pub opponent: String,
    pub gtype: String,
    pub time: String,
    pub max_games: usize,
    pub interval_secs: u64,
}

// ============================ Snapshot ============================

#[derive(Clone, Serialize, Default)]
pub struct LogLine {
    pub dir: String, // "in" | "out" | "info"
    pub text: String,
}

/// One ranking row (rank / top output).
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

/// Parsed finger output (heading/value pairs).
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
    /// Rating deviation; `/os top` provides it, `/os who` does not.
    pub dev: Option<f32>,
    /// Random-opening (8r) rating and deviation; only the who list has
    /// it (two pool fetches merged by name). Rankings are per-pool, so
    /// it stays empty there.
    pub rating_r: Option<f32>,
    pub dev_r: Option<f32>,
    /// Accepting flag. `/os who` marks each name: `+` = can accept
    /// (`open > games in progress`), `-` = not accepting, `x` = ghost
    /// (dead connection). Playing and accepting are not exclusive.
    /// `/os top` has no flag, hence `None`.
    pub open: Option<char>,
    pub raw: String,
}

/// Commands sent without `tell`. The server answers "name: value",
/// indistinguishable from a direct tell — the sender keeps count.
const BARE_CMDS: [&str; 2] = ["verbose", "chann"];

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct ChatMsg {
    /// Channel name (".chat" etc.); empty for directs.
    pub chan: String,
    pub from: String,
    pub text: String,
    /// Receive time (unix seconds); needed to order conversations.
    #[serde(default)]
    pub at: u64,
    /// Conversation key: the channel name, or the peer for directs.
    #[serde(default)]
    pub thread: String,
}

/// Forbid rated play. With `KUROOBI_NO_RATED=1`, offers and standby
/// both force `rated -` and the UI disables the toggle. This guards
/// automation, not fat fingers: a scripted screenshot run with standby
/// defaulting to rated could start a real rated game, so the send path
/// enforces it too.
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
    /// Player names (both).
    pub names: Vec<String>,
    /// Player ratings (same order as names).
    pub ratings: Vec<String>,
    pub gtype: String,
    /// Whether it is our game.
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

/// One point of the reported-eval series.
#[derive(Clone, Serialize, Default)]
pub struct EvalPoint {
    /// Move number (GGS numbering; passes count).
    pub n: u32,
    /// Whether we played the move.
    pub mine: bool,
    /// Reported value (mover-view discs).
    pub eval: f32,
}

#[derive(Clone, Serialize, Default)]
pub struct MatchView {
    pub id: String,
    pub base: String,
    /// Whether it ended (finished/adjourned/aborted); stays listed.
    pub over: bool,
    /// How it ended: empty = ongoing, `finished`, `adjourned`,
    /// `aborted`. Adjournment must not display as finished — no disc
    /// difference exists and the clock display would lie with it.
    pub ended: String,
    /// Who left, for adjournments.
    pub left_by: String,
    /// Final result (disc-difference string).
    pub result: String,
    /// Archive id (after the game only). With it the record can be
    /// re-fetched from the server — more reliable than local state, and
    /// synchro fetches include both boards and evals.
    pub archive: String,
    /// Current activity on this board: "" / think / ponder / solve /
    /// select; drives the status labels.
    pub busy: String,
    /// Progress depth (0 during solves).
    pub busy_depth: u32,
    /// Current best move (predicted reply while pondering);
    /// file-major 0..64.
    pub busy_best: Option<u32>,
    /// Its value in discs, mover view.
    pub busy_eval: Option<f32>,
    pub cells: Vec<u8>, // 0 empty, 1 black (*), 2 white (O)
    pub turn: String,   // "black" | "white" | ""
    pub my_color: String,
    pub opp_name: String,
    pub opp_rating: String,
    pub opp_clock: String,
    pub my_clock: String,
    /// Main-time seconds remaining (when parseable).
    pub my_secs: Option<u64>,
    pub opp_secs: Option<u64>,
    /// Overtime (grace) seconds; the third field of GGS "main/inc/ext".
    pub my_ext: Option<u64>,
    pub opp_ext: Option<u64>,
    /// Whether we entered overtime. The game is then already lost;
    /// only the wipeout remains avoidable. Never cleared once set
    /// (the server does not clear it either).
    pub in_overtime: bool,
    /// For watching: both players (not necessarily black-first).
    pub players: Vec<PlayerView>,
    /// Game type ("s8r14" etc.).
    pub gtype: String,
    pub moves: Vec<String>,
    /// GGF record; includes the start position, so drawn-opening games
    /// replay from it alone.
    pub ggf: String,
    pub last_eval: Option<f32>,
    pub last_eval_exact: bool,
    /// Opponent's reported value for their last move. GGS move rows
    /// arrive as `3: C2/20.00/122.16` (move/eval/time); `None` for
    /// opponents that do not report. Sign stays opponent-view.
    pub opp_eval: Option<f32>,
    /// Seconds the opponent reported spending on the move.
    pub opp_secs_used: Option<f32>,
    /// Both players' reported evals over the game, in move order,
    /// values as reported (mover view); `mine` marks whose move.
    pub eval_series: Vec<EvalPoint>,
    /// Whether our last move came from the book.
    pub last_from_book: bool,
    /// Watch-analysis result (Black-view value and best move).
    pub watch_eval: Option<f32>,
    pub watch_best: Option<String>,
    pub watch_exact: bool,
    pub seen: u64,
    /// Listing order (larger = newer); used only for sorting.
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
    /// GGF record with start position.
    #[serde(default)]
    pub ggf: String,
    /// GGS archive id; `look` can re-fetch the record later.
    #[serde(default)]
    pub archive: String,
    pub seq: u64,
    /// Our rating after the game (backfilled from who when available).
    #[serde(default)]
    pub my_rating: Option<f32>,
    /// Unix seconds.
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
    /// Latest own rating. Not displayed (per-pool `my_ranks` is) but
    /// kept for backfilling finished games.
    #[serde(skip)]
    pub my_rating: Option<f32>,
    /// Our per-pool ratings.
    pub my_ranks: Vec<RankRow>,
    pub log: VecDeque<LogLine>,
    pub users: Vec<UserRow>,
    pub ranking: Vec<UserRow>,
    pub fingers: HashMap<String, FingerInfo>,
    pub offers: Vec<Offer>,
    pub matches: Vec<MatchView>,
    pub ongoing: Vec<OngoingView>,
    /// One-line notice (watch failed etc.); short and shown-once.
    pub notice: String,
    pub stored: Vec<StoredView>,
    /// History results (name -> rows).
    pub history: HashMap<String, Vec<HistoryRow>>,
    pub chat: VecDeque<ChatMsg>,
    /// Read-up-to marker (unix seconds); newer messages count as
    /// unread. A count-based marker would drift when history is trimmed.
    pub chat_seen: u64,
    pub results: Vec<GameResult>,
    pub standby: StandbyCfg,
    pub standby_stats: StandbyStats,
    pub engine: EngineCfgView,
    pub auto_play: bool,
    /// Whether watched games are analyzed in the background.
    pub watch_analysis: bool,
    pub thinking: Option<String>,
    /// Last fetched record (GGF); cleared once the screen takes it.
    pub fetched_ggf: Option<FetchedGgf>,
}

/// Fixture snapshot for screenshots (`KUROOBI_GGS_DEMO=1`).
///
/// GGS screens show nothing without a connection, and the lobby/player/
/// results screens need opponents — so their look was unverifiable.
/// This renders them from fixtures; actions are inert.
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
        ("edax-bot", 2280.1, 22.0),
        ("nara", 1688.2, 91.0),
        ("kei", 1488.9, 216.0),
        ("newbie", 1200.0, 350.0),
    ]
    .into_iter()
    .map(|(n, r, d)| UserRow {
        name: n.into(),
        rating: Some(r),
        dev: Some(d),
        // Fixtures cover both pools (needed to check the layout).
        rating_r: Some(r - 30.0),
        dev_r: Some(d + 8.0),
        // Mix accepting and not-accepting (to check they read apart).
        open: Some(if r > 1700.0 { '+' } else { '-' }),
        raw: format!("{n} {r}@{d}"),
    })
    .collect();
    /* Pagination needs > 25 players; the live server rarely has 10,
    so it was never renderable without fixtures. */
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
            raw: "tamaki vs edax-bot".into(),
            watching: true,
            names: vec!["tamaki".into(), "edax-bot".into()],
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
    /* ---- Synchro's two boards; our color flips per board, so the
    headers are what tells them apart ---- */
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
    /* Vary the rating per game — identical values flatten the trend
    line and hide whether the graph works. Include the game type
    (empty shows "?"). */
    s.results = vec![
        (".70", "saio", 6i32, 1_754_003_000u64, 1842.3, "s8r16"),
        (".69", "tamaki", -4, 1_753_990_000, 1836.1, "8"),
        (".68", "nobu", 12, 1_753_900_000, 1840.0, "s8r16"),
        (".67", "kei", 18, 1_753_820_000, 1828.4, "8r16"),
        (".66", "edax-bot", -18, 1_753_740_000, 1812.9, "s8"),
        (".65", "nara", 0, 1_753_650_000, 1825.5, "8"),
    ]
    .into_iter()
    .map(|(id, opp, d, at, rate, gt)| GameResult {
        id: id.into(),
        /* The type is parsed from the head of `base`; a bare id
        renders as "?". */
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
    /* Finger fields; the formula tree (FormulaView) cannot render
    without them, and player cards read from the same data. */
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

/// Record fetched from a finished game.
#[derive(Clone, Serialize)]
pub struct FetchedGgf {
    pub id: String,
    /// First game (board 1 for synchro); kept for older callers.
    pub ggf: String,
    /// All returned games: a synchro archive holds two boards
    /// (`|2 (;..;)(;..;)`); taking only the first loses one.
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
    /// Whether the book is consulted.
    pub use_book: bool,
    /// Whether the book file loaded.
    pub book_loaded: bool,
    /// Whether finished games feed book learning.
    pub learn: bool,
    /// Whether to ponder on the opponent's turn. Ineffective under
    /// fixed depth (the real search runs to the end anyway).
    pub ponder: bool,
    /// Pacing ("fast" = save for the endgame / "depth" = fixed).
    /// "slow" and "even" were removed after measurement; old configs
    /// fall back to "fast" via `Pace::parse`.
    pub pace: String,
    /// Per-move cap (seconds); 0 = none.
    pub max_move_secs: u64,
    /// Seconds reserved for the solve.
    pub reserve_secs: u64,
    /// Clock aggressiveness; 1.0 = as allocated. Deepening uses ~47%
    /// of its deadline, so 1.0 spends half; the measured default 2.5
    /// raised utilization 45% -> 84%.
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

// ============================ Session ============================

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

/// Game-history path (ggs_games/history.jsonl in the repo).
fn history_path() -> PathBuf {
    for c in ["ggs_games", "../ggs_games", "../../ggs_games"] {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return p.join("history.jsonl");
        }
    }
    PathBuf::from("ggs_history.jsonl")
}

/// Load history at startup (up to 500, newest first).
fn load_history() -> Vec<GameResult> {
    let Ok(text) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    let mut out: Vec<GameResult> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<GameResult>(l).ok())
        .collect();
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

/// Chat path (`ggs_games/chat/<login>.jsonl`), split per login so
/// conversations from different accounts never mix. Names are
/// server-restricted to alphanumerics; separators are stripped anyway.
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

/// Retention count. The screen shows 300; this is how far back you
/// can dig — ~6 weeks at 100 messages/day.
const CHAT_KEEP: usize = 5000;

/// Load chat at startup (oldest first, last `CHAT_KEEP`). Unreadable
/// lines are dropped — losing everything over one torn line is worse.
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

/// GGS settings path (next to the history).
fn settings_path() -> PathBuf {
    history_path()
        .parent()
        .unwrap_or(&PathBuf::from("."))
        .join("ggs_settings.json")
}

/// Persist the screen-changeable settings. Absent, every restart reset
/// to defaults — a ponder-off verification game was actually played
/// ponder-on this way. A setting that vanishes on restart was never
/// really applied.
#[derive(Serialize, serde::Deserialize)]
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

/// Write the current settings; failure never interrupts play (it only
/// means defaults after the next restart).
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

/// Load at startup; absent file keeps the defaults.
fn load_settings() -> Option<SavedSettings> {
    let text = std::fs::read_to_string(settings_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Read-marker path (next to the chat log).
static MATCH_ORDER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn chat_seen_path(login: &str) -> PathBuf {
    chat_path(login).with_extension("seen")
}

/// Read the marker; a missing marker returns the current time, not 0
/// — first-time users must not face 5000 unread messages that predate
/// the mechanism.
fn load_chat_seen(login: &str) -> u64 {
    std::fs::read_to_string(chat_seen_path(login))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or_else(now_secs)
}

/// Write the marker, never backwards — a late write from a stale
/// screen must not resurrect read messages as unread.
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

/// Append one message; compact by rewriting once over the cap.
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
    /* Counting every append would re-read 5000 lines per message;
    compact only past a 20% margin. */
    if let Ok(meta) = std::fs::metadata(&path) {
        // ~120 bytes/line; compact past 1.2x the cap.
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

/// Resource config (weights/book locations); same file as local play.
fn resources() -> kuroobi::resources::Resources {
    crate::resources()
}

/// GGS rejects duplicate logins, so a second window's connection always
/// fails. A PID lock file prevents connecting while a live process
/// holds the session.
fn session_lock_path() -> PathBuf {
    /* Overridable via `KUROOBI_SESSION_LOCK`: the default lock admits
    one process, which also blocks two instances on different accounts
    (no duplicate-login risk there). Pair with
    `KUROOBI_KEYCHAIN_SERVICE`. */
    if let Ok(p) = std::env::var("KUROOBI_SESSION_LOCK") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::temp_dir().join("kuroobi_ggs.pid")
}

/// Whether another process holds the session (without taking the
/// lock); lets startup auto-login defer silently.
pub fn session_locked_by_other() -> bool {
    let Ok(s) = std::fs::read_to_string(session_lock_path()) else {
        return false;
    };
    match s.trim().parse::<i32>() {
        Ok(pid) => pid != std::process::id() as i32 && unsafe { libc::kill(pid, 0) } == 0,
        Err(_) => false,
    }
}

/// Take the lock; returns the PID if another process is connected.
fn try_lock_session() -> Result<(), i32> {
    let path = session_lock_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(pid) = s.trim().parse::<i32>() {
            // Liveness check via signal 0 (excluding ourselves).
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
    /// Drawn-opening start position (s8r14 etc.); captured from the
    /// first board on join, since moves alone cannot reconstruct the
    /// game. Empty for standard starts.
    start_cells: Vec<u8>,
    /// Mover at the start position ('*'/'O').
    start_turn: char,
    /// Game type ("s8r14" etc.); used to tell rating pools apart.
    gtype: String,
    turn: char, // '*' / 'O' / ' '
    my_color: Option<char>,
    /// Whether this turn was announced; updates arrive repeatedly per
    /// move and would chime every time.
    told_turn: bool,
    my_clock: String,
    my_clock_secs: Option<u64>,
    my_ext: Option<u64>,
    /// Whether we entered overtime (see `MatchView::in_overtime`).
    in_overtime: bool,
    /// How it ended (`MatchView::ended`).
    ended: String,
    /// Archive id (tail of the game-over line); only set after the game.
    archive: String,
    /// Who left, for adjournments.
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
    /// Evals and seconds carried by move rows (move number -> values);
    /// both players' moves included.
    move_evals: std::collections::BTreeMap<u32, (Option<f32>, Option<f32>)>,
    /// Opponent's latest reported value.
    opp_eval: Option<f32>,
    opp_secs_used: Option<f32>,
    /// Parity of our move numbers. Determined once from the turn and
    /// cached — after the game the turn is empty and the color could
    /// not be derived (the post-game eval chart used to vanish).
    eval_parity: Option<u32>,
    /// Watch analysis: mover-view value and best move (coordinate).
    watch_eval: Option<f32>,
    watch_best: Option<String>,
    watch_exact: bool,
    watch_hash: u64,
    last_played_hash: u64, // double-move protection
    seen: u64,
    /// Listing order. Ids cannot sort — GGS numbers are strings
    /// (`.23` < `.41` < `.8`) and get reused, which inserted new games
    /// mid-list.
    order: u64,
    /// Whether it ended; finished games stay listed (for record viewing
    /// and study hand-off) until closed manually.
    over: bool,
    /// Final result ("+12.00" etc.); display only.
    result: String,
}

impl MatchState {
    /// Build both players' reported-eval series.
    ///
    /// Whose move each one is follows from the turn alone: passes
    /// arrive numbered (`PA`), so number parity maps to color. After
    /// the game the turn is empty; the cached parity covers that.
    fn eval_series(&self) -> Vec<EvalPoint> {
        let Some(&last_n) = self.moves.keys().next_back() else {
            return Vec::new();
        };
        /* Use the cached parity; derive from the turn only when the
        cache is empty (the turn vanishes at game end). */
        let mine_parity = match self.eval_parity {
            Some(p) => p,
            None => {
                let Some(mc) = self.my_color else {
                    return Vec::new();
                };
                if self.turn != '*' && self.turn != 'O' {
                    return Vec::new();
                }
                // The previous move belongs to whoever is not to move.
                if self.turn != mc {
                    last_n % 2
                } else {
                    (last_n + 1) % 2
                }
            }
        };
        self.move_evals
            .iter()
            .filter_map(|(&n, &(ev, _))| {
                ev.map(|eval| EvalPoint {
                    n,
                    mine: (n % 2) == mine_parity,
                    eval,
                })
            })
            .collect()
    }

    /// Copy for history/learning; the original stays listed after the
    /// game, so consumers get a clone.
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
    /// Start position as a board string (empty for the standard start);
    /// same rank-major 64 cells + space + mover as kuroobi's Board.
    fn start_string(&self) -> String {
        if self.start_cells.len() != 64 {
            return String::new();
        }
        let mut out = String::with_capacity(66);
        for i in 0..64 {
            // Screen cells are file-major; board strings rank-major.
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

    /// Serialize to GGF: start in `BO[8 ...]`, moves in `B[]`/`W[]`.
    /// Round-trips drawn-opening games.
    fn ggf(&self, id: &str, result: Option<&str>) -> String {
        // BO uses '*' (black) and 'O' (white); default standard start.
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
        // Moves alternate from the start mover; passes advance color too.
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

/// Parse a GGS clock string into seconds. Observed format:
/// `15:00,0:0//02:00,0:0` = 15 min main / (no increment) / 2 min grace;
/// each `/`-separated field has a `,` subfield — read only the head.
///
/// The game-over line's disc difference is relative to the first-named
/// player, not our color:
///
///     - match .48 2639 kuroobi 2644 Rhapsody s8r16 R +2.00
///                    ~~~~~~~ +2 from this player's view
///
/// Deciding the sign from `my_color` once flipped a rated win into a
/// displayed loss (synchro boards carry opposite colors, so the answer
/// depended on which board arrived first).
fn my_stone_diff(score: f32, first_name: &str, login: &str) -> i32 {
    let v = score.round() as i32;
    if first_name == login {
        v
    } else {
        -v
    }
}

/// Borrow the opponent's already-played move from the synchro mirror.
///
/// `s8r16` plays the same opening color-swapped on two boards; while
/// the move sequences match, the positions are identical, so a move the
/// opponent played on one board is a candidate for us on the other.
/// Opponents demonstrably do this (measured: they wait for our move,
/// copy it, and diverge only on finding better). Because we move
/// faster, the mirror is ahead on only ~29% of our turns — still free
/// information. Never borrow after the sequences diverge (the borrowed
/// move may not even be legal).
fn mirror_hint(matches: &HashMap<String, MatchState>, mid: &str) -> Option<Position> {
    let (base, side) = mid.rsplit_once('.')?;
    let other = format!("{base}.{}", if side == "0" { "1" } else { "0" });
    let me = matches.get(mid)?;
    let you = matches.get(&other)?;
    // Next move number = our max + 1.
    let n = me.moves.keys().copied().max().unwrap_or(0) + 1;
    // The mirror holds only while the sequences match exactly.
    if (1..n).any(|i| me.moves.get(&i) != you.moves.get(&i)) {
        return None;
    }
    // Borrow if the other board already has that move number.
    let mv = you.moves.get(&n)?;
    let b = mv.as_bytes();
    if b.len() != 2 {
        return None;
    }
    let file = b[0].to_ascii_lowercase().wrapping_sub(b'a');
    let rank = b[1].wrapping_sub(b'1');
    Position::from_file_rank(file, rank)
}

/// Opponent's undo or abort request.
struct Request {
    verb: &'static str,
    id: String,
    who: String,
}

/// Parse `/os: undo .24 htz is asking`. Format captured from the live
/// server (docs only describe the sending side); our own requests echo
/// back in the same shape and are filtered by name.
fn parse_request(ln: &str, login: &str) -> Option<Request> {
    let rest = ln.strip_prefix("/os: ")?;
    let verb = ["undo", "abort"]
        .into_iter()
        .find(|v| rest.starts_with(&format!("{v} ")))?;
    let mut it = rest[verb.len() + 1..].split_whitespace();
    let id = it.next()?;
    let who = it.next()?;
    // Must end in "is asking"; anything else is an unknown notice.
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
        /* Negative clocks read as 0. `-0:01` can arrive right after
        exhaustion; failing to parse it falls to `None` = untimed = no
        deadline — the longest think at the worst moment. */
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

/// Learning evaluation depth: shallower than play (22) since every
/// legal alternative gets measured per position. One evaluation per
/// call keeps between-game responsiveness; local import uses the same.
pub(crate) const LEARN_DEPTH: u32 = 18;

fn base_id(id: &str) -> String {
    // ".82726.1" -> ".82726"; anything else unchanged.
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() >= 3 && parts.last().map(|s| s.len() == 1) == Some(true) {
        parts[..parts.len() - 1].join(".")
    } else {
        id.to_string()
    }
}

/// Remove every board belonging to a match id. Synchro splits one
/// match into `.N.0`/`.N.1` while start/end notify on the parent `.N`;
/// exact-match removal would miss those and leave stale boards.
fn drop_match(matches: &mut HashMap<String, MatchState>, id: &str) -> Vec<MatchState> {
    let keys: Vec<String> = matches
        .keys()
        .filter(|k| k.as_str() == id || base_id(k) == id)
        .cloned()
        .collect();
    keys.iter().filter_map(|k| matches.remove(k)).collect()
}

/// Mark games finished without removing them (records and study remain
/// reachable; closing is manual). Returns copies for history/learning.
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
            /* Keep the archive id: even if local state is unreadable,
            the record can be re-fetched (with both boards and evals). */
            if !archive.is_empty() {
                m.archive = archive.to_string();
            }
            if !result.is_empty() {
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

/// Move string sent to GGS, with eval and think time attached
/// (`tell /os play <id> D8/6/17.39` = move/eval/seconds). Nominally
/// optional, but omitting them triggers server trust violations (the
/// clock cannot be reconciled); added at the GGS admin's request.
/// Eval is mover-view discs; passes carry no eval.
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

/// Worker owning one game's search.
///
/// Synchro runs two games at once; with a single engine, one board
/// always waits — burning clock on our turns and ponder opportunity on
/// theirs. Engines are reused, never dropped: `Engine::new` leaks its
/// tables to 'static (167 MB each), so rebuilding per game accumulates
/// unfreed memory. Game end only clears the assignment.
struct EngineWorker {
    tx: std::sync::mpsc::Sender<Job>,
    rx: std::sync::mpsc::Receiver<Done>,
    stop: kuroobi::midgame::StopHandle,
    /// Whether a search is in flight (nothing new until it returns).
    busy: bool,
    /// Assigned match; empty = free.
    mid: Option<String>,
    /// Hash of the searched position (marks double-move protection on return).
    pending_hash: u64,
    /// A real search waiting to be dispatched; queued while busy,
    /// thrown the moment the worker frees.
    pending: Option<Pending>,
    /// Whether the running job is a ponder (discarded when a real
    /// search arrives).
    pondering: bool,
    /// When the real search was dispatched and its deadline; detects a
    /// search that never returns (stop and re-collect well past it).
    sent_at: Option<(Instant, Duration)>,
    /// When the stop was raised; decides when to abandon the worker.
    stopped_at: Option<Instant>,
    /// Progress peek: the engine writes at iteration boundaries, this
    /// side reads every loop, lock-free.
    progress: std::sync::Arc<kuroobi::engine::Progress>,
}

/// Give up on a worker this long after raising its stop. There are
/// only two seats; losing one cripples synchro, losing both halts the
/// game. Should never happen — but the cost if it does is too high.
const WORKER_GIVE_UP: Duration = Duration::from_secs(10);

/// A real search awaiting dispatch.
struct Pending {
    board: Board,
    levels: (u32, u8, u8),
    cap: Option<Duration>,
    hash: u64,
    /// Move borrowed from the mirror (`mirror_hint`); ordering only.
    hint: Option<Position>,
}

/// Worker instruction.
enum Job {
    /// Real search: read until the deadline, return a move.
    Think {
        board: Board,
        levels: (u32, u8, u8),
        cap: Option<Duration>,
        /// Mirror-borrowed move; ordering only, never a cutoff.
        hint: Option<Position>,
    },
    /// Ponder: costs none of our clock, so always safe when idle.
    /// Runs one `slice` at a time (re-dispatched in slices).
    Ponder {
        board: Board,
        slice: Duration,
    },
    SetUseBook(bool),
    SetThreads(usize),
}

/// Worker reply. Learning and watch analysis never ride workers —
/// both are idle-time jobs and stay on `Ctx.engine`.
enum Done {
    Moved(Box<kuroobi::engine::MoveEval>),
    Pondered,
}

struct Ctx {
    app: tauri::AppHandle,
    /// Engine stop handle (captured at engine creation).
    stop: Option<kuroobi::midgame::StopHandle>,
    snap: Arc<Mutex<Snapshot>>,
    engine: Option<Engine>,
    engine_cfg: EngineConfig,
    seq: u64,
    last_emit: Instant,
    dirty: bool,
    /// Verification auto-watch queue (KUROOBI_GGS_AUTOWATCH=auto).
    auto_watch: Vec<String>,
    /// Games awaiting learning import, advanced one search at a time
    /// between games; each carries its `LearnEntry` material (the match
    /// record is gone by the time the import finishes).
    learn_jobs: VecDeque<(String, kuroobi::learn::BackupJob, crate::LearnEntry)>,
    /// Local search stop handle (a starting GGS game halts local
    /// searches — clocked games come first).
    local_stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    /// Local activity record (notifies only while something runs).
    local_activity: Arc<Mutex<crate::Activity>>,
    /// Watch requests with deadlines; no board by the deadline means
    /// the game already ended (GGS may not report watch failures).
    pending_watch: HashMap<String, Instant>,
    /// Pacing and caps (screen-changeable).
    engine_cfg_pace: String,
    engine_cfg_max_move: u64,
    engine_cfg_reserve: u64,
    engine_cfg_budget_use: f64,
    /// Whether to ponder (screen-changeable). Only effective under a
    /// clock — fixed depth runs to the end regardless.
    engine_cfg_ponder: bool,
    /// Ponder target: the match whose turn is the opponent's, read in
    /// slices between receives; cleared when our turn returns. Unused
    /// when workers exist (each ponders its own game) — kept as the
    /// fallback when workers cannot be built.
    ponder_at: Option<String>,
    /// Per-game search workers (synchro runs two games; sharing one
    /// engine always starves a board). Pooled and never dropped — the
    /// leaked tables must be reused.
    workers: Vec<EngineWorker>,
    /// Threads per worker (set by `share_threads`); needed to look up
    /// the calibrated nps.
    worker_threads: usize,
}

/// Worker cap. Synchro needs 2; each costs 167 MB of tables. Games
/// beyond the cap share existing workers (they wait; nothing breaks).
const MAX_WORKERS: usize = 2;

impl Ctx {
    /// Worker index serving `mid`, creating one if needed: reuse a free
    /// worker, else build up to the cap, else steal the oldest
    /// assignment (sharing waits but never breaks).
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
                    // Redistribute threads; two engines run at once.
                    self.share_threads();
                    return Some(self.workers.len() - 1);
                }
                Err(e) => {
                    self.log("info", &format!("cannot spawn search worker: {e}"));
                    return None;
                }
            }
        }
        /* At the cap. Never repurpose a busy worker: `pending` holds
        one search, so overwriting kills another game's move — its clock
        then runs down unplayed (happened live: two dispatches, one
        return). Steal a worker still holding a finished game; else
        fall back to the shared synchronous path. */
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

    /// Split threads across workers: they run simultaneously, so the
    /// counts add. The calibrated nps must be looked up at the
    /// post-split count, hence it is recorded.
    fn share_threads(&mut self) {
        let total = resolve_threads(self.engine_cfg.threads);
        /* Divide by assigned workers, not `workers.len()`: workers
        survive their games, so after one synchro match a lone game
        would otherwise get half the threads forever. */
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

    /// Propagate the book setting to workers too — they do the actual
    /// game reading (`Ctx.engine` only analyzes/learns). The receiver
    /// side of `Job::SetUseBook` once existed with no sender, so the
    /// toggle never reached games (CI dead_code pointed at it).
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

    /// Release a worker's assignment at game end (keeping the engine),
    /// then redistribute threads so remaining games get everything.
    fn release_worker(&mut self, mid: &str) {
        let mut hit = false;
        for w in &mut self.workers {
            if w.mid.as_deref() == Some(mid) {
                w.mid = None;
                /* Discard the running search and the queue: playing
                into a finished match only yields `ERR match not found`
                while blocking another game's moves. */
                w.pending = None;
                if w.busy {
                    w.stop.stop();
                }
                /* Clear every per-game marker — workers are reused:
                - sent_at: the watchdog would treat our own deliberate
                  stop as an overdue search
                - stopped_at: a stale abandon-timer could scrap a
                  freshly reassigned worker (hundreds of ms + tables)
                - pending_hash: an accidental match with the next game's
                  first position would skip the dispatch */
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
    /// Spawn a thread owning one engine (weights load: hundreds of
    /// ms); called at game start, reused afterwards.
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
                        /* Notice leaked non-finites: `stone_scale`
                        silently rounds them to a plausible "+0.00" (a
                        live X-square blunder shipped that way). Non-zero
                        count = something in the search is broken. */
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

    /// Dispatch without waiting (waiting would defeat the design).
    fn send(&mut self, job: Job) {
        let counts = matches!(job, Job::Think { .. } | Job::Ponder { .. });
        if self.tx.send(job).is_ok() && counts {
            self.busy = true;
        }
    }
}

impl Ctx {
    /// macOS notification; only for events worth interrupting an
    /// unattended run.
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
        /* Diagnostics: the screen log truncates at 600 lines; keep the
        full wire when `KUROOBI_GGS_WIRE=<path>` is set. Needed in
        release too — the bugs it chases only appear live. */
        {
            use std::io::Write as _;
            let path = std::env::var("KUROOBI_GGS_WIRE")
                .ok()
                .or(if cfg!(debug_assertions) {
                    Some("/tmp/ggs_session_wire.log".to_string())
                } else {
                    None
                });
            if let Some(p) = path {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                {
                    let _ = writeln!(f, "{dir} {text}");
                }
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
        // Emit success is only visible in the diagnostics log.
        let _emit_ok = self.app.emit("ggs", &s).is_ok();
        // Diagnostics: state and emit results to a file (for bisecting
        // a stale UI; debug builds only).
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
            // 0 means auto; resolve to a real count just before building.
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

pub fn run(
    app: tauri::AppHandle,
    rx: Receiver<Cmd>,
    snap: Arc<Mutex<Snapshot>>,
    local_stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    local_activity: Arc<Mutex<crate::Activity>>,
) {
    let mut ctx = Ctx {
        app,
        stop: None,
        snap,
        engine: None,
        engine_cfg: EngineConfig {
            depth: 22,
            solve_empties: 26,
            band: 6,
            // Use the global setting (resources.conf threads); a GGS-only
            // one would need its own calibration.
            threads: resources().threads.unwrap_or_else(|| resolve_threads(0)),
            // Table sizes from the global config too; startup-only (the
            // leaked tables accumulate if rebuilt).
            midgame_hash_bits: resources().hash_mid_bits(),
            solver_hash_bits: resources().hash_end_bits(),
            ..Default::default()
        },
        seq: 0,
        last_emit: Instant::now(),
        dirty: true,
        auto_watch: Vec::new(),
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
        // Engines are not built until a game or watch analysis starts
        // (weights + tables are expensive). Book availability is
        // answered by file existence until then — otherwise the
        // settings screen would claim "missing" right after connect —
        // and ensure_engine overwrites it with the real load result.
        book_loaded: resources().book_path().exists(),
        learn: true,
        ponder: ctx.engine_cfg_ponder,
        pace: ctx.engine_cfg_pace.clone(),
        max_move_secs: ctx.engine_cfg_max_move,
        reserve_secs: ctx.engine_cfg_reserve,
    };
    /* Restore the saved settings; without this every restart silently
    reset them (a ponder-off run actually played ponder-on). */
    if let Some(v) = load_settings() {
        apply_engine_cfg(&mut ctx, v.depth, v.solve, v.band);
        ctx.engine_cfg_ponder = v.ponder;
        ctx.engine_cfg_pace = v.pace.clone();
        ctx.engine_cfg_max_move = v.max_move_secs;
        ctx.engine_cfg_reserve = v.reserve_secs;
        apply_pacing(
            &mut ctx,
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

    // Outer per-connection loop (command-wait while disconnected).
    'outer: loop {
        ctx.emit(true);
        // ---- Disconnected: wait for Connect ----
        let (login, pw) = loop {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(Cmd::Connect { login, pw }) => break (login, pw),
                Ok(Cmd::SetEngine {
                    depth,
                    solve,
                    band,
                    ponder,
                }) => {
                    apply_engine_cfg(&mut ctx, depth, solve, band);
                    ctx.engine_cfg_ponder = ponder;
                    ctx.snap.lock().unwrap().engine.ponder = ponder;
                    save_settings(&ctx);
                    ctx.emit(true);
                }
                Ok(Cmd::ReloadThreads) => {
                    apply_threads(&mut ctx);
                    ctx.emit(true);
                }
                Ok(Cmd::SetStandby(cfg)) => {
                    ctx.snap.lock().unwrap().standby = cfg;
                    ctx.emit(true);
                }
                Ok(Cmd::SetPacing {
                    pace,
                    max_move_secs,
                    reserve_secs,
                    budget_use,
                }) => {
                    apply_pacing(&mut ctx, pace, max_move_secs, reserve_secs, budget_use);
                    save_settings(&ctx);
                    ctx.emit(true);
                }
                Ok(Cmd::SetAutoPlay(b)) => {
                    ctx.snap.lock().unwrap().auto_play = b;
                    save_settings(&ctx);
                    ctx.emit(true);
                }
                Ok(Cmd::SetUseBook(b)) => {
                    ctx.set_use_book(b);
                    save_settings(&ctx);
                    ctx.emit(true);
                }
                Ok(Cmd::SetWatchAnalysis(b)) => {
                    ctx.snap.lock().unwrap().watch_analysis = b;
                    save_settings(&ctx);
                    ctx.emit(true);
                }
                Ok(Cmd::SetLearn(b)) => {
                    ctx.snap.lock().unwrap().engine.learn = b;
                    save_settings(&ctx);
                    ctx.emit(true);
                }
                Ok(Cmd::Rank { .. }) | Ok(Cmd::ListMatches) => {} // ignored while disconnected
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Disconnected idle time still advances learning.
                    learn_tick(&mut ctx);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        };

        // Never connect while another window is (duplicate logins are
        // always rejected).
        if let Err(pid) = try_lock_session() {
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
            continue 'outer;
        }

        let mut login_fails = 0u32; // drops before login completed
        let mut cred_saved = false; // whether credentials were saved to the keychain
                                    /* Restore past conversations (the
                                    screen keeps 300). Not on reconnect —
                                    that would double the messages. */
        {
            let mut s = ctx.snap.lock().unwrap();
            if s.chat.is_empty() {
                let mut past = load_chat(&login);
                if past.len() > 300 {
                    past.drain(..past.len() - 300);
                }
                s.chat.extend(past);
            }
            /* Restore the read marker too — otherwise the restored
            history all counted as unread (hundreds per launch). */
            s.chat_seen = load_chat_seen(&login);
        }
        // ---- Connected session (never abandon; reconnect on drop) ----
        'session: loop {
            {
                let mut s = ctx.snap.lock().unwrap();
                s.conn = "connecting".into();
                s.login = login.clone();
            }
            ctx.emit(true);
            let mut stream = match TcpStream::connect(("skatgame.net", 5000)) {
                Ok(s) => s,
                Err(e) => {
                    ctx.log("info", &format!("connect failed: {e} — retrying in 15s"));
                    ctx.emit(true);
                    std::thread::sleep(Duration::from_secs(15));
                    continue 'session;
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_millis(250)))
                .ok();
            let mut writer = stream.try_clone().expect("clone");

            ctx.snap.lock().unwrap().conn = "logging_in".into();
            ctx.emit(true);

            let mut raw = Vec::<u8>::new();
            let mut lines = VecDeque::<String>::new();
            let mut logged_in = false;
            // Surface a stalled login; the usual cause is a duplicate
            // login from another process.
            let login_started = Instant::now();
            let mut login_warned = false;
            let mut lost = false;
            let mut in_block = false;
            let mut block: Vec<String> = Vec::new();
            let mut matches: HashMap<String, MatchState> = HashMap::new();
            // Pending command kinds. GGS ACKs a tell with READY first
            // and sends the body (header + | lines + READY) later, so
            // collection starts at the header.
            let mut pending: Vec<String> = Vec::new();
            // When to re-poll ongoing games (the login-time poll is below).
            let mut next_match_at = Instant::now() + Duration::from_secs(60);
            let mut capture: Option<(String, Vec<String>)> = None; // (kind, lines)
            let mut next_ask_at: Option<Instant> = None;
            let mut want_quit = false;
            // Whether we had a game (previous value); halt local
            // searches the moment one starts.
            let mut had_own_match = false;

            macro_rules! send {
                ($ctx:expr, $cmd:expr) => {{
                    let c: String = $cmd.to_string();
                    $ctx.log("out", &c);
                    let _ = writer
                        .write_all(c.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"));
                }};
                // Send without logging (passwords etc.).
                ($ctx:expr, $cmd:expr, secret) => {{
                    let c: String = $cmd.to_string();
                    $ctx.log("out", "********");
                    let _ = writer
                        .write_all(c.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"));
                }};
            }

            loop {
                // ---------- UI commands ----------
                while let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        Cmd::Connect { .. } => {}
                        Cmd::Disconnect => {
                            if let Some(h) = &ctx.stop {
                                h.stop(); // abort any running think too
                            }
                            send!(ctx, "quit");
                            want_quit = true;
                        }
                        Cmd::Raw(c) => send!(ctx, c),
                        Cmd::ChatSeen(at) => {
                            save_chat_seen(&login, at);
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
                            // Rated-ness is per-account state; align it
                            // right before every offer.
                            send!(
                                ctx,
                                format!(
                                    "tell /os rated {}",
                                    if rated && !no_rated() { "+" } else { "-" }
                                )
                            );
                            // No opponent = an open offer; omit the arg
                            // to avoid a trailing space.
                            let cmd = if opponent.is_empty() {
                                format!("tell /os ask {gtype} {time}")
                            } else {
                                format!("tell /os ask {gtype} {time} {opponent}")
                            };
                            send!(ctx, cmd)
                        }
                        Cmd::Accept(id) => send!(ctx, format!("tell /os accept {id}")),
                        Cmd::Decline(id) => send!(ctx, format!("tell /os decline {id}")),
                        Cmd::Finger(name) => {
                            // Two fingers exist: server-wide (identity)
                            // and /os (game settings, accept formulas).
                            // Fetch both; the latter matters pre-offer.
                            pending.push(format!("finger:{name}"));
                            send!(ctx, format!("finger {name}"));
                            pending.push(format!("osfinger:{name}"));
                            send!(ctx, format!("tell /os finger {name}"));
                        }
                        /* The pool argument is ignored; both pools are
                        always fetched and merged. */
                        Cmd::Who => {
                            for t in ["8", "8r"] {
                                pending.push(format!("who:{t}"));
                                send!(ctx, format!("tell /os who {t}"));
                            }
                        }
                        Cmd::Top { gtype, n } => {
                            pending.push("top".into());
                            send!(ctx, format!("tell /os top {gtype} {n}"));
                        }
                        Cmd::Rank { gtype, name } => {
                            pending.push(format!("rank:{gtype}:{name}"));
                            send!(ctx, format!("tell /os rank {gtype} {name}"));
                        }
                        Cmd::Look(id) => {
                            pending.push(format!("look:{id}"));
                            send!(ctx, format!("tell /os look {id}"));
                        }
                        Cmd::CloseMatch(id) => {
                            // Close only finished games.
                            let keys: Vec<String> = matches
                                .iter()
                                .filter(|(k, m)| m.over && (k.as_str() == id || base_id(k) == id))
                                .map(|(k, _)| k.clone())
                                .collect();
                            for k in keys {
                                matches.remove(&k);
                                // Keep the worker, drop the assignment
                                // (rebuilding leaks its tables).
                                ctx.release_worker(&k);
                            }
                            sync_matches(&mut ctx, &matches);
                            ctx.emit(true);
                        }
                        Cmd::Watch(id) => {
                            send!(ctx, format!("tell /os watch + {id}"));
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
                            send!(ctx, format!("tell /os watch - {id}"));
                            let mut s = ctx.snap.lock().unwrap();
                            for o in s.ongoing.iter_mut() {
                                if o.id == id {
                                    o.watching = false;
                                }
                            }
                            s.matches.retain(|m| m.id != id && base_id(&m.id) != id);
                            drop(s);
                            drop_match(&mut matches, &id);
                            ctx.release_worker(&id);
                            ctx.dirty = true;
                        }
                        Cmd::Chat { target, text } => {
                            send!(ctx, format!("tell {target} {text}"));
                            // Show our own messages in the chat too.
                            let me = login.clone();
                            let is_chan = target.starts_with('.');
                            let msg = ChatMsg {
                                chan: if is_chan {
                                    target.clone()
                                } else {
                                    format!("→{target}")
                                },
                                from: me,
                                text,
                                at: now_secs(),
                                thread: target,
                            };
                            append_chat(&login, &msg);
                            let mut s = ctx.snap.lock().unwrap();
                            s.chat.push_back(msg);
                            while s.chat.len() > 300 {
                                s.chat.pop_front();
                            }
                            drop(s);
                            ctx.dirty = true;
                        }
                        Cmd::SetEngine {
                            depth,
                            solve,
                            band,
                            ponder,
                        } => {
                            apply_engine_cfg(&mut ctx, depth, solve, band);
                            ctx.engine_cfg_ponder = ponder;
                            ctx.snap.lock().unwrap().engine.ponder = ponder;
                            save_settings(&ctx);
                            ctx.dirty = true;
                        }
                        Cmd::ReloadThreads => {
                            apply_threads(&mut ctx);
                            ctx.dirty = true;
                        }
                        Cmd::SetPacing {
                            pace,
                            max_move_secs,
                            reserve_secs,
                            budget_use,
                        } => {
                            apply_pacing(&mut ctx, pace, max_move_secs, reserve_secs, budget_use);
                            save_settings(&ctx);
                        }
                        Cmd::SetAutoPlay(b) => {
                            ctx.snap.lock().unwrap().auto_play = b;
                            save_settings(&ctx);
                            ctx.dirty = true;
                        }
                        Cmd::SetUseBook(b) => {
                            ctx.set_use_book(b);
                            save_settings(&ctx);
                            ctx.dirty = true;
                        }
                        Cmd::SetWatchAnalysis(b) => {
                            ctx.snap.lock().unwrap().watch_analysis = b;
                            save_settings(&ctx);
                            ctx.dirty = true;
                        }
                        Cmd::SetLearn(b) => {
                            ctx.snap.lock().unwrap().engine.learn = b;
                            save_settings(&ctx);
                            ctx.dirty = true;
                        }
                        Cmd::ListStored => {
                            pending.push("stored_list".into());
                            send!(ctx, "tell /os stored");
                        }
                        Cmd::ListMatches => {
                            pending.push("match_list".into());
                            send!(ctx, "tell /os match");
                        }
                        Cmd::ResumeStored(id) => {
                            ctx.log("info", &format!("resuming adjourned game {id}"));
                            send!(ctx, format!("tell /os ask {id}"));
                        }
                        Cmd::History(name) => {
                            pending.push(format!("history:{name}"));
                            if name.is_empty() {
                                send!(ctx, "tell /os history");
                            } else {
                                send!(ctx, format!("tell /os history {name}"));
                            }
                        }
                        Cmd::SetFormula { kind, expr } => {
                            send!(ctx, format!("tell /os {kind} {expr}"));
                            ctx.log("info", &format!("set {kind}: {expr}"));
                        }
                        Cmd::MatchCmd { id, verb, arg } => {
                            if arg.is_empty() {
                                send!(ctx, format!("tell /os {verb} {id}"));
                            } else {
                                send!(ctx, format!("tell /os {verb} {id} {arg}"));
                            }
                        }
                        Cmd::SetStandby(cfg) => {
                            let mut s = ctx.snap.lock().unwrap();
                            let was = s.standby.enabled;
                            s.standby = cfg;
                            if !was && s.standby.enabled {
                                s.standby_stats = Default::default();
                            }
                            /* Pick up offers that already arrived:
                            auto-accept only fires on incoming `+ .N`, so
                            enabling standby after an offer would wait
                            forever (happened). */
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
                                send!(ctx, format!("tell /os accept {id}"));
                            }
                            next_ask_at = Some(Instant::now() + Duration::from_secs(3));
                            ctx.dirty = true;
                        }
                    }
                }
                if want_quit {
                    break 'session;
                }

                // ---------- Socket ----------
                let mut chunk = [0u8; 8192];
                match stream.read(&mut chunk) {
                    Ok(0) => lost = true,
                    Ok(n) => raw.extend_from_slice(&chunk[..n]),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => lost = true,
                }
                if lost {
                    if !logged_in {
                        // Dropped before login = auth failed; endless
                        // reconnects won't help. Report and go idle. The
                        // usual cause: a duplicate login.
                        login_fails += 1;
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
                        if login_fails >= 2 {
                            let mut s = ctx.snap.lock().unwrap();
                            s.conn = "disconnected".into();
                            drop(s);
                            ctx.emit(true);
                            break 'session; // back to idle (reconnect is a user action)
                        }
                        ctx.emit(true);
                        std::thread::sleep(Duration::from_secs(3));
                        continue 'session;
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
                    continue 'session;
                }
                while let Some(nl) = raw.iter().position(|&b| b == b'\n') {
                    let mut line: Vec<u8> = raw.drain(..=nl).collect();
                    while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
                        line.pop();
                    }
                    lines.push_back(String::from_utf8_lossy(&line).into_owned());
                }

                // ---------- Login ----------
                if !logged_in {
                    if !login_warned && login_started.elapsed() > Duration::from_secs(20) {
                        login_warned = true;
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
                    let tail = String::from_utf8_lossy(&raw).to_lowercase();
                    let tail2 = lines
                        .iter()
                        .rev()
                        .take(3)
                        .map(|l| l.to_lowercase())
                        .collect::<Vec<_>>()
                        .join(" ");
                    while let Some(l) = lines.pop_front() {
                        ctx.log("in", &l);
                    }
                    if tail.contains("enter login") || tail2.contains("enter login") {
                        send!(ctx, login);
                        raw.clear();
                    } else if tail.contains("password") || tail2.contains("password") {
                        send!(ctx, pw, secret);
                        raw.clear();
                        logged_in = true;
                        {
                            let mut s = ctx.snap.lock().unwrap();
                            s.conn = "online".into();
                        }
                        send!(ctx, "verbose -news -faq -help -ack");
                        send!(ctx, "tell /os client -");
                        send!(ctx, "tell /os trust +");
                        // Startup default; never raised when forbidden.
                        send!(
                            ctx,
                            if no_rated() {
                                "tell /os rated -"
                            } else {
                                "tell /os rated +"
                            }
                        );
                        /* Other players' game starts/ends arrive as
                        notifications — without subscribing they never
                        come, leaving the lobby's in-progress list frozen
                        at its login-time state. The receive side existed;
                        only the subscription was missing. */
                        send!(ctx, "tell /os notify +");
                        send!(ctx, "tell /os open 1");
                        send!(ctx, "chann + .chat");
                        /* Fetch both pools. The reply header names no
                        pool, so ordering in `pending` tells them apart
                        (as `rank` already does). */
                        for t in ["8", "8r"] {
                            pending.push(format!("who:{t}"));
                            send!(ctx, format!("tell /os who {t}"));
                        }
                        // GGS has exactly two pools: 8 and 8r. Synchro
                        // is a game format, not a pool.
                        for t in ["8", "8r"] {
                            pending.push(format!("rank:{t}:{login}"));
                            send!(ctx, format!("tell /os rank {t} {login}"));
                        }
                        pending.push("stored_list".into());
                        send!(ctx, "tell /os stored");
                        pending.push("match_list".into());
                        send!(ctx, "tell /os match");
                        pending.push("history:".into());
                        send!(ctx, "tell /os history");
                        /* Adjourned games are listed, never auto-resumed.
                        GGS saves them with clocks frozen and no expiry,
                        so an opponent can disconnect when losing,
                        analyze at leisure, and resume; auto-resume would
                        play along. A human decides, from the list. (A
                        silent departure always adjourns: is_aborted()
                        requires both sides' consent.) */
                        pending.push("stored_list".into());
                        send!(ctx, "tell /os stored");
                        ctx.emit(true);
                    }
                    ctx.emit(false);
                    continue;
                }

                // ---------- Line handling ----------
                while let Some(ln) = lines.pop_front() {
                    ctx.log("in", &ln);

                    // An /os reply after login = auth succeeded; save
                    // credentials now (a bad password disconnects before
                    // any reply). Enables next launch's auto-login.
                    if !cred_saved && (ln == "READY" || ln.starts_with("/os")) {
                        cred_saved = true;
                        crate::keychain::save(&login, &pw);
                    }

                    // Channel chat: ".chat name: text"
                    if let Some(rest) = ln.strip_prefix('.') {
                        if let Some((head, text)) = rest.split_once(": ") {
                            let mut it = head.split_whitespace();
                            if let (Some(chan), Some(from), None) =
                                (it.next(), it.next(), it.next())
                            {
                                let msg = ChatMsg {
                                    chan: format!(".{chan}"),
                                    from: from.to_string(),
                                    text: text.to_string(),
                                    at: now_secs(),
                                    thread: format!(".{chan}"),
                                };
                                append_chat(&login, &msg);
                                let mut s = ctx.snap.lock().unwrap();
                                s.chat.push_back(msg);
                                while s.chat.len() > 300 {
                                    s.chat.pop_front();
                                }
                                drop(s);
                                ctx.dirty = true;
                                continue;
                            }
                        }
                    }
                    // Direct tell: "name: text" (only for plain
                    // alphanumeric names, to exclude server lines).
                    if !ln.starts_with(['/', '|', ':', ' ']) && ln != "READY" {
                        if let Some((name, text)) = ln.split_once(": ") {
                            if !name.is_empty()
                                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                                && name.chars().next().unwrap().is_ascii_alphabetic()
                                // Server replies are not chat: bare
                                // commands answer as "name: value", which
                                // looks like a direct tell — "verbose"
                                // once appeared as a chat partner. Only
                                // words we send bare are filtered.
                                && !BARE_CMDS.contains(&name)
                            {
                                let msg = ChatMsg {
                                    chan: String::new(),
                                    from: name.to_string(),
                                    text: text.to_string(),
                                    at: now_secs(),
                                    thread: name.to_string(),
                                };
                                append_chat(&login, &msg);
                                let mut s = ctx.snap.lock().unwrap();
                                s.chat.push_back(msg);
                                while s.chat.len() > 300 {
                                    s.chat.pop_front();
                                }
                                drop(s);
                                ctx.dirty = true;
                                continue;
                            }
                        }
                    }
                    // Block (board) collection.
                    if ln.starts_with("/os: update") || ln.starts_with("/os: join") {
                        in_block = true;
                        block.clear();
                        block.push(ln);
                        continue;
                    }
                    if in_block {
                        if ln == "READY" {
                            in_block = false;
                            // A board arrived = the watch succeeded.
                            if let Some(mid) = block
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
                            handle_block(&mut ctx, &block, &login, &mut matches, |c| {
                                let c: String = c;
                                ctx_send(&mut writer, &c);
                            });
                        } else {
                            block.push(ln);
                        }
                        continue;
                    }

                    // Drain the auto-watch queue (verification).
                    if !ctx.auto_watch.is_empty() {
                        for id in std::mem::take(&mut ctx.auto_watch) {
                            send!(ctx, format!("tell /os watch + {id}"));
                        }
                    }

                    // capture (who / top / finger / stored):
                    // Collect from the header line; the next READY seals it.
                    if let Some((kind, buf)) = capture.as_mut() {
                        if ln == "READY" {
                            let kind = kind.clone();
                            let buf = buf.clone();
                            capture = None;
                            finish_capture(&mut ctx, &kind, &buf, &login);
                        } else {
                            buf.push(ln.clone());
                        }
                        // Common handling continues during capture
                        // (offers etc. still need picking up).
                    } else if let Some(pos) =
                        pending.iter().position(|k| capture_header_matches(k, &ln))
                    {
                        let kind = pending.remove(pos);
                        capture = Some((kind, vec![ln.clone()]));
                    }

                    // Offer / match additions and removals.
                    if let Some(rest) = ln.strip_prefix("/os: + ") {
                        let rest = rest.trim_start();
                        if let Some(mrest) = rest.strip_prefix("match ") {
                            let id = mrest.split_whitespace().next().unwrap_or("").to_string();
                            if !id.is_empty() {
                                let mine = mrest.contains(&login);
                                if mine {
                                    ctx.notify(
                                        &crate::i18n::t("backend.notify.game_start_title"),
                                        mrest,
                                    );
                                }
                                let mut s = ctx.snap.lock().unwrap();
                                s.ongoing.retain(|o| o.id != id);
                                if !mine {
                                    let t: Vec<&str> = mrest.split_whitespace().collect();
                                    let names: Vec<String> = t
                                        .iter()
                                        .filter(|x| {
                                            x.len() >= 2
                                                && x.chars().next().map(|c| c.is_ascii_alphabetic())
                                                    == Some(true)
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
                            add_offer(&mut ctx, rest, &login);
                            // Standby: auto-accept offers addressed to us.
                            let s = ctx.snap.lock().unwrap();
                            let auto = s.standby.enabled && s.standby.auto_accept;
                            // Don't count finished games (they stay
                            // listed); counting them would stop standby
                            // after the first game forever.
                            let in_match = s.matches.iter().any(|m| !m.over);
                            let incoming = s.offers.last().map(|o| (o.incoming, o.id.clone()));
                            drop(s);
                            if let Some((true, id)) = incoming {
                                if auto && !in_match {
                                    ctx.log("info", &format!("waiting mode: auto-accepting {id}"));
                                    send!(ctx, format!("tell /os accept {id}"));
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
                    } else if let Some(rest) = ln.strip_prefix("/os: - ") {
                        let rest = rest.trim_start();
                        if let Some(mrest) = rest.strip_prefix("match ") {
                            let was_mine = mrest.contains(&login);
                            {
                                // Remove from ongoing/watch lists (others' too).
                                let id = mrest.split_whitespace().next().unwrap_or("").to_string();
                                let mut s = ctx.snap.lock().unwrap();
                                s.ongoing.retain(|o| o.id != id);
                                drop(s);
                                if !was_mine {
                                    // Watched games stay too (records
                                    // remain fetchable); closing is manual.
                                    let (kind, who) = end_kind(mrest);
                                    finish_match(&mut matches, &id, "", kind, &who, "");
                                }
                            }
                            // Tell the screen the board is gone, or the
                            // finished game lingers until the next event.
                            sync_matches(&mut ctx, &matches);
                            ctx.emit(true);
                            handle_match_end(&mut ctx, mrest, &login, &mut matches);
                            if was_mine {
                                // Ratings moved; refresh who.
                                for t in ["8", "8r"] {
                                    pending.push(format!("who:{t}"));
                                    send!(ctx, format!("tell /os who {t}"));
                                }
                            }
                            // Standby: schedule the next game.
                            let s = ctx.snap.lock().unwrap();
                            let sb = s.standby.clone();
                            let games = s.standby_stats.games;
                            drop(s);
                            if sb.enabled
                                && (sb.max_games == 0 || games < sb.max_games)
                                && !sb.opponent.is_empty()
                            {
                                next_ask_at = Some(
                                    Instant::now() + Duration::from_secs(sb.interval_secs.max(5)),
                                );
                            }
                        } else if rest.starts_with('.') {
                            let id = rest.split_whitespace().next().unwrap_or("").to_string();
                            let mut s = ctx.snap.lock().unwrap();
                            s.offers.retain(|o| o.id != id);
                            drop(s);
                            ctx.dirty = true;
                        }
                    } else if let Some(req) = parse_request(&ln, &login) {
                        /* ---------- Opponent undo / abort ----------

                        Observed format (not guessed):

                            /os: undo  .24 htz is asking
                            /os: abort .24 htz is asking

                        These were silently ignored — the opponent waits
                        while nothing shows here. They can hurt us
                        (rolling back a winning position), so never
                        auto-accept; unattended standby declines (leaving
                        them hanging is worse), attended play notifies
                        and lets the human decide. */
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
                            /* Decline by player name: the notice carries
                            the match id, but `decline` accepts a request
                            id or a name — a match id errors (observed),
                            and the request id is not in the notice. */
                            ctx.log(
                                "info",
                                &format!("waiting mode: unattended, declining ({})", req.who),
                            );
                            send!(ctx, format!("tell /os decline {}", req.who));
                        }
                    } else if ln.starts_with("/os: ERR") {
                        ctx.log("info", &format!("server error: {ln}"));
                        /* Errors that merely raced the game end are
                        dropped: synchro boards end separately, so a move
                        can arrive after its game closed. Unavoidable and
                        harmless — no toast. */
                        if ln.contains("not found") && ln.contains("match") {
                            continue;
                        }
                        // The wire log may not be open; a refused action
                        // must be reported on the spot.
                        let msg = ln.trim_start_matches("/os: ERR").trim();
                        ctx.snap.lock().unwrap().notice = if msg.is_empty() {
                            "err.ggs_action_refused".into()
                        } else {
                            format!("GGS: {msg}")
                        };
                        ctx.dirty = true;
                    }
                }

                // ---------- Standby auto-offers ----------
                if let Some(t) = next_ask_at {
                    if Instant::now() >= t {
                        next_ask_at = None;
                        let s = ctx.snap.lock().unwrap();
                        let sb = s.standby.clone();
                        // Finished games don't count as playing (as above).
                        let in_match = s.matches.iter().any(|m| !m.over);
                        let games = s.standby_stats.games;
                        /* Is one of our offers still outstanding?
                        Re-offering would double it and could start two
                        games at once. */
                        let outgoing = s.offers.iter().any(|o| !o.incoming);
                        drop(s);
                        let more = sb.max_games == 0 || games < sb.max_games;
                        /* Re-arm on a dud: offers were only scheduled at
                        game end, so an unanswered offer silently halted
                        standby — exactly the unattended case it exists
                        for. Re-arming is harmless once a game starts
                        (in_match gates it). */
                        if sb.enabled && !sb.opponent.is_empty() && more {
                            next_ask_at = Some(
                                Instant::now() + Duration::from_secs(sb.interval_secs.max(30)),
                            );
                        }
                        if sb.enabled && !in_match && !outgoing && !sb.opponent.is_empty() && more {
                            ctx.log("info", &format!("waiting mode: asking {}", sb.opponent));
                            // Align rated-ness right before the offer (as Cmd::Ask).
                            send!(
                                ctx,
                                format!(
                                    "tell /os rated {}",
                                    if sb.rated && !no_rated() { "+" } else { "-" }
                                )
                            );
                            send!(
                                ctx,
                                format!("tell /os ask {} {} {}", sb.gtype, sb.time, sb.opponent)
                            );
                        }
                    }
                }

                // ---------- Re-polling ongoing games ----------
                /* Never rely on notifications alone: one missed event
                would leave the list stale forever, so re-poll every 60s
                (`tell /os match` is read-only). */
                if Instant::now() >= next_match_at {
                    next_match_at = Instant::now() + Duration::from_secs(60);
                    pending.push("match_list".into());
                    send!(ctx, "tell /os match");
                }

                /* ---------- Freeing finished games' workers ----------

                Finished games stay listed, so release_worker only fires
                on close; a still-held worker would make the next synchro
                pair fight over one worker and drop moves via `pending`
                overwrites. */
                {
                    let done: Vec<String> = ctx
                        .workers
                        .iter()
                        .filter_map(|w| w.mid.clone())
                        .filter(|mid| matches.get(mid).is_none_or(|m| m.over))
                        .collect();
                    for mid in done {
                        ctx.release_worker(&mid);
                    }
                }

                /* ---------- Missed-move rescue ----------

                think_and_play only fires on updates; a missed dispatch
                leaves the game unmoved until the next update — forever,
                if the opponent is also waiting — while the clock drains
                (happened live). So every loop re-scans for "our turn,
                not yet playing" games; double-move protection makes it
                idempotent. */
                {
                    let stalled: Vec<String> = matches
                        .iter()
                        .filter(|(_, m)| {
                            !m.over && m.my_color.is_some() && Some(m.turn) == m.my_color
                        })
                        .map(|(k, _)| k.clone())
                        .collect();
                    if !stalled.is_empty() {
                        let mut out: Vec<String> = Vec::new();
                        for mid in stalled {
                            think_and_play(&mut ctx, &mid, &mut matches, |s| out.push(s));
                        }
                        for line in out {
                            send!(ctx, line);
                        }
                    }
                }

                // ---------- Collecting worker results ----------
                /* Collected every loop — dispatches are fire-and-forget,
                so no collection means no move sent. Returns data because
                `send!` borrows `writer` directly. */
                for line in collect_workers(&mut ctx, &mut matches) {
                    send!(ctx, line);
                }
                sync_matches(&mut ctx, &matches);

                // ---------- Pondering (fallback without workers) ----------
                if ctx.workers.is_empty() {
                    ponder_slice(&mut ctx, &matches);
                }

                // ---------- Watch-failure detection ----------
                // GGS may not error a watch on a finished game; no board
                // by the deadline means the game is gone.
                let now = Instant::now();
                let stale: Vec<String> = ctx
                    .pending_watch
                    .iter()
                    .filter(|(_, &due)| now >= due)
                    .map(|(id, _)| id.clone())
                    .collect();
                if !stale.is_empty() {
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

                // ---------- Yielding local CPU to our GGS game ----------
                // A clocked real game outranks everything: stop running
                // local searches and notify; new ones are refused in
                // main.rs.
                let own_match = matches.values().any(|m| m.my_color.is_some());
                if own_match && !had_own_match {
                    let local_busy = ctx.local_activity.lock().unwrap().local.is_some();
                    if local_busy {
                        if let Some(h) = ctx.local_stop.lock().unwrap().as_ref() {
                            h.stop();
                        }
                        ctx.notify(
                            &crate::i18n::t("backend.notify.game_start_title"),
                            &crate::i18n::t("backend.notify.local_search_stopped_body"),
                        );
                        ctx.log("info", "GGS game started — stopped the local search");
                    }
                }
                had_own_match = own_match;

                // ---------- Game import (learning) ----------
                // Advances one search at a time while we have no game;
                // each block is at most one evaluation.
                if !own_match {
                    learn_tick(&mut ctx);
                }

                ctx.emit(false);
            }
        }

        // Explicit disconnect.
        unlock_session();
        {
            let mut s = ctx.snap.lock().unwrap();
            s.conn = "disconnected".into();
            s.offers.clear();
            s.matches.clear();
            s.thinking = None;
        }
        ctx.emit(true);
        continue 'outer;
    }
}

fn ctx_send(writer: &mut TcpStream, cmd: &str) {
    let _ = writer
        .write_all(cmd.as_bytes())
        .and_then(|_| writer.write_all(b"\n"));
}

/// Swap the time-usage settings.
fn apply_pacing(
    ctx: &mut Ctx,
    pace: String,
    max_move_secs: u64,
    reserve_secs: u64,
    budget_use: f64,
) {
    // Broken values fall to defaults (timectl checks too; the screen
    // should show the corrected value).
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

/// GGS engine thread count; 0 marks "auto" (half the cores). The
/// snapshot carries it, so 0 avoids a type change; the screen shows 0
/// as-is — showing the resolved count would freeze "auto" into a fixed
/// number when the config moves to a different machine.
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

/// Re-resolve the global thread count. There is no GGS-specific
/// setting: two settings would need two calibrations, and the
/// uncalibrated one would silently degrade time management.
fn apply_threads(ctx: &mut Ctx) {
    let n = resources().threads.unwrap_or_else(|| resolve_threads(0));
    ctx.engine_cfg.threads = n;
    if let Some(e) = ctx.engine.as_mut() {
        e.set_threads(n);
    }
    ctx.snap.lock().unwrap().engine.threads = n;
}

/// Parse an update/join block, refresh board state, and think+play if
/// it is our turn.
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
    // The 4th field of "/os: join .45.0 s8r14 K?" is the game type.
    if let Some(t) = block[0].split_whitespace().nth(3) {
        if t.starts_with('8') || t.starts_with("s8") {
            m.gtype = t.to_string();
        }
    }
    let was_overtime = m.in_overtime;
    let (rows_ok, turn) = apply_block(m, block, login);
    /* Always announce entering overtime: the game is decided yet the
    clock looks like a healthy 2 minutes (grace adds onto it). Without
    this line even the logs cannot tell. */
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

    // Snapshot update.
    sync_matches(ctx, matches);
    ctx.emit(true);

    // ---- Think if it is our turn ----
    let (auto, watch_an) = {
        let s = ctx.snap.lock().unwrap();
        (s.auto_play, s.watch_analysis)
    };
    let m = matches.get_mut(&mid).unwrap();
    if m.my_color.is_none() {
        // Watched game: analyze in the background.
        if watch_an && rows_ok && turn.is_some() {
            analyze_watch(ctx, &mid, matches);
        }
        return;
    }
    if turn.is_some() && turn == m.my_color && !m.told_turn {
        // Chime only when the human plays (auto off); with KUROOBI
        // playing it is pure noise.
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
        // Re-arm the chime when the turn passes back.
        let m = matches.get_mut(&mid).unwrap();
        m.told_turn = false;
    }
    /* Ponder marker: remember whose-turn games and read in slices
    between receives (a full read would block noticing their move). */
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

/// Ponder a slice during the opponent's turn. Each call reads only
/// 200ms — a full read would block the 250ms receive loop, so the two
/// alternate. Works under fixed depth too (same depth in 1/3 the
/// time); "useless under fixed depth" was written once and was wrong.
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
    /* Build the opponent-to-move board: `board_of` makes the given
    color the mover, so passing our color here would flip the turn
    (think_and_play correctly passes ours). */
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
    // Watch analysis stays light enough not to disturb our own games.
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

/// Build a Board from MatchState cells with the given mover.
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

/// Apply a block's players, clocks, boards and move history to
/// MatchState; returns (whether 8 board rows completed, mover).
fn apply_block(m: &mut MatchState, block: &[String], login: &str) -> (bool, Option<char>) {
    let mut rows: Vec<Vec<char>> = Vec::new();
    let mut boards: Vec<Vec<Vec<char>>> = Vec::new();
    let mut turns: Vec<char> = Vec::new();
    let mut turn: Option<char> = None;
    // Moves known before reading this block; needed for start-position
    // capture (the block itself carries move rows, so counting after
    // is too late).
    let moves_before = m.moves.len();
    m.players.clear();
    for l in block {
        let b = l.strip_prefix('|').unwrap_or(l);
        // Player row: `name (rating color) clock`.
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
                    // Join blocks send start and current pairs; a
                    // repeated name replaces with the current info.
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
                        /* ---------- Overtime detection ----------

                        A rising clock means overtime: GGS runs a single
                        clock and adds the grace onto it when main time
                        expires; the display's third field never moves,
                        so the jump is the only observable signal. Once
                        set, never cleared — the game is decided and only
                        the wipeout remains avoidable.

                        With increments the clock may rise every move, so
                        only a rise beyond the configured increment
                        counts. A "half the grace" threshold misses
                        cases: grace is additive, so remaining time is
                        subtracted from the jump (50s left + 2min grace
                        after a 30s overrun jumps only 40s — and that
                        slipped through, unreported on screen). Missing
                        it forfeits the wipeout-avoidance switch: a
                        minimal loss can become -64. */
                        if let (Some(now), Some(g)) = (main, ext) {
                            /* Two observed appearances: (1) the clock
                            jumps (`00:05` -> `01:59`); (2) it pins at
                            `00:00` while moves continue — the more
                            common. Both mean main time is gone; catch
                            both. The zero check may fire at 0.4s
                            remaining shown as 00:00, harmless — there is
                            no time to think either way. */
                            let bump = inc.unwrap_or(0);
                            let jumped = m
                                .my_clock_secs
                                .is_some_and(|prev| now > prev.saturating_add(bump));
                            /* Since the flag never clears, a false early
                            trigger throws away the whole game (1.5s/move
                            after). A zero before our first move must be
                            something else — you cannot exhaust time you
                            never used. The jump form is distinctive and
                            passes. */
                            let played = m.moves.len() >= 2;
                            if g > 0 && (jumped || (now == 0 && played)) {
                                m.in_overtime = true;
                            }
                        }
                        /* Freeze clocks after the game: updates still
                        arrive and once wrote the opponent's remaining
                        time into our field. The game-end value is final. */
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
        // Board row.
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
                // Eight rows complete one board.
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
        /* Move rows: `N: F5/...` or `N: F5//1.2`.

        Row 0 is not a move: the join marks "no moves yet" with
        `|0 move(s)` and `|  0: PASS`. Stacking it as a move breaks two
        things: a leading `B[PA]` shifts every color, and the "no moves
        yet" check fails, losing the drawn-opening start position — the
        record then cannot replay (viewer and book learning both broke). */
        if let Some(colon) = t.find(':') {
            let (num, rest) = t.split_at(colon);
            if let Some(n) = num.trim().parse::<u32>().ok().filter(|n| *n > 0) {
                /* Evals and times ride along: `3: C2/20.00/122.16`
                (move/eval/seconds), either tail field possibly empty.
                For opponents that report, this is the only window into
                their reading. Mover-view discs. */
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
                    let ev = parts.next().and_then(|x| x.trim().parse::<f32>().ok());
                    let sec = parts.next().and_then(|x| x.trim().parse::<f32>().ok());
                    m.moves.insert(n, mv);
                    // Never overwrite with blanks (join lists may lack values).
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

    /* Record the opponent's reported value when the last move was
    theirs (our turn now = their move last; numbered passes keep the
    parity sound). Non-reporting opponents leave None. */
    if let (Some(t), Some(mc)) = (turn, m.my_color) {
        /* Cache the parity once known — the turn vanishes at game end
        and the post-game eval chart used to vanish with it. */
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

    // GGS boards are row-major; map into file-major indices.
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
    // Watch joins carry two boards (start and current); our updates
    // carry one. Either way the last is current.
    if let Some(last) = boards.last() {
        m.cells = to_cells(last);
    }
    /* With two boards the first is the start position (not standard
    for drawn openings) — capture once, never overwrite. Our own games
    send only ONE board, and missing that case once saved drawn games
    as "standard start + moves from ply 17", unreplayable (11 of 12
    games). A board that arrives before any move is the start; capturing
    from a block that already has moves would take a mid-game board. */
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

/// Plan one move's search settings from the clock. The logic lives in
/// `kuroobi::timectl` (measurable from self-play); this is only the
/// bridge. Returns (midgame depth, solve-entry empties, band, deadline).
fn time_budget(
    mut s: kuroobi::timectl::Situation,
    base: (u32, u8, u8),
    pace: &str,
) -> (u32, u8, u8, Option<Duration>) {
    // Derive the solve entry from the clock when calibrated; the
    // config lookup stays here so GGS and local cannot diverge.
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

/// Run the engine on our turn and send the move.
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
    /* Double-move protection, mixing in the move count: board-only
    hashing matches after an opponent pass (same board, turn returned)
    and would silently never move again. */
    let bh = board
        .black
        .wrapping_mul(31)
        .wrapping_add(board.white)
        .wrapping_add((m.moves.len() as u64).wrapping_mul(0x9e37_79b9));
    if bh == m.last_played_hash {
        return;
    }
    /* Never re-dispatch a position already being searched.
    `last_played_hash` is set after sending, so during the search the
    missed-move rescue could queue the same position again and send the
    move twice. Spotted in the wire log: 58 plays in a 60-move game —
    the server rejects duplicates so the record stayed correct, but
    every move searched twice, halving the effective clock. Check both
    the running search and the queue. */
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
            // The post-split count: two run at once, and the global
            // count would overestimate the solve entry.
            threads: ctx.worker_threads,
            ..Default::default()
        },
        base,
        &ctx.engine_cfg_pace,
    );
    /* Dispatch and don't wait: waiting here would burn one board's
    clock. The receive loop collects results and sends the moves. */
    if let Some(i) = ctx.worker_for(mid) {
        /* Queue the pending search: think_and_play only fires on
        updates, so a missed dispatch means never moving (happened —
        clock drained to a flag). A busy ponder may be discarded: it is
        free, and not moving costs far more. */
        ctx.workers[i].pending = Some(Pending {
            board,
            levels: (d, solve, band),
            cap,
            hash: bh,
            // Try the mirror-borrowed move first if available.
            hint: mirror_hint(matches, mid),
        });
        if ctx.workers[i].pondering {
            ctx.workers[i].stop.stop();
        }
        pump_worker(ctx, i);
        return;
    }
    // Fallback when no worker could be built: search inline as before.
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
            /* Log the reached depth too — without it "losing while
            deep" and "simply shallow" cannot be told apart (7 analyzed
            games showed 0.55 discs/move lost with no way to attribute
            it). */
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
            // Solves and book moves carry no depth (0); omit it.
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

/// Dispatch the queued real search if the worker is free. Also called
/// right after collect_workers clears `busy`, so nothing stays stuck.
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
    // Reset the stop left over from aborting the ponder.
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

/// Collect worker moves. Returns the lines to send rather than sending
/// (`send!` borrows `writer` directly). Also dispatches ponders on free
/// workers for opponent-turn games — pondering is free and worth
/// +1.6-1.8 plies.
fn collect_workers(ctx: &mut Ctx, matches: &mut HashMap<String, MatchState>) -> Vec<String> {
    let mut out = Vec::new();
    /* Stop searches far past their deadline: if some path still fails
    to return (missed abort, unexpected think), the game stalls and the
    flag costs rating. Backup stop at 3x the deadline. */
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
    /* Abandon and rebuild a worker that ignores its stop — its seat
    would never free again, and there are only two. The abandoned
    thread exits itself once its search ends (one extra thread briefly:
    cheap insurance for a should-never-happen path). */
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
        // Collect a reply if present; otherwise do nothing.
        let done = ctx.workers[i].rx.try_recv().ok();
        let Some(done) = done else { continue };
        ctx.workers[i].busy = false;
        // Take the timing before re-dispatch (pump_worker overwrites sent_at).
        let took = ctx.workers[i].sent_at.map(|(t, _)| t.elapsed());
        ctx.workers[i].sent_at = None;
        // Re-dispatch immediately after collecting — otherwise the
        // queued search waits for the next update (possibly forever).
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
        /* Never move into a finished game (resignations land
        mid-search). Cleanup runs every loop, but check again right
        before sending rather than trusting ordering. Same when it is
        no longer our turn. */
        if m.over || m.my_color.is_none() || Some(m.turn) != m.my_color {
            continue;
        }
        /* Verify the searched position is still the current one: a
        queued search dispatches the board as of queueing, and if the
        game advanced meanwhile, the answer belongs to another position
        — possibly legal, silently accepted, and recorded as an absurd
        blunder. Should never happen, so say it loudly; discarding
        lets the next update re-search. */
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

    /* Free workers ponder opponent-turn games. The old 200ms slicing
    existed only because the engine ran inside the receive loop; workers
    are separate threads, and slicing a 30-second think into 150 pieces
    just restarts deepening from shallow each time. */
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
            // Skip finished games: the blank turn looks like "their
            // turn" and would ponder forever.
            if m.over {
                continue;
            }
            // Our turn = real search's turn; no ponder.
            if m.my_color.is_none() || Some(m.turn) == m.my_color {
                continue;
            }
            /* Build the opponent-to-move board; passing our color
            would flip the turn. */
            let Some(board) = board_of(m, m.turn) else {
                continue;
            };
            // A queued real search takes priority over pondering.
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
            ended: m.ended.clone(),
            left_by: m.left_by.clone(),
            archive: m.archive.clone(),
            /* Copy the assigned worker's progress: the search writes
            atomics at iteration boundaries; this only reads. */
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
            watch_exact: m.watch_exact,
            seen: m.seen,
            order: m.order,
        })
        .collect();
    /* Newest first. Sorting by id was string order (`.23` < `.41` <
    `.8`) and inserted new games mid-list; sort by arrival order. */
    view.sort_by_key(|v| std::cmp::Reverse(v.order));
    /* Overlay running workers' progress; the assignment (`mid`) says
    which board. During ponder the move is the predicted reply, so
    `busy` disambiguates. */
    for v in &mut view {
        let Some(w) = ctx.workers.iter().find(|w| w.mid.as_deref() == Some(&v.id)) else {
            continue;
        };
        if !w.busy {
            continue;
        }
        let (kind, depth, best, eval) = w.progress.snapshot();
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
    // Observed: ".25 1720.0 kuroobi  15:00//02:00  8 R 1438.6 fly"
    // (first name = offerer; R = rated).
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

/// Read how a game ended from the `- match` body.
///
/// Observed:
/// - finished:  `.13 1866 kuroobi 1411 fly 8 R +54.00  .82720`
/// - adjourned: `.52 2326 kuroobi 2447 piglet s8r16 R piglet left .84058`
/// - aborted:   `... aborted`
///
/// Never judge by disc-difference presence alone: an adjournment has
/// none and would masquerade as an unreadable finish, dragging the
/// clock display into lying with it.
fn end_kind(rest: &str) -> (&'static str, String) {
    if rest.contains(" aborted") {
        return ("aborted", String::new());
    }
    // "<name> left" — capture who departed.
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
    // Observed: ".13 1866 kuroobi 1411 fly 8 R +54.00  .82720"
    // (tail = archive id; score is from the FIRST-listed player's view
    //  — "Black's view" was wrong and flipped synchro results).
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let id = toks.first().copied().unwrap_or("").to_string();
    if !rest.contains(login) {
        // Ignore other players' game ends (offer updates only).
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
    // Synchro ends arrive on the parent id; collect `.N.0`/`.N.1`
    // together, keeping `.N.0` as the representative record.
    let (kind, who) = end_kind(rest);
    // The observed tail is the archive id: "... +54.00  .82720"
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
    /* The disc difference is from the FIRST-listed player's view, not
    a color:

        - match .48 2639 kuroobi 2644 Rhapsody s8r16 R +2.00
                       ~~~~~~~ +2 from this player's view

    Deciding by our color once flipped a rated +2.00 win into a
    displayed -2 loss (synchro boards carry opposite colors, and the
    representative depended on arrival order). */
    let my_diff = score.map(|s| my_stone_diff(s, first_name, login));
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
        // Observed tail = archive id: "... +54.00  .82720"
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
        /* Record the rating of the game's own pool: `my_rating` is
        whatever who/rank last returned, pool-blind, and once stamped
        8-pool ratings onto 8r games — a flat 2184 line in the 8r
        trend (4 of 73 games). */
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
    // Standby statistics.
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

    // ---- Queue game imports (learning) ----
    // Our games import win or lose: losses/draws to avoid repeats,
    // wins so mistake-gifted lines don't stay overrated. Runs one
    // search at a time between games; synchro imports both boards.
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
                // Count the final discs by replaying — the match record
                // is gone by the time the import finishes.
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
                    // Details fill in when the import completes.
                    changes: Vec::new(),
                    opponent: lm.opp_name.clone(),
                    // GGS knows our color ('*' black / 'O' white).
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

/// Advance the learning queue by one search (called while we have no
/// game); finished imports are logged and dequeued.
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
            // Log to the same place as local games; the opponent name
            // tells the source.
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

/// Whether a line is a reply-body header (vs the ACK READY).
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
        // E.g. a ": finger" echo, or starting from a "login  : <name>" line.
        ln.starts_with(": finger") || ln.trim_start().starts_with("login")
    } else {
        false
    }
}

/// Extract the leading rating from "1938.0@71.7=" / "1720.0@350.0";
/// deviation tokens (`74.9=` etc.) drop their trailing trend mark.
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

fn finish_capture(ctx: &mut Ctx, kind: &str, buf: &[String], login: &str) {
    if let Some(id) = kind.strip_prefix("look:") {
        // Observed: `|1 (;GM[Othello]PC[GGS/os]...;)` (leading serial).
        /* Arrives wrapped across lines; join first, then split.
        Synchro archives hold two games — take them all. */
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
        // who: `|Rhapsody + 1720.0@350.0 ->   +33.6 ...`
        // top: `|    2 kuroobi  2184.2@179.3=  ...` (leading rank).
        let mut users = Vec::new();
        let mut my_rating = None;
        for l in buf {
            let b = l.strip_prefix("/os: ").unwrap_or(l);
            let b = b.strip_prefix('|').unwrap_or(b);
            let mut toks: Vec<&str> = b.split_whitespace().collect();
            if kind == "top" {
                // Drop the leading rank number.
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
            // Observed: `| 1 scorpion 1938.0@ 74.9= ...` — the `@`
            // trails the rating and the deviation is the NEXT token
            // (with a trend mark suffix).
            let at = toks
                .iter()
                .enumerate()
                .skip(1)
                .find_map(|(i, t)| parse_rating_token(t).map(|v| (i, v)));
            let Some((ri, r)) = at else { continue };
            let rating = Some(r);
            // Both `1938.0@ 74.9=` (next token) and `2066.9@126.4=`
            // (same token) occur.
            let dev = match toks[ri].split_once('@') {
                Some((_, rest)) if !rest.is_empty() => parse_dev_token(rest),
                Some(_) => toks.get(ri + 1).and_then(|t| parse_dev_token(t)),
                None => None,
            };
            if name == login {
                my_rating = rating;
            }
            // The token after the name is the accept flag (who only).
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
        // GGS top is not rating-descending (deviation-adjusted), so
        // both lists are re-sorted by rating; GGS's ranks are unused.
        users.sort_by(|a, b| {
            b.rating
                .unwrap_or(0.0)
                .partial_cmp(&a.rating.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut s = ctx.snap.lock().unwrap();
        if !users.is_empty() {
            match kind {
                /* 8r only overlays: same people, so fill the 8r rating
                by name instead of rebuilding rows (a mixup would
                overwrite the normal rating). */
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
            // Backfill the latest game's rating (the post-game who).
            if let Some(latest) = s.results.first_mut() {
                if latest.my_rating.is_none() {
                    latest.my_rating = Some(r);
                }
            }
        }
        drop(s);
        ctx.dirty = true;
    } else if kind == "match_list" {
        // Observed: "| .70 2639 nyanyan  2606 egrcd  s8r14  R 0"
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
        // Preserve the watching flag.
        for o in list.iter_mut() {
            if s.ongoing.iter().any(|x| x.id == o.id && x.watching) {
                o.watching = true;
            }
        }
        let ids: Vec<String> = list.iter().map(|o| o.id.clone()).collect();
        s.ongoing = list;
        drop(s);
        ctx.dirty = true;
        // Verification: KUROOBI_GGS_AUTOWATCH=auto watches every
        // ongoing game when the list arrives (external ids go stale
        // between fetch and launch).
        if std::env::var("KUROOBI_GGS_AUTOWATCH").as_deref() == Ok("auto") {
            for id in ids {
                ctx.auto_watch.push(id);
            }
        }
    } else if kind == "stored_list" {
        // Observed: "|.82740  30 Jul 2026 22:30:00 kuroobi  Rhapsody s8r16:l"
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
    } else if let Some(target) = kind.strip_prefix("history:") {
        // Observed:
        // "|.82720   30 Jul 2026 17:36:36 1720 kuroobi  1439 fly       +54.0 8"
        let mut rows = Vec::new();
        for l in buf {
            let Some(rest) = l.strip_prefix('|') else {
                continue;
            };
            let t: Vec<&str> = rest.split_whitespace().collect();
            // id, day, mon, year, time, r1, n1, r2, n2, score, type
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
    } else if let Some(rest) = kind.strip_prefix("rank:") {
        // Observed: "|   17 kuroobi  2579.7@181.2=  21:50:36+@181.0  -0.6  0 2 3 <="
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
            // The last three integers are wins/draws/losses.
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
    } else if let Some(name) = kind.strip_prefix("osfinger:") {
        // /os finger is "heading : value"; game settings are appended
        // to the server-wide finger's fields.
        let mut add: Vec<(String, String)> = Vec::new();
        for l in buf {
            // The first reply line echoes the command.
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
    } else if let Some(name) = kind.strip_prefix("finger:") {
        // Finger is "heading : value"; split per heading.
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut raw: Vec<String> = Vec::new();
        for l in buf {
            let t = l.trim_end();
            if t.is_empty() || t == ":" {
                continue;
            }
            let t = t.strip_prefix(": ").unwrap_or(t);
            // Our own finger echoes the password; never retain it.
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
}

// ==================== Tests (formats from real logs) ====================

#[cfg(test)]
mod tests {

    /// Real log: the block received when starting to watch an s8r14
    /// game; carries drawn-start and current boards.
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
        // Real log: |kuroobi  (1720.0 *) 14:59,0:0//02:00,0:0
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

    /// Mirror-borrow conditions: the boards coincide only while the
    /// sequences match; after divergence the borrowed move may be illegal.
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
        // We are at 2 moves, the mirror at 3, sequences matching.
        ms.insert(".9.0".to_string(), mk(&[("", "e2"), ("", "g4")]));
        ms.insert(
            ".9.1".to_string(),
            mk(&[("", "e2"), ("", "g4"), ("", "g5")]),
        );
        let h = mirror_hint(&ms, ".9.0").expect("borrows the hint");
        assert_eq!(coord(h), "G5");

        // Nothing to borrow if the mirror is not ahead.
        assert!(mirror_hint(&ms, ".9.1").is_none());

        // Never borrow after divergence.
        ms.insert(
            ".9.1".to_string(),
            mk(&[("", "e2"), ("", "h4"), ("", "g5")]),
        );
        assert!(
            mirror_hint(&ms, ".9.0").is_none(),
            "borrowed after the boards diverged"
        );

        // No partner board, nothing to borrow.
        ms.remove(".9.1");
        assert!(mirror_hint(&ms, ".9.0").is_none());
    }

    /// The score sign follows the first-listed player, not a color;
    /// verified against 4 real rated game-over lines (one win was once
    /// displayed as a loss).
    #[test]
    fn the_stone_diff_follows_the_first_name() {
        // We are listed first: as-is.
        assert_eq!(my_stone_diff(2.0, "kuroobi", "kuroobi"), 2);
        assert_eq!(my_stone_diff(-10.0, "kuroobi", "kuroobi"), -10);
        assert_eq!(my_stone_diff(-5.0, "kuroobi", "kuroobi"), -5);
        assert_eq!(my_stone_diff(-7.0, "kuroobi", "kuroobi"), -7);
        // Opponent first: negate.
        // ".18 1720 htz 2580 kuroobi s8r16 U -6.00" = kuroobi wins by 6.
        assert_eq!(my_stone_diff(-6.0, "htz", "kuroobi"), 6);
        assert_eq!(my_stone_diff(2.0, "htz", "kuroobi"), -2);
        // A draw is 0 either way.
        assert_eq!(my_stone_diff(0.0, "kuroobi", "kuroobi"), 0);
        assert_eq!(my_stone_diff(0.0, "htz", "kuroobi"), 0);
    }

    /// Undo/abort requests; format captured live (undocumented).
    #[test]
    fn request_real_format() {
        let r = parse_request("/os: undo .24 htz is asking", "kuroobi").unwrap();
        assert_eq!(
            (r.verb, r.id.as_str(), r.who.as_str()),
            ("undo", ".24", "htz")
        );
        let r = parse_request("/os: abort .24 htz is asking", "kuroobi").unwrap();
        assert_eq!(r.verb, "abort");
        // Our own requests echo back identically; never react.
        assert!(parse_request("/os: undo .24 kuroobi is asking", "kuroobi").is_none());
        // Reject look-alikes.
        assert!(parse_request("/os: update .24 8 K?", "kuroobi").is_none());
        assert!(parse_request("/os: undo .24 htz declined", "kuroobi").is_none());
        assert!(parse_request("/os: - match .24 1720 htz", "kuroobi").is_none());
    }

    /// Negative clocks parse as 0 — a parse failure means `None` =
    /// untimed = thinking without a deadline.
    #[test]
    fn a_negative_clock_reads_as_zero() {
        let (main, _, ext) = parse_clock("-0:03,0:0//02:00,0:0");
        assert_eq!(main, Some(0));
        assert_eq!(ext, Some(120));
    }

    /// A clock jump means overtime: GGS adds the grace onto its single
    /// clock, and the display's third field never moves — the jump is
    /// the only signal.
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
            // Include a move row: the zero-pin check requires at least
            // one move (unused time cannot be exhausted).
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
        // Main time gone: the server adds the 2:00 grace.
        apply_block(&mut m, &with_clock("02:00,0:0"), "kuroobi");
        assert!(m.in_overtime, "missed the jump");
        // The more common form: pinned at 00:00 while moves continue.
        let mut m2 = MatchState::new();
        apply_block(&mut m2, &with_clock("00:03,0:0"), "kuroobi");
        assert!(!m2.in_overtime);
        apply_block(&mut m2, &with_clock("00:00,0:0"), "kuroobi");
        assert!(m2.in_overtime, "missed the clock stuck at 0");
        // Never clears once set (the clock then just runs down).
        apply_block(&mut m, &with_clock("01:12,0:0"), "kuroobi");
        assert!(m.in_overtime, "cleared the overtime flag");
    }

    /// Chat survives crashes: appended per-login JSONL, read back at
    /// startup; torn lines are dropped, the rest kept.
    #[test]
    fn chat_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("kuroobi-chat-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let prev = std::env::current_dir().unwrap();
        // chat_path sits next to the history; verify from a moved cwd.
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
        // A corrupt line must not take the rest down.
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

    /// The game-start `0: PASS` is not a move: it marks "no moves yet".
    /// Stacking it once shifted every color and lost drawn-opening
    /// start positions, leaving unreplayable records.
    #[test]
    fn the_zero_move_marker_is_not_a_move() {
        // A join with the drawn board (6 discs), no moves yet.
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

    /// Clocks freeze after the game; updates still arrive.
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

    /// Our own games capture the start position too: only watch joins
    /// send two boards, and the one-board case used to lose drawn
    /// starts, leaving unreplayable records.
    #[test]
    fn a_dealt_opening_is_kept_as_the_start() {
        // A single drawn (non-standard) board arrives.
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

    /// A mid-game board must not become the start position (blocks
    /// with moves attached do not qualify).
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

    /// Detect overtime even with time left at the overrun: grace is
    /// additive, so the remaining time subtracts from the jump (50s
    /// left -> a 40s jump); the old half-grace threshold missed this.
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

    /// Increment games must not mistake the increment for overtime.
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
        // 20s/move increment: spend 5s, gain 20s, net +15s.
        apply_block(&mut m, &with_clock2("05:00,0:0", "0:20"), "kuroobi");
        apply_block(&mut m, &with_clock2("05:15,0:0", "0:20"), "kuroobi");
        assert!(!m.in_overtime, "mistook an increment for overtime");
    }

    /// A zero before any move never triggers: the flag is sticky, and
    /// unused time cannot be exhausted.
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

    /// A normally draining clock is not overtime.
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
        // Our own offer (kuroobi listed first) is not incoming.
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
        // An opponent's offer (they are first) is incoming.
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
        // Real-log join block (excerpt).
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
        // Center four: D4=O E4=* D5=* E5=O (file-major = file*8 + rank).
        let disc = |f: usize, r: usize| m.cells[f * 8 + r];
        assert_eq!(disc(3, 3), 2); // D4 = O
        assert_eq!(disc(4, 3), 1); // E4 = *
        assert_eq!(disc(3, 4), 1); // D5 = *
        assert_eq!(disc(4, 4), 2); // E5 = O
        assert_eq!(m.cells.iter().filter(|&&c| c != 0).count(), 4);
        /* Move history (rows with eval/time too). Row 0 is never
        stacked — it is the no-moves-yet marker
        (`the_zero_move_marker_is_not_a_move`). */
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
        // Real log: block on starting to watch; carries start and
        // current boards — take the current.
        let block: Vec<String> = WATCH_JOIN_BLOCK.iter().map(|s| s.to_string()).collect();
        let mut m = MatchState::new();
        let (rows_ok, turn) = apply_block(&mut m, &block, "kuroobi");
        assert!(rows_ok, "parses even with two boards");
        assert_eq!(turn, Some('*'));
        // We are not a player: treat as watching.
        assert_eq!(m.my_color, None);
        // Two players; two clock rows arrive but never duplicate.
        assert_eq!(m.players.len(), 2);
        assert_eq!(m.players[0].name, "nyanyan");
        assert_eq!(m.players[0].color, "black");
        assert_eq!(m.players[1].name, "egrcd");
        assert_eq!(m.players[1].color, "white");
        // Adopt the current board: this s8r14 game has the drawn start
        // (14 discs) first and the current (38) second; grabbing the
        // start would show 14.
        assert_eq!(m.moves.len(), 24);
        let stones = m.cells.iter().filter(|&&c| c != 0).count();
        assert_eq!(stones, 38, "disc count of the current position");

        // The first board is the drawn start; kept for reconstruction.
        assert_eq!(m.start_cells.iter().filter(|&&c| c != 0).count(), 14);
        let start = m.start_string();
        assert_eq!(start.len(), 66, "64 squares + space + side to move");
        assert!(
            start.ends_with(" X"),
            "Black is to move in the start position"
        );
        // kuroobi must parse the same format.
        let board = kuroobi::Board::from_string(&start).expect("parses as a board string");
        assert_eq!((board.black | board.white).count_ones(), 14);
    }

    #[test]
    fn ggf_round_trips_a_drawn_opening() {
        // A watched game serialized to GGF must replay to the current
        // position on the kuroobi side.
        let block: Vec<String> = WATCH_JOIN_BLOCK.iter().map(|s| s.to_string()).collect();
        let mut m = MatchState::new();
        apply_block(&mut m, &block, "kuroobi");

        let ggf = m.ggf(".45.0", Some("+4.00"));
        assert!(ggf.starts_with("(;GM[Othello]"));
        assert!(ggf.ends_with(";)"));
        assert!(ggf.contains("PB[nyanyan]"), "Black is nyanyan");
        assert!(ggf.contains("PW[egrcd]"), "White is egrcd");
        assert!(ggf.contains("RE[+4.00]"));
        // BO carries '*'/'O', 64 cells + mover.
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
        // 24 moves alternating from black (PB/RB/PW/RW precede BO,
        // so they cannot interfere).
        let moves_part = ggf.split_once("BO[").unwrap().1;
        assert_eq!(
            moves_part.matches("B[").count() + moves_part.matches("W[").count(),
            24
        );
        assert!(ggf.contains("B[F3]"), "move 1 is Black F3");
        assert!(ggf.contains("W[D2]"), "move 2 is White D2");

        // Must read back as a kuroobi board string.
        let start = bo.replace('*', "X");
        let board = kuroobi::Board::from_string(&start).expect("BO parses");
        assert_eq!((board.black | board.white).count_ones(), 14);

        // The GGF-reconstructed position must match the watched one.
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
        // Watching synchro streams .N.0 and .N.1 together; same
        // players, same move counts, but the boards must stay separate.
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
        // C6 is file=2, rank=5; E6 is file=4, rank=5.
        assert_eq!(matches[".56.0"].cells[2 * 8 + 5], 1);
        assert_eq!(matches[".56.0"].cells[4 * 8 + 5], 0);
        assert_eq!(matches[".56.1"].cells[2 * 8 + 5], 0);
        assert_eq!(matches[".56.1"].cells[4 * 8 + 5], 1);
        // Both watched (we are not a player).
        assert!(matches.values().all(|m| m.my_color.is_none()));
        // base_id groups them; the keys stay distinct.
        assert_eq!(base_id(".56.0"), ".56");
        assert_eq!(base_id(".56.1"), ".56");
    }

    #[test]
    fn synchro_end_clears_both_boards() {
        // Real game-over messages arrive on the parent id while the
        // boards are .4.0/.4.1; exact-match removal would leave them.
        let mut matches: HashMap<String, MatchState> = HashMap::new();
        matches.insert(".4.0".into(), MatchState::new());
        matches.insert(".4.1".into(), MatchState::new());
        matches.insert(".9".into(), MatchState::new());
        let dropped = drop_match(&mut matches, ".4");
        assert_eq!(dropped.len(), 2, "both synchro games are collected");
        assert!(!matches.contains_key(".4.0"));
        assert!(!matches.contains_key(".4.1"));
        assert!(matches.contains_key(".9"), "unrelated games are kept");

        // Non-synchro (exact match) still removes.
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
        // Only .4 and .4.0 qualify; .40 and .44.1 are different games.
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

    fn cap(secs: Option<u64>, empties: u8, pace: &str, max: u64) -> Option<Duration> {
        time_budget(
            kuroobi::timectl::Situation {
                clock_secs: secs,
                grace_secs: 120,
                empties,
                max_move_secs: max,
                ..Default::default()
            },
            BASE,
            pace,
        )
        .3
    }

    #[test]
    fn depth_mode_ignores_the_clock() {
        // Fixed depth: no deadline, read as configured.
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
        let a = cap(Some(900), 40, "fast", 0).unwrap();
        let b = cap(Some(300), 40, "fast", 0).unwrap();
        let c = cap(Some(60), 40, "fast", 0).unwrap();
        assert!(
            a > b && b > c,
            "less time left means a shorter move: {a:?} > {b:?} > {c:?}"
        );
    }

    /// Removed pacing schemes fall to the default; a stale "slow" in
    /// an old config must not resurrect a 0%-win-rate allocator.
    #[test]
    fn dropped_paces_fall_back_to_the_default() {
        let fast = cap(Some(900), 50, "fast", 0).unwrap();
        for p in ["slow", "even", ""] {
            assert_eq!(cap(Some(900), 50, p, 0).unwrap(), fast, "{p:?}");
        }
        // The equal-split formula survives as the baseline (thicker).
        let even = cap(Some(900), 50, "tail:1.0", 0).unwrap();
        assert!(even > fast, "even split {even:?} > default {fast:?}");
    }

    #[test]
    fn the_per_move_cap_is_honoured() {
        let c = cap(Some(3600), 50, "fast", 5).unwrap();
        assert!(
            c <= Duration::from_secs(5),
            "stays within the 5-second cap: {c:?}"
        );
    }

    #[test]
    fn out_of_main_time_plays_fast() {
        // Zero main time with grace: move on a very short deadline.
        let c = cap(Some(0), 30, "fast", 0).unwrap();
        assert!(c <= Duration::from_secs(1), "{c:?}");
    }

    #[test]
    fn the_endgame_keeps_a_reserve() {
        // One solve's worth is reserved; the midgame budget is less
        // than the full remainder.
        let (_, _, _, c) = time_budget(
            kuroobi::timectl::Situation {
                clock_secs: Some(100),
                empties: 40,
                ..Default::default()
            },
            BASE,
            // Check with the equal split; the thin default would not
            // exercise the reserve.
            "tail:1.0",
        );
        /* The deadline is a cap, not an allocation: deepening spends
        ~47% of it and BUDGET_USE stretches accordingly, so deadline x
        moves may exceed the clock. What must hold: one move's deadline
        never exceeds everything available minus the reserve. */
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

    /* GGS wants eval and think time on moves; omitting them causes
    trust violations. Added at the admin's request — pin the format. */

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

    /// Passes carry no eval — meaningless, and extra fields risk
    /// server-side misparsing.
    #[test]
    fn a_pass_goes_bare() {
        assert_eq!(play_arg("pa", 12.0, Some(Duration::from_secs(3))), "pa");
    }

    /// Book moves can carry infinite values; flatten to 0 rather than
    /// send the word "inf".
    #[test]
    fn a_non_finite_score_becomes_zero() {
        let s = play_arg("A1", f32::INFINITY, Some(Duration::from_secs(1)));
        assert_eq!(s, "A1/0.00/1.00");
    }

    /// Unmeasured time drops the field (honester than claiming 0s).
    #[test]
    fn an_unmeasured_move_omits_the_time() {
        assert_eq!(play_arg("C4", 2.0, None), "C4/2.00");
    }
}
