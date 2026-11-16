//! The GGS client, apart from any screen: the session, the matches and the engines playing them
//! (`client`), driven through `client::Cmd` by the GUI or the CLI, which report back through [`Host`].

use std::time::Duration;

use kuroobi::Position;
use play::confidence_pct;

pub mod client;
pub mod env;
pub mod formula;
pub mod i18n;
pub mod net;

/// What the app around a client provides: where the state goes, how to tell the user, and
/// whether its own engine is busy.
pub trait Host: Send + Sync + 'static {
    /// The client's state changed; called at most every 120 ms unless forced.
    fn publish(&self, snapshot: &client::Snapshot);
    fn notify(&self, title: &str, body: &str);
    /// The app's own engine is searching a position of its own.
    fn local_search_running(&self) -> bool;
}

pub fn is_pass(mv: &str) -> bool {
    mv.eq_ignore_ascii_case("pa") || mv.eq_ignore_ascii_case("pass")
}

pub fn my_stone_diff(score: f32, first_name: &str, login: &str) -> i32 {
    let v = score.round() as i32;
    if first_name == login {
        v
    } else {
        -v
    }
}

pub struct Request {
    pub verb: &'static str,
    pub id: String,
    pub who: String,
}

pub fn parse_request(ln: &str, login: &str) -> Option<Request> {
    let rest = ln.strip_prefix("/os: ")?;
    let verb = ["undo", "abort"]
        .into_iter()
        .find(|v| rest.starts_with(&format!("{v} ")))?;
    let mut it = rest[verb.len() + 1..].split_whitespace();
    let id = it.next()?;
    let who = it.next()?;
    if it.next() != Some("is") || who == login || !id.starts_with('.') {
        return None;
    }
    Some(Request {
        verb,
        id: id.to_string(),
        who: who.to_string(),
    })
}

pub fn parse_clock(s: &str) -> (Option<u64>, Option<u64>, Option<u64>) {
    let mut out = [None, None, None];
    for (i, part) in s.split('/').take(3).enumerate() {
        let p = part.trim().split(',').next().unwrap_or("").trim();
        let (p, neg) = match p.strip_prefix('-') {
            Some(rest) => (rest.trim(), true),
            None => (p, false),
        };
        if p.is_empty()
            || !p
                .chars()
                .next()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
        {
            continue;
        }
        let secs = p
            .split(':')
            .try_fold(0u64, |acc, seg| Some(acc * 60 + seg.parse::<u64>().ok()?));
        if let Some(secs) = secs {
            out[i] = Some(if neg { 0 } else { secs });
        }
    }
    (out[0], out[1], out[2])
}

pub fn base_id(id: &str) -> &str {
    match id.rsplit_once('.') {
        Some((head, tail)) if tail.len() == 1 && head.contains('.') => head,
        _ => id,
    }
}

pub fn play_arg(mstr: &str, value: f32, took: Option<std::time::Duration>) -> String {
    if mstr == "pa" {
        return mstr.to_string();
    }
    let v = if value.is_finite() { value } else { 0.0 };
    match took {
        Some(t) => format!("{mstr}/{v:.2}/{:.2}", t.as_secs_f64()),
        None => format!("{mstr}/{v:.2}"),
    }
}

pub fn coord(p: Position) -> String {
    p.to_kifu().to_uppercase()
}

pub fn move_text(pos: Option<Position>) -> String {
    match pos {
        Some(p) => coord(p),
        None => "pa".to_string(),
    }
}

/// GGS caps channel names at 8 characters including the dot; empty turns the log off.
pub fn valid_log_channel(chan: &str) -> bool {
    chan.is_empty()
        || chan.strip_prefix('.').is_some_and(|name| {
            (1..=7).contains(&name.len())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

pub struct Searched {
    pub empties: u8,
    pub threads: usize,
    pub nodes: u64,
    pub took: Duration,
    pub pv: Vec<Option<Position>>,
    pub black: bool,
}

pub fn count(n: f64, unit: &str) -> String {
    if n >= 1e9 {
        format!("{:.3} G{unit}", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} M{unit}", n / 1e6)
    } else {
        format!("{:.1} K{unit}", n / 1e3)
    }
}

/// Edax's spectator log from the GGS guide: black's moves in capitals, white's in lower case.
pub fn search_log(
    chan: &str,
    login: &str,
    mid: &str,
    mv: &kuroobi::engine::MoveEval,
    s: &Searched,
) -> Vec<String> {
    let square = |p: Option<Position>, black: bool| {
        let t = p.map_or("PA".to_string(), coord);
        if black {
            t
        } else {
            t.to_lowercase()
        }
    };
    let mut lines = vec![
        "-----------------------------------------".to_string(),
        format!(
            "{login} plays {} in game {mid} using {} threads ;",
            square(mv.pos, s.black),
            s.threads
        ),
    ];
    let score = mv.value.round() as i32;
    if mv.from_book {
        lines.push(format!("score == {score:+} from book ;"));
    } else {
        let at = if mv.exact || mv.depth == 0 {
            format!("{}@100%", s.empties)
        } else {
            let t = mv.mpc_t.unwrap_or_else(kuroobi::midgame::mpc_t);
            format!("{}@{}%", mv.depth, confidence_pct(t))
        };
        let pv: Vec<String> =
            s.pv.iter()
                .scan(!s.black, |black, p| {
                    *black = !*black;
                    Some(square(*p, *black))
                })
                .collect();
        let secs = s.took.as_secs_f64();
        lines.push(format!(
            "score == {score:+} at {at} ; PV: {} ;",
            pv.join(" ")
        ));
        lines.push(format!(
            "nodes: {} ; time: search = {secs:.1}s, move = {secs:.1}s; speed: {}.",
            count(s.nodes as f64, "N"),
            count(s.nodes as f64 / secs.max(1e-3), "N/s"),
        ));
        lines.push(if mv.cut {
            format!("search out of time at {at}")
        } else {
            "search completed".to_string()
        });
    }
    lines
        .into_iter()
        .map(|l| format!("tell {chan} {l}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuroobi::Position;
    use std::time::Duration;

    #[test]
    fn a_log_channel_is_a_dot_and_up_to_seven_name_characters() {
        for ok in ["", ".kuroobi", ".Rhapsod", ".a", ".my_bot1", ".x-y_z"] {
            assert!(valid_log_channel(ok), "{ok:?}");
        }
        for bad in [
            "kuroobi",
            ".",
            ".Rhapsody",
            " .kuroobi",
            ".kuro obi",
            "..kuroo",
            ".くろおび",
        ] {
            assert!(!valid_log_channel(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_search_log_follows_edax_s_layout() {
        let sq = |t: &str| Position::from_kifu(t).ok();
        let s = |empties, black| Searched {
            empties,
            threads: 8,
            nodes: 2_149_000_000,
            took: Duration::from_millis(4300),
            pv: vec![sq("f5"), sq("d6"), None, sq("c3")],
            black,
        };
        let mid = kuroobi::engine::MoveEval {
            pos: sq("f5"),
            value: 2.4,
            depth: 22,
            ..Default::default()
        };
        assert_eq!(
            search_log(".kuroobi", "kuroobi", ".1.0", &mid, &s(40, true)),
            [
                "tell .kuroobi -----------------------------------------",
                "tell .kuroobi kuroobi plays F5 in game .1.0 using 8 threads ;",
                "tell .kuroobi score == +2 at 22@73% ; PV: F5 d6 PA c3 ;",
                "tell .kuroobi nodes: 2.149 GN ; time: search = 4.3s, move = 4.3s; speed: 499.8 MN/s.",
                "tell .kuroobi search completed",
            ]
        );
        let sel = kuroobi::engine::MoveEval {
            pos: sq("f5"),
            value: -1.0,
            depth: 32,
            mpc_t: Some(2.5),
            cut: true,
            ..Default::default()
        };
        let lines = search_log(".kuroobi", "kuroobi", ".1.0", &sel, &s(32, false));
        assert_eq!(
            lines[1],
            "tell .kuroobi kuroobi plays f5 in game .1.0 using 8 threads ;"
        );
        assert_eq!(
            lines[2],
            "tell .kuroobi score == -1 at 32@99% ; PV: f5 D6 pa C3 ;"
        );
        assert_eq!(lines[4], "tell .kuroobi search out of time at 32@99%");
        let exact = kuroobi::engine::MoveEval {
            pos: sq("f5"),
            value: 4.0,
            exact: true,
            ..Default::default()
        };
        assert!(
            search_log(".kuroobi", "kuroobi", ".1.0", &exact, &s(26, true))[2]
                .starts_with("tell .kuroobi score == +4 at 26@100% ;")
        );
        let book = kuroobi::engine::MoveEval {
            pos: sq("f5"),
            value: 0.5,
            from_book: true,
            ..Default::default()
        };
        assert_eq!(
            search_log(".kuroobi", "kuroobi", ".1.0", &book, &s(50, true))[2..],
            ["tell .kuroobi score == +1 from book ;"]
        );
    }

    #[test]
    fn clock_real_format() {
        let (main, inc, ext) = parse_clock("14:59,0:0//02:00,0:0");
        assert_eq!(main, Some(14 * 60 + 59));
        assert_eq!(inc, None);
        assert_eq!(ext, Some(120));
        let (main, _, ext) = parse_clock("15:00//02:00");
        assert_eq!(main, Some(900));
        assert_eq!(ext, Some(120));
        let (main, _, ext) = parse_clock("00:07");
        assert_eq!(main, Some(7));
        assert_eq!(ext, None);
    }

    #[test]
    fn the_stone_diff_follows_the_first_name() {
        assert_eq!(my_stone_diff(2.0, "kuroobi", "kuroobi"), 2);
        assert_eq!(my_stone_diff(-10.0, "kuroobi", "kuroobi"), -10);
        assert_eq!(my_stone_diff(-5.0, "kuroobi", "kuroobi"), -5);
        assert_eq!(my_stone_diff(-7.0, "kuroobi", "kuroobi"), -7);
        assert_eq!(my_stone_diff(-6.0, "htz", "kuroobi"), 6);
        assert_eq!(my_stone_diff(2.0, "htz", "kuroobi"), -2);
        assert_eq!(my_stone_diff(0.0, "kuroobi", "kuroobi"), 0);
        assert_eq!(my_stone_diff(0.0, "htz", "kuroobi"), 0);
    }

    #[test]
    fn request_real_format() {
        let r = parse_request("/os: undo .24 htz is asking", "kuroobi").unwrap();
        assert_eq!(
            (r.verb, r.id.as_str(), r.who.as_str()),
            ("undo", ".24", "htz")
        );
        let r = parse_request("/os: abort .24 htz is asking", "kuroobi").unwrap();
        assert_eq!(r.verb, "abort");
        assert!(parse_request("/os: undo .24 kuroobi is asking", "kuroobi").is_none());
        assert!(parse_request("/os: update .24 8 K?", "kuroobi").is_none());
        assert!(parse_request("/os: undo .24 htz declined", "kuroobi").is_none());
        assert!(parse_request("/os: - match .24 1720 htz", "kuroobi").is_none());
    }

    #[test]
    fn a_negative_clock_reads_as_zero() {
        let (main, _, ext) = parse_clock("-0:03,0:0//02:00,0:0");
        assert_eq!(main, Some(0));
        assert_eq!(ext, Some(120));
    }

    #[test]
    fn base_id_synchro() {
        assert_eq!(base_id(".82726.1"), ".82726");
        assert_eq!(base_id(".82726.2"), ".82726");
        assert_eq!(base_id(".13"), ".13");
    }
}

#[cfg(test)]
mod play_arg_tests {
    use super::play_arg;
    use std::time::Duration;

    #[test]
    fn a_move_carries_its_score_and_seconds() {
        let s = play_arg("D8", 6.0, Some(Duration::from_millis(17_390)));
        assert_eq!(s, "D8/6.00/17.39");
    }

    #[test]
    fn a_negative_score_keeps_its_sign() {
        let s = play_arg("F5", -3.5, Some(Duration::from_millis(1_200)));
        assert_eq!(s, "F5/-3.50/1.20");
    }

    #[test]
    fn a_pass_goes_bare() {
        assert_eq!(play_arg("pa", 12.0, Some(Duration::from_secs(3))), "pa");
    }

    #[test]
    fn a_non_finite_score_becomes_zero() {
        let s = play_arg("A1", f32::INFINITY, Some(Duration::from_secs(1)));
        assert_eq!(s, "A1/0.00/1.00");
    }

    #[test]
    fn an_unmeasured_move_omits_the_time() {
        assert_eq!(play_arg("C4", 2.0, None), "C4/2.00");
    }
}
