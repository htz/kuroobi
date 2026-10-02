//! Kuroobi's GUI (Tauri).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ggs;
mod i18n;
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
    stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    ggs: Mutex<Option<ggs::Handle>>,
    learn_on: Mutex<bool>,
    activity: Arc<Mutex<Activity>>,
    cpu_meter: Mutex<Option<(std::time::Instant, std::time::Duration)>>,
    clocks: Mutex<Clocks>,
}

#[derive(Default)]
struct Clocks {
    total: u64,
    black: f64,
    white: f64,
    lost: Option<kuroobi::Color>,
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

fn process_cpu_time() -> std::time::Duration {
    unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        let secs = (ru.ru_utime.tv_sec + ru.ru_stime.tv_sec).max(0) as u64;
        let micros = (ru.ru_utime.tv_usec + ru.ru_stime.tv_usec).max(0) as u64;
        std::time::Duration::from_secs(secs) + std::time::Duration::from_micros(micros)
    }
}

#[cfg(target_os = "macos")]
fn process_memory() -> u64 {
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

#[derive(Default)]
pub(crate) struct Activity {
    pub(crate) local: Option<&'static str>,
    learn: Option<(u32, u32)>,
    learn_paused: bool,
}

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

fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(4)
}

fn ggs_snap_arc(app: &State<App>) -> Option<Arc<Mutex<ggs::Snapshot>>> {
    app.ggs.lock().unwrap().as_ref().map(|h| h.snapshot.clone())
}

fn ggs_match_in(snap: &Option<Arc<Mutex<ggs::Snapshot>>>) -> bool {
    snap.as_ref().is_some_and(|s| {
        s.lock()
            .unwrap()
            .matches
            .iter()
            .any(|m| !m.my_color.is_empty() && !m.over)
    })
}

fn ggs_match_active(app: &State<App>) -> bool {
    ggs_match_in(&ggs_snap_arc(app))
}

fn same_board(a: &Board, b: &Board) -> bool {
    a.black == b.black && a.white == b.white && a.player() == b.player()
}

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

#[derive(Serialize, Clone)]
struct GameView {
    cells: Vec<u8>,
    player: String,
    legal: Vec<u8>,
    black: u8,
    white: u8,
    over: bool,
    last: Option<u8>,
    kifu: String,
    move_count: usize,
    moves: Vec<Option<u8>>,
    cursor: usize,
}

#[derive(Serialize)]
struct ThinkView {
    pos: Option<u8>,
    value: f32,
    exact: bool,
    from_book: bool,
    learned: bool,
    secs: f32,
    nodes: u64,
}

#[derive(Serialize, Clone)]
struct HintView {
    pos: u8,
    value: f32,
    exact: bool,
    from_book: bool,
    depth: u32,
}

#[derive(Serialize)]
struct EvalPoint {
    n: usize,
    value: f32,
    exact: bool,
    from_book: bool,
}

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

fn auto_pass(game: &mut Reversi) {
    while !game.is_game_over() && game.movable() == 0 {
        if game.pass().is_err() {
            break;
        }
    }
}

fn resources_path() -> PathBuf {
    let base =
        dirs_config().unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/..")));
    base.join("kuroobi").join("resources.conf")
}

fn learn_log_path() -> PathBuf {
    if let Ok(p) = std::env::var("KUROOBI_LEARN_LOG") {
        return PathBuf::from(p);
    }
    let base =
        dirs_config().unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/..")));
    base.join("kuroobi").join("learn_log.jsonl")
}

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

fn preload_engine(
    engine_slot: Arc<Mutex<Option<Engine>>>,
    stop_slot: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
    activity: Arc<Mutex<Activity>>,
) {
    std::thread::spawn(move || {
        let _guard = ActivityGuard::begin(&activity, "loading");
        let _ = ensure_engine_in(&engine_slot, &stop_slot);
    });
}

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
            if activity.lock().unwrap().local.is_some() {
                return;
            }
            let nps = {
                let _g = ActivityGuard::begin(&activity, "calibrating");
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
                use tauri::Emitter;
                let _ = app.emit("resources-changed", ());
            }
        }
    });
}

fn setup_error(e: String) -> String {
    let shape = !e.contains("(os error");
    let key = if e.starts_with("nnue ") {
        if shape {
            "err.nnue_weights_shape"
        } else {
            "err.nnue_weights_missing"
        }
    } else if e.starts_with("weights ") {
        "err.linear_weights_missing"
    } else {
        return e;
    };
    let path = e
        .split_once(' ')
        .and_then(|(_, rest)| rest.split_once(": "))
        .map(|(p, _)| p)
        .unwrap_or("");
    format!("{key}|path={path}")
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
    if let Some(e) = app.engine.lock().unwrap().as_mut() {
        e.clear_tables();
    }
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
    loop {
        game.undo().map_err(|e| format!("{e:?}"))?;
        let placed = game.history.last().map(|r| r.pos.is_some());
        if game.history.is_empty() || placed != Some(false) {
            break;
        }
    }
    Ok(view(&game))
}

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

#[tauri::command]
fn stop_search(app: State<App>) -> Result<(), String> {
    if let Some(h) = app.stop.lock().unwrap().as_ref() {
        h.stop();
    }
    Ok(())
}

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

#[tauri::command]
fn autoplay() -> String {
    std::env::var("KUROOBI_AUTOPLAY").unwrap_or_default()
}

#[tauri::command]
fn theme_override() -> String {
    std::env::var("KUROOBI_THEME").unwrap_or_default()
}

#[tauri::command]
fn lang_override() -> String {
    std::env::var("KUROOBI_LANG").unwrap_or_default()
}

#[tauri::command]
fn system_lang() -> String {
    sys_locale::get_locale().unwrap_or_default()
}

#[tauri::command]
fn has_book() -> bool {
    resources().book_path().exists()
}

#[tauri::command]
fn set_learn(app: State<App>, on: bool) {
    *app.learn_on.lock().unwrap() = on;
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LearnEntry {
    pub at: u64,
    pub kifu: String,
    pub black: u8,
    pub white: u8,
    pub positions: u32,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub changes: Vec<LearnChange>,
    #[serde(default)]
    pub opponent: String,
    #[serde(default)]
    pub my_color: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LearnChange {
    pub ply: usize,
    pub mv: String,
    pub before: Option<f32>,
    pub after: f32,
    pub best: f32,
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

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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

const LEARN_LOG_MAX: usize = 200;

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
    out.truncate(200);
    out
}

#[tauri::command]
async fn learn_undo(app: State<'_, App>, at: u64, kifu: String) -> Result<usize, String> {
    let log = learn_log();
    let e = log
        .into_iter()
        .find(|e| e.at == at && e.kifu == kifu)
        .ok_or("err.game_not_in_archive")?;
    if e.changes.is_empty() {
        return Err("err.no_change_detail".into());
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
        let engine = guard.as_mut().ok_or("err.engine_not_ready")?;
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

#[tauri::command]
fn learn_game(app: State<App>, my_color: String) -> Result<(), String> {
    if !*app.learn_on.lock().unwrap() {
        return Ok(());
    }
    let (kifu, board) = {
        let game = app.game.lock().unwrap();
        if !game.is_game_over() {
            return Err("err.game_not_over".into());
        }
        (game.to_kifu(), game.board)
    };
    let (_, fin) = kuroobi::learn::replay(None, &kifu)?;
    if fin.black != board.black || fin.white != board.white {
        return Err("err.not_from_standard_start".into());
    }
    let eng = app.engine.clone();
    let stop = app.stop.clone();
    let act = app.activity.clone();
    let ggs_snap = ggs_snap_arc(&app);
    tauri::async_runtime::spawn_blocking(move || {
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
        let mut changes: Vec<kuroobi::learn::BackupChange> = Vec::new();
        let mut last_yield: Option<std::time::Instant> = None;
        const YIELD_HOLD: std::time::Duration = std::time::Duration::from_millis(1500);
        loop {
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
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        learn_log_append(&LearnEntry {
            at: now_secs(),
            kifu,
            black: board.black.count_ones() as u8,
            white: board.white.count_ones() as u8,
            positions: total.saturating_sub(job.remaining() as u32),
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

#[derive(Serialize)]
struct ThreadsView {
    set: Option<u32>,
    auto: u32,
    nps: Option<f64>,
}

fn threads_view() -> ThreadsView {
    let r = resources();
    let now = r.threads.unwrap_or_else(auto_threads);
    ThreadsView {
        set: r.threads.map(|n| n as u32),
        auto: auto_threads() as u32,
        nps: r.nps_for(now),
    }
}

#[tauri::command]
fn local_threads() -> ThreadsView {
    threads_view()
}

#[derive(Serialize)]
struct HashView {
    mid: u32,
    end: u32,
    min: u32,
    max: u32,
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

#[tauri::command]
async fn calibrate_nps(app: State<'_, App>) -> Result<ThreadsView, String> {
    ensure_engine(&app)?;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let (nps, threads) = tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "calibrating");
        let mut guard = eng.lock().unwrap();
        let e = guard.as_mut().unwrap();
        let threads = e.config().threads;
        (e.measure_solve_nps(), threads)
    })
    .await
    .map_err(|e| e.to_string())?;
    if nps <= 0.0 {
        return Err("err.solve_speed_unmeasurable".into());
    }
    let mut r = resources();
    r.set_nps(threads, nps);
    r.save(&resources_path())?;
    Ok(threads_view())
}

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
    if let Ok(tx) = ggs_tx(&app) {
        let _ = tx.send(ggs::Cmd::ReloadThreads);
    }
    calibrate_missing(
        handle,
        eng,
        app.stop.clone(),
        app.activity.clone(),
        vec![threads],
    );
    Ok(())
}

#[derive(Serialize)]
struct ActivityView {
    local: Option<String>,
    local_threads: u32,
    learn: Option<(u32, u32)>,
    learn_paused: bool,
    ggs_match: bool,
    ggs_thinking: bool,
    ggs_threads: u32,
    cpu: f32,
    cores: u32,
    mem: u64,
    mem_total: u64,
}

#[tauri::command]
fn activity_status(app: State<App>) -> ActivityView {
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

#[tauri::command]
fn resource_status() -> Vec<(String, String, bool, u64, String)> {
    resources()
        .detailed()
        .into_iter()
        .map(|(n, p, ok, size, kind)| (n.to_string(), p.display().to_string(), ok, size, kind))
        .collect()
}

#[tauri::command]
async fn pick_resource(handle: tauri::AppHandle, kind: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let d = handle.dialog().file();
    let picked = match kind.as_str() {
        "dir" => d.blocking_pick_folder().map(|p| p.to_string()),
        "book" => d
            .add_filter(i18n::t("backend.filter.book"), &["txt"])
            .blocking_pick_file()
            .map(|p| p.to_string()),
        _ => d
            .add_filter(i18n::t("backend.filter.weights"), &["bin"])
            .blocking_pick_file()
            .map(|p| p.to_string()),
    };
    Ok(picked)
}

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
    let (eng, stop, act) = (app.engine.clone(), app.stop.clone(), app.activity.clone());
    let dropped = tauri::async_runtime::spawn_blocking({
        let (eng, stop) = (eng.clone(), stop.clone());
        move || {
            *eng.lock().unwrap() = None;
            *stop.lock().unwrap() = None;
        }
    })
    .await
    .map_err(|e| e.to_string());
    preload_engine(eng, stop, act);
    dropped
}

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

#[derive(Serialize)]
struct ClockView {
    total: u64,
    black: f64,
    white: f64,
    lost: Option<String>,
}

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

#[tauri::command]
fn set_clock(app: State<App>, secs: u64) -> ClockView {
    app.clocks.lock().unwrap().reset(secs);
    clocks(app)
}

#[tauri::command]
async fn think(app: State<'_, App>) -> Result<ThinkView, String> {
    if ggs_match_active(&app) {
        return Err("err.busy_ggs_search".into());
    }
    ensure_engine(&app)?;
    let board = app.game.lock().unwrap().board;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let stop = app.stop.clone();
    let (base, plan) = {
        let c = app.clocks.lock().unwrap();
        let e = app.engine.lock().unwrap();
        let cfg = e.as_ref().unwrap().config();
        let base = kuroobi::timectl::Levels {
            depth: cfg.depth,
            solve: cfg.solve_empties,
            band: cfg.band,
        };
        let threads = cfg.threads;
        drop(e);
        let plan = (c.total > 0).then(|| {
            kuroobi::timectl::plan(
                kuroobi::timectl::Situation {
                    clock_secs: Some(c.left(board.player()) as u64),
                    empties: board.empty_count(),
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
        let _g = ActivityGuard::begin(&act, "thinking");
        let mut guard = eng.lock().unwrap();
        let t0 = std::time::Instant::now();
        let n0 = guard.as_ref().unwrap().nodes();
        let e = guard.as_mut().unwrap();
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

#[tauri::command]
fn apply_move(app: State<App>, sq: Option<u8>) -> Result<GameView, String> {
    let mut game = app.game.lock().unwrap();
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

#[tauri::command]
async fn ponder_live(app: State<'_, App>) -> Result<(), String> {
    if ggs_match_active(&app) {
        return Err("err.busy_ggs".into());
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
        let _g = ActivityGuard::begin(&act, "pondering");
        let mut guard = eng.lock().unwrap();
        let Some(e) = guard.as_mut() else { return };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
        e.ponder(&board, until);
    });
    Ok(())
}

#[tauri::command]
async fn analyze_live(app: State<'_, App>, handle: tauri::AppHandle) -> Result<(), String> {
    if ggs_match_active(&app) {
        return Err("err.busy_ggs_analysis".into());
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
        let _g = ActivityGuard::begin(&act, "analyzing");
        let mut guard = eng.lock().unwrap();
        let Some(e) = guard.as_mut() else { return };
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
            handle
                .emit("hints", (depth, view, nodes, t0.elapsed().as_secs_f32()))
                .is_ok()
        });
    });
    Ok(())
}

#[tauri::command]
async fn eval_at(app: State<'_, App>, n: usize, depth: u32) -> Result<EvalPoint, String> {
    if ggs_match_active(&app) {
        return Err("err.busy_ggs_analysis".into());
    }
    ensure_engine(&app)?;
    let board = board_at_line(&mut app.game.lock().unwrap(), n)?;
    let white = board.player() == Color::White;
    let eng = app.engine.clone();
    let act = app.activity.clone();
    let stop = app.stop.clone();
    if let Some(h) = stop.lock().unwrap().as_ref() {
        h.reset();
    }
    let (value, exact, from_book, searched) = tauri::async_runtime::spawn_blocking(move || {
        let _g = ActivityGuard::begin(&act, "analyzing");
        let mut guard = eng.lock().unwrap();
        let e = guard.as_mut().unwrap();
        let from_book = e.book_base_value(&board);
        if let Some(v) = from_book {
            return (v, false, true, false);
        }
        let mv = e.eval_position(&board, depth);
        (mv.value, mv.exact, false, true)
    })
    .await
    .map_err(|e| e.to_string())?;
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
        return Err("err.record_empty".into());
    }
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter(i18n::t("backend.filter.record"), &["ggf", "txt", "kifu"])
        .set_file_name("kuroobi_game.ggf")
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let p = path.into_path().map_err(|e| e.to_string())?;
    let ggf_out = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ggf"));
    let body = if ggf_out { ggf } else { format!("{kifu}\n") };
    std::fs::write(&p, body).map_err(|e| e.to_string())?;
    Ok(Some(p.display().to_string()))
}

fn to_ggf(game: &Reversi, black: &str, white: &str) -> String {
    let start = start_board(game);
    let mut out = String::from("(;GM[Othello]PC[KUROOBI]");
    out.push_str(&format!("DT[{}]", ggf_now()));
    out.push_str(&format!("PB[{}]PW[{}]", ggf_text(black), ggf_text(white)));
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
            None => out.push_str(&format!("{tag}[PA]")),
            Some(p) => out.push_str(&format!("{}[{}]", tag, p.to_kifu().to_uppercase())),
        }
        color = color.opponent();
    }
    out.push_str(";)\n");
    out
}

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

fn ggf_text(s: &str) -> String {
    s.replace(']', ")")
}

fn ggf_now() -> String {
    let secs = now_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
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

fn extract_start(text: &str) -> Option<String> {
    let cell = |c: char| matches!(c.to_ascii_lowercase(), '-' | '.' | 'x' | 'o' | '*');
    let side = |c: char| matches!(c.to_ascii_lowercase(), 'x' | 'o' | '*');
    let ggf = text.find("BO[").map(|i| &text[i + 3..]).and_then(|rest| {
        let end = rest.find(']')?;
        let inner = rest[..end].trim_start_matches('8').trim();
        Some(inner.to_string())
    });
    for cand in ggf.into_iter().chain(text.lines().map(|l| l.to_string())) {
        let c: Vec<char> = cand.chars().filter(|c| !c.is_whitespace()).collect();
        if c.len() == 65 && c[..64].iter().all(|&x| cell(x)) && side(c[64]) {
            return Some(c.into_iter().collect());
        }
    }
    None
}

fn parse_ggf(text: &str) -> Option<(Option<String>, String)> {
    let body = {
        let start = text.find("(;")?;
        let rest = &text[start + 2..];
        let end = rest.find(";)").unwrap_or(rest.len());
        &rest[..end]
    };
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
        return Err("err.record_not_found".into());
    }
    let loaded = match &start {
        Some(b) => Reversi::from_kifu_with_start(b, &s),
        None => Reversi::from_kifu(&s),
    }
    .map_err(|e| {
        eprintln!("cannot parse game record: {e}");
        "err.record_unreadable".to_string()
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
        .add_filter(i18n::t("backend.filter.record"), &["ggf", "txt", "kifu"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let p = path.into_path().map_err(|e| e.to_string())?;
    let s = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    load_kifu_into(&app, &s).map(Some)
}

#[tauri::command]
fn load_kifu_text(app: State<App>, text: String) -> Result<GameView, String> {
    load_kifu_into(&app, &text)
}

#[derive(Serialize)]
struct KifuFrame {
    cells: Vec<u8>,
    last: Option<u8>,
    black: u8,
    white: u8,
    player: String,
}

#[derive(Serialize)]
struct BookMoveView {
    pos: u8,
    value: f32,
    games: u32,
}

#[derive(Serialize)]
struct BookNodeView {
    cells: Vec<u8>,
    player: String,
    black: u8,
    white: u8,
    moves: Vec<BookMoveView>,
    learned: bool,
    value: Option<f32>,
    depth: Option<u8>,
    size: usize,
    learned_size: usize,
}

#[tauri::command]
async fn book_node(app: State<'_, App>, kifu: String) -> Result<BookNodeView, String> {
    let game = if kifu.trim().is_empty() {
        Reversi::new()
    } else {
        game_from_text(&kifu).ok_or("err.record_parse_failed")?
    };
    let b = game.board;
    let player = game.player();
    let eng = app.engine.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = eng.lock().unwrap();
        let e = guard.as_mut().ok_or("err.engine_not_ready")?;
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
    let mut game = game_from_text(&text).ok_or("err.record_parse_failed")?;
    let line = game.line();
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

fn take_handoff_kifu() -> Option<String> {
    let path = std::env::temp_dir().join("kuroobi_handoff.txt");
    let s = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    (!s.trim().is_empty()).then_some(s)
}

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

fn ggs_tx(app: &State<App>) -> Result<std::sync::mpsc::Sender<ggs::Cmd>, String> {
    app.ggs
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| h.tx.clone())
        .ok_or_else(|| "err.ggs_session_not_started".into())
}

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
        return Err("err.login_and_password_required".into());
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

#[tauri::command]
fn set_backend_strings(strings: std::collections::HashMap<String, String>) {
    i18n::set(strings);
}

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

fn require_calibration() -> Result<(), String> {
    let t = resources().threads.unwrap_or_else(auto_threads);
    if resources().nps_for(t).is_none() {
        return Err(format!("err.solve_speed_not_measured|threads={t}"));
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

#[tauri::command]
fn ggs_ack(app: State<App>) -> Result<(), String> {
    let snap = ggs_snap_arc(&app).ok_or("err.ggs_not_connected")?;
    let mut s = snap.lock().unwrap();
    s.notice.clear();
    s.fetched_ggf = None;
    Ok(())
}

#[tauri::command]
fn ggs_look(app: State<App>, id: String) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::Look(id))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_chat(app: State<App>, target: String, text: String) -> Result<(), String> {
    if target.trim().is_empty() || text.trim().is_empty() {
        return Err("err.chat_target_and_text_required".into());
    }
    ggs_tx(&app)?
        .send(ggs::Cmd::Chat {
            target: target.trim().into(),
            text: text.trim().into(),
        })
        .map_err(|e| e.to_string())
}

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

#[tauri::command]
fn ggs_set_formula(app: State<App>, kind: String, expr: String) -> Result<(), String> {
    if kind != "aform" && kind != "dform" {
        return Err(format!("err.bad_formula_kind|kind={kind}"));
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

#[tauri::command]
fn ggs_set_learn(app: State<App>, on: bool) -> Result<(), String> {
    ggs_tx(&app)?
        .send(ggs::Cmd::SetLearn(on))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_set_standby(app: State<App>, cfg: ggs::StandbyCfg) -> Result<(), String> {
    if cfg.enabled {
        require_calibration()?;
    }
    ggs_tx(&app)?
        .send(ggs::Cmd::SetStandby(cfg))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn ggs_snapshot(app: State<App>) -> Result<ggs::Snapshot, String> {
    if let Ok(v) = std::env::var("KUROOBI_GGS_DEMO") {
        let mut s = ggs::demo_snapshot();
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
            .ok_or_else(|| "err.ggs_session_not_started".to_string())?;
        h.snapshot.clone()
    };
    let s = snap.lock().unwrap().clone();
    Ok(s)
}

#[tauri::command]
fn ggs_no_rated() -> bool {
    ggs::no_rated()
}

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

#[tauri::command]
fn ggs_autoview() -> String {
    std::env::var("KUROOBI_GGS_AUTOVIEW").unwrap_or_default()
}

#[tauri::command]
async fn ggs_save_log(handle: tauri::AppHandle, text: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    if text.is_empty() {
        return Err("err.log_empty".into());
    }
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter(i18n::t("backend.filter.log"), &["log", "txt"])
        .set_file_name("ggs-log.txt")
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let p = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| e.to_string())?;
    Ok(Some(p.display().to_string()))
}

#[tauri::command]
async fn ggs_save_kifu(
    handle: tauri::AppHandle,
    kifu: String,
    name: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    if kifu.is_empty() {
        return Err("err.record_empty".into());
    }
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter(i18n::t("backend.filter.record"), &["ggf", "txt", "kifu"])
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
            let st = app.state::<App>();
            if std::env::var("KUROOBI_GGS_DEMO").is_ok() {
                return Ok(());
            }
            preload_engine(st.engine.clone(), st.stop.clone(), st.activity.clone());
            calibrate_missing(
                app.handle().clone(),
                st.engine.clone(),
                st.stop.clone(),
                st.activity.clone(),
                vec![
                    resources().threads.unwrap_or_else(auto_threads),
                    auto_threads(),
                ],
            );
            let handle = ggs::spawn(app.handle().clone(), st.stop.clone(), st.activity.clone());
            let force = std::env::var("KUROOBI_GGS_AUTOCONNECT").is_ok();
            let demo = std::env::var("KUROOBI_AUTOPLAY").is_ok()
                || std::env::var("KUROOBI_GGS_AUTOVIEW").is_ok();
            if (force || !demo) && !ggs::session_locked_by_other() {
                if let Some((l, p)) = saved_credentials() {
                    let _ = handle.tx.send(ggs::Cmd::Connect { login: l, pw: p });
                }
            }
            if force {
                if let Ok(id) = std::env::var("KUROOBI_GGS_AUTOLOOK") {
                    let tx = handle.tx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(12));
                        let _ = tx.send(ggs::Cmd::Look(id));
                    });
                }
                if let Ok(ids) = std::env::var("KUROOBI_GGS_AUTOWATCH") {
                    let tx = handle.tx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(12));
                        if ids.trim() == "auto" {
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
            set_backend_strings,
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
            lang_override,
            system_lang,
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

    #[test]
    fn shape_and_absence_are_different_errors() {
        assert_eq!(
            setup_error("nnue weights/nnue-h64.bin: accumulator width mismatch".into()),
            "err.nnue_weights_shape|path=weights/nnue-h64.bin"
        );
        assert_eq!(
            setup_error("nnue weights/x.bin: No such file or directory (os error 2)".into()),
            "err.nnue_weights_missing|path=weights/x.bin"
        );
        assert_eq!(
            setup_error(
                "weights weights/linear.bin: No such file or directory (os error 2)".into()
            ),
            "err.linear_weights_missing|path=weights/linear.bin"
        );
    }

    #[test]
    fn reads_start_position_and_kifu() {
        let start = Board::new().to_string();
        let text = format!("{start}\nf5d6c3\n");
        assert_eq!(
            extract_start(&text).as_deref(),
            Some(start.replace(' ', "").as_str())
        );
        let g = game_from_text(&text).expect("parses");
        assert_eq!(g.move_count(), 3);
    }

    #[test]
    fn plain_kifu_still_works() {
        assert!(extract_start("f5d6c3").is_none());
        let g = game_from_text("f5d6c3").expect("parses");
        assert_eq!(g.move_count(), 3);
    }

    #[test]
    fn reads_start_from_ggf() {
        let start = Board::new().to_string();
        let text = format!("(;GM[Othello]PC[GGS/os]BO[8 {start}]B[F5]W[D6];)");
        assert_eq!(
            extract_start(&text).as_deref(),
            Some(start.replace(' ', "").as_str())
        );
    }

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
        let g = game_from_text(&format!("{start}\n{kifu}")).expect("parses");
        assert_eq!(g.board.black, drawn.board.black);
        assert_eq!(g.board.white, drawn.board.white);
    }

    #[test]
    fn reads_ggf() {
        let ggf = "(;GM[Othello]PC[GGS/os]PB[nyanyan]RB[2658.9]PW[egrcd]RW[2585.8]\
                   BO[8 ---------------------------O*------*O--------------------------- *]\
                   B[F5]W[D6]B[C3];)";
        let g = game_from_text(ggf).expect("parses");
        assert_eq!(g.move_count(), 3);
        let plain = Reversi::from_kifu("f5d6c3").unwrap();
        assert_eq!(g.board.black, plain.board.black);
        assert_eq!(g.board.white, plain.board.white);
    }

    #[test]
    fn ggf_ignores_coordinates_inside_names() {
        let ggf = "(;GM[Othello]PB[player1]PW[a1ice]\
                   BO[8 ---------------------------O*------*O--------------------------- *]\
                   B[F5];)";
        let g = game_from_text(ggf).expect("parses");
        assert_eq!(g.move_count(), 1, "a1 / r1 in names are not moves");
    }

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
        let g = game_from_text(&ggf).expect("parses");
        assert_eq!(g.board.black, drawn.board.black);
        assert_eq!(g.board.white, drawn.board.white);
    }

    #[test]
    fn reads_ggf_from_ggs_archive() {
        let ggf = "(;GM[Othello]PC[GGS/os]DT[2026.07.30_17:36:36.MDT]PB[kuroobi]PW[fly]\
                   RB[1720]RW[1438.62]TI[15:00//02:00]TY[8]RE[+54.000]\
                   BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]\
                   B[E6]W[f4/-25.99/0.20]B[C3]W[d6/-25.99/0.04]B[F6]W[e7/-25.99/0.02];)";
        let g = game_from_text(ggf).expect("parses");
        assert_eq!(g.move_count(), 6);
        let plain = Reversi::from_kifu("e6f4c3d6f6e7").unwrap();
        assert_eq!(g.board.black, plain.board.black);
        assert_eq!(g.board.white, plain.board.white);
    }

    #[test]
    fn replays_a_whole_archived_game() {
        let ggf = "(;GM[Othello]PC[GGS/os]DT[2026.07.30_17:36:36.MDT]PB[kuroobi]PW[fly]RB[1720]RW[1438.62]TI[15:00//02:00]TY[8]RE[+54.000]BO[8 -------- -------- -------- ---O*--- ---*O--- -------- -------- -------- *]B[E6]W[f4/-25.99/0.20]B[C3]W[d6/-25.99/0.04]B[F6]W[e7/-25.99/0.02]B[F5]W[g5/-25.99]B[E3]W[g4/-28.23]B[C7]W[d3/24.23]B[F3]W[c4/0.23]B[C6]W[c5/-7.29]B[B4]W[b6/-7.81]B[D7]W[b5/-8.06]B[C2]W[a3/-7.84]B[F8]W[e8/-11.18]B[D8]W[c8/-15.07]B[B8]W[d2/-19.02]B[G3]W[e2/-19.46]B[A6]W[c1/-20.24]B[D1]W[e1/-20.44]B[F2]W[f1/-17.83]B[F7]W[h3/-18.89]B[A5]W[a7/-29.57]B[A8]W[b7/-35.51]B[G2]W[g8/-38.82]B[H8]W[g1/-45.44]B[B3]W[a4/-38.31]B[A2]W[b2]B[A1]W[b1]B[G7]W[g6]B[H6]W[h7]B[H5]W[h4]B[H2]W[pass]B[H1];)";
        let g = game_from_text(ggf).expect("parses");
        assert!(g.board.is_game_over(), "replays to the end of the game");
        let (b, w) = (
            g.board.black.count_ones() as i32,
            g.board.white.count_ones() as i32,
        );
        assert_eq!(b - w, 54, "disc difference matches RE");
        assert!(
            ggf.contains("W[pass]"),
            "this game contains an endgame pass"
        );
    }

    #[test]
    fn writes_ggf_that_reads_back() {
        let g = Reversi::from_kifu("e6f4c3d6f6e7").unwrap();
        let ggf = to_ggf(&g, "KUROOBI", "Player");
        assert!(ggf.contains("PB[KUROOBI]PW[Player]"), "{ggf}");
        assert!(ggf.contains("RE[?]"), "an unfinished game has no result");
        assert!(
            ggf.contains("B[E6]W[F4]"),
            "moves are upper case and coloured: {ggf}"
        );
        let back = game_from_text(&ggf).expect("parses");
        assert_eq!(back.board.black, g.board.black);
        assert_eq!(back.board.white, g.board.white);
    }

    #[test]
    fn writes_pass_and_result() {
        let src =
            "(;GM[Othello]BO[8 ---------------------------O*------*O--------------------------- *]\
                   B[E6]W[F4]B[C3]W[D6]B[F6]W[E7]B[F5]W[G5]B[E3]W[G4]B[C7]W[D3]B[F3]W[C4]\
                   B[C6]W[C5]B[B4]W[B6]B[D7]W[B5]B[C2]W[A3]B[F8]W[E8]B[D8]W[C8]B[B8]W[D2]\
                   B[G3]W[E2]B[A6]W[C1]B[D1]W[E1]B[F2]W[F1]B[F7]W[H3]B[A5]W[A7]B[A8]W[B7]\
                   B[G2]W[G8]B[H8]W[G1]B[B3]W[A4]B[A2]W[B2]B[A1]W[B1]B[G7]W[G6]B[H6]W[H7]\
                   B[H5]W[H4]B[H2]W[PASS]B[H1];)";
        let g = game_from_text(src).expect("parses");
        assert!(g.board.is_game_over());
        let ggf = to_ggf(&g, "a", "b");
        assert!(
            ggf.contains("W[PA]"),
            "dropping the pass would shift the side to move: {ggf}"
        );
        assert!(
            ggf.contains("RE[+54]"),
            "a finished game records the disc difference: {ggf}"
        );
        let back = game_from_text(&ggf).expect("parses");
        assert_eq!(back.board.black, g.board.black);
        assert_eq!(back.board.white, g.board.white);
    }

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
            "the start position is missing: {ggf}"
        );
        let back = game_from_text(&ggf).expect("parses");
        assert_eq!(back.board.black, g.board.black);
        assert_eq!(back.board.white, g.board.white);
    }

    #[test]
    fn ggf_escapes_bracket_in_names() {
        let g = Reversi::from_kifu("e6").unwrap();
        let ggf = to_ggf(&g, "a]b", "c");
        assert!(
            !ggf.contains("a]b"),
            "an unescaped bracket would cut the tag short"
        );
        assert_eq!(game_from_text(&ggf).unwrap().move_count(), 1);
    }
}
