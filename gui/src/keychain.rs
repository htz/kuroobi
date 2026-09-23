//! GGS credential storage (macOS Keychain).

use std::io::Write as _;
use std::process::{Command, Stdio};

const SERVICE_DEFAULT: &str = "kuroobi-ggs";

fn service() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        std::env::var("KUROOBI_KEYCHAIN_SERVICE")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| SERVICE_DEFAULT.to_string())
    })
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn clear() {
    loop {
        let deleted = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", service()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !deleted {
            break;
        }
    }
}

pub fn save(login: &str, pw: &str) {
    clear();
    let Ok(mut child) = Command::new("/usr/bin/security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(
            stdin,
            "add-generic-password -U -s {} -a {} -w {}",
            quote(service()),
            quote(login),
            quote(pw)
        );
    }
    let _ = child.wait();
}

pub fn forget() {
    save("-", "");
}

pub fn exists() -> bool {
    Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn load() -> Option<(String, String)> {
    let meta = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service()])
        .output()
        .ok()?;
    if !meta.status.success() {
        return None;
    }
    let meta = String::from_utf8_lossy(&meta.stdout);
    let login = meta.lines().find_map(|l| {
        l.trim()
            .strip_prefix("\"acct\"<blob>=\"")?
            .strip_suffix('"')
            .map(str::to_string)
    })?;
    let pw = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service(), "-w"])
        .output()
        .ok()?;
    if !pw.status.success() {
        return None;
    }
    let pw = String::from_utf8_lossy(&pw.stdout)
        .trim_end_matches('\n')
        .to_string();
    if pw.is_empty() {
        return None;
    }
    Some((login, pw))
}
