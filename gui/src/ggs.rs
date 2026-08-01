//! GGS (Generic Game Server, skatgame.net:5000) のセッションスレッド。
//!
//! 1 本のスレッドが TCP 接続・プロトコル解析・エンジン思考・待機モードを
//! すべて所有し、状態スナップショットを Tauri イベント "ggs" でフロントへ
//! 流す。設計は src/bin/ggs.rs (CLI クライアント) を基にした常駐 UI 化:
//! 対局中は絶対に自発離脱せず、切断は自動再接続 + stored 自動再開する。

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

// ============================ 外部コマンド ============================

pub enum Cmd {
    Connect {
        login: String,
        pw: String,
    },
    Disconnect,
    Raw(String),
    Ask {
        gtype: String,
        time: String,
        opponent: String,
    },
    Accept(String),
    Decline(String),
    Finger(String),
    Who(String),
    Top {
        gtype: String,
        n: u32,
    },
    /// 指定プールでの順位・レートを取得する。
    Rank {
        gtype: String,
        name: String,
    },
    Watch(String),
    /// 終わった対局の棋譜 (GGF) を取り出す。
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
        threads: usize,
    },
    SetAutoPlay(bool),
    SetWatchAnalysis(bool),
    /// 定石 book を使うか。研究や検証で自力の手を見たいときに切る。
    SetUseBook(bool),
    /// 対局操作: undo / abort / resign / (対局内 tell)。
    /// GGS 側の呼び名に合わせているので、enum 名との重なりは許す。
    #[allow(clippy::enum_variant_names)]
    MatchCmd {
        id: String,
        verb: String,
        arg: String,
    },
    /// サーバー側の自動受諾 (aform) / 自動拒否 (dform) 式を設定する。
    SetFormula {
        kind: String,
        expr: String,
    },
    /// 中断対局の一覧を取り直す。
    ListStored,
    /// 進行中の全対局を取り直す (ロビーの「対局中」表示用)。
    ListMatches,
    /// 中断対局を再開する (ask <.stored>)。
    ResumeStored(String),
    /// 対戦履歴 (自分または相手)。
    History(String),
    SetStandby(StandbyCfg),
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct StandbyCfg {
    pub enabled: bool,
    pub auto_accept: bool,
    pub opponent: String,
    pub gtype: String,
    pub time: String,
    pub max_games: usize,
    pub interval_secs: u64,
}

// ============================ スナップショット ============================

#[derive(Clone, Serialize, Default)]
pub struct LogLine {
    pub dir: String, // "in" | "out" | "info"
    pub text: String,
}

/// ランキング 1 行 (rank / top の出力)。
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

/// finger の解析結果 (見出しと値の組)。
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
    pub raw: String,
}

#[derive(Clone, Serialize, Default)]
pub struct ChatMsg {
    /// チャンネル名 (".chat" 等)。ダイレクトは空文字。
    pub chan: String,
    pub from: String,
    pub text: String,
    /// 受信時刻 (UNIX 秒)。会話として並べるのに要る。
    #[serde(default)]
    pub at: u64,
    /// 会話のまとまり。チャンネルはチャンネル名、ダイレクトは相手の名前。
    /// 画面はこれで会話を分ける。
    #[serde(default)]
    pub thread: String,
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
    /// 対局者名 (2 人)。
    pub names: Vec<String>,
    /// 対局者のレート (名前と同じ並び)。
    pub ratings: Vec<String>,
    pub gtype: String,
    /// 自分の対局か。
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
pub struct MatchView {
    pub id: String,
    pub base: String,
    pub cells: Vec<u8>, // 0 空 1 黒(*) 2 白(O)
    pub turn: String,   // "black" | "white" | ""
    pub my_color: String,
    pub opp_name: String,
    pub opp_rating: String,
    pub opp_clock: String,
    pub my_clock: String,
    /// 本時間の残り秒 (解析できた場合)。
    pub my_secs: Option<u64>,
    pub opp_secs: Option<u64>,
    /// ロスタイム (延長時間) の秒数。GGS の時計 "main/inc/ext" の第 3 要素。
    pub my_ext: Option<u64>,
    pub opp_ext: Option<u64>,
    /// 観戦用: 両プレイヤーの情報 (黒, 白 の順とは限らない)。
    pub players: Vec<PlayerView>,
    /// 対局の種別 ("s8r14" など)。
    pub gtype: String,
    pub moves: Vec<String>,
    /// GGF (Generic Game Format) の棋譜。開始局面を含むので、抽選
    /// オープニングの対局もこれだけで再生できる。
    pub ggf: String,
    pub last_eval: Option<f32>,
    pub last_eval_exact: bool,
    /// 直前の自分の手が定石 book 由来か。
    pub last_from_book: bool,
    /// 観戦解析の結果 (黒視点の評価値と最善手)。
    pub watch_eval: Option<f32>,
    pub watch_best: Option<String>,
    pub watch_exact: bool,
    pub seen: u64,
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct GameResult {
    pub id: String,
    pub base: String,
    pub raw: String,
    pub my_diff: Option<i32>,
    pub opp: String,
    pub kifu: String,
    /// GGF の棋譜 (開始局面つき)。
    #[serde(default)]
    pub ggf: String,
    /// GGS のアーカイブ番号。あとから `look` で棋譜を取り出せる。
    #[serde(default)]
    pub archive: String,
    pub seq: u64,
    /// 対局後の自分のレート (who で取得できた場合に後追いで埋まる)。
    #[serde(default)]
    pub my_rating: Option<f32>,
    /// UNIX 秒。
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
    pub my_rating: Option<f32>,
    /// プール別の自分のレート。
    pub my_ranks: Vec<RankRow>,
    pub log: VecDeque<LogLine>,
    pub users: Vec<UserRow>,
    pub ranking: Vec<UserRow>,
    pub fingers: HashMap<String, FingerInfo>,
    pub offers: Vec<Offer>,
    pub matches: Vec<MatchView>,
    pub ongoing: Vec<OngoingView>,
    pub stored: Vec<StoredView>,
    /// history の結果 (対象名 → 行)。
    pub history: HashMap<String, Vec<HistoryRow>>,
    pub chat: VecDeque<ChatMsg>,
    pub results: Vec<GameResult>,
    pub standby: StandbyCfg,
    pub standby_stats: StandbyStats,
    pub engine: EngineCfgView,
    pub auto_play: bool,
    /// 観戦中の対局を裏で解析する。
    pub watch_analysis: bool,
    pub thinking: Option<String>,
    /// 直近に取り出した棋譜 (GGF)。画面が受け取ったら消す。
    pub fetched_ggf: Option<FetchedGgf>,
}

/// 終わった対局から取り出した棋譜。
#[derive(Clone, Serialize)]
pub struct FetchedGgf {
    pub id: String,
    pub ggf: String,
    pub error: String,
}

#[derive(Clone, Serialize)]
pub struct EngineCfgView {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
    pub threads: usize,
    pub ready: bool,
    /// 定石 book を参照するか。
    pub use_book: bool,
    /// book のファイルを読み込めているか。
    pub book_loaded: bool,
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
        }
    }
}

// ============================ セッション ============================

pub struct Handle {
    pub tx: Sender<Cmd>,
    pub snapshot: Arc<Mutex<Snapshot>>,
}

pub fn spawn(app: tauri::AppHandle) -> Handle {
    let (tx, rx) = mpsc::channel::<Cmd>();
    let snapshot = Arc::new(Mutex::new(Snapshot {
        conn: "disconnected".into(),
        standby: StandbyCfg {
            enabled: false,
            auto_accept: true,
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
    std::thread::spawn(move || run(app, rx, snap2));
    Handle { tx, snapshot }
}

/// 対局履歴の保存先 (リポジトリの ggs_games/history.jsonl)。
fn history_path() -> PathBuf {
    for c in ["ggs_games", "../ggs_games", "../../ggs_games"] {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return p.join("history.jsonl");
        }
    }
    PathBuf::from("ggs_history.jsonl")
}

/// 起動時に履歴を読み込む (新しい順に最大 500 件)。
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

/// 設定ファイル (エンジンが使う重み・定石の場所)。ローカル対局と同じものを読む。
fn resources() -> kuroobi::resources::Resources {
    crate::resources()
}

/// GGS は同名の二重ログインを弾くため、複数ウィンドウから同時に接続すると
/// 後から繋いだ方が必ず失敗する。接続時にロックファイルへ PID を書き、
/// 生きている別プロセスが持っていれば繋ぎに行かない。
fn session_lock_path() -> PathBuf {
    std::env::temp_dir().join("kuroobi_ggs.pid")
}

/// ロックを取る。別プロセスが接続中ならその PID を返す。
fn try_lock_session() -> Result<(), i32> {
    let path = session_lock_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if let Ok(pid) = s.trim().parse::<i32>() {
            // シグナル 0 で生存確認 (自分自身は除く)
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
    /// 抽選オープニング (s8r14 など) の開始局面。着手列だけでは対局を
    /// 復元できないので、観戦開始時に届く 1 枚目の盤面を控えておく。
    /// 標準の初期局面から始まる対局では空のまま。
    start_cells: Vec<u8>,
    /// 開始局面の手番 ('*' / 'O')。
    start_turn: char,
    /// 対局の種別 ("s8r14" など)。レートのプールを見分けるのに使う。
    gtype: String,
    turn: char, // '*' / 'O' / ' '
    my_color: Option<char>,
    my_clock: String,
    my_clock_secs: Option<u64>,
    my_ext: Option<u64>,
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
    /// 観戦解析: 手番視点の評価値と最善手 (座標)。
    watch_eval: Option<f32>,
    watch_best: Option<String>,
    watch_exact: bool,
    watch_hash: u64,
    last_played_hash: u64, // 同一局面への二重着手防止
    seen: u64,
}

impl MatchState {
    fn new() -> Self {
        MatchState {
            cells: vec![0; 64],
            start_cells: Vec::new(),
            start_turn: ' ',
            gtype: String::new(),
            turn: ' ',
            my_color: None,
            my_clock: String::new(),
            my_clock_secs: None,
            my_ext: None,
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
            watch_eval: None,
            watch_best: None,
            watch_exact: false,
            watch_hash: 0,
            last_played_hash: 0,
            seen: 0,
        }
    }
    /// 開始局面を盤面文字列にする。標準の初期局面から始まるなら空を返す。
    ///
    /// 形式は kuroobi の Board と同じ rank-major 64 マス + 空白 + 手番。
    fn start_string(&self) -> String {
        if self.start_cells.len() != 64 {
            return String::new();
        }
        let mut out = String::with_capacity(66);
        for i in 0..64 {
            // 画面の cells は file-major、盤面文字列は rank-major
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

    /// GGF (Generic Game Format) にする。GGS が使う形式で、開始局面を
    /// `BO[8 ...]` に、着手を `B[]` / `W[]` に持つ。抽選オープニングの対局も
    /// これ 1 つで往復できる。
    fn ggf(&self, id: &str, result: Option<&str>) -> String {
        // BO は '*' (黒) と 'O' (白)。開始局面が無ければ標準の初期局面。
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
        // 着手は開始局面の手番から交互に。パスも 1 手として色を進める。
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

/// GGS の時計表記を秒に分解する。実形式 (実測ログより):
/// `15:00,0:0//02:00,0:0` = 本時間15分 / (加算なし) / 延長2分。
/// `/` 区切りの各要素は `,` の後ろに副フィールドを持つので先頭だけ読む。
fn parse_clock(s: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    let mut out = [None, None, None];
    for (i, part) in s.split('/').take(3).enumerate() {
        let p = part.trim().split(',').next().unwrap_or("").trim();
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
            out[i] = Some(secs);
        }
    }
    (out[0], out[1], out[2])
}

fn base_id(id: &str) -> String {
    // ".82726.1" → ".82726"、それ以外はそのまま
    let parts: Vec<&str> = id.split('.').collect();
    if parts.len() >= 3 && parts.last().map(|s| s.len() == 1) == Some(true) {
        parts[..parts.len() - 1].join(".")
    } else {
        id.to_string()
    }
}

/// 対局 ID に属する盤面をすべて取り除く。
///
/// synchro は 1 マッチが `.N.0` と `.N.1` の 2 局に分かれるが、開始と終了は
/// 親の `.N` で通知される。完全一致だけで消すと親 ID の通知が空振りし、
/// 終わった対局が盤面に残り続ける。
fn drop_match(matches: &mut HashMap<String, MatchState>, id: &str) -> Vec<MatchState> {
    let keys: Vec<String> = matches
        .keys()
        .filter(|k| k.as_str() == id || base_id(k) == id)
        .cloned()
        .collect();
    keys.iter().filter_map(|k| matches.remove(k)).collect()
}

fn coord(p: Position) -> String {
    let f = (b'A' + p.index() / 8) as char;
    let r = (b'1' + p.index() % 8) as char;
    format!("{f}{r}")
}

struct Ctx {
    app: tauri::AppHandle,
    /// エンジンの停止ハンドル (Engine 生成時に控える)。
    stop: Option<kuroobi::midgame::StopHandle>,
    snap: Arc<Mutex<Snapshot>>,
    engine: Option<Engine>,
    engine_cfg: EngineConfig,
    seq: u64,
    last_emit: Instant,
    dirty: bool,
    /// 検証用の自動観戦キュー (KUROOBI_GGS_AUTOWATCH=auto)。
    auto_watch: Vec<String>,
}

impl Ctx {
    /// macOS 通知。放置運用中に気付くべきイベントだけに使う。
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
        // 診断: 画面のログは 600 行で丸めるので、切り分け用に全量を残す
        // (デバッグビルドのみ)。
        #[cfg(debug_assertions)]
        {
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/ggs_session_wire.log")
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
        let r = self.app.emit("ggs", &s);
        // 診断: 状態と emit の成否をファイルに残す (UI が更新されない場合の
        // 切り分け用。デバッグビルドのみ)。
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
                            " [{} {}石 開始{}石 手番={} 自分={} 解析={}]",
                            m.id,
                            m.cells.iter().filter(|&&c| c != 0).count(),
                            m.ggf
                                .split_once("BO[8 ")
                                .map(|(_, r)| { r.chars().take(64).filter(|c| *c != '-').count() })
                                .unwrap_or(0),
                            if m.turn.is_empty() { "-" } else { &m.turn },
                            if m.my_color.is_empty() {
                                "観戦"
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
                        format!(" 棋譜取得={} {}文字", g.id, g.ggf.len())
                    }
                    Some(g) => format!(" 棋譜取得={} 失敗:{}", g.id, g.error),
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
                    if r.is_ok() { "ok" } else { "ERR" },
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

pub fn run(app: tauri::AppHandle, rx: Receiver<Cmd>, snap: Arc<Mutex<Snapshot>>) {
    let mut ctx = Ctx {
        app,
        stop: None,
        snap,
        engine: None,
        engine_cfg: EngineConfig {
            depth: 22,
            solve_empties: 26,
            band: 6,
            threads: (std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(8)
                / 2)
            .max(1),
            ..Default::default()
        },
        seq: 0,
        last_emit: Instant::now(),
        dirty: true,
        auto_watch: Vec::new(),
    };
    ctx.snap.lock().unwrap().engine = EngineCfgView {
        depth: 22,
        solve: 26,
        band: 6,
        threads: ctx.engine_cfg.threads,
        ready: false,
        use_book: ctx.engine_cfg.use_book,
        book_loaded: false,
    };

    // 接続ごとの外側ループ (未接続時はコマンド待ち)
    'outer: loop {
        ctx.emit(true);
        // ---- 未接続: Connect を待つ ----
        let (login, pw) = loop {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok(Cmd::Connect { login, pw }) => break (login, pw),
                Ok(Cmd::SetEngine {
                    depth,
                    solve,
                    band,
                    threads,
                }) => {
                    apply_engine_cfg(&mut ctx, depth, solve, band, threads);
                    ctx.emit(true);
                }
                Ok(Cmd::SetStandby(cfg)) => {
                    ctx.snap.lock().unwrap().standby = cfg;
                    ctx.emit(true);
                }
                Ok(Cmd::SetAutoPlay(b)) => {
                    ctx.snap.lock().unwrap().auto_play = b;
                    ctx.emit(true);
                }
                Ok(Cmd::SetUseBook(b)) => {
                    ctx.engine_cfg.use_book = b;
                    if let Some(e) = ctx.engine.as_mut() {
                        e.set_use_book(b);
                    }
                    ctx.snap.lock().unwrap().engine.use_book = b;
                    ctx.emit(true);
                }
                Ok(Cmd::SetWatchAnalysis(b)) => {
                    ctx.snap.lock().unwrap().watch_analysis = b;
                    ctx.emit(true);
                }
                Ok(Cmd::Rank { .. }) | Ok(Cmd::ListMatches) => {} // 未接続時は無視
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        };

        // 別ウィンドウが接続中なら繋ぎに行かない (二重ログインは必ず弾かれる)
        if let Err(pid) = try_lock_session() {
            ctx.log(
                "info",
                &format!(
                    "別のウィンドウ (PID {pid}) が GGS に接続しています。\
                     そちらを使うか、先に切断してください"
                ),
            );
            ctx.notify("GGS: 接続できません", "別のウィンドウが接続中です");
            ctx.snap.lock().unwrap().conn = "disconnected".into();
            ctx.emit(true);
            continue 'outer;
        }

        let mut reconnect = false; // 再接続中か (stored 再開を試す)
        let mut login_fails = 0u32; // ログイン前に切られた回数
                                    // ---- 接続セッション (切断時は絶対不放棄で再接続) ----
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
                    ctx.log("info", &format!("接続失敗: {e} — 15 秒後に再試行"));
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
            // ログインが進まないときに気付けるようにする。よくある原因は
            // 同じアカウントで別プロセスが接続したままの二重ログイン。
            let login_started = Instant::now();
            let mut login_warned = false;
            let mut lost = false;
            let mut in_block = false;
            let mut block: Vec<String> = Vec::new();
            let mut matches: HashMap<String, MatchState> = HashMap::new();
            // 応答待ちのコマンド種別。GGS は tell の ACK として先に READY を
            // 返し、本文 (ヘッダ行 + | 行 + READY) は後から届くので、ヘッダ行を
            // 見てから収集を始める。
            let mut pending: Vec<String> = Vec::new();
            let mut capture: Option<(String, Vec<String>)> = None; // (kind, lines)
            let mut next_ask_at: Option<Instant> = None;
            let mut want_quit = false;

            macro_rules! send {
                ($ctx:expr, $cmd:expr) => {{
                    let c: String = $cmd.to_string();
                    $ctx.log("out", &c);
                    let _ = writer
                        .write_all(c.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"));
                }};
                // ログには出さずに送る (パスワードなど)
                ($ctx:expr, $cmd:expr, secret) => {{
                    let c: String = $cmd.to_string();
                    $ctx.log("out", "********");
                    let _ = writer
                        .write_all(c.as_bytes())
                        .and_then(|_| writer.write_all(b"\n"));
                }};
            }

            loop {
                // ---------- UI コマンド ----------
                while let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        Cmd::Connect { .. } => {}
                        Cmd::Disconnect => {
                            if let Some(h) = &ctx.stop {
                                h.stop(); // 進行中の思考も打ち切る
                            }
                            send!(ctx, "quit");
                            want_quit = true;
                        }
                        Cmd::Raw(c) => send!(ctx, c),
                        Cmd::Ask {
                            gtype,
                            time,
                            opponent,
                        } => {
                            send!(ctx, format!("tell /os ask {gtype} {time} {opponent}"))
                        }
                        Cmd::Accept(id) => send!(ctx, format!("tell /os accept {id}")),
                        Cmd::Decline(id) => send!(ctx, format!("tell /os decline {id}")),
                        Cmd::Finger(name) => {
                            // finger は 2 種類ある。サーバー全体の finger は
                            // 素性 (登録名・接続時刻) を返し、/os finger は
                            // 対局に関わる設定 (受付・自動受諾条件) を返す。
                            // 相手に申し込む前に見たいのは後者なので両方取る。
                            pending.push(format!("finger:{name}"));
                            send!(ctx, format!("finger {name}"));
                            pending.push(format!("osfinger:{name}"));
                            send!(ctx, format!("tell /os finger {name}"));
                        }
                        Cmd::Who(t) => {
                            pending.push("who".into());
                            send!(ctx, format!("tell /os who {t}"));
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
                        Cmd::Watch(id) => {
                            send!(ctx, format!("tell /os watch + {id}"));
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
                            ctx.dirty = true;
                        }
                        Cmd::Chat { target, text } => {
                            send!(ctx, format!("tell {target} {text}"));
                            // 自分の発言もチャット欄に出す
                            let me = login.clone();
                            let mut s = ctx.snap.lock().unwrap();
                            let is_chan = target.starts_with('.');
                            s.chat.push_back(ChatMsg {
                                chan: if is_chan {
                                    target.clone()
                                } else {
                                    format!("→{target}")
                                },
                                from: me,
                                text,
                                at: now_secs(),
                                thread: target,
                            });
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
                            threads,
                        } => {
                            apply_engine_cfg(&mut ctx, depth, solve, band, threads);
                        }
                        Cmd::SetAutoPlay(b) => {
                            ctx.snap.lock().unwrap().auto_play = b;
                            ctx.dirty = true;
                        }
                        Cmd::SetUseBook(b) => {
                            ctx.engine_cfg.use_book = b;
                            if let Some(e) = ctx.engine.as_mut() {
                                e.set_use_book(b);
                            }
                            ctx.snap.lock().unwrap().engine.use_book = b;
                            ctx.dirty = true;
                        }
                        Cmd::SetWatchAnalysis(b) => {
                            ctx.snap.lock().unwrap().watch_analysis = b;
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
                            ctx.log("info", &format!("中断対局 {id} を再開します"));
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
                            ctx.log("info", &format!("{kind} を設定: {expr}"));
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
                            drop(s);
                            next_ask_at = Some(Instant::now() + Duration::from_secs(3));
                            ctx.dirty = true;
                        }
                    }
                }
                if want_quit {
                    break 'session;
                }

                // ---------- ソケット ----------
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
                        // ログイン前に切られた = 認証が通っていない。無限に
                        // 再接続しても同じなので、原因を出して待機に戻る。
                        // 最頻の原因は同じアカウントでの二重ログイン。
                        login_fails += 1;
                        ctx.log(
                            "info",
                            "ログイン中にサーバーから切断されました。同じアカウントで\
                             別のプロセスが接続していないか確認してください \
                             (GGS は同名の二重ログインを弾きます)",
                        );
                        ctx.notify("GGS: ログインできません", "二重ログインの可能性があります");
                        if login_fails >= 2 {
                            let mut s = ctx.snap.lock().unwrap();
                            s.conn = "disconnected".into();
                            drop(s);
                            ctx.emit(true);
                            break 'session; // 待機に戻る (再接続はユーザー操作で)
                        }
                        ctx.emit(true);
                        std::thread::sleep(Duration::from_secs(3));
                        continue 'session;
                    }
                    ctx.notify("GGS: 接続断", "10 秒後に自動再接続します");
                    ctx.log("info", "接続断 — 10 秒後に再接続して対局を再開します");
                    ctx.emit(true);
                    std::thread::sleep(Duration::from_secs(10));
                    reconnect = true;
                    continue 'session;
                }
                while let Some(nl) = raw.iter().position(|&b| b == b'\n') {
                    let mut line: Vec<u8> = raw.drain(..=nl).collect();
                    while matches!(line.last(), Some(b'\n') | Some(b'\r')) {
                        line.pop();
                    }
                    lines.push_back(String::from_utf8_lossy(&line).into_owned());
                }

                // ---------- ログイン ----------
                if !logged_in {
                    if !login_warned && login_started.elapsed() > Duration::from_secs(20) {
                        login_warned = true;
                        ctx.log(
                            "info",
                            "ログインが進みません。同じアカウントで他のプロセスが\
                             接続していないか確認してください (GGS は二重ログインを弾きます)",
                        );
                        ctx.notify(
                            "GGS: ログインが進みません",
                            "二重ログインの可能性があります",
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
                        send!(ctx, "tell /os rated +");
                        send!(ctx, "tell /os open 1");
                        send!(ctx, "chann + .chat");
                        pending.push("who".into());
                        send!(ctx, "tell /os who 8");
                        // GGS のレートプールは 8 (通常) と 8r (ランダム開局) の
                        // 2 つだけ。synchro は対局形式でありプールではない。
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
                        if reconnect {
                            pending.push("stored".into());
                            send!(ctx, "tell /os stored");
                        }
                        ctx.emit(true);
                    }
                    ctx.emit(false);
                    continue;
                }

                // ---------- 行処理 ----------
                while let Some(ln) = lines.pop_front() {
                    ctx.log("in", &ln);

                    // チャンネルチャット: ".chat name: text"
                    if let Some(rest) = ln.strip_prefix('.') {
                        if let Some((head, text)) = rest.split_once(": ") {
                            let mut it = head.split_whitespace();
                            if let (Some(chan), Some(from), None) =
                                (it.next(), it.next(), it.next())
                            {
                                let mut s = ctx.snap.lock().unwrap();
                                s.chat.push_back(ChatMsg {
                                    chan: format!(".{chan}"),
                                    from: from.to_string(),
                                    text: text.to_string(),
                                    at: now_secs(),
                                    thread: format!(".{chan}"),
                                });
                                while s.chat.len() > 300 {
                                    s.chat.pop_front();
                                }
                                drop(s);
                                ctx.dirty = true;
                                continue;
                            }
                        }
                    }
                    // ダイレクト tell: "name: text" (サーバー行と区別するため
                    // 名前が単純な英数のときだけ)
                    if !ln.starts_with(['/', '|', ':', ' ']) && ln != "READY" {
                        if let Some((name, text)) = ln.split_once(": ") {
                            if !name.is_empty()
                                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                                && name.chars().next().unwrap().is_ascii_alphabetic()
                            {
                                let mut s = ctx.snap.lock().unwrap();
                                s.chat.push_back(ChatMsg {
                                    chan: String::new(),
                                    from: name.to_string(),
                                    text: text.to_string(),
                                    at: now_secs(),
                                    thread: name.to_string(),
                                });
                                while s.chat.len() > 300 {
                                    s.chat.pop_front();
                                }
                                drop(s);
                                ctx.dirty = true;
                                continue;
                            }
                        }
                    }
                    // ブロック (盤面) の収集
                    if ln.starts_with("/os: update") || ln.starts_with("/os: join") {
                        in_block = true;
                        block.clear();
                        block.push(ln);
                        continue;
                    }
                    if in_block {
                        if ln == "READY" {
                            in_block = false;
                            handle_block(&mut ctx, &block, &login, &mut matches, |c| {
                                let c: String = c;
                                ctx_send(&mut writer, &c);
                            });
                        } else {
                            block.push(ln);
                        }
                        continue;
                    }

                    // 自動観戦キュー (検証用) を掃き出す
                    if !ctx.auto_watch.is_empty() {
                        for id in std::mem::take(&mut ctx.auto_watch) {
                            send!(ctx, format!("tell /os watch + {id}"));
                        }
                    }

                    // capture (who / top / finger / stored):
                    // ヘッダ行が来てから収集を始め、次の READY で確定する
                    if let Some((kind, buf)) = capture.as_mut() {
                        if ln == "READY" {
                            let kind = kind.clone();
                            let buf = buf.clone();
                            capture = None;
                            if kind == "stored" {
                                // 中断対局を探して自動再開
                                let mut ids = Vec::new();
                                for l in &buf {
                                    if let Some(rest) = l.strip_prefix('|') {
                                        let id = rest.split_whitespace().next().unwrap_or("");
                                        if id.starts_with('.') && rest.contains(&login) {
                                            ids.push(id.to_string());
                                        }
                                    }
                                }
                                if let Some(id) = ids.first().cloned() {
                                    ctx.log("info", &format!("中断対局 {id} を再開します"));
                                    send!(ctx, format!("tell /os ask {id}"));
                                }
                            } else {
                                finish_capture(&mut ctx, &kind, &buf, &login);
                            }
                        } else {
                            buf.push(ln.clone());
                        }
                        // capture 中も以降の共通処理は行う (offer 等を拾うため)
                    } else if let Some(pos) =
                        pending.iter().position(|k| capture_header_matches(k, &ln))
                    {
                        let kind = pending.remove(pos);
                        capture = Some((kind, vec![ln.clone()]));
                    }

                    // offer / match の増減
                    if let Some(rest) = ln.strip_prefix("/os: + ") {
                        let rest = rest.trim_start();
                        if let Some(mrest) = rest.strip_prefix("match ") {
                            let id = mrest.split_whitespace().next().unwrap_or("").to_string();
                            if !id.is_empty() {
                                let mine = mrest.contains(&login);
                                if mine {
                                    ctx.notify("GGS: 対局開始", mrest);
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
                            // 待機モード: 自分宛の申し込みを自動受諾
                            let s = ctx.snap.lock().unwrap();
                            let auto = s.standby.enabled && s.standby.auto_accept;
                            let in_match = !s.matches.is_empty();
                            let incoming = s.offers.last().map(|o| (o.incoming, o.id.clone()));
                            drop(s);
                            if let Some((true, id)) = incoming {
                                if auto && !in_match {
                                    ctx.log("info", &format!("待機モード: {id} を自動受諾"));
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
                                    ctx.notify("GGS: 対局の申し込み", &format!("{who} ({id})"));
                                }
                            }
                        }
                    } else if let Some(rest) = ln.strip_prefix("/os: - ") {
                        let rest = rest.trim_start();
                        if let Some(mrest) = rest.strip_prefix("match ") {
                            let was_mine = mrest.contains(&login);
                            {
                                // 進行中一覧・観戦盤から除去 (他人の対局も)
                                let id = mrest.split_whitespace().next().unwrap_or("").to_string();
                                let mut s = ctx.snap.lock().unwrap();
                                s.ongoing.retain(|o| o.id != id);
                                drop(s);
                                if !was_mine {
                                    drop_match(&mut matches, &id);
                                }
                            }
                            // 観戦盤から消えたことを画面に伝える。ここで
                            // 通知しないと、次に何か届くまで終わった対局が
                            // 残って見える。
                            sync_matches(&mut ctx, &matches);
                            ctx.emit(true);
                            handle_match_end(&mut ctx, mrest, &login, &mut matches);
                            if was_mine {
                                // レートが動いたはずなので who を取り直す
                                pending.push("who".into());
                                send!(ctx, "tell /os who 8");
                            }
                            // 待機モード: 次の対局を予約
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
                    } else if ln.starts_with("/os: ERR") {
                        ctx.log("info", &format!("サーバーエラー: {ln}"));
                    }
                }

                // ---------- 待機モードの自動申し込み ----------
                if let Some(t) = next_ask_at {
                    if Instant::now() >= t {
                        next_ask_at = None;
                        let s = ctx.snap.lock().unwrap();
                        let sb = s.standby.clone();
                        let in_match = !s.matches.is_empty();
                        let games = s.standby_stats.games;
                        drop(s);
                        if sb.enabled
                            && !in_match
                            && !sb.opponent.is_empty()
                            && (sb.max_games == 0 || games < sb.max_games)
                        {
                            ctx.log("info", &format!("待機モード: {} に申し込み", sb.opponent));
                            send!(
                                ctx,
                                format!("tell /os ask {} {} {}", sb.gtype, sb.time, sb.opponent)
                            );
                        }
                    }
                }

                ctx.emit(false);
            }
        }

        // 明示的な切断
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

fn apply_engine_cfg(ctx: &mut Ctx, depth: u32, solve: u8, band: u8, threads: usize) {
    ctx.engine_cfg.depth = depth;
    ctx.engine_cfg.solve_empties = solve;
    ctx.engine_cfg.band = band;
    ctx.engine_cfg.threads = threads;
    if let Some(e) = ctx.engine.as_mut() {
        e.set_levels(depth, solve, band);
    }
    let mut s = ctx.snap.lock().unwrap();
    s.engine.depth = depth;
    s.engine.solve = solve;
    s.engine.band = band;
    s.engine.threads = threads;
    drop(s);
    ctx.dirty = true;
}

/// update/join ブロックを解析して盤面状態を更新し、自分の手番なら思考して指す。
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
    // "/os: join .45.0 s8r14 K?" の 4 つ目が種別
    if let Some(t) = block[0].split_whitespace().nth(3) {
        if t.starts_with('8') || t.starts_with("s8") {
            m.gtype = t.to_string();
        }
    }
    let (rows_ok, turn) = apply_block(m, block, login);

    // スナップショット反映
    sync_matches(ctx, matches);
    ctx.emit(true);

    // ---- 自分の手番なら思考 ----
    let (auto, watch_an) = {
        let s = ctx.snap.lock().unwrap();
        (s.auto_play, s.watch_analysis)
    };
    let m = matches.get_mut(&mid).unwrap();
    if m.my_color.is_none() {
        // 観戦中の対局: 裏で解析して評価値と最善手を出す
        if watch_an && rows_ok && turn.is_some() {
            analyze_watch(ctx, &mid, matches);
        }
        return;
    }
    if !auto || !rows_ok || turn.is_none() || turn != m.my_color {
        return;
    }
    think_and_play(ctx, &mid, matches, send);
}

/// 観戦中の局面を解析する (エンジンの通常レベルより浅め・短時間で)。
fn analyze_watch(ctx: &mut Ctx, mid: &str, matches: &mut HashMap<String, MatchState>) {
    let m = matches.get_mut(mid).unwrap();
    let Some(board) = board_of(m, m.turn) else {
        return;
    };
    let bh = board.black.wrapping_mul(31).wrapping_add(board.white) ^ (m.turn as u64);
    if bh == m.watch_hash {
        return; // 同一局面は再解析しない
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
    // 観戦解析は自分の対局の思考を邪魔しない範囲で軽く
    engine.set_levels(base.0.min(14), base.1.min(20), 0);
    let mv = engine.choose(&board);
    engine.set_levels(base.0, base.1, base.2);

    let m = matches.get_mut(mid).unwrap();
    let v = if mv.value.is_finite() { mv.value } else { 0.0 };
    m.watch_eval = Some(if black_turn { v } else { -v }); // 黒視点へ
    m.watch_best = mv.pos.map(coord);
    m.watch_exact = mv.exact;
    sync_matches(ctx, matches);
    ctx.emit(true);
}

/// MatchState の盤面から、指定手番の Board を作る。
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

/// ブロックからプレイヤー・時計・盤面・着手履歴を MatchState へ反映する。
/// 戻り値は (盤面 8 行が揃ったか, 手番)。
fn apply_block(m: &mut MatchState, block: &[String], login: &str) -> (bool, Option<char>) {
    let mut rows: Vec<Vec<char>> = Vec::new();
    let mut boards: Vec<Vec<Vec<char>>> = Vec::new();
    let mut turns: Vec<char> = Vec::new();
    let mut turn: Option<char> = None;
    m.players.clear();
    for l in block {
        let b = l.strip_prefix('|').unwrap_or(l);
        // プレイヤー行: `name (rating color) clock`
        if let Some(open) = b.find('(') {
            let name = b[..open].trim();
            if !name.is_empty() && !name.contains(' ') && b[open..].contains(')') {
                let close = open + b[open..].find(')').unwrap();
                let inner = b[open + 1..close].trim();
                let color = inner.chars().last().unwrap_or(' ');
                if color == '*' || color == 'O' {
                    let rating = inner.trim_end_matches(['*', 'O']).trim().to_string();
                    let clock = b[close + 1..].trim().to_string();
                    let (main, _inc, ext) = parse_clock(&clock);
                    // join ブロックは開始時と現在で 2 組送ってくる。
                    // 同じ名前が来たら現在の情報で置き換える。
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
                        m.my_clock_secs = main;
                        m.my_ext = ext;
                    } else {
                        m.opp_name = name.to_string();
                        m.opp_rating = rating;
                        m.opp_clock = clock;
                        m.opp_secs = main;
                        m.opp_ext = ext;
                    }
                }
            }
        }
        let t = b.trim_start();
        // 盤面行
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
                // 8 行そろったら 1 枚として切り出す
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
        // 着手履歴行: `N: F5/...` または `N: F5//1.2`
        if let Some(colon) = t.find(':') {
            let (num, rest) = t.split_at(colon);
            if let Ok(n) = num.trim().parse::<u32>() {
                let mv = rest[1..]
                    .trim()
                    .split(['/', ' '])
                    .next()
                    .unwrap_or("")
                    .to_string();
                if ((2..=4).contains(&mv.len()) || mv.eq_ignore_ascii_case("pa")) && !mv.is_empty()
                {
                    m.moves.insert(n, mv);
                }
            }
        }
    }

    // GGS の盤は row-major (行 = rank)。file-major の index へ写像
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
    // 観戦の join ブロックには盤面が 2 枚入る (開始局面と現在局面)。
    // 自分の対局の update は 1 枚。どちらでも最後の 1 枚が現在の盤面。
    if let Some(last) = boards.last() {
        m.cells = to_cells(last);
    }
    // 2 枚あるなら 1 枚目が開始局面。抽選オープニングだと初期局面ではない
    // ので、棋譜を復元するために控えておく (一度掴んだら上書きしない)。
    if boards.len() >= 2 && m.start_cells.is_empty() {
        m.start_cells = to_cells(&boards[0]);
        m.start_turn = turns.first().copied().unwrap_or('*');
    }
    m.turn = turn.unwrap_or(' ');
    (!boards.is_empty(), turn)
}

/// 残り時間と残り手数から 1 手の探索設定を決める。
///
/// 固定閾値ではなく「残り時間 ÷ 自分の残り手数」で 1 手の予算を出す。
/// 自分が指す残り手数はおおよそ空きマスの半分 (パスがあるので下振れする)。
/// 予算に対する深さの対応は実測ベースのざっくりした階段で、
/// 深い設定ほど 1 手のコストが跳ねるため安全側に倒してある。
/// 戻り値は (中盤深さ, 完全読み開始の空き, 帯)。
fn time_budget(
    clock_secs: Option<u64>,
    ext_secs: u64,
    empties: u8,
    base: (u32, u8, u8),
) -> (u32, u8, u8) {
    let Some(secs) = clock_secs else {
        return (base.0, base.1, base.2);
    };
    // 本時間が尽きていればロスタイム勝負: 最速で指す
    if secs == 0 {
        return if ext_secs > 0 {
            (4, base.1.min(14), 0)
        } else {
            (2, base.1.min(10), 0)
        };
    }
    // 自分が指す残り手数 (最低 1)。終盤の完全読みは 1 手で全部読むので、
    // 読切に入る手前までを予算配分の対象にする。
    let my_moves = ((empties.saturating_sub(base.1) as f64 / 2.0).ceil() as u64).max(1);
    // 完全読み 1 回分を確保したうえで中盤に配る
    let reserve = 20u64.min(secs / 3);
    let budget = (secs.saturating_sub(reserve)) as f64 / my_moves as f64;

    // 1 手予算 → 深さ (帯は予算に余裕があるときだけ)
    let (d, band) = if budget >= 25.0 {
        (base.0, base.2)
    } else if budget >= 12.0 {
        (base.0.min(20), base.2.min(4))
    } else if budget >= 6.0 {
        (base.0.min(16), 0)
    } else if budget >= 3.0 {
        (base.0.min(12), 0)
    } else if budget >= 1.5 {
        (base.0.min(9), 0)
    } else if budget >= 0.6 {
        (base.0.min(6), 0)
    } else {
        (base.0.min(4), 0)
    };
    // 残りが極端に少ないときは完全読みの開始も遅らせる (読切自体が高いため)
    let solve = if secs < 20 {
        base.1.min(14)
    } else if secs < 60 {
        base.1.min(20)
    } else {
        base.1
    };
    (d.max(1), solve, band)
}

/// 自分の手番の局面でエンジンを回して着手を送る。
fn think_and_play(
    ctx: &mut Ctx,
    mid: &str,
    matches: &mut HashMap<String, MatchState>,
    mut send: impl FnMut(String),
) {
    let m = matches.get_mut(mid).unwrap();
    let Some(board) = board_of(m, m.my_color.unwrap_or(' ')) else {
        ctx.log("info", "盤面の解析に失敗しました");
        return;
    };
    let bh = board.black.wrapping_mul(31).wrapping_add(board.white);
    if bh == m.last_played_hash {
        return; // 同一局面への二重着手防止
    }
    if let Err(e) = ctx.ensure_engine() {
        ctx.log("info", &format!("エンジン初期化失敗: {e}"));
        return;
    }
    let clock_secs = m.my_clock_secs;
    let ext = m.my_ext.unwrap_or(0);
    let empties = board.empty_count();
    {
        let mut s = ctx.snap.lock().unwrap();
        s.thinking = Some(mid.to_string());
    }
    ctx.emit(true);

    let engine = ctx.engine.as_mut().unwrap();
    let base = (
        ctx.engine_cfg.depth,
        ctx.engine_cfg.solve_empties,
        ctx.engine_cfg.band,
    );
    let (d, solve, band) = time_budget(clock_secs, ext, empties, base);
    engine.set_levels(d, solve, band);
    let mv = engine.choose(&board);
    engine.set_levels(base.0, base.1, base.2);

    let mstr = match mv.pos {
        Some(p) => coord(p),
        None => "pa".to_string(),
    };
    send(format!("tell /os play {mid} {mstr}"));
    ctx.log("out", &format!("tell /os play {mid} {mstr}"));
    ctx.log(
        "info",
        &format!(
            "{mid} {mstr}: {} {:+.2}{}",
            if mv.from_book { "定石" } else { "探索" },
            mv.value,
            if mv.exact { " (完全読み)" } else { "" }
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

fn sync_matches(ctx: &mut Ctx, matches: &HashMap<String, MatchState>) {
    let mut view: Vec<MatchView> = matches
        .iter()
        .map(|(id, m)| MatchView {
            id: id.clone(),
            base: base_id(id),
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
            players: m.players.clone(),
            gtype: m.gtype.clone(),
            ggf: m.ggf(id, None),
            moves: m.moves.values().cloned().collect(),
            last_eval: m.last_eval,
            last_eval_exact: m.last_eval_exact,
            last_from_book: m.last_from_book,
            watch_eval: m.watch_eval,
            watch_best: m.watch_best.clone(),
            watch_exact: m.watch_exact,
            seen: m.seen,
        })
        .collect();
    view.sort_by(|a, b| a.id.cmp(&b.id));
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
    // 実形式: ".25 1720.0 kuroobi  15:00//02:00  8 R 1438.6 fly"
    // (先頭に並ぶ名前 = 申し込んだ側。R = rated)
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

fn handle_match_end(
    ctx: &mut Ctx,
    rest: &str,
    login: &str,
    matches: &mut HashMap<String, MatchState>,
) {
    // 実形式: ".13 1866 kuroobi 1411 fly 8 R +54.00  .82720"
    // (黒番側が先に並ぶ。スコアは黒視点の小数、末尾はアーカイブ番号)
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let id = toks.first().copied().unwrap_or("").to_string();
    if !rest.contains(login) {
        // 他人の対局の終了は無視 (offer 更新のみ)
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
    // synchro は親 ID で終局が来るので、`.N.0` / `.N.1` をまとめて回収する。
    // 棋譜は先に始まった方 (`.N.0`) を代表として残す。
    let mut dropped = drop_match(matches, &id);
    dropped.sort_by_key(|m| m.seen);
    let m = dropped.into_iter().next();
    let re = score.map(|s| format!("{s:+.2}"));
    let (kifu, ggf, opp, i_am_black) = match &m {
        Some(m) if m.my_color.is_some() => (
            m.kifu(),
            m.ggf(&id, re.as_deref()),
            m.opp_name.clone(),
            m.my_color == Some('*'),
        ),
        Some(m) => (
            m.kifu(),
            m.ggf(&id, re.as_deref()),
            m.opp_name.clone(),
            first_name == login,
        ),
        None => (
            String::new(),
            String::new(),
            String::new(),
            first_name == login,
        ),
    };
    let my_diff = score.map(|s| {
        let v = s.round() as i32;
        if i_am_black {
            v
        } else {
            -v
        }
    });
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
        // 実形式の末尾がアーカイブ番号: ".13 1866 kuroobi ... +54.00  .82720"
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
        my_rating: ctx.snap.lock().unwrap().my_rating,
        at: now,
    };
    append_history(&result);
    let mut s = ctx.snap.lock().unwrap();
    s.results.insert(0, result);
    s.results.truncate(200);
    s.thinking = None;
    // 待機モードの統計
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
        Some(d) if d > 0 => format!("勝ち +{d} (vs {opp2})", opp2 = opp_for_note),
        Some(d) if d < 0 => format!("負け {d} (vs {opp_for_note})"),
        Some(_) => format!("引き分け (vs {opp_for_note})"),
        None => rest.to_string(),
    };
    ctx.notify("GGS: 対局終了", &msg);
    ctx.log("info", &format!("対局終了: {rest}"));
    ctx.emit(true);
}

/// 応答本文の先頭行 (ヘッダ) かどうか。ACK の READY と区別するために使う。
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
    if kind == "who" {
        ln.starts_with("/os: who")
    } else if kind == "top" {
        ln.starts_with("/os: top")
    } else if kind == "stored" || kind == "stored_list" {
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
        // 例: ": finger" のエコー、または "login  : <name>" 行から始まる
        ln.starts_with(": finger") || ln.trim_start().starts_with("login")
    } else {
        false
    }
}

/// "1938.0@71.7=" / "1720.0@350.0" などから先頭のレート値を取り出す。
fn parse_rating_token(t: &str) -> Option<f32> {
    let head = t.split('@').next()?;
    head.parse::<f32>()
        .ok()
        .filter(|v| (100.0..4000.0).contains(v))
}

fn finish_capture(ctx: &mut Ctx, kind: &str, buf: &[String], login: &str) {
    if let Some(id) = kind.strip_prefix("look:") {
        // 実形式: `|1 (;GM[Othello]PC[GGS/os]...;)` (先頭の番号は連番)
        let ggf = buf
            .iter()
            .filter_map(|l| l.find("(;").map(|i| l[i..].to_string()))
            .find(|g| g.contains("GM[Othello]"));
        let mut s = ctx.snap.lock().unwrap();
        match ggf {
            Some(g) => {
                s.fetched_ggf = Some(FetchedGgf {
                    id: id.to_string(),
                    ggf: g,
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
                    error: if err.is_empty() {
                        "棋譜が見つかりません".into()
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
    if kind == "who" || kind == "top" {
        // who 実形式: `|Rhapsody + 1720.0@350.0 ->   +33.6 ...`
        // top 実形式: `|    2 kuroobi  2184.2@179.3=  ...` (先頭に順位)
        let mut users = Vec::new();
        let mut my_rating = None;
        for l in buf {
            let b = l.strip_prefix("/os: ").unwrap_or(l);
            let b = b.strip_prefix('|').unwrap_or(b);
            let mut toks: Vec<&str> = b.split_whitespace().collect();
            if kind == "top" {
                // 先頭の順位番号を捨てる
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
            let rating = toks.iter().skip(1).find_map(|t| parse_rating_token(t));
            if rating.is_none() {
                continue;
            }
            if name == login {
                my_rating = rating;
            }
            users.push(UserRow {
                name: name.to_string(),
                rating,
                raw: b.to_string(),
            });
        }
        // GGS の top はレート降順ではない (Glickman レーティングの偏差を
        // 加味した並び) ので、どちらもレート降順に揃える。GGS が付けた順位は
        // 使わない。
        users.sort_by(|a, b| {
            b.rating
                .unwrap_or(0.0)
                .partial_cmp(&a.rating.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut s = ctx.snap.lock().unwrap();
        if !users.is_empty() {
            if kind == "who" {
                s.users = users;
            } else {
                s.ranking = users;
            }
        }
        if let Some(r) = my_rating {
            s.my_rating = Some(r);
            // 直近の対局にレートが入っていなければ埋める (対局直後の who)
            if let Some(latest) = s.results.first_mut() {
                if latest.my_rating.is_none() {
                    latest.my_rating = Some(r);
                }
            }
        }
        drop(s);
        ctx.dirty = true;
    } else if kind == "match_list" {
        // 実形式: "| .70 2639 nyanyan  2606 egrcd  s8r14  R 0"
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
        // 観戦中フラグは維持する
        for o in list.iter_mut() {
            if s.ongoing.iter().any(|x| x.id == o.id && x.watching) {
                o.watching = true;
            }
        }
        let ids: Vec<String> = list.iter().map(|o| o.id.clone()).collect();
        s.ongoing = list;
        drop(s);
        ctx.dirty = true;
        // 検証用: KUROOBI_GGS_AUTOWATCH=auto なら、一覧が届いた時点で
        // 進行中の対局をまとめて観戦する。ID を外から渡すと取得から起動
        // までの間に対局が終わってしまい、観戦経路を確かめられないため。
        if std::env::var("KUROOBI_GGS_AUTOWATCH").as_deref() == Ok("auto") {
            for id in ids {
                ctx.auto_watch.push(id);
            }
        }
    } else if kind == "stored_list" {
        // 実形式: "|.82740  30 Jul 2026 22:30:00 kuroobi  Rhapsody s8r16:l"
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
        // 実形式:
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
        rows.reverse(); // 新しい順
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
        // 実形式: "|   17 kuroobi  2579.7@181.2=  21:50:36+@181.0  -0.6  0 2 3 <="
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
            // 末尾から 3 つの整数が 勝/分/敗
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
        // /os finger は "見出し : 値" の形。対局の設定だけを拾って、
        // サーバー全体の finger で作った項目に足す。
        let mut add: Vec<(String, String)> = Vec::new();
        for l in buf {
            // 応答の 1 行目はコマンドのエコー ("/os: finger <name>")
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
        // finger は "見出し : 値" 形式なので、見出しごとに分解して持つ
        let mut fields: Vec<(String, String)> = Vec::new();
        let mut raw: Vec<String> = Vec::new();
        for l in buf {
            let t = l.trim_end();
            if t.is_empty() || t == ":" {
                continue;
            }
            let t = t.strip_prefix(": ").unwrap_or(t);
            // パスワードは自分の finger にそのまま出るので保持しない
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

// ============================ テスト (実ログ由来の形式) ============================

#[cfg(test)]
mod tests {

    /// 実ログ: 他人の対局 (s8r14) を観戦し始めたときに届くブロック。
    /// 抽選された開始局面と現在局面の 2 枚が入っている。
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
        // 実ログ: |kuroobi  (1720.0 *) 14:59,0:0//02:00,0:0
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
    fn offer_real_format() {
        // 自分の申し込み (kuroobi が先頭) → incoming ではない
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
        // 相手からの申し込み (相手が先頭) → incoming
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
        // 実ログの join ブロック (抜粋)
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
        // 中央 4 石: D4=O E4=* D5=* E5=O (file-major index = file*8 + rank)
        let disc = |f: usize, r: usize| m.cells[f * 8 + r];
        assert_eq!(disc(3, 3), 2); // D4 = O
        assert_eq!(disc(4, 3), 1); // E4 = *
        assert_eq!(disc(3, 4), 1); // D5 = *
        assert_eq!(disc(4, 4), 2); // E5 = O
        assert_eq!(m.cells.iter().filter(|&&c| c != 0).count(), 4);
        // 着手履歴 (0 始まり、eval/time 付きの行も拾う)
        assert_eq!(m.moves.get(&0).map(|s| s.as_str()), Some("PASS"));
        assert_eq!(m.moves.get(&1).map(|s| s.as_str()), Some("E6"));
        assert_eq!(m.moves.get(&2).map(|s| s.as_str()), Some("f4"));
        assert_eq!(m.kifu(), "e6f4");
    }

    #[test]
    fn watch_join_block_two_boards() {
        // 実ログ: 他人の対局を観戦し始めたときに届くブロック。
        // 開始局面と現在局面の 2 枚が入っているので、現在の方を採る。
        let block: Vec<String> = WATCH_JOIN_BLOCK.iter().map(|s| s.to_string()).collect();
        let mut m = MatchState::new();
        let (rows_ok, turn) = apply_block(&mut m, &block, "kuroobi");
        assert!(rows_ok, "盤面が 2 枚でも読めること");
        assert_eq!(turn, Some('*'));
        // 自分は当事者ではない → 観戦として扱う
        assert_eq!(m.my_color, None);
        // 対局者は 2 人。時計行も 2 組来るが重複させない
        assert_eq!(m.players.len(), 2);
        assert_eq!(m.players[0].name, "nyanyan");
        assert_eq!(m.players[0].color, "black");
        assert_eq!(m.players[1].name, "egrcd");
        assert_eq!(m.players[1].color, "white");
        // 採用するのは現在の盤面。この対局は rand オープニング (s8r14) で、
        // 1 枚目が抽選された開始局面 (14 石)、2 枚目がそこから 24 手進んだ
        // 現在局面 (38 石)。開始局面を掴んでいたら 14 になる。
        assert_eq!(m.moves.len(), 24);
        let stones = m.cells.iter().filter(|&&c| c != 0).count();
        assert_eq!(stones, 38, "現在局面の石数");

        // 1 枚目は抽選された開始局面。棋譜を復元するために控えておく。
        assert_eq!(m.start_cells.iter().filter(|&&c| c != 0).count(), 14);
        let start = m.start_string();
        assert_eq!(start.len(), 66, "盤面 64 + 空白 + 手番");
        assert!(start.ends_with(" X"), "開始局面の手番は黒");
        // kuroobi 側が同じ形式で読めること
        let board = kuroobi::Board::from_string(&start).expect("盤面文字列として読める");
        assert_eq!((board.black | board.white).count_ones(), 14);
    }

    #[test]
    fn ggf_round_trips_a_drawn_opening() {
        // 観戦した対局を GGF にして、kuroobi 側で現在局面まで再生できること。
        let block: Vec<String> = WATCH_JOIN_BLOCK.iter().map(|s| s.to_string()).collect();
        let mut m = MatchState::new();
        apply_block(&mut m, &block, "kuroobi");

        let ggf = m.ggf(".45.0", Some("+4.00"));
        assert!(ggf.starts_with("(;GM[Othello]"));
        assert!(ggf.ends_with(";)"));
        assert!(ggf.contains("PB[nyanyan]"), "黒は nyanyan");
        assert!(ggf.contains("PW[egrcd]"), "白は egrcd");
        assert!(ggf.contains("RE[+4.00]"));
        // BO は '*' と 'O'、64 マス + 手番
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
        // 着手は 24 手、黒から交互 (PB/RB/PW/RW は BO より前なので混ざらない)
        let moves_part = ggf.split_once("BO[").unwrap().1;
        assert_eq!(
            moves_part.matches("B[").count() + moves_part.matches("W[").count(),
            24
        );
        assert!(ggf.contains("B[F3]"), "1 手目は黒の F3");
        assert!(ggf.contains("W[D2]"), "2 手目は白の D2");

        // kuroobi 側の盤面文字列として読めること
        let start = bo.replace('*', "X");
        let board = kuroobi::Board::from_string(&start).expect("BO が読める");
        assert_eq!((board.black | board.white).count_ones(), 14);

        // GGF から復元した局面が、観戦していた現在局面と一致すること
        let kifu: String = m.kifu();
        let replayed =
            kuroobi::game::Reversi::from_kifu_with_start(&start, &kifu).expect("再生できる");
        for (i, &want) in m.cells.iter().enumerate() {
            let bit = 1u64 << i;
            let got = if replayed.board.black & bit != 0 {
                1
            } else if replayed.board.white & bit != 0 {
                2
            } else {
                0
            };
            assert_eq!(got, want, "マス {i} が一致しない");
        }
    }

    #[test]
    fn watch_synchro_keeps_two_boards_apart() {
        // synchro (s8r14) を観戦すると .N.0 と .N.1 の 2 局が同時に流れる。
        // 同じ対局者・同じ手数でも、盤面は別々に持たなければならない。
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
        assert_eq!(matches.len(), 2, "2 局が別々に入る");
        // C6 は file=2, rank=5 / E6 は file=4, rank=5
        assert_eq!(matches[".56.0"].cells[2 * 8 + 5], 1);
        assert_eq!(matches[".56.0"].cells[4 * 8 + 5], 0);
        assert_eq!(matches[".56.1"].cells[2 * 8 + 5], 0);
        assert_eq!(matches[".56.1"].cells[4 * 8 + 5], 1);
        // どちらも観戦 (自分は当事者ではない)
        assert!(matches.values().all(|m| m.my_color.is_none()));
        // base_id はまとめるが、キーは分かれたまま
        assert_eq!(base_id(".56.0"), ".56");
        assert_eq!(base_id(".56.1"), ".56");
    }

    #[test]
    fn synchro_end_clears_both_boards() {
        // 実ログの終局メッセージは親 ID で来る:
        //   /os: - match .4 2617 nyanyan 2628 egrcd s8r14 R +0.00  .83128
        // 盤面は .4.0 / .4.1 なので、完全一致だけで消すと残ってしまう。
        let mut matches: HashMap<String, MatchState> = HashMap::new();
        matches.insert(".4.0".into(), MatchState::new());
        matches.insert(".4.1".into(), MatchState::new());
        matches.insert(".9".into(), MatchState::new());
        let dropped = drop_match(&mut matches, ".4");
        assert_eq!(dropped.len(), 2, "synchro の 2 局とも回収する");
        assert!(!matches.contains_key(".4.0"));
        assert!(!matches.contains_key(".4.1"));
        assert!(matches.contains_key(".9"), "無関係な対局は残す");

        // 非 synchro (完全一致) も従来どおり消える
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
        // .4 自身と .4.0 だけが対象。.40 / .44.1 は別の対局
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
        assert_eq!(parse_rating_token("+33.6"), None); // 変動値は範囲外
        assert_eq!(parse_rating_token("71.7="), None);
    }
}

#[cfg(test)]
mod budget_tests {
    use super::time_budget;

    const BASE: (u32, u8, u8) = (22, 26, 6);

    #[test]
    fn plenty_of_time_uses_full_strength() {
        // 15 分・空き 50 → 1 手あたり十分 → 満額
        assert_eq!(time_budget(Some(900), 120, 50, BASE), (22, 26, 6));
    }

    #[test]
    fn shrinks_as_clock_drains() {
        // 残り 60 秒・空き 40 (自分の残り 7 手) → 中盤を絞る
        let (d, _, band) = time_budget(Some(60), 120, 40, BASE);
        assert!(d < 22, "深さが絞られる: {d}");
        assert_eq!(band, 0, "帯は落とす");
        // さらに少ないほど浅くなる (単調)
        let (d2, _, _) = time_budget(Some(20), 120, 40, BASE);
        assert!(d2 <= d, "{d2} <= {d}");
        let (d3, _, _) = time_budget(Some(5), 120, 40, BASE);
        assert!(d3 <= d2, "{d3} <= {d2}");
    }

    #[test]
    fn endgame_gets_the_whole_clock() {
        // 空き 26 = 読切域。残り手数の分母が小さいので深さは満額のまま
        let (d, solve, _) = time_budget(Some(300), 120, 26, BASE);
        assert_eq!(d, 22);
        assert_eq!(solve, 26);
    }

    #[test]
    fn main_time_exhausted_uses_fastest() {
        // 本時間 0・ロスタイムあり → 最速設定
        let (d, solve, band) = time_budget(Some(0), 120, 30, BASE);
        assert_eq!((d, band), (4, 0));
        assert!(solve <= 14);
        // ロスタイムも無ければさらに最速
        let (d2, _, _) = time_budget(Some(0), 0, 30, BASE);
        assert!(d2 < d);
    }

    #[test]
    fn unknown_clock_keeps_base() {
        assert_eq!(time_budget(None, 0, 40, BASE), BASE);
    }
}
