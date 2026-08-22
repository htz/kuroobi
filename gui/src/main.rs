//! Kuroobi's GUI (Tauri). The engine links into the same process; the
//! blocking search runs on worker threads (spawn_blocking). The
//! frontend is the static page under gui/ui/.

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
    /// Stop handle kept outside the Engine mutex so it stays reachable
    /// during a search.
    stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    /// GGS session (resident thread).
    ggs: Mutex<Option<ggs::Handle>>,
    /// Whether local games feed book learning.
    learn_on: Mutex<bool>,
    /// Which feature currently uses the CPU (status display, and the
    /// yield decision for learning).
    activity: Arc<Mutex<Activity>>,
    /// Previous CPU sample (wall time, process CPU time).
    cpu_meter: Mutex<Option<(std::time::Instant, std::time::Duration)>>,
    /// Local game clock, shaped like GGS's — without a place to
    /// practice under a clock, pacing could only be tried live.
    clocks: Mutex<Clocks>,
}

/// Local game clock; 0 seconds means untimed.
#[derive(Default)]
struct Clocks {
    /// Time per player in seconds; 0 disables the clock.
    total: u64,
    /// Remaining seconds, kept separately for the human and KUROOBI.
    black: f64,
    white: f64,
    /// Which side flagged; frozen once set.
    lost: Option<kuroobi::Color>,
    /// When the current turn started; charged on every move. Humans and
    /// KUROOBI are timed the same way (charging only measured think time
    /// would leave human deliberation uncounted).
    turn_started: Option<std::time::Instant>,
}

impl Clocks {
    fn reset(&mut self, total: u64) {
        self.total = total;
        self.black = total as f64;
        self.white = total as f64;
        self.lost = None;
        self.turn_started = if total > 0 {
            Some(std::time::Instant::now())
        } else {
            None
        };
    }

    /// End the turn: charge the elapsed time, start the next turn.
    fn turn_done(&mut self, mover: kuroobi::Color) {
        if self.total == 0 {
            return;
        }
        if let Some(t) = self.turn_started.take() {
            self.spend(mover, t.elapsed().as_secs_f64());
        }
        self.turn_started = Some(std::time::Instant::now());
    }
    fn left(&self, c: kuroobi::Color) -> f64 {
        if c == kuroobi::Color::Black {
            self.black
        } else {
            self.white
        }
    }
    /// Charge used time, clamped at 0 (negatives break the display).
    fn spend(&mut self, c: kuroobi::Color, secs: f64) {
        let v = if c == kuroobi::Color::Black {
            &mut self.black
        } else {
            &mut self.white
        };
        *v = (*v - secs).max(0.0);
        if *v <= 0.0 && self.lost.is_none() {
            self.lost = Some(c);
        }
    }
}

/// CPU time used by this process (user + sys); no privileges needed.
fn process_cpu_time() -> std::time::Duration {
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        let secs = (ru.ru_utime.tv_sec + ru.ru_stime.tv_sec).max(0) as u64;
        let micros = (ru.ru_utime.tv_usec + ru.ru_stime.tv_usec).max(0) as u64;
        std::time::Duration::from_secs(secs) + std::time::Duration::from_micros(micros)
    }
}

/// Current resident memory of this process (not the peak, so table
/// resizes show up directly).
#[cfg(target_os = "macos")]
fn process_memory() -> u64 {
    // Structs/constants from libc; only the task port from mach2
    // (libc's mach_task_self_ is deprecated).
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

/// Total physical memory (the utilization denominator).
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

/// The local feature using the CPU. Searches are exclusive, so one slot
/// suffices; learning runs in the background but yields to everything.
#[derive(Default)]
pub(crate) struct Activity {
    /// Kind of running local search (think / analyze / review).
    pub(crate) local: Option<&'static str>,
    /// Learning import progress (done, total); None without a job.
    learn: Option<(u32, u32)>,
    /// Whether learning is paused, yielding to another feature.
    learn_paused: bool,
}

/// Running marker for a local search; clears itself on scope exit.
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

/// Default thread count (half the cores).
fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(4)
}

fn ggs_snap_arc(app: &State<App>) -> Option<Arc<Mutex<ggs::Snapshot>>> {
    app.ggs.lock().unwrap().as_ref().map(|h| h.snapshot.clone())
}

/// Whether one of our GGS games is in progress. A clocked real game
/// gets the CPU first: local searches refuse to start and learning
/// yields.
fn ggs_match_in(snap: &Option<Arc<Mutex<ggs::Snapshot>>>) -> bool {
    snap.as_ref().is_some_and(|s| {
        s.lock()
            .unwrap()
            .matches
            .iter()
            // Finished games stay listed; look only at ongoing ones.
            .any(|m| !m.my_color.is_empty() && !m.over)
    })
}

fn ggs_match_active(app: &State<App>) -> bool {
    ggs_match_in(&ggs_snap_arc(app))
}

fn same_board(a: &Board, b: &Board) -> bool {
    a.black == b.black && a.white == b.white && a.player() == b.player()
}

/// NaN/inf become null in JSON and break the frontend; clamp at the boundary.
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

/// Board and game-state snapshot (the shape sent to the frontend).
#[derive(Serialize, Clone)]
struct GameView {
    /// 64 cells: 0 = empty, 1 = black, 2 = white (file-major, A1 = 0).
    cells: Vec<u8>,
    /// "black" | "white"
    player: String,
    legal: Vec<u8>,
    black: u8,
    white: u8,
    over: bool,
    /// Last move square (null on pass).
    last: Option<u8>,
    /// Game record in f5d6... form.
    kifu: String,
    move_count: usize,
    /// Full move line (including undone moves); null = pass.
    moves: Vec<Option<u8>>,
    /// Which move of `moves` the current position follows.
    cursor: usize,
}

#[derive(Serialize)]
struct ThinkView {
    /// Chosen square (null = pass); the position has not moved yet.
    pos: Option<u8>,
    /// Mover-view value in discs.
    value: f32,
    exact: bool,
    /// Whether the move came from the book.
    from_book: bool,
    /// Whether it is a game-learned book entry (display only).
    learned: bool,
    /// Seconds spent on this move (book moves ~0).
    secs: f32,
    /// Nodes visited for this move (0 for book moves).
    nodes: u64,
}

#[derive(Serialize, Clone)]
struct HintView {
    pos: u8,
    value: f32,
    exact: bool,
    /// Whether the value came from the book, not search.
    from_book: bool,
    /// Search depth behind the value (0 for solves and book).
    depth: u32,
}

#[derive(Serialize)]
struct EvalPoint {
    n: usize,
    /// Disc difference from Black's view.
    value: f32,
    exact: bool,
    /// Whether the value came from the book, not search.
    from_book: bool,
}

/// Board right after move n of the line.
///
/// Never replay from the standard start: GGS drawn openings start
/// elsewhere and would fail on the first move. Without the start
/// position, walk with undo/redo to the target and back.
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

/// Consume a pass when the mover has no legal move and the game continues.
fn auto_pass(game: &mut Reversi) {
    while !game.is_game_over() && game.movable() == 0 {
        if game.pass().is_err() {
            break;
        }
    }
}

/// Config file location, in the OS config directory (the only place
/// available once packaged).
fn resources_path() -> PathBuf {
    let base =
        dirs_config().unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/..")));
    base.join("kuroobi").join("resources.conf")
}

/// Import log. Lives in the config directory, not next to the book:
/// books get swapped, but "what did I import" should survive a swap —
/// different lifetimes, different homes.
fn learn_log_path() -> PathBuf {
    /* Overridable via `KUROOBI_LEARN_LOG`: the screen only shows real
    data, so empty/filtered states could not be exercised otherwise. */
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

/// Create the engine if absent. Takes an Arc so worker threads
/// (spawn_blocking) can use it — sync commands run on the main thread,
/// and waiting for the engine lock there freezes the whole UI.
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
            // Table sizes only apply at startup: the tables are leaked
            // to 'static, so rebuilding stacks the old ones unfreed.
            midgame_hash_bits: res.hash_mid_bits(),
            solver_hash_bits: res.hash_end_bits(),
            ..Default::default()
        };
        let engine = Engine::new(cfg).map_err(setup_error)?;
        *stop_slot.lock().unwrap() = Some(engine.stop_handle());
        *guard = Some(engine);
    }
    Ok(())
}

/// Calibrate any uncalibrated thread counts in the background.
///
/// Without calibration `timectl` degrades to the fixed ladder, and a
/// measurement button nobody presses measures nothing — so do it
/// automatically. Takes 1-3 seconds, and never during a game (it would
/// steal CPU from a clocked search); only right after startup while
/// idle. Local and GGS thread settings differ, so measure both.
fn calibrate_missing(
    app: tauri::AppHandle,
    engine_slot: Arc<Mutex<Option<Engine>>>,
    stop_slot: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    activity: Arc<Mutex<Activity>>,
    wanted: Vec<usize>,
) {
    std::thread::spawn(move || {
        let missing: Vec<usize> = {
            let r = resources();
            let mut v: Vec<usize> = wanted
                .into_iter()
                .filter(|t| r.nps_for(*t).is_none())
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        if missing.is_empty() {
            return;
        }
        if ensure_engine_in(&engine_slot, &stop_slot).is_err() {
            return;
        }
        for t in missing {
            // A game started meanwhile: skip the rest (next launch).
            if activity.lock().unwrap().local.is_some() {
                return;
            }
            let nps = {
                let _g = ActivityGuard::begin(&activity, "較正");
                let mut guard = engine_slot.lock().unwrap();
                let Some(e) = guard.as_mut() else { return };
                let keep = e.config().threads;
                e.set_threads(t);
                let n = e.measure_solve_nps();
                e.set_threads(keep);
                n
            };
            if nps > 0.0 {
                let mut r = resources();
                r.set_nps(t, nps);
                let _ = r.save(&resources_path());
                /* Notify an open settings screen: it reads once on open,
                so silent writes would leave it showing "unmeasured". */
                use tauri::Emitter;
                let _ = app.emit("resources-changed", ());
            }
        }
    });
}

/// Translate engine-init failures into display language. The library
/// messages are shared with the CLI (`nnue <path>: ...`); toasts must
/// not show internal jargon without a fix suggestion, so rephrase here.
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
    // Passes are stacked as moves too; rewind to the last stone placed.
    loop {
        game.undo().map_err(|e| format!("{e:?}"))?;
        let placed = game.history.last().map(|r| r.pos.is_some());
        if game.history.is_empty() || placed != Some(false) {
            break;
        }
    }
    Ok(view(&game))
}

/// Jump to just after move n of the line (walked via undo/redo, so
/// both directions work).
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

/// Abort the running search (think and analysis); the frontend
/// discards the result.
#[tauri::command]
fn stop_search(app: State<App>) -> Result<(), String> {
    if let Some(h) = app.stop.lock().unwrap().as_ref() {
        h.stop();
    }
    Ok(())
}

/// Toggle book use (off to study the engine's own moves).
/// async + spawn_blocking keeps the lock wait off the main thread.
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

/// Smoke-test hook (KUROOBI_AUTOPLAY): start something right after
/// launch. "vs" = a game, "both" = engine vs engine; ":<level>" sets
/// strength (e.g. "both:11").
#[tauri::command]
fn autoplay() -> String {
    std::env::var("KUROOBI_AUTOPLAY").unwrap_or_default()
}

/// Screenshot hook: pin the theme (`KUROOBI_THEME=light`/`dark`).
/// The only toggle lives in Settings > Display, which made light-theme
/// checks manual; not persisted — effective for this launch only.
#[tauri::command]
fn theme_override() -> String {
    std::env::var("KUROOBI_THEME").unwrap_or_default()
}

/// Whether a book is available (for display); answered by file
/// existence since engine init is expensive.
#[tauri::command]
fn has_book() -> bool {
    resources().book_path().exists()
}

/// Whether local games feed book learning.
#[tauri::command]
fn set_learn(app: State<App>, on: bool) {
    *app.learn_on.lock().unwrap() = on;
}

/// One imported game. Times stay as unix seconds — formatting belongs
/// to the viewer's timezone and calendar, not the record.
#[derive(Serialize, Deserialize, Clone)]
pub struct LearnEntry {
    pub at: u64,
    pub kifu: String,
    pub black: u8,
    pub white: u8,
    /// Positions written back; an abandoned import records what it got through.
    pub positions: u32,
    /// Drawn-opening start position (board string); empty for the
    /// standard start. Without it a drawn game reopens as a different game.
    #[serde(default)]
    pub start: String,
    /// Rewrite details; without them a bad game could be neither found
    /// nor reverted.
    #[serde(default)]
    pub changes: Vec<LearnChange>,
    /// Opponent name for GGS games, empty for local. Defaults exist
    /// because old log lines lack the field.
    #[serde(default)]
    pub opponent: String,
    /// Which color we played ("b"/"w"); without it disc counts cannot
    /// decide the result and the "lost games" filter breaks. Old lines
    /// lack it — default empty, excluded from the filter.
    #[serde(default)]
    pub my_color: String,
}

/// One book-move rewrite record.
#[derive(Serialize, Deserialize, Clone)]
pub struct LearnChange {
    /// Move number (1-based, passes excluded).
    pub ply: usize,
    pub mv: String,
    /// Value before the overwrite; null if the move was absent.
    pub before: Option<f32>,
    pub after: f32,
    /// Best value after the rewrite; `best - after` = discs lost.
    pub best: f32,
    /// Whether this import created the entry.
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

/// Current unix seconds, for log timestamps.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append one line per record; crash-safe apart from the one line in
/// flight.
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

/// Log cap. Lines carry rewrite details (3-4 KB each); 200 stays under
/// 1 MB and matches what the screen shows.
const LEARN_LOG_MAX: usize = 200;

/// Rewrite the log keeping the newest entries, only once over the cap
/// (rewriting every time would defeat appending).
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

/// Imported games, newest first; unreadable lines skipped.
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
    // Only the recent slice; returning everything grows with age.
    out.truncate(200);
    out
}

/// Undo one import: find the log line (keyed by `at` + record), revert
/// the book, then remove the line. Never remove first — a failed revert
/// with a deleted record would be unverifiable.
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

/// Remove one log line (after the undo).
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

/// Import a finished local game into book learning (learn.rs). Called
/// by the frontend when a played game ends; merely-loaded records don't
/// qualify. The import advances one search at a time and releases the
/// engine lock between searches, so starting a think waits at most one
/// search.
#[tauri::command]
/// `my_color` is the human's color ("b"/"w"), which only the screen
/// knows (KUROOBI's side is configurable, the human is its complement).
/// Without it, disc counts alone cannot decide the result. Empty when
/// undecidable.
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
    // Import only games that replay from the standard start to the
    // final board (keeps loaded drawn-opening games out).
    let (_, fin) = kuroobi::learn::replay(None, &kifu)?;
    if fin.black != board.black || fin.white != board.white {
        return Err("初期局面から始まった対局ではないため取り込みません".into());
    }
    let eng = app.engine.clone();
    let stop = app.stop.clone();
    let act = app.activity.clone();
    let ggs_snap = ggs_snap_arc(&app);
    tauri::async_runtime::spawn_blocking(move || {
        // Engine setup happens here too (waiting for the lock inside a
        // sync command would freeze the main thread).
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
        // Empty until finished; abandoned imports leave no details.
        let mut changes: Vec<kuroobi::learn::BackupChange> = Vec::new();
        /* Last yield time. Yields are many and momentary (analysis
        toggles the search marker per position), while the screen samples
        once a second — so a real, frequent yield would never display.
        Hold the flag briefly to make it true at human timescales. */
        let mut last_yield: Option<std::time::Instant> = None;
        const YIELD_HOLD: std::time::Duration = std::time::Duration::from_millis(1500);
        loop {
            // Yield while games/study/GGS run (background learning must
            // not steal CPU); resume where it left off.
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
                // Engine rebuilt by a settings change: abandon this import.
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
            // Pause before re-locking: mutexes are not FIFO, and an
            // immediate re-lock can starve strength changes or thinks.
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // One log line at the end; abandoned imports record their
        // progress (the write-backs up to that point are real).
        learn_log_append(&LearnEntry {
            at: now_secs(),
            kifu,
            black: board.black.count_ones() as u8,
            white: board.white.count_ones() as u8,
            positions: total.saturating_sub(job.remaining() as u32),
            // Local games import only from the standard start.
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

/// Thread-count setting (local engine); None = auto.
#[derive(Serialize)]
struct ThreadsView {
    set: Option<u32>,
    auto: u32,
    /// Solve nps calibrated at the current thread count (null if not);
    /// other counts' values differ 4x and are not shown.
    nps: Option<f64>,
}

fn threads_view() -> ThreadsView {
    let r = resources();
    let now = r.threads.unwrap_or_else(auto_threads);
    ThreadsView {
        set: r.threads.map(|n| n as u32),
        auto: auto_threads() as u32,
        // Only the current count's value: another count's number looks
        // calibrated but is never used (4x apart).
        nps: r.nps_for(now),
    }
}

#[tauri::command]
fn local_threads() -> ThreadsView {
    threads_view()
}

/// Table sizes (2^bits) and the memory they use.
#[derive(Serialize)]
struct HashView {
    mid: u32,
    end: u32,
    min: u32,
    max: u32,
    /// Combined actual bytes (for display).
    bytes: u64,
}

fn hash_view() -> HashView {
    let r = resources();
    let (mid, end) = (r.hash_mid_bits(), r.hash_end_bits());
    HashView {
        mid,
        end,
        min: kuroobi::resources::HASH_BITS_MIN,
        max: kuroobi::resources::HASH_BITS_MAX,
        bytes: kuroobi::resources::midgame_bytes(mid) + kuroobi::resources::endgame_bytes(end),
    }
}

#[tauri::command]
fn hash_sizes() -> HashView {
    hash_view()
}

/// Set table sizes, effective from the next launch. Tables are leaked
/// to 'static (searches demand lifetime references); rebuilding stacks
/// the old ones, so the current engine is left alone.
#[tauri::command]
fn set_hash_sizes(mid: u32, end: u32) -> Result<HashView, String> {
    let (lo, hi) = (
        kuroobi::resources::HASH_BITS_MIN,
        kuroobi::resources::HASH_BITS_MAX,
    );
    let mut r = resources();
    r.hash_mid = Some(mid.clamp(lo, hi));
    r.hash_end = Some(end.clamp(lo, hi));
    r.save(&resources_path())?;
    Ok(hash_view())
}

/// Measure this machine's solve speed and record it.
///
/// `timectl` needs a nodes-to-seconds factor and it is the only
/// machine-dependent layer; three 22-empty solves (1-3s) measure it.
/// The benefit is avoided breakdowns, not strength (1400 self-play
/// games: no win-rate change, worst-case leftover 5.0s -> 8.9s). The
/// thread count is recorded with it — a mismatched value is unused.
#[tauri::command]
async fn calibrate_nps(app: State<'_, App>) -> Result<ThreadsView, String> {
    ensure_engine(&app)?;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let (nps, threads) = tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "較正");
        let mut guard = eng.lock().unwrap();
        let e = guard.as_mut().unwrap();
        let threads = e.config().threads;
        (e.measure_solve_nps(), threads)
    })
    .await
    .map_err(|e| e.to_string())?;
    if nps <= 0.0 {
        return Err("読切の速度を測れませんでした".into());
    }
    let mut r = resources();
    r.set_nps(threads, nps);
    r.save(&resources_path())?;
    Ok(threads_view())
}

/// Set the local thread count (None = auto). Saved to resources.conf;
/// existing engines pick it up on the next search.
#[tauri::command]
async fn set_local_threads(
    app: State<'_, App>,
    handle: tauri::AppHandle,
    n: Option<u32>,
) -> Result<(), String> {
    let mut r = resources();
    r.threads = n.map(|v| v.clamp(1, 64) as usize);
    r.save(&resources_path())?;
    let threads = r.threads.unwrap_or_else(auto_threads);
    let eng = app.engine.clone();
    tauri::async_runtime::spawn_blocking({
        let eng = eng.clone();
        move || {
            if let Some(e) = eng.lock().unwrap().as_mut() {
                e.set_threads(threads);
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    /* Tell the GGS side too: thread count is a single global setting
    and must reach the GGS engines as well. */
    if let Ok(tx) = ggs_tx(&app) {
        let _ = tx.send(ggs::Cmd::ReloadThreads);
    }
    // The new count is uncalibrated; time management falls to the
    // ladder until measured, so fill it in the background.
    calibrate_missing(
        handle,
        eng,
        app.stop.clone(),
        app.activity.clone(),
        vec![threads],
    );
    Ok(())
}

/// What currently uses the CPU (the nav's always-on display).
#[derive(Serialize)]
struct ActivityView {
    /// Local search kind (think / analyze / review); null if none.
    local: Option<String>,
    local_threads: u32,
    /// Learning import (done, total).
    learn: Option<(u32, u32)>,
    learn_paused: bool,
    /// Whether our GGS game is in progress.
    ggs_match: bool,
    ggs_thinking: bool,
    ggs_threads: u32,
    /// Process CPU usage (%), 100% = one core.
    cpu: f32,
    /// Core count (usage ceiling = cores x 100%).
    cores: u32,
    /// Resident memory and total physical memory (bytes).
    mem: u64,
    mem_total: u64,
}

#[tauri::command]
fn activity_status(app: State<App>) -> ActivityView {
    // CPU usage from the delta since the last call (1s cadence).
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

/// Files in use (name, path, found, size, format tag). Size and format
/// matter because weights get swapped: without them you cannot tell
/// which file explains a change in play.
#[tauri::command]
fn resource_status() -> Vec<(String, String, bool, u64, String)> {
    resources()
        .detailed()
        .into_iter()
        .map(|(n, p, ok, size, kind)| (n.to_string(), p.display().to_string(), ok, size, kind))
        .collect()
}

/* Auxiliary windows were retired (2026-08-08); settings live in an
 * overlay again. Only the Display tab benefited from a window, yet it
 * kept overlapping the board and dragged in localStorage sync, window
 * chrome and placement logic. If a window is ever needed again, use
 * WebviewWindowBuilder. */

/// Open a file dialog, filtered by `kind`.
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

/// Re-point a resource ("dir" | "weights" | "nnue" | "book"); null
/// clears back to the default. The engine rebuilds on the next think.
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
    // Reload happens on the next engine build; drop the current one.
    // Dropping needs the lock, so wait on a worker to keep the UI live.
    let (eng, stop) = (app.engine.clone(), app.stop.clone());
    tauri::async_runtime::spawn_blocking(move || {
        *eng.lock().unwrap() = None;
        *stop.lock().unwrap() = None;
    })
    .await
    .map_err(|e| e.to_string())
}

/// Strength change; async + spawn_blocking as with set_use_book. The
/// running search finishes as-is; the change applies to the next.
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

/// Display clock, deliberately outside `GameView`: boards update per
/// move, clocks tick per second — mixing them would redraw the board
/// every tick.
#[derive(Serialize)]
struct ClockView {
    /// Time per player (seconds); 0 = no clock.
    total: u64,
    black: f64,
    white: f64,
    /// Which side flagged ("black"|"white"); null if none.
    lost: Option<String>,
}

/// Read the clock, subtracting the running turn's elapsed time on the
/// fly — the stored values only move per move and would look frozen.
#[tauri::command]
fn clocks(app: State<App>) -> ClockView {
    let c = app.clocks.lock().unwrap();
    let mut black = c.black;
    let mut white = c.white;
    if c.total > 0 && c.lost.is_none() {
        if let Some(t) = c.turn_started {
            let used = t.elapsed().as_secs_f64();
            let turn = app.game.lock().unwrap().board.player();
            if turn == kuroobi::Color::Black {
                black = (black - used).max(0.0);
            } else {
                white = (white - used).max(0.0);
            }
        }
    }
    ClockView {
        total: c.total,
        black,
        white,
        lost: c.lost.map(|x| {
            if x == kuroobi::Color::Black {
                "black".into()
            } else {
                "white".into()
            }
        }),
    }
}

/// Initialize the clock (0 = none). Called on every new game; unlike
/// GGS it never changes mid-game, so it arrives as an argument.
#[tauri::command]
fn set_clock(app: State<App>, secs: u64) -> ClockView {
    app.clocks.lock().unwrap().reset(secs);
    clocks(app)
}

/// Compute the best move without moving the position (`apply_move`
/// applies it). The split makes stop trivial: the frontend just
/// discards the result.
#[tauri::command]
async fn think(app: State<'_, App>) -> Result<ThinkView, String> {
    // GGS games come first; no local search while one runs.
    if ggs_match_active(&app) {
        return Err("GGS 対局中はローカルの探索を控えます (終局までお待ちください)".into());
    }
    ensure_engine(&app)?;
    let board = app.game.lock().unwrap().board;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let stop = app.stop.clone();
    /* With a clock, search under a deadline through the same timectl
    as GGS — a different local scheme would make practice misleading. */
    let (base, plan) = {
        let c = app.clocks.lock().unwrap();
        let e = app.engine.lock().unwrap();
        let cfg = e.as_ref().unwrap().config();
        let base = kuroobi::timectl::Levels {
            depth: cfg.depth,
            solve: cfg.solve_empties,
            band: cfg.band,
            auto_band: true,
        };
        let threads = cfg.threads;
        drop(e);
        let plan = (c.total > 0).then(|| {
            kuroobi::timectl::plan(
                kuroobi::timectl::Situation {
                    clock_secs: Some(c.left(board.player()) as u64),
                    empties: board.empty_count(),
                    // Calibrated: derive the solve entry from the clock.
                    nps: resources().nps_for(threads),
                    threads,
                    ..Default::default()
                },
                base,
                kuroobi::timectl::Pace::Fast,
            )
        });
        (base, plan)
    };
    let (mv, secs, nodes, aborted) = tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "思考");
        let mut guard = eng.lock().unwrap();
        let t0 = std::time::Instant::now();
        // Node counts are cumulative; diff for this move's share.
        let n0 = guard.as_ref().unwrap().nodes();
        let e = guard.as_mut().unwrap();
        /* Swap strength for this one move only. Forgetting to restore
        feeds this plan's output into the next plan's input and ratchets
        the settings down every move (hit in `arena`; one side broke and
        masqueraded as an effect). */
        if let Some(p) = plan {
            e.set_levels(p.depth, p.solve, p.band);
        }
        let mv = match plan.and_then(|p| p.cap) {
            Some(d) => e.choose_within(&board, Some(t0 + d)),
            None => e.choose(&board),
        };
        if plan.is_some() {
            e.set_levels(base.depth, base.solve, base.band);
        }
        let nodes = guard.as_ref().unwrap().nodes() - n0;
        // Stopped searches return incomplete values; drop them. But only
        // when a search actually ran — book moves never reset the stop
        // flag, and a stale flag would discard correct moves.
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
    // Invalid if the position moved during the think (frontend checks too).
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

/// Apply a think result (or any move); null sq = pass.
#[tauri::command]
fn apply_move(app: State<App>, sq: Option<u8>) -> Result<GameView, String> {
    let mut game = app.game.lock().unwrap();
    // The mover's turn ended; charge its elapsed time.
    let mover = game.board.player();
    app.clocks.lock().unwrap().turn_done(mover);
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

/// Ponder the human's likely move during their turn.
///
/// Local games are fixed-depth, so the benefit is speed, not depth:
/// the same depth in 1/3 the time (measured -62 to -65%). Mutually
/// exclusive with `analyze_live` (they contend for the engine); called
/// only when the eval display is off. Stops itself at the target depth,
/// so a long human think does not keep it spinning.
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
        /* 60-second lid; normally it stops at depth by itself — this
        only guards against an absent human. */
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        e.ponder(&board, until);
    });
    Ok(())
}

/// Stream evaluations while deepening; each finished pass goes to the
/// screen, deepening until the position changes or a stop.
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
        // No book during analysis: book values are past search results
        // and would break comparability with the deepening values.
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
            // Send workload (nodes, elapsed) too; the screen derives speed.
            handle
                .emit("hints", (depth, view, nodes, t0.elapsed().as_secs_f32()))
                .is_ok()
        });
    });
    Ok(())
}

/// Evaluate the position after move n at fixed depth (for the eval
/// graph); Black's view.
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
    // Clear stale stops: book hits never invoke the search (or its
    // reset), and a leftover stop would stall analysis entirely.
    if let Some(h) = stop.lock().unwrap().as_ref() {
        h.reset();
    }
    let (value, exact, from_book, searched) = tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "分析");
        let mut guard = eng.lock().unwrap();
        let e = guard.as_mut().unwrap();
        /* Gold dot only when the book was actually used — the mark
        means "analysis took this value from the book", not "the
        position is in the book" (`book_node` ignores use_book by
        design; `book_value` respects it).

        The learned overlay is excluded: base values are deep search and
        comparable, learned values are backed-up game outcomes reaching
        down to 1 empty — mixing them lines the whole endgame with gold
        dots of one's own past games. And never filter by "also in the
        overlay": openings live in both, and that filter erased genuine
        base entries. Hence `book_base_value`. */
        let from_book = e.book_base_value(&board);
        if let Some(v) = from_book {
            // Both book and search are mover-view; convert to Black's
            // view once at the exit (converting twice flips the dots).
            return (v, false, true, false);
        }
        let mv = e.eval_position(&board, depth);
        (mv.value, mv.exact, false, true)
    })
    .await
    .map_err(|e| e.to_string())?;
    // Stopped searches return no value (it would linger in the graph);
    // book paths skip the check.
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
    // Format follows the extension; asking again in the dialog makes
    // the user decide the same thing twice.
    let ggf_out = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ggf"));
    let body = if ggf_out { ggf } else { format!("{kifu}\n") };
    std::fs::write(&p, body).map_err(|e| e.to_string())?;
    Ok(Some(p.display().to_string()))
}

/* ---------------- GGF (interchange format) ---------------- */

/// Serialize a game as GGF. Unlike the bare f5 form it carries colors,
/// result and start position — the only format that can convey drawn
/// openings and passes.
fn to_ggf(game: &Reversi, black: &str, white: &str) -> String {
    let start = start_board(game);
    let mut out = String::from("(;GM[Othello]PC[KUROOBI]");
    out.push_str(&format!("DT[{}]", ggf_now()));
    out.push_str(&format!("PB[{}]PW[{}]", ggf_text(black), ggf_text(white)));
    // Result is Black's disc difference; unfinished games write "?"
    // (0 would claim a draw).
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
            // Keep passes; dropping them desyncs the turn order.
            None => out.push_str(&format!("{tag}[PA]")),
            Some(p) => out.push_str(&format!("{}[{}]", tag, p.to_kifu().to_uppercase())),
        }
        color = color.opponent();
    }
    out.push_str(";)\n");
    out
}

/// Start position, rebuilt by unwinding moves from the current board
/// (the start itself is not stored); `flipped` makes the unwind exact.
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

/// Write 64 cells in GGF order (a1..h1, a2..h2, ...).
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

/// `]` terminates a tag and would break other readers.
fn ggf_text(s: &str) -> String {
    s.replace(']', ")")
}

/// GGF DT tag: UTC "YYYY-MM-DD HH:MM:SS GMT". Calendar math is done
/// by hand rather than adding a dependency for one call site.
fn ggf_now() -> String {
    let secs = now_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days: days since 1970-01-01 to a date.
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

/// Extract a start position from pasted text.
///
/// Games not starting from the standard position (GGS drawn openings)
/// cannot replay from moves alone; accept a 64-cell + mover line or a
/// GGF `BO[8 ...]`. Fall back to the standard start.
fn extract_start(text: &str) -> Option<String> {
    let cell = |c: char| matches!(c.to_ascii_lowercase(), '-' | '.' | 'x' | 'o' | '*');
    let side = |c: char| matches!(c.to_ascii_lowercase(), 'x' | 'o' | '*');
    // GGF stores the start as BO[8 <64 cells> <mover>].
    let ggf = text.find("BO[").map(|i| &text[i + 3..]).and_then(|rest| {
        let end = rest.find(']')?;
        let inner = rest[..end].trim_start_matches('8').trim();
        Some(inner.to_string())
    });
    for cand in ggf.into_iter().chain(text.lines().map(|l| l.to_string())) {
        let c: Vec<char> = cand.chars().filter(|c| !c.is_whitespace()).collect();
        if c.len() == 65 && c[..64].iter().all(|&x| cell(x)) && side(c[64]) {
            // Board::from_string reads 'x'/'*' as black, 'o' as white.
            return Some(c.into_iter().collect());
        }
    }
    None
}

/// Parse GGF (Generic Game Format): `(;` ... `;)` wraps a game,
/// `BO[8 <cells> <mover>]` is the start, `B[F5/eval/time]` /
/// `W[D6//time]` are moves. Only move tags are read — scanning body
/// text would fabricate moves from names like `PB[player1]`.
fn parse_ggf(text: &str) -> Option<(Option<String>, String)> {
    let body = {
        let start = text.find("(;")?;
        let rest = &text[start + 2..];
        let end = rest.find(";)").unwrap_or(rest.len());
        &rest[..end]
    };
    // Read tags in order: uppercase name + [ ... ].
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
                // "8 <64 cells> <mover>" — drop the size, pack the rest.
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
                // "F5/eval/time". Passes (PA/PASS) are not recorded;
                // replay inserts them automatically.
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
    // Start-position lines contain coordinate-like chars; strip before
    // scanning for moves.
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
    /* Never pass engine errors through raw: `invalid KIFU: xx` used to
    reach the toast in English next to Japanese messages. Log the cause;
    show only actionable display-language text. */
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

/// Load a game record from pasted text.
#[tauri::command]
fn load_kifu_text(app: State<App>, text: String) -> Result<GameView, String> {
    load_kifu_into(&app, &text)
}

/// Expand a record into per-move boards — a viewing shape that leaves
/// the game state untouched.
#[derive(Serialize)]
struct KifuFrame {
    /// 64 cells: 0 empty / 1 black / 2 white.
    cells: Vec<u8>,
    /// Square placed this move (null for the start and passes).
    last: Option<u8>,
    black: u8,
    white: u8,
    /// Mover ("black" | "white").
    player: String,
}

/* ---------------- Book browsing ---------------- */

/// One book move; mover-view value, `games` = adoption count.
#[derive(Serialize)]
struct BookMoveView {
    pos: u8,
    value: f32,
    games: u32,
}

/// One book position.
#[derive(Serialize)]
struct BookNodeView {
    /// 64 cells: 0 empty / 1 black / 2 white.
    cells: Vec<u8>,
    /// "black" | "white"
    player: String,
    black: u8,
    white: u8,
    /// By value, descending; empty = not in the book.
    moves: Vec<BookMoveView>,
    /// Whether the position was game-learned.
    learned: bool,
    /// Book value in discs; null if absent.
    value: Option<f32>,
    /// Search depth behind the value — its trustworthiness hint.
    depth: Option<u8>,
    /// Total book positions (shown in the header).
    size: usize,
    /// Of which game-learned.
    learned_size: usize,
}

/// Book data for a record's positions, never touching game state —
/// browsing and playing are separate activities.
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
    // Rewind to the start position (drawn openings differ from standard).
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

/// An externally handed-off record (read and delete if present).
fn take_handoff_kifu() -> Option<String> {
    let path = std::env::temp_dir().join("kuroobi_handoff.txt");
    let s = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    (!s.trim().is_empty()).then_some(s)
}

/// Materialize hand-off text (GGF / start+moves / bare record) into a game.
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

/// Read `.ggs_credentials` (repo root, name:pw). GUI login moved to the
/// keychain; this remains only as the first-run migration source (the
/// file stays for the CLI ggs).
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

/// Stored credentials. The legacy file is imported only on a true first
/// run (no keychain item at all); a logout tombstone blocks the import
/// so credentials cannot resurrect.
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

/// Diagnostic command capturing frontend exceptions; traceable via the
/// /tmp log even without WebView console access.
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

/// Logout: disconnect and forget stored credentials (the usual
/// convention — closing the app keeps the session, explicit logout
/// stops future auto-login).
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

/// Block clock-related actions until the solve speed is calibrated —
/// uncalibrated time management degrades to the fixed ladder, and GGS
/// flag falls cost rating. Calibration runs in the background at
/// startup, so waiting a few seconds normally clears this.
fn require_calibration() -> Result<(), String> {
    let t = resources().threads.unwrap_or_else(auto_threads);
    if resources().nps_for(t).is_none() {
        return Err(format!(
            "読切の速度をまだ測っていません ({t} スレッド)。設定 → エンジン の「読切の速度」で測ってから設定してください"
        ));
    }
    Ok(())
}

#[tauri::command]
fn ggs_ask(
    app: State<App>,
    gtype: String,
    time: String,
    opponent: String,
    rated: bool,
) -> Result<(), String> {
    // Flag falls cost rating; only offer with working time management.
    require_calibration()?;
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
fn ggs_who(app: State<App>) -> Result<(), String> {
    ggs_tx(&app)?.send(ggs::Cmd::Who).map_err(|e| e.to_string())
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

/// Close a finished game from the list.
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

/// Clear the notice and the fetched record — the receiver clears them,
/// or reopening the screen replays stale messages.
#[tauri::command]
fn ggs_ack(app: State<App>) -> Result<(), String> {
    let snap = ggs_snap_arc(&app).ok_or("GGS に接続していません")?;
    let mut s = snap.lock().unwrap();
    s.notice.clear();
    s.fetched_ggf = None;
    Ok(())
}

/// Fetch a finished game's GGF from GGS; the result lands in the snapshot.
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

/// Match command; verb is undo / abort / resign / tell.
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

/// Set the server-side aform (auto-accept) / dform (auto-decline) formulas.
#[tauri::command]
fn ggs_set_formula(app: State<App>, kind: String, expr: String) -> Result<(), String> {
    if kind != "aform" && kind != "dform" {
        // The screen only passes constants, so this rarely fires — but
        // the string reaches a toast verbatim, so keep it display-language.
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

/// Advance the chat read marker (read up to this time).
#[tauri::command]
fn ggs_chat_seen(app: State<App>, at: u64) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::ChatSeen(at))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_set_engine(
    app: State<App>,
    depth: u32,
    solve: u8,
    band: u8,
    ponder: bool,
) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetEngine {
            depth,
            solve,
            band,
            ponder,
        })
        .map_err(|e| e.to_string())
}

/// Time-usage settings (pacing, per-move cap, reserve).
#[tauri::command]
fn ggs_set_pacing(
    app: State<App>,
    pace: String,
    max_move_secs: u64,
    reserve_secs: u64,
    budget_use: f64,
) -> Result<(), String> {
    require_calibration()?;
    ggs_tx(&app)?
        .send(ggs::Cmd::SetPacing {
            pace,
            max_move_secs,
            reserve_secs,
            budget_use,
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

/// Whether GGS games feed book learning.
#[tauri::command]
fn ggs_set_learn(app: State<App>, on: bool) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetLearn(on))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_set_standby(app: State<App>, cfg: ggs::StandbyCfg) -> Result<(), String> {
    // Turning things off is never blocked; being stuck uncalibrated is worse.
    if cfg.enabled {
        require_calibration()?;
    }
    ggs_tx(&app)?
        .send(ggs::Cmd::SetStandby(cfg))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_snapshot(app: State<App>) -> Result<ggs::Snapshot, String> {
    /* Screenshot hook: render the GGS screens without connecting (lobby
    and results need opponents and were never verifiable). Actions are
    inert. */
    if let Ok(v) = std::env::var("KUROOBI_GGS_DEMO") {
        let mut s = ggs::demo_snapshot();
        /* `=empty` clears the fixtures — empty states were otherwise
        unphotographable. */
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

/// Whether rated play is forbidden (`KUROOBI_NO_RATED=1`). The screen
/// disables the toggle, and the sender enforces it too — a stale screen
/// cannot start a rated game.
#[tauri::command]
fn ggs_no_rated() -> bool {
    ggs::no_rated()
}

/// Active override environment variables, shown in the status strip.
/// With overrides, what the screen shows may be fixture data or an
/// altered configuration — the strip marks "not a plain launch".
/// Nothing here needs masking (no variable carries credentials).
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

/// Screenshot hook: screen to open at launch (KUROOBI_GGS_AUTOVIEW).
#[tauri::command]
fn ggs_autoview() -> String {
    std::env::var("KUROOBI_GGS_AUTOVIEW").unwrap_or_default()
}

/// Save the protocol log to a file; separate from `ggs_save_kifu`
/// (different filters and default names).
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

/// Save a GGS record (passed as a string) to a file.
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
            clocks: Mutex::new(Clocks::default()),
            engine: Arc::new(Mutex::new(None)),
            stop: Arc::new(Mutex::new(None)),
            ggs: Mutex::new(None),
            learn_on: Mutex::new(true),
            activity: Arc::new(Mutex::new(Activity::default())),
            cpu_meter: Mutex::new(None),
        })
        .setup(|app| {
            // Hand GGS the local stop handle and activity record so a
            // starting GGS game can halt local searches.
            let st = app.state::<App>();
            /* Fixture mode (`KUROOBI_GGS_DEMO`) never starts the
            session — it would stream "disconnected" snapshots over the
            fixtures. */
            if std::env::var("KUROOBI_GGS_DEMO").is_ok() {
                return Ok(());
            }
            /* Calibrate solve speed at startup; once a game runs the
            CPU cannot be spared. */
            calibrate_missing(
                app.handle().clone(),
                st.engine.clone(),
                st.stop.clone(),
                st.activity.clone(),
                // Local and GGS thread settings differ; their defaults
                // coincide, so usually one measurement covers both.
                vec![
                    resources().threads.unwrap_or_else(auto_threads),
                    auto_threads(),
                ],
            );
            let handle = ggs::spawn(app.handle().clone(), st.stop.clone(), st.activity.clone());
            // Auto-login with stored credentials at startup. Screenshot
            // automation and demo modes skip the real server
            // (KUROOBI_GGS_AUTOCONNECT=1 forces it); if another window
            // is connected, silently defer to it.
            let force = std::env::var("KUROOBI_GGS_AUTOCONNECT").is_ok();
            let demo = std::env::var("KUROOBI_AUTOPLAY").is_ok()
                || std::env::var("KUROOBI_GGS_AUTOVIEW").is_ok();
            if (force || !demo) && !ggs::session_locked_by_other() {
                if let Some((l, p)) = saved_credentials() {
                    let _ = handle.tx.send(ggs::Cmd::Connect { login: l, pw: p });
                }
            }
            if force {
                // KUROOBI_GGS_AUTOLOOK=<id>: also fetch a record
                // (exercises that path without UI interaction).
                if let Ok(id) = std::env::var("KUROOBI_GGS_AUTOLOOK") {
                    let tx = handle.tx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(12));
                        let _ = tx.send(ggs::Cmd::Look(id));
                    });
                }
                // KUROOBI_GGS_AUTOWATCH=<ids>/auto: also start watching.
                if let Ok(ids) = std::env::var("KUROOBI_GGS_AUTOWATCH") {
                    let tx = handle.tx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(12));
                        if ids.trim() == "auto" {
                            // Refresh the list; the session watches the
                            // batch when it arrives.
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
            clocks,
            set_clock,
            state,
            new_game,
            play,
            undo,
            goto,
            set_levels,
            set_use_book,
            local_threads,
            set_local_threads,
            hash_sizes,
            set_hash_sizes,
            calibrate_nps,
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
            ggs_chat_seen,
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

    /// Hand-off form: line 1 = start position, line 2 = moves.
    #[test]
    fn reads_start_position_and_kifu() {
        let start = Board::new().to_string();
        let text = format!("{start}\nf5d6c3\n");
        assert_eq!(
            extract_start(&text).as_deref(),
            Some(start.replace(' ', "").as_str())
        );
        // The start-position line must not be scanned as moves.
        let g = game_from_text(&text).expect("読めること");
        assert_eq!(g.move_count(), 3);
    }

    /// Without a start position, begin from the standard start.
    #[test]
    fn plain_kifu_still_works() {
        assert!(extract_start("f5d6c3").is_none());
        let g = game_from_text("f5d6c3").expect("読めること");
        assert_eq!(g.move_count(), 3);
    }

    /// The GGF BO tag also yields a start position.
    #[test]
    fn reads_start_from_ggf() {
        let start = Board::new().to_string();
        let text = format!("(;GM[Othello]PC[GGS/os]BO[8 {start}]B[F5]W[D6];)");
        assert_eq!(
            extract_start(&text).as_deref(),
            Some(start.replace(' ', "").as_str())
        );
    }

    /// A drawn start position plus moves reproduces the original game.
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

    /// GGS-flavored GGF; the BO tag makes it round-trippable.
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

    /// Coordinate-like characters in player names must not become moves.
    #[test]
    fn ggf_ignores_coordinates_inside_names() {
        let ggf = "(;GM[Othello]PB[player1]PW[a1ice]\
                   BO[8 ---------------------------O*------*O--------------------------- *]\
                   B[F5];)";
        let g = game_from_text(ggf).expect("読めること");
        assert_eq!(g.move_count(), 1, "名前の a1 / r1 を手にしない");
    }

    /// A drawn-opening GGF reproduces the original game.
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

    /// Parse real `look` output (with evals and time spent).
    #[test]
    fn reads_ggf_from_ggs_archive() {
        let ggf = "(;GM[Othello]PC[GGS/os]DT[2026.07.30_17:36:36.MDT]PB[kuroobi]PW[fly]\
                   RB[1720]RW[1438.62]TI[15:00//02:00]TY[8]RE[+54.000]\
                   BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]\
                   B[E6]W[f4/-25.99/0.20]B[C3]W[d6/-25.99/0.04]B[F6]W[e7/-25.99/0.02];)";
        let g = game_from_text(ggf).expect("読めること");
        assert_eq!(g.move_count(), 6);
        // Mixed case, evals and times present — extract moves only.
        let plain = Reversi::from_kifu("e6f4c3d6f6e7").unwrap();
        assert_eq!(g.board.black, plain.board.black);
        assert_eq!(g.board.white, plain.board.white);
    }

    /// Real `look` output: one full game including a pass.
    #[test]
    fn replays_a_whole_archived_game() {
        let ggf = "(;GM[Othello]PC[GGS/os]DT[2026.07.30_17:36:36.MDT]PB[kuroobi]PW[fly]RB[1720]RW[1438.62]TI[15:00//02:00]TY[8]RE[+54.000]BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]B[E6]W[f4/-25.99/0.20]B[C3]W[d6/-25.99/0.04]B[F6]W[e7/-25.99/0.02]B[F5]W[g5/-25.99]B[E3]W[g4/-28.23]B[C7]W[d3/24.23]B[F3]W[c4/0.23]B[C6]W[c5/-7.29]B[B4]W[b6/-7.81]B[D7]W[b5/-8.06]B[C2]W[a3/-7.84]B[F8]W[e8/-11.18]B[D8]W[c8/-15.07]B[B8]W[d2/-19.02]B[G3]W[e2/-19.46]B[A6]W[c1/-20.24]B[D1]W[e1/-20.44]B[F2]W[f1/-17.83]B[F7]W[h3/-18.89]B[A5]W[a7/-29.57]B[A8]W[b7/-35.51]B[G2]W[g8/-38.82]B[H8]W[g1/-45.44]B[B3]W[a4/-38.31]B[A2]W[b2]B[A1]W[b1]B[G7]W[g6]B[H6]W[h7]B[H5]W[h4]B[H2]W[pass]B[H1];)";
        let g = game_from_text(ggf).expect("読めること");
        assert!(g.board.is_game_over(), "終局まで再生できる");
        // GGF RE[+54.000] is the disc difference from Black's view.
        let (b, w) = (
            g.board.black.count_ones() as i32,
            g.board.white.count_ones() as i32,
        );
        assert_eq!(b - w, 54, "結果が RE と一致する");
        assert!(ggf.contains("W[pass]"), "終盤にパスが入っている局");
    }

    /* ---- GGF writing ---- */

    /// Written GGF reads back into the original game.
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

    /// A game with a pass round-trips without turn drift.
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

    /// Drawn-opening games write their start into BO (not the standard).
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

    /// A `]` in a name must not break the tag.
    #[test]
    fn ggf_escapes_bracket_in_names() {
        let g = Reversi::from_kifu("e6").unwrap();
        let ggf = to_ggf(&g, "a]b", "c");
        assert!(!ggf.contains("a]b"), "そのまま入れるとタグが切れる");
        assert_eq!(game_from_text(&ggf).unwrap().move_count(), 1);
    }
}
