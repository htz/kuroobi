//! Background jobs (the data repair) stopped while a match runs, so the engine has the CPU.

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

pub struct Pauser {
    pattern: String,
    log: PathBuf,
    ggs_wire: PathBuf,
    stopped: BTreeSet<i32>,
}

impl Pauser {
    pub fn new(pattern: &str, log: PathBuf, ggs_wire: PathBuf) -> Pauser {
        Pauser {
            pattern: pattern.to_string(),
            log,
            ggs_wire,
            stopped: BTreeSet::new(),
        }
    }

    fn pids(&self) -> Vec<i32> {
        if self.pattern.is_empty() {
            return Vec::new();
        }
        let out = std::process::Command::new("pgrep")
            .args(["-x", &self.pattern])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let me = std::process::id() as i32;
        out.split_whitespace()
            .filter_map(|s| s.parse().ok())
            .filter(|&p| p != me)
            .collect()
    }

    fn note(&self, msg: &str) {
        crate::log(msg);
        let line = format!("{} {msg}", crate::clock_text());
        ggs::env::append_line(&self.log, &line);
    }

    /// Stops the jobs while `on`; resumes the ones it stopped once off, unless a GGS game holds them.
    pub fn hold(&mut self, on: bool) {
        if on {
            let new: Vec<i32> = self
                .pids()
                .into_iter()
                .filter(|p| !self.stopped.contains(p))
                .filter(|&p| unsafe { libc::kill(p, libc::SIGSTOP) } == 0)
                .collect();
            if !new.is_empty() {
                self.stopped.extend(&new);
                self.note(&format!("paused for a FlipSight match: {new:?}"));
            }
        } else if !self.stopped.is_empty() && !self.ggs_playing() {
            for &p in &self.stopped {
                unsafe { libc::kill(p, libc::SIGCONT) };
            }
            let all: Vec<i32> = self.stopped.iter().copied().collect();
            self.note(&format!("resumed after FlipSight: {all:?}"));
            self.stopped.clear();
        }
    }

    /// The last kuroobi match line of the GGS wire log opens a match.
    fn ggs_playing(&self) -> bool {
        let Ok(mut f) = std::fs::File::open(&self.ggs_wire) else {
            return false;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(2_000_000)));
        let mut buf = Vec::new();
        let _ = f.read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf);
        text.lines()
            .rfind(|l| {
                (l.contains("/os: + match") || l.contains("/os: - match"))
                    && l.contains(" kuroobi ")
            })
            .is_some_and(|l| l.contains("/os: + match"))
    }
}
