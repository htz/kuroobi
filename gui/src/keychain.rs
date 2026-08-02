//! GGS 認証情報の保存先 (macOS キーチェーン)。
//!
//! Keychain API をプロセスから直接叩くと、項目のアクセス許可がバイナリの
//! 署名に紐づき、再ビルドのたびに許可ダイアログが出る (画面の自動確認も
//! 止まる)。Apple 署名済みの /usr/bin/security コマンドを介せば、項目の
//! 所有者が security になるためダイアログが出ず、.app 化前の開発ビルド
//! でも同じに動く。
//!
//! パスワードを引数に渡すと ps に一瞬映るので、書き込みは `security -i`
//! (標準入力からコマンドを読むモード) で行う。

use std::io::Write as _;
use std::process::{Command, Stdio};

/// キーチェーン上のサービス名。項目はこの名前で 1 つだけ持つ。
const SERVICE: &str = "kuroobi-ggs";

/// security -i のコマンド行に埋め込むための引用。
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 保存する (同じサービス名の項目は上書き)。失敗しても呼び出し側の
/// ログイン自体は成功しているので、黙って諦める。
pub fn save(login: &str, pw: &str) {
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
            "add-generic-password -U -s {SERVICE} -a {} -w {}",
            quote(login),
            quote(pw)
        );
    }
    let _ = child.wait();
}

/// 保存済みの (ログイン名, パスワード) を読む。無ければ None。
pub fn load() -> Option<(String, String)> {
    // 項目のメタデータからログイン名 (acct) を取り出す
    let meta = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", SERVICE])
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
    // パスワード本体は -w で標準出力に出る
    let pw = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", SERVICE, "-w"])
        .output()
        .ok()?;
    if !pw.status.success() {
        return None;
    }
    let pw = String::from_utf8_lossy(&pw.stdout)
        .trim_end_matches('\n')
        .to_string();
    Some((login, pw))
}
