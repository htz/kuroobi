//! Playing games on a clock: an engine on its own thread per game, shared by every client
//! (the GUI's GGS client, the CLI players, and bots for other servers).

use std::time::{Duration, Instant};

mod learner;
pub use learner::{learner, LearnDone, LearnStatus, LearnTask, LEARN_DEPTH};

use kuroobi::engine::{Engine, EngineAssets, EngineConfig};
use kuroobi::{Board, Position};

/// An engine on its own thread, taking one job at a time; `mid` is the game it is serving.
pub struct EngineWorker {
    pub tx: std::sync::mpsc::Sender<Job>,
    pub rx: std::sync::mpsc::Receiver<Done>,
    pub stop: kuroobi::midgame::StopHandle,
    pub busy: bool,
    pub mid: Option<String>,
    pub pending_hash: u64,
    pub pending: Option<Pending>,
    pub sent_at: Option<(Instant, Duration)>,
    pub stopped_at: Option<Instant>,
    pub progress: std::sync::Arc<kuroobi::engine::Progress>,
    pub threads: usize,
    pub sent_ms: u64,
    pub sent_empties: u8,
    /// The position (a caller's hash of it) a ponder was sent for and not yet stopped.
    pub pondering: Option<u64>,
    /// One for every ponder that ran, as it ends.
    pub pondered: std::sync::mpsc::Receiver<Pondered>,
}

pub struct Pondered {
    pub secs: f64,
    pub threads: usize,
    pub empties: u8,
    pub nodes: u64,
}

pub const WORKER_GIVE_UP: Duration = Duration::from_secs(10);

pub struct Pending {
    pub board: Board,
    pub clock: Duration,
    pub hash: u64,
    pub hint: Option<Position>,
    pub queued: Instant,
}

pub enum Job {
    Think {
        board: Board,
        clock: Duration,
        hint: Option<Position>,
    },
    /// Searches the opponent's position while it thinks, for up to `span` or until `stop`; no
    /// `Done` comes back. The stop is cleared once it ends, so a stop meant for it never reaches
    /// the next job.
    Ponder {
        board: Board,
        span: Duration,
    },
    NewGame,
    SetUseBook(bool),
    SetThreads(usize),
    /// Plays every move at this depth and solve instead of by the clock, still within an even
    /// share of the clock; `None` goes back to how the worker was started.
    SetLevel(Option<(u32, u8)>),
}

pub struct Report {
    pub nodes: u64,
    pub pv: Vec<Option<Position>>,
}

/// Edax's log shows eight plies.
pub const PV_LEN: usize = 8;

pub fn report(
    engine: &kuroobi::engine::Engine,
    board: &Board,
    mv: &kuroobi::engine::MoveEval,
    nodes: u64,
) -> Report {
    let pv = match mv.pos {
        Some(p) if !mv.from_book => engine.principal_variation(board, p, PV_LEN),
        _ => vec![mv.pos],
    };
    Report { nodes, pv }
}

pub struct Done(pub Box<kuroobi::engine::MoveEval>, pub Report);

impl EngineWorker {
    /// Starts the engine thread with `cfg` as given (resources and table sizes already resolved).
    /// `fixed_level` plays every move at the configured depth instead of by the clock.
    pub fn spawn(cfg: EngineConfig, fixed_level: bool) -> Result<EngineWorker, String> {
        let assets = EngineAssets::load(&cfg)?;
        EngineWorker::spawn_with(assets, cfg, fixed_level)
    }

    /// As [`EngineWorker::spawn`], on weights already loaded: many workers share one copy, and
    /// each after the first starts in a fraction of a second instead of seconds.
    pub fn spawn_with(
        assets: EngineAssets,
        cfg: EngineConfig,
        fixed_level: bool,
    ) -> Result<EngineWorker, String> {
        let mut threads = cfg.threads;
        let engine = Engine::with_assets(assets, cfg)?;
        let stop = engine.stop_handle();
        let progress = engine.progress();
        let (jtx, jrx) = std::sync::mpsc::channel::<Job>();
        let (dtx, drx) = std::sync::mpsc::channel::<Done>();
        let (ptx, prx) = std::sync::mpsc::channel::<Pondered>();
        std::thread::spawn(move || {
            let mut engine = engine;
            let mut fixed = fixed_level;
            let mut within_clock = false;
            let mut queued: Option<Job> = None;
            loop {
                let job = match queued.take() {
                    Some(j) => j,
                    None => match jrx.recv() {
                        Ok(j) => j,
                        Err(_) => break,
                    },
                };
                match job {
                    Job::Think { board, clock, hint } => {
                        if let Some(h) = hint {
                            engine.hint_move(&board, h);
                        }
                        let nf0 = kuroobi::engine::NON_FINITE_VALUES
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let n0 = engine.nodes();
                        let mv = if fixed {
                            let share = (board.empty_count() as u32).div_ceil(2).max(1);
                            let deadline = within_clock.then(|| Instant::now() + clock / share);
                            engine.choose_within(&board, deadline)
                        } else {
                            engine.play_clock(&board, clock)
                        };
                        let nodes = engine.nodes() - n0;
                        let nf1 = kuroobi::engine::NON_FINITE_VALUES
                            .load(std::sync::atomic::Ordering::Relaxed);
                        if nf1 != nf0 {
                            eprintln!(
                                "!! search returned a non-finite value (empties {} depth {} cut {})",
                                board.empty_count(),
                                mv.depth,
                                mv.cut
                            );
                        }
                        engine.progress().clear();
                        let r = report(&engine, &board, &mv, nodes);
                        if dtx.send(Done(Box::new(mv), r)).is_err() {
                            return;
                        }
                    }
                    Job::Ponder { board, span } => {
                        // A job behind it (our move, or a newer ponder) means this one is stale:
                        // the stop that was meant for it may already be spent on the one before.
                        match jrx.try_recv() {
                            Ok(next) => queued = Some(next),
                            Err(_) => {
                                let (t0, n0) = (Instant::now(), engine.nodes());
                                engine.ponder(&board, span);
                                let _ = ptx.send(Pondered {
                                    secs: t0.elapsed().as_secs_f64(),
                                    threads,
                                    empties: board.empty_count(),
                                    nodes: engine.nodes() - n0,
                                });
                            }
                        }
                        engine.stop_handle().reset();
                    }
                    Job::NewGame => engine.clear_tables(),
                    Job::SetUseBook(b) => engine.set_use_book(b),
                    Job::SetThreads(n) => {
                        threads = n.max(1);
                        engine.set_threads(n);
                    }
                    Job::SetLevel(Some((depth, solve))) => {
                        engine.set_levels(depth, solve);
                        (fixed, within_clock) = (true, true);
                    }
                    Job::SetLevel(None) => (fixed, within_clock) = (fixed_level, false),
                }
            }
        });
        Ok(EngineWorker {
            tx: jtx,
            rx: drx,
            stop,
            busy: false,
            mid: None,
            pending_hash: 0,
            pending: None,
            sent_at: None,
            stopped_at: None,
            progress,
            threads: 0,
            sent_ms: 0,
            sent_empties: 0,
            pondering: None,
            pondered: prx,
        })
    }

    pub fn send(&mut self, job: Job) {
        let counts = matches!(job, Job::Think { .. });
        if self.tx.send(job).is_ok() && counts {
            self.busy = true;
        }
    }
}

pub fn confidence_pct(t: f32) -> u32 {
    // Abramowitz-Stegun 7.1.26 for erf(t / sqrt 2): the two-sided normal coverage.
    let x = f64::from(t) / std::f64::consts::SQRT_2;
    let k = 1.0 / (1.0 + 0.3275911 * x);
    let poly = k
        * (0.254829592
            + k * (-0.284496736 + k * (1.421413741 + k * (-1.453152027 + k * 1.061405429))));
    ((1.0 - poly * (-x * x).exp()) * 100.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probcut_margins_read_as_their_confidence() {
        assert_eq!([1.5, 2.0, 2.5].map(confidence_pct), [87, 95, 99]);
    }
}
