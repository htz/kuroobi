//! The tournament director's (`/td`) messages folded into a list of tournaments.

use super::*;

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct TourneyView {
    pub id: u32,
    /// announced | running | break | over | cancelled
    pub state: String,
    pub desc: String,
    pub director: String,
    pub clock: String,
    pub gtype: String,
    pub players: Vec<String>,
    pub n_players: usize,
    pub rounds_played: u32,
    pub rounds: u32,
    /// Unix seconds of the next start (tournament or round), when known.
    pub next_at: Option<u64>,
    pub joined: bool,
    pub winner: String,
    /// Latest raw `finger`, `rankings` and full-schedule replies for this tournament.
    pub info: String,
    pub rankings: String,
    pub schedule: String,
}

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct TdReply {
    pub cmd: String,
    pub text: String,
    pub err: bool,
    pub at: u64,
}

#[derive(Clone, Serialize, Default)]
pub struct TdView {
    pub tourneys: Vec<TourneyView>,
    pub reply: Option<TdReply>,
    /// Our matches the director started (`+ match ... T`), newest last.
    pub games: Vec<TdGame>,
}

#[derive(Clone, Serialize, Default, Debug, PartialEq)]
pub struct TdGame {
    pub id: String,
    pub tid: u32,
    pub opp: String,
    /// Disc difference for us, summed over both boards of a synchro pair.
    pub diff: Option<i32>,
}

/// The tournament we are playing in right now, which a new `T` match belongs to.
pub(super) fn td_running(td: &TdView) -> u32 {
    td.tourneys
        .iter()
        .find(|t| t.joined && t.state == "running")
        .or_else(|| td.tourneys.iter().find(|t| t.joined && t.state != "over"))
        .map_or(0, |t| t.id)
}

/// `m:ss` as printed by the director (`TimeOut`).
fn td_secs(t: &str) -> Option<u64> {
    let (m, s) = t.trim().split_once(':')?;
    Some(m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?)
}

fn td_entry(td: &mut TdView, id: u32) -> &mut TourneyView {
    if let Some(i) = td.tourneys.iter().position(|t| t.id == id) {
        return &mut td.tourneys[i];
    }
    td.tourneys.push(TourneyView {
        id,
        ..Default::default()
    });
    td.tourneys.sort_by_key(|t| std::cmp::Reverse(t.id));
    let i = td.tourneys.iter().position(|t| t.id == id).unwrap();
    &mut td.tourneys[i]
}

/// The number right after `"tournament "` in `s`.
fn td_id_after(s: &str) -> Option<u32> {
    let rest = &s[s.find("tournament ")? + "tournament ".len()..];
    let n: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    n.parse().ok()
}

/// Folds one /td message (wording from GGS Service/TD/src) into the tournament list.
pub(super) fn td_apply(td: &mut TdView, login: &str, text: &str, direct: bool, now: u64) {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("").trim();
    let rest: Vec<&str> = lines.collect();
    let (first, rest) = if first.is_empty() || first.starts_with("---") {
        let mut it = rest
            .iter()
            .copied()
            .filter(|l| !l.trim_start().starts_with("---"));
        (it.next().unwrap_or("").trim(), it.collect::<Vec<_>>())
    } else {
        (first, rest)
    };

    if direct {
        let (err, body) = match first.strip_prefix("ERR ") {
            Some(e) => (true, e.split_once(": ").map_or(e, |x| x.1)),
            None => (false, first),
        };
        let (cmd, head) = body.split_once(": ").unwrap_or(("", body));
        let mut full = vec![head];
        full.extend(rest.iter().copied());
        td.reply = Some(TdReply {
            cmd: cmd.to_string(),
            text: full.join("\n"),
            err,
            at: now,
        });
        if err {
            return;
        }
        match cmd {
            "tourneys" => {
                td_apply_list(td, &rest);
                return;
            }
            "finger" => {
                td_apply_finger(td, login, head, &rest);
                if let Ok(id) = head.trim().parse::<u32>() {
                    td_entry(td, id).info = rest.join("\n");
                }
                return;
            }
            "rankings" | "st" => {
                if let Some(id) = td_id_after(head) {
                    let full = td
                        .reply
                        .as_ref()
                        .map(|r| r.text.clone())
                        .unwrap_or_default();
                    let t = td_entry(td, id);
                    if cmd == "rankings" {
                        t.rankings = full;
                    } else {
                        t.schedule = full;
                    }
                }
                return;
            }
            _ => {}
        }
        if let Some(text) = body.split_once(": ").map(|x| x.1) {
            td_apply_event(td, login, text, &rest, now);
        }
        return;
    }
    td_apply_event(td, login, first, &rest, now);
}

fn td_apply_event(td: &mut TdView, login: &str, first: &str, rest: &[&str], now: u64) {
    let Some(id) = td_id_after(first) else {
        return;
    };
    // "join: X joins tournament N" and "withdraw: X withdrawn from tournament N".
    let event = first.split_once(": ").map_or(first, |x| x.1);
    let t = td_entry(td, id);
    if let Some(m) = first
        .strip_prefix(&format!("tournament {id} starting in "))
        .and_then(|r| r.strip_suffix(" minutes"))
    {
        t.state = "announced".into();
        if let Ok(m) = m.trim().parse::<u64>() {
            t.next_at = Some(now + m * 60);
        }
        if let Some(d) = rest
            .iter()
            .find_map(|l| l.trim().strip_prefix('(').and_then(|x| x.strip_suffix(')')))
        {
            t.desc = d.to_string();
        }
    } else if let Some(r) = first.strip_prefix(&format!("tournament {id} starts in ")) {
        t.state = "announced".into();
        t.next_at = td_secs(r).map(|x| now + x);
    } else if let Some(r) = first.strip_prefix(&format!("tournament {id} round ")) {
        if let Some((round, wait)) = r.split_once(" starts in ") {
            t.state = "break".into();
            if let Ok(n) = round.trim().parse::<u32>() {
                t.rounds_played = n.saturating_sub(1);
            }
            t.next_at = td_secs(wait).map(|x| now + x);
        }
    } else if let Some(r) = first.strip_prefix("starting round ") {
        t.state = "running".into();
        t.next_at = None;
        if let Some(n) = r
            .split_whitespace()
            .next()
            .and_then(|x| x.parse::<u32>().ok())
        {
            t.rounds_played = n.saturating_sub(1);
        }
    } else if let Some(r) = first.strip_prefix("ending round ") {
        t.state = "break".into();
        if let Some(n) = r
            .split_whitespace()
            .next()
            .and_then(|x| x.parse::<u32>().ok())
        {
            t.rounds_played = n;
        }
    } else if first.contains(" is over.") {
        t.state = "over".into();
        t.next_at = None;
        if t.rounds > 0 {
            t.rounds_played = t.rounds;
        }
    } else if first.contains(" is cancelled") {
        t.state = "cancelled".into();
        t.next_at = None;
    } else if let Some(w) = first.strip_prefix("Congratulations to ") {
        t.winner = w.split(',').next().unwrap_or("").trim().to_string();
        t.state = "over".into();
        t.next_at = None;
    } else if let Some((who, _)) = event.split_once(" joins tournament ") {
        let who = who.trim();
        if who == login {
            t.joined = true;
        }
        if !t.players.iter().any(|p| p == who) {
            t.players.push(who.to_string());
            t.n_players += 1;
        }
    } else if let Some((who, _)) = event.split_once(" withdrawn from tournament ") {
        let who = who.trim();
        if who == login {
            t.joined = false;
        }
        if t.players.iter().any(|p| p == who) {
            t.players.retain(|p| p != who);
            t.n_players = t.n_players.saturating_sub(1);
        }
    }
}

/// Rows of `tourneys`: `id director service #pl played/ rounds start clock type [Winner: X]`.
fn td_apply_list(td: &mut TdView, rows: &[&str]) {
    for row in rows {
        // `%3d/%3d` runs together once both reach three digits ("100/100").
        let mut joined = row.to_string();
        while joined.contains("/ ") {
            joined = joined.replace("/ ", "/");
        }
        let mut f: Vec<&str> = joined.split_whitespace().collect();
        let Some(id) = f.first().and_then(|x| x.parse::<u32>().ok()) else {
            continue;
        };
        if f.len() < 8 {
            continue;
        }
        let Some((played, rounds)) = f[4].split_once('/') else {
            continue;
        };
        f.splice(4..5, [played, rounds]);
        let t = td_entry(td, id);
        t.director = f[1].to_string();
        // The count includes the bye once the tournament runs; the names, once known, do not.
        let listed = f[3].parse().unwrap_or(t.n_players);
        t.n_players = if t.players.is_empty() {
            listed
        } else {
            t.players.len()
        };
        t.rounds_played = f[4].trim_end_matches('/').parse().unwrap_or(0);
        t.rounds = f[5].parse().unwrap_or(0);
        t.clock = f[7].to_string();
        t.gtype = f[8].to_string();
        if let Some(i) = f.iter().position(|x| *x == "Winner:") {
            t.winner = f.get(i + 1).unwrap_or(&"").to_string();
        }
        match f[6] {
            "now" => {
                t.state = "running".into();
                t.next_at = None;
            }
            "done" => {
                t.state = "over".into();
                t.next_at = None;
            }
            w => {
                if t.state.is_empty() || t.state == "over" {
                    t.state = if t.rounds_played == 0 {
                        "announced"
                    } else {
                        "break"
                    }
                    .into();
                }
                t.next_at = td_secs(w).map(|x| now_secs() + x);
            }
        }
    }
}

/// `finger N`: `key : value` lines after the id.
fn td_apply_finger(td: &mut TdView, login: &str, head: &str, rest: &[&str]) {
    let Some(id) = head.trim().parse::<u32>().ok() else {
        return;
    };
    let t = td_entry(td, id);
    for l in rest {
        let l = l.trim();
        if let Some((k, v)) = l.split_once(" : ") {
            let v = v.trim();
            match k.trim() {
                "director" => t.director = v.to_string(),
                "clock" => t.clock = v.to_string(),
                "type" => t.gtype = v.to_string(),
                "rounds" => {
                    if let Some((a, b)) = v.split_once('/') {
                        t.rounds_played = a.trim().parse().unwrap_or(t.rounds_played);
                        t.rounds = b.trim().parse().unwrap_or(t.rounds);
                    }
                }
                "players" => {
                    // The director's count includes the bye it adds for an odd field.
                    t.players = v
                        .split_whitespace()
                        .skip(1)
                        .filter(|p| *p != "(bye)")
                        .map(|p| p.trim_matches(|c| c == '(' || c == ')').to_string())
                        .collect();
                    t.n_players = t.players.len();
                    t.joined = t.players.iter().any(|p| p == login);
                }
                _ => {}
            }
        } else if let Some(r) = l.strip_prefix("round ") {
            if let Some((_, w)) = r.split_once(" begins in ") {
                t.next_at = td_secs(w).map(|x| now_secs() + x);
                if t.rounds_played == 0 {
                    t.state = "announced".into();
                } else {
                    t.state = "break".into();
                }
            } else if r.contains("is being played") {
                t.state = "running".into();
                t.next_at = None;
            }
        } else if l == "tournament has finished" {
            t.state = "over".into();
            t.next_at = None;
        }
    }
}

/// Whether a request to join a tournament is carried out: from whom, and for which game types.
#[derive(Clone, Serialize, serde::Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct TdJoin {
    pub on: bool,
    /// Game types without their random-disc count: `s8r`, `8r`, `s8`, `8`.
    pub kinds: Vec<String>,
    /// From anyone; otherwise only from the tournament's director (when `from_director`) and `players`.
    pub from_any: bool,
    pub from_director: bool,
    pub players: Vec<String>,
}

impl Default for TdJoin {
    fn default() -> Self {
        TdJoin {
            on: false,
            kinds: vec!["s8r".into()],
            from_any: false,
            from_director: true,
            players: Vec::new(),
        }
    }
}

impl TdJoin {
    /// What the screen refuses to apply: on with no kind, or narrowed to nobody.
    pub fn incomplete(&self) -> bool {
        self.on
            && (self.kinds.is_empty()
                || (!self.from_any && !self.from_director && self.players.is_empty()))
    }
}

/// The tournament a private tell asks us to join: the command itself, written for us to send.
pub(super) fn join_request(text: &str) -> Option<u32> {
    let rest = text.trim();
    let rest = rest
        .strip_prefix("tell /td join ")
        .or_else(|| rest.strip_prefix("t /td join "))?;
    rest.trim().parse().ok()
}

/// `s8r14` -> `s8r`: the random-disc count does not change the kind.
pub(super) fn gtype_kind(gtype: &str) -> &str {
    gtype.trim_end_matches(|c: char| c.is_ascii_digit() && gtype.contains('r'))
}

/// Why `from`'s request to join tournament `id` is not carried out, judged on /td's own list.
/// Directors write the request, as GGS lets only superusers join someone else; others may too.
pub(super) fn director_join_refusal(
    td: &TdView,
    cfg: &TdJoin,
    id: u32,
    from: &str,
) -> Option<&'static str> {
    if !cfg.on {
        return Some("off");
    }
    let Some(t) = td.tourneys.iter().find(|t| t.id == id) else {
        return Some("unknown tournament");
    };
    let director = !t.director.is_empty() && t.director.eq_ignore_ascii_case(from);
    let listed = cfg.players.iter().any(|p| p.eq_ignore_ascii_case(from));
    if !(cfg.from_any || (cfg.from_director && director) || listed) {
        return Some("not someone we take it from");
    }
    if !cfg.kinds.iter().any(|k| k == gtype_kind(&t.gtype)) {
        return Some("game type not chosen");
    }
    if t.joined {
        return Some("already joined");
    }
    if matches!(t.state.as_str(), "over" | "cancelled") {
        return Some("over");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed() -> TdView {
        let mut td = TdView::default();
        td_apply(
            &mut td,
            "kuroobi",
            "tourneys: 2\n   id director  game #pl   round start             clock       type\n    3  HCyrano   /os  10   0/ 18  4:10        1:00//0:20      s8r14\n    4  HCyrano   /os   2   0/  4  4:10        1:00//0:20      8",
            true,
            100,
        );
        td
    }

    #[test]
    fn a_director_tell_is_read_as_a_join_request() {
        assert_eq!(join_request("tell /td join 3"), Some(3));
        assert_eq!(join_request(" t /td join 12 "), Some(12));
        assert_eq!(join_request("tell /td join three"), None);
        assert_eq!(join_request("please tell /td join 3"), None);
        assert_eq!(join_request("tell /td withdraw 3"), None);
    }

    #[test]
    fn kinds_drop_the_random_disc_count() {
        assert_eq!(gtype_kind("s8r14"), "s8r");
        assert_eq!(gtype_kind("8r16"), "8r");
        assert_eq!(gtype_kind("s8"), "s8");
        assert_eq!(gtype_kind("8"), "8");
    }

    #[test]
    fn only_the_director_of_a_chosen_kind_is_obeyed() {
        let td = listed();
        let on = TdJoin {
            on: true,
            ..TdJoin::default()
        };
        assert_eq!(director_join_refusal(&td, &on, 3, "HCyrano"), None);
        assert_eq!(director_join_refusal(&td, &on, 3, "hcyrano"), None);
        assert_eq!(
            director_join_refusal(&td, &on, 3, "someone"),
            Some("not someone we take it from")
        );
        assert_eq!(
            director_join_refusal(&td, &on, 4, "HCyrano"),
            Some("game type not chosen")
        );
        assert_eq!(
            director_join_refusal(&td, &on, 9, "HCyrano"),
            Some("unknown tournament")
        );
        assert_eq!(
            director_join_refusal(&td, &TdJoin::default(), 3, "HCyrano"),
            Some("off")
        );
    }

    #[test]
    fn listed_players_or_anyone_may_ask_too() {
        let td = listed();
        let players = TdJoin {
            on: true,
            from_director: false,
            players: vec!["Nyanyan4".into()],
            ..TdJoin::default()
        };
        assert_eq!(director_join_refusal(&td, &players, 3, "nyanyan4"), None);
        assert_eq!(
            director_join_refusal(&td, &players, 3, "HCyrano"),
            Some("not someone we take it from")
        );
        let any = TdJoin {
            on: true,
            from_any: true,
            from_director: false,
            ..TdJoin::default()
        };
        assert_eq!(director_join_refusal(&td, &any, 3, "someone"), None);
        assert_eq!(
            director_join_refusal(&td, &any, 4, "someone"),
            Some("game type not chosen")
        );
    }

    #[test]
    fn nobody_to_take_it_from_is_incomplete() {
        let none = TdJoin {
            on: true,
            from_director: false,
            ..TdJoin::default()
        };
        assert!(none.incomplete());
        assert!(!TdJoin {
            on: false,
            ..none.clone()
        }
        .incomplete());
        assert!(TdJoin {
            on: true,
            kinds: vec![],
            ..TdJoin::default()
        }
        .incomplete());
    }

    #[test]
    fn td_follows_a_tournament_from_announcement_to_end() {
        let mut td = TdView::default();
        let me = "kuroobi";
        td_apply(
            &mut td,
            me,
            "tournament 31 starting in 5 minutes\n(8x8 SynchroRand Reversi clock: 3:00//0:30, 10 rounds rrobin)\nto play: tell /td join 31\nfor tournament messages: chann + .tourney",
            false,
            1000,
        );
        let t = &td.tourneys[0];
        assert_eq!(
            (t.id, t.state.as_str(), t.next_at),
            (31, "announced", Some(1300))
        );
        assert_eq!(
            t.desc,
            "8x8 SynchroRand Reversi clock: 3:00//0:30, 10 rounds rrobin"
        );

        td_apply(&mut td, me, "join: egrcd joins tournament 31", false, 1005);
        td_apply(&mut td, me, "join: kuroobi joins tournament 31", true, 1010);
        assert!(td.tourneys[0].joined);
        assert_eq!(td.tourneys[0].n_players, 2);
        assert_eq!(td.reply.as_ref().unwrap().cmd, "join");

        td_apply(&mut td, me, "tournament 31 starts in  1:00", false, 1240);
        assert_eq!(td.tourneys[0].next_at, Some(1300));
        td_apply(
            &mut td,
            me,
            "starting round 1 of tournament 31",
            false,
            1300,
        );
        assert_eq!(td.tourneys[0].state, "running");
        td_apply(
            &mut td,
            me,
            "ending round 1 of tournament 31\nround   1:\n  1.1  kuroobi   egrcd    +2.0",
            false,
            1500,
        );
        let t = &td.tourneys[0];
        assert_eq!((t.state.as_str(), t.rounds_played), ("break", 1));
        td_apply(
            &mut td,
            me,
            "tournament 31 round 2 starts in  1:00",
            false,
            1500,
        );
        assert_eq!(td.tourneys[0].next_at, Some(1560));
        td_apply(
            &mut td,
            me,
            "Congratulations to kuroobi, winner of tournament 31",
            false,
            9000,
        );
        let t = &td.tourneys[0];
        assert_eq!((t.state.as_str(), t.winner.as_str()), ("over", "kuroobi"));
    }

    #[test]
    fn td_does_not_count_the_bye_as_a_player() {
        let mut td = TdView::default();
        td_apply(
            &mut td,
            "alice",
            "finger: 2\nt alice finger: \nid        : 2\njoining   : closed\nrounds    : 1/2\nplayers   : 3 alice bob (bye) ",
            true,
            100,
        );
        td_apply(
            &mut td,
            "alice",
            "tourneys: 1\n   id director  game #pl   round start             clock       type\n    2     root   /os   3   1/  2  0:58        0:30//0:10      s8r14",
            true,
            200,
        );
        let t = &td.tourneys[0];
        assert_eq!(
            (t.n_players, t.players.clone()),
            (2, vec!["alice".to_string(), "bob".to_string()])
        );
    }

    #[test]
    fn td_reads_list_finger_and_errors() {
        let mut td = TdView::default();
        td_apply(
            &mut td,
            "kuroobi",
            "tourneys: 2\n   id director  game #pl   round start             clock       type\n   27  HCyrano    os   7  10/ 10  done        3:00//0:30      s8r14   Winner: ymatioun\n   31  HCyrano    os   3   0/ 10  4:10        3:00//0:30      s8r14\n   29  HCyrano   /os   2   2/  2  done        1:00//0:30      s8r14   Winner: iEdax\n   11  HCyrano   /os   6 100/100  done        1:00//2:00      s8r14   Winner: ymatioun",
            true,
            100,
        );
        assert_eq!(td.tourneys.len(), 4);
        let t29 = td.tourneys.iter().find(|t| t.id == 29).unwrap();
        assert_eq!(
            (t29.rounds_played, t29.rounds, t29.state.as_str()),
            (2, 2, "over")
        );
        let t11 = td.tourneys.iter().find(|t| t.id == 11).unwrap();
        assert_eq!(
            (
                t11.state.as_str(),
                t11.rounds,
                t11.gtype.as_str(),
                t11.winner.as_str()
            ),
            ("over", 100, "s8r14", "ymatioun")
        );
        let t27 = td.tourneys.iter().find(|t| t.id == 27).unwrap();
        assert_eq!(
            (t27.state.as_str(), t27.winner.as_str(), t27.rounds),
            ("over", "ymatioun", 10)
        );
        let t31 = td.tourneys.iter().find(|t| t.id == 31).unwrap();
        assert_eq!(
            (t31.state.as_str(), t31.n_players, t31.clock.as_str()),
            ("announced", 3, "3:00//0:30")
        );

        td_apply(
            &mut td,
            "kuroobi",
            "finger: 31\nt kuroobi finger: \nid        : 31\njoining   : open\nrounds    : 0/10\nplayers   : 3 egrcd kuroobi (ymatioun) \nround 1 begins in  4:00",
            true,
            200,
        );
        let t31 = td.tourneys.iter().find(|t| t.id == 31).unwrap();
        assert!(t31.joined);
        assert_eq!(t31.players, ["egrcd", "kuroobi", "ymatioun"]);

        td_apply(
            &mut td,
            "kuroobi",
            "ERR 7: join: tournament id does not exist",
            true,
            300,
        );
        let r = td.reply.as_ref().unwrap();
        assert!(r.err);
        assert_eq!(
            (r.cmd.as_str(), r.text.as_str()),
            ("join", "tournament id does not exist")
        );
    }
}
