//! FlipSight behind the screens. It is never connected while GGS is: waiting for games on both
//! would make them fight over the CPU, so connecting one first disconnects the other.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use flipsight::manager::{self, Cmd, Snapshot};
use tauri::{Emitter, State};

use crate::App;

pub struct FsHost {
    pub app: tauri::AppHandle,
}

impl manager::Host for FsHost {
    fn publish(&self, snapshot: &Snapshot) {
        let _ = self.app.emit("fs", snapshot);
    }
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
}

fn fs_tx(app: &State<App>) -> Result<Sender<Cmd>, String> {
    app.fs
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| h.tx.clone())
        .ok_or_else(|| "err.fs_not_started".into())
}

fn send(app: &State<App>, cmd: Cmd) -> Result<(), String> {
    fs_tx(app)?.send(cmd).map_err(|e| e.to_string())
}

fn snapshot(app: &State<App>) -> Snapshot {
    app.fs
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| h.snapshot.lock().unwrap().clone())
        .unwrap_or_default()
}

fn wait_until(cond: impl Fn() -> bool) {
    let t = Instant::now();
    while !cond() && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Sends `cmd`, logging out of GGS first when it is on; refused while a GGS game is on.
fn after_ggs(app: &State<App>, cmd: Cmd) -> Result<(), String> {
    let snap = crate::ggs_snap_arc(app);
    let on = snap
        .as_ref()
        .is_some_and(|s| s.lock().unwrap().conn != "disconnected");
    if !on {
        return send(app, cmd);
    }
    if crate::ggs_match_in(&snap) {
        return Err("err.fs_ggs_playing".into());
    }
    let tx = fs_tx(app)?;
    ::ggs::client::clear_auto();
    crate::ggs_send(app, ::ggs::client::Cmd::Disconnect)?;
    std::thread::spawn(move || {
        if let Some(s) = &snap {
            wait_until(|| s.lock().unwrap().conn == "disconnected");
        }
        let _ = tx.send(cmd);
    });
    Ok(())
}

/// Runs `then` (a GGS login), disconnecting FlipSight first when it is on; refused while a
/// FlipSight match is on.
pub fn release_then(app: &State<App>, then: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let Some(snap) = app.fs.lock().unwrap().as_ref().map(|h| h.snapshot.clone()) else {
        then();
        return Ok(());
    };
    let (connected, playing) = {
        let s = snap.lock().unwrap();
        (s.connected, s.playing)
    };
    if !connected {
        then();
        return Ok(());
    }
    if playing {
        return Err("err.ggs_fs_playing".into());
    }
    send(app, Cmd::DisconnectAll)?;
    std::thread::spawn(move || {
        wait_until(|| !snap.lock().unwrap().connected);
        then();
    });
    Ok(())
}

#[tauri::command]
pub fn fs_snapshot(app: State<App>) -> Snapshot {
    snapshot(&app)
}

#[tauri::command]
pub fn fs_add(
    app: State<App>,
    token: String,
    cap: Option<usize>,
    threads: Option<usize>,
    level: Option<[u32; 2]>,
) -> Result<(), String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("err.fs_token_required".into());
    }
    send(
        &app,
        Cmd::Add {
            token,
            cap,
            threads,
            level,
        },
    )
}

#[tauri::command]
pub fn fs_connect(app: State<App>, id: i64) -> Result<(), String> {
    after_ggs(&app, Cmd::Connect(id))
}

#[tauri::command]
pub fn fs_disconnect(app: State<App>, id: i64) -> Result<(), String> {
    send(&app, Cmd::Disconnect(id))
}

#[tauri::command]
pub fn fs_remove(app: State<App>, id: i64) -> Result<(), String> {
    send(&app, Cmd::Remove(id))
}

/// `cap`: 0 is the site's own setting; `level`: `[0, 0]` plays by the clock; left out, each stays.
#[tauri::command]
pub fn fs_set(
    app: State<App>,
    id: i64,
    cap: Option<usize>,
    threads: Option<usize>,
    book: Option<bool>,
    accept: Option<bool>,
    level: Option<[u32; 2]>,
) -> Result<(), String> {
    send(
        &app,
        Cmd::Set {
            id,
            cap: cap.map(|c| (c > 0).then_some(c)),
            threads,
            book,
            accept,
            level: level.map(|l| (l[0] > 0).then_some(l)),
        },
    )
}

#[tauri::command]
pub fn fs_challenge(app: State<App>, id: i64, target: i64) -> Result<(), String> {
    send(&app, Cmd::Challenge { id, target })
}

#[tauri::command]
pub fn fs_change_token(app: State<App>, id: i64, token: String) -> Result<(), String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err("err.fs_token_required".into());
    }
    after_ggs(&app, Cmd::ChangeToken { id, token })
}

#[tauri::command]
pub fn fs_refresh(app: State<App>, id: i64) -> Result<(), String> {
    send(&app, Cmd::Refresh(id))
}

#[tauri::command]
pub fn fs_clear_pending(app: State<App>) -> Result<(), String> {
    send(&app, Cmd::ClearPending)
}

#[tauri::command]
pub fn fs_results(id: i64) -> Vec<serde_json::Value> {
    manager::results(id)
}
