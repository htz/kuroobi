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

/// 同じサービス名の項目を全部消す。add の -U は「サービス+アカウントが
/// 一致」した項目しか上書きせず、アカウントが違う項目が併存すると
/// find が古い方を返し続けるため、書く前に必ず掃除する。
fn clear() {
    loop {
        let deleted = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", SERVICE])
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

/// 保存する (既存の項目は入れ替え)。失敗しても呼び出し側の
/// ログイン自体は成功しているので、黙って諦める。
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
            "add-generic-password -U -s {SERVICE} -a {} -w {}",
            quote(login),
            quote(pw)
        );
    }
    let _ = child.wait();
}

/// ログアウト。削除ではなく空パスワードで上書きする (墓標)。項目ごと
/// 消すと「キーチェーンが空なら旧ファイルから取り込む」移行処理が次回
/// 起動時に働き、自動ログインが復活してしまう。
pub fn forget() {
    save("-", "");
}

/// 項目があるか (ログアウトの墓標も含む)。旧ファイルからの取り込みを
/// 項目が全く無い初回だけに限るための照会。
pub fn exists() -> bool {
    Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", SERVICE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 保存済みの (ログイン名, パスワード) を読む。無ければ None。
/// ログアウトの墓標 (空パスワード) も None。
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
    if pw.is_empty() {
        return None;
    }
    Some((login, pw))
}
