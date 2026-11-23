//! A FlipSight bot: accepts standard 8x8 matches and plays every leg with its own engine worker.

pub mod api;
pub mod manager;
mod pause;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kuroobi::engine::{EngineAssets, EngineConfig};
use kuroobi::{Board, Position};
use play::{Done, EngineWorker, Job};
use serde::Serialize;
use serde_json::{json, Map, Value};

use api::{Api, Leg, Match, Reply, Wire};

pub struct Options {
    pub base: String,
    pub token: String,
    /// Threads for each leg's engine, as the site counts them; a match's two legs use twice that.
    pub threads: usize,
    /// Folder holding `linear.bin`, `nnue.bin` and `book.txt`; `None` keeps `resources.conf`'s.
    pub weights: Option<PathBuf>,
    pub use_book: bool,
    /// Seconds kept back from the clock on every move.
    pub margin: f64,
    /// First guess of the seconds the site's clock loses on a move outside the search; then measured.
    pub lag: f64,
    pub practice: bool,
    /// Search the opponent's position while it thinks, on the clock.
    pub ponder: bool,
    /// Learn each finished game into the book (`book_learn.txt`).
    pub learn: bool,
    /// Process names (`pgrep -x` pattern) of background jobs stopped while a match runs; empty leaves them.
    pub pause: String,
    pub pause_log: PathBuf,
    /// A GGS wire log; while it shows a live match the paused jobs stay paused.
    pub ggs_wire: PathBuf,
    pub out: PathBuf,
    pub challenge: Challenge,
    /// `(n, points)`: stop (as on a first Ctrl-C) once the last `n` rating changes moved the
    /// rating by no more than `points` in all.
    pub until_stable: Option<(usize, f64)>,
}

/// Whom to challenge, one match at a time, between the server's own matches.
#[derive(Clone, Default, PartialEq)]
pub enum Challenge {
    #[default]
    None,
    /// The site's built-in bots, weakest first.
    Builtin,
    Ids(Vec<i64>),
    /// Every online bot rated above us, weakest first, over and over.
    Stronger,
}

impl Challenge {
    /// The name the screen and the saved settings use.
    pub fn key(&self) -> &'static str {
        match self {
            Challenge::None => "none",
            Challenge::Builtin => "builtin",
            Challenge::Stronger => "stronger",
            Challenge::Ids(_) => "ids",
        }
    }

    pub fn from_key(k: &str) -> Challenge {
        match k {
            "builtin" => Challenge::Builtin,
            "stronger" => Challenge::Stronger,
            _ => Challenge::None,
        }
    }
}

/// What may change while the bot runs.
#[derive(Clone)]
pub struct Control {
    /// Take challenges at all.
    pub accept: bool,
    pub challenge: Challenge,
    /// Matches at once this side takes; `None` is the bot's own setting on the site.
    pub cap: Option<usize>,
    /// Threads for each leg's engine, as the site counts them.
    pub threads: usize,
    pub use_book: bool,
    /// Depth and solve to play every move at; `None` plays by the clock. A leg keeps the
    /// setting it started with.
    pub level: Option<(u32, u8)>,
    /// One bot to challenge now, taken once sent.
    pub challenge_now: Option<i64>,
    /// Midgame and endgame transposition table bits; an engine keeps the tables it was built
    /// with until no match is running.
    pub hash: (u32, u32),
    /// Search the opponent's position while it thinks; on the clock only.
    pub ponder: bool,
    /// Learn each finished game into the book.
    pub learn: bool,
    pub book_tolerance: f32,
}

/// What the bot shows of itself.
#[derive(Clone, Default, Serialize)]
pub struct BotView {
    pub id: i64,
    pub name: String,
    pub photo: Option<String>,
    pub rating: Option<f64>,
    /// The site's matches at once (`max_games`).
    pub max_games: Option<usize>,
    /// connecting | online | draining (finishing its matches, then stopping) | retrying | refused | stopped
    pub conn: String,
    pub matches: Vec<LiveMatch>,
    pub outstanding: Option<Outstanding>,
    /// Bots that turned a challenge down and are left alone for now.
    pub cooling: Vec<i64>,
    pub bots: Vec<OtherBot>,
    /// Moves on every time a match is written to the results.
    pub finished: u64,
}

#[derive(Clone, Default, Serialize)]
pub struct Outstanding {
    pub target: i64,
    pub name: String,
    pub since_ms: i64,
}

#[derive(Clone, Default, Serialize)]
pub struct OtherBot {
    pub id: i64,
    pub name: String,
    pub rating: Option<f64>,
    /// `None`: the site does not say (it names only the bots we have played, in their records).
    pub online: Option<bool>,
    pub playing: bool,
    /// The owner paused its matches on the site: `match_paused`, in the site's list but not the bot API.
    pub paused: bool,
    pub builtin: bool,
    /// The bot's picture on the site, as a full address.
    pub photo: Option<String>,
}

#[derive(Clone, Default, Serialize)]
pub struct LiveMatch {
    pub id: i64,
    /// bot: two legs from an opening; human: one game from the start position.
    pub kind: String,
    pub opponent: i64,
    pub opponent_name: String,
    pub opponent_rating: Option<f64>,
    pub legs: Vec<LiveLeg>,
}

#[derive(Clone, Default, Serialize)]
pub struct LiveLeg {
    /// a1 = 0, b1 = 1, ...; 1 black, 2 white.
    pub cells: Vec<u8>,
    pub turn: u8,
    pub my_color: u8,
    /// Black's and white's time left as of the last look at the state.
    pub clocks_ms: Vec<f64>,
    pub discs: Vec<i64>,
    pub last: Option<i64>,
    pub ply: usize,
    pub active: bool,
    pub thinking: bool,
    /// Our moves: (move number, value, seconds).
    pub evals: Vec<(usize, f32, f32)>,
    /// The opponent's moves: (move number, seconds), from its clock.
    pub opp_secs: Vec<(usize, f32)>,
    /// The opponent's moves: (move number, the value it sent, from its side).
    pub opp_evals: Vec<(usize, f32)>,
    /// The moves so far (`f5d6...`), when known from the first.
    pub moves: String,
}

/// One leg as followed state by state, to keep its moves.
#[derive(Default)]
struct LegHist {
    cells: Vec<u8>,
    turn: u8,
    moves: Vec<i64>,
    /// Seen from the first move on, so `moves` is the whole game.
    complete: bool,
    /// Our moves: (move number, value, seconds); the seconds are NaN for a move we did not see
    /// ourselves search (the site's record of it, after a restart).
    evals: Vec<(usize, f32, f32)>,
    /// The opponent's moves: (move number, seconds).
    opp_secs: Vec<(usize, f32)>,
    /// The opponent's moves: (move number, the value it sent, from its side).
    opp_evals: Vec<(usize, f32)>,
    /// The opponent's clock when its turn began.
    opp_clock: Option<f64>,
}

fn record_key(r: &Value) -> (String, i64) {
    (
        r["kind"].as_str().unwrap_or("").to_string(),
        r["id"].as_i64().unwrap_or(0),
    )
}

/// The opponent of one of the site's records: the stream names no bot but ourselves, so the bots
/// we know are the ones we have played.
fn learn_opponent(names: &mut HashMap<i64, OtherBot>, r: &Value, base: &str) {
    let o = &r["opponent"];
    let (Some(id), Some(name)) = (o["bot_id"].as_i64(), o["name"].as_str()) else {
        return;
    };
    let b = names.entry(id).or_default();
    b.id = id;
    b.name = name.to_string();
    b.builtin = o["builtin"] == Value::Bool(true);
    if let Some(r) = o["rating_after"].as_f64().or(o["rating_before"].as_f64()) {
        b.rating = Some(r);
    }
    // Where the site's own pages take a bot's picture from.
    b.photo.get_or_insert_with(|| {
        let base = base.trim_end_matches('/');
        format!("{base}/flipsight/bot_match_api.php?action=photo&bot_id={id}")
    });
}

fn json_lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The standard bots of the site's list of bots, by id.
fn bots_of(list: &Value, base: &str) -> HashMap<i64, OtherBot> {
    let mut names = HashMap::new();
    for b in list.as_array().into_iter().flatten() {
        let Some(id) = b["id"].as_i64() else { continue };
        let std = b["supported_games"]
            .as_array()
            .is_none_or(|g| g.iter().any(|x| x == "std"));
        if !std || b["deleted"] == Value::Bool(true) || b["disabled"] == Value::Bool(true) {
            continue;
        }
        names.insert(
            id,
            OtherBot {
                id,
                name: b["name"].as_str().unwrap_or("").to_string(),
                rating: b["rating"]["rating"].as_f64(),
                online: Some(b["online"] == Value::Bool(true)),
                playing: b["games_now"].as_u64().is_some_and(|n| n > 0),
                paused: b["match_paused"] == Value::Bool(true),
                builtin: b["is_builtin"] == Value::Bool(true),
                photo: photo_url(base, &b["photo"]),
            },
        );
    }
    names
}

/// What a bot that is not running shows of itself: from the site's list of bots when it has
/// been read, else from the records it keeps (its last rating and the bots it has played).
pub fn offline_view(
    dir: &std::path::Path,
    base: &str,
    me: &OtherBot,
    roster: Option<&Value>,
) -> BotView {
    let mut names = HashMap::new();
    let mut rating = None;
    for r in json_lines(&dir.join("games.jsonl")) {
        learn_opponent(&mut names, &r, base);
        rating = r["me"]["rating_after"].as_f64().or(rating);
    }
    if let Some(list) = roster {
        names = bots_of(list, base);
        rating = names.get(&me.id).and_then(|b| b.rating).or(rating);
    }
    let listed = names.remove(&me.id);
    names.insert(
        me.id,
        OtherBot {
            rating,
            photo: me.photo.clone().or(listed.and_then(|b| b.photo)),
            ..me.clone()
        },
    );
    let mut bots: Vec<OtherBot> = names.into_values().collect();
    bots.sort_by(|a, b| b.rating.unwrap_or(0.0).total_cmp(&a.rating.unwrap_or(0.0)));
    BotView {
        id: me.id,
        name: me.name.clone(),
        photo: me.photo.clone(),
        rating,
        conn: "off".into(),
        bots,
        finished: json_lines(&dir.join("results.jsonl")).len() as u64,
        ..Default::default()
    }
}

/// For a bot that is not running: reads the site's records since the last one kept, and writes
/// a result for each match that ended after the last result here (played while this program
/// was not connected). Returns the results written.
pub fn sync_records(dir: &std::path::Path, api: &Api) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut kept: HashSet<(String, i64)> = HashSet::new();
    let mut until = 0;
    for r in json_lines(&dir.join("games.jsonl")) {
        kept.insert(record_key(&r));
        until = until.max(r["updated_ms"].as_i64().unwrap_or(0));
    }
    let results = json_lines(&dir.join("results.jsonl"));
    let mut finished: HashSet<i64> = results.iter().filter_map(|r| r["match"].as_i64()).collect();
    let last = results
        .iter()
        .filter_map(|r| r["t"].as_i64())
        .max()
        .unwrap_or(0);
    let open = |name: &str| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(name))
            .map_err(|e| e.to_string())
    };
    let (mut games, mut out) = (open("games.jsonl")?, open("results.jsonl")?);
    let mut written = 0;
    for page in 0..SYNC_PAGES {
        if page > 0 {
            std::thread::sleep(RECORDS_APART);
        }
        let mut new = 0;
        for r in api.games(until)? {
            until = until.max(r["updated_ms"].as_i64().unwrap_or(0));
            if !kept.insert(record_key(&r)) {
                continue;
            }
            new += 1;
            let _ = writeln!(games, "{r}");
            let id = r["id"].as_i64().unwrap_or(0);
            if r["kind"] == "bot"
                && r["finished_ms"].as_i64().unwrap_or(0) > last
                && finished.insert(id)
            {
                let _ = writeln!(out, "{}", result_of_record(&r));
                written += 1;
            }
        }
        if new == 0 {
            break;
        }
    }
    Ok(written)
}

/// How often the public list of bots is read: their ratings, who is online and who is playing.
/// The site's own bot page reads it at most every 5 minutes besides its socket's changes.
const ROSTER_EVERY: Duration = Duration::from_secs(60);

/// The site answers 429 to a second call for records within a few seconds.
const RECORDS_APART: Duration = Duration::from_secs(10);
const SYNC_PAGES: usize = 6;

/// A line of results.jsonl from the site's record, for a match whose end we did not see.
fn result_of_record(r: &Value) -> Value {
    let opening: Vec<&str> = chunks2(r["opening"].as_str().unwrap_or(""));
    let (mut mine, mut theirs) = (0, 0);
    let legs: Vec<Value> = r["legs"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let black = l["my_color"] == "B";
            let me = if black { 'B' } else { 'W' };
            let discs = if l["result_discs"].is_array() {
                &l["result_discs"]
            } else {
                &l["discs"]
            };
            let (b, w) = (
                discs[0].as_i64().unwrap_or(0),
                discs[1].as_i64().unwrap_or(0),
            );
            mine += if black { b } else { w };
            theirs += if black { w } else { b };
            let played = chunks2(l["moves"].as_str().unwrap_or(""));
            let colors: Vec<char> = l["colors"].as_str().unwrap_or("").chars().collect();
            let mut moves: String = opening.concat();
            let mut ply = opening.iter().filter(|m| **m != "pa").count();
            let (mut evals, mut opp_evals, mut opp_secs) = (Vec::new(), Vec::new(), Vec::new());
            for (j, m) in played.iter().enumerate() {
                if *m == "pa" {
                    continue;
                }
                ply += 1;
                moves.push_str(m);
                let secs = l["used_ms"][j].as_f64().map(|ms| ms / 1000.0);
                let eval = l["evals"][j].as_f64();
                if colors.get(j) == Some(&me) {
                    if let Some(e) = eval {
                        evals.push(json!([ply, e, secs]));
                    }
                } else {
                    if let Some(e) = eval {
                        opp_evals.push(json!([ply, e]));
                    }
                    if let Some(s) = secs {
                        opp_secs.push(json!([ply, s]));
                    }
                }
            }
            let end = final_board(&moves);
            json!({"black": black, "discs": l["discs"], "result_discs": l["result_discs"],
                   "status": l["status"], "moves": moves,
                   "cells": end.as_ref().map(|e| e.0.clone()), "last": end.and_then(|e| e.1),
                   "evals": evals, "opp_evals": opp_evals, "opp_secs": opp_secs})
        })
        .collect();
    let result = match r["result"].as_str().unwrap_or("") {
        x @ ("win" | "loss" | "draw") => x,
        _ => "aborted",
    };
    json!({
        "t": r["finished_ms"], "match": r["id"], "kind": r["kind"],
        "opponent": r["opponent"]["bot_id"], "status": r["status"], "result": result,
        "opponent_name": r["opponent"]["name"], "opponent_rating": r["opponent"]["rating_before"],
        "my_rating": r["me"]["rating_after"], "my_rating_before": r["me"]["rating_before"],
        "mine": mine, "theirs": theirs, "diff": mine - theirs,
        "finish_reason": r["finish_reason"], "legs": legs, "record": r,
    })
}

/// A game against a person has no winner or totals in its state, so a result built from the
/// state alone reads as a 0-0 draw; its record says how it ended, a resignation included
/// ([4, 4] can be a win). Results written before this are mended as they are read.
pub fn settled_by_record(mut x: Value) -> Value {
    let r = x["record"].clone();
    if r["kind"] != "human" {
        return x;
    }
    let result = r["result"]
        .as_str()
        .filter(|v| ["win", "loss", "draw"].contains(v))
        .unwrap_or("aborted");
    let (mine, theirs) = (r["discs"][0].as_i64(), r["discs"][1].as_i64());
    let reason = r["finish_reason"].clone();
    let name = r["opponent"]["name"].clone();
    x["kind"] = json!("human");
    x["result"] = json!(result);
    if let (Some(a), Some(b)) = (mine, theirs) {
        x["mine"] = json!(a);
        x["theirs"] = json!(b);
        x["diff"] = json!(a - b);
    }
    x["finish_reason"] = reason;
    if x["opponent_name"].is_null() {
        x["opponent_name"] = name;
    }
    x
}

/// Games against people come as matches with a negative id (`-room_id`).
fn kind_of(m: &Match) -> &str {
    match m.kind.as_deref() {
        Some(k) => k,
        None if m.id < 0 => "human",
        None => "bot",
    }
}

/// The site's notation: two characters a move, `pa` for a pass.
fn chunks2(s: &str) -> Vec<&str> {
    (0..s.len() / 2)
        .filter_map(|i| s.get(i * 2..i * 2 + 2))
        .collect()
}

/// A bot's `photo` is a path on the site; anything else (another host) is not shown.
fn photo_url(base: &str, photo: &Value) -> Option<String> {
    let p = photo.as_str()?;
    (p.starts_with('/') && !p.starts_with("//"))
        .then(|| format!("{}{p}", base.trim_end_matches('/')))
}

fn cells_of_board(b: &Board) -> Vec<u8> {
    format!("{b}")
        .chars()
        .take(64)
        .map(|c| match c {
            'X' => 1,
            'O' => 2,
            _ => 0,
        })
        .collect()
}

/// The board a leg ended on and its last square, from its moves played from the start position;
/// none when the moves stop short of the end.
pub fn final_board(moves: &str) -> Option<(Vec<u8>, Option<usize>)> {
    let mut b = Board::new();
    let mut last = None;
    for m in moves.as_bytes().chunks(2) {
        let sq = std::str::from_utf8(m).ok()?;
        let pos = Position::from_kifu(sq).ok()?;
        if b.make_move(pos).is_err() {
            b.pass();
            b.make_move(pos).ok()?;
        }
        last = Some(index_of(pos) as usize);
    }
    b.is_game_over().then(|| (cells_of_board(&b), last))
}

/// The moves that turned one board into the next: the newly filled squares, in an order that
/// replays to it (a side with no move at a square passes). Several moves can come at once, most
/// often at the end of a game.
fn infer_moves(prev: &[u8], turn: u8, now: &[u8]) -> Option<Vec<usize>> {
    let new: Vec<usize> = (0..64).filter(|&i| prev[i] == 0 && now[i] != 0).collect();
    if new.len() > MAX_INFERRED {
        return None;
    }
    let leg = Leg {
        board: prev.to_vec(),
        turn,
        ..Default::default()
    };
    let mut seq = Vec::with_capacity(new.len());
    find_order(board_of(&leg)?, &new, &mut seq, now).then_some(seq)
}

/// Squares filled between two states that are still searched for an order.
const MAX_INFERRED: usize = 12;

fn find_order(b: Board, left: &[usize], seq: &mut Vec<usize>, now: &[u8]) -> bool {
    if left.is_empty() {
        return cells_of_board(&b) == now;
    }
    for (k, &sq) in left.iter().enumerate() {
        let Ok(pos) = Position::from_kifu(&coord(sq as i64)) else {
            return false;
        };
        let mut next = b;
        if next.make_move(pos).is_err() {
            next.pass();
            if next.make_move(pos).is_err() {
                continue;
            }
        }
        let rest: Vec<usize> = left
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != k)
            .map(|(_, &q)| q)
            .collect();
        seq.push(sq);
        if find_order(next, &rest, seq, now) {
            return true;
        }
        seq.pop();
    }
    false
}

/// The engine settings a bot runs with, from the shared resources.
pub fn engine_config(threads: usize, use_book: bool) -> EngineConfig {
    let res = ggs::env::resources();
    EngineConfig {
        threads,
        midgame_hash_bits: res.hash_mid_bits(),
        solver_hash_bits: res.hash_end_bits(),
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: res.book_path(),
        use_book,
        ..Default::default()
    }
}

/// The engines start before the first match: loading one costs seconds of the first move.
const WARM_WORKERS: usize = 2;

/// The server lets a challenge stand for 120 s.
const CHALLENGE_WAIT: Duration = Duration::from_secs(125);
const COOL_DOWN: Duration = Duration::from_secs(600);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Local wall-clock time, `HH:MM:SS`.
fn clock_text() -> String {
    let t = (now_ms() / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn log(msg: &str) {
    println!("{} {msg}", clock_text());
}

/// The site writes ratings as strings ("2463.4217") or numbers.
fn rating_of(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse().ok())
}

fn coord(i: i64) -> String {
    if i < 0 {
        return "pass".into();
    }
    format!("{}{}", (b'a' + (i % 8) as u8) as char, i / 8 + 1)
}

fn index_of(p: Position) -> i64 {
    let k = p.to_kifu();
    let b = k.as_bytes();
    i64::from(b[1] - b'1') * 8 + i64::from(b[0] - b'a')
}

/// The server's board (index a1 = 0, b1 = 1, ...; 1 black, 2 white) with `turn` to move.
fn board_of(leg: &Leg) -> Option<Board> {
    if leg.board.len() != 64 {
        return None;
    }
    let mut s: String = leg
        .board
        .iter()
        .map(|c| match c {
            1 => 'X',
            2 => 'O',
            _ => '-',
        })
        .collect();
    s.push(if leg.turn == 1 { 'X' } else { 'O' });
    Board::from_string(&s).ok()
}

enum Asked {
    Yes,
    Busy,
    Refused,
    Failed,
}

/// The least the site's clock is taken to lose on a move outside the search. With the state from
/// the stream every one of a match's 51 moves lost 0.18 to 0.44 s (polling lost 0.85 at the
/// median and 1.44 at the 99th percentile, and this was 1.0).
const MOVE_COST: f64 = 0.5;

/// The seconds a search may take with `left` on the site's clock and `empties` squares to fill.
fn budget(left: f64, margin: f64, lag: f64, empties: u8) -> f64 {
    let later = f64::from(empties.div_ceil(2)).max(1.0) - 1.0;
    (left - margin - MOVE_COST.max(1.1 * lag) * later).max(0.05)
}

/// A search sent for one leg at one revision.
struct Sent {
    key: (i64, usize),
    revision: i64,
    color: u8,
    match_id: i64,
    human: Option<(Option<i64>, Option<String>)>,
    at: Instant,
    left: f64,
    budget: f64,
    empties: u32,
    threads: usize,
}

struct Slot {
    worker: EngineWorker,
    sent: Option<Sent>,
    /// The midgame and endgame table bits its engine was built with.
    hash: (u32, u32),
    /// The revision of the leg it is pondering, the opponent to move.
    pondering: Option<i64>,
}

impl Drop for Bot {
    fn drop(&mut self) {
        play::learner().hold(&self.learn_hold(), false);
    }
}

pub struct Bot {
    o: Options,
    api: Api,
    me: i64,
    engine_cfg: EngineConfig,
    assets: EngineAssets,
    slots: Vec<Slot>,
    /// Which slot serves which (match, leg).
    owner: HashMap<(i64, usize), usize>,
    answered: HashSet<(i64, usize, i64)>,
    finished: HashSet<i64>,
    lag: f64,
    /// Each leg's clock when its last search began, and the seconds that search took.
    spent: HashMap<(i64, usize), (f64, f64)>,
    /// Challenges answered (true: taken) that the stream still shows as waiting.
    decided: HashMap<i64, bool>,
    world: World,
    /// The engine details the site was last told.
    env_sent: Option<Value>,
    /// Bots still to challenge, once the list is known.
    to_challenge: Option<std::collections::VecDeque<(i64, String)>>,
    /// Set from outside: take no new match, and stop once the running ones are over.
    drain: Arc<AtomicBool>,
    draining_logged: bool,
    /// Our rating after each change, oldest first.
    ratings: Vec<f64>,
    last_challenge: Option<Instant>,
    /// Our challenge waiting for an answer: match id, target id, when sent.
    outstanding: Option<(i64, i64, Instant)>,
    /// Targets that turned a challenge down, and until when they are left alone.
    cooling: HashMap<i64, Instant>,
    pauser: pause::Pauser,
    moves: std::fs::File,
    results: std::fs::File,
    /// The site's own records of our games, as it sends them.
    games: std::fs::File,
    /// The records already kept, as (kind, id), and the newest one's time.
    kept: HashSet<(String, i64)>,
    kept_until: i64,
    /// Matches that ended while the stream was down: written from the site's record of them.
    lost: HashSet<i64>,
    /// When to ask the site for the records since the last one kept; `None` once read.
    catch_up_at: Option<Instant>,
    ctl: Arc<Mutex<Control>>,
    view: Arc<Mutex<BotView>>,
    hist: HashMap<(i64, usize), LegHist>,
    challenge_key: String,
    max_games: Option<usize>,
    /// Every standard bot the site has told us of.
    names: HashMap<i64, OtherBot>,
    cpu: Option<String>,
}

impl Bot {
    pub fn new(o: Options) -> Result<Bot, String> {
        let mut engine_cfg = engine_config(o.threads, o.use_book);
        if let Some(d) = &o.weights {
            engine_cfg.weights = d.join("linear.bin");
            engine_cfg.nnue = d.join("nnue.bin");
            engine_cfg.book = d.join("book.txt");
        }
        let assets = EngineAssets::load(&engine_cfg)?;
        Bot::with_assets(o, engine_cfg, assets)
    }

    /// As [`Bot::new`], on weights already loaded: several bots share one copy.
    pub fn with_assets(
        o: Options,
        engine_cfg: EngineConfig,
        assets: EngineAssets,
    ) -> Result<Bot, String> {
        std::fs::create_dir_all(&o.out).map_err(|e| e.to_string())?;
        let open = |name: &str| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(o.out.join(name))
                .map_err(|e| e.to_string())
        };
        let moves = open("moves.jsonl")?;
        let results = open("results.jsonl")?;
        // The server keeps listing finished matches; those already written are not written again.
        let finished: HashSet<i64> = std::fs::read_to_string(o.out.join("results.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok()?["match"].as_i64())
            .collect();
        let games = open("games.jsonl")?;
        let mut kept = HashSet::new();
        let mut kept_until = 0;
        let mut names = HashMap::new();
        for r in std::fs::read_to_string(o.out.join("games.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        {
            kept.insert(record_key(&r));
            kept_until = kept_until.max(r["updated_ms"].as_i64().unwrap_or(0));
            learn_opponent(&mut names, &r, &o.base);
        }
        let began = Instant::now();
        let mut slots = Vec::new();
        for _ in 0..WARM_WORKERS {
            slots.push(Slot {
                worker: EngineWorker::spawn_with(assets.clone(), engine_cfg.clone(), false)?,
                sent: None,
                hash: (engine_cfg.midgame_hash_bits, engine_cfg.solver_hash_bits),
                pondering: None,
            });
        }
        log(&format!(
            "{WARM_WORKERS} engines ready in {:.1}s",
            began.elapsed().as_secs_f64()
        ));
        let ctl = Control {
            accept: true,
            challenge: o.challenge.clone(),
            cap: None,
            threads: o.threads,
            use_book: o.use_book,
            level: None,
            challenge_now: None,
            hash: (engine_cfg.midgame_hash_bits, engine_cfg.solver_hash_bits),
            ponder: o.ponder,
            learn: o.learn,
            book_tolerance: engine_cfg.book_tolerance,
        };
        let cpu = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        Ok(Bot {
            ctl: Arc::new(Mutex::new(ctl)),
            view: Arc::new(Mutex::new(BotView {
                conn: "connecting".into(),
                ..Default::default()
            })),
            hist: HashMap::new(),
            challenge_key: String::new(),
            max_games: None,
            names,
            games,
            kept,
            kept_until,
            catch_up_at: Some(Instant::now()),
            lost: HashSet::new(),
            cpu,
            api: Api::new(&o.base, o.token.clone()),
            pauser: pause::Pauser::new(&o.pause, o.pause_log.clone(), o.ggs_wire.clone()),
            lag: o.lag,
            spent: HashMap::new(),
            decided: HashMap::new(),
            world: World::default(),
            env_sent: None,
            to_challenge: None,
            drain: Arc::new(AtomicBool::new(false)),
            draining_logged: false,
            ratings: Vec::new(),
            last_challenge: None,
            outstanding: None,
            cooling: HashMap::new(),
            o,
            me: 0,
            engine_cfg,
            assets,
            slots,
            owner: HashMap::new(),
            answered: HashSet::new(),
            finished,
            moves,
            results,
        })
    }

    /// The switches the bot reads every time it acts.
    pub fn control(&self) -> Arc<Mutex<Control>> {
        self.ctl.clone()
    }

    /// What the bot shows of itself.
    pub fn view(&self) -> Arc<Mutex<BotView>> {
        self.view.clone()
    }

    fn set_conn(&self, conn: &str) {
        self.view.lock().unwrap().conn = conn.into();
    }

    fn environment(&self) -> Value {
        let ctl = self.ctl.lock().unwrap().clone();
        json!({
            "cpu": self.cpu,
            "engine": "KUROOBI",
            "os": std::env::consts::OS,
            "engine_threads": ctl.threads,
            // One leg's: a match's two legs each have their own engine and tables.
            "hash_mb": self.tables_of(ctl.hash).table_mb(ctl.level.is_none()).round(),
            "ponder": ctl.ponder && ctl.level.is_none(),
            "practice": self.o.practice,
        })
    }

    /// The engine settings with the table sizes `hash`.
    fn tables_of(&self, hash: (u32, u32)) -> EngineConfig {
        EngineConfig {
            midgame_hash_bits: hash.0,
            solver_hash_bits: hash.1,
            ..self.engine_cfg.clone()
        }
    }

    /// Rebuilds the engines whose tables are not the size now set, while no match runs: building
    /// one allocates its tables, which inside a match would take a move's time.
    fn refit_engines(&mut self, hash: (u32, u32)) {
        let cfg = self.tables_of(hash);
        for slot in &mut self.slots {
            if slot.hash == hash || slot.worker.busy {
                continue;
            }
            match EngineWorker::spawn_with(self.assets.clone(), cfg.clone(), false) {
                Ok(w) => {
                    slot.worker = w;
                    slot.sent = None;
                    slot.hash = hash;
                    slot.pondering = None;
                }
                Err(e) => log(&format!("engine not rebuilt: {e}")),
            }
        }
    }

    /// The flag that, once set, makes the bot finish its matches and stop.
    pub fn drain_flag(&self) -> Arc<AtomicBool> {
        self.drain.clone()
    }

    pub fn run(&mut self, stop: Arc<AtomicBool>) -> Result<(), String> {
        let (tx, wire) = mpsc::channel::<Result<Wire, String>>();
        {
            let stop = stop.clone();
            let api = Api::new(&self.o.base, self.o.token.clone());
            std::thread::spawn(move || {
                let events = tx.clone();
                if let Err(e) = api.stream(&stop, |w| {
                    let _ = events.send(Ok(w));
                }) {
                    let _ = tx.send(Err(e));
                }
            });
        }
        let mut last: Option<Instant> = None;
        let mut busy = false;
        let (roster_tx, roster_rx) = mpsc::channel::<Result<Value, String>>();
        let mut roster_at: Option<Instant> = None;
        while !stop.load(Ordering::Relaxed) {
            let mut changed = false;
            if roster_at.is_none_or(|t| t.elapsed() >= ROSTER_EVERY) {
                roster_at = Some(Instant::now());
                let (api, tx) = (Api::new(&self.o.base, String::new()), roster_tx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(api.roster());
                });
            }
            while let Ok(r) = roster_rx.try_recv() {
                match r {
                    Ok(bots) => {
                        self.world.bots = bots;
                        changed |= self.world.synced;
                    }
                    Err(e) => log(&format!("list of bots not read: {e}")),
                }
            }
            let mut next = wire.recv_timeout(Duration::from_millis(30)).ok();
            while let Some(w) = next {
                match w {
                    Ok(w) => changed |= self.on_wire(w),
                    Err(e) => {
                        log(&format!("stream stopped: {e}"));
                        self.set_conn("refused");
                        self.pauser.hold(false);
                        return Err(format!("{e}; check FS_BOT_TOKEN"));
                    }
                }
                next = wire.try_recv().ok();
            }
            self.collect();
            if !self.world.synced {
                continue;
            }
            let every = Duration::from_secs(if busy { 1 } else { 5 });
            if !changed && last.is_some_and(|t| t.elapsed() < every) {
                continue;
            }
            last = Some(Instant::now());
            self.report_environment();
            if self.catch_up_at.is_some_and(|t| t <= Instant::now()) {
                self.catch_up();
            }
            let v = self.world.snapshot();
            let leaving = stop.load(Ordering::Relaxed) || self.drain.load(Ordering::Relaxed);
            self.set_conn(match (self.world.up, leaving) {
                (false, _) => "retrying",
                (true, true) => "draining",
                (true, false) => "online",
            });
            busy = self.handle(&v, Instant::now());
            self.world
                .matches
                .retain(|_, m| matches!(m["status"].as_str(), Some("pending" | "running")));
            let known = &self.world.matches;
            self.decided.retain(|id, _| known.contains_key(id));
            if self.drain.load(Ordering::Relaxed) && !busy && !self.has_pending(&v) {
                log("no match running; stopped");
                break;
            }
        }
        self.pauser.hold(false);
        self.set_conn("stopped");
        Ok(())
    }

    /// Takes one thing from the stream; true when our state changed.
    fn on_wire(&mut self, w: Wire) -> bool {
        let ev = match w {
            Wire::Down(why) => {
                log(&format!("stream down ({why}); reconnecting"));
                self.world.up = false;
                return self.world.synced;
            }
            Wire::Event(ev) => ev,
        };
        if let Some(t) = ev["server_now_ms"].as_i64() {
            let d = t - now_ms();
            self.world.offset = Some(self.world.offset.map_or(d, |o| o.max(d)));
        }
        match ev["type"].as_str() {
            Some("hello") => {
                log(&format!("stream connected, bot id {}", ev["bot_id"]));
                // What the site holds of a connection that went down is not known, so say it again.
                self.env_sent = None;
                false
            }
            Some("state") if ev["ok"] != Value::Bool(false) => {
                let mut now = BTreeMap::new();
                for m in ev["matches"].as_array().into_iter().flatten() {
                    if let Some(id) = m["id"].as_i64() {
                        now.insert(id, m.clone());
                    }
                }
                for (id, m) in &self.world.matches {
                    if m["status"] == "running" && !now.contains_key(id) {
                        log(&format!("match {id} ended while the stream was down"));
                        self.lost.insert(*id);
                        self.catch_up_at = Some(Instant::now());
                    }
                }
                self.world.matches = now;
                self.world.synced = true;
                self.world.up = true;
                self.absorb(&ev);
                true
            }
            Some("match") => self.world.merge(&ev["match"]),
            _ => false,
        }
    }

    /// Keeps what an answer or an event says of us, the other bots and a match of ours.
    fn absorb(&mut self, r: &Value) {
        if r["bot"].is_object() {
            self.world.bot = r["bot"].clone();
        }
        if r["bots"].is_array() {
            self.world.bots = r["bots"].clone();
        }
        self.world.merge(&r["match"]);
    }

    /// Writes the site's records not kept yet, and learns the opponents in them.
    fn keep(&mut self, records: &[Value]) {
        for r in records {
            self.kept_until = self.kept_until.max(r["updated_ms"].as_i64().unwrap_or(0));
            learn_opponent(&mut self.names, r, &self.o.base);
            if self.kept.insert(record_key(r)) {
                let _ = writeln!(self.games, "{r}");
            }
            let id = r["id"].as_i64().unwrap_or(0);
            if r["kind"] == "bot" && self.lost.remove(&id) && self.finished.insert(id) {
                let _ = writeln!(self.results, "{}", result_of_record(r));
                self.view.lock().unwrap().finished += 1;
                log(&format!(
                    "match {id} over, from the site's record: {}",
                    r["result"]
                ));
            }
        }
    }

    /// Asks the site for our games since the last record kept: the ones played before this
    /// program kept records, and the ones that ended while the stream was down. One page a
    /// call; a page that fails is asked for again a minute later.
    fn catch_up(&mut self) {
        match self.api.games(self.kept_until) {
            Ok(records) => {
                let before = self.kept.len();
                self.keep(&records);
                let new = self.kept.len() - before;
                if !records.is_empty() {
                    log(&format!("{} records read, {new} new", records.len()));
                }
                // The newest record's time comes back again, so a page with nothing new is the end.
                self.catch_up_at = (new > 0).then(|| Instant::now() + RECORDS_APART);
            }
            Err(e) => {
                log(&format!("records not read: {e}"));
                self.catch_up_at = Some(Instant::now() + Duration::from_secs(60));
            }
        }
    }

    /// Tells the site the engine's details, at the start and when they change.
    fn report_environment(&mut self) {
        let env = self.environment();
        if self.env_sent.as_ref() == Some(&env) {
            return;
        }
        let mut body = Map::new();
        body.insert("environment".into(), env.clone());
        match self.api.call("agent_environment", body) {
            Reply::Ok(r) => {
                self.env_sent = Some(env);
                self.absorb(&r);
            }
            Reply::Refused { status, error } => {
                log(&format!("environment refused ({status}): {error}"));
                self.env_sent = Some(env);
            }
            Reply::Failed(e) => log(&format!("environment not sent: {e}")),
        }
    }
}

/// Our state as the stream gives it: the last `state` event, kept current by `match` events.
#[derive(Default)]
struct World {
    bot: Value,
    bots: Value,
    matches: BTreeMap<i64, Value>,
    /// The server's clock minus ours, as the least delayed event showed it.
    offset: Option<i64>,
    /// A `state` event has come.
    synced: bool,
    /// The stream is connected.
    up: bool,
}

impl World {
    /// Our view of one match replaces the one held, unless it is older.
    fn merge(&mut self, m: &Value) -> bool {
        let Some(id) = m["id"].as_i64() else {
            return false;
        };
        // An answer may carry less than our view; such a match would look like nobody's.
        let whole =
            m["status"].is_string() && (id < 0 || (m["bot_a"].is_i64() && m["bot_b"].is_i64()));
        if !whole {
            return false;
        }
        let rev = |x: &Value| x["state"]["revision"].as_i64();
        let mut m = m.clone();
        if let Some(old) = self.matches.get(&id) {
            let same_game =
                old["room_created_at"] == m["room_created_at"] && old["round_no"] == m["round_no"];
            match (rev(old), rev(&m)) {
                (Some(a), Some(b)) if same_game && b < a => return false,
                (Some(_), None) if same_game => m["state"] = old["state"].clone(),
                _ => {}
            }
        }
        self.matches.insert(id, m);
        true
    }

    /// The whole state as one object: us, the other bots and our matches.
    fn snapshot(&self) -> Value {
        json!({
            "bot": self.bot,
            "bots": self.bots,
            "matches": self.matches.values().collect::<Vec<_>>(),
            "server_now_ms": now_ms() + self.offset.unwrap_or(0),
        })
    }
}

impl Bot {
    /// Acts on the state as it stands; returns whether a match of ours is running.
    fn handle(&mut self, v: &Value, recv: Instant) -> bool {
        let ctl = self.ctl.lock().unwrap().clone();
        if ctl.use_book != self.engine_cfg.use_book {
            self.engine_cfg.use_book = ctl.use_book;
            for slot in &mut self.slots {
                slot.worker.send(Job::SetUseBook(ctl.use_book));
            }
        }
        if ctl.book_tolerance != self.engine_cfg.book_tolerance {
            self.engine_cfg.book_tolerance = ctl.book_tolerance;
            for slot in &mut self.slots {
                slot.worker.send(Job::SetBookTolerance(ctl.book_tolerance));
            }
        }
        self.me = v["bot"]["id"].as_i64().unwrap_or(self.me);
        if let Some(n) = v["bot"]["max_games"].as_u64() {
            self.max_games = Some(n as usize);
        }
        self.learn_bots(v);
        if let (Some(id), Some(name)) = (v["bot"]["id"].as_i64(), v["bot"]["name"].as_str()) {
            let me = self.names.entry(id).or_default();
            me.id = id;
            me.name = name.to_string();
            me.rating = v["bot"]["rating"]["rating"].as_f64().or(me.rating);
            me.photo = photo_url(&self.o.base, &v["bot"]["photo"]).or(me.photo.take());
        }
        self.track_rating(v);
        let server_now = v["server_now_ms"].as_i64().unwrap_or_else(now_ms);
        let matches: Vec<Match> = v["matches"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| serde_json::from_value(m.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let mine: Vec<&Match> = matches
            .iter()
            .filter(|m| m.bot_a == self.me || m.bot_b == self.me || m.id < 0)
            .collect();
        self.settle_challenge(&matches);
        let running = mine.iter().filter(|m| m.status == "running").count();
        let mut taking = running;
        for m in &mine {
            match m.status.as_str() {
                "pending" if m.bot_b == self.me || m.id < 0 => {
                    let taken = match self.decided.get(&m.id) {
                        Some(&taken) => taken,
                        None => self.answer(m, &ctl, taking),
                    };
                    taking += usize::from(taken);
                }
                "running" => {
                    if let Some(s) = &m.state {
                        self.track_legs(m, s);
                        self.play(m, s, server_now, recv, &ctl);
                    }
                }
                "finished" | "aborted" if !self.finished.contains(&m.id) => {
                    // the last moves came with the event that ended the match
                    if let Some(s) = &m.state {
                        self.track_legs(m, s);
                    }
                    self.report(m);
                }
                _ => {}
            }
        }
        let live: HashSet<i64> = mine
            .iter()
            .filter(|m| m.status == "running")
            .map(|m| m.id)
            .collect();
        self.release(&live);
        if running == 0 && self.owner.is_empty() {
            self.refit_engines(ctl.hash);
        }
        self.pauser.hold(running > 0);
        play::learner().hold(&self.learn_hold(), running > 0);
        let waiting = mine.iter().any(|m| m.status == "pending");
        if self.drain.load(Ordering::Relaxed) {
            if !self.draining_logged {
                self.draining_logged = true;
                log(&format!("stopping: taking no new match; {running} running"));
            }
        } else if let Some(target) = ctl.challenge_now.filter(|_| self.outstanding.is_none()) {
            self.ctl.lock().unwrap().challenge_now = None;
            // Our own challenge counts against the matches at once as one taken would.
            if taking >= self.cap(&ctl) {
                log("challenge not sent: the matches at once are full");
                self.publish_view(&mine, server_now, recv);
                return running > 0;
            }
            let name = self
                .names
                .get(&target)
                .map(|b| b.name.clone())
                .unwrap_or_default();
            self.send_challenge(target, &name);
        } else if running == 0 && !waiting && self.cap(&ctl) > 0 {
            if ctl.challenge.key() != self.challenge_key {
                self.challenge_key = ctl.challenge.key().into();
                self.to_challenge = None;
            }
            // With no list from the site, the bots to challenge are the ones we have played;
            // whether one is online shows only in its answer.
            if v["bots"].is_array() {
                self.challenge_next(v, &ctl.challenge);
            } else {
                let bots: Vec<Value> = self
                    .names
                    .values()
                    .map(|b| {
                        json!({"id": b.id, "name": b.name, "rating": {"rating": b.rating},
                               "online": true, "is_builtin": b.builtin, "supported_games": ["std"]})
                    })
                    .collect();
                let known = json!({"bot": v["bot"], "bots": bots});
                self.challenge_next(&known, &ctl.challenge);
            }
        }
        self.publish_view(&mine, server_now, recv);
        running > 0
    }

    /// Names, ratings and states of the standard bots, and our matches at once.
    fn learn_bots(&mut self, v: &Value) {
        let Some(bots) = v["bots"].as_array() else {
            return;
        };
        if let Some(n) = bots
            .iter()
            .find(|b| b["id"].as_i64() == Some(self.me))
            .and_then(|b| b["max_games"].as_u64())
        {
            self.max_games = Some(n as usize);
        }
        self.names = bots_of(&v["bots"], &self.o.base);
    }

    /// Follows each leg state by state, keeping its moves.
    fn track_legs(&mut self, m: &Match, s: &api::State) {
        let part = if m.bot_a == self.me { 0 } else { 1 };
        for (i, leg) in s.legs.iter().enumerate() {
            if leg.board.len() != 64 {
                continue;
            }
            let h = self.hist.entry((m.id, i)).or_insert_with(|| LegHist {
                cells: leg.board.clone(),
                turn: leg.turn,
                complete: leg.board.iter().filter(|&&c| c != 0).count() == 4,
                ..Default::default()
            });
            // The opponent's thinking time is what its clock lost: the clock of the side to move
            // stands as of its turn's start until it moves.
            let opp = if usize::from(leg.black_participant) == part {
                2
            } else {
                1
            };
            if let Some(&c) = leg.clocks_ms.get(usize::from(opp - 1)) {
                if leg.status == "active" && leg.turn == opp {
                    h.opp_clock.get_or_insert(c);
                } else if let Some(start) = h.opp_clock.take() {
                    let ply = leg
                        .board
                        .iter()
                        .filter(|&&c| c != 0)
                        .count()
                        .saturating_sub(4);
                    h.opp_secs
                        .push((ply, ((start - c) / 1000.0).max(0.0) as f32));
                }
            }
            // The site's own record of the moves, when it sends one; else they are read off the boards.
            if !leg.history.is_empty() || !s.opening_moves.is_empty() {
                h.moves = s
                    .opening_moves
                    .iter()
                    .copied()
                    .chain(leg.history.iter().map(|p| p.mv))
                    .filter(|&q| q >= 0)
                    .collect();
                h.complete = true;
                let mut ply = s.opening_moves.iter().filter(|&&q| q >= 0).count();
                h.opp_evals.clear();
                for p in leg.history.iter().filter(|p| p.mv >= 0) {
                    ply += 1;
                    let Some(e) = p.eval else { continue };
                    if p.color == opp {
                        h.opp_evals.push((ply, e as f32));
                    } else if !h.evals.iter().any(|x| x.0 == ply) {
                        h.evals.push((ply, e as f32, f32::NAN));
                        h.evals.sort_by_key(|x| x.0);
                    }
                }
            } else if h.cells != leg.board {
                match infer_moves(&h.cells, h.turn, &leg.board) {
                    Some(ms) => h.moves.extend(ms.into_iter().map(|q| q as i64)),
                    None => h.complete = false,
                }
            }
            h.cells = leg.board.clone();
            h.turn = leg.turn;
        }
    }

    fn publish_view(&mut self, mine: &[&Match], server_now: i64, recv: Instant) {
        let thinking: HashSet<(i64, usize)> = self
            .slots
            .iter()
            .filter(|s| s.worker.busy)
            .filter_map(|s| s.sent.as_ref().map(|x| x.key))
            .collect();
        let waited = recv.elapsed().as_secs_f64() * 1000.0;
        let matches: Vec<LiveMatch> = mine
            .iter()
            .filter(|m| m.status == "running")
            .filter_map(|m| {
                let s = m.state.as_ref()?;
                let part = if m.bot_a == self.me { 0 } else { 1 };
                let opp = if part == 0 { m.bot_b } else { m.bot_a };
                let legs = s
                    .legs
                    .iter()
                    .enumerate()
                    .map(|(i, leg)| {
                        let h = self.hist.get(&(m.id, i));
                        let mut clocks = leg.clocks_ms.clone();
                        if leg.status == "active" {
                            let at = usize::from(leg.turn.max(1) - 1);
                            if let Some(c) = clocks.get_mut(at) {
                                let gone = (server_now - leg.turn_started_ms) as f64 + waited;
                                *c = (*c - gone).max(0.0);
                            }
                        }
                        LiveLeg {
                            cells: leg.board.clone(),
                            turn: leg.turn,
                            my_color: if usize::from(leg.black_participant) == part {
                                1
                            } else {
                                2
                            },
                            clocks_ms: clocks,
                            discs: leg.discs.clone(),
                            last: h.and_then(|h| h.moves.last().copied()),
                            ply: leg
                                .board
                                .iter()
                                .filter(|&&c| c != 0)
                                .count()
                                .saturating_sub(4),
                            active: leg.status == "active",
                            thinking: thinking.contains(&(m.id, i)),
                            evals: h.map(|h| h.evals.clone()).unwrap_or_default(),
                            opp_secs: h.map(|h| h.opp_secs.clone()).unwrap_or_default(),
                            opp_evals: h.map(|h| h.opp_evals.clone()).unwrap_or_default(),
                            moves: h
                                .filter(|h| h.complete)
                                .map(|h| h.moves.iter().map(|&q| coord(q)).collect())
                                .unwrap_or_default(),
                        }
                    })
                    .collect();
                let o = self.names.get(&opp);
                Some(LiveMatch {
                    id: m.id,
                    kind: kind_of(m).into(),
                    opponent: opp,
                    opponent_name: o.map(|b| b.name.clone()).unwrap_or_default(),
                    opponent_rating: o.and_then(|b| b.rating),
                    legs,
                })
            })
            .collect();
        let me = self.names.get(&self.me).cloned();
        let outstanding = self.outstanding.map(|(_, target, at)| Outstanding {
            target,
            name: self
                .names
                .get(&target)
                .map(|b| b.name.clone())
                .unwrap_or_default(),
            since_ms: now_ms() - at.elapsed().as_millis() as i64,
        });
        let mut bots: Vec<OtherBot> = self.names.values().cloned().collect();
        bots.sort_by(|a, b| b.rating.unwrap_or(0.0).total_cmp(&a.rating.unwrap_or(0.0)));
        let mut v = self.view.lock().unwrap();
        v.id = self.me;
        if let Some(me) = me {
            v.name = me.name;
            v.photo = me.photo;
        }
        v.rating = self.ratings.last().copied();
        v.max_games = self.max_games;
        v.matches = matches;
        v.outstanding = outstanding;
        v.cooling = self.cooling.keys().copied().collect();
        v.bots = bots;
    }

    fn track_rating(&mut self, v: &Value) {
        let Some(r) = v["bot"]["rating"]["rating"].as_f64() else {
            return;
        };
        if self.ratings.last() == Some(&r) {
            return;
        }
        match self.ratings.last() {
            Some(&prev) => log(&format!("rating {r:.1} ({:+.1})", r - prev)),
            None => log(&format!("rating {r:.1}")),
        }
        self.ratings.push(r);
        let Some((n, points)) = self.o.until_stable else {
            return;
        };
        if self.ratings.len() > n && !self.drain.load(Ordering::Relaxed) {
            let moved = r - self.ratings[self.ratings.len() - 1 - n];
            if moved.abs() <= points {
                log(&format!(
                    "rating settled: {moved:+.1} over the last {n} changes; finishing up"
                ));
                self.drain.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Clears our challenge once answered; one not taken leaves that bot alone for a while.
    fn settle_challenge(&mut self, matches: &[Match]) {
        let Some((id, target, at)) = self.outstanding else {
            return;
        };
        let status = matches
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.status.as_str());
        let taken = matches!(status, Some("running") | Some("finished") | Some("aborted"));
        let waiting = status == Some("pending") && at.elapsed() < CHALLENGE_WAIT;
        if taken {
            self.outstanding = None;
        } else if !waiting && (status.is_some() || at.elapsed() >= CHALLENGE_WAIT) {
            log(&format!(
                "bot {target} did not take match {id} ({}); left alone for {} min",
                status.unwrap_or("gone"),
                COOL_DOWN.as_secs() / 60
            ));
            self.cooling.insert(target, Instant::now() + COOL_DOWN);
            self.outstanding = None;
        }
    }

    fn challenge_next(&mut self, v: &Value, challenge: &Challenge) {
        if self.outstanding.is_some() {
            return;
        }
        self.cooling.retain(|_, until| *until > Instant::now());
        let stronger = matches!(challenge, Challenge::Stronger);
        if stronger && self.to_challenge.as_ref().is_some_and(|q| q.is_empty()) {
            self.to_challenge = None;
        }
        if self.to_challenge.is_none() {
            let bots = v["bots"].as_array().cloned().unwrap_or_default();
            let std_ok = |b: &Value| {
                b["supported_games"]
                    .as_array()
                    .is_some_and(|g| g.iter().any(|x| x == "std"))
                    && b["deleted"] != Value::Bool(true)
                    && b["disabled"] != Value::Bool(true)
            };
            let name = |b: &Value| b["name"].as_str().unwrap_or("").to_string();
            let mine = v["bot"]["rating"]["rating"].as_f64().unwrap_or(0.0);
            let list: Vec<(i64, String)> = match challenge {
                Challenge::None => Vec::new(),
                Challenge::Stronger => {
                    let r = |x: &Value| x["rating"]["rating"].as_f64().unwrap_or(0.0);
                    let mut l: Vec<&Value> = bots
                        .iter()
                        .filter(|b| {
                            std_ok(b)
                                && b["online"] == Value::Bool(true)
                                && b["id"].as_i64() != Some(self.me)
                                && r(b) > mine
                        })
                        .collect();
                    l.sort_by(|a, b| r(a).total_cmp(&r(b)));
                    l.into_iter()
                        .filter_map(|b| Some((b["id"].as_i64()?, name(b))))
                        .collect()
                }
                Challenge::Builtin => {
                    let mut l: Vec<&Value> = bots
                        .iter()
                        .filter(|b| b["is_builtin"] == Value::Bool(true) && std_ok(b))
                        .collect();
                    l.sort_by(|a, b| {
                        let r = |x: &Value| x["rating"]["rating"].as_f64().unwrap_or(0.0);
                        r(a).total_cmp(&r(b))
                    });
                    l.into_iter()
                        .filter_map(|b| Some((b["id"].as_i64()?, name(b))))
                        .collect()
                }
                Challenge::Ids(ids) => ids
                    .iter()
                    .map(|&id| {
                        let n = bots
                            .iter()
                            .find(|b| b["id"].as_i64() == Some(id))
                            .map(name)
                            .unwrap_or_default();
                        (id, n)
                    })
                    .collect(),
            };
            if !list.is_empty() {
                let names: Vec<String> = list.iter().map(|(i, n)| format!("{n} ({i})")).collect();
                log(&format!("to challenge: {}", names.join(", ")));
            }
            self.to_challenge = Some(list.into());
        }
        if self
            .last_challenge
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
        {
            return;
        }
        let Some(queue) = self.to_challenge.as_mut() else {
            return;
        };
        // A bot that is not online now is kept for later.
        let online = |id: i64| {
            v["bots"].as_array().is_some_and(|bots| {
                bots.iter()
                    .any(|b| b["id"].as_i64() == Some(id) && b["online"] == Value::Bool(true))
            })
        };
        let cooling = &self.cooling;
        let Some(at) = queue
            .iter()
            .position(|&(id, _)| online(id) && !cooling.contains_key(&id))
        else {
            return;
        };
        let Some((id, name)) = queue.remove(at) else {
            return;
        };
        let last = queue.is_empty();
        match self.send_challenge(id, &name) {
            Asked::Yes if last && !stronger => log("every bot on the list has been challenged"),
            Asked::Busy => {
                if let Some(q) = self.to_challenge.as_mut() {
                    q.push_back((id, name));
                }
            }
            Asked::Failed => {
                if let Some(q) = self.to_challenge.as_mut() {
                    q.push_front((id, name));
                }
            }
            _ => {}
        }
    }

    fn send_challenge(&mut self, id: i64, name: &str) -> Asked {
        self.last_challenge = Some(Instant::now());
        let mut body = Map::new();
        body.insert("target_id".into(), json!(id));
        body.insert("game_mode".into(), json!("std"));
        match self.api.call("agent_challenge", body) {
            Reply::Ok(r) => {
                if let Some(mid) = r["match_id"].as_i64() {
                    self.outstanding = Some((mid, id, Instant::now()));
                }
                log(&format!(
                    "challenged {name} ({id}): match {}",
                    r["match_id"]
                ));
                self.absorb(&r);
                Asked::Yes
            }
            Reply::Refused { status: 409, error }
                if error.contains("busy") || error.contains("active") =>
            {
                log(&format!("{name} ({id}) is busy; later"));
                Asked::Busy
            }
            Reply::Refused { error, .. } => {
                log(&format!("challenge {name} ({id}): {error}"));
                // Offline, or not taking challenges: asking again at once would only repeat it.
                self.cooling.insert(id, Instant::now() + COOL_DOWN);
                Asked::Refused
            }
            Reply::Failed(e) => {
                log(&format!("challenge {name} ({id}) not sent: {e}"));
                Asked::Failed
            }
        }
    }

    fn has_pending(&self, v: &Value) -> bool {
        v["matches"].as_array().is_some_and(|ms| {
            ms.iter().any(|m| {
                m["status"] == "pending"
                    && (m["bot_a"].as_i64() == Some(self.me)
                        || m["bot_b"].as_i64() == Some(self.me))
            })
        })
    }

    /// Matches at once: ours here, else the site's.
    fn cap(&self, ctl: &Control) -> usize {
        ctl.cap.or(self.max_games).unwrap_or(usize::MAX)
    }

    /// Takes or declines a challenge; `taking` is the matches already running or just taken.
    fn answer(&mut self, m: &Match, ctl: &Control, taking: usize) -> bool {
        let mut body = Map::new();
        body.insert("match_id".into(), json!(m.id));
        // A challenge of ours still waiting for its answer holds a place, so both cannot start.
        let held = taking + usize::from(self.outstanding.is_some());
        let take = (m.game_mode.is_empty() || m.game_mode == "std")
            && !self.drain.load(Ordering::Relaxed)
            && ctl.accept
            && held < self.cap(ctl);
        let action = if take { "agent_accept" } else { "agent_reject" };
        match self.api.call(action, body) {
            Reply::Ok(r) => {
                if take {
                    log(&format!("accepted match {}", m.id));
                } else {
                    log(&format!("declined match {} ({})", m.id, m.game_mode));
                }
                self.decided.insert(m.id, take);
                self.absorb(&r);
                return take;
            }
            Reply::Refused { error, .. } => {
                log(&format!("{action} {}: {error}", m.id));
                self.decided.insert(m.id, false);
            }
            Reply::Failed(e) => log(&format!("{action} {}: {e}", m.id)),
        }
        false
    }

    /// Starts a search for each leg to move, each on its own engine with the bot's threads.
    fn play(&mut self, m: &Match, s: &api::State, server_now: i64, recv: Instant, ctl: &Control) {
        if s.board_size != 8 || !(s.game_mode.is_empty() || s.game_mode == "std") {
            return;
        }
        let part = if m.bot_a == self.me { 0 } else { 1 };
        let threads = ctl.threads.max(1);
        for (i, leg) in s.legs.iter().enumerate() {
            let color = if leg.black_participant == part { 1 } else { 2 };
            if leg.status == "active" && leg.turn != color {
                if ctl.ponder && ctl.level.is_none() && m.id > 0 {
                    self.ponder(m.id, i, leg, threads, server_now, recv);
                } else {
                    self.stop_pondering((m.id, i));
                }
            }
            if leg.status != "active" || leg.turn != color {
                continue;
            }
            let key = (m.id, i);
            if self.answered.contains(&(m.id, i, leg.revision)) {
                continue;
            }
            if self
                .owner
                .get(&key)
                .and_then(|&k| self.slots[k].sent.as_ref())
                .is_some_and(|x| x.revision == leg.revision)
            {
                continue;
            }
            let human = (m.id < 0).then(|| (m.round_no, m.room_created_at.clone()));
            if leg.legal_moves.is_empty() {
                if human.is_none() {
                    self.send_move(m.id, i, leg.revision, -1, None, None);
                }
                continue;
            }
            let Some(board) = board_of(leg) else {
                log(&format!("match {} leg {i}: unreadable board", m.id));
                continue;
            };
            let waited =
                (server_now - leg.turn_started_ms) as f64 / 1000.0 + recv.elapsed().as_secs_f64();
            let left = leg
                .clocks_ms
                .get(usize::from(color - 1))
                .copied()
                .unwrap_or(0.0)
                / 1000.0
                - waited;
            if let Some((before, searched)) = self.spent.remove(&key) {
                let lost = (before - left - searched).clamp(0.0, 5.0);
                self.lag = 0.8 * self.lag + 0.2 * lost;
            }
            let budget = budget(left, self.o.margin, self.lag, board.empty_count());
            let Some(k) = self.slot_for(key) else {
                log("no engine free for a leg");
                continue;
            };
            let slot = &mut self.slots[k];
            if slot.worker.threads != threads {
                slot.worker.send(Job::SetThreads(threads));
                slot.worker.threads = threads;
            }
            // A ponder clears the stop once it has ended; cleared here, it would run on.
            if slot.pondering.take().is_some() {
                slot.worker.stop.stop();
            } else {
                slot.worker.stop.reset();
            }
            slot.worker.send(Job::Think {
                board,
                clock: Duration::from_secs_f64(budget),
                hint: None,
            });
            slot.sent = Some(Sent {
                key,
                revision: leg.revision,
                color,
                match_id: m.id,
                human,
                at: Instant::now(),
                left,
                budget,
                empties: u32::from(board.empty_count()),
                threads,
            });
        }
    }

    /// Searches a leg's position while the opponent thinks on it, until it moves (its clock is
    /// the longest it can take).
    fn ponder(
        &mut self,
        match_id: i64,
        i: usize,
        leg: &Leg,
        threads: usize,
        server_now: i64,
        recv: Instant,
    ) {
        let key = (match_id, i);
        if self
            .owner
            .get(&key)
            .is_some_and(|&k| self.slots[k].pondering == Some(leg.revision))
        {
            return;
        }
        let Some(board) = board_of(leg).filter(|b| b.movable() != 0) else {
            return;
        };
        let waited =
            (server_now - leg.turn_started_ms) as f64 / 1000.0 + recv.elapsed().as_secs_f64();
        let left = leg
            .clocks_ms
            .get(usize::from(leg.turn.max(1) - 1))
            .copied()
            .unwrap_or(0.0)
            / 1000.0
            - waited;
        if left < 0.5 {
            return;
        }
        let Some(k) = self.slot_for(key) else {
            return;
        };
        let slot = &mut self.slots[k];
        if slot.pondering.is_some() {
            slot.worker.stop.stop();
        }
        if slot.worker.threads != threads {
            slot.worker.send(Job::SetThreads(threads));
            slot.worker.threads = threads;
        }
        slot.worker.send(Job::Ponder {
            board,
            span: Duration::from_secs_f64(left),
        });
        slot.pondering = Some(leg.revision);
    }

    fn stop_pondering(&mut self, key: (i64, usize)) {
        if let Some(&k) = self.owner.get(&key) {
            if self.slots[k].pondering.take().is_some() {
                self.slots[k].worker.stop.stop();
            }
        }
    }

    fn slot_for(&mut self, key: (i64, usize)) -> Option<usize> {
        if let Some(&k) = self.owner.get(&key) {
            return (!self.slots[k].worker.busy).then_some(k);
        }
        let taken: HashSet<usize> = self.owner.values().copied().collect();
        let k = match (0..self.slots.len()).find(|k| !taken.contains(k)) {
            Some(k) => k,
            None => {
                let hash = self.ctl.lock().unwrap().hash;
                let w = EngineWorker::spawn_with(self.assets.clone(), self.tables_of(hash), false)
                    .ok()?;
                self.slots.push(Slot {
                    worker: w,
                    sent: None,
                    hash,
                    pondering: None,
                });
                self.slots.len() - 1
            }
        };
        let level = self.ctl.lock().unwrap().level;
        self.slots[k].worker.send(Job::SetLevel(level));
        self.slots[k].worker.send(Job::NewGame);
        self.owner.insert(key, k);
        Some(k)
    }

    /// Sends the moves the engines have found.
    fn collect(&mut self) {
        for k in 0..self.slots.len() {
            let slot = &mut self.slots[k];
            while let Ok(p) = slot.worker.pondered.try_recv() {
                log(&format!(
                    "pondered: empties {} threads {} {:.2}s {} nodes",
                    p.empties, p.threads, p.secs, p.nodes
                ));
            }
            if let Some(sent) = &slot.sent {
                if slot.worker.busy && sent.at.elapsed().as_secs_f64() > sent.budget * 2.0 + 3.0 {
                    slot.worker.stop.stop();
                }
            }
            let Ok(Done(mv, report)) = slot.worker.rx.try_recv() else {
                continue;
            };
            slot.worker.busy = false;
            let Some(sent) = slot.sent.take() else {
                continue;
            };
            let secs = sent.at.elapsed().as_secs_f64();
            let idx = mv.pos.map_or(-1, index_of);
            let rec = json!({
                "t": now_ms(), "match": sent.match_id, "leg": sent.key.1, "color": sent.color,
                "empties": sent.empties, "move": coord(idx), "value": mv.value, "depth": mv.depth,
                "exact": mv.exact, "cut": mv.cut, "book": mv.from_book, "nodes": report.nodes,
                "threads": sent.threads, "secs": (secs * 1000.0).round() / 1000.0,
                "left": (sent.left * 1000.0).round() / 1000.0,
                "budget": (sent.budget * 1000.0).round() / 1000.0,
                "lag": (self.lag * 1000.0).round() / 1000.0,
            });
            let _ = writeln!(self.moves, "{rec}");
            self.spent.insert(sent.key, (sent.left, secs));
            if let Some(h) = self.hist.get_mut(&sent.key) {
                h.evals
                    .push((61 - sent.empties as usize, mv.value, secs as f32));
            }
            // In discs from our side; the site shows it only in bot matches.
            let eval = (sent.human.is_none() && mv.pos.is_some() && mv.value.is_finite())
                .then(|| mv.value.round().clamp(-64.0, 64.0) as i64);
            self.send_move(
                sent.match_id,
                sent.key.1,
                sent.revision,
                idx,
                sent.human,
                eval,
            );
        }
    }

    fn send_move(
        &mut self,
        match_id: i64,
        leg: usize,
        revision: i64,
        mv: i64,
        human: Option<(Option<i64>, Option<String>)>,
        eval: Option<i64>,
    ) {
        let mut body = Map::new();
        body.insert("match_id".into(), json!(match_id));
        body.insert("leg".into(), json!(leg));
        body.insert("revision".into(), json!(revision));
        body.insert("move".into(), json!(mv));
        if let Some((round, created)) = human {
            body.insert("round_no".into(), json!(round));
            body.insert("room_created_at".into(), json!(created));
        }
        if let Some(e) = eval {
            body.insert("eval".into(), json!(e));
        }
        for attempt in 0..3 {
            match self.api.call("agent_move", body.clone()) {
                Reply::Ok(r) => {
                    self.answered.insert((match_id, leg, revision));
                    self.absorb(&r);
                    return;
                }
                Reply::Refused { status, error } if status == 429 || status == 503 => {
                    log(&format!("move {} retried ({status}): {error}", coord(mv)));
                    std::thread::sleep(Duration::from_millis(300 * (attempt + 1)));
                }
                Reply::Refused { error, .. } => {
                    log(&format!(
                        "move {} refused in match {match_id} leg {leg}: {error}",
                        coord(mv)
                    ));
                    return;
                }
                Reply::Failed(e) => {
                    log(&format!("move {} not sent: {e}", coord(mv)));
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    fn learn_hold(&self) -> String {
        format!("flipsight:{}", self.me)
    }

    /// Hands each game of a finished match to the learner, from the site's record: its moves
    /// from the standard start, the opening included, for people's games and bots' alike.
    fn learn(&self, m: &Match) {
        let Some(r) = m.record.as_ref() else { return };
        if !self.ctl.lock().unwrap().learn
            || !matches!(r["result"].as_str(), Some("win" | "loss" | "draw"))
        {
            return;
        }
        let x = result_of_record(r);
        for leg in x["legs"].as_array().into_iter().flatten() {
            let Some(kifu) = leg["moves"].as_str().filter(|k| !k.is_empty()) else {
                continue;
            };
            let kifu = kifu.to_string();
            let (black, white) = match kuroobi::learn::replay(None, &kifu) {
                Ok((_, fin)) => (fin.black.count_ones() as u8, fin.white.count_ones() as u8),
                Err(e) => {
                    log(&format!("match {} not learned: {e}", m.id));
                    continue;
                }
            };
            let mut entry = ggs::env::LearnEntry {
                at: ggs::env::now_secs(),
                kifu: kifu.clone(),
                black,
                white,
                positions: kuroobi::learn::BackupJob::new(None, &kifu, play::LEARN_DEPTH as u8)
                    .map_or(0, |j| j.remaining() as u32),
                start: String::new(),
                changes: Vec::new(),
                opponent: r["opponent"]["name"].as_str().unwrap_or("").to_string(),
                my_color: if leg["black"] == true { "b" } else { "w" }.into(),
            };
            let id = m.id;
            let task = play::LearnTask {
                start: None,
                kifu,
                done: Box::new(move |out| match out {
                    Ok(out) => {
                        entry.changes = out.changes.iter().map(ggs::env::LearnChange::of).collect();
                        ggs::env::learn_log_append(&entry);
                        log(&format!(
                            "match {id} learned: {} values updated, {} positions added",
                            out.updated, out.added
                        ));
                    }
                    Err(e) => log(&format!("match {id} not learned: {e}")),
                }),
            };
            if let Err(e) = play::learner().submit(self.engine_cfg.clone(), task) {
                log(&format!("match {} not learned: {e}", m.id));
            }
        }
    }

    fn report(&mut self, m: &Match) {
        self.finished.insert(m.id);
        self.learn(m);
        let human = kind_of(m) == "human";
        let Some(s) = &m.state else {
            if let Some(r) = m.record.as_ref().filter(|_| human) {
                let mut rec = settled_by_record(result_of_record(r));
                rec["match"] = json!(m.id);
                let _ = writeln!(self.results, "{rec}");
                self.view.lock().unwrap().finished += 1;
                self.keep(std::slice::from_ref(r));
            }
            return;
        };
        let part = if m.bot_a == self.me { 0 } else { 1 };
        let (mine, theirs) = (
            s.totals.get(part).copied().unwrap_or(0),
            s.totals.get(1 - part).copied().unwrap_or(0),
        );
        let result = match (m.status.as_str(), s.winner) {
            ("aborted", _) => "aborted",
            (_, None) => "draw",
            (_, Some(w)) if w == part as i64 => "win",
            _ => "loss",
        };
        let opp = if part == 0 { m.bot_b } else { m.bot_a };
        let legs: Vec<Value> = s
            .legs
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let h = self.hist.remove(&(m.id, i));
                self.spent.remove(&(m.id, i));
                let moves = h
                    .as_ref()
                    .filter(|h| h.complete)
                    .map(|h| h.moves.iter().map(|&q| coord(q)).collect::<String>());
                let last = h.as_ref().and_then(|h| h.moves.last().copied());
                json!({"black": usize::from(l.black_participant) == part, "discs": l.discs,
                       "result_discs": l.result_discs, "status": l.status, "moves": moves,
                       "cells": (l.board.len() == 64).then_some(&l.board), "last": last,
                       "evals": h.as_ref().map(|h| h.evals.clone()).unwrap_or_default(),
                       "opp_evals": h.as_ref().map(|h| h.opp_evals.clone()).unwrap_or_default(),
                       "opp_secs": h.map(|h| h.opp_secs).unwrap_or_default()})
            })
            .collect();
        if let Some(r) = m.record.clone() {
            self.keep(&[r]);
        }
        let o = self.names.get(&opp);
        // The site's record names the opponent when no list of bots has.
        let theirs_in = |k: &str| m.record.as_ref().map(|r| r["opponent"][k].clone());
        let rec = json!({
            "t": now_ms(), "match": m.id, "kind": kind_of(m), "opponent": opp, "status": m.status,
            "result": result,
            "opponent_name": o.map(|b| json!(b.name)).or_else(|| theirs_in("name")),
            "opponent_rating": o.and_then(|b| b.rating).map(|r| json!(r))
                .or_else(|| theirs_in("rating_before")),
            "opponent_photo": o.and_then(|b| b.photo.clone()),
            "record": m.record,
            "my_rating": m.rating_after.get(part).and_then(rating_of),
            "my_rating_before": m.rating_before.get(part).and_then(rating_of),
            "mine": mine, "theirs": theirs, "diff": mine - theirs, "finish_reason": s.finish_reason,
            "legs": legs, "rating_before": m.rating_before, "rating_after": m.rating_after,
        });
        let rec = settled_by_record(rec);
        let _ = writeln!(self.results, "{rec}");
        self.view.lock().unwrap().finished += 1;
        log(&format!(
            "match {} over vs {opp}: {} {}-{} ({})",
            m.id,
            rec["result"].as_str().unwrap_or("-"),
            rec["mine"],
            rec["theirs"],
            rec["finish_reason"].as_str().unwrap_or("-")
        ));
    }

    /// Frees the engines of matches no longer running.
    fn release(&mut self, live: &HashSet<i64>) {
        self.owner.retain(|key, k| {
            let keep = live.contains(&key.0);
            if !keep {
                let slot = &mut self.slots[*k];
                if slot.worker.busy || slot.pondering.take().is_some() {
                    slot.worker.stop.stop();
                }
                slot.sent = None;
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_keeps_the_margin_and_a_cost_for_every_later_move() {
        assert_eq!(budget(30.0, 4.0, 0.2, 20), 30.0 - 4.0 - 4.5);
        assert!((budget(30.0, 4.0, 1.5, 20) - (30.0 - 4.0 - 1.65 * 9.0)).abs() < 1e-9);
        assert_eq!(budget(10.0, 4.0, 0.8, 1), 6.0);
        assert_eq!(budget(5.0, 4.0, 0.8, 12), 0.05);
    }

    #[test]
    fn a_match_event_replaces_ours_unless_it_is_older_or_partial() {
        let ev = |status: &str, rev: Option<i64>| {
            let mut m = json!({"id": 5, "bot_a": 1, "bot_b": 2, "status": status});
            if let Some(r) = rev {
                m["state"] = json!({"revision": r});
            }
            m
        };
        let mut w = World::default();
        assert!(w.merge(&ev("pending", None)));
        assert!(w.merge(&ev("running", Some(3))));
        assert!(!w.merge(&ev("running", Some(2))), "a late event");
        assert!(
            !w.merge(&json!({"id": 5, "status": "running"})),
            "an answer's short form"
        );
        assert!(w.merge(&ev("finished", None)));
        assert_eq!(w.matches[&5]["state"]["revision"], 3, "the state is kept");
        assert_eq!(w.snapshot()["matches"][0]["status"], "finished");
        assert!(!w.merge(&Value::Null));
    }

    #[test]
    fn a_result_is_read_from_the_sites_record() {
        let r = json!({
            "id": 9, "kind": "bot", "status": "finished", "result": "win", "finished_ms": 5,
            "finish_reason": "discs", "opening": "f5d6",
            "me": {"rating_before": 1500.0, "rating_after": 1510.0},
            "opponent": {"bot_id": 3, "name": "nova", "rating_before": 1490.0},
            "legs": [{"my_color": "B", "status": "finished", "discs": [40, 24],
                      "moves": "c3pad3", "colors": "BWW", "used_ms": [1500, 0, 2000],
                      "evals": [2.5, null, -3.0]},
                     {"my_color": "W", "status": "timeout", "discs": [30, 20],
                      "result_discs": [0, 64], "moves": "", "colors": ""}],
        });
        let x = result_of_record(&r);
        assert_eq!(
            (x["mine"].clone(), x["theirs"].clone(), x["diff"].clone()),
            (json!(104), json!(24), json!(80))
        );
        assert_eq!(x["legs"][0]["moves"], "f5d6c3d3");
        assert_eq!(x["legs"][0]["evals"], json!([[3, 2.5, 1.5]]));
        assert_eq!(x["legs"][0]["opp_evals"], json!([[4, -3.0]]));
        assert_eq!(x["legs"][0]["opp_secs"], json!([[4, 2.0]]));
        assert_eq!(
            (x["opponent_name"].clone(), x["my_rating"].clone()),
            (json!("nova"), json!(1510.0))
        );
    }

    #[test]
    fn a_resignation_against_a_person_is_a_win_at_equal_discs() {
        let r = json!({
            "id": 188072, "kind": "human", "status": "finished", "result": "win",
            "finished_ms": 5, "finish_reason": "resign", "discs": [4, 4], "opening": "",
            "me": {"rating_before": null, "rating_after": null},
            "opponent": {"kind": "human", "name": "Within_"},
            "legs": [{"my_color": "W", "status": "finished", "discs": [4, 4],
                      "moves": "f5d6c3d3", "colors": "BWBW"}],
        });
        let x = result_of_record(&r);
        assert_eq!(
            (x["kind"].clone(), x["result"].clone(), x["diff"].clone()),
            (json!("human"), json!("win"), json!(0))
        );
        assert_eq!(x["legs"].as_array().map(Vec::len), Some(1));
        assert_eq!(x["opponent_name"], "Within_");
    }

    #[test]
    fn a_result_against_a_person_takes_the_outcome_from_its_record() {
        // as written from the state alone, which has no winner or totals for such a game
        let old = json!({
            "match": -1572, "opponent": 0, "status": "finished", "result": "draw",
            "mine": 0, "theirs": 0, "diff": 0, "finish_reason": null, "opponent_name": null,
            "record": {"kind": "human", "result": "win", "discs": [50, 14],
                       "finish_reason": null, "opponent": {"name": "autumn"}},
        });
        let x = settled_by_record(old);
        assert_eq!(
            (x["kind"].clone(), x["result"].clone(), x["diff"].clone()),
            (json!("human"), json!("win"), json!(36))
        );
        assert_eq!(x["opponent_name"], "autumn");
        let bot = json!({"result": "loss", "diff": -4, "record": {"kind": "bot", "result": "win"}});
        assert_eq!(settled_by_record(bot.clone()), bot);
    }

    fn after(moves: &[&str]) -> Vec<u8> {
        let mut b = Board::new();
        for m in moves {
            b.make_move(Position::from_kifu(m).unwrap()).unwrap();
        }
        cells_of_board(&b)
    }

    fn sq(m: &str) -> usize {
        index_of(Position::from_kifu(m).unwrap()) as usize
    }

    #[test]
    fn moves_are_read_back_from_the_boards() {
        let start = after(&[]);
        assert_eq!(
            infer_moves(&start, 1, &after(&["f5"])),
            Some(vec![sq("f5")])
        );
        assert_eq!(
            infer_moves(&start, 1, &after(&["f5", "d6"])),
            Some(vec![sq("f5"), sq("d6")])
        );
        let mid = after(&["f5", "d6"]);
        assert_eq!(
            infer_moves(&mid, 1, &after(&["f5", "d6", "c3", "d3", "c4"])),
            Some(vec![sq("c3"), sq("d3"), sq("c4")])
        );
        assert_eq!(
            infer_moves(
                &mid,
                1,
                &after(&["f5", "d6", "c3", "d3", "c4", "f4", "f6", "f3"])
            ),
            Some(vec![
                sq("c3"),
                sq("d3"),
                sq("c4"),
                sq("f4"),
                sq("f6"),
                sq("f3")
            ])
        );
        let mut odd = after(&["f5"]);
        odd[sq("a1")] = 1;
        assert_eq!(infer_moves(&after(&[]), 1, &odd), None);
        assert_eq!(infer_moves(&mid, 1, &mid), Some(vec![]));
    }

    #[test]
    fn a_finished_leg_is_replayed_from_its_moves() {
        let game = "d3c5f6e3b5g7e6e7f7f8f5f4g6c4d6c3g3b6h8g4b4c6g8g5e8a6f3c7h5h4\
                    h3a5b8g2f2d7h1h2e2b7a8a7d8h6h7c8d2d1b3a4e1c1a3c2b1b2a1a2f1g1";
        let (cells, last) = final_board(game).unwrap();
        let count = |c| cells.iter().filter(|&&x| x == c).count();
        assert_eq!((count(1), count(2), last), (45, 19, Some(sq("g1"))));
        assert_eq!(final_board(&game[..game.len() - 2]), None, "not over yet");
        assert_eq!(final_board("f5f5"), None);
    }
}
