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
    /// 終局した対局を一覧から閉じる (進行中のものは閉じない)。
    CloseMatch(String),
    Raw(String),
    Ask {
        gtype: String,
        time: String,
        opponent: String,
        /// レート戦にするか。GGS の `/os` ではレート有無は**アカウント単位の
        /// 設定**なので、申し込みの直前に毎回 `/os rated +|-` を送って
        /// 揃える。送らずに済ませると、前回の申し込みの設定が残る。
        rated: bool,
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
        /// 相手の手番中に先読みするか。
        ponder: bool,
    },
    /// **全体設定のスレッド数が変わった。** GGS 専用の欄は持たない —
    /// 別々に持つと読切速度の較正が 2 つ要り、片方が未較正のまま
    /// 気づかず対局に入る (`resources.conf` の `nps.<スレッド数>`)。
    ReloadThreads,
    /// 持ち時間の使い方 (配り方・1 手の上限・予備)。
    SetPacing {
        pace: String,
        max_move_secs: u64,
        reserve_secs: u64,
        /// 持ち時間をどれだけ攻めて使うか (`timectl::Situation::budget_use`)。
        budget_use: f64,
    },
    SetAutoPlay(bool),
    SetWatchAnalysis(bool),
    /// 定石 book を使うか。研究や検証で自力の手を見たいときに切る。
    SetUseBook(bool),
    /// 終わった対局を定石の学習に取り込むか。
    SetLearn(bool),
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
    /// 待機モードから申し込むときレート戦にするか。
    pub rated: bool,
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
    /// レートの偏差。`/os top` は返すが `/os who` は返さない。
    pub dev: Option<f32>,
    pub raw: String,
}

/// `tell` を付けずに送る命令。サーバーはこれらに「命令名: いまの値」で
/// 返すので、**ダイレクト tell と見分けが付かない**。送る側で数え上げる。
const BARE_CMDS: [&str; 2] = ["verbose", "chann"];

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

/// **レート戦を禁じる。** `KUROOBI_NO_RATED=1` で起動すると、申し込みも
/// 待機モードも必ず `rated -` になり、画面からも「する」を選べなくなる。
///
/// **人が押し間違えないための蓋ではなく、自動で動かすときの蓋。**
/// 画面確認を自動で回している最中に、既定が「する」のまま待機モードが
/// 始まると本当にレート戦が成立してしまう。**画面側の見た目だけでは
/// 足りない**ので、サーバーへ送る直前でも潰す。
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
    /// 終局したか。終わっても一覧には残す (手動で閉じる)。
    pub over: bool,
    /// 終局の結果 (石差の文字列)。
    pub result: String,
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
    /// **自分がロスタイムに入ったか。** 入ったらこの対局は時間切れ負けが
    /// 確定していて、残るのは全滅 (`timeout_hard`) を避けることだけ。
    /// 一度立てたら下ろさない (サーバー側も戻さない)。
    pub in_overtime: bool,
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
    /// 直近に取れた自分のレート。**画面には出さない** (プールごとの
    /// `my_ranks` が出す) が、終わった対局に後追いで刻むために持つ。
    #[serde(skip)]
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
    /// 画面に出す一言 (観戦に失敗した等)。表示したら消える前提の短い文。
    pub notice: String,
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

/// 画面確認用の作り物のスナップショット (`KUROOBI_GGS_DEMO=1`)。
///
/// **GGS の画面は繋がないと何も出ない。** ロビー・プレイヤー・対局結果は
/// 相手が要るので、寸法も配色も一度も実機で確かめられていなかった
/// (台帳の「未検証」の大半がこれ)。繋ぐには許可が要り、しかも相手が
/// 居るとは限らないので、**中身だけを作って画面を描かせる**口を用意する。
///
/// 打つ・申し込むといった操作は繋がっていないので通らない。**見た目を
/// 確かめるためだけのもの。**
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
        raw: format!("{n} {r}@{d}"),
    })
    .collect();
    /* **頁送り (規則 74) を出すには 25 人を超えないといけない。**
    実サーバーは接続中が 10 件ほどしか居らず、一度も出せていなかった */
    for i in 0..26 {
        let r = 1900.0 - i as f32 * 21.0;
        users.push(UserRow {
            name: format!("player{:02}", i + 1),
            rating: Some(r),
            dev: Some(40.0 + i as f32),
            raw: format!("player{:02} {r}@40", i + 1),
        });
    }
    s.users = users;
    s.ranking = s.users.clone();
    s.ongoing = vec![
        OngoingView {
            id: ".71.0".into(),
            raw: "tamaki 対 edax-bot".into(),
            watching: true,
            names: vec!["tamaki".into(), "edax-bot".into()],
            ratings: vec!["2011.4".into(), "2280.1".into()],
            gtype: "s8r14".into(),
            mine: false,
        },
        OngoingView {
            id: ".72.0".into(),
            raw: "nara 対 kei".into(),
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
    /* ---- 同期対局の 2 面 (規則 28)。**面ごとに自分の色が逆**なので、
    見出しが無いと並んだ 2 枚を見分けられない ---- */
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
    /* レートは 1 局ごとに動かす — **同じ値を並べると推移の線が平らになり、
    グラフが描けているのか壊れているのか分からない**。形式も入れる
    (空だと「?」と出る) */
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
        /* **形式は `base` の頭から取り出される** (`base.split('.')[0]`)。
        番号だけを入れると画面に「?」と出る */
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
            text: "接続しました (skatgame.net:5000)".into(),
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
    /* finger の項目。**条件式の木 (`FormulaView`) はこれが無いと描けない** —
    待機モードの「申し込みの条件」と、設定の「申し込みの扱い」の両方が
    ここから来る。相手の名刺 (プロフィール) も同じ */
    let finger = |name: &str, accept: &str| FingerInfo {
        name: name.into(),
        fields: vec![
            ("open".into(), "1".into()),
            ("rated".into(), "+".into()),
            ("accept".into(), accept.into()),
            ("decline".into(), "rated&or>2400".into()),
            ("play".into(), "0".into()),
            ("stored (+)".into(), "0".into()),
            ("info".into(), "画面確認用の作り物".into()),
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
    /// 終わった対局を定石の学習に取り込むか。
    pub learn: bool,
    /// 相手の手番中に先読みするか。**「深さ固定」では効かない** —
    /// 本番の探索がどのみち最後まで走るので、先に読んでも得るものが無い。
    pub ponder: bool,
    /// 持ち時間の配り方 ("fast" 終盤に残す / "depth" 深さ固定)。
    ///
    /// **"slow" と "even" は落とした。** 自己対局で "slow" は 3 秒・8 秒の
    /// 対局で勝率 0.0% (石差 −34)、"fast" は全条件で "even" に劣らなかった。
    /// 古い設定に残っていても `Pace::parse` が既定 ("fast") へ倒す。
    pub pace: String,
    /// 1 手に使う上限 (秒)。0 で上限なし。
    pub max_move_secs: u64,
    /// 読み切り用に取っておく秒数。
    pub reserve_secs: u64,
    /// **持ち時間をどれだけ攻めて使うか。** 1.0 で「配分どおり」。
    ///
    /// 反復深化は期限まで粘らない (実測で 47%) ので、1.0 だと配ったぶんの
    /// 半分しか使わない。既定 2.5 は実測 15 分の対局で使用率 45% → 84%。
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

// ============================ セッション ============================

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
    /* `KUROOBI_SESSION_LOCK` で差し替えられる。**このロックは 1 プロセス
    しか GGS に繋がせない** ので、既定のままでは 2 つの実体を別々の
    アカウントで同時に動かせない (後から起動したほうが黙って見送る)。
    アカウントが違うなら二重ログインにはならないので、分けてよい。
    キーチェーンの `KUROOBI_KEYCHAIN_SERVICE` と対で使う。 */
    if let Ok(p) = std::env::var("KUROOBI_SESSION_LOCK") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::temp_dir().join("kuroobi_ggs.pid")
}

/// 別プロセスがセッションを掴んでいるか (ロックは取らない)。起動時の
/// 自動ログインが、通知を出さずに見送るための照会用。
pub fn session_locked_by_other() -> bool {
    let Ok(s) = std::fs::read_to_string(session_lock_path()) else {
        return false;
    };
    match s.trim().parse::<i32>() {
        Ok(pid) => pid != std::process::id() as i32 && unsafe { libc::kill(pid, 0) } == 0,
        Err(_) => false,
    }
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
    /// この手番について報せたか。1 手のあいだに update が何度も来るので、
    /// これが無いと同じ手番で何度も鳴る。
    told_turn: bool,
    my_clock: String,
    my_clock_secs: Option<u64>,
    my_ext: Option<u64>,
    /// ロスタイムに入ったか (`MatchView::in_overtime` を参照)。
    in_overtime: bool,
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
    /// 終局したか。終わっても一覧からは消さない (終局後の盤面から
    /// 棋譜を見たり検討へ送ったりしたいため)。閉じるのは手動。
    over: bool,
    /// 終局の結果 ("+12.00" など)。表示用。
    result: String,
}

impl MatchState {
    /// 履歴と学習へ渡すための写し。終局後も一覧に本体を残すので、
    /// 取り出す側には複製を渡す。
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
            watch_eval: self.watch_eval,
            watch_best: self.watch_best.clone(),
            watch_exact: self.watch_exact,
            watch_hash: self.watch_hash,
            last_played_hash: self.last_played_hash,
            seen: self.seen,
            over: self.over,
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
            over: false,
            result: String::new(),
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
/// **終局行の石差を自分視点に直す。**
///
///     - match .48 2639 kuroobi 2644 Rhapsody s8r16 R +2.00
///                    ~~~~~~~ この人から見て +2
///
/// 以前は自分の色 (`my_color == '*'`) で符号を決めていた。**同期対局では
/// 面ごとに色が逆**なので、どちらの面を代表に選んだかで勝敗が反転する。
/// 代表は「先に見た面」なので、届く順で結果が変わっていた。
///
/// 実際にレート戦の 1 局目 (+2.00 の勝ち) を「1 敗 −2 石差」と表示した。
/// サーバーのレートは +85.3 動いており、画面だけが逆を向いていた。
fn my_stone_diff(score: f32, first_name: &str, login: &str) -> i32 {
    let v = score.round() as i32;
    if first_name == login {
        v
    } else {
        -v
    }
}

/// **同期対局の鏡像から、相手が既に指した手を借りる。**
///
/// `s8r16` は同じ開局を先後入れ替えて 2 面同時に打つので、2 面が同じ手順を
/// たどっている間は**同一局面**になる。手番は原理的にずれるため、片方で
/// 相手が指した手は、もう片方でこちらが指す局面の候補そのものになる。
///
/// **相手はこれをやっている。** 実測 (3 局) では、相手はこちらの着手を待って
/// から同じ手を指し続け、より良い手を見つけたところで初めて分岐した。持ち時間
/// の 7〜9 割を最初の 10 手に投じていたのは、この「待つ時間」だった。
///
/// こちら側は相手より速く指すため、鏡像が先に埋まるのは自分の手番の
/// **29%** (24 手中 7 手) に留まる。それでも只で手に入る情報なので使う。
///
/// **手順が分岐したら使わない。** 分岐後は別の局面なので、借りた手は
/// 合法とも限らない。
fn mirror_hint(matches: &HashMap<String, MatchState>, mid: &str) -> Option<Position> {
    let (base, side) = mid.rsplit_once('.')?;
    let other = format!("{base}.{}", if side == "0" { "1" } else { "0" });
    let me = matches.get(mid)?;
    let you = matches.get(&other)?;
    // 次に指す手数 = こちらの最大手数 + 1
    let n = me.moves.keys().copied().max().unwrap_or(0) + 1;
    // ここまでの手順が完全に一致している間だけ鏡像が成り立つ
    if (1..n).any(|i| me.moves.get(&i) != you.moves.get(&i)) {
        return None;
    }
    // 相手の面が既にその手数を埋めていれば借りられる
    let mv = you.moves.get(&n)?;
    let b = mv.as_bytes();
    if b.len() != 2 {
        return None;
    }
    let file = b[0].to_ascii_lowercase().wrapping_sub(b'a');
    let rank = b[1].wrapping_sub(b'1');
    Position::from_file_rank(file, rank)
}

/// 相手からの待った (`undo`) / 中断 (`abort`) の申し出。
struct Request {
    verb: &'static str,
    id: String,
    who: String,
}

/// `/os: undo .24 htz is asking` を読む。
///
/// **書式は実サーバーで採った。** 資料には `undo [.match]` という送る側の
/// 形しか無く、届く側の形は書かれていない。自分が出した申し出も同じ形で
/// 返ってくるので、名前で弾く。
fn parse_request(ln: &str, login: &str) -> Option<Request> {
    let rest = ln.strip_prefix("/os: ")?;
    let verb = ["undo", "abort"]
        .into_iter()
        .find(|v| rest.starts_with(&format!("{v} ")))?;
    let mut it = rest[verb.len() + 1..].split_whitespace();
    let id = it.next()?;
    let who = it.next()?;
    // 末尾は "is asking"。違う形なら知らない通知として見送る
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
        /* **負の残り時間は 0 として読む。** 使い切った直後に `-0:01` の
        ような値が来ることがある。数字始まりだけを受けていると解析に失敗し、
        「時間制でない対局」と同じ `None` に落ちる — その扱いは**期限なし**
        なので、いちばん時間の無い場面でいちばん長く考えることになる。 */
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

/// 学習 (実戦の取り込み) の評価深さ。「実戦手以外の合法手の最善」を
/// 局面ごとに測るので、実戦 (22) より浅い速報値にしてある。1 回の
/// 呼び出しは 1 評価だけなので、対局の合間に回しても応答を壊さない。
/// ローカル対局の取り込み (main.rs) も同じ深さを使う。
pub(crate) const LEARN_DEPTH: u32 = 18;

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

/// 終局しても一覧からは消さず、終わった印だけ付ける。終局後の盤面から
/// 棋譜を取り出したり検討へ送ったりできるようにするため。閉じるのは手動。
/// 戻り値は終局した対局の写し (履歴と学習に使う)。
fn finish_match(
    matches: &mut HashMap<String, MatchState>,
    id: &str,
    result: &str,
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
            if !result.is_empty() {
                m.result = result.to_string();
            }
            out.push(m.snapshot());
        }
    }
    out
}

fn coord(p: Position) -> String {
    let f = (b'A' + p.index() / 8) as char;
    let r = (b'1' + p.index() % 8) as char;
    format!("{f}{r}")
}

/// **1 局ぶんの探索を持つワーカー。**
///
/// 同期対局は 2 局が同時に進む。エンジンが 1 つだと、片方を読んでいる間
/// もう片方は何もできない — 本探索なら時計を捨てているのと同じで、相手の
/// 手番なら先読みの機会を捨てている (先読みは実測で +1.6〜1.8 段)。
///
/// **エンジンは使い回す。捨てない。** `Engine::new` は置換表を `Box::leak`
/// で `'static` にするので、対局のたびに作り直すと解放されないまま積み上がる
/// (1 つ 167 MB)。対局が終わっても割り当てを外すだけにして、次の対局で
/// 同じワーカーを使う。
struct EngineWorker {
    tx: std::sync::mpsc::Sender<Job>,
    rx: std::sync::mpsc::Receiver<Done>,
    stop: kuroobi::midgame::StopHandle,
    /// いま探索中か (結果を受け取るまで次を投げない)。
    busy: bool,
    /// 割り当てている対局。無ければ空き。
    mid: Option<String>,
    /// 投げた探索の局面ハッシュ (返ってきたときに二重着手を防ぐ印を書くため)。
    pending_hash: u64,
    /// **まだ投げられていない本探索。** 塞がっている間に自分の手番が来たら
    /// ここへ控え、空いた瞬間に投げる。
    pending: Option<Pending>,
    /// いま走らせているのが先読みか (本探索が来たら捨てる)。
    pondering: bool,
    /// 本探索を投げた時刻と、そのとき渡した期限。**返ってこないことを
    /// 検出する**ために持つ (期限を大きく過ぎたら停止を立てて拾い直す)。
    sent_at: Option<(Instant, Duration)>,
    /// 停止を立てた時刻。**返らないワーカーを見捨てる判断に使う。**
    stopped_at: Option<Instant>,
}

/// 停止を立ててからこれだけ待っても返らなければ、そのワーカーを捨てる。
///
/// 席は 2 つしかない。1 つ失うと同期対局が片肺になり、2 つ失うと対局が
/// 止まる。**起きるべきでない経路だが、起きたときの損が大きすぎる。**
const WORKER_GIVE_UP: Duration = Duration::from_secs(10);

/// 投げ待ちの本探索。
struct Pending {
    board: Board,
    levels: (u32, u8, u8),
    cap: Option<Duration>,
    hash: u64,
    /// 鏡像から借りた手 (`mirror_hint`)。並べ替えの先頭に置くだけ。
    hint: Option<Position>,
}

/// ワーカーへの指示。
enum Job {
    /// 本探索。期限まで読んで着手を返す。
    Think {
        board: Board,
        levels: (u32, u8, u8),
        cap: Option<Duration>,
        /// 鏡像から借りた手 (`mirror_hint`)。並べ替えの先頭に置くだけで、
        /// 打ち切りには使われない。
        hint: Option<Position>,
    },
    /// 先読み。**自分の時計は減らない**ので、空いていれば常に投げてよい。
    /// 1 回で読むのは `slice` だけ (刻んで投げ直す)。
    Ponder {
        board: Board,
        slice: Duration,
    },
    SetUseBook(bool),
    SetThreads(usize),
}

/// ワーカーからの返事。
///
/// **学習の取り込みと観戦の解析はワーカーへ載せない。** どちらも対局が
/// 空いているときだけ動く仕事で、従来どおり `Ctx.engine` が受け持つ。
/// 対局を並行にすることと、余った時間の使い道を変えることは別の話。
enum Done {
    Moved(Box<kuroobi::engine::MoveEval>),
    Pondered,
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
    /// 終わった対局の取り込み (学習) の残り。対局の合間に 1 探索ずつ進める。
    /// 取り込み待ちの対局。控えに書く材料 (`LearnEntry`) を一緒に持つ —
    /// 終わってから作ろうとしても、そのころには対局の記録が消えている。
    learn_jobs: VecDeque<(String, kuroobi::learn::BackupJob, crate::LearnEntry)>,
    /// ローカル側の探索の停止ハンドル (GGS の自分の対局を最優先にするため、
    /// 対局が始まったらローカルの探索を止める)。
    local_stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    /// ローカル側の稼働記録 (何か走っているときだけ通知を出す)。
    local_activity: Arc<Mutex<crate::Activity>>,
    /// 観戦を頼んだ対局と期限。盤面が届かないまま過ぎたら、その対局は
    /// もう終わっている (GGS は watch の失敗を返さないことがある)。
    pending_watch: HashMap<String, Instant>,
    /// 持ち時間の配り方と上限 (画面から変えられる)。
    engine_cfg_pace: String,
    engine_cfg_max_move: u64,
    engine_cfg_reserve: u64,
    engine_cfg_budget_use: f64,
    /// 相手の手番中に先読みするか (画面から変えられる)。
    ///
    /// **時間制限があるときにしか効かない。** `pace` が「深さ固定」なら
    /// 探索はどのみち最後まで走るので、先に読んでも得るものが無い。
    engine_cfg_ponder: bool,
    /// 先読みの相手。**相手の手番になった対局の番号**を入れておき、
    /// 受信の合間に少しずつ読む。自分の手番に戻したら空にする。
    ///
    /// **ワーカーを使うときは見ない** (対局ごとに割り当てたワーカーが
    /// それぞれ先読みする)。ワーカーを作れなかったときの退路として残す。
    ponder_at: Option<String>,
    /// 対局ごとの探索ワーカー。**同期対局は 2 局が同時に進む**ので、
    /// 1 つのエンジンを取り合うと片方は必ず待たされる。
    ///
    /// プールとして持ち、対局が終わっても捨てない (置換表のリークを避ける)。
    workers: Vec<EngineWorker>,
    /// ワーカー 1 つあたりのスレッド数 (`share_threads` が決める)。
    /// 較正した nps を引くのに要る。
    worker_threads: usize,
}

/// ワーカーの上限。
///
/// 同期対局は 2 局なのでふつうは 2 で足りる。1 つ 167 MB (既定の置換表)
/// なので、増やすほどメモリを食う。超える対局は既存のワーカーを共有する
/// (従来どおり待たされるだけで、壊れはしない)。
const MAX_WORKERS: usize = 2;

impl Ctx {
    /// `mid` の探索を受け持つワーカーの番号。無ければ用意する。
    ///
    /// 空いているワーカーを再利用し、無ければ上限まで新しく立てる。上限に
    /// 達していたら**いちばん古い割り当てを奪う** (共有すると待たされるが、
    /// 壊れはしない)。
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
                    // **スレッドを配り直す。** 2 つのエンジンが同時に走るので、
                    // 片方が全部使うと取り合いになる
                    self.share_threads();
                    return Some(self.workers.len() - 1);
                }
                Err(e) => {
                    self.log("info", &format!("探索ワーカーを作れません: {e}"));
                    return None;
                }
            }
        }
        /* 上限に達した。**空いているワーカーを流用してはいけない** —
        `pending` は 1 つしか持てないので、別の対局の手を上書きして消す。
        消された対局は指されないまま時計だけ減り、時間切れになる
        (実戦で踏んだ: 2 局同時に投げて 1 局しか返らなかった)。

        **終わった対局を掴んだままのワーカーがあれば奪う。** それも無ければ
        諦めて従来の同期経路へ落とす (待たされるが、指されないよりよい)。 */
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

    /// ワーカーの数でスレッドを分ける。
    ///
    /// **同時に走るので足し算で決まる。** 1 つが全部使うと、もう 1 つが
    /// 立ち上がった瞬間に取り合いになる。較正 (`nps.<スレッド数>`) も
    /// 分けた後の数で引く必要があるので、その数を控えておく。
    fn share_threads(&mut self) {
        let total = resolve_threads(self.engine_cfg.threads);
        /* **割り当て中の数で割る。ワーカーの数ではない。** ワーカーは対局が
        終わっても捨てない (置換表を抱えているため) ので、`workers.len()` で
        割ると 2 局を戦った後は単独対局でも半分しか使わなくなる。 */
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

    /// 対局が終わったらワーカーの割り当てを外す (エンジンは残す)。
    ///
    /// **外したらスレッドを配り直す。** 残った対局が全部使えるようにする。
    fn release_worker(&mut self, mid: &str) {
        let mut hit = false;
        for w in &mut self.workers {
            if w.mid.as_deref() == Some(mid) {
                w.mid = None;
                /* **走っている探索と控えを捨てる。** 終わった対局の手を
                指しても `ERR match not found` になるだけで、その間ワーカー
                が塞がって別の対局が指されない。 */
                w.pending = None;
                if w.busy {
                    w.stop.stop();
                }
                /* **前の対局の印を残さない。** ワーカーは使い回すので、
                消さないと次の対局へそのまま持ち越す。

                - `sent_at`: 見張りが「期限を大きく超えた」と誤って言い、
                  こちらが**わざと止めた**探索を事故として扱う
                - `stopped_at`: 止まるのを待つ時計。残っていると、次の
                  対局に割り当てた直後のワーカーを見捨てて作り直しかねない
                  (置換表を捨てて数百 ms 払う)
                - `pending_hash`: 「もう読んでいる局面か」の判定に使う。
                  次の対局の初手とたまたま一致すると**投げずに素通り**する */
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
    /// エンジンを 1 つ抱えたスレッドを立てる。**重みの読み込みが入るので
    /// 数百 ms かかる。** 呼ぶのは対局が始まるときで、以後は使い回す。
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
                        let mv = engine.choose_within(&board, dl);
                        engine.set_levels(base.0, base.1, base.2);
                        if dtx.send(Done::Moved(Box::new(mv))).is_err() {
                            return;
                        }
                    }
                    Job::Ponder { board, slice } => {
                        engine.ponder(&board, Instant::now() + slice);
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
        })
    }

    /// 指示を投げる。**返事を待たない** — 待つとこの構造の意味が無い。
    fn send(&mut self, job: Job) {
        let counts = matches!(job, Job::Think { .. } | Job::Ponder { .. });
        if self.tx.send(job).is_ok() && counts {
            self.busy = true;
        }
    }
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
        /* 診断: 画面のログは 600 行で丸めるので、切り分け用に全量を残す。
        **実戦でしか出ない不具合を追うので release でも要る** —
        `KUROOBI_GGS_WIRE=<path>` を渡したときだけ書く。 */
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
        // emit の成否は下の診断ログでだけ見る (リリースでは未使用)
        let _emit_ok = self.app.emit("ggs", &s).is_ok();
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
            // 0 は「自動」の印なので、エンジンを作る直前に実数へ直す
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
            // **全体設定 (resources.conf の threads) を使う。** GGS 専用の
            // 欄は持たない — 別々だと較正が 2 つ要る
            threads: resources().threads.unwrap_or_else(|| resolve_threads(0)),
            // 置換表の大きさも全体設定から。**起動時にしか効かない**
            // (中身は `Box::leak` で `'static`。作り直すと積み上がる)
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
        // エンジンは対局や観戦解析が始まるまで作らない (重みの読み込みと
        // 置換表の確保が高いので、接続しただけでは持たない)。定石があるかを
        // エンジン生成まで分からないままにすると、繋いだ直後の設定画面が
        // 「ファイルがありません」と出てしまう — 実際にはあるのに。
        // 「使うファイルがそこにあるか」はエンジンの生成状態とは別の話なので、
        // 歯車の設定と同じくファイルの有無で答える。エンジンができたら
        // ensure_engine が実際に読めたかどうかで上書きする。
        book_loaded: resources().book_path().exists(),
        learn: true,
        ponder: ctx.engine_cfg_ponder,
        pace: ctx.engine_cfg_pace.clone(),
        max_move_secs: ctx.engine_cfg_max_move,
        reserve_secs: ctx.engine_cfg_reserve,
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
                    ponder,
                }) => {
                    apply_engine_cfg(&mut ctx, depth, solve, band);
                    ctx.engine_cfg_ponder = ponder;
                    ctx.snap.lock().unwrap().engine.ponder = ponder;
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
                Ok(Cmd::SetLearn(b)) => {
                    ctx.snap.lock().unwrap().engine.learn = b;
                    ctx.emit(true);
                }
                Ok(Cmd::Rank { .. }) | Ok(Cmd::ListMatches) => {} // 未接続時は無視
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // 未接続の待ち時間も学習 (実戦の取り込み) を消化する
                    learn_tick(&mut ctx);
                }
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

        let mut login_fails = 0u32; // ログイン前に切られた回数
        let mut cred_saved = false; // 認証情報をキーチェーンへ保存したか
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
            // 進行中の対局を聞き直す時刻 (login 直後の 1 回は下で送る)
            let mut next_match_at = Instant::now() + Duration::from_secs(60);
            let mut capture: Option<(String, Vec<String>)> = None; // (kind, lines)
            let mut next_ask_at: Option<Instant> = None;
            let mut want_quit = false;
            // 自分の対局の有無 (前回値)。始まった瞬間にローカル探索を止める
            let mut had_own_match = false;

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
                            rated,
                        } => {
                            // レート有無はアカウント単位の設定なので、申し込みの
                            // 直前に必ず送って揃える (前回のぶんが残るのを防ぐ)
                            send!(
                                ctx,
                                format!(
                                    "tell /os rated {}",
                                    if rated && !no_rated() { "+" } else { "-" }
                                )
                            );
                            // 相手を指定しないと「誰でも受けられる」募集になる。
                            // 空のまま繋ぐと末尾に空白が残るので、そのときは付けない
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
                        Cmd::CloseMatch(id) => {
                            // 終局したものだけ閉じる (進行中は残す)
                            let keys: Vec<String> = matches
                                .iter()
                                .filter(|(k, m)| m.over && (k.as_str() == id || base_id(k) == id))
                                .map(|(k, _)| k.clone())
                                .collect();
                            for k in keys {
                                matches.remove(&k);
                                // ワーカーは残して割り当てだけ外す (置換表を
                                // 抱えているので、作り直すとリークが積み上がる)
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
                            ponder,
                        } => {
                            apply_engine_cfg(&mut ctx, depth, solve, band);
                            ctx.engine_cfg_ponder = ponder;
                            ctx.snap.lock().unwrap().engine.ponder = ponder;
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
                        Cmd::SetLearn(b) => {
                            ctx.snap.lock().unwrap().engine.learn = b;
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
                            /* **開始した時点で既に届いている申し込みを拾う。**
                            自動受諾は「`+ .N` が届いたとき」にしか動かないので、
                            相手が先に申し込んで、こちらが後から待機モードを
                            入れると永久に受けない。実際に踏んだ (相手は待って
                            いるのに、こちらは申し込み待ちのまま並んでいた)。 */
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
                                ctx.log("info", &format!("待機モード: 届いていた {id} を受けます"));
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
                        // 起動時の既定。禁止されているときは立てない
                        send!(
                            ctx,
                            if no_rated() {
                                "tell /os rated -"
                            } else {
                                "tell /os rated +"
                            }
                        );
                        /* **他人の対局の開始・終了は「通知」で届く。**購読しないと
                        一生届かない — 一覧は login 時の `tell /os match` の
                        1 回きりになり、そのとき誰も打っていなければロビーの
                        「対局中」は空のままになる。
                        `+ match` / `- match` を受ける側の処理は前からあるのに、
                        **購読だけが抜けていた** (finger が `notify (-)` の
                        まま = 空だったのが証拠)。 */
                        send!(ctx, "tell /os notify +");
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
                        /* **起動でも中断対局を再開する。** 以前は再接続
                        のときだけだった。**落ちて上がり直した
                        ときが抜けていた** — 相手は待っているのに、こちらは
                        一覧に出して眺めるだけで指さない。

                        中断対局は定義上「終わっていない自分の対局」なので、
                        見つけたら戻るのが筋。放置すれば時間切れになる。

                        なお実測では、**こちらを強制終了しても中断対局には
                        ならなかった** (`stored 0`)。サーバーは切断を「退出」
                        と見て対局を畳む (相手側に `kuroobi left` と出る)。
                        つまりここが効くのは別の経路 — サーバーの再起動など
                        — で中断が作られたときで、落ちた対局は戻らない。 */
                        pending.push("stored".into());
                        send!(ctx, "tell /os stored");
                        ctx.emit(true);
                    }
                    ctx.emit(false);
                    continue;
                }

                // ---------- 行処理 ----------
                while let Some(ln) = lines.pop_front() {
                    ctx.log("in", &ln);

                    // ログイン後に /os の応答が届いた = 認証が通った。ここで
                    // 保存する (誤ったパスワードは応答の前に切断される)。
                    // 次回起動時はこれで自動ログインする。
                    if !cred_saved && (ln == "READY" || ln.starts_with("/os")) {
                        cred_saved = true;
                        crate::keychain::save(&login, &pw);
                    }

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
                                // **サーバーの返事を発言にしない。**`tell` を
                                // 付けずに送る命令には、サーバーが
                                // 「命令名: いまの値」の形で返す。これが
                                // ダイレクト tell と同じ見た目なので、
                                // **`verbose` という名前の相手からの発言**
                                // として会話一覧に並んでいた
                                // (実物: `verbose: -news -ack -help -faq`)。
                                // ここで弾く語は、こちらが素で送る命令だけ
                                && !BARE_CMDS.contains(&name)
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
                            // 盤面が届いた = 観戦できた
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
                                /* **全部再開する。先頭 1 件では足りない。**

                                同期対局は 2 面あり、中断の一覧に別々に
                                並びうる。1 件しか投げないと、もう 1 面は
                                誰も指さないまま時計だけ減って時間切れ —
                                レート戦では 1 面落とすだけで負けになる。

                                余分に投げても、既に動いている対局への
                                `ask` はサーバーが弾くだけで害がない。 */
                                for id in &ids {
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
                            // **終局した対局は数えない。** 棋譜を見られるよう
                            // 一覧には残す作りなので、そのまま数えると一度
                            // 対局しただけで待機モードが二度と受けなくなる
                            let in_match = s.matches.iter().any(|m| !m.over);
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
                                    // 観戦していた対局も残す (終局後の盤面から
                                    // 棋譜を取り出したいため)。閉じるのは手動
                                    finish_match(&mut matches, &id, "");
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
                    } else if let Some(req) = parse_request(&ln, &login) {
                        /* ---------- 相手からの undo / abort ----------

                        実測した書式 (推測ではない):

                            /os: undo  .24 htz is asking
                            /os: abort .24 htz is asking

                        **今まで完全に無視していた。** 相手は返事を待っている
                        のに、こちらの画面には何も出ない。放っておいても対局は
                        続くので気付けなかった。

                        待った・中断はこちらの不利益になりうる (勝勢の局面を
                        巻き戻される) ので、**自動では受けない**。ただし待機
                        モードで無人で回しているときは、答えないまま相手を
                        待たせるほうが悪いので断る。人が見ているときは知らせて
                        判断を任せる (画面から accept / decline を送れる)。 */
                        let unattended = {
                            let s = ctx.snap.lock().unwrap();
                            s.standby.enabled
                        };
                        let what = if req.verb == "undo" {
                            "待った"
                        } else {
                            "中断"
                        };
                        ctx.log(
                            "info",
                            &format!("{} が {} を求めています ({})", req.who, what, req.id),
                        );
                        ctx.notify(
                            &format!("GGS: {what}の申し出"),
                            &format!("{} ({})", req.who, req.id),
                        );
                        if unattended {
                            /* **断るのは相手の名前に対して。** 通知が持って
                            いるのは対局 ID (`.27`) だが、`decline` が受ける
                            のは要求 ID か**プレイヤー名**で、対局 ID を渡すと
                            `decline ERR not found: .27` になる (実測)。
                            要求 ID は通知に含まれないので名前で断る。 */
                            ctx.log(
                                "info",
                                &format!("待機モード: 無人なので断ります ({})", req.who),
                            );
                            send!(ctx, format!("tell /os decline {}", req.who));
                        }
                    } else if ln.starts_with("/os: ERR") {
                        ctx.log("info", &format!("サーバーエラー: {ln}"));
                        // 通信ログは開いていないと読めない。押した結果が
                        // 断られたことは、その場で言わないと伝わらない
                        let msg = ln.trim_start_matches("/os: ERR").trim();
                        ctx.snap.lock().unwrap().notice = if msg.is_empty() {
                            "GGS がこの操作を受け付けませんでした".into()
                        } else {
                            format!("GGS: {msg}")
                        };
                        ctx.dirty = true;
                    }
                }

                // ---------- 待機モードの自動申し込み ----------
                if let Some(t) = next_ask_at {
                    if Instant::now() >= t {
                        next_ask_at = None;
                        let s = ctx.snap.lock().unwrap();
                        let sb = s.standby.clone();
                        // 終局したものは「対局中」に数えない (上と同じ理由)
                        let in_match = s.matches.iter().any(|m| !m.over);
                        let games = s.standby_stats.games;
                        /* **自分が出したまま残っている申し込みがあるか。**
                        受けられずに残っている間に再申し込みすると、
                        募集が二重になって 2 局同時に成立しうる。 */
                        let outgoing = s.offers.iter().any(|o| !o.incoming);
                        drop(s);
                        let more = sb.max_games == 0 || games < sb.max_games;
                        /* **不発なら鳴らし直す。** 申し込みは「終局した」
                        ときにしか予約しておらず、送った後は `None` に
                        戻していた。相手が離席・拒否・接続断で受けな
                        かったら、そこで待機モードが黙って止まる
                        (放置運用ではこれが効く場面そのもの)。
                        成立すれば `in_match` で弾かれるので、鳴らし
                        続けても害はない。 */
                        if sb.enabled && !sb.opponent.is_empty() && more {
                            next_ask_at = Some(
                                Instant::now() + Duration::from_secs(sb.interval_secs.max(30)),
                            );
                        }
                        if sb.enabled && !in_match && !outgoing && !sb.opponent.is_empty() && more {
                            ctx.log("info", &format!("待機モード: {} に申し込み", sb.opponent));
                            // 申し込みの直前にレート有無を揃える (Cmd::Ask と同じ)
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

                // ---------- 進行中の対局を取り直す ----------
                /* **通知だけに頼らない。**他人の対局の開始・終了は
                `+ match` / `- match` で届くはずだが、購読が効いていな
                かった間、ロビーの「対局中」は login 時の 1 回きりの
                一覧のまま (そのとき誰も打っていなければ空のまま) だった。
                **一度でも取りこぼすと二度と埋まらない**作りなので、
                60 秒ごとに聞き直す。`tell /os match` は一覧を返すだけで
                何も動かさない。 */
                if Instant::now() >= next_match_at {
                    next_match_at = Instant::now() + Duration::from_secs(60);
                    pending.push("match_list".into());
                    send!(ctx, "tell /os match");
                }

                /* ---------- 終わった対局のワーカーを解放する ----------

                **終局しても一覧からは消さない作り** (棋譜を見られるように)
                なので、`release_worker` は「閉じたとき」にしか呼ばれない。
                掴んだままだと次の対局で 2 局が 1 つのワーカーを取り合い、
                `pending` の上書きで片方が指されなくなる。 */
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

                /* ---------- 指し忘れの救済 ----------

                **`think_and_play` は update が来たときにしか呼ばれない。**
                そこで投げ損ねると、その対局は次の update まで — 相手も
                待っているなら永久に — 指されない。時計だけが減り、
                時間切れで負ける (実戦で踏んだ)。

                update に頼らず、**毎周「自分の手番なのに指していない対局」を
                探して投げ直す**。二重着手防止 (盤面 + 手数のハッシュ) が
                効くので、既に指した局面は素通りする。 */
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

                // ---------- 探索ワーカーの結果を拾う ----------
                /* **毎周ここで拾う。** 投げっぱなしにしてあるので、拾わないと
                着手が送られない。返す形にしているのは `send!` が `writer` を
                直に持っており、関数の中から呼ぶと借用が噛み合わないため。 */
                for line in collect_workers(&mut ctx, &mut matches) {
                    send!(ctx, line);
                }
                sync_matches(&mut ctx, &matches);

                // ---------- 相手の手番中の先読み (ワーカーが無いときの退路) ----------
                if ctx.workers.is_empty() {
                    ponder_slice(&mut ctx, &matches);
                }

                // ---------- 観戦の失敗を拾う ----------
                // GGS は終わった対局への watch にエラーを返さないことがある。
                // 盤面が来ないまま期限が過ぎたら、その対局はもう無い
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
                        ctx.log(
                            "info",
                            &format!("{id} は観戦できませんでした (対局が終わっています)"),
                        );
                    }
                    let mut s = ctx.snap.lock().unwrap();
                    s.ongoing.retain(|o| !stale.contains(&o.id));
                    s.notice = format!(
                        "{} を観戦できませんでした。対局がすでに終わっているか、\
                         参加できない対局です。",
                        stale.join(" / ")
                    );
                    drop(s);
                    ctx.emit(true);
                }

                // ---------- 自分の対局が始まったらローカルへ CPU を渡させる ----------
                // GGS は時計のある実対局なので最優先。走っているローカルの
                // 探索 (思考・検討) を止めて知らせる。以後の開始は
                // コマンド側 (main.rs) が断る
                let own_match = matches.values().any(|m| m.my_color.is_some());
                if own_match && !had_own_match {
                    let local_busy = ctx.local_activity.lock().unwrap().local.is_some();
                    if local_busy {
                        if let Some(h) = ctx.local_stop.lock().unwrap().as_ref() {
                            h.stop();
                        }
                        ctx.notify("GGS: 対局開始", "ローカルの探索を停止しました");
                        ctx.log("info", "GGS 対局開始 — ローカルの探索を停止しました");
                    }
                }
                had_own_match = own_match;

                // ---------- 実戦の取り込み (学習) ----------
                // 自分の対局が無い間に 1 探索ずつ進める。1 回のブロックは
                // 高々 1 評価ぶんなので、着信への応答が数秒以上遅れない。
                if !own_match {
                    learn_tick(&mut ctx);
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

/// 持ち時間の使い方を差し替える。
fn apply_pacing(
    ctx: &mut Ctx,
    pace: String,
    max_move_secs: u64,
    reserve_secs: u64,
    budget_use: f64,
) {
    // 壊れた値は既定へ倒す (timectl 側でも見るが、画面にも正しい値を出す)
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

/// GGS 用エンジンの並列数。0 は「自動」の印で、コア数の半分にする。
///
/// ローカル側 (`resources.conf` の `threads`) は `Option<usize>` で持てるが、
/// こちらはスナップショットに載る値なので、型を変えずに済む 0 を印にした。
/// **画面には解決後の数ではなく 0 のまま出す** — 解決した数を出すと、
/// コア数が違う機械へ設定を移したときに自動が固定値に化ける。
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

/// **全体設定のスレッド数を引き直して反映する。**
///
/// GGS 専用の欄は持たない。別々に持つと読切速度の較正
/// (`resources.conf` の `nps.<スレッド数>`) が 2 つ要り、片方が未較正の
/// まま対局に入ってしまう。持ち時間の管理はそこで固定の階段に落ちる。
fn apply_threads(ctx: &mut Ctx) {
    let n = resources().threads.unwrap_or_else(|| resolve_threads(0));
    ctx.engine_cfg.threads = n;
    if let Some(e) = ctx.engine.as_mut() {
        e.set_threads(n);
    }
    ctx.snap.lock().unwrap().engine.threads = n;
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
    let was_overtime = m.in_overtime;
    let (rows_ok, turn) = apply_block(m, block, login);
    /* **ロスタイムに入ったら必ず言う。** 勝敗が決まる状態なのに、時計は
    健全な残り 2 分に見える (猶予が同じ時計へ加算されるため)。ここで
    出しておかないと、後からログを見ても入ったかどうか分からない。 */
    if m.in_overtime && !was_overtime {
        ctx.log(
            "info",
            &format!(
                "{mid}: ロスタイムに入りました。**この対局は時間切れ負けが確定** \
                 しています (結果は最小差負けで頭打ち)。以降は全滅を避けるため \
                 速く指し切ります"
            ),
        );
    }

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
    if turn.is_some() && turn == m.my_color && !m.told_turn {
        // 自分が打つ設定 (auto を切っている) のときだけ報せる。KUROOBI が
        // 打つなら人は何もしないので、鳴らすと邪魔にしかならない
        m.told_turn = true;
        if !auto {
            let who = if m.opp_name.is_empty() {
                mid.clone()
            } else {
                m.opp_name.clone()
            };
            ctx.notify("GGS: あなたの手番です", &who);
        }
    } else if turn != m.my_color {
        // 相手の手番に戻ったら次の手番でまた報せる
        let m = matches.get_mut(&mid).unwrap();
        m.told_turn = false;
    }
    /* **先読みの印。** 相手の手番になった対局を覚えておき、受信の合間に
    少しずつ読む (`ponder_slice`)。ここで読み切らないのは、思考が
    終わるまで受信が止まって相手の着手に気付けなくなるため。 */
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

/// 観戦中の局面を解析する (エンジンの通常レベルより浅め・短時間で)。
/// 相手の手番中に少しだけ先読みする。
///
/// **1 回の呼び出しで読むのは 200ms だけ。** 読み切ると受信が止まり、
/// 相手が指したことに気付けなくなる — 受信の待ちが 250ms なので、
/// 同じくらいの刻みで交互に回す。
///
/// **「深さ固定」でも効く。** 効き方が変わるだけで、
///
/// * 持ち時間で刻むとき … 同じ時間で **+1.25 段**深く読める
/// * 深さ固定のとき … 同じ深さへ **1/3 の時間**で着く (実測 −62〜65%)
///
/// 「深さ固定では探索がどのみち最後まで走るので無駄」と一度書いたが誤り。
/// 走り切る先が置換表に載っていれば、走り切るのが速くなる。
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
    /* **相手の手番の盤を組む。** `board_of` は「渡した色が手番」の盤を返す
    ので、ここで自分の色を渡すと手番が入れ替わった別の局面になる
    (`think_and_play` は自分の手番で呼ばれるので自分の色でよい)。 */
    let Some(board) = board_of(m, m.turn) else {
        return;
    };
    if Some(m.turn) == m.my_color {
        return; // 自分の手番に戻っていた
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
                        /* ---------- ロスタイムに入ったかを見る ----------

                        **時計が増えたら入っている。** GGS の時計は 1 本で
                        (`GAME_Clock.C::Update`)、本時間を切らすと猶予ぶんが
                        **その時計に加算される**。表示の第 3 項は設定値を
                        出しているだけなので動かない。つまり「残り時間が
                        0 になった」も「第 3 項が減った」も観測できず、
                        **時計が跳ね上がったことだけが手掛かり**になる。

                        入った時点でその対局は時間切れ負けが確定していて、
                        残るのは全滅を避けることだけ ([`timectl`] を参照)。
                        だから一度立てたら下ろさない。

                        加算は減っていくものが増える唯一の場面。持ち時間の
                        加算 (`inc`) がある対局では毎手増えうるので、猶予の
                        設定値ぶん近く跳ねたときだけ拾う。 */
                        if let (Some(now), Some(g)) = (main, ext) {
                            /* **実測で 2 通りの見え方があった。**

                            ① 猶予が加算されて跳ね上がる (`00:05` → `01:59`)
                            ② `00:00` に張り付いたまま手数だけ進む

                            ② のほうが多い。どちらでも「本時間を使い切った」
                            ことに変わりはなく、やることも同じなので、両方を
                            拾う。0 を見て立てるほうは、まだ厳密には切れて
                            いない (0.4 秒残りが 00:00 と出る) 場合も含むが、
                            **どのみち読んでいる余裕は無い**ので害がない。 */
                            let jumped = m.my_clock_secs.is_some_and(|prev| now > prev + g / 2);
                            /* **一度立てたら下ろさない**ので、序盤で誤って
                            立てるとその対局を丸ごと捨てることになる (以後
                            1.5 秒/手)。自分がまだ 1 手も指していないうちに
                            0 を見るのは、時計が届いていないなど別の理由の
                            はず — **持ち時間を使っていないのに使い切ることは
                            ない**。跳ね上がりのほうは形が特徴的なので通す。 */
                            let played = m.moves.len() >= 2;
                            if g > 0 && (jumped || (now == 0 && played)) {
                                m.in_overtime = true;
                            }
                        }
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
/// 1 手ぶんの計画を立てる。**中身は `kuroobi::timectl` が持つ** —
/// 配り方の良し悪しは持ち時間制の対局でしか測れないのに、GUI の中にあると
/// CLI から呼べず自己対局で比べられなかった。ここは呼ぶだけの橋。
///
/// 戻り値は (中盤深さ, 完全読み開始の空き, 帯, 1 手の期限)。
fn time_budget(
    mut s: kuroobi::timectl::Situation,
    base: (u32, u8, u8),
    pace: &str,
) -> (u32, u8, u8, Option<Duration>) {
    // 較正済みなら読切の入り口を残り時間から逆算する。**呼び出し側に
    // 覚えさせない** — 設定ファイルの読み方をここに閉じておけば、GGS と
    // ローカルで食い違いようがない
    s.nps = resources().nps_for(s.threads);
    let p = kuroobi::timectl::plan(
        s,
        kuroobi::timectl::Levels {
            depth: base.0,
            solve: base.1,
            band: base.2,
        },
        kuroobi::timectl::Pace::parse(pace),
    );
    (p.depth, p.solve, p.band, p.cap)
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
    /* 同一局面への二重着手防止。**手数も混ぜる** — 盤面だけで見ると、
    相手がパスしたときに「自分が最後に指した局面」と一致してしまい、
    黙って return して二度と指さなくなる (時計だけが減り続ける)。
    パスは石を置かないので盤面が変わらず、手番だけが戻ってくる。 */
    let bh = board
        .black
        .wrapping_mul(31)
        .wrapping_add(board.white)
        .wrapping_add((m.moves.len() as u64).wrapping_mul(0x9e37_79b9));
    if bh == m.last_played_hash {
        return;
    }
    /* **もう読んでいる局面なら投げ直さない。**

    `last_played_hash` は着手を**送った後**に立つ。読んでいる最中はまだ
    立っていないので、毎周の「指し忘れの救済」がその隙に同じ局面をもう一度
    積み、返ってきた瞬間に控えが投げられて**同じ手を 2 回送る**。

    実戦の通信ログで気付いた — 60 手の対局で 58 回 `play` を送っており、
    ほとんどの手が重複していた。サーバーは 2 通目を撥ねるので**棋譜は
    正しいまま**で、そこが厄介だった。実害は指し手ではなく**探索が毎手
    二度走ること** (持ち時間が実質半分になる)。

    走っている探索と控えの両方を見る。控えだけを見ると、投げた直後の
    `pending` が空の窓で素通りする。 */
    if ctx.workers.iter().any(|w| {
        w.mid.as_deref() == Some(mid)
            && ((w.busy && !w.pondering && w.pending_hash == bh)
                || w.pending.as_ref().is_some_and(|p| p.hash == bh))
    }) {
        return;
    }
    if let Err(e) = ctx.ensure_engine() {
        ctx.log("info", &format!("エンジン初期化失敗: {e}"));
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
            // **ワーカーで分けた後の数。** 2 つ同時に走るので、全体の数で
            // 見積もると読切に入りすぎる
            threads: ctx.worker_threads,
            ..Default::default()
        },
        base,
        &ctx.engine_cfg_pace,
    );
    /* **ワーカーへ投げて返事を待たない。** 同期対局は 2 局が同時に進むので、
    ここで待つと片方の時計を捨てることになる。結果は受信ループが毎周
    `collect_worker` で拾い、そこで着手を送る。 */
    if let Some(i) = ctx.worker_for(mid) {
        /* **待っている仕事を控えておく。** `think_and_play` は update が
        来たときにしか呼ばれないので、ここで投げ損ねるとその対局は二度と
        指されない — 先読みで塞がっている隙に自分の手番が来ると、時計だけ
        減って時間切れになる (実戦で踏んだ)。

        塞がっているのが**先読み**なら捨ててよい。自分の時計は減らないので
        価値はあるが、**指さないほうが遥かに高くつく**。 */
        ctx.workers[i].pending = Some(Pending {
            board,
            levels: (d, solve, band),
            cap,
            hash: bh,
            // 同期対局の鏡像で相手が既に指していれば、その手を先に見る
            hint: mirror_hint(matches, mid),
        });
        if ctx.workers[i].pondering {
            ctx.workers[i].stop.stop();
        }
        pump_worker(ctx, i);
        return;
    }
    // ワーカーを作れなかったときの退路: 従来どおりここで読む
    if let Err(e) = ctx.ensure_engine() {
        ctx.log("info", &format!("エンジン初期化失敗: {e}"));
        return;
    }
    let engine = ctx.engine.as_mut().unwrap();
    engine.set_levels(d, solve, band);
    let deadline = cap.map(|c| std::time::Instant::now() + c);
    let mv = engine.choose_within(&board, deadline);
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
            /* **到達深さも出す。** これが無いと「深く読めているのに
            負けている」のか「そもそも浅い」のかが切り分けられない。
            実戦 7 局を解析したとき、中盤で 1 手あたり 0.55 石ずつ
            失っていることは分かったが、原因が評価なのか深さなのかを
            ログから判断できなかった。 */
            "{mid} {mstr}: {} {:+.2}{}{}",
            if mv.from_book && mv.learned {
                "定石 (実戦の学習)"
            } else if mv.from_book {
                "定石"
            } else {
                "探索"
            },
            mv.value,
            if mv.exact { " (完全読み)" } else { "" },
            // 読切と定石は深さを持たない (0 が入る) ので出さない
            if mv.depth > 0 {
                format!(" 深さ {}", mv.depth)
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

/// **控えてある本探索を、空いていれば投げる。**
///
/// 塞がっているうちは何もしない。`collect_workers` が結果を拾って `busy` を
/// 下ろした直後にも呼ぶので、**投げ損ねが残らない**。
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
    // 先読みを打ち切った直後は停止が立ったままなので戻す
    ctx.workers[i].stop.reset();
    let empties = p.board.empty_count();
    let movable = p.board.movable_count();
    ctx.log(
        "info",
        &format!(
            "投: 空き {empties} 手数 {movable} 期限 {:.1}s (深さ {} 読切 {} 帯 {})",
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

/// **ワーカーが返した着手を拾う。**
///
/// 送るべき行を返すだけで、自分では送らない — 受信ループの `send!` は
/// `writer` を直に持っており、ここへ渡すと `ctx` の可変借用と噛み合わない。
///
/// あわせて、相手の手番の対局には**空いているワーカーで先読みを投げる**。
/// 先読みは自分の時計を減らさないので、空いているなら常に読ませたほうが得
/// (実測で予測手 1 本のとき +1.6〜1.8 段)。
fn collect_workers(ctx: &mut Ctx, matches: &mut HashMap<String, MatchState>) -> Vec<String> {
    let mut out = Vec::new();
    /* **期限を大きく過ぎても返らない探索を止める。**

    読切は期限で打ち切るようにしたが、それでも返らない経路が残っていたら
    (打ち切りの取りこぼし、想定外の長考) 対局が止まる。時間切れは一発で
    レートを失うので、**保険としてここでも止める**。余裕を持って 3 倍。 */
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
                    "探索が期限を大きく超えたので止めました ({:.1} 秒 / 期限 {:.1} 秒)",
                    at.elapsed().as_secs_f32(),
                    cap.as_secs_f32()
                ),
            );
        }
    }
    /* **止まらないワーカーは見捨てて作り直す。**

    停止を立てても返らないなら、その席は二度と空かない。上限は 2 なので
    1 つ失うだけで同期対局が片肺になり、2 つ失えば対局そのものが止まる。
    見捨てた側のスレッドは探索が終われば送信口が閉じて自分で抜ける
    (一時的にスレッドが 1 本余分に残るが、起きるべきでない経路の保険と
    しては安い)。 */
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
                ctx.log("info", "止まらない探索を見捨てて、ワーカーを作り直しました");
            }
            Err(e) => ctx.log("info", &format!("ワーカーの作り直しに失敗: {e}")),
        }
    }
    for i in 0..ctx.workers.len() {
        // 返事が来ていれば拾う (来ていなければ何もしない)
        let done = ctx.workers[i].rx.try_recv().ok();
        let Some(done) = done else { continue };
        ctx.workers[i].busy = false;
        // 実測は投げ直す前に取る (pump_worker が sent_at を上書きする)
        let took = ctx.workers[i].sent_at.map(|(t, _)| t.elapsed());
        ctx.workers[i].sent_at = None;
        // **拾った直後に投げ直す。** ここを忘れると、控えた本探索が
        // 次の update まで投げられない (来なければ永久に指さない)
        pump_worker(ctx, i);
        let Done::Moved(mv) = done else { continue };
        if let Some(t) = took {
            ctx.log(
                "info",
                &format!(
                    "着: {:.1}s{}{} {}",
                    t.as_secs_f32(),
                    if mv.cut { " (打切)" } else { "" },
                    if mv.depth > 0 {
                        format!(" 深さ {}", mv.depth)
                    } else {
                        String::new()
                    },
                    if mv.exact { "読切" } else { "探索" }
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
        /* **終わった対局へは指さない。** 相手が投了・中断すると、
        こちらが読んでいる最中に終局する。解放は毎周やっているので
        普段はここへ来ないが、順序に頼らず**指す直前でも確かめる**
        (終局後の着手はサーバーに撥ねられ、ログだけが荒れる)。
        自分の手番でなくなっている場合も同じ。 */
        if m.over || m.my_color.is_none() || Some(m.turn) != m.my_color {
            continue;
        }
        let mstr = match mv.pos {
            Some(p) => coord(p),
            None => "pa".to_string(),
        };
        out.push(format!("tell /os play {mid} {mstr}"));
        m.last_eval = Some(if mv.value.is_finite() { mv.value } else { 0.0 });
        m.last_eval_exact = mv.exact;
        m.last_from_book = mv.from_book;
        m.last_played_hash = bh;
        ctx.log(
            "info",
            &format!(
                "{mid} {mstr}: {} {:+.2}{}",
                if mv.from_book && mv.learned {
                    "定石 (実戦の学習)"
                } else if mv.from_book {
                    "定石"
                } else {
                    "探索"
                },
                mv.value,
                if mv.exact { " (完全読み)" } else { "" }
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

    /* 空いているワーカーは相手の手番の局を先読みしておく。

    **刻みが 200ms なのは受信ループの都合だった。** エンジンを受信ループで
    直に回していたころは、長く読むと相手の着手に気付けなくなるので細かく
    刻むしかなかった。ワーカーは別スレッドなので、その制約はもう無い。
    **相手が長考する 30 秒を 150 回に切り刻む理由が無い** — 刻むたびに
    反復深化が浅い段から回り直すので、実質的に浅い読みを繰り返していた。 */
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
            // 終わった対局は読まない (終局で `turn` が ' ' になるため、
            // 手番の判定だけだと「相手の手番」に見えて読み続けてしまう)
            if m.over {
                continue;
            }
            // 自分の手番なら本探索の番。先読みはしない
            if m.my_color.is_none() || Some(m.turn) == m.my_color {
                continue;
            }
            /* **相手の手番の盤を組む。** `board_of` は「渡した色が手番」の盤を
            返すので、自分の色を渡すと手番が入れ替わった別の局面になる。 */
            let Some(board) = board_of(m, m.turn) else {
                continue;
            };
            // 控えている本探索があるなら先読みしない (そちらが先)
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

/// 結果の表示文字列 (石差)。
fn re_text(score: Option<f32>) -> Option<String> {
    score.map(|s| format!("{s:+.2}"))
}

fn handle_match_end(
    ctx: &mut Ctx,
    rest: &str,
    login: &str,
    matches: &mut HashMap<String, MatchState>,
) {
    // 実形式: ".13 1866 kuroobi 1411 fly 8 R +54.00  .82720"
    // (末尾はアーカイブ番号。スコアは**先に並ぶ側**から見た石差 —
    //  「黒視点」と書いていたが誤りで、同期対局で勝敗が反転していた)
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
    let mut dropped = finish_match(matches, &id, re_text(score).as_deref().unwrap_or(""));
    dropped.sort_by_key(|m| m.seen);
    let m = dropped.first();
    let re = score.map(|s| format!("{s:+.2}"));
    let (kifu, ggf, opp) = match &m {
        Some(m) => (m.kifu(), m.ggf(&id, re.as_deref()), m.opp_name.clone()),
        None => (String::new(), String::new(), String::new()),
    };
    /* **石差は「先に並ぶ側」から見た値。色ではない。**

        - match .48 2639 kuroobi 2644 Rhapsody s8r16 R +2.00
                       ~~~~~~~ この人から見て +2

    以前は自分の色 (`my_color == '*'`) で符号を決めていた。**同期対局では
    面ごとに色が逆**なので、どちらの面を代表に選んだかで勝敗が反転する。
    代表は「先に見た面」なので、届く順で結果が変わっていた。

    実際にレート戦の 1 局目 (+2.00 の勝ち) を「1 敗 −2 石差」と表示した。
    サーバーのレートは +85.3 動いており、画面だけが逆を向いていた。 */
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

    // ---- 実戦の取り込み (学習) をキューに積む ----
    // 勝敗にかかわらず自分の対局を取り込む。負け・引き分けは同じ展開の
    // 反復を避けるため、勝ちは相手のミスでしか勝てなかったラインを良いと
    // 思い込み続けないため。実行は対局の合間に 1 探索ずつ (メインループ)。
    // synchro は 2 局それぞれを取り込む。
    if !ctx.snap.lock().unwrap().engine.learn {
        return;
    }
    for lm in &dropped {
        if lm.my_color.is_none() {
            continue; // 観戦は取り込まない (自分の選択ではない)
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
                    &format!("学習: {id} を取り込み待ちに追加 ({} 局面)", job.remaining()),
                );
                // 終局の石差は棋譜を再生して数える。対局の記録は取り込みが
                // 終わるころには残っていない
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
                    // 明細は取り込みが終わってから入る
                    changes: Vec::new(),
                    opponent: lm.opp_name.clone(),
                    // GGS は自分の色を知っている ('*' が黒 / 'O' が白)
                    my_color: match lm.my_color {
                        Some('*') => "b".into(),
                        Some('O') => "w".into(),
                        _ => String::new(),
                    },
                };
                ctx.learn_jobs.push_back((id.clone(), job, entry));
            }
            Err(e) => ctx.log("info", &format!("学習の準備に失敗 ({id}): {e}")),
        }
    }
    ctx.emit(true);
}

/// 学習キューを 1 探索ぶんだけ進める。自分の対局が無い間に呼ぶ。
/// 完了した対局はログに出してキューから外す。
fn learn_tick(ctx: &mut Ctx) {
    if ctx.learn_jobs.is_empty() || ctx.ensure_engine().is_err() {
        return;
    }
    let (id, job, entry) = ctx.learn_jobs.front_mut().expect("非空を確認済み");
    let id = id.clone();
    let entry = entry.clone();
    match ctx.engine.as_mut().unwrap().learn_step(job, LEARN_DEPTH) {
        Ok(Some(out)) => {
            ctx.learn_jobs.pop_front();
            // 控えはローカル対局と同じ場所へ。どちらから覚えた手かは
            // 相手の名前で分かる
            let mut entry = entry;
            entry.changes = out.changes.iter().map(crate::LearnChange::of).collect();
            crate::learn_log_append(&entry);
            ctx.log(
                "info",
                &format!(
                    "学習: {id} を取り込んだ ({} 手の値を更新、{} 局面を追加。残り {} 局)",
                    out.updated,
                    out.added,
                    ctx.learn_jobs.len()
                ),
            );
        }
        Ok(None) => {}
        Err(e) => {
            ctx.learn_jobs.pop_front();
            ctx.log("info", &format!("学習に失敗 ({id}): {e}"));
        }
    }
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
/// 偏差のトークン (`74.9=` `267.8` など)。末尾の傾向印を落として読む。
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
            // 実際の行: `| 1 scorpion 1938.0@ 74.9= 7.12:05:00+@ …`
            // レートの直後に `@` が付き、**偏差は次のトークン**に来る
            // (末尾に `=` `+` `-` の傾向印が付く)。
            let at = toks
                .iter()
                .enumerate()
                .skip(1)
                .find_map(|(i, t)| parse_rating_token(t).map(|v| (i, v)));
            let Some((ri, r)) = at else { continue };
            let rating = Some(r);
            // `1938.0@ 74.9=` (次のトークン) と `2066.9@126.4=` (同じ
            // トークンの続き) の両方がある
            let dev = match toks[ri].split_once('@') {
                Some((_, rest)) if !rest.is_empty() => parse_dev_token(rest),
                Some(_) => toks.get(ri + 1).and_then(|t| parse_dev_token(t)),
                None => None,
            };
            if name == login {
                my_rating = rating;
            }
            users.push(UserRow {
                name: name.to_string(),
                rating,
                dev,
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

    /// **鏡像から手を借りる条件。**
    ///
    /// 同期対局の 2 面は同じ手順をたどる間だけ同一局面になる。分岐したら
    /// 別の局面なので、借りた手は合法とも限らない。
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
        // こちらは 2 手、相手の面は 3 手進んでいて、手順は一致
        ms.insert(".9.0".to_string(), mk(&[("", "e2"), ("", "g4")]));
        ms.insert(
            ".9.1".to_string(),
            mk(&[("", "e2"), ("", "g4"), ("", "g5")]),
        );
        let h = mirror_hint(&ms, ".9.0").expect("借りられるはず");
        assert_eq!(coord(h), "G5");

        // 相手の面が先に進んでいなければ借りるものが無い
        assert!(mirror_hint(&ms, ".9.1").is_none());

        // 手順が分岐していたら使わない
        ms.insert(
            ".9.1".to_string(),
            mk(&[("", "e2"), ("", "h4"), ("", "g5")]),
        );
        assert!(mirror_hint(&ms, ".9.0").is_none(), "分岐後に借りた");

        // 相方が無ければ何も返さない
        ms.remove(".9.1");
        assert!(mirror_hint(&ms, ".9.0").is_none());
    }

    /// **石差の符号は「先に並ぶ側」から見る。色ではない。**
    ///
    /// 実際のレート戦 4 局の終局行で確かめる。1 局目は勝ちなのに
    /// 「1 敗 −2 石差」と表示していた (サーバーのレートは +85.3 動いていた)。
    #[test]
    fn the_stone_diff_follows_the_first_name() {
        // 自分が先に並ぶ → そのまま
        assert_eq!(my_stone_diff(2.0, "kuroobi", "kuroobi"), 2);
        assert_eq!(my_stone_diff(-10.0, "kuroobi", "kuroobi"), -10);
        assert_eq!(my_stone_diff(-5.0, "kuroobi", "kuroobi"), -5);
        assert_eq!(my_stone_diff(-7.0, "kuroobi", "kuroobi"), -7);
        // 相手が先に並ぶ → 反転
        // ".18 1720 htz 2580 kuroobi s8r16 U -6.00" は kuroobi の 6 石勝ち
        assert_eq!(my_stone_diff(-6.0, "htz", "kuroobi"), 6);
        assert_eq!(my_stone_diff(2.0, "htz", "kuroobi"), -2);
        // 引き分けはどちらでも 0
        assert_eq!(my_stone_diff(0.0, "kuroobi", "kuroobi"), 0);
        assert_eq!(my_stone_diff(0.0, "htz", "kuroobi"), 0);
    }

    /// 待った・中断の申し出。**書式は実サーバーで採った** (資料に無い)。
    #[test]
    fn request_real_format() {
        let r = parse_request("/os: undo .24 htz is asking", "kuroobi").unwrap();
        assert_eq!(
            (r.verb, r.id.as_str(), r.who.as_str()),
            ("undo", ".24", "htz")
        );
        let r = parse_request("/os: abort .24 htz is asking", "kuroobi").unwrap();
        assert_eq!(r.verb, "abort");
        // 自分が出した申し出も同じ形で返る。反応してはいけない
        assert!(parse_request("/os: undo .24 kuroobi is asking", "kuroobi").is_none());
        // 似て非なるものを拾わない
        assert!(parse_request("/os: update .24 8 K?", "kuroobi").is_none());
        assert!(parse_request("/os: undo .24 htz declined", "kuroobi").is_none());
        assert!(parse_request("/os: - match .24 1720 htz", "kuroobi").is_none());
    }

    /// 負の残り時間は 0 として読む。**解析に失敗させてはいけない** —
    /// `None` は「時間制でない対局」の意味で、期限なしで読み始める。
    #[test]
    fn a_negative_clock_reads_as_zero() {
        let (main, _, ext) = parse_clock("-0:03,0:0//02:00,0:0");
        assert_eq!(main, Some(0));
        assert_eq!(ext, Some(120));
    }

    /// **時計が跳ね上がったらロスタイムに入っている。**
    ///
    /// GGS の時計は 1 本しかなく、本時間を切らすと猶予ぶんが加算される
    /// (`GAME_Clock.C::Update`)。表示の第 3 項は設定値なので動かない。
    /// つまりこの跳ね上がりだけが手掛かりになる。
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
            // 手数の行を入れる。**指してもいないのに時間切れにはならない**
            // ので、0 への張り付きは 1 手以上進んでからしか拾わない
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
        assert!(!m.in_overtime, "まだ本時間");
        // 本時間を切らした → サーバーが 2:00 を足して寄こす
        apply_block(&mut m, &with_clock("02:00,0:0"), "kuroobi");
        assert!(m.in_overtime, "跳ね上がりを拾えていない");
        // 実測ではこちらのほうが多い: 00:00 に張り付いたまま手数だけ進む
        let mut m2 = MatchState::new();
        apply_block(&mut m2, &with_clock("00:03,0:0"), "kuroobi");
        assert!(!m2.in_overtime);
        apply_block(&mut m2, &with_clock("00:00,0:0"), "kuroobi");
        assert!(m2.in_overtime, "0 への張り付きを拾えていない");
        // 一度入ったら下ろさない (以後は普通に減っていく)
        apply_block(&mut m, &with_clock("01:12,0:0"), "kuroobi");
        assert!(m.in_overtime, "下ろしてしまった");
    }

    /// **指してもいないうちの 0 は拾わない。**
    ///
    /// 一度立てたら下ろさないので、序盤で誤ると対局を丸ごと捨てる
    /// (以後 1.5 秒/手)。持ち時間を使っていないのに使い切ることはない。
    #[test]
    fn a_zero_before_any_move_is_not_overtime() {
        let mut m = MatchState::new();
        let b = vec![
            "|kuroobi  (1720.0 *) 00:00,0:0//02:00,0:0".to_string(),
            "|* to move".to_string(),
        ];
        apply_block(&mut m, &b, "kuroobi");
        assert!(!m.in_overtime, "1 手も指していないのに立った");
    }

    /// 普通に減っていくだけの時計をロスタイムと誤認しない。
    #[test]
    fn a_falling_clock_is_not_overtime() {
        let mut m = MatchState::new();
        for secs in ["15:00,0:0", "14:31,0:0", "13:02,0:0", "00:41,0:0"] {
            let b = vec![
                format!("|kuroobi  (1720.0 *) {secs}//02:00,0:0"),
                "|* to move".to_string(),
            ];
            apply_block(&mut m, &b, "kuroobi");
            assert!(!m.in_overtime, "{secs} でロスタイム扱いになった");
        }
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
        // 深さで決める: 期限を付けない (設定どおり読み切る)
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
        assert!(c.is_none(), "期限なし");
    }

    #[test]
    fn the_budget_shrinks_with_the_clock() {
        let a = cap(Some(900), 40, "fast", 0).unwrap();
        let b = cap(Some(300), 40, "fast", 0).unwrap();
        let c = cap(Some(60), 40, "fast", 0).unwrap();
        assert!(
            a > b && b > c,
            "残りが減るほど 1 手が短い: {a:?} > {b:?} > {c:?}"
        );
    }

    /// **落とした配り方は既定へ倒れる。** 古い設定ファイルに "slow" が
    /// 残っていても、3 秒の対局で勝率 0% だった配り方に戻らない。
    #[test]
    fn dropped_paces_fall_back_to_the_default() {
        let fast = cap(Some(900), 50, "fast", 0).unwrap();
        for p in ["slow", "even", ""] {
            assert_eq!(cap(Some(900), 50, p, 0).unwrap(), fast, "{p:?}");
        }
        // 等分の式そのものは基準として生きている (既定より厚い)
        let even = cap(Some(900), 50, "tail:1.0", 0).unwrap();
        assert!(even > fast, "等分 {even:?} > 既定 {fast:?}");
    }

    #[test]
    fn the_per_move_cap_is_honoured() {
        let c = cap(Some(3600), 50, "fast", 5).unwrap();
        assert!(c <= Duration::from_secs(5), "上限 5 秒を超えない: {c:?}");
    }

    #[test]
    fn out_of_main_time_plays_fast() {
        // 本時間 0・ロスタイムあり → ごく短い期限で指す
        let c = cap(Some(0), 30, "fast", 0).unwrap();
        assert!(c <= Duration::from_secs(1), "{c:?}");
    }

    #[test]
    fn the_endgame_keeps_a_reserve() {
        // 読み切り 1 回分を残すので、中盤の予算は残り時間そのものではない
        let (_, _, _, c) = time_budget(
            kuroobi::timectl::Situation {
                clock_secs: Some(100),
                empties: 40,
                ..Default::default()
            },
            BASE,
            // 等分の式で見る。既定 (fast) は 1 手を薄くするので、
            // 「取り置きぶんだけ残るか」の確認には向かない
            "tail:1.0",
        );
        /* **期限は「配分」ではなく「上限」。**

        反復深化は期限まで粘らず、次の段が入らないと判断した時点で返る
        (実測で期限の 47%)。そのぶん期限を伸ばしてあるので
        (`timectl::BUDGET_USE`)、期限 × 残り手数は残り時間を超えてよい。

        守るべきは**使い切っても尽きないこと**。1 手ぶんの期限が、取り置きを
        除いた残り全部を超えないことを見る。 */
        let b = c.unwrap().as_secs_f64();
        assert!(b <= 80.0, "1 手の期限が配れるぶん (80 秒) を超えた: {b:.1}");
        assert!(b > 0.0);
    }
}
