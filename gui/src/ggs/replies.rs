//! Replies to the queries the client sends (`who`, `match`, `stored`, `finger` ...), read into the snapshot.

use super::*;

fn is_month(t: &str) -> bool {
    matches!(
        t,
        "Jan"
            | "Feb"
            | "Mar"
            | "Apr"
            | "May"
            | "Jun"
            | "Jul"
            | "Aug"
            | "Sep"
            | "Oct"
            | "Nov"
            | "Dec"
    )
}

/// Everyone logged in to GGS, from the server's own `who`; `/os who` leaves out players who are in a game or not taking requests.
fn online_names(buf: &[String]) -> Vec<String> {
    buf.iter()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| n.starts_with(|c: char| c.is_ascii_alphabetic()))
        .map(str::to_string)
        .collect()
}

pub(super) fn capture_header_matches(kind: &str, ln: &str) -> bool {
    if kind.starts_with("who") {
        ln.starts_with("/os: who")
    } else if kind == "top" {
        ln.starts_with("/os: top")
    } else if kind == "stored_list" {
        ln.starts_with("/os: stored")
    } else if kind.starts_with("history:") {
        ln.starts_with("/os: history")
    } else if kind.starts_with("rank:") {
        ln.starts_with("/os: rank")
    } else if kind.starts_with("look:") {
        ln.starts_with("/os: look")
    } else if kind == "match_list" {
        ln.starts_with("/os: match")
    } else if kind.starts_with("osfinger:") {
        ln.starts_with("/os: finger")
    } else if kind.starts_with("watchers:") {
        ln.starts_with("/os: watch")
    } else if kind == "online" {
        ln.starts_with(": who")
    } else if kind.starts_with("finger:") {
        ln.starts_with(": finger") || ln.trim_start().starts_with("login")
    } else {
        false
    }
}

fn parse_dev_token(t: &str) -> Option<f32> {
    let head: String = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    head.parse::<f32>()
        .ok()
        .filter(|v| (0.0..1000.0).contains(v))
}

fn parse_rating_token(t: &str) -> Option<f32> {
    let head = t.split('@').next()?;
    head.parse::<f32>()
        .ok()
        .filter(|v| (100.0..4000.0).contains(v))
}

fn capture_who(ctx: &mut Ctx, kind: &str, buf: &[String], login: &str) {
    let mut users = Vec::new();
    let mut my_rating = None;
    for l in buf {
        let b = l.strip_prefix("/os: ").unwrap_or(l);
        let b = b.strip_prefix('|').unwrap_or(b);
        let mut toks: Vec<&str> = b.split_whitespace().collect();
        if kind == "top" {
            if toks.first().map(|t| t.chars().all(|c| c.is_ascii_digit())) == Some(true) {
                toks.remove(0);
            } else {
                continue;
            }
        }
        let Some(&name) = toks.first() else { continue };
        if name.is_empty()
            || !name.chars().next().unwrap().is_ascii_alphabetic()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+')
        {
            continue;
        }
        let at = toks
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(i, t)| parse_rating_token(t).map(|v| (i, v)));
        let Some((ri, r)) = at else { continue };
        let dev = match toks[ri].split_once('@') {
            Some((_, rest)) if !rest.is_empty() => parse_dev_token(rest),
            Some(_) => toks.get(ri + 1).and_then(|t| parse_dev_token(t)),
            None => None,
        };
        if name == login {
            my_rating = Some(r);
        }
        let open = if kind.starts_with("who") {
            toks.get(1)
                .filter(|t| matches!(*t, &"+" | &"-" | &"x"))
                .and_then(|t| t.chars().next())
        } else {
            None
        };
        users.push(UserRow {
            name: name.to_string(),
            rating: Some(r),
            dev,
            rating_r: None,
            dev_r: None,
            open,
            raw: b.to_string(),
        });
    }
    users.sort_by(|a, b| {
        b.rating
            .unwrap_or(0.0)
            .partial_cmp(&a.rating.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut s = ctx.snap.lock().unwrap();
    if !users.is_empty() {
        match kind {
            "who:8r" => {
                for u in &users {
                    if let Some(t) = s.users.iter_mut().find(|x| x.name == u.name) {
                        t.rating_r = u.rating;
                        t.dev_r = u.dev;
                    }
                }
            }
            k if k.starts_with("who") => s.users = users,
            _ => s.ranking = users,
        }
    }
    if let Some(r) = my_rating {
        s.my_rating = Some(r);
        if let Some(latest) = s.results.first_mut() {
            if latest.my_rating.is_none() {
                latest.my_rating = Some(r);
            }
        }
    }
    drop(s);
    ctx.dirty = true;
}

/// The `|.id ...` rows of a list reply, with their id.
fn id_rows(buf: &[String]) -> impl Iterator<Item = (&str, &str)> {
    buf.iter().filter_map(|l| {
        let rest = l.strip_prefix('|')?;
        let id = rest.split_whitespace().next()?;
        id.starts_with('.').then_some((id, rest))
    })
}

fn capture_match_list(ctx: &mut Ctx, buf: &[String], login: &str) {
    let mut list = Vec::new();
    for (id, rest) in id_rows(buf) {
        let (names, ratings, gtype) = match_line_parts(rest);
        list.push(OngoingView {
            id: id.to_string(),
            watching: false,
            names,
            ratings,
            gtype,
            mine: rest.contains(login),
        });
    }
    let mut s = ctx.snap.lock().unwrap();
    for o in list.iter_mut() {
        if s.ongoing.iter().any(|x| x.id == o.id && x.watching) {
            o.watching = true;
        }
    }
    if std::env::var("KUROOBI_GGS_AUTOWATCH").as_deref() == Ok("auto") {
        ctx.auto_watch.extend(list.iter().map(|o| o.id.clone()));
    }
    s.ongoing = list;
    drop(s);
    ctx.dirty = true;
}

fn capture_stored_list(ctx: &mut Ctx, buf: &[String], login: &str) {
    let mut list = Vec::new();
    for (id, rest) in id_rows(buf) {
        let toks: Vec<&str> = rest.split_whitespace().collect();
        let names: Vec<&str> = toks
            .iter()
            .filter(|t| {
                t.len() >= 2
                    && t.chars().next().map(|c| c.is_ascii_alphabetic()) == Some(true)
                    && !t.contains(':')
            })
            .copied()
            .collect();
        let opp = names
            .iter()
            .find(|n| **n != login && !is_month(n))
            .map(|s| s.to_string())
            .unwrap_or_default();
        let gtype = toks
            .iter()
            .rev()
            .find(|t| t.starts_with("s8") || t.starts_with('8'))
            .map(|s| s.to_string())
            .unwrap_or_default();
        list.push(StoredView {
            id: id.to_string(),
            opp,
            gtype,
        });
    }
    let mut s = ctx.snap.lock().unwrap();
    s.stored = list;
    drop(s);
    ctx.dirty = true;
}

fn capture_history(ctx: &mut Ctx, buf: &[String], login: &str, target: &str) {
    let mut rows = Vec::new();
    for l in buf {
        let Some(rest) = l.strip_prefix('|') else {
            continue;
        };
        let t: Vec<&str> = rest.split_whitespace().collect();
        if t.len() < 11 || !t[0].starts_with('.') {
            continue;
        }
        rows.push(HistoryRow {
            id: t[0].to_string(),
            at: format!("{} {} {} {}", t[1], t[2], t[3], t[4]),
            black_rating: t[5].to_string(),
            black: t[6].to_string(),
            white_rating: t[7].to_string(),
            white: t[8].to_string(),
            score: t[9].to_string(),
            gtype: t[10].to_string(),
        });
    }
    rows.reverse(); // newest first
    let key = if target.is_empty() {
        login.to_string()
    } else {
        target.to_string()
    };
    let mut s = ctx.snap.lock().unwrap();
    s.history.insert(key, rows);
    drop(s);
    ctx.dirty = true;
}

fn capture_rank(ctx: &mut Ctx, buf: &[String], login: &str, rest: &str) {
    let (gtype, target) = rest.split_once(':').unwrap_or((rest, ""));
    for l in buf {
        let b = l.strip_prefix("/os: ").unwrap_or(l);
        let b = b.strip_prefix('|').unwrap_or(b);
        let t: Vec<&str> = b.split_whitespace().collect();
        if t.len() < 3 {
            continue;
        }
        let Ok(rank) = t[0].parse::<u32>() else {
            continue;
        };
        if t[1] != target {
            continue;
        }
        // "1722.1@ 76.1=": a deviation under 100 is padded, so it lands in the next token.
        let (rating, dev) = match t[2].split_once('@') {
            Some((r, d)) => {
                let d = if d.is_empty() {
                    t.get(3).copied().unwrap_or("")
                } else {
                    d
                };
                (
                    r.parse::<f32>().unwrap_or(0.0),
                    d.trim_end_matches(['=', '+']).parse::<f32>().unwrap_or(0.0),
                )
            }
            None => continue,
        };
        let nums: Vec<u64> = t
            .iter()
            .rev()
            .filter_map(|x| x.parse::<u64>().ok())
            .take(3)
            .collect();
        let (wins, draws, losses) = match nums.len() {
            3 => (nums[2], nums[1], nums[0]),
            _ => (0, 0, 0),
        };
        let row = RankRow {
            gtype: gtype.to_string(),
            name: target.to_string(),
            rating,
            dev,
            rank,
            wins,
            draws,
            losses,
        };
        let mut s = ctx.snap.lock().unwrap();
        if target == login {
            s.my_ranks.retain(|r| r.gtype != row.gtype);
            s.my_ranks.push(row);
            s.my_ranks.sort_by(|a, b| a.gtype.cmp(&b.gtype));
        }
        drop(s);
        ctx.dirty = true;
    }
}

fn capture_osfinger(ctx: &mut Ctx, buf: &[String], name: &str) {
    let mut add: Vec<(String, String)> = Vec::new();
    for l in buf {
        if l.starts_with("/os:") {
            continue;
        }
        let t = l.trim_end().trim_start_matches('|');
        let Some((k, v)) = t.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || (k.contains(' ') && !k.contains('(')) {
            continue;
        }
        if k.to_lowercase().starts_with("passw") {
            continue;
        }
        add.push((k.to_string(), v.to_string()));
    }
    let mut s = ctx.snap.lock().unwrap();
    if let Some(f) = s.fingers.get_mut(name) {
        for (k, v) in add {
            if let Some(slot) = f.fields.iter_mut().find(|(kk, _)| *kk == k) {
                slot.1 = v;
            } else {
                f.fields.push((k, v));
            }
        }
    }
    drop(s);
    ctx.dirty = true;
}

fn capture_finger(ctx: &mut Ctx, buf: &[String], name: &str) {
    let mut fields: Vec<(String, String)> = Vec::new();
    for l in buf {
        let t = l.trim_end();
        if t.is_empty() || t == ":" {
            continue;
        }
        let t = t.strip_prefix(": ").unwrap_or(t);
        let lower = t.to_lowercase();
        if lower.starts_with("passw") {
            continue;
        }
        if let Some((k, v)) = t.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if !k.is_empty() && !k.contains(' ') || k.contains('(') {
                fields.push((k.to_string(), v.to_string()));
            }
        }
    }
    let mut s = ctx.snap.lock().unwrap();
    s.fingers.insert(name.to_string(), FingerInfo { fields });
    drop(s);
    ctx.dirty = true;
}

/// The answer to `watch .7 .9`: `/os: watch 2`, then `|.7        2 : root tdir` per match.
pub(super) fn watch_list(buf: &[String]) -> Vec<(String, Vec<String>)> {
    buf.iter()
        .filter_map(|l| {
            let rest = l.strip_prefix('|')?;
            let (head, names) = rest.split_once(':')?;
            let id = head.split_whitespace().next()?;
            id.starts_with('.').then(|| {
                (
                    id.to_string(),
                    names.split_whitespace().map(str::to_string).collect(),
                )
            })
        })
        .collect()
}

pub(super) fn finish_capture(ctx: &mut Ctx, kind: &str, buf: &[String], login: &str) {
    if kind == "online" {
        let names = online_names(buf);
        if !names.is_empty() {
            ctx.snap.lock().unwrap().online = Some(names);
            ctx.dirty = true;
        }
        return;
    }
    if let Some(id) = kind.strip_prefix("look:") {
        let joined: String = buf.iter().map(|l| l.trim()).collect::<Vec<_>>().join("");
        let mut parts: Vec<String> = Vec::new();
        let mut rest = joined.as_str();
        while let Some(i) = rest.find("(;") {
            let after = &rest[i..];
            let Some(end) = after.find(";)") else { break };
            let one = &after[..end + 2];
            if one.contains("GM[Othello]") {
                parts.push(one.to_string());
            }
            rest = &after[end + 2..];
        }
        let ggf = parts.first().cloned();
        let mut s = ctx.snap.lock().unwrap();
        match ggf {
            Some(g) => {
                s.fetched_ggf = Some(FetchedGgf {
                    id: id.to_string(),
                    ggf: g,
                    parts,
                    error: String::new(),
                });
            }
            None => {
                let err = buf
                    .iter()
                    .find(|l| l.contains("ERR"))
                    .cloned()
                    .unwrap_or_default();
                s.fetched_ggf = Some(FetchedGgf {
                    id: id.to_string(),
                    ggf: String::new(),
                    parts: Vec::new(),
                    error: if err.is_empty() {
                        "err.record_not_found".into()
                    } else {
                        err
                    },
                });
            }
        }
        drop(s);
        ctx.dirty = true;
        return;
    }
    if kind.starts_with("watchers:") {
        for (id, names) in watch_list(buf) {
            ctx.watchers.insert(id, names.into_iter().collect());
        }
        publish_watchers(ctx);
        return;
    }
    if kind.starts_with("who") || kind == "top" {
        capture_who(ctx, kind, buf, login);
    } else if kind == "match_list" {
        capture_match_list(ctx, buf, login);
    } else if kind == "stored_list" {
        capture_stored_list(ctx, buf, login);
    } else if let Some(target) = kind.strip_prefix("history:") {
        capture_history(ctx, buf, login, target);
    } else if let Some(rest) = kind.strip_prefix("rank:") {
        capture_rank(ctx, buf, login, rest);
    } else if let Some(name) = kind.strip_prefix("osfinger:") {
        capture_osfinger(ctx, buf, name);
    } else if let Some(name) = kind.strip_prefix("finger:") {
        capture_finger(ctx, buf, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_who_lists_every_login_but_not_the_services() {
        let buf: Vec<String> = [
            ": who   17        idle      online         ip-addr hostname",
            "/os      +    00:00:12312.04:55:04",
            "HCyrano  +    00:02:29  1.20:22:29",
            "kuroobi  +    00:00:00    00:06:42 153.242.133.003  <=",
            "nyanyan3 -    00:00:12    06:02:18",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(online_names(&buf), ["HCyrano", "kuroobi", "nyanyan3"]);
    }

    #[test]
    fn rating_token() {
        assert_eq!(parse_rating_token("1720.0@350.0"), Some(1720.0));
        assert_eq!(parse_rating_token("1938.0@"), Some(1938.0));
        assert_eq!(parse_rating_token("+33.6"), None); // a delta is out of range
        assert_eq!(parse_rating_token("71.7="), None);
    }
}
