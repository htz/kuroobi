//! The client's files: saved accounts and servers, settings, chat, the wire log and the session lock.

use super::*;

const DEFAULT_SERVER: &str = "skatgame.net:5000";
pub(super) static SERVER: Mutex<String> = Mutex::new(String::new());

pub(super) fn server() -> String {
    let s = SERVER.lock().unwrap();
    if s.is_empty() {
        DEFAULT_SERVER.to_string()
    } else {
        s.clone()
    }
}

/// Test games must not be learned into the shared book: only a server marked public counts.
pub(super) fn local_server() -> bool {
    let addr = server();
    !saved_servers().iter().any(|s| s.addr == addr && s.public)
}

/// `ggs_games/`, holding the account store and one folder per account.
fn base_dir() -> PathBuf {
    for c in ["ggs_games", "../ggs_games", "../../ggs_games"] {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from(".")
}

fn safe_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The account whose history, chat and wire log are read and written.
pub(super) static USER: Mutex<Option<(String, String)>> = Mutex::new(None);

fn user_dir(server: &str, login: &str) -> PathBuf {
    base_dir()
        .join("users")
        .join(safe_name(server))
        .join(safe_name(login))
}

pub(super) fn set_user(server: &str, login: &str) {
    let dir = user_dir(server, login);
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
        adopt_legacy_data(server, login, &dir);
    }
    *USER.lock().unwrap() = Some((server.to_string(), login.to_string()));
}

/// Data written before per-account folders belongs to the public account whose chat file it kept.
fn adopt_legacy_data(server: &str, login: &str, dir: &std::path::Path) {
    if server != DEFAULT_SERVER {
        return;
    }
    let base = base_dir();
    let chat = base
        .join("chat")
        .join(format!("{}.jsonl", safe_name(login)));
    if !chat.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(dir.join("chat"));
    for f in [chat.clone(), chat.with_extension("seen")] {
        let _ = std::fs::rename(&f, dir.join("chat").join(f.file_name().unwrap()));
    }
    for f in ["history.jsonl", "ggs_wire.log", "ggs_wire.log.1"] {
        let _ = std::fs::rename(base.join(f), dir.join(f));
    }
}

fn data_dir() -> PathBuf {
    match &*USER.lock().unwrap() {
        Some((srv, login)) => user_dir(srv, login),
        None => base_dir(),
    }
}

pub(super) fn history_path() -> PathBuf {
    data_dir().join("history.jsonl")
}

#[derive(Clone, Serialize, serde::Deserialize, PartialEq, Default)]
pub struct SavedAccount {
    pub server: String,
    pub login: String,
    /// Unix seconds of the last successful login.
    #[serde(default)]
    pub last: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
}

/// A server the user has named; `public` marks the real one, whose games teach the book.
#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct SavedServer {
    pub addr: String,
    pub name: String,
    #[serde(default)]
    pub public: bool,
}

#[derive(Serialize, serde::Deserialize, Default)]
struct AccountStore {
    #[serde(default)]
    servers: Vec<SavedServer>,
    accounts: Vec<SavedAccount>,
    /// `server|login` to log in as at startup; cleared by logging out.
    #[serde(default)]
    auto: Option<String>,
}

// The `.ggs_credentials` prefix keeps the file under the repo's credential ignore rule.
fn store_path() -> PathBuf {
    base_dir().join(".ggs_credentials.accounts.json")
}

fn key_of(server: &str, login: &str) -> String {
    format!("{server}|{login}")
}

fn load_store() -> AccountStore {
    if let Some(s) = std::fs::read_to_string(store_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
    {
        return s;
    }
    let s = import_old_credentials();
    write_store(&s);
    s
}

fn write_store(s: &AccountStore) {
    let Ok(text) = serde_json::to_string_pretty(s) else {
        return;
    };
    let p = store_path();
    if std::fs::write(&p, text).is_ok() {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
}

/// One-time pickup of the login the keychain or `.ggs_credentials` held before the store existed.
fn import_old_credentials() -> AccountStore {
    let run = |args: &[&str]| {
        std::process::Command::new("/usr/bin/security")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };
    let keychain = || -> Option<(String, String)> {
        let meta = run(&["find-generic-password", "-s", "kuroobi-ggs"])?;
        let login = meta.lines().find_map(|l| {
            l.trim()
                .strip_prefix("\"acct\"<blob>=\"")?
                .strip_suffix('"')
                .map(str::to_string)
        })?;
        let pw = run(&["find-generic-password", "-s", "kuroobi-ggs", "-w"])?;
        let pw = pw.trim_end_matches('\n').to_string();
        (!pw.is_empty() && login != "-").then_some((login, pw))
    };
    let mut s = AccountStore::default();
    if let Some((login, pw)) = keychain().or_else(crate::read_credentials) {
        s.auto = Some(key_of(DEFAULT_SERVER, &login));
        s.accounts.push(SavedAccount {
            server: DEFAULT_SERVER.to_string(),
            login,
            last: now_secs(),
            password: pw,
        });
    }
    s
}

/// Most recently used first, without passwords.
pub fn saved_accounts() -> Vec<SavedAccount> {
    let mut list: Vec<SavedAccount> = load_store()
        .accounts
        .into_iter()
        .map(|a| SavedAccount {
            password: String::new(),
            ..a
        })
        .collect();
    list.sort_by_key(|a| std::cmp::Reverse(a.last));
    list
}

pub fn account_password(server: &str, login: &str) -> Option<String> {
    load_store()
        .accounts
        .into_iter()
        .find(|a| a.server == server && a.login == login && !a.password.is_empty())
        .map(|a| a.password)
}

/// The account to log in as at startup: `(server, login, password)`.
pub fn auto_account() -> Option<(String, String, String)> {
    let s = load_store();
    let key = s.auto?;
    s.accounts
        .into_iter()
        .find(|a| key_of(&a.server, &a.login) == key && !a.password.is_empty())
        .map(|a| (a.server, a.login, a.password))
}

pub fn clear_auto() {
    let mut s = load_store();
    s.auto = None;
    write_store(&s);
}

pub(super) fn remember_account(server: &str, login: &str, pw: &str) {
    let mut s = load_store();
    s.accounts
        .retain(|x| !(x.server == server && x.login == login));
    s.accounts.push(SavedAccount {
        server: server.to_string(),
        login: login.to_string(),
        last: now_secs(),
        password: pw.to_string(),
    });
    s.auto = Some(key_of(server, login));
    write_store(&s);
}

/// The named servers: the public one first, then every server an account was saved on.
pub fn saved_servers() -> Vec<SavedServer> {
    let s = load_store();
    let mut out = s.servers.clone();
    if !out.iter().any(|x| x.addr == DEFAULT_SERVER) {
        out.insert(
            0,
            SavedServer {
                addr: DEFAULT_SERVER.to_string(),
                name: "GGS".to_string(),
                public: true,
            },
        );
    }
    for a in &s.accounts {
        if !out.iter().any(|x| x.addr == a.server) {
            out.push(SavedServer {
                addr: a.server.clone(),
                name: a.server.clone(),
                public: false,
            });
        }
    }
    out
}

/// Adds or edits a server; when its address changes, its accounts move with it.
pub fn save_server(old: &str, name: &str, addr: &str, public: bool) -> Result<(), String> {
    let addr = addr.trim();
    if addr.is_empty() || !addr.contains(':') {
        return Err("err.server_address".to_string());
    }
    let mut s = load_store();
    let mut list = saved_servers();
    if addr != old && list.iter().any(|x| x.addr == addr) {
        return Err("err.server_exists".to_string());
    }
    let entry = SavedServer {
        addr: addr.to_string(),
        name: if name.trim().is_empty() {
            addr.to_string()
        } else {
            name.trim().to_string()
        },
        public,
    };
    match list.iter_mut().find(|x| !old.is_empty() && x.addr == old) {
        Some(x) => *x = entry,
        None => list.push(entry),
    }
    if !old.is_empty() && old != addr {
        for a in s.accounts.iter_mut().filter(|a| a.server == old) {
            a.server = addr.to_string();
        }
        if let Some(k) = s.auto.as_mut() {
            if let Some(login) = k.strip_prefix(&format!("{old}|")) {
                *k = key_of(addr, login);
            }
        }
    }
    s.servers = list;
    write_store(&s);
    Ok(())
}

/// Removes a server and the accounts saved on it.
pub fn forget_server(addr: &str) {
    let mut s = load_store();
    s.servers = saved_servers()
        .into_iter()
        .filter(|x| x.addr != addr)
        .collect();
    s.accounts.retain(|a| a.server != addr);
    if s.auto
        .as_deref()
        .is_some_and(|k| k.starts_with(&format!("{addr}|")))
    {
        s.auto = None;
    }
    write_store(&s);
}

/// The account to log in as at startup, as `(server, login)`.
pub fn auto_key() -> Option<(String, String)> {
    let k = load_store().auto?;
    let (server, login) = k.split_once('|')?;
    Some((server.to_string(), login.to_string()))
}

pub fn set_auto(server: &str, login: &str) {
    let mut s = load_store();
    s.auto = (!login.is_empty()).then(|| key_of(server, login));
    write_store(&s);
}

pub fn forget_account(server: &str, login: &str) {
    let mut s = load_store();
    s.accounts
        .retain(|x| !(x.server == server && x.login == login));
    if s.auto.as_deref() == Some(&key_of(server, login)) {
        s.auto = None;
    }
    write_store(&s);
}

pub(super) fn chat_path(login: &str) -> PathBuf {
    let dir = data_dir().join("chat");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{}.jsonl", safe_name(login).replace('.', "_")))
}

const CHAT_KEEP: usize = 5000;
// Per-thread trimming can leave the file above the fixed floor, so the next trim waits for growth past it.
static CHAT_TRIMMED_LEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn load_chat(login: &str) -> Vec<ChatMsg> {
    let Ok(text) = std::fs::read_to_string(chat_path(login)) else {
        return Vec::new();
    };
    let out: Vec<ChatMsg> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<ChatMsg>(l).ok())
        .collect();
    keep_latest_per_thread(out, CHAT_KEEP)
}

fn settings_path() -> PathBuf {
    base_dir().join("ggs_settings.json")
}

/// A missing field falls back to its default instead of failing the read and wiping settings.
#[derive(Serialize, serde::Deserialize)]
#[serde(default)]
struct SavedSettings {
    auto_play: bool,
    open: bool,
    watch_analysis: bool,
    log_channel: String,
    use_book: bool,
    learn: bool,
}

impl Default for SavedSettings {
    fn default() -> Self {
        SavedSettings {
            auto_play: true,
            open: true,
            watch_analysis: true,
            log_channel: String::new(),
            use_book: true,
            learn: true,
        }
    }
}

pub(super) fn save_settings(ctx: &Ctx) {
    let s = ctx.snap.lock().unwrap();
    let e = &s.engine;
    let v = SavedSettings {
        auto_play: s.auto_play,
        open: s.open,
        watch_analysis: s.watch_analysis,
        log_channel: s.log_channel.clone(),
        use_book: e.use_book,
        learn: e.learn,
    };
    drop(s);
    if let Ok(text) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(settings_path(), text);
    }
}

pub(super) fn wire_path() -> PathBuf {
    data_dir().join("ggs_wire.log")
}

pub(super) fn roll_if_large(p: &std::path::Path) {
    const CAP: u64 = 8 * 1024 * 1024;
    if std::fs::metadata(p).is_ok_and(|m| m.len() > CAP) {
        let _ = std::fs::rename(p, p.with_extension("log.1"));
    }
}

fn load_settings() -> Option<SavedSettings> {
    let text = std::fs::read_to_string(settings_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub(super) fn restore_settings(ctx: &mut Ctx) {
    let Some(v) = load_settings() else { return };
    ctx.set_use_book(v.use_book);
    let mut s = ctx.snap.lock().unwrap();
    s.engine.learn = v.learn;
    s.auto_play = v.auto_play;
    s.open = v.open;
    s.watch_analysis = v.watch_analysis;
    if valid_log_channel(&v.log_channel) {
        s.log_channel = v.log_channel;
    }
}

fn chat_seen_path(login: &str) -> PathBuf {
    chat_path(login).with_extension("seen")
}

fn read_chat_seen(path: &std::path::Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub(super) fn load_chat_seen(login: &str) -> u64 {
    read_chat_seen(&chat_seen_path(login)).unwrap_or_else(now_secs)
}

pub(super) fn save_chat_seen(login: &str, at: u64) {
    if login.is_empty() {
        return;
    }
    let path = chat_seen_path(login);
    if at > read_chat_seen(&path).unwrap_or(0) {
        let _ = std::fs::write(&path, at.to_string());
    }
}

pub(super) fn append_chat(login: &str, m: &ChatMsg) {
    if login.is_empty() {
        return;
    }
    let path = chat_path(login);
    let Ok(line) = serde_json::to_string(m) else {
        return;
    };
    append_line(&path, &line);
    if let Ok(meta) = std::fs::metadata(&path) {
        let floor = (CHAT_KEEP as u64) * 120 * 12 / 10;
        let last = CHAT_TRIMMED_LEN.load(std::sync::atomic::Ordering::Relaxed);
        if meta.len() > floor.max(last * 12 / 10) {
            let kept = load_chat(login);
            let body: String = kept
                .iter()
                .filter_map(|m| serde_json::to_string(m).ok())
                .map(|l| l + "\n")
                .collect();
            let tmp = path.with_extension("jsonl.tmp");
            if std::fs::write(&tmp, &body).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
                CHAT_TRIMMED_LEN.store(body.len() as u64, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

fn session_lock_path() -> PathBuf {
    if let Ok(p) = std::env::var("KUROOBI_SESSION_LOCK") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::temp_dir().join("kuroobi_ggs.pid")
}

fn other_session_pid(path: &std::path::Path) -> Option<i32> {
    let pid = std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()?;
    (pid != std::process::id() as i32 && unsafe { libc::kill(pid, 0) } == 0).then_some(pid)
}

pub fn session_locked_by_other() -> bool {
    other_session_pid(&session_lock_path()).is_some()
}

pub(super) fn try_lock_session() -> Result<(), i32> {
    let path = session_lock_path();
    if let Some(pid) = other_session_pid(&path) {
        return Err(pid);
    }
    let _ = std::fs::write(&path, std::process::id().to_string());
    Ok(())
}

pub(super) fn unlock_session() {
    let path = session_lock_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        if s.trim() == std::process::id().to_string() {
            let _ = std::fs::remove_file(&path);
        }
    }
}
