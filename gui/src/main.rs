//! Kuroobi の GUI (Tauri)。エンジンはライブラリとして同一プロセスに
//! リンクし、探索はブロッキングのままワーカースレッド (spawn_blocking) で
//! 回す。フロントは gui/ui/ の静的ページ。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ggs;
mod keychain;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::game::Reversi;
use kuroobi::resources::Resources;
use kuroobi::{Board, Color, Position};

struct App {
    game: Mutex<Reversi>,
    engine: Arc<Mutex<Option<Engine>>>,
    /// 探索中でも触れるよう Engine の Mutex の外に置く停止ハンドル。
    stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    /// GGS セッション (常駐スレッド)。
    ggs: Mutex<Option<ggs::Handle>>,
    /// ローカル対局を定石の学習に取り込むか。
    learn_on: Mutex<bool>,
    /// いま CPU を使っている機能の記録 (画面の常時表示と、学習が譲る判断)。
    activity: Arc<Mutex<Activity>>,
    /// CPU 使用率の前回サンプル (実時間, プロセスの CPU 時間)。
    cpu_meter: Mutex<Option<(std::time::Instant, std::time::Duration)>>,
}

/// このプロセスが使った CPU 時間 (user + sys)。自プロセスなので特別な
/// 権限は要らない。
fn process_cpu_time() -> std::time::Duration {
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        let secs = (ru.ru_utime.tv_sec + ru.ru_stime.tv_sec).max(0) as u64;
        let micros = (ru.ru_utime.tv_usec + ru.ru_stime.tv_usec).max(0) as u64;
        std::time::Duration::from_secs(secs) + std::time::Duration::from_micros(micros)
    }
}

/// このプロセスが今使っている物理メモリ (常駐サイズ)。ピーク値ではなく
/// 現在値なので、置換表を張り直したときの増減がそのまま見える。
#[cfg(target_os = "macos")]
fn process_memory() -> u64 {
    // 構造体と定数は libc、タスクポートだけ mach2 から取る
    // (libc の mach_task_self_ は非推奨になったため)
    unsafe {
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info>()
            / std::mem::size_of::<libc::natural_t>())
            as libc::mach_msg_type_number_t;
        let rc = libc::task_info(
            mach2::traps::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as libc::task_info_t,
            &mut count,
        );
        if rc == 0 {
            info.resident_size
        } else {
            0
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn process_memory() -> u64 {
    0
}

/// 積んでいる物理メモリの総量 (使用率の分母)。
#[cfg(target_os = "macos")]
fn total_memory() -> u64 {
    let mut sz: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let Ok(name) = std::ffi::CString::new("hw.memsize") else {
        return 0;
    };
    unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut sz as *mut _ as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        );
    }
    sz
}

#[cfg(not(target_os = "macos"))]
fn total_memory() -> u64 {
    0
}

/// ローカルで CPU を使っている機能。探索は排他 (同時に 1 つ) なので
/// local は 1 枠でよい。学習は裏で進むが、他が動いている間は譲って止まる。
#[derive(Default)]
pub(crate) struct Activity {
    /// 走っているローカル探索の種別 (思考 / 解析 / 分析)。
    pub(crate) local: Option<&'static str>,
    /// 学習の取り込み進捗 (済み, 総数)。ジョブが無ければ None。
    learn: Option<(u32, u32)>,
    /// 学習が他の機能に譲って止まっているか。
    learn_paused: bool,
}

/// ローカル探索の実行中マーク。スコープを抜けたら自動で消える。
struct ActivityGuard(Arc<Mutex<Activity>>);
impl ActivityGuard {
    fn begin(slot: &Arc<Mutex<Activity>>, kind: &'static str) -> Self {
        slot.lock().unwrap().local = Some(kind);
        Self(slot.clone())
    }
}
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap().local = None;
    }
}

/// スレッド数の既定 (コア数の半分)。
fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(4)
}

fn ggs_snap_arc(app: &State<App>) -> Option<Arc<Mutex<ggs::Snapshot>>> {
    app.ggs.lock().unwrap().as_ref().map(|h| h.snapshot.clone())
}

/// GGS で自分の対局が進行中か。時計のある実対局なので最優先で CPU を
/// 渡す — この間ローカルの探索は開始を断り、学習も譲って止まる。
fn ggs_match_in(snap: &Option<Arc<Mutex<ggs::Snapshot>>>) -> bool {
    snap.as_ref().is_some_and(|s| {
        s.lock()
            .unwrap()
            .matches
            .iter()
            // 終局した対局は一覧に残るので、進行中のものだけを見る
            .any(|m| !m.my_color.is_empty() && !m.over)
    })
}

fn ggs_match_active(app: &State<App>) -> bool {
    ggs_match_in(&ggs_snap_arc(app))
}

fn same_board(a: &Board, b: &Board) -> bool {
    a.black == b.black && a.white == b.white && a.player() == b.player()
}

/// f32 の NaN/∞ は JSON で null になりフロントを壊すので、境界で有限値に丸める。
fn finite(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else if v.is_nan() {
        0.0
    } else if v > 0.0 {
        64.0
    } else {
        -64.0
    }
}

/// 盤面と対局状態のスナップショット (フロントへ渡す形)。
#[derive(Serialize, Clone)]
struct GameView {
    /// 64 マス: 0 = 空き, 1 = 黒, 2 = 白 (A1=0 の file-major)。
    cells: Vec<u8>,
    /// "black" | "white"
    player: String,
    legal: Vec<u8>,
    black: u8,
    white: u8,
    over: bool,
    /// 直前の着手マス (パスなら null)。
    last: Option<u8>,
    /// f5d6... 形式の棋譜。
    kifu: String,
    move_count: usize,
    /// 全手順 (undo で戻った先の手も含む)。null = パス。
    moves: Vec<Option<u8>>,
    /// 現在の局面が moves の何手目の後か。
    cursor: usize,
}

#[derive(Serialize)]
struct ThinkView {
    /// 選んだマス (パスなら null)。局面はまだ動かしていない。
    pos: Option<u8>,
    /// 手番視点の評価値 (石差)。
    value: f32,
    exact: bool,
    /// 定石 book から返した手か。
    from_book: bool,
    /// 実戦から学習した局面の定石か (表示用)。
    learned: bool,
    /// この手に使った時間 (秒)。定石から返した手はほぼ 0。
    secs: f32,
    /// この手を選ぶまでに訪れたノード数。定石から返した手は 0。
    nodes: u64,
}

#[derive(Serialize, Clone)]
struct HintView {
    pos: u8,
    value: f32,
    exact: bool,
    /// 定石 book の値か (探索でなく)。
    from_book: bool,
    /// この値を出した探索の深さ (読み切り・定石は 0)。
    depth: u32,
}

#[derive(Serialize)]
struct EvalPoint {
    n: usize,
    /// 黒視点の石差。
    value: f32,
    exact: bool,
    /// 定石 book の値か (探索でなく)。
    from_book: bool,
}

/// 手順 (line) 上の n 手目直後の盤面。
///
/// 初期配置から再生してはいけない。GGS の抽選オープニングは開始局面が
/// 標準と違うので、そこから並べ直すと最初の手から打てなくなる。開始局面を
/// 持っていないので、undo/redo で目的の手数まで動かして読み、元へ戻す。
fn board_at_line(game: &mut Reversi, n: usize) -> Result<Board, String> {
    if n > game.line().len() {
        return Err("out of range".into());
    }
    let saved = game.move_count();
    let goto = |to: usize, game: &mut Reversi| -> Result<(), String> {
        while game.move_count() > to {
            game.undo().map_err(|e| format!("{e:?}"))?;
        }
        while game.move_count() < to {
            game.redo().map_err(|e| format!("{e:?}"))?;
        }
        Ok(())
    };
    goto(n, game)?;
    let b = game.board;
    goto(saved, game)?;
    Ok(b)
}

fn view(game: &Reversi) -> GameView {
    let b = &game.board;
    let mut cells = vec![0u8; 64];
    for i in 0..64u8 {
        let bit = 1u64 << i;
        if b.black & bit != 0 {
            cells[i as usize] = 1;
        } else if b.white & bit != 0 {
            cells[i as usize] = 2;
        }
    }
    let (black, white) = game.piece_count();
    GameView {
        cells,
        player: match game.player() {
            Color::Black => "black".into(),
            Color::White => "white".into(),
        },
        legal: game.movable_list().iter().map(|p| p.index()).collect(),
        black,
        white,
        over: game.is_game_over(),
        last: game.history.last().and_then(|r| r.pos).map(|p| p.index()),
        kifu: game.to_kifu(),
        move_count: game.move_count(),
        moves: game.line().iter().map(|p| p.map(|p| p.index())).collect(),
        cursor: game.move_count(),
    }
}

/// 手番側に合法手がなく対局も終わっていなければパスを消化する。
fn auto_pass(game: &mut Reversi) {
    while !game.is_game_over() && game.movable() == 0 {
        if game.pass().is_err() {
            break;
        }
    }
}

/// 設定ファイルの場所。OS の設定ディレクトリに置く (配布時はここしかない)。
fn resources_path() -> PathBuf {
    let base =
        dirs_config().unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/..")));
    base.join("kuroobi").join("resources.conf")
}

/// 取り込んだ対局の控え。定石ファイルの隣ではなく設定ディレクトリに置く —
/// 定石は差し替えられるが、「自分が何を取り込んだか」は差し替えても残って
/// ほしい記録なので、寿命の違うものを同じ場所に置かない。
fn learn_log_path() -> PathBuf {
    /* **場所を差し替えられるようにする** (`KUROOBI_LEARN_LOG`)。
    この画面は実データしか出ないので、控えが 1 つも無いときや期間で
    隠れたときの見た目を確かめる手が他にない。重みと定石は
    `resources.conf` で差し替えられるのに、控えだけ固定だった。 */
    if let Ok(p) = std::env::var("KUROOBI_LEARN_LOG") {
        return PathBuf::from(p);
    }
    let base =
        dirs_config().unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/..")));
    base.join("kuroobi").join("learn_log.jsonl")
}

/// $XDG_CONFIG_HOME / ~/Library/Application Support / %APPDATA%。
fn dirs_config() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var("HOME").ok()?;
    #[cfg(target_os = "macos")]
    return Some(PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(target_os = "macos"))]
    return Some(PathBuf::from(home).join(".config"));
}

fn resources() -> Resources {
    Resources::load(&resources_path())
}

/// エンジンが無ければ作る。Arc を受け取るのは、ワーカースレッド
/// (spawn_blocking) からも使えるようにするため — 同期コマンドは
/// メインスレッドで走るので、そこでエンジンのロックを待つと探索が
/// 終わるまで UI 全体が固まる。
fn ensure_engine_in(
    engine_slot: &Arc<Mutex<Option<Engine>>>,
    stop_slot: &Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
) -> Result<(), String> {
    let mut guard = engine_slot.lock().unwrap();
    if guard.is_none() {
        let res = resources();
        let cfg = EngineConfig {
            weights: res.weights_path(),
            nnue: res.nnue_path(),
            book: res.book_path(),
            threads: res.threads.unwrap_or_else(auto_threads),
            ..Default::default()
        };
        let engine = Engine::new(cfg).map_err(setup_error)?;
        *stop_slot.lock().unwrap() = Some(engine.stop_handle());
        *guard = Some(engine);
    }
    Ok(())
}

/// エンジンを用意できなかった理由を、画面に出す言葉へ直す。
///
/// ライブラリ側の文言は CLI と共用なので `nnue <path>: …` のように
/// 内部の名前と英語のまま来る。**トーストに内部符丁と直し方の無い文は
/// 出さない** (デザイン規則 34 / 61) ので、ここで言い換える。
fn setup_error(e: String) -> String {
    let what = if e.starts_with("nnue ") {
        "NNUE の重み"
    } else if e.starts_with("weights ") {
        "線形評価の重み"
    } else {
        return e;
    };
    // "nnue /path/to/x.bin: No such file or directory (os error 2)"
    let path = e
        .split_once(' ')
        .and_then(|(_, rest)| rest.split_once(": "))
        .map(|(p, _)| p)
        .unwrap_or("");
    format!("{what}を読み込めません ({path})。設定でファイルの場所を指定してください。")
}

fn ensure_engine(app: &State<App>) -> Result<(), String> {
    ensure_engine_in(&app.engine, &app.stop)
}

#[tauri::command]
fn state(app: State<App>) -> GameView {
    view(&app.game.lock().unwrap())
}

#[tauri::command]
fn new_game(app: State<App>) -> GameView {
    let mut game = app.game.lock().unwrap();
    *game = Reversi::new();
    view(&game)
}

#[tauri::command]
fn play(app: State<App>, sq: u8) -> Result<GameView, String> {
    let mut game = app.game.lock().unwrap();
    let pos = Position::from_index(sq as u32).ok_or("bad square")?;
    game.make_move(pos).map_err(|e| format!("{e:?}"))?;
    auto_pass(&mut game);
    Ok(view(&game))
}

#[tauri::command]
fn undo(app: State<App>) -> Result<GameView, String> {
    let mut game = app.game.lock().unwrap();
    // パスも 1 手として積まれているので、直前の「石を置いた手」まで戻す
    loop {
        game.undo().map_err(|e| format!("{e:?}"))?;
        let placed = game.history.last().map(|r| r.pos.is_some());
        if game.history.is_empty() || placed != Some(false) {
            break;
        }
    }
    Ok(view(&game))
}

/// 手順上の n 手目の直後へ移動する (undo/redo で辿るので前後どちらへも動ける)。
#[tauri::command]
fn goto(app: State<App>, n: usize) -> Result<GameView, String> {
    let mut game = app.game.lock().unwrap();
    if n > game.line().len() {
        return Err("out of range".into());
    }
    while game.move_count() > n {
        game.undo().map_err(|e| format!("{e:?}"))?;
    }
    while game.move_count() < n {
        game.redo().map_err(|e| format!("{e:?}"))?;
    }
    Ok(view(&game))
}

/// 進行中の探索を中断する (思考・解析の両方)。結果はフロントが捨てる。
#[tauri::command]
fn stop_search(app: State<App>) -> Result<(), String> {
    if let Some(h) = app.stop.lock().unwrap().as_ref() {
        h.stop();
    }
    Ok(())
}

/// 定石 book を使うかどうか。研究中は切って自力の手を見たいことがある。
/// async + spawn_blocking なのはロック待ちを UI から切り離すため
/// (同期コマンドはメインスレッドで走るので、探索がロックを持っていると
/// 解放まで画面ごと固まる)。
#[tauri::command]
async fn set_use_book(app: State<'_, App>, on: bool) -> Result<(), String> {
    let (eng, stop) = (app.engine.clone(), app.stop.clone());
    tauri::async_runtime::spawn_blocking(move || {
        ensure_engine_in(&eng, &stop)?;
        eng.lock()
            .unwrap()
            .as_mut()
            .ok_or("engine")?
            .set_use_book(on);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 動作確認用: 起動直後に自動で始めたいこと (KUROOBI_AUTOPLAY)。
/// 空なら何もしない。"vs" で対局、"both" でエンジン同士。":<レベル番号>"
/// を付けると強さも指定できる (例: "both:11")。
#[tauri::command]
fn autoplay() -> String {
    std::env::var("KUROOBI_AUTOPLAY").unwrap_or_default()
}

/// 画面確認用: テーマを固定する (`KUROOBI_THEME=light` / `dark`)。
///
/// テーマの切り替えは 設定 → 表示 のタブにしかなく、撮るたびに人が押す
/// しかなかった。ライトの確認が後回しになる原因だったので、環境変数で
/// 指定できるようにする。**保存はしない** — その起動の間だけ効く。
#[tauri::command]
fn theme_override() -> String {
    std::env::var("KUROOBI_THEME").unwrap_or_default()
}

/// book を使えるか (画面の表示に使う)。エンジンの初期化は重いので、
/// ファイルの有無だけで答える。
#[tauri::command]
fn has_book() -> bool {
    resources().book_path().exists()
}

/// ローカル対局を定石の学習に取り込むかどうか。
#[tauri::command]
fn set_learn(app: State<App>, on: bool) {
    *app.learn_on.lock().unwrap() = on;
}

/// 取り込んだ対局 1 件。時刻は unix 秒のまま持つ — 書式は見る側の
/// 時間帯と暦で決まるので、記録側で文字列にしてしまうと直せない。
#[derive(Serialize, Deserialize, Clone)]
pub struct LearnEntry {
    pub at: u64,
    pub kifu: String,
    pub black: u8,
    pub white: u8,
    /// 書き戻した局面数。途中で諦めた取り込みは実際に進んだぶんだけになる。
    pub positions: u32,
    /// 抽選開局の開始局面 (盤面文字列)。標準の初期局面なら空。
    /// これが無いと、抽選開局の対局を後から開いても別の対局になる。
    #[serde(default)]
    pub start: String,
    /// 書き換えの明細。何がどう変わったかが残っていないと、変な対局が
    /// 混ざったときに見つけることも戻すこともできない。
    #[serde(default)]
    pub changes: Vec<LearnChange>,
    /// GGS の対局なら相手の名前。ローカル対局は空。
    /// **古い控えには無い項目なので既定を持たせる** — 無いと過去の行が
    /// 丸ごと読めなくなる。
    #[serde(default)]
    pub opponent: String,
    /// 自分がどちらの色だったか (`"b"` / `"w"`)。**これが無いと石数だけでは
    /// 勝敗を判定できない** — 学習ログの「負けた対局」の絞り込みが作れない
    /// (依頼 5-9)。古い控えには無いので既定は空で、そのときは絞り込みから
    /// 外れる。
    #[serde(default)]
    pub my_color: String,
}

/// 定石を 1 手ぶん書き換えた記録。
#[derive(Serialize, Deserialize, Clone)]
pub struct LearnChange {
    /// 棋譜の何手目か (パスを除いた 1 始まり)。
    pub ply: usize,
    pub mv: String,
    /// 上書き前の値。定石に無かった手なら null。
    pub before: Option<f32>,
    pub after: f32,
    /// 書き換えたあとのその局面の最善値。`best - after` が損した石差。
    pub best: f32,
    /// この取り込みで学習分に新しく作った局面か。
    #[serde(default)]
    pub new_entry: bool,
}

impl LearnChange {
    pub fn of(c: &kuroobi::learn::BackupChange) -> LearnChange {
        LearnChange {
            ply: c.ply,
            mv: c.mv.to_kifu(),
            before: c.before,
            after: c.after,
            best: c.best,
            new_entry: c.new_entry,
        }
    }
}

/// いまの unix 秒。控えの時刻に使う。
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 1 行 1 件で追記する。読みながら書いても壊れないので、途中で落ちても
/// それまでの記録は残る (途中の 1 行だけが読めなくなる)。
pub fn learn_log_append(e: &LearnEntry) {
    let path = learn_log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(line) = serde_json::to_string(e) else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
    trim_learn_log(&path);
}

/// 控えの上限。書き換えの明細を持つので 1 行が 3〜4 KB になる。
/// 200 件で 1 MB 弱に収まり、画面に出す件数とも一致する。
const LEARN_LOG_MAX: usize = 200;

/// 伸びすぎた控えを新しい側だけ残して書き直す。
/// 毎回書き直すと追記の意味が無いので、上限を超えたときだけ。
fn trim_learn_log(path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= LEARN_LOG_MAX + 100 {
        return;
    }
    let keep = lines[lines.len() - LEARN_LOG_MAX..].join("\n");
    let _ = std::fs::write(path, keep + "\n");
}

/// 取り込んだ対局の控え (新しい順)。読めない行は飛ばす。
#[tauri::command]
fn learn_log() -> Vec<LearnEntry> {
    let Ok(text) = std::fs::read_to_string(learn_log_path()) else {
        return Vec::new();
    };
    let mut out: Vec<LearnEntry> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    out.reverse();
    // 画面に出すのは直近だけ。全部返すと、長く使うほど起動のたびに重くなる
    out.truncate(200);
    out
}

/// 取り込みを 1 局ぶん取り消す。控えの行 (`at` と棋譜で 1 件に決まる) を
/// 探し、定石を戻してから控えからも消す。
///
/// 控えを先に消さない — 定石を戻せなかったのに記録だけ消えると、
/// 「取り消したはずなのに残っている」を確かめる手立てが無くなる。
#[tauri::command]
async fn learn_undo(app: State<'_, App>, at: u64, kifu: String) -> Result<usize, String> {
    let log = learn_log();
    let e = log
        .into_iter()
        .find(|e| e.at == at && e.kifu == kifu)
        .ok_or("その対局は控えにありません")?;
    if e.changes.is_empty() {
        return Err("書き換えの明細が無いので戻せません".into());
    }
    let changes: Vec<kuroobi::learn::BackupChange> = e
        .changes
        .iter()
        .map(|c| {
            Ok(kuroobi::learn::BackupChange {
                ply: c.ply,
                mv: Position::from_kifu(&c.mv).map_err(|x| x.to_string())?,
                before: c.before,
                after: c.after,
                best: c.best,
                new_entry: c.new_entry,
            })
        })
        .collect::<Result<_, String>>()?;
    let eng = app.engine.clone();
    let stop = app.stop.clone();
    let start = e.start.clone();
    let n = tauri::async_runtime::spawn_blocking(move || {
        ensure_engine_in(&eng, &stop)?;
        let mut guard = eng.lock().unwrap();
        let engine = guard.as_mut().ok_or("エンジンがまだありません")?;
        engine.undo_learn(
            (!start.is_empty()).then_some(start.as_str()),
            &kifu,
            &changes,
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    learn_log_remove(at, &e.kifu);
    Ok(n)
}

/// 控えから 1 件消す (取り消したあと)。
fn learn_log_remove(at: u64, kifu: &str) {
    let path = learn_log_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| {
            serde_json::from_str::<LearnEntry>(l)
                .map(|e| !(e.at == at && e.kifu == kifu))
                .unwrap_or(true)
        })
        .collect();
    let _ = std::fs::write(&path, kept.join("\n") + "\n");
}

/// 終局したローカル対局を定石の学習に取り込む (learn.rs)。
/// フロントが「手が指されて終局した」ときに呼ぶ。読み込んだだけの棋譜は
/// 対象にしない。取り込みは裏で 1 探索ずつ進み、エンジンのロックを
/// 探索ごとに手放すので、途中で思考を始めても 1 探索ぶんしか待たない。
#[tauri::command]
/// `my_color` は**人**がどちらの色だったか (`"b"` / `"w"`)。
/// **画面しか知らない** — KUROOBI の担当は 黒 / 白 / 両方 / なし から
/// 選べるので、その裏が人の色になる。控えに残さないと、あとから石数だけを
/// 見ても勝敗が決まらない (依頼 5-9)。決められないときは空。
fn learn_game(app: State<App>, my_color: String) -> Result<(), String> {
    if !*app.learn_on.lock().unwrap() {
        return Ok(());
    }
    let (kifu, board) = {
        let game = app.game.lock().unwrap();
        if !game.is_game_over() {
            return Err("終局していません".into());
        }
        (game.to_kifu(), game.board)
    };
    // 標準初期局面から再生できて最終盤面が一致する対局だけを取り込む
    // (読み込んだ抽選開局の対局などを誤って学習しないため)
    let (_, fin) = kuroobi::learn::replay(None, &kifu)?;
    if fin.black != board.black || fin.white != board.white {
        return Err("初期局面から始まった対局ではないため取り込みません".into());
    }
    let eng = app.engine.clone();
    let stop = app.stop.clone();
    let act = app.activity.clone();
    let ggs_snap = ggs_snap_arc(&app);
    tauri::async_runtime::spawn_blocking(move || {
        // エンジンの用意もここで行う (同期コマンド内でロックを待つと
        // メインスレッドごと固まるため)
        if ensure_engine_in(&eng, &stop).is_err() {
            return;
        }
        let mut job = {
            let mut guard = eng.lock().unwrap();
            let Some(engine) = guard.as_mut() else { return };
            match engine.learn_start(None, &kifu, ggs::LEARN_DEPTH) {
                Ok(j) => j,
                Err(_) => return,
            }
        };
        let total = job.remaining() as u32;
        // 終わるまで空。途中で諦めた取り込みは明細を残さない
        let mut changes: Vec<kuroobi::learn::BackupChange> = Vec::new();
        /* 直近で譲った時刻。**譲りは一瞬ずつ何度も起きる。**
        分析は 1 局面ごとに探索の印を立てて外すので、その瞬間だけを
        見ていると譲っている間隔が数十ミリ秒しか続かない。画面は
        1 秒ごとにしか状態を取らないので、**実際には何度も譲っている
        のに「譲り中」が一度も出ない**。少しのあいだ立てたままにして、
        人の時間の刻みで正しくする (譲ったこと自体は本当に起きている)。 */
        let mut last_yield: Option<std::time::Instant> = None;
        const YIELD_HOLD: std::time::Duration = std::time::Duration::from_millis(1500);
        loop {
            // 対局・検討・GGS 対局が動いている間は譲って止まる
            // (裏の学習が CPU を奪わない)。空いたら続きから再開する
            let busy = act.lock().unwrap().local.is_some() || ggs_match_in(&ggs_snap);
            if busy {
                last_yield = Some(std::time::Instant::now());
            }
            let recently = last_yield.is_some_and(|t| t.elapsed() < YIELD_HOLD);
            {
                let mut a = act.lock().unwrap();
                a.learn_paused = busy || recently;
                a.learn = Some((total.saturating_sub(job.remaining() as u32), total));
            }
            if busy {
                std::thread::sleep(std::time::Duration::from_millis(300));
                continue;
            }
            let step = {
                let mut guard = eng.lock().unwrap();
                // 設定変更でエンジンが作り直されたら、この取り込みは諦める
                let Some(engine) = guard.as_mut() else { break };
                engine.learn_step(&mut job, ggs::LEARN_DEPTH)
            };
            match step {
                Ok(None) => {}
                Ok(Some(out)) => {
                    changes = out.changes;
                    break;
                }
                Err(_) => break,
            }
            // ロックを取り直す前に一拍置く。すぐ取り直すと (Mutex は待った
            // 順に渡るとは限らない)、強さ変更や思考の開始が何探索ぶんも
            // 割り込めないことがある
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // 控えは終わってから 1 行だけ書く。途中で諦めた取り込みも、進んだ
        // ぶんだけを記録する (書き戻し自体はそこまで済んでいるため)
        learn_log_append(&LearnEntry {
            at: now_secs(),
            kifu,
            black: board.black.count_ones() as u8,
            white: board.white.count_ones() as u8,
            positions: total.saturating_sub(job.remaining() as u32),
            // ローカル対局は初期局面から始まったものだけを取り込む
            start: String::new(),
            changes: changes.iter().map(LearnChange::of).collect(),
            opponent: String::new(),
            my_color: my_color.clone(),
        });
        let mut a = act.lock().unwrap();
        a.learn = None;
        a.learn_paused = false;
    });
    Ok(())
}

/// スレッド数の設定 (ローカルのエンジン)。set が None なら自動。
#[derive(Serialize)]
struct ThreadsView {
    set: Option<u32>,
    auto: u32,
}

#[tauri::command]
fn local_threads() -> ThreadsView {
    ThreadsView {
        set: resources().threads.map(|n| n as u32),
        auto: auto_threads() as u32,
    }
}

/// ローカルのスレッド数を設定する (None で自動へ戻す)。resources.conf に
/// 保存し、既存エンジンには次の探索から効かせる。
#[tauri::command]
async fn set_local_threads(app: State<'_, App>, n: Option<u32>) -> Result<(), String> {
    let mut r = resources();
    r.threads = n.map(|v| v.clamp(1, 64) as usize);
    r.save(&resources_path())?;
    let threads = r.threads.unwrap_or_else(auto_threads);
    let eng = app.engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(e) = eng.lock().unwrap().as_mut() {
            e.set_threads(threads);
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// いま何が CPU を使っているか (ナビの常時表示用)。
#[derive(Serialize)]
struct ActivityView {
    /// ローカル探索の種別 (思考 / 解析 / 分析)。無ければ null。
    local: Option<String>,
    local_threads: u32,
    /// 学習の取り込み (済み, 総数)。
    learn: Option<(u32, u32)>,
    learn_paused: bool,
    /// GGS の自分の対局が進行中か。
    ggs_match: bool,
    ggs_thinking: bool,
    ggs_threads: u32,
    /// プロセス全体の CPU 使用率 (%)。100% = 1 コア相当。
    cpu: f32,
    /// マシンのコア数 (使用率の上限 = cores × 100%)。
    cores: u32,
    /// 使用中の物理メモリと、積んでいる総量 (バイト)。
    mem: u64,
    mem_total: u64,
}

#[tauri::command]
fn activity_status(app: State<App>) -> ActivityView {
    // 前回の呼び出しからの差分で CPU 使用率を出す (呼び出しは 1 秒間隔)
    let cpu = {
        let now = (std::time::Instant::now(), process_cpu_time());
        let mut meter = app.cpu_meter.lock().unwrap();
        let pct = meter.take().map_or(0.0, |(t0, c0)| {
            let wall = now.0.duration_since(t0).as_secs_f32();
            if wall > 0.05 {
                (now.1.saturating_sub(c0).as_secs_f32() / wall) * 100.0
            } else {
                0.0
            }
        });
        *meter = Some(now);
        pct
    };
    let (ggs_match, ggs_thinking, ggs_threads) = ggs_snap_arc(&app)
        .map(|s| {
            let s = s.lock().unwrap();
            (
                s.matches.iter().any(|m| !m.my_color.is_empty() && !m.over),
                s.thinking.is_some(),
                s.engine.threads as u32,
            )
        })
        .unwrap_or((false, false, 0));
    let a = app.activity.lock().unwrap();
    ActivityView {
        local: a.local.map(String::from),
        local_threads: resources().threads.unwrap_or_else(auto_threads) as u32,
        learn: a.learn,
        learn_paused: a.learn_paused,
        ggs_match,
        ggs_thinking,
        ggs_threads,
        cpu,
        cores: std::thread::available_parallelism().map_or(1, |n| n.get()) as u32,
        mem: process_memory(),
        mem_total: total_memory(),
    }
}

/// 使うファイルの一覧 (名前・パス・見つかったか・大きさ・中身の見分け)。
///
/// 大きさと形式まで返すのは、**「ある」だけでは足りない**から。重みは
/// 差し替えて使うものなので、いま読んでいるのがどれか分からないと、
/// 指し手が変わった理由を追えない。
#[tauri::command]
fn resource_status() -> Vec<(String, String, bool, u64, String)> {
    resources()
        .detailed()
        .into_iter()
        .map(|(n, p, ok, size, kind)| (n.to_string(), p.display().to_string(), ok, size, kind))
        .collect()
}

/* 付属ウィンドウは廃止した (2026-08-08)。設定は主画面の覆いに戻っている。
 * 窓である値打ちは「表示」タブだけが持っていたのに、1240 幅の主画面では
 * 盤に重なるのを避けられず (置き場所を直しても 12%)、そのために
 * localStorage 越しの同期・窓の札・置き場所の計算まで抱えていた。
 * 窓が要るようになったら `WebviewWindowBuilder` で書き直す。 */

/// ファイル選択ダイアログを開く。`kind` に応じて絞り込む。
#[tauri::command]
async fn pick_resource(handle: tauri::AppHandle, kind: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let d = handle.dialog().file();
    let picked = match kind.as_str() {
        "dir" => d.blocking_pick_folder().map(|p| p.to_string()),
        "book" => d
            .add_filter("定石 book", &["txt"])
            .blocking_pick_file()
            .map(|p| p.to_string()),
        _ => d
            .add_filter("重み", &["bin"])
            .blocking_pick_file()
            .map(|p| p.to_string()),
    };
    Ok(picked)
}

/// 使うファイルを選び直す。`kind` は "dir" | "weights" | "nnue" | "book"。
/// path が null なら指定を外して既定に戻す。エンジンは次回の思考で作り直す。
#[tauri::command]
async fn set_resource(
    app: State<'_, App>,
    kind: String,
    path: Option<String>,
) -> Result<(), String> {
    let mut r = resources();
    let p = path.map(PathBuf::from);
    match kind.as_str() {
        "dir" => r.dir = p,
        "weights" => r.weights = p,
        "nnue" => r.nnue = p,
        "book" => r.book = p,
        other => return Err(format!("unknown resource: {other}")),
    }
    r.save(&resources_path())?;
    // 読み直しは次のエンジン生成から。今のエンジンは捨てる。捨てるにも
    // ロックが要るので、探索中でも UI が固まらないようワーカーで待つ。
    let (eng, stop) = (app.engine.clone(), app.stop.clone());
    tauri::async_runtime::spawn_blocking(move || {
        *eng.lock().unwrap() = None;
        *stop.lock().unwrap() = None;
    })
    .await
    .map_err(|e| e.to_string())
}

/// 強さの変更。set_use_book と同じ理由で async + spawn_blocking。
/// 進行中の探索はそのまま走り切り、変更は次の探索から効く。
#[tauri::command]
async fn set_levels(
    app: State<'_, App>,
    depth: u32,
    solve_empties: u8,
    band: u8,
) -> Result<(), String> {
    let (eng, stop) = (app.engine.clone(), app.stop.clone());
    tauri::async_runtime::spawn_blocking(move || {
        ensure_engine_in(&eng, &stop)?;
        eng.lock()
            .unwrap()
            .as_mut()
            .ok_or("engine")?
            .set_levels(depth, solve_empties, band);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 現局面の最善手を計算する。**局面は動かさない** — 適用は `apply_move`。
/// 分離してあるので、思考中に停止された場合はフロントが結果を捨てるだけで
/// 「停止」が成立する。
#[tauri::command]
async fn think(app: State<'_, App>) -> Result<ThinkView, String> {
    // GGS の対局が最優先。進行中はローカルの探索を始めない
    if ggs_match_active(&app) {
        return Err("GGS 対局中はローカルの探索を控えます (終局までお待ちください)".into());
    }
    ensure_engine(&app)?;
    let board = app.game.lock().unwrap().board;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let stop = app.stop.clone();
    let (mv, secs, nodes, aborted) = tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "思考");
        let mut guard = eng.lock().unwrap();
        let t0 = std::time::Instant::now();
        // ノードは累計で持っているので、この 1 手ぶんは前後の差で取る
        let n0 = guard.as_ref().unwrap().nodes();
        let mv = guard.as_mut().unwrap().choose(&board);
        let nodes = guard.as_ref().unwrap().nodes() - n0;
        // 停止された探索の値は不完全 (番兵が混じる)。呼び出し元へ返さない。
        // ただし判定するのは**探索したときだけ** — 定石から返した手は探索を
        // 呼ばないのでフラグを消す機会がなく、前に立った停止が残っていると
        // 正しい手まで捨ててしまう (分析側で踏んだのと同じ形)
        let aborted = nodes > 0
            && stop
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|h| h.is_stopped());
        (mv, t0.elapsed().as_secs_f32(), nodes, aborted)
    })
    .await
    .map_err(|e| e.to_string())?;
    if aborted {
        return Err("stopped".into());
    }
    // 思考中に局面が動いていたら無効 (フロント側でも照合する)
    if !same_board(&app.game.lock().unwrap().board, &board) {
        return Err("position changed".into());
    }
    Ok(ThinkView {
        pos: mv.pos.map(|p| p.index()),
        value: finite(mv.value),
        exact: mv.exact,
        from_book: mv.from_book,
        learned: mv.learned,
        secs,
        nodes,
    })
}

/// think の結果 (または任意の手) を現局面に適用する。sq が null ならパス。
#[tauri::command]
fn apply_move(app: State<App>, sq: Option<u8>) -> Result<GameView, String> {
    let mut game = app.game.lock().unwrap();
    match sq {
        Some(s) => {
            let pos = Position::from_index(s as u32).ok_or("bad square")?;
            game.make_move(pos).map_err(|e| format!("{e:?}"))?;
        }
        None => game.pass().map_err(|e| format!("{e:?}"))?,
    }
    auto_pass(&mut game);
    Ok(view(&game))
}

/// 人の手番のあいだ、相手 (人) が指すと思う手の先を裏で読んでおく。
///
/// **ローカル対局は深さ固定**なので、効き方は「深く」ではなく「速く」。
/// 実測で同じ深さへ **1/3 の時間**で着く (深さ 12 / 先読み 300ms / −62〜65%)。
/// KUROOBI の応答が速くなるだけで、手の質は変わらない。
///
/// `analyze_live` (評価値の表示) とは**同じエンジンを取り合う**ので、
/// 両方は走らせない。画面側が評価値を出していないときだけ呼ぶ。
///
/// 読む深さに達したら自分で止まるので、人が長考しても回し続けない。
#[tauri::command]
async fn ponder_live(app: State<'_, App>) -> Result<(), String> {
    if ggs_match_active(&app) {
        return Err("GGS 対局中は控えます".into());
    }
    ensure_engine(&app)?;
    let board = app.game.lock().unwrap().board;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let stop = app.stop.clone();
    if let Some(h) = stop.lock().unwrap().as_ref() {
        h.reset();
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "先読み");
        let mut guard = eng.lock().unwrap();
        let Some(e) = guard.as_mut() else { return };
        /* 上限は 60 秒。**読む深さに達すれば自分で止まる**ので普段は
        使われないが、人が席を外したときに回し続けないための蓋 */
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        e.ponder(&board, until);
    });
    Ok(())
}

/// 反復深化しながら評価値を出し続ける。段が終わるたびに画面へ送るので、
/// 見ている間じわじわ深くなる。局面を変えるか止めるまで続く。
#[tauri::command]
async fn analyze_live(app: State<'_, App>, handle: tauri::AppHandle) -> Result<(), String> {
    if ggs_match_active(&app) {
        return Err("GGS 対局中は分析を控えます".into());
    }
    ensure_engine(&app)?;
    let board = app.game.lock().unwrap().board;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let stop = app.stop.clone();
    if let Some(h) = stop.lock().unwrap().as_ref() {
        h.reset();
    }
    tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "分析");
        let mut guard = eng.lock().unwrap();
        let Some(e) = guard.as_mut() else { return };
        // 分析では定石を引かない。定石の値は「どこかの時点の探索結果」で、
        // 深めている探索値と混ざると手どうしを比べられなくなる
        let t0 = std::time::Instant::now();
        e.analyze_deepening(&board, 1, |depth, hints, nodes| {
            let view: Vec<HintView> = hints
                .iter()
                .map(|(p, ev)| HintView {
                    pos: p.index(),
                    value: finite(ev.value),
                    exact: ev.exact,
                    from_book: false,
                    depth: ev.depth,
                })
                .collect();
            // 働きぶり (ノード数と経過) も一緒に送る。速さは画面側で割る
            handle
                .emit("hints", (depth, view, nodes, t0.elapsed().as_secs_f32()))
                .is_ok()
        });
    });
    Ok(())
}

/// 手順上の n 手目直後の局面を固定深さで評価する (評価値グラフ用)。
/// 返す値は黒視点。
#[tauri::command]
async fn eval_at(app: State<'_, App>, n: usize, depth: u32) -> Result<EvalPoint, String> {
    if ggs_match_active(&app) {
        return Err("GGS 対局中は分析を控えます".into());
    }
    ensure_engine(&app)?;
    let board = board_at_line(&mut app.game.lock().unwrap(), n)?;
    let white = board.player() == Color::White;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let stop = app.stop.clone();
    // 前の停止を引きずらない。定石に当たった局面は探索を呼ばず、探索側の
    // リセットも走らないので、ここで消しておかないと以降ずっと「停止済み」
    // と誤判定して分析が 1 局面も進まなくなる
    if let Some(h) = stop.lock().unwrap().as_ref() {
        h.reset();
    }
    let (value, exact, from_book, searched) = tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "分析");
        let mut guard = eng.lock().unwrap();
        let e = guard.as_mut().unwrap();
        // **定石は引かない。** 分析は「いまの強さ設定で測り直す」もので、
        // 定石の値は「どこかの時点の探索結果」。混ぜると局面どうしを比べ
        // られなくなる (盤の評価値表示も同じ理由で引いていない)。
        // 実害も出ていた: 再計算中の定石は未評価 (0.000) をそのまま返し、
        // 実戦から学習した分は終局石差を根まで書き戻した値で、どちらも
        // 「その局面の評価」ではないのにグラフへ出ていた。
        // **値は必ず探索で出す** (上の理由)。ただし「その局面が定石にあるか」は
        // 別の話で、規則 19 はグラフの点を 定石 / 読切 / 探索 の 3 色で塗り
        // 分けると決めている。出所を見ずに一律 false を返していたので、定石を
        // 「使う」にしていても金の点が 1 つも出なかった (ユーザー指摘)。
        // 値は探索のまま、**印だけ立てる** — こうすれば序盤のどこまでが定石で
        // 覆われているかが読め、未評価の 0.000 や書き戻した終局石差を
        // グラフへ持ち込む害も起きない。
        let in_book = e.book_node(&board).is_some_and(|(mv, _)| !mv.is_empty());
        let mv = e.eval_position(&board, depth);
        (mv.value, mv.exact, in_book, true)
    })
    .await
    .map_err(|e| e.to_string())?;
    // 停止された探索の値は不完全なので返さない (グラフに残ってしまう)。
    // 探索していない (定石) ときは見ない
    if searched
        && stop
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|h| h.is_stopped())
    {
        return Err("stopped".into());
    }
    let value = finite(if white { -value } else { value });
    Ok(EvalPoint {
        n,
        value,
        exact,
        from_book,
    })
}

#[tauri::command]
async fn save_kifu(
    app: State<'_, App>,
    handle: tauri::AppHandle,
    black: Option<String>,
    white: Option<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (kifu, ggf) = {
        let game = app.game.lock().unwrap();
        (
            game.to_kifu(),
            to_ggf(
                &game,
                black.as_deref().unwrap_or("Black"),
                white.as_deref().unwrap_or("White"),
            ),
        )
    };
    if kifu.is_empty() {
        return Err("棋譜が空です".into());
    }
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter("棋譜", &["ggf", "txt", "kifu"])
        .set_file_name("kuroobi_game.ggf")
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let p = path.into_path().map_err(|e| e.to_string())?;
    // 書き分けは拡張子で決める。保存の窓で形式をもう一度聞くと、名前を
    // 決めたばかりの人に同じことを二度考えさせることになる
    let ggf_out = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ggf"));
    let body = if ggf_out { ggf } else { format!("{kifu}\n") };
    std::fs::write(&p, body).map_err(|e| e.to_string())?;
    Ok(Some(p.display().to_string()))
}

/* ---------------- GGF (他のソフトへ渡す形) ---------------- */

/// 対局を GGF 1 局ぶんにする。
///
/// f5 の並びだけの形と違い、**どちらがどの色か・結果・開始局面**が入る。
/// 抽選開局から始めた対局や、パスの入った対局を渡せるのはこちらだけ。
fn to_ggf(game: &Reversi, black: &str, white: &str) -> String {
    let start = start_board(game);
    let mut out = String::from("(;GM[Othello]PC[KUROOBI]");
    out.push_str(&format!("DT[{}]", ggf_now()));
    out.push_str(&format!("PB[{}]PW[{}]", ggf_text(black), ggf_text(white)));
    // 結果は黒から見た石差。終局していない対局は「?」— 0 と書くと
    // 引き分けで終わったことになってしまう
    if game.is_game_over() {
        let d = game.board.black.count_ones() as i32 - game.board.white.count_ones() as i32;
        out.push_str(&format!("RE[{d:+}]"));
    } else {
        out.push_str("RE[?]");
    }
    out.push_str("TI[0]TY[8]");
    out.push_str(&format!(
        "BO[8 {} {}]",
        ggf_squares(&start),
        if start.player == Color::Black {
            "*"
        } else {
            "O"
        }
    ));
    let mut color = start.player;
    for r in &game.history {
        let tag = if color == Color::Black { "B" } else { "W" };
        match r.pos {
            // パスも残す。落とすと手番がずれた棋譜になる
            None => out.push_str(&format!("{tag}[PA]")),
            Some(p) => out.push_str(&format!("{}[{}]", tag, p.to_kifu().to_uppercase())),
        }
        color = color.opponent();
    }
    out.push_str(";)\n");
    out
}

/// 開始局面。いまの盤から着手を巻き戻して作る (開始局面そのものは
/// 持っていないため)。返した石は `flipped` に残っているので厳密に戻せる。
fn start_board(game: &Reversi) -> Board {
    let mut b = game.board;
    for r in game.history.iter().rev() {
        b.player = b.player.opponent();
        if let Some(p) = r.pos {
            let bit = 1u64 << p.index();
            let (mine, theirs) = if b.player == Color::Black {
                (&mut b.black, &mut b.white)
            } else {
                (&mut b.white, &mut b.black)
            };
            *mine &= !(bit | r.flipped);
            *theirs |= r.flipped;
        }
    }
    b.empty_count = (!(b.black | b.white)).count_ones() as u8;
    b
}

/// 64 マスを GGF の並び (a1..h1, a2..h2, …) で書く。
fn ggf_squares(b: &Board) -> String {
    let mut s = String::with_capacity(64);
    for rank in 0..8 {
        for file in 0..8 {
            let bit = 1u64 << (file * 8 + rank);
            s.push(if b.black & bit != 0 {
                '*'
            } else if b.white & bit != 0 {
                'O'
            } else {
                '-'
            });
        }
    }
    s
}

/// `]` はタグの終わりなので入れられない。他のソフトが読めなくなる。
fn ggf_text(s: &str) -> String {
    s.replace(']', ")")
}

/// GGF の DT。UTC で「YYYY-MM-DD HH:MM:SS GMT」。
/// 暦は自前で出す — この 1 か所のために依存を 1 つ増やしたくない。
fn ggf_now() -> String {
    let secs = now_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant の civil_from_days。1970-01-01 からの日数を暦に直す
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} GMT",
        y,
        m,
        d,
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// 貼り付け・ファイル内容から f5 形式の着手列だけを抽出する。
/// 手番号 (`12.`) や空白・区切り文字が混ざっていても「英字 a-h + 数字 1-8」の
/// ペアだけを拾うので壊れない。
/// テキストから開始局面を拾う。
///
/// 初期局面から始まらない対局 (GGS の抽選オープニング) は、着手列だけでは
/// 再生できない。盤面 64 マス + 手番の 1 行、または GGF の `BO[8 ...]` を
/// 開始局面として受け取る。見つからなければ初期局面から始める。
fn extract_start(text: &str) -> Option<String> {
    let cell = |c: char| matches!(c.to_ascii_lowercase(), '-' | '.' | 'x' | 'o' | '*');
    let side = |c: char| matches!(c.to_ascii_lowercase(), 'x' | 'o' | '*');
    // GGF は BO[8 <64 マス> <手番>] に開始局面を持つ
    let ggf = text.find("BO[").map(|i| &text[i + 3..]).and_then(|rest| {
        let end = rest.find(']')?;
        let inner = rest[..end].trim_start_matches('8').trim();
        Some(inner.to_string())
    });
    for cand in ggf.into_iter().chain(text.lines().map(|l| l.to_string())) {
        let c: Vec<char> = cand.chars().filter(|c| !c.is_whitespace()).collect();
        if c.len() == 65 && c[..64].iter().all(|&x| cell(x)) && side(c[64]) {
            // Board::from_string は 'x'/'*' を黒、'o' を白として読む
            return Some(c.into_iter().collect());
        }
    }
    None
}

/// GGF (Generic Game Format) を読む。
///
/// GGS が使う形式で、`(;` と `;)` で 1 局を囲み、`BO[8 <64 マス> <手番>]` に
/// 開始局面、`B[F5/評価/時間]` / `W[D6//時間]` に着手を持つ。座標を本文から
/// 拾うやり方だと `PB[player1]` のような名前から誤って手を作ってしまうので、
/// 着手タグだけを見る。
fn parse_ggf(text: &str) -> Option<(Option<String>, String)> {
    let body = {
        let start = text.find("(;")?;
        let rest = &text[start + 2..];
        let end = rest.find(";)").unwrap_or(rest.len());
        &rest[..end]
    };
    // タグを順に読む: 大文字の名前 + [ ... ]
    let mut start_pos: Option<String> = None;
    let mut kifu = String::new();
    let bytes: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_uppercase() {
            i += 1;
            continue;
        }
        let name_start = i;
        while i < bytes.len() && bytes[i].is_ascii_uppercase() {
            i += 1;
        }
        let name: String = bytes[name_start..i].iter().collect();
        if i >= bytes.len() || bytes[i] != '[' {
            continue;
        }
        i += 1;
        let val_start = i;
        while i < bytes.len() && bytes[i] != ']' {
            i += 1;
        }
        let value: String = bytes[val_start..i].iter().collect();
        i += 1;
        match name.as_str() {
            "BO" => {
                // "8 <64 マス> <手番>" — 盤サイズを落として詰める
                let c: Vec<char> = value
                    .trim_start_matches('8')
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                if c.len() == 65 {
                    start_pos = Some(c.into_iter().collect());
                }
            }
            "B" | "W" => {
                // "F5/評価/時間"。パスは PA / PASS で、棋譜には残さない
                // (再生側が打てない手番を見て自動でパスする)。
                let mv = value.split('/').next().unwrap_or("").trim().to_lowercase();
                if mv.len() == 2 && mv != "pa" {
                    kifu.push_str(&mv);
                }
            }
            _ => {}
        }
    }
    (start_pos.is_some() || !kifu.is_empty()).then_some((start_pos, kifu))
}

fn extract_kifu(text: &str) -> String {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let mut s = String::new();
    let mut i = 0;
    while i < chars.len() {
        if ('a'..='h').contains(&chars[i])
            && i + 1 < chars.len()
            && ('1'..='8').contains(&chars[i + 1])
        {
            s.push(chars[i]);
            s.push(chars[i + 1]);
            i += 2;
        } else {
            i += 1;
        }
    }
    s
}

fn load_kifu_into(app: &State<App>, text: &str) -> Result<GameView, String> {
    if let Some(loaded) = game_from_text(text) {
        let mut game = app.game.lock().unwrap();
        *game = loaded;
        return Ok(view(&game));
    }
    let start = extract_start(text);
    // 開始局面の行は座標に見える文字を含むので、棋譜を拾う前に取り除く
    let body = match &start {
        Some(_) => text
            .lines()
            .filter(|l| {
                let c: Vec<char> = l.chars().filter(|c| !c.is_whitespace()).collect();
                c.len() != 65 && !l.contains("BO[")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => text.to_string(),
    };
    let s = extract_kifu(&body);
    if s.is_empty() && start.is_none() {
        return Err("棋譜が見つかりません".into());
    }
    /* **エンジンのエラーを素通しにしない。** `invalid KIFU: xx` が
    そのままトーストに出ていた — すぐ上の「棋譜が見つかりません」は
    日本語なのに、こちらだけ英語だった。原因はログに残して、画面には
    直し方の分かる日本語だけを出す (設計 §9 の注記)。 */
    let loaded = match &start {
        Some(b) => Reversi::from_kifu_with_start(b, &s),
        None => Reversi::from_kifu(&s),
    }
    .map_err(|e| {
        eprintln!("棋譜を読み取れません: {e}");
        "棋譜を読み取れません。手の並びが正しいか確かめてください".to_string()
    })?;
    let mut game = app.game.lock().unwrap();
    *game = loaded;
    Ok(view(&game))
}

#[tauri::command]
async fn load_kifu(
    app: State<'_, App>,
    handle: tauri::AppHandle,
) -> Result<Option<GameView>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter("棋譜", &["ggf", "txt", "kifu"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let p = path.into_path().map_err(|e| e.to_string())?;
    let s = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    load_kifu_into(&app, &s).map(Some)
}

/// 貼り付けテキストから棋譜を読み込む。
#[tauri::command]
fn load_kifu_text(app: State<App>, text: String) -> Result<GameView, String> {
    load_kifu_into(&app, &text)
}

/// 棋譜を「1 手ごとの盤面」に開く。読むためではなく見るための形。
/// 対局の状態は変えないので、履歴から盤面を確かめるのに使える。
#[derive(Serialize)]
struct KifuFrame {
    /// 64 マス: 0 空き / 1 黒 / 2 白。
    cells: Vec<u8>,
    /// この手で置いたマス (初期局面は null、パスも null)。
    last: Option<u8>,
    black: u8,
    white: u8,
    /// 手番 ("black" | "white")。
    player: String,
}

/* ---------------- 定石を眺める ---------------- */

/// 定石の 1 手。値は手番視点の石差、`games` は棋譜での採用回数。
#[derive(Serialize)]
struct BookMoveView {
    pos: u8,
    value: f32,
    games: u32,
}

/// 定石のある 1 局面。
#[derive(Serialize)]
struct BookNodeView {
    /// 64 マス: 0 空き / 1 黒 / 2 白。
    cells: Vec<u8>,
    /// "black" | "white"
    player: String,
    black: u8,
    white: u8,
    /// 値の高い順。空なら「この局面は定石に無い」。
    moves: Vec<BookMoveView>,
    /// 実戦から学習して書き戻された局面か。
    learned: bool,
    /// この局面の定石の値 (石差)。定石に無ければ null。
    value: Option<f32>,
    /// その値を付けたときの読み深さ。**値がどれくらい確かかの手がかり**。
    depth: Option<u8>,
    /// 定石に載っている局面の総数 (見出しに出す)。
    size: usize,
    /// そのうち実戦から書き戻したぶん。
    learned_size: usize,
}

/// 棋譜で指した局面の定石を返す。
///
/// **対局の状態は触らない。** 眺めるのと打つのは別の営みで、定石を辿って
/// いる最中に対局の盤が動くと、どちらを見ているのか分からなくなる。
#[tauri::command]
async fn book_node(app: State<'_, App>, kifu: String) -> Result<BookNodeView, String> {
    let game = if kifu.trim().is_empty() {
        Reversi::new()
    } else {
        game_from_text(&kifu).ok_or("棋譜を読めません")?
    };
    let b = game.board;
    let player = game.player();
    let eng = app.engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = eng.lock().unwrap();
        let e = guard.as_mut().ok_or("エンジンがまだありません")?;
        let (moves, learned) = e.book_node(&b).unwrap_or((Vec::new(), false));
        let entry = e.book_entry(&b);
        let mut cells = vec![0u8; 64];
        for i in 0..64u8 {
            let bit = 1u64 << i;
            if b.black & bit != 0 {
                cells[i as usize] = 1;
            } else if b.white & bit != 0 {
                cells[i as usize] = 2;
            }
        }
        Ok(BookNodeView {
            value: entry.map(|(v, _, _)| v),
            depth: entry.map(|(_, _, d)| d),
            cells,
            player: if player == Color::Black {
                "black"
            } else {
                "white"
            }
            .into(),
            black: b.black.count_ones() as u8,
            white: b.white.count_ones() as u8,
            moves: moves
                .into_iter()
                .map(|(p, value, games)| BookMoveView {
                    pos: p.index(),
                    value,
                    games,
                })
                .collect(),
            learned,
            size: e.book_size(),
            learned_size: e.learned_size(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn preview_kifu(text: String) -> Result<Vec<KifuFrame>, String> {
    let mut game = game_from_text(&text).ok_or("棋譜を読めません")?;
    let line = game.line();
    // 開始局面まで巻き戻す (抽選オープニングは初期配置と違う)
    while game.move_count() > 0 {
        game.undo().map_err(|e| format!("{e:?}"))?;
    }
    let mut b = game.board;

    let frame = |b: &Board, last: Option<u8>| {
        let mut cells = vec![0u8; 64];
        for i in 0..64u8 {
            let bit = 1u64 << i;
            if b.black & bit != 0 {
                cells[i as usize] = 1;
            } else if b.white & bit != 0 {
                cells[i as usize] = 2;
            }
        }
        KifuFrame {
            cells,
            last,
            black: b.black.count_ones() as u8,
            white: b.white.count_ones() as u8,
            player: match b.player() {
                Color::Black => "black".into(),
                Color::White => "white".into(),
            },
        }
    };

    let mut out = vec![frame(&b, None)];
    for e in line {
        match e {
            Some(p) => {
                b.make_move(p).map_err(|e| format!("{e:?}"))?;
                out.push(frame(&b, Some(p.index())));
            }
            None => {
                b.pass();
                out.push(frame(&b, None));
            }
        }
    }
    Ok(out)
}

/// 外部から渡された棋譜 (あれば読んで削除)。
fn take_handoff_kifu() -> Option<String> {
    let path = std::env::temp_dir().join("kuroobi_handoff.txt");
    let s = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    (!s.trim().is_empty()).then_some(s)
}

/// 受け渡しテキスト (GGF・開始局面つき・素の棋譜) を対局に起こす。
fn game_from_text(text: &str) -> Option<Reversi> {
    if let Some((start, kifu)) = parse_ggf(text) {
        return match &start {
            Some(b) => Reversi::from_kifu_with_start(b, &kifu).ok(),
            None => Reversi::from_kifu(&kifu).ok(),
        };
    }
    let start = extract_start(text);
    let body: String = match &start {
        Some(_) => text
            .lines()
            .filter(|l| {
                let c: Vec<char> = l.chars().filter(|c| !c.is_whitespace()).collect();
                c.len() != 65 && !l.contains("BO[")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => text.to_string(),
    };
    let kifu = extract_kifu(&body);
    match &start {
        Some(b) => Reversi::from_kifu_with_start(b, &kifu).ok(),
        None => Reversi::from_kifu(&kifu).ok(),
    }
}

// ============================ GGS ============================

fn ggs_tx(app: &State<App>) -> Result<std::sync::mpsc::Sender<ggs::Cmd>, String> {
    app.ggs
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| h.tx.clone())
        .ok_or_else(|| "GGS セッションが起動していません".into())
}

/// `.ggs_credentials` (repo 直下、name:pw) を探して読む。GUI のログインは
/// キーチェーンに移したので、これは初回の取り込み元としてだけ残っている
/// (ファイル自体は CLI 版 ggs が使うため消さない)。
fn read_credentials() -> Option<(String, String)> {
    for c in [
        ".ggs_credentials",
        "../.ggs_credentials",
        "../../.ggs_credentials",
    ] {
        if let Ok(s) = std::fs::read_to_string(c) {
            if let Some((l, p)) = s.lines().next().and_then(|l| l.split_once(':')) {
                return Some((l.trim().to_string(), p.trim().to_string()));
            }
        }
    }
    None
}

/// 保存済みの認証情報。キーチェーンに項目が全く無い初回だけ旧ファイル
/// から取り込む (次からはキーチェーンで完結する)。ログアウト済み
/// (墓標あり) のときは取り込まない — 復活してしまうため。
fn saved_credentials() -> Option<(String, String)> {
    if keychain::exists() {
        return keychain::load();
    }
    let (l, p) = read_credentials()?;
    keychain::save(&l, &p);
    Some((l, p))
}

#[tauri::command]
fn ggs_connect(app: State<App>, login: String, pw: String) -> Result<String, String> {
    if login.trim().is_empty() || pw.is_empty() {
        return Err("ログイン名とパスワードを入力してください".into());
    }
    let l = login.trim().to_string();
    ggs_tx(&app)?
        .send(ggs::Cmd::Connect {
            login: l.clone(),
            pw,
        })
        .map_err(|e| e.to_string())?;
    Ok(l)
}

/// フロントエンドの例外を拾うための診断コマンド。WebView のコンソールが
/// 見えない環境でも /tmp のログで追える。
#[tauri::command]
fn js_log(msg: String) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/kuroobi_js.log")
    {
        let _ = writeln!(f, "[JS] {msg}");
    }
}

/// ログアウト。切断に加えて保存済みの認証情報も忘れる (この手のアプリの
/// 通例に合わせる: アプリを閉じただけならログイン状態が続き、明示的な
/// ログアウトで次回の自動ログインも止まる)。
#[tauri::command]
fn ggs_disconnect(app: State<App>) -> Result<(), String> {
    keychain::forget();
    ggs_tx(&app)?
        .send(ggs::Cmd::Disconnect)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_raw(app: State<App>, cmd: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Raw(cmd))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_ask(
    app: State<App>,
    gtype: String,
    time: String,
    opponent: String,
    rated: bool,
) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Ask {
            gtype,
            time,
            opponent,
            rated,
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_accept(app: State<App>, id: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Accept(id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_decline(app: State<App>, id: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Decline(id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_finger(app: State<App>, name: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Finger(name))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_who(app: State<App>, gtype: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Who(gtype))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_top(app: State<App>, gtype: String, n: u32) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Top { gtype, n })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_rank(app: State<App>, gtype: String, name: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Rank { gtype, name })
        .map_err(|e| e.to_string())
}

/// 終局した対局を一覧から閉じる。
#[tauri::command]
fn ggs_close_match(app: State<App>, id: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::CloseMatch(id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_watch(app: State<App>, id: String, on: bool) -> Result<(), String> {
    let cmd = if on {
        ggs::Cmd::Watch(id)
    } else {
        ggs::Cmd::Unwatch(id)
    };
    ggs_tx(&app)?.send(cmd).map_err(|e| e.to_string())
}

/// 画面が受け取った一言と取り出した棋譜を消す。
///
/// **受け取った側が消す。** 出しっぱなしにすると、画面を開き直すたびに
/// 同じ報せが出るし、次に取り出した棋譜と見分けが付かない。
#[tauri::command]
fn ggs_ack(app: State<App>) -> Result<(), String> {
    let snap = ggs_snap_arc(&app).ok_or("GGS に接続していません")?;
    let mut s = snap.lock().unwrap();
    s.notice.clear();
    s.fetched_ggf = None;
    Ok(())
}

/// 終わった対局の棋譜 (GGF) を GGS から取り出す。結果は snapshot に載る。
#[tauri::command]
fn ggs_look(app: State<App>, id: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Look(id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_chat(app: State<App>, target: String, text: String) -> Result<(), String> {
    if target.trim().is_empty() || text.trim().is_empty() {
        return Err("宛先と本文が必要です".into());
    }
    ggs_tx(&app)?
        .send(ggs::Cmd::Chat {
            target: target.trim().into(),
            text: text.trim().into(),
        })
        .map_err(|e| e.to_string())
}

/// 対局操作。verb は undo / abort / resign / tell のいずれか。
#[tauri::command]
fn ggs_match_cmd(app: State<App>, id: String, verb: String, arg: String) -> Result<(), String> {
    const ALLOWED: [&str; 4] = ["undo", "abort", "resign", "tell"];
    if !ALLOWED.contains(&verb.as_str()) {
        return Err(format!("unsupported verb: {verb}"));
    }
    ggs_tx(&app)?
        .send(ggs::Cmd::MatchCmd { id, verb, arg })
        .map_err(|e| e.to_string())
}

/// aform (自動受諾) / dform (自動拒否) の式をサーバーに設定する。
#[tauri::command]
fn ggs_set_formula(app: State<App>, kind: String, expr: String) -> Result<(), String> {
    if kind != "aform" && kind != "dform" {
        // 画面からは定数しか渡さないので普段は起きない。**それでも日本語で
        // 返す** — この文字列はトーストに素通しで出る経路にいる
        return Err(format!("申し込みの扱いの種別が不正です ({kind})"));
    }
    ggs_tx(&app)?
        .send(ggs::Cmd::SetFormula { kind, expr })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_list_stored(app: State<App>) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::ListStored)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_list_matches(app: State<App>) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::ListMatches)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_resume_stored(app: State<App>, id: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::ResumeStored(id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_history(app: State<App>, name: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::History(name))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_set_engine(
    app: State<App>,
    depth: u32,
    solve: u8,
    band: u8,
    threads: usize,
    ponder: bool,
) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetEngine {
            depth,
            solve,
            band,
            threads,
            ponder,
        })
        .map_err(|e| e.to_string())
}

/// 持ち時間の使い方 (配り方・1 手の上限・予備)。
#[tauri::command]
fn ggs_set_pacing(
    app: State<App>,
    pace: String,
    max_move_secs: u64,
    reserve_secs: u64,
) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetPacing {
            pace,
            max_move_secs,
            reserve_secs,
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_set_auto_play(app: State<App>, on: bool) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetAutoPlay(on))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_set_watch_analysis(app: State<App>, on: bool) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetWatchAnalysis(on))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_set_use_book(app: State<App>, on: bool) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetUseBook(on))
        .map_err(|e| e.to_string())
}

/// GGS の対局を定石の学習に取り込むかどうか。
#[tauri::command]
fn ggs_set_learn(app: State<App>, on: bool) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetLearn(on))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_set_standby(app: State<App>, cfg: ggs::StandbyCfg) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetStandby(cfg))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_snapshot(app: State<App>) -> Result<ggs::Snapshot, String> {
    /* 画面確認用。**繋がずに GGS の画面を描かせる** — ロビー・プレイヤー・
    対局結果は相手が要るので、一度も実機で確かめられていなかった。
    操作は通らない (見た目を確かめるためだけ)。 */
    if let Ok(v) = std::env::var("KUROOBI_GGS_DEMO") {
        let mut s = ggs::demo_snapshot();
        /* `=empty` で中身を空にする。**空状態は作り物にも対局が入っている
        ので出せなかった** — 釦の並びを確かめるのに実機で撮れる道が要る */
        if v == "empty" {
            s.matches.clear();
            s.ongoing.clear();
            s.offers.clear();
            s.stored.clear();
        }
        return Ok(s);
    }
    let snap = {
        let guard = app.ggs.lock().unwrap();
        let h = guard
            .as_ref()
            .ok_or_else(|| "GGS セッションが起動していません".to_string())?;
        h.snapshot.clone()
    };
    let s = snap.lock().unwrap().clone();
    Ok(s)
}

/// レート戦が禁じられているか (`KUROOBI_NO_RATED=1`)。画面はこれを見て
/// 「する」を選べなくする。**送る側でも潰している**ので、画面が古くても
/// レート戦は成立しない。
#[tauri::command]
fn ggs_no_rated() -> bool {
    ggs::no_rated()
}

/// **いま効いている環境変数の一覧。** 画面の下の帯に出す。
///
/// 環境変数を付けて起動すると、レート戦が禁じられていたり、繋ぐ先が
/// 作り物だったり、控えの置き場所が違ったりする。**それを知らずに画面を
/// 触ると、出ている絵が本物なのか確認用なのか区別が付かない。**
/// 名前と値をそのまま返し、画面は「素の起動と違う」ことだけを示す。
///
/// 値そのものを隠す必要があるものは無い (認証情報を持つ変数は無く、
/// `KUROOBI_KEYCHAIN_SERVICE` は項目の名前でしかない)。
#[tauri::command]
fn env_overrides() -> Vec<(String, String)> {
    const NAMES: &[&str] = &[
        "KUROOBI_NO_RATED",
        "KUROOBI_GGS_DEMO",
        "KUROOBI_GGS_AUTOCONNECT",
        "KUROOBI_GGS_AUTOVIEW",
        "KUROOBI_GGS_AUTOWATCH",
        "KUROOBI_GGS_AUTOLOOK",
        "KUROOBI_AUTOPLAY",
        "KUROOBI_THEME",
        "KUROOBI_LEARN_LOG",
        "KUROOBI_KEYCHAIN_SERVICE",
        "KUROOBI_SESSION_LOCK",
        "KUROOBI_WEIGHTS_DIR",
    ];
    NAMES
        .iter()
        .filter_map(|n| {
            std::env::var(n)
                .ok()
                .map(|v| ((*n).to_string(), if v.is_empty() { "1".into() } else { v }))
        })
        .collect()
}

/// 画面確認用: 起動直後に開く画面 (KUROOBI_GGS_AUTOVIEW)。
#[tauri::command]
fn ggs_autoview() -> String {
    std::env::var("KUROOBI_GGS_AUTOVIEW").unwrap_or_default()
}

/// 通信ログをファイルへ落とす。棋譜とは絞り込みも既定の名前も違うので
/// `ggs_save_kifu` を使い回さない (「棋譜」の絞り込みでログを保存させない)。
#[tauri::command]
async fn ggs_save_log(handle: tauri::AppHandle, text: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    if text.is_empty() {
        return Err("ログが空です".into());
    }
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter("ログ", &["log", "txt"])
        .set_file_name("ggs-log.txt")
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let p = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| e.to_string())?;
    Ok(Some(p.display().to_string()))
}

/// GGS の棋譜 (文字列で渡される) をファイルに保存する。
#[tauri::command]
async fn ggs_save_kifu(
    handle: tauri::AppHandle,
    kifu: String,
    name: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    if kifu.is_empty() {
        return Err("棋譜が空です".into());
    }
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter("棋譜", &["ggf", "txt", "kifu"])
        .set_file_name(format!("{name}.txt"))
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let p = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&p, format!("{kifu}\n")).map_err(|e| e.to_string())?;
    Ok(Some(p.display().to_string()))
}

fn main() {
    let initial = take_handoff_kifu()
        .as_deref()
        .and_then(game_from_text)
        .unwrap_or_default();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(App {
            game: Mutex::new(initial),
            engine: Arc::new(Mutex::new(None)),
            stop: Arc::new(Mutex::new(None)),
            ggs: Mutex::new(None),
            learn_on: Mutex::new(true),
            activity: Arc::new(Mutex::new(Activity::default())),
            cpu_meter: Mutex::new(None),
        })
        .setup(|app| {
            // GGS 側にはローカル探索の停止ハンドルと稼働記録を渡す。
            // 自分の GGS 対局が始まったらローカルの探索を止めるため
            let st = app.state::<App>();
            /* 見た目を確かめるためだけの作り物 (`KUROOBI_GGS_DEMO`) では
            **通信そのものを始めない。** 始めると「未接続」のスナップ
            ショットを繰り返し流してきて、作り物を上書きしてしまう。 */
            if std::env::var("KUROOBI_GGS_DEMO").is_ok() {
                return Ok(());
            }
            let handle = ggs::spawn(app.handle().clone(), st.stop.clone(), st.activity.clone());
            // 保存済みの認証情報があれば起動時に自動ログインする。
            // 画面確認の自動運転 (KUROOBI_AUTOPLAY) とデモ
            // (KUROOBI_GGS_AUTOVIEW) では実サーバーに繋がない
            // (KUROOBI_GGS_AUTOCONNECT=1 はその場合でも強制的に繋ぐ)。
            // 別ウィンドウが接続中のときは黙って見送る (そちらを使う)。
            let force = std::env::var("KUROOBI_GGS_AUTOCONNECT").is_ok();
            let demo = std::env::var("KUROOBI_AUTOPLAY").is_ok()
                || std::env::var("KUROOBI_GGS_AUTOVIEW").is_ok();
            if (force || !demo) && !ggs::session_locked_by_other() {
                if let Some((l, p)) = saved_credentials() {
                    let _ = handle.tx.send(ggs::Cmd::Connect { login: l, pw: p });
                }
            }
            if force {
                // KUROOBI_GGS_AUTOLOOK=<id> なら棋譜取得も試す
                // (取得経路を UI 操作なしで確かめるため)。
                if let Ok(id) = std::env::var("KUROOBI_GGS_AUTOLOOK") {
                    let tx = handle.tx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(12));
                        let _ = tx.send(ggs::Cmd::Look(id));
                    });
                }
                // KUROOBI_GGS_AUTOWATCH=<id>,… / auto なら観戦も始める。
                if let Ok(ids) = std::env::var("KUROOBI_GGS_AUTOWATCH") {
                    let tx = handle.tx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(12));
                        if ids.trim() == "auto" {
                            // 一覧を取り直す。届いた時点でセッション側が
                            // まとめて観戦する。
                            let _ = tx.send(ggs::Cmd::ListMatches);
                        } else {
                            for id in ids.split(',').filter(|s| !s.trim().is_empty()) {
                                let _ = tx.send(ggs::Cmd::Watch(id.trim().to_string()));
                            }
                        }
                    });
                }
            }
            *app.state::<App>().ggs.lock().unwrap() = Some(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            state,
            new_game,
            play,
            undo,
            goto,
            set_levels,
            set_use_book,
            local_threads,
            set_local_threads,
            activity_status,
            set_learn,
            learn_game,
            learn_log,
            learn_undo,
            has_book,
            book_node,
            autoplay,
            theme_override,
            resource_status,
            pick_resource,
            set_resource,
            stop_search,
            think,
            apply_move,
            analyze_live,
            ponder_live,
            eval_at,
            save_kifu,
            load_kifu,
            load_kifu_text,
            preview_kifu,
            js_log,
            ggs_connect,
            ggs_disconnect,
            ggs_raw,
            ggs_ask,
            ggs_accept,
            ggs_decline,
            ggs_finger,
            ggs_who,
            ggs_top,
            ggs_rank,
            ggs_watch,
            ggs_close_match,
            ggs_look,
            ggs_ack,
            ggs_chat,
            ggs_match_cmd,
            ggs_set_formula,
            ggs_list_stored,
            ggs_list_matches,
            ggs_resume_stored,
            ggs_history,
            ggs_set_engine,
            ggs_set_pacing,
            ggs_set_auto_play,
            ggs_set_watch_analysis,
            ggs_set_use_book,
            ggs_set_learn,
            ggs_set_standby,
            ggs_snapshot,
            ggs_autoview,
            ggs_no_rated,
            env_overrides,
            ggs_save_kifu,
            ggs_save_log
        ])
        .run(tauri::generate_context!())
        .expect("tauri run");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 外部から渡される形: 1 行目が開始局面、2 行目が着手列。
    #[test]
    fn reads_start_position_and_kifu() {
        let start = Board::new().to_string();
        let text = format!("{start}\nf5d6c3\n");
        assert_eq!(
            extract_start(&text).as_deref(),
            Some(start.replace(' ', "").as_str())
        );
        // 開始局面の行を棋譜として拾ってしまわないこと
        let g = game_from_text(&text).expect("読めること");
        assert_eq!(g.move_count(), 3);
    }

    /// 開始局面が無ければ従来どおり初期局面から。
    #[test]
    fn plain_kifu_still_works() {
        assert!(extract_start("f5d6c3").is_none());
        let g = game_from_text("f5d6c3").expect("読めること");
        assert_eq!(g.move_count(), 3);
    }

    /// GGF の BO タグからも開始局面を拾う。
    #[test]
    fn reads_start_from_ggf() {
        let start = Board::new().to_string();
        let text = format!("(;GM[Othello]PC[GGS/os]BO[8 {start}]B[F5]W[D6];)");
        assert_eq!(
            extract_start(&text).as_deref(),
            Some(start.replace(' ', "").as_str())
        );
    }

    /// 開始局面が抽選局面でも、着手列と合わせて元の対局に戻る。
    #[test]
    fn drawn_opening_round_trips() {
        let mut drawn = Reversi::new();
        for _ in 0..5 {
            let p = drawn.board.movable_iter().next().unwrap();
            drawn.make_move(p).unwrap();
        }
        let start = drawn.board.to_string();
        let mut kifu = String::new();
        for _ in 0..3 {
            let p = drawn.board.movable_iter().next().unwrap();
            drawn.make_move(p).unwrap();
            kifu.push_str(&p.to_kifu().to_lowercase());
        }
        let g = game_from_text(&format!("{start}\n{kifu}")).expect("読めること");
        assert_eq!(g.board.black, drawn.board.black);
        assert_eq!(g.board.white, drawn.board.white);
    }

    /// GGS が使う GGF。開始局面を BO タグに持つので往復できる。
    #[test]
    fn reads_ggf() {
        let ggf = "(;GM[Othello]PC[GGS/os]PB[nyanyan]RB[2658.9]PW[egrcd]RW[2585.8]\
                   BO[8 ---------------------------O*------*O--------------------------- *]\
                   B[F5]W[D6]B[C3];)";
        let g = game_from_text(ggf).expect("読めること");
        assert_eq!(g.move_count(), 3);
        let plain = Reversi::from_kifu("f5d6c3").unwrap();
        assert_eq!(g.board.black, plain.board.black);
        assert_eq!(g.board.white, plain.board.white);
    }

    /// 対局者名に座標に見える文字が入っていても手として拾わない。
    #[test]
    fn ggf_ignores_coordinates_inside_names() {
        let ggf = "(;GM[Othello]PB[player1]PW[a1ice]\
                   BO[8 ---------------------------O*------*O--------------------------- *]\
                   B[F5];)";
        let g = game_from_text(ggf).expect("読めること");
        assert_eq!(g.move_count(), 1, "名前の a1 / r1 を手にしない");
    }

    /// 抽選オープニングの GGF が元の対局に戻る。
    #[test]
    fn ggf_round_trips_a_drawn_opening() {
        let mut drawn = Reversi::new();
        for _ in 0..5 {
            let p = drawn.board.movable_iter().next().unwrap();
            drawn.make_move(p).unwrap();
        }
        let bo = drawn.board.to_string().replace('X', "*");
        let mut tags = String::new();
        let mut black = bo.ends_with(" *");
        for _ in 0..3 {
            let p = drawn.board.movable_iter().next().unwrap();
            drawn.make_move(p).unwrap();
            tags.push_str(&format!(
                "{}[{}]",
                if black { "B" } else { "W" },
                p.to_kifu().to_uppercase()
            ));
            black = !black;
        }
        let ggf = format!("(;GM[Othello]BO[8 {bo}]{tags};)");
        let g = game_from_text(&ggf).expect("読めること");
        assert_eq!(g.board.black, drawn.board.black);
        assert_eq!(g.board.white, drawn.board.white);
    }

    /// GGS の `look` が返す実データ (評価値と消費時間つき) を読む。
    #[test]
    fn reads_ggf_from_ggs_archive() {
        let ggf = "(;GM[Othello]PC[GGS/os]DT[2026.07.30_17:36:36.MDT]PB[kuroobi]PW[fly]\
                   RB[1720]RW[1438.62]TI[15:00//02:00]TY[8]RE[+54.000]\
                   BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]\
                   B[E6]W[f4/-25.99/0.20]B[C3]W[d6/-25.99/0.04]B[F6]W[e7/-25.99/0.02];)";
        let g = game_from_text(ggf).expect("読めること");
        assert_eq!(g.move_count(), 6);
        // 大文字・小文字が混ざり、評価値と時間が付いていても手だけ拾う
        let plain = Reversi::from_kifu("e6f4c3d6f6e7").unwrap();
        assert_eq!(g.board.black, plain.board.black);
        assert_eq!(g.board.white, plain.board.white);
    }

    /// GGS の `look` が返した実データ (パスを含む 1 局まるごと)。
    #[test]
    fn replays_a_whole_archived_game() {
        let ggf = "(;GM[Othello]PC[GGS/os]DT[2026.07.30_17:36:36.MDT]PB[kuroobi]PW[fly]RB[1720]RW[1438.62]TI[15:00//02:00]TY[8]RE[+54.000]BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]B[E6]W[f4/-25.99/0.20]B[C3]W[d6/-25.99/0.04]B[F6]W[e7/-25.99/0.02]B[F5]W[g5/-25.99]B[E3]W[g4/-28.23]B[C7]W[d3/24.23]B[F3]W[c4/0.23]B[C6]W[c5/-7.29]B[B4]W[b6/-7.81]B[D7]W[b5/-8.06]B[C2]W[a3/-7.84]B[F8]W[e8/-11.18]B[D8]W[c8/-15.07]B[B8]W[d2/-19.02]B[G3]W[e2/-19.46]B[A6]W[c1/-20.24]B[D1]W[e1/-20.44]B[F2]W[f1/-17.83]B[F7]W[h3/-18.89]B[A5]W[a7/-29.57]B[A8]W[b7/-35.51]B[G2]W[g8/-38.82]B[H8]W[g1/-45.44]B[B3]W[a4/-38.31]B[A2]W[b2]B[A1]W[b1]B[G7]W[g6]B[H6]W[h7]B[H5]W[h4]B[H2]W[pass]B[H1];)";
        let g = game_from_text(ggf).expect("読めること");
        assert!(g.board.is_game_over(), "終局まで再生できる");
        // GGF の RE[+54.000] は黒 (kuroobi) から見た石差
        let (b, w) = (
            g.board.black.count_ones() as i32,
            g.board.white.count_ones() as i32,
        );
        assert_eq!(b - w, 54, "結果が RE と一致する");
        assert!(ggf.contains("W[pass]"), "終盤にパスが入っている局");
    }

    /* ---- GGF を書く側 ---- */

    /// 書いた GGF を読み直すと元の対局に戻る。
    #[test]
    fn writes_ggf_that_reads_back() {
        let g = Reversi::from_kifu("e6f4c3d6f6e7").unwrap();
        let ggf = to_ggf(&g, "KUROOBI", "Player");
        assert!(ggf.contains("PB[KUROOBI]PW[Player]"), "{ggf}");
        assert!(ggf.contains("RE[?]"), "終局していない対局は結果を書かない");
        assert!(ggf.contains("B[E6]W[F4]"), "手は大文字・色つき: {ggf}");
        let back = game_from_text(&ggf).expect("読めること");
        assert_eq!(back.board.black, g.board.black);
        assert_eq!(back.board.white, g.board.white);
    }

    /// パスを含む 1 局を書いて読み直しても手番がずれない。
    #[test]
    fn writes_pass_and_result() {
        let src =
            "(;GM[Othello]BO[8 ---------------------------O*------*O--------------------------- *]\
                   B[E6]W[F4]B[C3]W[D6]B[F6]W[E7]B[F5]W[G5]B[E3]W[G4]B[C7]W[D3]B[F3]W[C4]\
                   B[C6]W[C5]B[B4]W[B6]B[D7]W[B5]B[C2]W[A3]B[F8]W[E8]B[D8]W[C8]B[B8]W[D2]\
                   B[G3]W[E2]B[A6]W[C1]B[D1]W[E1]B[F2]W[F1]B[F7]W[H3]B[A5]W[A7]B[A8]W[B7]\
                   B[G2]W[G8]B[H8]W[G1]B[B3]W[A4]B[A2]W[B2]B[A1]W[B1]B[G7]W[G6]B[H6]W[H7]\
                   B[H5]W[H4]B[H2]W[PASS]B[H1];)";
        let g = game_from_text(src).expect("読めること");
        assert!(g.board.is_game_over());
        let ggf = to_ggf(&g, "a", "b");
        assert!(ggf.contains("W[PA]"), "パスを落とすと手番がずれる: {ggf}");
        assert!(ggf.contains("RE[+54]"), "終局した対局は石差を書く: {ggf}");
        let back = game_from_text(&ggf).expect("読めること");
        assert_eq!(back.board.black, g.board.black);
        assert_eq!(back.board.white, g.board.white);
    }

    /// 抽選開局から始めた対局は BO に開始局面が入る (初期局面ではない)。
    #[test]
    fn writes_drawn_opening_start() {
        let mut drawn = Reversi::new();
        for _ in 0..5 {
            let p = drawn.board.movable_iter().next().unwrap();
            drawn.make_move(p).unwrap();
        }
        let start = drawn.board;
        let mut kifu = String::new();
        for _ in 0..4 {
            let p = drawn.board.movable_iter().next().unwrap();
            drawn.make_move(p).unwrap();
            kifu.push_str(&p.to_kifu());
        }
        let g = Reversi::from_kifu_with_start(&start.to_string(), &kifu).unwrap();
        let ggf = to_ggf(&g, "a", "b");
        assert!(
            ggf.contains(&format!("BO[8 {}", ggf_squares(&start))),
            "開始局面が入っていない: {ggf}"
        );
        let back = game_from_text(&ggf).expect("読めること");
        assert_eq!(back.board.black, g.board.black);
        assert_eq!(back.board.white, g.board.white);
    }

    /// 名前に `]` が入ってもタグが壊れない。
    #[test]
    fn ggf_escapes_bracket_in_names() {
        let g = Reversi::from_kifu("e6").unwrap();
        let ggf = to_ggf(&g, "a]b", "c");
        assert!(!ggf.contains("a]b"), "そのまま入れるとタグが切れる");
        assert_eq!(game_from_text(&ggf).unwrap().move_count(), 1);
    }
}
