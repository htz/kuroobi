//! Drives a running KUROOBI GUI through its control socket: from the shell, or as an MCP server
//! on stdio. Both share one tool list.
//!
//!   kuroobi-mcp [--instance main|test] <tool> [key=value ...]
//!   kuroobi-mcp --mcp

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Map, Value};

fn repo() -> PathBuf {
    let p = std::env::var_os("KUROOBI_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")));
    std::fs::canonicalize(&p).unwrap_or(p)
}

/// `main` is the real app launched from `crates/gui/`; `test` is a folder whose `ggs_games` holds only
/// local-server accounts.
fn launch_dir(instance: &str) -> PathBuf {
    match instance {
        "test" => std::env::var_os("KUROOBI_TEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo().join("tmp/guitest")),
        _ => repo().join("crates/gui"),
    }
}

fn socket(instance: &str) -> PathBuf {
    if let Some(p) = std::env::var_os("KUROOBI_CONTROL_SOCK") {
        return PathBuf::from(p);
    }
    match instance {
        "test" => launch_dir("test").join("ggs_games/.control.sock"),
        _ => repo().join("ggs_games/.control.sock"),
    }
}

/// One tool: its name, what it does, its arguments (JSON Schema properties) and which are required.
type Tool = (&'static str, &'static str, Value, Vec<&'static str>);

/// Tools that only pass their arguments on to one GUI command: (tool, command, arguments, help).
/// An argument is `name:type` with type s(tring), b(ool), i(nteger), o(bject) or a(rray); `?` makes
/// it optional.
const PASS: &[(&str, &str, &[&str], &str)] = &[
    ("ggs__logout", "ggs_disconnect", &[], "Log out, as the Logout button does (auto login at the next launch is cleared)."),
    ("ggs__accounts", "ggs_accounts", &[], "The saved accounts."),
    ("ggs__login_new", "ggs_connect", &["login:s", "pw:s", "server:s?"], "Log in with an account not saved yet (it is saved on success)."),
    ("ggs__switch_account", "ggs_switch_account", &["server:s", "login:s"], "Log out and back in as another saved account; auto login stays on."),
    ("ggs__forget_account", "ggs_forget_account", &["server:s", "login:s"], "Remove a saved account."),
    ("ggs__set_auto_account", "ggs_set_auto_account", &["server:s", "login:s"], "The account to log in as at launch."),
    ("ggs__servers", "ggs_servers", &[], "The named servers."),
    ("ggs__save_server", "ggs_save_server", &["old:s", "name:s", "addr:s", "public:b"], "Add or rename a server (old: its current address, empty for a new one)."),
    ("ggs__forget_server", "ggs_forget_server", &["addr:s"], "Remove a server."),
    ("ggs__state", "ggs_snapshot", &[], "Everything known about the GGS session: matches, offers, users, chat, rankings asked for (large)."),
    ("ggs__request", "ggs_ask", &["opponent:s", "gtype:s", "time:s", "rated:b"], "Ask a player for a match (gtype e.g. s8r14, time e.g. 00:01:00)."),
    ("ggs__unfit", "ggs_unfit", &["opponent:s", "gtype:s", "time:s", "rated:b"], "Why such a request would be declined by the other side's conditions, if it would."),
    ("ggs__accept", "ggs_accept", &["id:s"], "Accept a match request (the id from ggs__state offers)."),
    ("ggs__decline", "ggs_decline", &["id:s"], "Decline a match request."),
    ("ggs__accept_requests", "ggs_set_open", &["on:b"], "Whether others may send match requests."),
    ("ggs__open_requests", "ggs_set_open_requests", &["on:b"], "Whether requests addressed to anyone reach us (listed as open offers in ggs__state)."),
    ("ggs__set_formula", "ggs_set_formula", &["kind:s", "expr:s"], "The conditions for automatically accepting (kind aform) or declining (kind dform) requests."),
    ("ggs__matches", "ggs_list_matches", &[], "Ask for the matches in progress on the server (they appear in ggs__state)."),
    ("ggs__match", "ggs_match_cmd", &["id:s", "verb:s", "arg:s"], "A command on one of our matches, e.g. verb resign / adjourn / abort."),
    ("ggs__watch", "ggs_watch", &["id:s", "on:b"], "Start or stop watching a match (id e.g. .37)."),
    ("ggs__look", "ggs_look", &["id:s"], "Fetch a match's position once."),
    ("ggs__close", "ggs_close_match", &["id:s"], "Close a finished match."),
    ("ggs__stored", "ggs_list_stored", &[], "Ask for the adjourned matches (they appear in ggs__state)."),
    ("ggs__resume", "ggs_resume_stored", &["id:s"], "Resume an adjourned match."),
    ("ggs__chat", "ggs_chat", &["target:s", "text:s"], "Send a chat message to a player or channel."),
    ("ggs__who", "ggs_who", &[], "Ask who is online (appears in ggs__state)."),
    ("ggs__finger", "ggs_finger", &["name:s"], "Ask for a player's profile (appears in ggs__state)."),
    ("ggs__rank", "ggs_rank", &["gtype:s", "name:s"], "Ask for a player's rank in a game type (appears in ggs__state)."),
    ("ggs__top", "ggs_top", &["gtype:s", "n:i"], "Ask for the top players of a game type (appears in ggs__state)."),
    ("ggs__history", "ggs_history", &["name:s"], "Ask for a player's recent games (appears in ggs__state)."),
    ("ggs__send", "ggs_raw", &["cmd:s"], "Send one raw line to the GGS server."),
    ("ggs__set_auto_play", "ggs_set_auto_play", &["on:b"], "Whether the engine plays our GGS games."),
    ("ggs__set_use_book", "ggs_set_use_book", &["on:b"], "Whether GGS games use the opening book."),
    ("ggs__set_watch_analysis", "ggs_set_watch_analysis", &["on:b"], "Whether watched games are analysed."),
    ("ggs__set_ponder", "ggs_set_ponder", &["on:b"], "Whether the threads search the opponent's positions while it is to move on every board of ours."),
    ("ggs__set_log_channel", "ggs_set_log_channel", &["channel:s"], "The channel the search log is told to (empty: none)."),
    ("ggs__set_learn", "ggs_set_learn", &["on:b"], "Whether finished GGS games are learned into the book."),
    ("ggs__set_book_tolerance", "ggs_set_book_tolerance", &["discs:n"], "How far below the best book move, in discs, a GGS game's random book pick may go (0: always the best). Moves learning demoted are never picked."),
    ("ggs__set_list", "ggs_set_list", &["list:s", "name:s", "on:b"], "Put a player on or off a list: notify (log-ins and games, shown in ggs__state notices), track (watch their games automatically) or ignore (their chat and requests stop)."),
    ("game__state", "state", &[], "The local board: position, moves, whose turn, result."),
    ("game__set_learn", "set_learn", &["on:b"], "Whether finished games on the local board are learned into the book."),
    ("game__set_book_tolerance", "set_book_tolerance", &["discs:n"], "How far below the best book move, in discs, the local board's random book pick may go (0: always the best). Moves learning demoted are never picked."),
    ("game__new", "new_game", &[], "Start a new local game."),
    ("game__undo", "undo", &[], "Take back a move."),
    ("game__goto", "goto", &["n:i"], "Go to the position after n moves."),
    ("game__load_board", "load_board", &["board:s"], "Set up a position: 64 squares of X / O / - then the side to move."),
    ("game__think", "think", &[], "Let the engine choose a move for the side to move (it is not played)."),
    ("game__stop", "stop_search", &[], "Stop the engine's search."),
    ("game__eval", "eval_at", &["n:i", "depth:i"], "Evaluate the position after n moves at a depth."),
    ("game__set_levels", "set_levels", &["depth:i", "solve_empties:i"], "The engine's strength in local games."),
    ("game__clocks", "clocks", &[], "The local game's clocks."),
    ("game__set_clock", "set_clock", &["secs:i"], "Each side's time for local games."),
    ("game__run_clock", "run_clock", &["on:b"], "Start or stop the local clocks."),
    ("kifu__load", "load_kifu_text", &["text:s"], "Load a game record (GGF, KIFU moves, ...) into the local board."),
    ("kifu__preview", "preview_kifu", &["text:s"], "Read a game record without loading it."),
    ("kifu__export", "export_kifu", &["format:s", "black:s", "white:s"], "The local game as text (format moves, ggf, board_moves or board)."),
    ("engine__resources", "resource_status", &[], "The weights and book files in use, and whether they load."),
    ("engine__set_resource", "set_resource", &["kind:s", "path:s?"], "Point the engine at a file (kind weights / nnue / book / dir; no path: back to the default)."),
    ("engine__threads", "local_threads", &[], "The search threads setting."),
    ("engine__set_threads", "set_local_threads", &["n:i?"], "Search threads (none: automatic)."),
    ("engine__hash", "hash_sizes", &[], "The transposition table sizes."),
    ("engine__set_hash", "set_hash_sizes", &["mid:i", "end:i"], "Transposition table sizes in bits (from the next launch)."),
    ("engine__activity", "activity_status", &[], "What the engine is busy with."),
    ("fs__state", "fs_snapshot", &[], "FlipSight: my bots with their settings and connection, each one's matches in progress, and the bots on the site (large). Bots are added and tokens changed on the screen only."),
    ("fs__connect", "fs_connect", &["id:i"], "Connect a bot (logs out of GGS first; refused during a GGS game)."),
    ("fs__disconnect", "fs_disconnect", &["id:i"], "Disconnect a bot: at once with no match, else after its matches."),
    ("fs__set", "fs_set", &["id:i", "cap:i?", "threads:i?", "book:b?", "accept:b?", "level:a?", "hash_mid:i?", "hash_end:i?", "ponder:b?", "learn:b?", "book_tolerance:n?"], "A bot's settings, each left out stays: cap matches at once (0: the site's), threads per game (1-4), book, accept direct challenges, level [depth, solve] ([0, 0]: by the clock), midgame and endgame transposition table bits (0: the app's setting; rebuilt once no match runs), ponder (search the opponent's position while it thinks; on the clock only), learn (each finished game goes into the book), book_tolerance (discs below the best book move a random pick may go; 0: always the best). Apply from the next game."),
    ("fs__refresh", "fs_refresh", &["id:i"], "Read a disconnected bot's settings on the site again (a connected one has them from its stream)."),
    ("fs__sync", "fs_sync", &["id:i"], "Read the site's records of a disconnected bot's games since the last one kept, at most once a minute (a connected one reads them itself); new results appear in fs__results."),
    ("fs__challenge", "fs_challenge", &["id:i", "target:i"], "Challenge another bot (target: its id from fs__state)."),
    ("fs__results", "fs_results", &["id:i"], "A bot's finished matches: result, rating, and each game's moves, evals and final board."),
    ("gui__ai_control", "control_info", &[], "This control socket's state and the registration for MCP clients."),
    ("gui__set_ai_control", "set_control", &["on:b"], "Turn the control socket on or off (off cuts this very connection)."),
];

fn schema_of(spec: &'static [&'static str]) -> (Value, Vec<&'static str>) {
    let mut props = Map::new();
    let mut req = Vec::new();
    for a in spec.iter().copied() {
        let (name, ty) = a.split_once(':').unwrap_or((a, "s"));
        let t = match ty.trim_end_matches('?') {
            "b" => "boolean",
            "i" => "integer",
            "n" => "number",
            "o" => "object",
            "a" => "array",
            _ => "string",
        };
        props.insert(name.to_string(), json!({"type": t}));
        if !ty.ends_with('?') {
            req.push(name);
        }
    }
    (Value::Object(props), req)
}

/// Names are `namespace__operation`: `ggs__` GGS, `fs__` FlipSight bots, `game__` the local
/// board, `kifu__` game records, `engine__` its files and resources, and `gui__` what only the GUI
/// has (its menus and window); the app itself has no namespace.
fn tools() -> Vec<Tool> {
    let s = json!({"type": "string"});
    let b = json!({"type": "boolean"});
    let none = || json!({});
    let mut t: Vec<Tool> = vec![
        ("launch", "Launch the GUI from its proper folder (weights and GGS data are found from there). Builds nothing.", none(), vec![]),
        ("quit", "Quit the GUI.", none(), vec![]),
        ("status", "Engine loaded; GGS connection; auto match and request acceptance; games in progress with clocks and evaluation.", none(), vec![]),
        ("ggs__login", "Log in with a saved account (default: the auto-login account, else the one used last).", json!({"server": {"type": "string", "description": "host:port"}, "login": s}), vec![]),
        ("ggs__auto_match", "Start or stop auto match. Unset fields keep the current settings.", json!({
            "on": b, "opponent": s, "gtype": s, "time": s,
            "max_games": {"type": "integer", "description": "0: until stopped"},
            "interval_secs": {"type": "integer"}, "rated": b,
            "auto_accept": {"type": "boolean", "description": "also accept requests that arrive meanwhile"}}), vec!["on"]),
        ("ggs__tourneys", "The tournaments the director (/td) lists: id, state, game type, clock, rounds played, players, whether we joined.", none(), vec![]),
        ("ggs__tourney", "One tournament now: state, round, next start, players, rankings and schedule text, and our matches in it with their disc differences.", json!({"id": {"type": "integer"}}), vec!["id"]),
        ("ggs__tourney_join", "Join a tournament (before it starts).", json!({"id": {"type": "integer"}}), vec!["id"]),
        ("ggs__tourney_withdraw", "Withdraw from a tournament (before it starts).", json!({"id": {"type": "integer"}}), vec!["id"]),
        ("ggs__tourney_create", "Create a tournament on the director (/td); whoever creates it directs it, and everyone is told at once. style r (round robin) or s (Swiss); gtype as in a request (s8r14, 8, ...); clock as GGS writes it (05:00//02:00); rounds 1 or more; start_minutes 1-30.",
         json!({"style": {"type": "string", "enum": ["r", "s"]}, "gtype": {"type": "string"}, "clock": {"type": "string"}, "rounds": {"type": "integer"}, "start_minutes": {"type": "integer"}}),
         vec!["style", "gtype", "clock", "rounds", "start_minutes"]),
        ("ggs__results", "Finished games, newest first.", json!({"limit": {"type": "integer", "description": "default 20"}}), vec![]),
        ("game__play", "Play a move on the local board.", json!({"move": {"type": "string", "description": "e.g. f5"}}), vec!["move"]),
        ("game__apply", "Play the engine's move from game__think, or pass when no move is given.", json!({"move": s}), vec![]),
        ("gui__show", "Select a menu in the GUI, and for settings a category.", json!({
            "menu": {"type": "string", "description": "play, study, book, settings, ggs-play, ggs-lobby, ggs-players, ggs-results, ggs-chat, ggs-tourney, ggs-console, ggs-login"},
            "category": {"type": "string", "description": "settings only: engine, view, ai, ggs-me, ggs-account, ggs-play, ggs-form"}}), vec!["menu"]),
        ("gui__current", "The menu (and settings category) selected in the GUI now.", none(), vec![]),
        ("gui__text", "The text the GUI window shows now.", none(), vec![]),
        ("gui__screenshot", "A picture of the GUI window only (other windows never show in it).", none(), vec![]),
    ];
    for (name, _, spec, help) in PASS {
        let (props, req) = schema_of(spec);
        t.push((name, help, props, req));
    }
    t
}

fn call_socket(instance: &str, req: Value) -> Result<Value, String> {
    let path = socket(instance);
    let mut s = UnixStream::connect(&path).map_err(|_| {
        format!(
            "the {instance} GUI is not running (no control socket at {})",
            path.display()
        )
    })?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    writeln!(s, "{req}").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(&s)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let reply: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if reply["ok"].as_bool() == Some(true) {
        Ok(reply["result"].clone())
    } else {
        Err(reply["error"].as_str().unwrap_or("failed").to_string())
    }
}

fn launch(instance: &str) -> Result<Value, String> {
    if let Ok(st) = call_socket(instance, json!({"op": "status"})) {
        return Ok(json!({"already_running": st["pid"]}));
    }
    let bin = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("kuroobi-gui");
    if !bin.exists() {
        return Err(format!("{} is not built", bin.display()));
    }
    let cwd = launch_dir(instance);
    let data = if instance == "test" {
        cwd.join("ggs_games")
    } else {
        cwd.join("../../ggs_games")
    };
    if !data.is_dir() {
        return Err(format!(
            "{} has no ggs_games to launch against",
            cwd.display()
        ));
    }
    let mut child = Command::new(&bin)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = child.id();
    // Unwaited, a GUI that quits stays a zombie of the long-lived MCP server and keeps its GGS session lock.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(json!({"launched_pid": pid, "cwd": cwd}))
}

/// A command of the GUI, run through its screens exactly as their own controls run it.
fn command(instance: &str, name: &str, args: Value) -> Result<Value, String> {
    call_socket(
        instance,
        json!({"op": "call", "command": name, "args": args}),
    )
}

/// A command to the tournament director, as the tournament screen sends it.
fn td(instance: &str, cmd: &str) -> Result<Value, String> {
    command(
        instance,
        "ggs_raw",
        json!({"cmd": format!("tell /td {cmd}")}),
    )
}

fn arg<'a>(args: &'a Map<String, Value>, k: &str) -> Result<&'a Value, String> {
    args.get(k).ok_or_else(|| format!("`{k}` is required"))
}

fn screen(instance: &str, action: &str, mut args: Map<String, Value>) -> Result<Value, String> {
    args.insert("op".into(), json!("screen"));
    args.insert("action".into(), json!(action));
    call_socket(instance, Value::Object(args))
}

fn square(mv: &Value) -> Result<Value, String> {
    let s = mv.as_str().ok_or("`move` is a square such as f5")?;
    let p = kuroobi::Position::from_kifu(&s.to_ascii_lowercase())?;
    Ok(json!(p.index()))
}

fn screenshot(instance: &str) -> Result<Value, String> {
    let num = call_socket(instance, json!({"op": "window"}))?;
    let path = socket(instance).with_file_name("control-shot.png");
    let st = Command::new("screencapture")
        .args(["-x", "-o", &format!("-l{num}")])
        .arg(&path)
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() || !path.exists() {
        return Err(
            "screencapture failed (the caller needs the screen recording permission)".into(),
        );
    }
    let png = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(json!({"path": path, "png_base64": base64(&png)}))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() {
                T[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

fn run_tool(name: &str, mut args: Map<String, Value>) -> Result<Value, String> {
    let instance = match args.remove("instance") {
        None => "main".to_string(),
        Some(Value::String(s)) if s == "main" || s == "test" => s,
        Some(_) => return Err("instance is main or test".into()),
    };
    let i = instance.as_str();
    if let Some((_, cmd, spec, _)) = PASS.iter().find(|p| p.0 == name) {
        let mut a = Map::new();
        for p in *spec {
            let (k, ty) = p.split_once(':').unwrap_or((p, "s"));
            match args.get(k) {
                Some(v) => {
                    a.insert(k.to_string(), v.clone());
                }
                None if ty.ends_with('?') => {
                    a.insert(k.to_string(), Value::Null);
                }
                None => return Err(format!("`{k}` is required")),
            }
        }
        return command(i, cmd, Value::Object(a));
    }
    match name {
        "launch" => launch(i),
        "quit" => call_socket(i, json!({"op": "quit"})),
        "status" => call_socket(i, json!({"op": "status"})),
        "ggs__login" => {
            let (server, login) = match (args.get("server"), args.get("login")) {
                (Some(s), Some(l)) => (s.clone(), l.clone()),
                _ => {
                    let auto = command(i, "ggs_auto_account", json!({}))?;
                    if auto.is_null() {
                        // After a logout: the most recently used account, as the login screen lists first.
                        let last = command(i, "ggs_accounts", json!({}))?[0].clone();
                        (last["server"].clone(), last["login"].clone())
                    } else {
                        (auto[0].clone(), auto[1].clone())
                    }
                }
            };
            if server.is_null() {
                return Err("no saved account to log in as".into());
            }
            command(
                i,
                "ggs_connect_saved",
                json!({"server": server, "login": login}),
            )
        }
        "ggs__auto_match" => {
            let mut cfg = call_socket(i, json!({"op": "status"}))?["auto_match"].clone();
            cfg["enabled"] = arg(&args, "on")?.clone();
            for k in [
                "opponent",
                "gtype",
                "time",
                "max_games",
                "interval_secs",
                "rated",
                "auto_accept",
            ] {
                if let Some(v) = args.get(k) {
                    cfg[k] = v.clone();
                }
            }
            command(i, "ggs_set_standby", json!({"cfg": cfg}))
        }
        "ggs__results" => {
            let n = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            let all = command(i, "ggs_results", json!({}))?;
            Ok(Value::Array(
                all.as_array()
                    .map(|a| a.iter().take(n).cloned().collect())
                    .unwrap_or_default(),
            ))
        }
        "ggs__tourneys" => {
            td(i, "t")?;
            std::thread::sleep(std::time::Duration::from_secs(2));
            Ok(command(i, "ggs_snapshot", json!({}))?["td"]["tourneys"].clone())
        }
        "ggs__tourney" => {
            let id = arg(&args, "id")?.as_u64().ok_or("`id` is a number")?;
            for c in ["t", "f", "r", "st"] {
                td(
                    i,
                    &if c == "t" {
                        c.to_string()
                    } else {
                        format!("{c} {id}")
                    },
                )?;
            }
            // The director answers within a second or two; a GUI just launched may need longer.
            let (mut snap, mut found) = (Value::Null, None);
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                snap = command(i, "ggs_snapshot", json!({}))?;
                found = snap["td"]["tourneys"]
                    .as_array()
                    .and_then(|a| a.iter().find(|t| t["id"] == id))
                    .cloned();
                if found.is_some() {
                    break;
                }
            }
            let t = found.ok_or(format!("no tournament {id}"))?;
            let games: Vec<Value> = snap["td"]["games"]
                .as_array()
                .map(|a| a.iter().filter(|g| g["tid"] == id).cloned().collect())
                .unwrap_or_default();
            Ok(json!({"tourney": t, "our_games": games}))
        }
        "ggs__tourney_join" => td(i, &format!("join {}", arg(&args, "id")?)),
        "ggs__tourney_withdraw" => td(i, &format!("withdraw {}", arg(&args, "id")?)),
        "ggs__tourney_create" => {
            let word = |k: &str| -> Result<String, String> {
                let v = arg(&args, k)?;
                let w = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                if w.is_empty() || w.contains(char::is_whitespace) {
                    return Err(format!("`{k}` must be one word"));
                }
                Ok(w)
            };
            td(
                i,
                &format!(
                    "new {} /os {} {} {} {}",
                    word("style")?,
                    word("gtype")?,
                    word("clock")?,
                    word("rounds")?,
                    word("start_minutes")?
                ),
            )
        }
        "game__play" => command(i, "play", json!({"sq": square(arg(&args, "move")?)?})),
        "game__apply" => {
            let sq = match args.get("move") {
                Some(m) => square(m)?,
                None => {
                    // The command passes whatever the position; the GUI only calls it so when no move exists.
                    let legal = command(i, "state", json!({}))?["legal"].clone();
                    if legal.as_array().is_some_and(|l| !l.is_empty()) {
                        return Err(
                            "there are moves to play; a pass is only possible when there are none"
                                .into(),
                        );
                    }
                    Value::Null
                }
            };
            command(i, "apply_move", json!({"sq": sq}))
        }
        "gui__show" => screen(i, "navigate", args),
        "gui__current" => screen(i, "current", args),
        "gui__text" => screen(i, "text", args),
        "gui__screenshot" => screenshot(i),
        _ => {
            let names: Vec<_> = tools().iter().map(|t| t.0).collect();
            Err(format!(
                "unknown tool `{name}`; one of {}",
                names.join(", ")
            ))
        }
    }
}

fn mcp() -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            continue; // notifications need no reply
        };
        let method = msg["method"].as_str().unwrap_or("");
        let reply = match method {
            "initialize" => json!({"result": {
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "kuroobi-gui", "version": env!("CARGO_PKG_VERSION")}}}),
            "tools/list" => {
                let list: Vec<Value> = tools()
                    .into_iter()
                    .map(|(name, desc, props, req)| {
                        let mut p = props.as_object().cloned().unwrap_or_default();
                        p.insert("instance".into(), json!({"type": "string", "enum": ["main", "test"], "default": "main",
                            "description": "main: the real app (real GGS account). test: the test folder, local-server accounts only."}));
                        json!({"name": name, "description": desc,
                            "inputSchema": {"type": "object", "properties": p, "required": req}})
                    })
                    .collect();
                json!({"result": {"tools": list}})
            }
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("");
                let args = msg["params"]["arguments"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                let r = run_tool(name, args);
                let content = match &r {
                    Ok(v) if v.get("png_base64").is_some() => json!([
                        {"type": "image", "data": v["png_base64"], "mimeType": "image/png"},
                        {"type": "text", "text": v["path"].to_string()}]),
                    Ok(v) => {
                        json!([{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}])
                    }
                    Err(e) => json!([{"type": "text", "text": e}]),
                };
                json!({"result": {"content": content, "isError": r.is_err()}})
            }
            "ping" => json!({"result": {}}),
            _ => json!({"error": {"code": -32601, "message": format!("unknown method {method}")}}),
        };
        let mut reply = reply.as_object().cloned().unwrap_or_default();
        reply.insert("jsonrpc".into(), json!("2.0"));
        reply.insert("id".into(), id);
        if writeln!(out, "{}", Value::Object(reply)).is_err() || out.flush().is_err() {
            break;
        }
    }
    0
}

fn usage() -> i32 {
    eprintln!("usage: kuroobi-mcp [--instance main|test] <tool> [key=value ...]\n       kuroobi-mcp --mcp\ntools:");
    for (name, desc, ..) in tools() {
        eprintln!("  {name:<20} {desc}");
    }
    2
}

fn cli(argv: &[String]) -> i32 {
    let mut argv = argv.to_vec();
    let mut args = Map::new();
    if argv.len() >= 2 && argv[0] == "--instance" {
        args.insert("instance".into(), json!(argv[1]));
        argv.drain(..2);
    }
    let Some(tool) = argv.first().cloned() else {
        return usage();
    };
    for kv in &argv[1..] {
        let Some((k, v)) = kv.split_once('=') else {
            return usage();
        };
        // Arguments typed string stay as written (a game type "8" is not a number); others are read as JSON.
        let string_typed = tools()
            .iter()
            .find(|t| t.0 == tool)
            .is_some_and(|t| t.2[k]["type"] == "string");
        let v = if string_typed {
            json!(v)
        } else {
            serde_json::from_str(v).unwrap_or_else(|_| json!(v))
        };
        args.insert(k.to_string(), v);
    }
    match run_tool(&tool, args) {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.remove("png_base64");
            }
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(if args.first().map(String::as_str) == Some("--mcp") {
        mcp()
    } else {
        cli(&args)
    });
}
