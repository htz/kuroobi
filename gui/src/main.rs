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
        let cfg = EngineConfig {
            weights: dir.join("weights_full.bin"),
            nnue: dir.join("nnue_champion.bin"),
            threads: std::thread::available_parallelism()
                .map(|n| (n.get() / 2).max(1))
                .unwrap_or(4),
            ..Default::default()
        };
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
    guard
        .as_mut()
        .unwrap()
        .set_levels(depth, solve_empties, band);
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
        .map(|(p, e)| HintView {
            pos: p.index(),
            value: finite(e.value),
            exact: e.exact,
        })
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
    Ok(EvalPoint {
        n,
        value,
        exact: mv.exact,
    })
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
        .add_filter("棋譜", &["ggf", "txt", "kifu"])
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
    let loaded = match &start {
        Some(b) => Reversi::from_kifu_with_start(b, &s)?,
        None => Reversi::from_kifu(&s)?,
    };
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

fn main() {
    let initial = take_handoff_kifu()
        .as_deref()
        .and_then(game_from_text)
        .unwrap_or_default();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(App {
            game: Mutex::new(initial),
            engine: Arc::new(Mutex::new(None)),
            stop: Arc::new(Mutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            state,
            new_game,
            play,
            undo,
            goto,
            set_levels,
            stop_search,
            think,
            apply_move,
            analyze,
            eval_at,
            save_kifu,
            load_kifu,
            load_kifu_text
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
}
