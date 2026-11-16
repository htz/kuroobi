//! What a client shares with the app around it: the config folder, engine resources and thread
//! count, the book-learning log, and the saved GGS credentials.

use std::path::PathBuf;

use kuroobi::resources::Resources;
use serde::{Deserialize, Serialize};

pub fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(4)
}

pub fn config_dir() -> PathBuf {
    dirs_config()
        .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")))
        .join("kuroobi")
}

pub fn resources_path() -> PathBuf {
    config_dir().join("resources.conf")
}

pub fn learn_log_path() -> PathBuf {
    if let Ok(p) = std::env::var("KUROOBI_LEARN_LOG") {
        return PathBuf::from(p);
    }
    config_dir().join("learn_log.jsonl")
}

pub fn dirs_config() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var("HOME").ok()?;
    #[cfg(target_os = "macos")]
    return Some(PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(target_os = "macos"))]
    return Some(PathBuf::from(home).join(".config"));
}

pub fn resources() -> Resources {
    Resources::load(&resources_path())
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
    append_line(&path, &line);
    trim_learn_log(&path);
}

pub fn append_line(path: &std::path::Path, line: &str) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

pub const LEARN_LOG_MAX: usize = 200;

pub fn trim_learn_log(path: &std::path::Path) {
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

pub fn read_credentials() -> Option<(String, String)> {
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
