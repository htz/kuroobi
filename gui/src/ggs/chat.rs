//! Chat lines: what counts as a message, its `|` continuation lines, and the per-thread caps.

use super::*;

const BARE_CMDS: [&str; 2] = ["verbose", "chann"];

pub(super) const CHAT_IN_MEMORY: usize = 300;

fn parse_chat(ln: &str) -> Option<ChatMsg> {
    let at = now_secs();
    if let Some(rest) = ln.strip_prefix('.') {
        let (head, text) = rest.split_once(": ")?;
        let mut it = head.split_whitespace();
        if let (Some(chan), Some(from), None) = (it.next(), it.next(), it.next()) {
            return Some(ChatMsg {
                chan: format!(".{chan}"),
                from: from.to_string(),
                text: text.to_string(),
                at,
                thread: format!(".{chan}"),
            });
        }
        return None;
    }
    if ln.starts_with(['/', '|', ':', ' ']) || ln == "READY" {
        return None;
    }
    let (name, text) = ln.split_once(": ")?;
    let named = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && !BARE_CMDS.contains(&name);
    named.then(|| ChatMsg {
        chan: String::new(),
        from: name.to_string(),
        text: text.to_string(),
        at,
        thread: name.to_string(),
    })
}

/// Returns the messages this line completes, and whether the line belonged to a chat.
pub(super) fn take_chat_line(block: &mut Option<ChatMsg>, ln: &str) -> (Vec<ChatMsg>, bool) {
    if let Some(m) = block.as_mut() {
        if let Some(body) = ln.strip_prefix('|') {
            if !body.starts_with("---") {
                if !m.text.is_empty() {
                    m.text.push('\n');
                }
                m.text.push_str(body.trim_end());
            }
            return (Vec::new(), true);
        }
    }
    let done: Vec<ChatMsg> = block.take().into_iter().collect();
    let Some(msg) = parse_chat(ln) else {
        return (done, false);
    };
    // A message may continue on "|" lines (a /td announcement even starts with an empty header), so it waits for the next line.
    *block = Some(msg);
    (done, true)
}

pub(super) fn deliver_chat(ctx: &mut Ctx, login: &str, msg: ChatMsg) {
    let chan = ctx.snap.lock().unwrap().log_channel.clone();
    if msg.chan != chan || msg.from != login {
        push_chat(ctx, login, msg);
    }
}

pub(super) fn push_chat(ctx: &mut Ctx, login: &str, msg: ChatMsg) {
    append_chat(login, &msg);
    let mut s = ctx.snap.lock().unwrap();
    let thread = msg.thread.clone();
    s.chat.push_back(msg);
    if s.chat.iter().filter(|m| m.thread == thread).count() > CHAT_IN_MEMORY {
        if let Some(i) = s.chat.iter().position(|m| m.thread == thread) {
            s.chat.remove(i);
        }
    }
    drop(s);
    ctx.dirty = true;
}

/// Caps each thread separately so a busy analysis channel cannot push out conversations.
pub(super) fn keep_latest_per_thread(msgs: Vec<ChatMsg>, n: usize) -> Vec<ChatMsg> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut keep: Vec<ChatMsg> = msgs
        .into_iter()
        .rev()
        .filter(|m| {
            let c = seen.entry(m.thread.clone()).or_default();
            *c += 1;
            *c <= n
        })
        .collect();
    keep.reverse();
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn td_announcement_keeps_its_body() {
        let mut block = None;
        let lines = [
            ".chat /td: ",
            "|---------------- /td --------------",
            "|tournament 30 starting in 2 minutes",
            "|to play: tell /td join 30",
            "|---------------- /td --------------",
            "READY",
        ];
        let mut got = Vec::new();
        for ln in lines {
            got.extend(take_chat_line(&mut block, ln).0);
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].from, "/td");
        assert_eq!(
            got[0].text,
            "tournament 30 starting in 2 minutes\nto play: tell /td join 30"
        );
        assert!(block.is_none());
    }

    #[test]
    fn busy_channel_does_not_evict_other_threads() {
        let msg = |thread: &str, at: u64| ChatMsg {
            chan: thread.into(),
            from: "x".into(),
            text: "t".into(),
            at,
            thread: thread.into(),
        };
        let mut v = vec![msg(".chat", 0)];
        v.extend((1..=10).map(|i| msg(".Harmony", i)));
        let kept = keep_latest_per_thread(v, 3);
        assert_eq!(kept.iter().filter(|m| m.thread == ".chat").count(), 1);
        let harmony: Vec<u64> = kept
            .iter()
            .filter(|m| m.thread == ".Harmony")
            .map(|m| m.at)
            .collect();
        assert_eq!(harmony, [8, 9, 10]);
    }
}
