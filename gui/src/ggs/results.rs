//! Finished matches: the history file, each board's record, and what the wire log still tells about older ones.

use super::*;

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct GameResult {
    pub id: String,
    pub base: String,
    pub raw: String,
    pub my_diff: Option<i32>,
    pub opp: String,
    pub kifu: String,
    #[serde(default)]
    pub ggf: String,
    #[serde(default)]
    pub archive: String,
    pub seq: u64,
    #[serde(default)]
    pub my_rating: Option<f32>,
    #[serde(default)]
    pub at: u64,
    /// Every board of the match; `kifu` and `ggf` above hold only the first, which lost half of each synchro pair.
    #[serde(default)]
    pub boards: Vec<BoardRecord>,
    /// How the whole match was decided when it was not played out: "resign" or "adjourn" (GGS decides both
    /// per match, both boards of a synchro at once); empty otherwise.
    #[serde(default)]
    pub ending: String,
    #[serde(default)]
    pub ending_by: String,
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct BoardRecord {
    pub id: String,
    pub kifu: String,
    pub ggf: String,
    /// "black" | "white": the side the player had on this board.
    #[serde(default)]
    pub my_color: String,
    /// The board's result from the player's side as GGS scored it (±64 for running out of time, the
    /// smallest loss for over time); boards add up to the match's recorded result.
    #[serde(default)]
    pub disc_diff: Option<i32>,
    /// "over_time" (main time ran out, the board went on in the extension) or "out_of_time"
    /// (the extension ran out too); empty when the board was played out.
    #[serde(default)]
    pub ending: String,
    #[serde(default)]
    pub ending_by: String,
    /// Each player's clock when the board ended.
    #[serde(default)]
    pub clocks: Vec<ClockRecord>,
}

#[derive(Clone, Serialize, serde::Deserialize, Default)]
pub struct ClockRecord {
    pub name: String,
    pub color: String,
    pub main: Option<u64>,
    pub ext: Option<u64>,
}

/// The line is signed for the name written first; a synchro match scores the mean of two boards.
pub(super) fn diff_from_raw(raw: &str, opp: &str) -> Option<i32> {
    if opp.is_empty() {
        return None;
    }
    let (Some(score), Some(first)) = match_score(raw) else {
        return None;
    };
    let v = score.round() as i32;
    Some(if first == opp { -v } else { v })
}

pub(super) fn load_history() -> Vec<GameResult> {
    let Ok(text) = std::fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    let mut out: Vec<GameResult> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<GameResult>(l).ok())
        .collect();
    for r in &mut out {
        if let Some(d) = diff_from_raw(&r.raw, &r.opp) {
            r.my_diff = Some(d);
        }
        for b in &mut r.boards {
            fill_board_from_ggf(b, &r.opp);
        }
    }
    // Games finished before endings and clocks were kept: the wire log still has them.
    if out
        .iter()
        .any(|r| r.ending.is_empty() && r.boards.iter().all(|b| b.clocks.is_empty()))
    {
        let wire = wire_endings(&[wire_path().with_extension("log.1"), wire_path()]);
        let login = USER
            .lock()
            .unwrap()
            .as_ref()
            .map(|u| u.1.clone())
            .unwrap_or_default();
        for r in &mut out {
            if let Some(w) = wire.get(r.raw.trim()) {
                backfill_from_wire(r, w, &login);
            }
        }
    }
    out.reverse();
    out.truncate(500);
    out
}

pub(super) fn append_history(r: &GameResult) {
    if let Ok(line) = serde_json::to_string(r) {
        append_line(&history_path(), &line);
    }
}

/// Records written before the colour and the final count were kept: both come from the board's own GGF.
fn fill_board_from_ggf(b: &mut BoardRecord, opp: &str) {
    if b.ggf.is_empty() {
        return;
    }
    let Some(color) = ggf_color(&b.ggf, opp) else {
        return;
    };
    // The board's own result line, else the final position counted the same way.
    let black = ggf_tag(&b.ggf, "RE")
        .and_then(|v| v.split(':').next()?.trim().parse::<f32>().ok())
        .map(|v| v.round() as i32)
        .or_else(|| {
            let (_, g) = replay_board(b, opp)?;
            Some(scored(
                g.board.black.count_ones() as i32,
                g.board.white.count_ones() as i32,
            ))
        });
    let Some(black) = black else {
        return;
    };
    b.my_color = color.to_string();
    b.disc_diff = Some(if color == "black" { black } else { -black });
}

fn ggf_tag(ggf: &str, t: &str) -> Option<String> {
    let open = format!("{t}[");
    let i = ggf.find(&open)? + open.len();
    Some(ggf[i..].split(']').next()?.to_string())
}

fn ggf_color(ggf: &str, opp: &str) -> Option<&'static str> {
    match (ggf_tag(ggf, "PB"), ggf_tag(ggf, "PW")) {
        (_, Some(w)) if w == opp => Some("black"),
        (Some(bl), _) if bl == opp => Some("white"),
        _ => None,
    }
}

/// The player's colour and the board's final position, from its own GGF.
fn replay_board(b: &BoardRecord, opp: &str) -> Option<(&'static str, kuroobi::game::Reversi)> {
    let color = ggf_color(&b.ggf, opp)?;
    let start = ggf_tag(&b.ggf, "BO")?.strip_prefix("8 ")?.to_string();
    let game = kuroobi::game::Reversi::from_kifu_with_start(&start, &b.kifu).ok()?;
    Some((color, game))
}

#[derive(Default)]
struct WireBoard {
    over_by: String,
    out_by: String,
    /// Each player's last clock line: name, colour, main (or extension once in it), extension.
    clocks: Vec<(String, String, Option<u64>, Option<u64>)>,
    /// The server's result, from black's side.
    result: Option<f32>,
}

/// What a wire log says about each match that ended in it, by the match's end line (with its archive
/// number, so a reused match number cannot be mistaken for another).
fn wire_endings(paths: &[PathBuf]) -> HashMap<String, HashMap<String, WireBoard>> {
    let mut out = HashMap::new();
    let mut live: HashMap<String, WireBoard> = HashMap::new();
    let mut board = String::new();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            let Some(ln) = line.strip_prefix("in ") else {
                continue;
            };
            if let Some(rest) = ln
                .strip_prefix("/os: update ")
                .or_else(|| ln.strip_prefix("/os: join "))
            {
                board = rest.split_whitespace().next().unwrap_or("").to_string();
                continue;
            }
            if let Some(rest) = ln.strip_prefix('|') {
                if let Some(c) = wire_clock_line(rest) {
                    let b = live.entry(board.clone()).or_default();
                    b.clocks.retain(|x| x.0 != c.0);
                    b.clocks.push(c);
                }
                continue;
            }
            board.clear();
            if let Some((id, name, fatal)) = timeout_line(ln) {
                let b = live.entry(id.to_string()).or_default();
                if fatal {
                    b.out_by = name.to_string();
                } else if b.over_by.is_empty() {
                    b.over_by = name.to_string();
                }
            } else if let Some(rest) = ln.strip_prefix("/os: end ") {
                let (id, v, first) = end_line(rest);
                let b = live.entry(id.to_string()).or_default();
                // The first name's colour is in the clock lines.
                let first_white = b.clocks.iter().any(|c| c.0 == first && c.1 == "white");
                b.result = v.map(|v| if first_white { -v } else { v });
            } else if let Some(rest) = ln.strip_prefix("/os: + match ") {
                let id = rest.split_whitespace().next().unwrap_or("").to_string();
                live.retain(|k, _| k != &id && base_id(k) != id);
            } else if let Some(rest) = ln.strip_prefix("/os: - match ") {
                let id = rest.split_whitespace().next().unwrap_or("").to_string();
                let boards: HashMap<String, WireBoard> = live
                    .extract_if(|k, _| k == &id || base_id(k) == id)
                    .collect();
                out.insert(rest.trim().to_string(), boards);
            }
        }
    }
    out
}

/// `morgoth  (2629.3 *) 00:02,23:0//02:00,23:0`: a player's clock in an update.
fn wire_clock_line(rest: &str) -> Option<(String, String, Option<u64>, Option<u64>)> {
    let (name, tail) = rest.split_once(" (")?;
    let (inside, clock) = tail.split_once(") ")?;
    let color = match inside.split_whitespace().nth(1)? {
        "*" => "black",
        "O" => "white",
        _ => return None,
    };
    let (secs, _, ext) = parse_clock(clock.trim());
    Some((name.trim().to_string(), color.to_string(), secs, ext))
}

fn backfill_from_wire(r: &mut GameResult, wire: &HashMap<String, WireBoard>, login: &str) {
    let mut resigned = false;
    for b in &mut r.boards {
        let Some(w) = wire.get(&b.id) else {
            continue;
        };
        if b.ending.is_empty() {
            if !w.out_by.is_empty() {
                b.ending = "out_of_time".into();
                b.ending_by = w.out_by.clone();
            } else if !w.over_by.is_empty() {
                b.ending = "over_time".into();
                b.ending_by = w.over_by.clone();
            }
        }
        if b.clocks.is_empty() {
            b.clocks = w
                .clocks
                .iter()
                .map(|(n, c, secs, ext)| clock_record(n, c, *secs, *ext, &w.over_by, &w.out_by))
                .collect();
        }
        let cut_short = replay_board(b, &r.opp).is_some_and(|(_, g)| !g.is_game_over());
        if w.out_by.is_empty() && cut_short && w.result.is_some_and(|v| v.abs() >= 63.5) {
            resigned = true;
        }
    }
    if !r.ending.is_empty() {
        return;
    }
    let (kind, who) = end_kind(&r.raw);
    if kind == "adjourned" {
        r.ending = "adjourn".into();
        r.ending_by = who;
    } else if resigned {
        if let Some(d) = r.my_diff.filter(|d| *d != 0) {
            r.ending = "resign".into();
            r.ending_by = if d < 0 {
                login.to_string()
            } else {
                r.opp.clone()
            };
        }
    }
}

fn board_of_cells(cells: &[u8]) -> Option<Board> {
    if cells.len() != 64 {
        return None;
    }
    Board::from_string(&format!("{} x", cells_text(cells))).ok()
}

/// The disc difference from black's side, the empty squares going to the side ahead as the game's
/// score (and GGS) counts them.
fn scored(black: i32, white: i32) -> i32 {
    let d = black - white;
    d + d.signum() * (64 - black - white)
}

/// The server sends no clock with a timeout: the notice itself says the time is gone. Once in the extension
/// the clock's first field counts the extension down; the last stays at its setting.
fn clock_record(
    name: &str,
    color: &str,
    secs: Option<u64>,
    ext: Option<u64>,
    over_by: &str,
    out_by: &str,
) -> ClockRecord {
    let (main, ext) = if name == out_by {
        (Some(0), Some(0))
    } else if name == over_by {
        (Some(0), secs)
    } else {
        (secs, ext)
    };
    ClockRecord {
        name: name.to_string(),
        color: color.to_string(),
        main,
        ext,
    }
}

pub(super) fn board_record(id: &str, m: &MatchState) -> BoardRecord {
    let black = m.cells.iter().filter(|&&c| c == 1).count() as i32;
    let white = m.cells.iter().filter(|&&c| c == 2).count() as i32;
    // GGS's own result for the board, else the final position counted the same way.
    let d = m
        .result
        .parse::<f32>()
        .map(|v| v.round() as i32)
        .unwrap_or_else(|_| scored(black, white));
    let my_color = color_word(m.my_color);
    let (ending, ending_by) = if !m.out_of_time_by.is_empty() {
        ("out_of_time", m.out_of_time_by.clone())
    } else if !m.over_time_by.is_empty() {
        ("over_time", m.over_time_by.clone())
    } else {
        ("", String::new())
    };
    BoardRecord {
        id: id.to_string(),
        kifu: m.kifu(),
        ggf: m.ggf(id, Some(m.result.as_str()).filter(|r| !r.is_empty()), true),
        my_color: my_color.to_string(),
        disc_diff: match my_color {
            "black" => Some(d),
            "white" => Some(-d),
            _ => None,
        },
        ending: ending.to_string(),
        ending_by,
        clocks: m
            .players
            .iter()
            .map(|p| {
                clock_record(
                    &p.name,
                    &p.color,
                    p.secs,
                    p.ext,
                    &m.over_time_by,
                    &m.out_of_time_by,
                )
            })
            .collect(),
    }
}

/// GGS reports a resignation only as a ±64 result: a board that ended there although it could go on and
/// nobody ran out of time was resigned, by the side that lost it.
pub(super) fn match_ending(
    kind: &str,
    who: &str,
    boards: &[(String, MatchState)],
    my_diff: Option<i32>,
    login: &str,
    opp: &str,
) -> (String, String) {
    if kind == "adjourned" {
        return ("adjourn".into(), who.to_string());
    }
    let resigned = boards.iter().any(|(_, m)| {
        let full = m.result.trim_start_matches('+').trim_start_matches('-');
        full.starts_with("64")
            && m.out_of_time_by.is_empty()
            && board_of_cells(&m.cells).is_some_and(|b| !b.is_game_over())
    });
    match (resigned, my_diff) {
        (true, Some(d)) if d < 0 => ("resign".into(), login.to_string()),
        (true, Some(d)) if d > 0 => ("resign".into(), opp.to_string()),
        _ => (String::new(), String::new()),
    }
}

/// `/os: timeout .7.0 name` (main time used up) or `/os: fatal-timeout .7.0 name` (the extension too).
pub(super) fn timeout_line(ln: &str) -> Option<(&str, &str, bool)> {
    let (rest, fatal) = ln
        .strip_prefix("/os: fatal-timeout ")
        .map(|r| (r, true))
        .or_else(|| ln.strip_prefix("/os: timeout ").map(|r| (r, false)))?;
    let mut it = rest.split_whitespace();
    Some((it.next()?, it.next()?, fatal))
}

/// The rest of `/os: end .7.0 (first second) score`: the board, its score and the name it is scored for,
/// which is not always black's.
pub(super) fn end_line(rest: &str) -> (&str, Option<f32>, &str) {
    let id = rest.split_whitespace().next().unwrap_or("");
    let score = rest.rsplit(' ').next().and_then(|v| v.trim().parse().ok());
    let first = rest
        .split('(')
        .nth(1)
        .and_then(|t| t.split_whitespace().next())
        .unwrap_or("");
    (id, score, first)
}

/// A match line's score and the name it is signed for (the one written first).
pub(super) fn match_score(line: &str) -> (Option<f32>, Option<&str>) {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let score = toks
        .iter()
        .find_map(|t| t.starts_with(['+', '-']).then(|| t.parse().ok()).flatten());
    let first = toks
        .iter()
        .skip(1)
        .find(|t| t.len() >= 2 && t.starts_with(|c: char| c.is_ascii_alphabetic()))
        .copied();
    (score, first)
}

pub(super) fn end_kind(rest: &str) -> (&'static str, String) {
    if rest.contains(" aborted") {
        return ("aborted", String::new());
    }
    let toks: Vec<&str> = rest.split_whitespace().collect();
    if let Some(i) = toks.iter().position(|t| *t == "left") {
        let who = i
            .checked_sub(1)
            .map(|j| toks[j].to_string())
            .unwrap_or_default();
        return ("adjourned", who);
    }
    ("finished", String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The .46.0 board of a real match: morgoth's extension ran out with four squares empty, 25 to 35.
    #[test]
    fn an_older_record_gets_its_colour_and_result_from_its_ggf() {
        let mut b = BoardRecord {
            id: ".46.0".into(),
            kifu: "f5b3c3b5d7c7b6e8a6e7f7b4a3g5g4a4a5g6h5h6e2h4h3h2g7e1g8c2f8f2c1b2g3b1d2d1a1h8h7d8h1g2f1g1b8c8".into(),
            ggf: "(;GM[Othello]PC[GGS/os]ID[.46.0]PB[morgoth]RB[2629.3]PW[kuroobi]RW[2602.3]RE[-64.00]\
                  BO[8 -------------------*O*----**O*----OOO-----*OO*------------------ *];)"
                .into(),
            ..Default::default()
        };
        fill_board_from_ggf(&mut b, "morgoth");
        assert_eq!(
            (b.my_color.as_str(), b.disc_diff),
            ("white", Some(64)),
            "the board's own result"
        );
        b.ggf = b.ggf.replace("RE[-64.00]", "");
        b.disc_diff = None;
        fill_board_from_ggf(&mut b, "morgoth");
        assert_eq!(b.disc_diff, Some(-14), "without one, the final position");
    }

    // The end of the real .46 match (morgoth's extension ran out on both boards), as the wire log has it.
    #[test]
    fn a_past_result_gets_its_endings_and_clocks_from_the_wire_log() {
        let log = "in /os: + match .46 2629 morgoth 2602 kuroobi s8r14 R
in /os: update .46.1 s8r14 K?
in | 45: A8/-6.00/0.25
in |kuroobi  (2602.3 *) 00:04,23:0//02:00,23:0
in |morgoth  (2629.3 O) 00:03,22:0//02:00,22:0
in |
in /os: update .46.0 s8r14 K?
in |morgoth  (2629.3 *) 00:02,23:0//02:00,23:0
in |kuroobi  (2602.3 O) 00:04,23:0//02:00,23:0
in /os: timeout .46.1 morgoth (O)
in /os: fatal-timeout .46.1 morgoth (O)
in /os: end .46.1 ( morgoth vs. kuroobi ) -64.00
in /os: timeout .46.0 morgoth (*)
in /os: fatal-timeout .46.0 morgoth (*)
in /os: end .46.0 ( morgoth vs. kuroobi ) -64.00
in /os: - match .46 2612 morgoth 2619 kuroobi s8r14 R -64.00  .90661
";
        let path = std::env::temp_dir().join(format!("kuroobi-wire-{}.log", std::process::id()));
        std::fs::write(&path, log).unwrap();
        let wire = wire_endings(std::slice::from_ref(&path));
        let _ = std::fs::remove_file(&path);
        let raw = ".46 2612 morgoth 2619 kuroobi s8r14 R -64.00  .90661";
        let mut r = GameResult {
            raw: raw.into(),
            opp: "morgoth".into(),
            my_diff: Some(64),
            boards: vec![
                BoardRecord {
                    id: ".46.0".into(),
                    ..Default::default()
                },
                BoardRecord {
                    id: ".46.1".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        backfill_from_wire(&mut r, &wire[raw], "kuroobi");
        assert_eq!(r.ending, "", "a timeout is no resignation");
        for b in &r.boards {
            assert_eq!(
                (b.ending.as_str(), b.ending_by.as_str()),
                ("out_of_time", "morgoth")
            );
            let c = |n: &str| {
                b.clocks
                    .iter()
                    .find(|c| c.name == n)
                    .map(|c| (c.main, c.ext))
            };
            assert_eq!(c("morgoth"), Some((Some(0), Some(0))));
            assert_eq!(c("kuroobi"), Some((Some(4), Some(120))));
        }
    }

    #[test]
    fn a_board_records_who_ran_out_of_main_time_and_of_the_extension() {
        let mut m = MatchState::new();
        m.my_color = Some('O');
        m.over_time_by = "morgoth".into();
        let r = board_record(".46.1", &m);
        assert_eq!(
            (r.ending.as_str(), r.ending_by.as_str()),
            ("over_time", "morgoth")
        );
        m.out_of_time_by = "morgoth".into();
        m.players = vec![PlayerView {
            name: "morgoth".into(),
            secs: Some(10),
            ext: Some(5),
            ..Default::default()
        }];
        let r = board_record(".46.1", &m);
        assert_eq!(r.ending, "out_of_time");
        assert_eq!((r.clocks[0].main, r.clocks[0].ext), (Some(0), Some(0)));
    }

    // Seen on the local server: `00:42,1:0//01:00,1:0` after moving 13 seconds into a one-minute extension.
    #[test]
    fn a_clock_in_the_extension_counts_it_down_in_its_first_field() {
        let mut m = MatchState::new();
        m.over_time_by = "bob".into();
        let (secs, _, ext) = parse_clock("00:42,1:0//01:00,1:0");
        m.players = vec![PlayerView {
            name: "bob".into(),
            secs,
            ext,
            ..Default::default()
        }];
        let c = &board_record(".0", &m).clocks[0];
        assert_eq!((c.main, c.ext), (Some(0), Some(42)));
    }

    #[test]
    fn the_empty_squares_go_to_the_side_ahead() {
        assert_eq!(scored(33, 30), 4);
        assert_eq!(scored(30, 30), 0);
        assert_eq!(scored(25, 35), -14);
    }

    #[test]
    fn a_64_on_an_unfinished_board_without_a_timeout_is_a_resignation() {
        let mut m = MatchState::new();
        m.cells = vec![0; 64];
        for (sq, c) in [(27, 2), (36, 2), (28, 1), (35, 1)] {
            m.cells[sq] = c;
        }
        m.result = "-64.00".into();
        let boards = vec![(".9.0".to_string(), m.clone())];
        assert_eq!(
            match_ending("finished", "", &boards, Some(-64), "me", "you"),
            ("resign".into(), "me".into())
        );
        assert_eq!(
            match_ending("finished", "", &boards, Some(64), "me", "you").1,
            "you"
        );
        let mut t = m;
        t.out_of_time_by = "you".into();
        assert_eq!(
            match_ending("finished", "", &[(".9.0".into(), t)], Some(64), "me", "you").0,
            ""
        );
        assert_eq!(
            match_ending("adjourned", "you", &[], None, "me", "you"),
            ("adjourn".into(), "you".into())
        );
    }
}
