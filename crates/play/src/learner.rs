//! Learning finished games into the book: one engine and one queue per process, whichever client
//! played the game, so the learned book has a single writer and every engine re-reads it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::learn::BackupOutcome;

/// The depth each position of a learned game is searched again at.
pub const LEARN_DEPTH: u32 = 18;

/// After a hold lifts, learning waits this long before it takes the CPU back.
const YIELD_HOLD: Duration = Duration::from_millis(1500);

pub type LearnDone = Box<dyn FnOnce(Result<BackupOutcome, String>) + Send>;

pub struct LearnTask {
    /// The position the moves start from (`Board::from_string`), or the standard start.
    pub start: Option<String>,
    pub kifu: String,
    /// Called on the learner's thread once the game is in the book, or could not be.
    pub done: LearnDone,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct LearnStatus {
    /// Positions of the game being learned that are done, out of `total`.
    pub done: u32,
    pub total: u32,
    /// Waiting for a game somewhere to end.
    pub paused: bool,
    /// Games waiting behind it.
    pub queued: usize,
}

type Probe = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Default)]
struct State {
    queue: VecDeque<(EngineConfig, LearnTask)>,
    holds: HashMap<String, Probe>,
    status: Option<LearnStatus>,
    running: bool,
}

pub struct Learner {
    state: Mutex<State>,
    wake: Condvar,
}

/// The process's learner; its thread and engine start with the first game queued.
pub fn learner() -> &'static Learner {
    static LEARNER: OnceLock<Learner> = OnceLock::new();
    LEARNER.get_or_init(|| Learner {
        state: Mutex::new(State::default()),
        wake: Condvar::new(),
    })
}

impl Learner {
    /// Queues a game; returns the positions it will search. `cfg` builds the learning engine if
    /// none runs yet (its book and weights files and its threads).
    pub fn submit(&'static self, cfg: EngineConfig, task: LearnTask) -> Result<usize, String> {
        let n =
            kuroobi::learn::BackupJob::new(task.start.as_deref(), &task.kifu, LEARN_DEPTH as u8)?
                .remaining();
        let mut s = self.state.lock().unwrap();
        s.queue.push_back((cfg, task));
        if !s.running {
            s.running = true;
            std::thread::spawn(move || self.run());
        }
        self.wake.notify_all();
        Ok(n)
    }

    /// While `who` holds, learning waits: a game is being played or searched there.
    pub fn hold(&self, who: &str, on: bool) {
        if on {
            self.hold_while(who, Arc::new(|| true));
        } else {
            self.state.lock().unwrap().holds.remove(who);
        }
    }

    /// As [`Learner::hold`], asked each time learning would take a step.
    pub fn hold_while(&self, who: &str, probe: Probe) {
        self.state
            .lock()
            .unwrap()
            .holds
            .insert(who.to_string(), probe);
    }

    pub fn status(&self) -> Option<LearnStatus> {
        self.state.lock().unwrap().status
    }

    fn held(&self) -> bool {
        let probes: Vec<Probe> = self.state.lock().unwrap().holds.values().cloned().collect();
        probes.iter().any(|p| p())
    }

    fn run(&'static self) {
        let mut engine: Option<Engine> = None;
        let mut last_held: Option<Instant> = None;
        loop {
            let (cfg, task) = {
                let mut s = self.state.lock().unwrap();
                loop {
                    if let Some(next) = s.queue.pop_front() {
                        break next;
                    }
                    s.status = None;
                    s = self.wake.wait(s).unwrap();
                }
            };
            if engine.is_none() {
                match Engine::new(cfg) {
                    Ok(e) => engine = Some(e),
                    Err(e) => {
                        (task.done)(Err(e));
                        continue;
                    }
                }
            }
            let eng = engine.as_mut().expect("built above");
            // Another process may have written the file since this engine last read it.
            eng.reload_learned();
            let mut job = match eng.learn_start(task.start.as_deref(), &task.kifu, LEARN_DEPTH) {
                Ok(j) => j,
                Err(e) => {
                    (task.done)(Err(e));
                    continue;
                }
            };
            let total = job.remaining() as u32;
            let result = loop {
                if self.held() {
                    last_held = Some(Instant::now());
                }
                let paused = last_held.is_some_and(|t| t.elapsed() < YIELD_HOLD);
                {
                    let mut s = self.state.lock().unwrap();
                    let queued = s.queue.len();
                    s.status = Some(LearnStatus {
                        done: total.saturating_sub(job.remaining() as u32),
                        total,
                        paused,
                        queued,
                    });
                }
                if paused {
                    std::thread::sleep(Duration::from_millis(300));
                    continue;
                }
                match eng.learn_step(&mut job, LEARN_DEPTH) {
                    Ok(None) => {}
                    Ok(Some(out)) => break Ok(out),
                    Err(e) => break Err(e),
                }
            };
            (task.done)(result);
        }
    }
}
