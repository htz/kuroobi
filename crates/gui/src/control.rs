//! A local control socket so that `kuroobi-mcp` (from the shell or as an MCP server) can drive the app:
//! one JSON request per connection, one JSON reply, then the connection closes.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::{ggs, App};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static FORCED_OFF: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<AppHandle> = OnceLock::new();

fn conf_path() -> std::path::PathBuf {
    crate::config_dir().join("control.conf")
}

/// The saved switch: on unless turned off in Settings.
fn setting_on() -> bool {
    std::fs::read_to_string(conf_path())
        .map_or(true, |t| !t.lines().any(|l| l.trim() == "enabled=false"))
}

/// At launch; `--no-control` keeps the socket closed whatever the setting says.
pub fn init(app: AppHandle, forced_off: bool) {
    let _ = APP.set(app);
    FORCED_OFF.store(forced_off, Relaxed);
    if !forced_off && setting_on() {
        start();
    }
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    let path = conf_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, format!("enabled={on}\n")).map_err(|e| e.to_string())?;
    if FORCED_OFF.load(Relaxed) {
        return Ok(());
    }
    if on {
        start();
    } else {
        stop();
    }
    Ok(())
}

pub fn info() -> Value {
    // The CLI and MCP server sits beside the GUI binary.
    let ctl = std::env::current_exe()
        .unwrap_or_default()
        .with_file_name("kuroobi-mcp");
    json!({
        "enabled": setting_on(),
        "active": ACTIVE.load(Relaxed),
        "forced_off": FORCED_OFF.load(Relaxed),
        "socket": ggs::control_socket_path(),
        "ctl": ctl,
    })
}

fn start() {
    let Some(app) = APP.get().cloned() else {
        return;
    };
    let path = ggs::control_socket_path();
    if ACTIVE.load(Relaxed) || UnixStream::connect(&path).is_ok() {
        return; // already ours, or another GUI launched from the same folder answers there
    }
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        return;
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    ACTIVE.store(true, Relaxed);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if !ACTIVE.load(Relaxed) {
                break;
            }
            let app = app.clone();
            std::thread::spawn(move || serve(&app, stream));
        }
    });
}

pub fn stop() {
    if ACTIVE.swap(false, Relaxed) {
        let path = ggs::control_socket_path();
        // Wakes the accept loop so it sees the switch and drops the listener.
        let _ = UnixStream::connect(&path);
        let _ = std::fs::remove_file(path);
    }
}

fn serve(app: &AppHandle, stream: UnixStream) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(req) => match handle(app, &req) {
            Ok(v) => json!({ "ok": true, "result": v }),
            Err(e) => json!({ "ok": false, "error": e }),
        },
        Err(e) => json!({ "ok": false, "error": format!("bad request: {e}") }),
    };
    let mut w = &stream;
    let _ = writeln!(w, "{reply}");
}

include!(concat!(env!("OUT_DIR"), "/control_commands.rs"));

type Answer = mpsc::Sender<Result<Value, String>>;

/// Requests handed to the screens, by id, waiting for `control_reply`.
static PENDING: Mutex<Vec<(u64, Answer)>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Runs `req` in the screens (the same `call` their own buttons use) and waits for the answer.
fn via_screens(app: &AppHandle, event: &str, mut req: Value) -> Result<Value, String> {
    let id = NEXT_ID.fetch_add(1, Relaxed);
    let (tx, rx) = mpsc::channel();
    PENDING.lock().unwrap().push((id, tx));
    req["id"] = json!(id);
    let sent = app.emit(event, req).map_err(|e| e.to_string());
    let r = sent.and_then(|_| {
        rx.recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| Err("the screens did not answer".into()))
    });
    PENDING.lock().unwrap().retain(|(i, _)| *i != id);
    r
}

pub fn reply(id: u64, ok: bool, value: Value) {
    let pending = PENDING.lock().unwrap();
    if let Some((_, tx)) = pending.iter().find(|(i, _)| *i == id) {
        let msg = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        let _ = tx.send(if ok { Ok(value) } else { Err(msg) });
    }
}

fn summary(app: &AppHandle) -> Result<Value, String> {
    let st = app.state::<App>();
    let engine_loaded = st.engine.lock().unwrap().is_some();
    let g = st.ggs.lock().unwrap();
    let s = g
        .as_ref()
        .ok_or("the GGS session has not started")?
        .snapshot
        .lock()
        .unwrap()
        .clone();
    let playing: Vec<Value> = s
        .matches
        .iter()
        .filter(|m| !m.over)
        .map(|m| {
            json!({
                "id": m.id, "opponent": m.opp_name, "opponent_rating": m.opp_rating,
                "my_color": m.my_color, "turn": m.turn,
                "my_clock": m.my_clock, "opponent_clock": m.opp_clock, "in_extension": m.in_extension,
                "moves": m.moves.len(), "discs": m.cells.iter().filter(|&&c| c != 0).count(),
                "my_eval": m.last_eval, "opponent_eval": m.opp_eval,
                "searching_eval": m.busy_eval, "searching_depth": m.busy_depth,
            })
        })
        .collect();
    Ok(json!({
        "pid": std::process::id(),
        "socket": ggs::control_socket_path(),
        "engine_loaded": engine_loaded,
        "ggs": { "conn": s.conn, "server": s.server, "login": s.login },
        "accept_requests": s.open,
        "auto_match": s.standby,
        "auto_match_stats": s.standby_stats,
        "playing": playing,
        "offers": s.offers.len(),
        "notice": s.notice,
    }))
}

/// The window's number for `screencapture -l`, which the caller runs: it holds the screen
/// recording permission, not the app.
#[cfg(target_os = "macos")]
fn window_number(app: &AppHandle) -> Result<Value, String> {
    let w = app
        .webview_windows()
        .into_values()
        .next()
        .ok_or("no window")?;
    let ns = w.ns_window().map_err(|e| e.to_string())? as *const objc2::runtime::AnyObject;
    // SAFETY: the window outlives this call; windowNumber only reads it.
    let num: isize = unsafe { objc2::msg_send![&*ns, windowNumber] };
    Ok(json!(num))
}

#[cfg(not(target_os = "macos"))]
fn window_number(_: &AppHandle) -> Result<Value, String> {
    Err("window capture is macOS only".into())
}

/// `status` and `quit` work on the app itself, `screen` on what only the GUI shows, and `call`
/// runs one of the GUI's commands through the screens, as their own controls do.
fn handle(app: &AppHandle, req: &Value) -> Result<Value, String> {
    match req["op"].as_str().ok_or("`op` is required")? {
        "status" => summary(app),
        "quit" => {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                app.exit(0);
            });
            Ok(json!("quitting"))
        }
        "call" => {
            let cmd = req["command"].as_str().ok_or("`command` is required")?;
            if !COMMANDS.iter().any(|(n, _)| *n == cmd) || cmd == "control_reply" {
                return Err(format!("unknown command `{cmd}`"));
            }
            via_screens(
                app,
                "control-call",
                json!({ "command": cmd, "args": req.get("args").cloned().unwrap_or(json!({})) }),
            )
        }
        "screen" => via_screens(app, "control-screen", req.clone()),
        "window" => window_number(app),
        other => Err(format!(
            "unknown op `{other}`; one of status, commands, call, screen, quit"
        )),
    }
}
