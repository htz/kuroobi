//! Playing games on a clock: an engine on its own thread per game, shared by every client
//! (the GUI's GGS client, the CLI players, and bots for other servers).

use std::time::{Duration, Instant};

use kuroobi::engine::{Engine, EngineConfig};
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
    NewGame,
    SetUseBook(bool),
    SetThreads(usize),
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
        let engine = Engine::new(cfg)?;
        let stop = engine.stop_handle();
        let progress = engine.progress();
        let (jtx, jrx) = std::sync::mpsc::channel::<Job>();
        let (dtx, drx) = std::sync::mpsc::channel::<Done>();
        std::thread::spawn(move || {
            let mut engine = engine;
            while let Ok(job) = jrx.recv() {
                match job {
                    Job::Think { board, clock, hint } => {
                        if let Some(h) = hint {
                            engine.hint_move(&board, h);
                        }
                        let nf0 = kuroobi::engine::NON_FINITE_VALUES
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let n0 = engine.nodes();
                        let mv = if fixed_level {
                            engine.choose_within(&board, None)
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
                    Job::NewGame => engine.clear_tables(),
                    Job::SetUseBook(b) => engine.set_use_book(b),
                    Job::SetThreads(n) => engine.set_threads(n),
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
