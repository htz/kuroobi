//! Kuroobi の GUI (Tauri)。エンジンはライブラリとして同一プロセスに
//! リンクし、探索はブロッキングのままワーカースレッド (spawn_blocking) で
//! 回す。フロントは gui/ui/ の静的ページ。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::State;

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::game::Reversi;
use kuroobi::{Board, Color, Position};

struct App {
    game: Mutex<Reversi>,
    engine: Arc<Mutex<Option<Engine>>>,
    /// 探索中でも触れるよう Engine の Mutex の外に置く停止ハンドル。
    stop: Arc<Mutex<Option<kuroobi::midgame::StopHandle>>>,
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
}

#[derive(Serialize)]
struct HintView {
    pos: u8,
    value: f32,
    exact: bool,
}

#[derive(Serialize)]
struct EvalPoint {
    n: usize,
    /// 黒視点の石差。
    value: f32,
    exact: bool,
}

/// 手順 (line) 上の n 手目直後の盤面。redo 側 (現在より先) も辿れるよう、
/// 初期局面から line を再生して作る (GUI の対局は常に標準初期配置)。
fn board_at_line(game: &Reversi, n: usize) -> Result<Board, String> {
    let line = game.line();
    if n > line.len() {
        return Err("out of range".into());
    }
    let mut b = Board::new();
    for e in &line[..n] {
        match e {
            Some(p) => {
                b.make_move(*p).map_err(|e| format!("{e:?}"))?;
            }
            None => b.pass(),
        }
    }
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

fn weights_dir() -> PathBuf {
    if let Ok(d) = std::env::var("KUROOBI_WEIGHTS_DIR") {
        return PathBuf::from(d);
    }
    // 開発時はワークスペースルートで cargo run するのでカレント直下、
    // それ以外はリポジトリ相対を順に試す。
    for c in ["weights", "../weights", "../../weights"] {
        let p = PathBuf::from(c);
        if p.join("nnue_champion.bin").exists() {
            return p;
        }
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../weights"))
}

fn ensure_engine(app: &State<App>) -> Result<(), String> {
    let mut guard = app.engine.lock().unwrap();
    if guard.is_none() {
        let dir = weights_dir();
        let mut cfg = EngineConfig::default();
        cfg.weights = dir.join("weights_full.bin");
        cfg.nnue = dir.join("nnue_champion.bin");
        cfg.threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).max(1))
            .unwrap_or(4);
        let engine = Engine::new(cfg)?;
        *app.stop.lock().unwrap() = Some(engine.stop_handle());
        *guard = Some(engine);
    }
    Ok(())
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

#[tauri::command]
fn set_levels(app: State<App>, depth: u32, solve_empties: u8, band: u8) -> Result<(), String> {
    ensure_engine(&app)?;
    let mut guard = app.engine.lock().unwrap();
    guard.as_mut().unwrap().set_levels(depth, solve_empties, band);
    Ok(())
}

/// 現局面の最善手を計算する。**局面は動かさない** — 適用は `apply_move`。
/// 分離してあるので、思考中に停止された場合はフロントが結果を捨てるだけで
/// 「停止」が成立する。
#[tauri::command]
async fn think(app: State<'_, App>) -> Result<ThinkView, String> {
    ensure_engine(&app)?;
    let board = app.game.lock().unwrap().board;
    let eng = app.engine.clone();
    let mv = tauri::async_runtime::spawn_blocking(move || {
        let mut guard = eng.lock().unwrap();
        guard.as_mut().unwrap().choose(&board)
    })
    .await
    .map_err(|e| e.to_string())?;
    // 思考中に局面が動いていたら無効 (フロント側でも照合する)
    if !same_board(&app.game.lock().unwrap().board, &board) {
        return Err("position changed".into());
    }
    Ok(ThinkView {
        pos: mv.pos.map(|p| p.index()),
        value: finite(mv.value),
        exact: mv.exact,
        from_book: mv.from_book,
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

#[tauri::command]
async fn analyze(app: State<'_, App>, depth: u32) -> Result<Vec<HintView>, String> {
    ensure_engine(&app)?;
    let board = app.game.lock().unwrap().board;
    let eng = app.engine.clone();
    let hints = tauri::async_runtime::spawn_blocking(move || {
        let mut guard = eng.lock().unwrap();
        guard.as_mut().unwrap().analyze(&board, depth)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(hints
        .into_iter()
        .map(|(p, e)| HintView { pos: p.index(), value: finite(e.value), exact: e.exact })
        .collect())
}

/// 手順上の n 手目直後の局面を固定深さで評価する (評価値グラフ用)。
/// 返す値は黒視点。
#[tauri::command]
async fn eval_at(app: State<'_, App>, n: usize, depth: u32) -> Result<EvalPoint, String> {
    ensure_engine(&app)?;
    let board = board_at_line(&app.game.lock().unwrap(), n)?;
    let white = board.player() == Color::White;
    let eng = app.engine.clone();
    let mv = tauri::async_runtime::spawn_blocking(move || {
        let mut guard = eng.lock().unwrap();
        guard.as_mut().unwrap().eval_position(&board, depth)
    })
    .await
    .map_err(|e| e.to_string())?;
    let value = finite(if white { -mv.value } else { mv.value });
    Ok(EvalPoint { n, value, exact: mv.exact })
}

#[tauri::command]
async fn save_kifu(
    app: State<'_, App>,
    handle: tauri::AppHandle,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let kifu = app.game.lock().unwrap().to_kifu();
    if kifu.is_empty() {
        return Err("棋譜が空です".into());
    }
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter("kifu", &["txt", "kifu"])
        .set_file_name("kuroobi_game.txt")
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let p = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&p, format!("{kifu}\n")).map_err(|e| e.to_string())?;
    Ok(Some(p.display().to_string()))
}

/// 貼り付け・ファイル内容から f5 形式の着手列だけを抽出する。
/// 手番号 (`12.`) や空白・区切り文字が混ざっていても「英字 a-h + 数字 1-8」の
/// ペアだけを拾うので壊れない。
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
    let s = extract_kifu(text);
    if s.is_empty() {
        return Err("棋譜が見つかりません".into());
    }
    let loaded = Reversi::from_kifu(&s)?;
    let mut game = app.game.lock().unwrap();
    *game = loaded;
    Ok(view(&game))
}

#[tauri::command]
async fn load_kifu(app: State<'_, App>, handle: tauri::AppHandle) -> Result<Option<GameView>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(path) = handle
        .dialog()
        .file()
        .add_filter("kifu", &["txt", "kifu"])
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

/// 外部から渡された棋譜 (あれば読んで削除)。
fn take_handoff_kifu() -> Option<String> {
    let path = std::env::temp_dir().join("kuroobi_handoff.txt");
    let s = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let s: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    (!s.is_empty()).then_some(s)
}

fn main() {
    let initial = take_handoff_kifu()
        .and_then(|k| Reversi::from_kifu(&k).ok())
        .unwrap_or_else(Reversi::new);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(App {
            game: Mutex::new(initial),
            engine: Arc::new(Mutex::new(None)),
            stop: Arc::new(Mutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            state, new_game, play, undo, goto, set_levels, stop_search, think, apply_move, analyze,
            eval_at, save_kifu, load_kifu, load_kifu_text
        ])
        .run(tauri::generate_context!())
        .expect("tauri run");
}
