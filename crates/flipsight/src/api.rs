//! The FlipSight bot API: JSON actions on one endpoint, and an NDJSON event stream.

use std::collections::VecDeque;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

pub struct Api {
    base: String,
    token: String,
    agent: ureq::Agent,
}

/// An answer the server gave (`status` is the HTTP status), or why none came.
pub enum Reply {
    Ok(Value),
    Refused { status: u16, error: String },
    Failed(String),
}

impl Api {
    pub fn new(base: &str, token: String) -> Api {
        Api {
            base: base.trim_end_matches('/').to_string(),
            token,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(15))
                .build(),
        }
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    pub fn call(&self, action: &str, mut body: serde_json::Map<String, Value>) -> Reply {
        body.insert("action".into(), Value::from(action));
        let url = format!("{}/flipsight/bot_match_api.php", self.base);
        let res = self
            .agent
            .post(&url)
            .set("Authorization", &self.auth())
            .set("Content-Type", "application/json")
            .send_string(&Value::Object(body).to_string());
        let (status, text) = match res {
            Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
            Err(e) => return Reply::Failed(e.kind().to_string()),
        };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status == 200 && v["ok"] == Value::Bool(true) {
            Reply::Ok(v)
        } else {
            Reply::Refused {
                status,
                error: v["error"].as_str().unwrap_or("no error text").to_string(),
            }
        }
    }

    /// Our finished games from `since` (ms) on, oldest first, at most 300: the site's records, one
    /// per line of its answer.
    pub fn games(&self, since: i64) -> Result<Vec<Value>, String> {
        let body = serde_json::json!({"action": "agent_games", "since": since, "max": 300});
        let url = format!("{}/flipsight/bot_match_api.php", self.base);
        let res = self
            .agent
            .post(&url)
            .set("Authorization", &self.auth())
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        let text = match res {
            Ok(r) => r.into_string().map_err(|e| e.kind().to_string())?,
            Err(ureq::Error::Status(code, r)) => {
                let v: Value = serde_json::from_str(&r.into_string().unwrap_or_default())
                    .unwrap_or(Value::Null);
                return Err(format!("{code}: {}", v["error"].as_str().unwrap_or("-")));
            }
            Err(e) => return Err(e.kind().to_string()),
        };
        Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .filter(|v| v["id"].is_i64() && v["legs"].is_array())
            .collect())
    }

    /// The `state` event a fresh stream begins with, then hangs up: who a token belongs to.
    /// It closes any stream the token already has, so it is for bots that are not running.
    pub fn first_state(&self) -> Reply {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(10))
            .build();
        let url = format!("{}/flipsight_api/bot-stream", self.base);
        match agent.get(&url).set("Authorization", &self.auth()).call() {
            Ok(r) => {
                for line in std::io::BufReader::new(r.into_reader()).lines().take(8) {
                    let Ok(line) = line else { break };
                    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                        continue;
                    };
                    if v["type"] == "state" {
                        return if v["ok"] == Value::Bool(false) {
                            Reply::Failed("state unavailable".into())
                        } else {
                            Reply::Ok(v)
                        };
                    }
                }
                Reply::Failed("no state".into())
            }
            Err(ureq::Error::Status(status, _)) => Reply::Refused {
                status,
                error: "stream refused".into(),
            },
            Err(e) => Reply::Failed(e.kind().to_string()),
        }
    }

    /// Reads the event stream until `stop`, reconnecting from 1 s up to 30 s apart.
    /// Returns early only when the token is refused.
    pub fn stream(&self, stop: &Arc<AtomicBool>, mut on: impl FnMut(Wire)) -> Result<(), String> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(STREAM_QUIET)
            .build();
        let url = format!("{}/flipsight_api/bot-stream", self.base);
        let mut delay = 1;
        let mut opened: VecDeque<Instant> = VecDeque::new();
        while !stop.load(Ordering::Relaxed) {
            while opened
                .front()
                .is_some_and(|t| t.elapsed() >= CONNECT_WINDOW)
            {
                opened.pop_front();
            }
            if opened.len() >= MAX_CONNECTS {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            let began = Instant::now();
            opened.push_back(began);
            let why = match agent.get(&url).set("Authorization", &self.auth()).call() {
                Ok(r) => {
                    let mut why = "closed".to_string();
                    for line in std::io::BufReader::new(r.into_reader()).lines() {
                        let line = match line {
                            Ok(l) => l,
                            Err(e) => {
                                why = e.kind().to_string();
                                break;
                            }
                        };
                        if stop.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
                            continue;
                        };
                        let unavailable = v["type"] == "state" && v["ok"] == Value::Bool(false);
                        on(Wire::Event(v));
                        if unavailable {
                            why = "state unavailable".into();
                            break;
                        }
                    }
                    why
                }
                Err(ureq::Error::Status(401, _)) => return Err("invalid bot credential".into()),
                Err(ureq::Error::Status(code, _)) => format!("HTTP {code}"),
                Err(e) => e.kind().to_string(),
            };
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            on(Wire::Down(why));
            if began.elapsed() >= Duration::from_secs(60) {
                delay = 1;
            }
            std::thread::sleep(Duration::from_secs(delay));
            delay = (delay * 2).min(30);
        }
        Ok(())
    }
}

/// What the stream hands on: one event, or that the connection ended and why.
pub enum Wire {
    Event(Value),
    Down(String),
}

/// The server pings every 10 s; no line for 15 s means the stream is stuck.
const STREAM_QUIET: Duration = Duration::from_secs(15);

/// The site leaves a bot out of automatic matching past 10 connections in 10 minutes.
const MAX_CONNECTS: usize = 9;
const CONNECT_WINDOW: Duration = Duration::from_secs(600);

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Match {
    pub id: i64,
    pub kind: Option<String>,
    pub bot_a: i64,
    pub bot_b: i64,
    pub game_mode: String,
    pub status: String,
    pub state: Option<State>,
    pub round_no: Option<i64>,
    pub room_created_at: Option<String>,
    pub rating_before: Value,
    pub rating_after: Value,
    /// The finished game from our side, with the event that ends the match.
    pub record: Option<Value>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct State {
    pub game_mode: String,
    pub board_size: u32,
    pub legs: Vec<Leg>,
    pub totals: Vec<i64>,
    pub winner: Option<i64>,
    pub finish_reason: Option<String>,
    /// The moves before the legs split (a1 = 0, b1 = 1, ...), when the match starts from an opening.
    pub opening_moves: Vec<i64>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Leg {
    pub board: Vec<u8>,
    pub turn: u8,
    pub black_participant: u8,
    /// Black's and white's time left (ms) as of `turn_started_ms`.
    pub clocks_ms: Vec<f64>,
    pub turn_started_ms: i64,
    pub revision: i64,
    pub status: String,
    pub legal_moves: Vec<i64>,
    pub discs: Vec<i64>,
    pub result_discs: Option<Vec<i64>>,
    /// This leg's moves after the opening; -1 is a pass.
    pub history: Vec<Played>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct Played {
    pub color: u8,
    #[serde(rename = "move")]
    pub mv: i64,
    /// What the mover sent with the move, in discs from its own side.
    pub eval: Option<f64>,
}
