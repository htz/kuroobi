//! The FlipSight bot API: JSON actions on one endpoint, and an NDJSON event stream.

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

    /// Reads the event stream until `stop`, reconnecting from 1 s up to 30 s apart.
    /// Returns early only when the token is refused.
    pub fn stream(
        &self,
        stop: &Arc<AtomicBool>,
        mut on_event: impl FnMut(&Value),
    ) -> Result<(), String> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(40))
            .build();
        let url = format!("{}/flipsight_api/bot-stream", self.base);
        let mut delay = 1;
        while !stop.load(Ordering::Relaxed) {
            match agent.get(&url).set("Authorization", &self.auth()).call() {
                Ok(r) => {
                    delay = 1;
                    for line in std::io::BufReader::new(r.into_reader()).lines() {
                        let Ok(line) = line else { break };
                        if stop.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                            on_event(&v);
                        }
                    }
                }
                Err(ureq::Error::Status(401, _)) => return Err("invalid bot credential".into()),
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_secs(delay));
            delay = (delay * 2).min(30);
        }
        Ok(())
    }
}

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
    #[serde(rename = "move")]
    pub mv: i64,
}
