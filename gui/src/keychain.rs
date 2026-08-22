//! GGS credential storage (macOS Keychain).
//!
//! Calling the Keychain API directly ties item ACLs to the binary
//! signature, so every rebuild pops a permission dialog (and stalls
//! automated screenshots). Going through the Apple-signed
//! /usr/bin/security makes `security` the item owner: no dialogs, and
//! dev builds behave like the packaged app.
//!
//! Passwords passed as arguments flash in `ps`, so writes go through
//! `security -i` (commands on stdin).

use std::io::Write as _;
use std::process::{Command, Stdio};

/// Keychain service name; exactly one item is kept under it.
const SERVICE_DEFAULT: &str = "kuroobi-ggs";

/// Service name, overridable via `KUROOBI_KEYCHAIN_SERVICE`.
///
/// Only one item is kept, so two instances cannot remember different
/// accounts under the default (the later login evicts the earlier).
/// Rename one side when running dev and production side by side.
fn service() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        std::env::var("KUROOBI_KEYCHAIN_SERVICE")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| SERVICE_DEFAULT.to_string())
    })
}

/// Quote for embedding in a `security -i` command line.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Delete every item under the service name. `add -U` only replaces
/// items whose service AND account match; with a different-account item
/// around, find keeps returning the stale one — sweep before writing.
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

/// Save (replacing any existing item). Failure is silent — the login
/// itself already succeeded.
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

/// Logout: overwrite with an empty password (a tombstone) rather than
/// delete. Deleting would re-trigger the legacy-file migration on next
/// launch and resurrect auto-login.
pub fn forget() {
    save("-", "");
}

/// Whether any item exists (tombstones included); gates the legacy-file
/// migration to the true first run.
pub fn exists() -> bool {
    Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Read the stored (login, password); None if absent or a tombstone.
pub fn load() -> Option<(String, String)> {
    // Pull the login (acct) from the item metadata.
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
    // The password itself comes out on stdout with -w.
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
