//! Midgame NNUE search — the engine that plays matches.
//!
//! Extracted from the match harness (`lab`) so that
//! other binaries (the GGS client, future frontends) can drive the same
//! engine. Structure: iterative deepening + Lazy SMP at
//! shallow depths, YBWC splits below, Multi-ProbCut with unpruned probes and
//! a static-eval gate, and a 4-way shared transposition table.

// 添字ループは走査順そのものが意味を持つ (連続領域の走査・SIMD 的な
// 展開) ため、イテレータ化の助言は採らない。引数の多い探索関数も、
// 構造体に束ねると呼び出しごとの構築が入るので現状の形を保つ。
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]

use crate::nnue::Nnue;
use crate::pattern_index::PatternIndices;
use crate::zobrist;
use crate::{Board, Position};

const PVS_EPS: f32 = 0.01;

/// One ordered child: move, its flip mask (reused so the search never
/// recomputes the flips), and the ordering key.
type Kid = (Position, u64, f32);

/// Upper bound on legal moves in a Reversi position.
const MAX_KIDS: usize = 34;

/// From this remaining depth upward, order children by a full NNUE eval;
/// shallower nodes use cheap mobility ordering (see `ordered`).
/// Tunable so it can be swept: work below `ybwc_min_depth` cannot be split, so
/// in a parallel search a second of it costs as much wall-clock as six seconds
/// of splittable work. The sequential optimum is not the parallel optimum.
fn eval_order_depth() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EVAL_ORDER_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3)
    })
}

/// A worker whose subtree became irrelevant returns this instead of a score,
/// so the caller knows to discard it rather than treat it as an evaluation.
const ABORTED: f32 = f32::NEG_INFINITY;

/// Nodes between abort checks: the flag is shared, so reading it every node
/// would put a contended load in the hot path.
/// Below `ybwc_min_depth` a subtree is cheaper to
/// search than to hand off.
/// How much each step of `mpc_relax` widens the ProbCut margin.
const MPC_RELAX_STEP: f32 = 1.18;

/// 反復深化で「次の段」が前の段の何倍かかるかの見積もり。リバーシの中盤は
/// 分岐が 8〜12 で、置換表と手順付けが効くぶん実測はこれより小さい。
/// 大きく見るほど早めに切り上げる (時間切れで段を丸ごと捨てるより得)。
const NEXT_PASS_FACTOR: f32 = 3.0;

/// Minimum remaining depth for a YBWC split. Overridable so the split can be
/// switched off (set it above the search depth) when isolating which half of
/// the parallel scheme costs strength.
fn ybwc_min_depth() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("YBWC_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(6)
    })
}
const ABORT_CHECK_INTERVAL: u64 = 512;

/// Multi-ProbCut: from this remaining depth up, a shallow search decides
/// whether the node lies far enough outside the window to skip entirely.
/// Strong engines prune this way at deep settings, so a fair deep comparison
/// needs it on both sides.
const MPC_MIN_DEPTH: u32 = 4;

/// Confidence in standard deviations of the shallow search's prediction error.
/// The same knob is often expressed as a selectivity percentage (74% ~ 1.13σ).
const MPC_T: f32 = 1.1;

/// Experiment overrides (strength decomposition): `MPC_T` rescales the margin,
/// `MPC_MIN_DEPTH` moves the depth ProbCut starts firing at. Defaults above.
pub fn mpc_t() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MPC_T")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MPC_T)
    })
}

pub fn mpc_min_depth() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MPC_MIN_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MPC_MIN_DEPTH)
    })
}

/// Static-eval gate offset, in discs.
const MPC_D0_OFFSET: f32 = 4.0;

/// `MPC_OLD=1` restores the pre-2026-07-30 ProbCut: pruned probes, no
/// static-eval gate, both sides probed.
fn mpc_old() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("MPC_OLD").is_ok_and(|v| v != "0"))
}

/// How deeply ProbCut may nest. A single level leaves the deep tree barely
/// pruned; letting the probe itself prune is what makes MPC "multi".
const MPC_MAX_LEVEL: u32 = 3;

/// Probe depth for a node at `depth`, mirroring the linear searcher's rule
/// (keeps the parity of `depth`, which matters because odd and even plies
/// evaluate from opposite sides).
pub fn mpc_reduced_depth(depth: u32) -> u32 {
    2 * (depth / 4) + (depth & 1)
}

/// Standard deviation of the error between a `pc_depth` search and a `depth`
/// search at `empties` empties. Fitted from measurements for this pattern
/// evaluator (see `search::mpc_sigma`); the NNUE output is in the same
/// disc-difference units, and it is *more* accurate, so this is a safe
/// (slightly conservative) model to prune against.
fn mpc_sigma(empties: u32, depth: u32, pc_depth: u32) -> f32 {
    const A: f32 = -0.068941;
    const B: f32 = 0.368775;
    const C: f32 = -0.713476;
    const QA: f32 = 0.010223;
    const QB: f32 = 0.647219;
    const QC: f32 = 4.050545;
    let s = A * empties as f32 + B * depth as f32 + C * pc_depth as f32;
    QA * s * s + QB * s + QC
}

/// Confidence to read the rest of the game with, or `None` to search the
/// midgame instead. The schedule, anchored at an exact solve from 24 empties,
/// reads to the end at 93% from 30 empties, 98% from 28, 99% from 26
/// and exactly from 24. The sigma multipliers are the two-sided z-scores for
/// those percentages, the same scale `MPC_T` uses.
pub fn selective_band(empties: u8, solve_empties: u8, width: u8) -> Option<f32> {
    if empties <= solve_empties || empties > solve_empties + width {
        return None;
    }
    match empties - solve_empties {
        1..=2 => Some(2.58), // 99%
        3..=4 => Some(2.33), // 98%
        5..=6 => Some(1.81), // 93%
        _ => None,
    }
}

/// Static square priority for cheap ordering: corner, box, block, then
/// X and C squares. The four centre squares are missing from the union, which is
/// safe — they are occupied from the opening position on and can never be a
/// legal move.
const STATIC_CELL_PRIORITY: [u64; 4] = [
    0x8100000000000081, // corner
    0x00003C24243C0000, // box
    0x3C3CC3C3C3C33C3C, // block
    0x42C300000000C342, // X, C
];

/// Corner mask: a legal move into a corner counts one
/// extra in the weighted mobility.
const CORNERS: u64 = 0x8100000000000081;

/// Potential mobility: empty squares next to an opponent disc.
/// A move that leaves the opponent few of those is good even when it does not
/// reduce their legal moves yet.
#[inline]
fn potential_mobility(discs: u64, empties: u64) -> u32 {
    let hmask = discs & 0x7E7E7E7E7E7E7E7E;
    let vmask = discs & 0x00FFFFFFFFFFFF00;
    let hvmask = discs & 0x007E7E7E7E7E7E00;
    let res = (hmask << 1)
        | (hmask >> 1)
        | (vmask << 8)
        | (vmask >> 8)
        | (hvmask << 7)
        | (hvmask >> 7)
        | (hvmask << 9)
        | (hvmask >> 9);
    (res & empties).count_ones()
}

/// How many tasks may be queued beyond the workers that are idle right now.
/// Measured here (d16, 20 games, min of 3):
/// 0 -> 2.25x, 2 -> 2.39x, 8 -> 2.43x, 32 -> 2.20x. Two is the robust choice —
/// it is the best of the four at 8 threads and within noise of the best at 6.
fn pool_slack() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("POOL_SLACK")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2)
    })
}

/// Slots probed per position. Four
/// 16-byte entries are exactly one 64-byte cache line, so the whole probe is a
/// single memory transaction.
const TT_WAYS: usize = 4;

/// How valuable an entry is: depth first,
/// accuracy as the tie-break.
#[inline]
fn tt_level(depth: u8, relax: u8) -> u32 {
    ((depth as u32) << 8) | relax as u32
}

/// A transposition table shared by the root-split workers.
///
/// Writes race, but every entry carries the position key and is compared
/// exactly, so a torn read can only produce a *mismatch* (recomputed), never a
/// wrong value for another position. This is the same argument the linear
/// searcher's shared table relies on.
pub struct SharedTt {
    buckets: std::cell::UnsafeCell<Vec<TtBucket>>,
    mask: u64,
}
// SAFETY: see the note above — entries are self-validating.
unsafe impl Sync for SharedTt {}

/// One cache line of the table. Probing *consecutive*
/// slots from the hash would straddle two lines whenever the
/// hash lands near a boundary — every probe then costs two memory transactions
/// instead of one, and every store dirties two lines that other threads may
/// hold. Aligning the group instead makes the whole probe one line.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct TtBucket([TtEntry; TT_WAYS]);

impl SharedTt {
    pub fn new(bits: u32) -> SharedTt {
        let n = 1usize << bits.saturating_sub(2);
        SharedTt {
            buckets: std::cell::UnsafeCell::new(vec![TtBucket([TtEntry::EMPTY; TT_WAYS]); n]),
            mask: (n - 1) as u64,
        }
    }

    // 共有置換表は「&self から可変参照を返す」構造そのものが設計。競合は
    // キー不一致 (= ミス) にしかならず、ロックの代わりにそれを許容している。
    #[allow(clippy::mut_from_ref)]
    #[inline]
    fn bucket(&self, hash: u64) -> &mut TtBucket {
        // SAFETY: index is masked into range; a racing write can only make the
        // key mismatch, which the caller treats as a miss.
        unsafe {
            let v = &mut *self.buckets.get();
            v.get_unchecked_mut((hash & self.mask) as usize)
        }
    }

    #[allow(clippy::mut_from_ref)]
    #[inline]
    fn slot(&self, hash: u64, i: u64) -> &mut TtEntry {
        // SAFETY: `i` is always below TT_WAYS.
        unsafe { self.bucket(hash).0.get_unchecked_mut(i as usize) }
    }

    /// Scan the bucket for this position.
    #[inline]
    fn get(&self, hash: u64) -> TtEntry {
        let b = self.bucket(hash);
        for e in b.0.iter() {
            if e.key == hash && e.flag != 0 {
                return *e;
            }
        }
        TtEntry::EMPTY
    }

    /// Store: walk the bucket and take the first slot that is worth
    /// no more than what is being stored, where worth is `(depth, accuracy)`.
    /// If all three hold deeper or more accurate results, store nothing.
    ///
    /// A one-way table cannot express that: every store evicts whatever was
    /// there, so a shallow probe near a leaf throws away the deep entry the
    /// iteration above it will ask for. Under six threads the shallow stores
    /// arrive from every worker at once and the deep entries barely survive.
    #[inline]
    /// `alpha` / `beta` are the window the node *started* with, not the one the
    /// table narrowed it to. Registering against the narrowed window would claim
    /// a bound the search never established.
    #[allow(clippy::too_many_arguments)]
    fn put(
        &self,
        hash: u64,
        depth: u8,
        relax: u8,
        alpha: f32,
        beta: f32,
        value: f32,
        best_move: u8,
    ) {
        let level = tt_level(depth, relax);
        let upper = if value < beta { value } else { f32::INFINITY };
        let lower = if value > alpha {
            value
        } else {
            f32::NEG_INFINITY
        };
        for i in 0..TT_WAYS {
            let slot = self.slot(hash, i as u64);
            if slot.key == hash && slot.flag != 0 {
                let slot_level = tt_level(slot.depth, slot.relax);
                if slot_level > level {
                    return;
                }
                if slot_level == level {
                    // Same level: the same node searched again
                    // through a different window, so the two bounds combine —
                    // *and each assignment repairs the other end if it would
                    // cross it*. ProbCut makes every stored value probabilistic
                    // and two threads at the same level can disagree, so
                    // `lower > upper` really does happen; leaving those repair
                    // clauses out cost 20% more nodes than a single tagged
                    // value, which is what made this look like a bad idea the
                    // first time it was tried.
                    if value < beta && value < slot.upper {
                        slot.upper = value;
                        if value > alpha && value < slot.lower {
                            slot.lower = value;
                        }
                    }
                    if value > alpha && slot.lower < value {
                        slot.lower = value;
                        if value < beta && slot.upper < value {
                            slot.upper = value;
                        }
                    }
                } else {
                    // Deeper result: the old bounds no
                    // longer apply and are replaced outright.
                    slot.lower = lower;
                    slot.upper = upper;
                    slot.depth = depth;
                    slot.relax = relax;
                }
                if value > alpha && best_move < 64 && slot.best != best_move {
                    slot.best2 = slot.best;
                    slot.best = best_move;
                }
                return;
            }
        }
        for i in 0..TT_WAYS {
            let slot = self.slot(hash, i as u64);
            if slot.flag == 0 || tt_level(slot.depth, slot.relax) <= level {
                *slot = TtEntry {
                    key: hash,
                    lower,
                    upper,
                    // The move is dropped when the search
                    // failed low, since nothing was proven about it. That was
                    // ruinous while the ordering fell back to bit order without
                    // a table move; with the scored ordering pass in place it
                    // costs only the head start.
                    best: best_move,
                    best2: 64,
                    depth,
                    flag: 1,
                    relax,
                };
                return;
            }
        }
    }

    pub fn clear(&self) {
        // SAFETY: called between games, with no workers running.
        unsafe {
            for b in (*self.buckets.get()).iter_mut() {
                for e in b.0.iter_mut() {
                    e.flag = 0;
                }
            }
        }
    }
}

/// One transposition-table slot for the NNUE search. `flag`: 0 empty,
/// 1 exact, 2 lower bound, 3 upper bound.
#[derive(Clone, Copy)]
struct TtEntry {
    key: u64,
    /// The entry keeps a **pair of bounds** rather than one value
    /// plus a lower/upper/exact tag. A tagged value answers only half the
    /// questions asked of it: a node that failed low here leaves an upper bound,
    /// and the next visit — arriving with a window that needs a lower bound —
    /// learns nothing and searches again. Both ends accumulate into
    /// the same entry over successive visits and the table hands out
    /// whichever end the caller can use.
    lower: f32,
    upper: f32,
    best: u8,
    /// The move the current best displaced, kept as the
    /// runner-up for ordering.
    best2: u8,
    depth: u8,
    flag: u8,
    /// How *accurately* this entry was searched: 0 is the main thread's
    /// selectivity, higher means a wider ProbCut margin and therefore fewer
    /// cuts. The table refuses
    /// to hand out bounds unless `(depth, relax)` both meet what the asking
    /// node needs. Without that gate a helper's aggressive cut silently becomes
    /// the main search's answer — which reads as a speedup (fewer nodes) but is
    /// really a strength loss.
    relax: u8,
}

impl TtEntry {
    const EMPTY: TtEntry = TtEntry {
        key: 0,
        lower: f32::NEG_INFINITY,
        upper: f32::INFINITY,
        best: 64,
        best2: 64,
        depth: 0,
        flag: 0,
        relax: 0,
    };
}

/// How often a split was attempted and how often the pool accepted it. A large
/// gap means the workers are saturated; a small `SPLIT_TRIED` means the search
/// is not offering them work in the first place.
pub static SPLIT_TRIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nanoseconds pool workers spent inside tasks. Against (workers x our search
/// time) this is worker utilisation: it separates "the workers are idle" from
/// "the workers are busy but contending".
pub static WORKER_BUSY_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static POOL_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static SPLIT_DONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nanoseconds a splitting thread spent inside `Pool::wait` with nothing left
/// to steal, split by who was waiting. This is the part of the machine that
/// YBWC is *paying* for and not using: the parent is blocked on a child while
/// the queue is empty. Separating it from real search time is the only way to
/// tell "the workers are starved" from "the work is there but redundant".
pub static WAIT_IDLE_MAIN_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static WAIT_IDLE_WORKER_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// How often the pool was seen with k workers busy, sampled on a timer. The
/// averages cannot tell "steadily half loaded" from "alternately saturated and
/// empty", and the two call for opposite fixes: the first needs more split
/// points, the second needs the bursts smoothed out.
pub static BUSY_HIST: [std::sync::atomic::AtomicU64; 17] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 17];
/// Raised only while the NNUE midgame search is running. Without it the sampler
/// also counts the opponent's turns and the endgame solver, where the pool idles
/// by construction — three quarters of the samples, which made the pool look
/// empty when it was simply not in use.
pub static SEARCH_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    static IS_POOL_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SPLIT_TRIED_LOCAL: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A node's own "stop the fan-out"
/// flag chained to every ancestor's. A task carries the whole chain, so an
/// abort raised anywhere above it reaches it — `is_searching` walks the vector.
///
/// Without the chain a split subtree only ever sees the flag of the node that
/// created it: when a grandparent cuts off, the grandchild keeps searching a
/// tree nobody will read, and the parent sits blocked in `wait` for it.
/// 外部 (UI 等) から探索を中断させるためのハンドル。
///
/// 中盤探索・終盤ソルバはどちらもノードごとに参照する。立てた後は
/// `reset` するまで新しい探索も即座に諦めるので、次の探索の前に必ず戻す。
#[derive(Clone, Default)]
pub struct StopHandle(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl StopHandle {
    pub fn new() -> StopHandle {
        StopHandle(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        )))
    }
    /// 進行中の探索を打ち切る。
    pub fn stop(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn reset(&self) {
        self.0.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    #[inline]
    pub fn is_stopped(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

struct AbortChain {
    flag: std::sync::atomic::AtomicBool,
    parent: Option<std::sync::Arc<AbortChain>>,
}

impl AbortChain {
    fn child(parent: Option<std::sync::Arc<AbortChain>>) -> std::sync::Arc<AbortChain> {
        std::sync::Arc::new(AbortChain {
            flag: std::sync::atomic::AtomicBool::new(false),
            parent,
        })
    }
    fn raise(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn stopped(&self) -> bool {
        let mut n = self;
        loop {
            if n.flag.load(std::sync::atomic::Ordering::Relaxed) {
                return true;
            }
            match &n.parent {
                Some(p) => n = p,
                None => return false,
            }
        }
    }
}

/// Where a split-off null-window search leaves its answer.
struct Slot {
    done: std::sync::atomic::AtomicBool,
    bits: std::sync::atomic::AtomicU32,
    nodes: std::sync::atomic::AtomicU64,
}

impl Slot {
    fn new() -> Slot {
        use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
        Slot {
            done: AtomicBool::new(false),
            bits: AtomicU32::new(0),
            nodes: AtomicU64::new(0),
        }
    }
    fn set(&self, v: f32, nodes: u64) {
        use std::sync::atomic::Ordering;
        self.bits.store(v.to_bits(), Ordering::Relaxed);
        self.nodes.store(nodes, Ordering::Relaxed);
        self.done.store(true, Ordering::Release);
    }
}

/// Task pool shared by both forms of parallelism — one pool serves both the
/// Lazy SMP helpers and the YBWC splits.
///
/// The lifetime is the scope the workers were spawned in, so tasks may borrow
/// the net and the table instead of forcing everything to be `'static`.
struct Pool {
    q: std::sync::Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send + 'static>>>,
    cv: std::sync::Condvar,
    /// Workers currently blocked waiting for work.
    idle: std::sync::atomic::AtomicUsize,
    /// Queue length, readable without taking the lock. A thread waiting on a
    /// split polls for work; if that poll locks the mutex every iteration it
    /// serialises every worker against the waiters and throughput collapses.
    queued: std::sync::atomic::AtomicUsize,
    workers: usize,
    /// Set at shutdown; the workers only look at it when woken, so raising it
    /// has to be followed by `cv.notify_all()`.
    #[allow(dead_code)]
    stop: std::sync::atomic::AtomicBool,
}

impl Pool {
    fn new(workers: usize) -> Pool {
        use std::sync::atomic::{AtomicBool, AtomicUsize};
        Pool {
            q: std::sync::Mutex::new(std::collections::VecDeque::new()),
            cv: std::sync::Condvar::new(),
            idle: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            workers,
            stop: AtomicBool::new(false),
        }
    }

    /// Queue a task if a worker can take it. Returns false when the pool is
    /// saturated — the caller then does the work itself, which is what keeps
    /// the split decision cheap.
    fn try_push(&self, f: impl FnOnce() + Send + 'static) -> bool {
        use std::sync::atomic::Ordering;
        if self.workers == 0 {
            return false;
        }
        // Counted thread-locally and flushed in batches: a shared atomic
        // incremented on every split *attempt* is a contended cache line hit
        // hundreds of thousands of times a second, which would be measurement
        // that costs what it measures.
        SPLIT_TRIED_LOCAL.with(|c| {
            let n = c.get() + 1;
            if n >= 4096 {
                c.set(0);
                SPLIT_TRIED.fetch_add(n, Ordering::Relaxed);
            } else {
                c.set(n);
            }
        });
        // Only hand work over when a worker is actually free to start it now,
        // reporting the failure to the caller, which then searches the child
        // itself. Queueing beyond that looks like handing work off but really
        // parks it: the task waits in line while the parent blocks on it.
        //
        // The test runs *unlocked* first, and only
        // then takes the lock to re-check. Nearly every split attempt is a
        // rejection — the pool is busy — and taking the mutex to discover that
        // funnels every searching thread through one lock several hundred
        // thousand times a second.
        // Handing work over only when a worker is idle *now* (zero slack) was
        // the rule here once before; queueing beyond it was measured as a loss:
        // the table said 1.87x the sequential node count, and
        // parking a task behind a full queue while its parent blocks on it is
        // exactly how that happens.
        //
        // That node blow-up is gone (1.03x now that aborts propagate through
        // the whole ancestor chain), so the trade is worth re-measuring. The
        // reason to want a queue: a node with nine young brothers and five idle
        // workers hands out five and then searches the other four itself, one
        // after another — and the five tasks finish long before it is done, so
        // the workers sit idle through the rest of the fan-out.
        let slack = pool_slack();
        let free = |queued: usize| self.idle.load(Ordering::Relaxed) + slack > queued;
        if !free(self.queued.load(Ordering::Relaxed)) {
            return false;
        }
        let mut q = self.q.lock().unwrap();
        if !free(q.len()) {
            return false;
        }
        SPLIT_DONE.fetch_add(1, Ordering::Relaxed);
        q.push_back(Box::new(f));
        self.queued.fetch_add(1, Ordering::Relaxed);
        drop(q);
        self.cv.notify_one();
        true
    }

    /// Run one queued task if there is one. Used both by the workers and by a
    /// thread waiting on a split, so waiting never wastes a core.
    fn run_one(&self) -> bool {
        use std::sync::atomic::Ordering;
        // Cheap reject first: taking the lock just to find the queue empty is
        // what makes polling expensive for everyone else.
        if self.queued.load(Ordering::Relaxed) == 0 {
            return false;
        }
        let task = {
            let mut q = self.q.lock().unwrap();
            let t = q.pop_front();
            if t.is_some() {
                self.queued.fetch_sub(1, Ordering::Relaxed);
            }
            t
        };
        match task {
            Some(t) => {
                t();
                true
            }
            None => false,
        }
    }

    /// Block until `slot` is filled, running other queued tasks in the
    /// meantime rather than idling.
    fn wait(&self, slot: &Slot) {
        use std::sync::atomic::Ordering;
        let mut idle = 0u32;
        let mut starved: Option<std::time::Instant> = None;
        while !slot.done.load(Ordering::Acquire) {
            if self.run_one() {
                idle = 0;
                if let Some(t0) = starved.take() {
                    let ns = t0.elapsed().as_nanos() as u64;
                    if IS_POOL_WORKER.with(|c| c.get()) {
                        WAIT_IDLE_WORKER_NS.fetch_add(ns, Ordering::Relaxed);
                    } else {
                        WAIT_IDLE_MAIN_NS.fetch_add(ns, Ordering::Relaxed);
                    }
                }
                continue;
            }
            // Nothing to steal. `run_one` reads `queued`, and that line is
            // written by every push and every pop — spinning on it in a tight
            // loop does not just waste this core, it slows down the threads
            // that are actually searching. Back off geometrically so a waiter
            // touches the line rarely once it is clear no work is coming.
            if starved.is_none() {
                starved = Some(std::time::Instant::now());
            }
            idle += 1;
            if idle < 8 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
                if idle > 64 {
                    std::thread::sleep(std::time::Duration::from_micros(20));
                }
            }
        }
        if let Some(t0) = starved {
            let ns = t0.elapsed().as_nanos() as u64;
            if IS_POOL_WORKER.with(|c| c.get()) {
                WAIT_IDLE_WORKER_NS.fetch_add(ns, Ordering::Relaxed);
            } else {
                WAIT_IDLE_MAIN_NS.fetch_add(ns, Ordering::Relaxed);
            }
        }
    }

    fn worker_loop(&self) {
        use std::sync::atomic::Ordering;
        IS_POOL_WORKER.with(|c| c.set(true));
        loop {
            // The worker counts as idle for the whole time it is looking for
            // work, spinning included — the split gate is `idle > queued`, so a
            // spinning worker that did not count itself would not be offered
            // anything.
            self.idle.fetch_add(1, Ordering::Relaxed);
            // 外側は値を返すためのラベルで、実際には 1 周もしない (内側の
            // 条件待ちループから `break 'get` で値を持ち出す形)。
            #[allow(clippy::never_loop)]
            let task = 'get: loop {
                // Sleep until woken,
                // with no timeout. A 200us poll instead had all the idle workers
                // waking twenty-five thousand times a second to take the queue
                // mutex, find nothing, and go back to sleep — every one of those
                // acquisitions serialises against a thread trying to hand work
                // over.
                //
                // Spinning before parking to hide the measured 9us handoff was
                // tried and is not here: it only pays when tasks are small
                // enough for 9us to matter, and splitting that shallow loses
                // more than the latency costs (see `ybwc_min_depth`).
                let mut q = self.q.lock().unwrap();
                loop {
                    if let Some(t) = q.pop_front() {
                        self.queued.fetch_sub(1, Ordering::Relaxed);
                        break 'get Some(t);
                    }
                    if self.stop.load(Ordering::Relaxed) {
                        break 'get None;
                    }
                    q = self.cv.wait(q).unwrap();
                }
            };
            self.idle.fetch_sub(1, Ordering::Relaxed);
            match task {
                Some(t) => {
                    let t0 = std::time::Instant::now();
                    t();
                    WORKER_BUSY_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                None => return,
            }
        }
    }
}

/// Fixed-depth NNUE alpha-beta with a transposition table and NNUE-eval move
/// ordering — the pieces a real engine has, and what keeps the wall-clock
/// competitive (a naive search without them explodes).
pub struct NnueSearch {
    pub nn: &'static Nnue,
    pub tt: &'static SharedTt,
    /// Workers for the root split (1 = sequential).
    pub threads: usize,
    /// Nodes visited, to diagnose ordering quality (effective branching).
    pub nodes: u64,
    /// Enable ProbCut (selective pruning).
    pub mpc: bool,
    /// How many ProbCut probes are nested above this node.
    probcut_level: u32,
    /// Set by the root when a sibling's result makes this worker's subtree
    /// irrelevant; checked periodically so the worker can stop immediately
    /// instead of finishing work nobody will use.
    abort: Option<std::sync::Arc<AbortChain>>,
    /// 外部からの中断ハンドル (UI の停止ボタン等)。
    stop: Option<StopHandle>,
    abort_countdown: u64,
    /// Set once the main Lazy SMP thread has reached the target depth; helpers
    /// stop as soon as they notice, since their results are no longer needed.
    /// Shared iteration counter, and the iteration this worker is serving.
    /// Helpers are spawned once per move and loop over iterations themselves,
    /// so the main thread never has to join them mid-search; it just bumps the
    /// counter and a helper notices its pass is stale on the next check.
    done: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    my_gen: u32,
    /// Pool used to split null-window child searches (YBWC). Workers split
    /// their own subtrees too — without that a worker handed a large subtree
    /// becomes the critical path and the speedup collapses to the size of the
    /// largest single task. It is deadlock-free because a thread waiting on a
    /// split runs queued tasks while it waits, so no task can be stranded
    /// behind the thread that needs it.
    pool: Option<&'static Pool>,
    /// Extra ProbCut aggressiveness for Lazy SMP helpers: shrinks the margin so
    /// two helpers at the same depth explore differently instead of re-deriving
    /// the same entries.
    mpc_relax: u32,
}

impl NnueSearch {
    pub fn new(nn: &'static Nnue, tt: &'static SharedTt) -> Self {
        NnueSearch {
            nn,
            tt,
            threads: 1,
            nodes: 0,
            mpc: false,
            probcut_level: 0,
            abort: None,
            stop: None,
            abort_countdown: ABORT_CHECK_INTERVAL,
            done: None,
            my_gen: 0,
            pool: None,
            mpc_relax: 0,
        }
    }

    /// A worker sharing this search's model and table, with its own counters.
    /// The process-wide task pool, built on first use. A
    /// single global pool is the point: workers that live
    /// across moves cost nothing per search, and both Lazy SMP and YBWC draw
    /// from the same set of cores instead of oversubscribing them.
    fn shared_pool(&self, workers: usize) -> Option<&'static Pool> {
        if workers == 0 {
            // Checked before the cell, not inside it: a sequential searcher
            // must not inherit a pool that a parallel one happened to build.
            return None;
        }
        static POOL: std::sync::OnceLock<Option<&'static Pool>> = std::sync::OnceLock::new();
        *POOL.get_or_init(|| {
            POOL_WORKERS.store(workers, std::sync::atomic::Ordering::Relaxed);
            let pool: &'static Pool = Box::leak(Box::new(Pool::new(workers)));
            for _ in 0..workers {
                std::thread::spawn(move || pool.worker_loop());
            }
            if std::env::var("POOL_SAMPLE").is_ok() {
                std::thread::spawn(move || loop {
                    if SEARCH_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
                        let idle = pool.idle.load(std::sync::atomic::Ordering::Relaxed);
                        let busy = workers.saturating_sub(idle).min(16);
                        BUSY_HIST[busy].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    std::thread::sleep(std::time::Duration::from_micros(100));
                });
            }
            Some(pool)
        })
    }

    fn worker(&self) -> NnueSearch {
        NnueSearch {
            nn: self.nn,
            tt: self.tt,
            threads: 1,
            nodes: 0,
            mpc: self.mpc,
            probcut_level: 0,
            abort: self.abort.clone(),
            stop: self.stop.clone(),
            abort_countdown: ABORT_CHECK_INTERVAL,
            done: self.done.clone(),
            my_gen: self.my_gen,
            pool: self.pool,
            mpc_relax: self.mpc_relax,
        }
    }

    /// Whether a Lazy SMP helper should stop looping.
    #[inline]
    fn stopped(&self) -> bool {
        self.done
            .as_ref()
            .is_some_and(|g| g.load(std::sync::atomic::Ordering::Relaxed) != self.my_gen)
    }

    /// Whether this worker has been told to stop. Checked once every
    /// `ABORT_CHECK_INTERVAL` nodes.
    #[inline]
    fn should_stop(&mut self) -> bool {
        if self.abort.is_none() && self.stop.is_none() {
            return false;
        }
        self.abort_countdown -= 1;
        if self.abort_countdown != 0 {
            return false;
        }
        self.abort_countdown = ABORT_CHECK_INTERVAL;
        if self.stop.as_ref().is_some_and(|s| s.is_stopped()) {
            return true;
        }
        self.abort.as_ref().is_some_and(|f| f.stopped())
    }

    /// 外部からの中断ハンドルを設定する。
    pub fn set_stop(&mut self, stop: Option<StopHandle>) {
        self.stop = stop;
    }

    /// Periodic check that also honours the Lazy SMP done flag, so helpers
    /// unwind promptly instead of finishing a deep pass nobody will read.
    #[inline]
    fn should_stop_or_done(&mut self) -> bool {
        if self.should_stop() {
            return true;
        }
        self.done.is_some() && self.stopped()
    }

    pub fn clear(&mut self) {
        self.tt.clear();
    }

    /// Best move at `depth`, via iterative deepening: each pass seeds the TT
    /// so the next pass orders by the prior best move — this is what lets the
    /// deep pass skip the expensive per-node eval ordering (only new nodes pay
    /// for it), the way real engines avoid a full 1-ply scan everywhere.
    pub fn best_move(&mut self, b: &Board, depth: u32) -> Option<Position> {
        self.best_move_valued(b, depth).0
    }

    /// Best move *and* the root value, so a caller can check that a parallel
    /// search reproduces the sequential one exactly — the move alone can
    /// coincide while the value diverges.
    pub fn best_move_valued(&mut self, b: &Board, depth: u32) -> (Option<Position>, f32) {
        let (p, v, _) = self.best_move_deadline(b, depth, None);
        (p, v)
    }

    /// 期限つきの探索。反復深化なので、期限が来たら**直前に完了した段**の
    /// 答えを返せる (途中の段は捨てる)。戻り値の 3 つ目は到達した深さ。
    ///
    /// 期限の見張りは専用スレッドに任せて停止ハンドルを立てさせる。探索の
    /// 内側で時計を読むと、読む頻度を落とせば効きが鈍り、上げれば探索が
    /// 遅くなる。停止ハンドルの確認はもともと一定ノードごとに走っている。
    pub fn best_move_deadline(
        &mut self,
        b: &Board,
        depth: u32,
        deadline: Option<std::time::Instant>,
    ) -> (Option<Position>, f32, u32) {
        if b.movable() == 0 {
            return (None, f32::NAN, 0);
        }
        SEARCH_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        let watcher = deadline.and_then(|dl| {
            let stop = self.stop.clone()?;
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let d2 = done.clone();
            std::thread::spawn(move || {
                // 細かく起きて、探索が先に終わっていれば黙って抜ける
                while !d2.load(std::sync::atomic::Ordering::Relaxed) {
                    let now = std::time::Instant::now();
                    if now >= dl {
                        stop.stop();
                        return;
                    }
                    std::thread::sleep((dl - now).min(std::time::Duration::from_millis(20)));
                }
            });
            Some(done)
        });
        let r = self.best_move_valued_inner(b, depth, deadline);
        if let Some(d) = watcher {
            d.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        SEARCH_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        r
    }

    fn best_move_valued_inner(
        &mut self,
        b: &Board,
        depth: u32,
        deadline: Option<std::time::Instant>,
    ) -> (Option<Position>, f32, u32) {
        if self.threads > 1 && depth >= 2 && deadline.is_none() {
            // Lazy SMP runs its own iterative deepening on every worker; doing
            // the shallow passes here as well would just duplicate them.
            let (p, v) = self.lazy_smp(b, depth);
            return (p, v, depth);
        }
        let mut acc = self.nn.indices(b.black, b.white);
        // Iterative deepening: each pass seeds the table so the next orders by
        // the prior best move.
        let mut value = f32::NAN;
        let mut best = None;
        let mut reached = 0;
        let mut last_pass = std::time::Duration::ZERO;
        for d in 1..=depth {
            if let Some(dl) = deadline {
                let now = std::time::Instant::now();
                if now >= dl {
                    break;
                }
                // 次の段は前の段の数倍かかる。入っても終わらないなら始めない
                // (始めれば見張りに切られ、その段はまるごと捨て札になる)
                if reached > 0 && now + last_pass.mul_f32(NEXT_PASS_FACTOR) >= dl {
                    break;
                }
            }
            let t0 = std::time::Instant::now();
            let v = self.negamax(b, &mut acc, d, f32::NEG_INFINITY, f32::INFINITY);
            // 期限で切られた段は不完全。直前の段の答えを残す
            if self.stop.as_ref().is_some_and(|s| s.is_stopped()) {
                break;
            }
            last_pass = t0.elapsed();
            value = v;
            best = self.root_best(b, &mut acc);
            reached = d;
        }
        (best.or_else(|| self.root_best(b, &mut acc)), value, reached)
    }

    /// The move the table records for `b`, falling back to a 1-ply ordering.
    fn root_best(&self, b: &Board, acc: &mut PatternIndices) -> Option<Position> {
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let e = self.tt.get(h);
        if e.key == h && e.best < 64 {
            return Position::from_index(e.best as u32);
        }
        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0.0); MAX_KIDS];
        let n = self.ordered_into(b, acc, 1, b.movable(), &mut kids, false, 64, 64);
        (n > 0).then(|| kids[0].0)
    }

    /// Lazy SMP.
    ///
    /// The main thread owns the iterative deepening. **Helpers are launched and
    /// joined inside each iteration**, not once for the whole search: their job
    /// is to fill the table for the iteration the main thread is about to run,
    /// and a helper still running a stale iteration is wasted work.
    ///
    /// Helpers diverge along two axes:
    ///
    /// - **depth** `main_depth + ctz(idx + 1)`: helper 0 shares the main depth,
    ///   1 goes one deeper, 2 shares again, 3 goes two deeper... so the search
    ///   effort stays concentrated near the current iteration while a few
    ///   threads scout ahead. A flat 0..2 spread (what this used to do) puts
    ///   too much work on plies the main thread will not reach this iteration.
    /// - **selectivity**: when several helpers land on the same depth, each
    ///   subsequent one prunes harder (`sub_mpc_level` increments). Two threads
    ///   searching the same depth with the same selectivity just re-derive the
    ///   same entries.
    ///
    /// Helpers are also only worth launching while the iteration is cheap
    /// (see `smp_max_depth`).
    fn lazy_smp(&mut self, b: &Board, depth: u32) -> (Option<Position>, f32) {
        use std::sync::atomic::{AtomicU64, Ordering};
        // Tuning knobs read once from the environment, so a sweep does not need
        // one build per point (thermal drift makes serial rebuild-and-measure
        // unreliable — see the measurement protocol).
        fn env_u32(key: &'static str, default: u32) -> u32 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        fn smp_min_depth() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_MIN_DEPTH", 1))
        }
        fn smp_spread() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_SPREAD", 2))
        }
        fn smp_sharpen_max() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_SHARPEN_MAX", 2))
        }

        /// Above this iteration Lazy SMP is switched off and the whole pool
        /// goes into YBWC instead.
        ///
        /// The two are not interchangeable. Lazy SMP buys nothing but table
        /// entries: every helper searches the same tree, so its value decays
        /// as soon as the table is warm, and on a machine with c cores it can
        /// never exceed a small constant. YBWC divides the actual work. Cheap
        /// early iterations are where redundant searches are affordable and
        /// where a split would cost more than the subtree; deep iterations are
        /// the opposite.
        fn smp_max_depth() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_MAX_DEPTH", 10))
        }

        let nodes = AtomicU64::new(0);
        let mut acc = self.nn.indices(b.black, b.white);
        let mut value = f32::NAN;

        // One pool for both kinds of parallel work, so a core
        // freed by one is immediately usable by the other. It is built once and
        // outlives every search, which is also what lets tasks be `'static`.
        let workers = self.threads - 1;
        let pool = self.shared_pool(workers);

        {
            for main_depth in 1..=depth {
                let lazy = main_depth >= smp_min_depth() && main_depth <= smp_max_depth();
                // Helpers are retired at the end of the iteration by this flag;
                // the generation counter doubles as their stop signal.
                let gen = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(main_depth));
                let mut slots = Vec::new();
                if let Some(pool) = pool.filter(|_| lazy) {
                    for idx in 0..workers {
                        let slot = std::sync::Arc::new(Slot::new());
                        let mut w = self.worker();
                        w.done = Some(gen.clone());
                        w.my_gen = main_depth;
                        // ctz(idx+1): half the helpers share the main depth,
                        // the rest scout progressively further ahead.
                        let ahead = (idx as u32 + 1).trailing_zeros() * smp_spread();
                        // Same depth => prune harder, so they do not duplicate.
                        w.mpc_relax = (idx as u32 / 2).min(smp_sharpen_max());
                        let d = (main_depth + ahead).min(depth);
                        let task_slot = slot.clone();
                        let root = *b;
                        if pool.try_push(move || {
                            let mut wacc = w.nn.indices(root.black, root.white);
                            w.negamax(&root, &mut wacc, d, f32::NEG_INFINITY, f32::INFINITY);
                            task_slot.set(0.0, w.nodes);
                        }) {
                            slots.push(slot);
                        }
                    }
                } else {
                    // No redundant helpers to compete with: let the main search
                    // hand its younger brothers to the pool.
                    self.pool = pool;
                }

                value = self.negamax(b, &mut acc, main_depth, f32::NEG_INFINITY, f32::INFINITY);
                self.pool = None;

                // Retire this iteration's helpers before starting the next, so
                // stale passes do not keep cores away from the deeper search.
                gen.store(u32::MAX, Ordering::Relaxed);
                for slot in slots {
                    pool.unwrap().wait(&slot);
                    nodes.fetch_add(slot.nodes.load(Ordering::Relaxed), Ordering::Relaxed);
                }
            }
        }
        self.nodes += nodes.load(Ordering::Relaxed);
        (self.root_best(b, &mut acc), value)
    }

    /// Children with their flip masks, best-first.
    ///
    /// The shape matters more than the numbers. *Every* move is scored
    /// as a weighted sum and the table move wins on a huge constant;
    /// the ordering is never skipped. And the two
    /// signals are weighted differently by node type: at a PV node the
    /// evaluation dominates (269 against 35 for mobility), at a null-window
    /// node it barely counts (7 against 17). Null-window nodes
    /// are most of the tree, so most of the tree is ordered by mobility.
    ///
    /// That split matters all the more with this evaluator: a full NNUE eval
    /// per child is by far the most expensive thing this search does — it is
    /// not even counted as a node — where a pattern evaluator would pay only a
    /// table lookup. Ordering every null-window node by NNUE eval, which is
    /// what this used to do from remaining depth 3 upward, spends the budget in
    /// exactly the wrong place.
    ///
    /// Mobility is weighted moves (moves counted double,
    /// corner moves once more) plus potential mobility (empties adjacent
    /// to opponent discs) at PV nodes only, matching which weights are non-zero
    /// in each variant.
    ///
    /// Fills `out` and returns how many children there are. Writing into a
    /// caller-owned stack array keeps this off the heap — returning a `Vec`
    /// meant one allocation per interior node, which at ~60k nodes a move is
    /// pure overhead.
    fn ordered_into(
        &self,
        b: &Board,
        acc: &mut PatternIndices,
        depth: u32,
        moves: u64,
        out: &mut [Kid; MAX_KIDS],
        null_window: bool,
        tt_move: u8,
        tt_move2: u8,
    ) -> usize {
        let mover = b.player();
        // Ordering weights, scaled so the eval term stays in disc units.
        let (w_mob, w_pm, w_val) = if null_window {
            (17.0 / 7.0, 0.0, 1.0)
        } else {
            (35.0 / 269.0, 17.0 / 269.0, 1.0)
        };
        let eval_order = depth >= eval_order_depth();
        let mut n = 0usize;
        let mut m = moves;
        while m != 0 {
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            m &= m - 1;
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            // The table's moves are
            // scored so far above everything else that the sort puts them first
            // and second. Doing it with the score rather than by swapping them
            // to the front matters — two swaps do not land the runner-up in
            // second place, they displace whatever the ordering had chosen.
            let key = if pos.index() == tt_move {
                1.0e9
            } else if pos.index() == tt_move2 {
                1.0e8
            } else {
                let legal = nb.movable();
                // Offset minus weighted moves, so fewer replies scores higher.
                let mob = 38.0 - (legal.count_ones() * 2 + (legal & CORNERS).count_ones()) as f32;
                let mut k = mob * w_mob;
                if w_pm != 0.0 {
                    let empties = !(nb.black | nb.white);
                    k += (38.0 - potential_mobility(nb.opponent_bb(), empties) as f32) * w_pm;
                }
                if eval_order {
                    self.nn.ix_apply(acc, pos, flipped, mover);
                    k += -self.nn.eval_from_indices(acc, &nb) * w_val;
                    self.nn.ix_undo(acc, pos, flipped, mover);
                }
                k
            };
            out[n] = (pos, flipped, key);
            n += 1;
        }
        out[..n].sort_unstable_by(|a, b| b.2.total_cmp(&a.2));
        n
    }

    /// Depth-1 null-window fast path. No table, no move list, no ordering
    /// pass — the children are leaves, so their eval *is* the ordering.
    fn eval1_nws(&mut self, b: &Board, acc: &mut PatternIndices, alpha: f32) -> f32 {
        self.nodes += 1;
        let moves = b.movable();
        if moves == 0 {
            let mut nb = *b;
            nb.pass();
            if nb.movable() == 0 {
                let p = b.player_bb().count_ones() as i32;
                let o = b.opponent_bb().count_ones() as i32;
                let e = 64 - p - o;
                let diff = if p > o {
                    p - o + e
                } else if o > p {
                    p - o - e
                } else {
                    0
                };
                return diff as f32 * 1000.0;
            }
            let raw = self.eval1_nws(&nb, acc, -alpha - PVS_EPS);
            return -raw;
        }
        let mover = b.player();
        let mut v = f32::NEG_INFINITY;
        for mask in STATIC_CELL_PRIORITY {
            let mut l = moves & mask;
            while l != 0 {
                let pos = Position::from_index(l.trailing_zeros()).unwrap();
                l &= l - 1;
                let mut nb = *b;
                let flipped = nb.make_move_bits(pos);
                self.nodes += 1;
                self.nn.ix_apply(acc, pos, flipped, mover);
                let g = -self.nn.eval_from_indices(acc, &nb);
                self.nn.ix_undo(acc, pos, flipped, mover);
                if g > v {
                    if g > alpha {
                        return g;
                    }
                    v = g;
                }
            }
        }
        v
    }

    pub fn negamax(
        &mut self,
        b: &Board,
        acc: &mut PatternIndices,
        depth: u32,
        mut alpha: f32,
        beta: f32,
    ) -> f32 {
        self.nodes += 1;
        if self.should_stop_or_done() {
            return ABORTED;
        }
        // One move generation per node. `is_game_over()` generates moves for
        // both sides internally, and the pass check and child loop each
        // generated them again — three or four generations where one suffices.
        let moves = b.movable();
        if moves == 0 {
            let mut nb = *b;
            nb.pass();
            if nb.movable() == 0 {
                let p = b.player_bb().count_ones() as i32;
                let o = b.opponent_bb().count_ones() as i32;
                let e = 64 - p - o;
                let diff = if p > o {
                    p - o + e
                } else if o > p {
                    p - o - e
                } else {
                    0
                };
                return diff as f32 * 1000.0;
            }
            if depth == 0 {
                return self.nn.eval_from_indices(acc, b);
            }
            let raw = self.negamax(&nb, acc, depth, -beta, -alpha);
            return if raw == ABORTED { ABORTED } else { -raw };
        }
        if depth == 0 {
            return self.nn.eval_from_indices(acc, b);
        }

        // Depth-1 fast path: a null-window node one ply from
        // the leaves touches no transposition table and builds no move list. It
        // walks the legal moves in static square-priority order and returns the
        // moment one beats alpha.
        //
        // These are the most numerous interior nodes in the tree, and they sit
        // below the depth where a subtree can be split — so the two probes into
        // a 400 MB table that the general path does here are paid entirely out
        // of the sequential part of the search.
        if depth == 1 && beta - alpha <= PVS_EPS * 1.5 {
            return self.eval1_nws(b, acc, alpha);
        }

        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        // The window this node was *asked* about. The table may narrow the
        // working window below, but what gets registered has to be measured
        // against this one.
        let (first_alpha, first_beta) = (alpha, beta);
        let mut beta = beta;
        let mut tt_move = 64u8;
        let mut tt_move2 = 64u8;
        {
            let e = self.tt.get(h);
            if e.key == h && e.flag != 0 {
                // The stored moves are always worth having for ordering; the
                // stored *bounds* only if the entry was searched at least as
                // deep and at least as accurately as this node needs.
                tt_move = e.best;
                tt_move2 = e.best2;
                if e.depth as u32 >= depth && e.relax >= self.mpc_relax as u8 {
                    // Transposition cutoff.
                    if e.upper <= alpha || e.upper == e.lower {
                        return e.upper;
                    }
                    if beta <= e.lower {
                        return e.lower;
                    }
                    // Narrow the working window to what the table already
                    // knows. Measured worth having (36.1M nodes against 37.2M
                    // without) even though ProbCut makes the stored bounds
                    // probabilistic rather than sound.
                    if alpha < e.lower {
                        alpha = e.lower;
                    }
                    if e.upper < beta {
                        beta = e.upper;
                    }
                }
            }
        }

        // Multi-ProbCut: a reduced-depth null-window probe decides whether this
        // node lies so far outside [alpha, beta] that a full search cannot
        // change the parent's decision. The margin is T sigma of the shallow
        // search's error. Only at null-window nodes (never on the PV).
        //
        // Two safeguards that the sigma cannot substitute for
        // (missing them measured ~3pt of win rate even
        // though the sigma itself calibrates correctly on unpruned probes):
        // - the *static* eval must already sit outside the window by
        //   margin - 4 before a probe runs, and it picks which side to try
        //   (`MPC_D0_OFFSET`) — a cheap second opinion, and half the
        //   probes never run at all;
        // - the probe itself searches **unpruned**; this
        //   used to keep cutting inside the probe up
        //   to MPC_MAX_LEVEL deep, which recursively degrades the very
        //   values the margins are calibrated for.
        // `MPC_OLD=1` restores the old behaviour for A/B runs.
        if self.mpc
            && depth >= mpc_min_depth()
            && self.probcut_level < MPC_MAX_LEVEL
            && alpha.is_finite()
            && beta.is_finite()
            && beta - alpha <= PVS_EPS * 1.5
        {
            let pd = mpc_reduced_depth(depth);
            if pd >= 1 && pd < depth {
                // Helpers widen the margin (prune less) so same-depth workers
                // diverge; the main thread always uses MPC_T. Widening, not
                // tightening, is what makes a helper's entry safe for the main
                // thread to reuse.
                let t = mpc_t() * MPC_RELAX_STEP.powi(self.mpc_relax as i32);
                let margin = t * mpc_sigma(b.empty_count() as u32, depth, pd);
                let old_style = mpc_old();
                let (try_high, try_low) = if old_style {
                    (true, true)
                } else {
                    let d0 = self.nn.eval_from_indices(acc, b);
                    let gate = (margin - MPC_D0_OFFSET).max(1.0);
                    (d0 >= beta + gate, d0 <= alpha - gate)
                };
                if try_high || try_low {
                    self.probcut_level += 1;
                    let saved_mpc = self.mpc;
                    if !old_style {
                        self.mpc = false;
                    }
                    let mut cut = None;
                    let mut aborted = false;
                    if try_high {
                        let hi = beta + margin;
                        let high = self.negamax(b, acc, pd, hi - PVS_EPS, hi);
                        if high == ABORTED {
                            // Aborted probe: no information. Bail out of the
                            // node rather than mistake -inf for "far below".
                            aborted = true;
                        } else if high >= hi {
                            cut = Some(beta);
                        }
                    }
                    if !aborted && cut.is_none() && try_low {
                        let lo = alpha - margin;
                        let low = self.negamax(b, acc, pd, lo, lo + PVS_EPS);
                        if low == ABORTED {
                            aborted = true;
                        } else if low <= lo {
                            cut = Some(alpha);
                        }
                    }
                    self.mpc = saved_mpc;
                    self.probcut_level -= 1;
                    if aborted {
                        return ABORTED;
                    }
                    if let Some(v) = cut {
                        return v;
                    }
                }
            }
        }

        // Which of the two ordering weight sets to use. Null-window nodes
        // are most of the tree and get the cheap one.
        let mover = b.player();
        let mut best = f32::NEG_INFINITY;
        let mut best_move = 64u8;
        let mut moves = moves;

        // Search the table's move on its own *before* the ordering
        // pass. If it cuts, the pass never
        // happens — and here that pass costs a full NNUE evaluation per child,
        // so on a cut node it is the single most expensive thing avoided. About
        // half the interior nodes of an alpha-beta tree are cut nodes.
        let mut pre_searched = false;
        if tt_move < 64 && depth >= 2 && moves.count_ones() > 1 && moves >> tt_move & 1 == 1 {
            let pos = Position::from_index(tt_move as u32).unwrap();
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            self.nn.ix_apply(acc, pos, flipped, mover);
            let raw = self.negamax(&nb, acc, depth - 1, -beta, -alpha);
            self.nn.ix_undo(acc, pos, flipped, mover);
            if raw == ABORTED {
                return ABORTED;
            }
            best = -raw;
            best_move = tt_move;
            if best > alpha {
                alpha = best;
            }
            moves &= !(1u64 << tt_move);
            if alpha >= beta || moves == 0 {
                self.tt.put(
                    h,
                    depth as u8,
                    self.mpc_relax as u8,
                    first_alpha,
                    first_beta,
                    best,
                    best_move,
                );
                return best;
            }
            pre_searched = true;
        }
        // After that search alpha may have moved, and the updated alpha is
        // what decides the weight set.
        let null_window = beta - alpha <= PVS_EPS * 1.5;

        // Score every move, and let the table
        // move win on a constant rather than skip the scoring.
        // Skipping it — which is what this used to do whenever the table had a
        // move — left every child but the first in bit order, and those are
        // exactly the young brothers the pool searches. An unordered young
        // brother that beats alpha stops the fan-out and forces a re-search of
        // everything behind it.
        //
        // The work is also skipped when there is nothing to order
        // (a single legal move).
        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0.0); MAX_KIDS];
        let will_split = self.pool.is_some() && depth >= ybwc_min_depth();
        let ordered = depth >= 2 && moves.count_ones() > 1;
        let n_kids = if ordered {
            self.ordered_into(b, acc, depth, moves, &mut kids, null_window, 64, tt_move2)
        } else {
            let mut n = 0usize;
            let mut m = moves;
            while m != 0 {
                let pos = Position::from_index(m.trailing_zeros()).unwrap();
                m &= m - 1;
                let mut nb = *b;
                kids[n] = (pos, nb.make_move_bits(pos), 0.0);
                n += 1;
            }
            n
        };
        // The scored path already put the table moves first; the
        // unscored path (one legal move, or depth below 2) still needs the swap.
        if !ordered && tt_move < 64 {
            if let Some(i) = kids[..n_kids].iter().position(|k| k.0.index() == tt_move) {
                kids.swap(0, i);
            }
        }

        // PVS / NegaScout: full window for the best-ordered child, a null
        // window for later siblings, re-searched only when one beats alpha.
        // Same value as plain alpha-beta. (Measured: no node reduction here —
        // the eval-based ordering already gets the effective branching to ~3,
        // near the sqrt(b) ideal, so there is nothing left for PVS to prune.
        // Kept because it costs nothing and helps when ordering degrades.)
        // YBWC (Young Brothers Wait Concept): the eldest child is searched
        // sequentially to establish the window, and only then are the younger
        // siblings' null-window searches handed to idle workers. Splitting
        // before the window is known would parallelise work that a cutoff was
        // about to make unnecessary.
        //
        // Every non-eldest child is searched with a null window whatever kind
        // of node its parent is, so PV nodes have young brothers to give away
        // too — and being at the top of the tree, theirs are the largest
        // subtrees there are. Both kinds split here;
        // restricting this to null-window *parents*
        // excluded the whole PV spine and left the workers idle 68% of the
        // time.
        //
        // Splits start at `ybwc_min_depth`; below that a
        // subtree is cheaper than the handoff.
        let split_ok = will_split;

        let mut aborted = false;
        // Which children already have a final answer.
        let mut settled = [false; MAX_KIDS];
        let mut n_settled = 0usize;
        // The table move already fixed the window, so the rest are young
        // brothers even though none of them has been searched here yet.
        let mut first = !pre_searched;

        // Fan the young brothers out
        // against the alpha in force *now*, and the moment one of them beats
        // it, stop the fan-out, re-search the winners with the real window, and
        // fan the rest out again against the improved alpha.
        //
        // Letting the fan-out run to completion instead — which is what this
        // used to do — leaves every sibling searching against an alpha that is
        // known to be too low, and the extra nodes are what eats the parallel
        // gain (1.87x the sequential node count at 6 threads).
        // Only split-eligible nodes ever swap `self.abort`, so only they need a
        // copy of the enclosing chain. Cloning it at *every* interior node put
        // an atomic increment on one shared refcount in the hot path — and
        // under six threads that line is being incremented by all of them.
        let outer = if split_ok { self.abort.clone() } else { None };
        'fanout: loop {
            // The fan-out flag, with the polarity this codebase uses
            // for aborts: raised by whoever beats alpha, which ends this
            // fan-out (not the node). It is handed to the tasks as their abort
            // flag, so it *must* start clear — initialising it the other way
            // round made every task stop the instant it started, leaving the
            // node to be decided by its eldest child alone (28% in self-play).
            //
            // Only nodes that can actually split get one. The flag is heap
            // allocated, and allocating it at every interior node cost 13% of
            // single-thread throughput for a flag nobody could ever raise.
            let fan_stop = if split_ok {
                Some(AbortChain::child(outer.clone()))
            } else {
                None
            };
            // The loop control is a plain local, never the shared flag: a node
            // that cannot split has no flag, and gating the break on the flag
            // meant a fail-high stopped nothing — every remaining brother was
            // still probed against an alpha already known to be too low. That
            // is the fan-out flag losing its only job, and it cost 4x
            // the nodes single-threaded.
            let mut stop_fanout = false;
            let mut pending: Vec<(usize, std::sync::Arc<Slot>)> = Vec::new();
            let mut research: Vec<usize> = Vec::new();
            let mut next_alpha = alpha;

            for i in 0..n_kids {
                if settled[i] {
                    continue;
                }
                if stop_fanout || fan_stop.as_ref().is_some_and(|f| f.stopped()) {
                    break;
                }
                let (pos, flipped, _) = kids[i];
                // The flips are already known from the ordering pass; applying
                // them directly avoids recomputing a full flip per child.
                let mut nb = *b;
                nb.apply_flips(pos, flipped);

                // The eldest brother fixes the window before anything is given
                // away, and is always searched here with the full window.
                if first {
                    self.nn.ix_apply(acc, pos, flipped, mover);
                    let raw = self.negamax(&nb, acc, depth - 1, -beta, -alpha);
                    self.nn.ix_undo(acc, pos, flipped, mover);
                    // Inspect the child's own return value: negating it would
                    // turn the ABORTED sentinel into +inf and hide the abort.
                    if raw == ABORTED {
                        aborted = true;
                        break;
                    }
                    first = false;
                    settled[i] = true;
                    n_settled += 1;
                    let v = -raw;
                    if v > best {
                        best = v;
                        best_move = pos.index();
                    }
                    if best > alpha {
                        alpha = best;
                        next_alpha = alpha;
                    }
                    if alpha >= beta {
                        break;
                    }
                    continue;
                }

                // The last unsettled young brother is never split, it is what
                // the splitting thread does while the others run.
                let is_last = (i + 1..n_kids).all(|j| settled[j]);
                if split_ok && !is_last {
                    let pool = self.pool.unwrap();
                    let slot = std::sync::Arc::new(Slot::new());
                    let (nn, tt, mpc, relax) = (self.nn, self.tt, self.mpc, self.mpc_relax);
                    let (gen, my_gen) = (self.done.clone(), self.my_gen);
                    let stop = fan_stop.clone().unwrap();
                    let (task_slot, child, a, d) = (slot.clone(), nb, alpha, depth - 1);
                    let pushed = pool.try_push(move || {
                        // Check before doing anything. A task that waited in the
                        // queue may have been made irrelevant while it sat
                        // there — a brother beat alpha and the fan-out stopped —
                        // and the periodic check inside the search only fires
                        // every ABORT_CHECK_INTERVAL nodes, so without this the
                        // task searches half a thousand nodes of a tree nobody
                        // will read. That waste is exactly what made a queue
                        // unprofitable: nodes grew 30% at a queue depth of 32.
                        if stop.stopped() {
                            task_slot.set(ABORTED, 0);
                            return;
                        }
                        let mut w = NnueSearch::new(nn, tt);
                        w.mpc = mpc;
                        w.mpc_relax = relax;
                        w.pool = Some(pool);
                        // The fan-out flag doubles as the task's abort signal:
                        // once someone has beaten alpha every other subtree is
                        // about to be restarted against a better window.
                        w.abort = Some(stop.clone());
                        // Look at the flag on the first node too, not only after
                        // the first full interval.
                        w.abort_countdown = 1;
                        w.done = gen;
                        w.my_gen = my_gen;
                        let mut cacc = w.nn.indices(child.black, child.white);
                        let v = w.negamax(&child, &mut cacc, d, -(a + PVS_EPS), -a);
                        if v != ABORTED && -v > a {
                            stop.raise();
                        }
                        task_slot.set(v, w.nodes);
                    });
                    if pushed {
                        pending.push((i, slot));
                        continue;
                    }
                }

                // No worker free: search this young brother here. It runs under
                // the fan-out flag — so a sibling task that beats
                // alpha cuts this search short too instead of letting the
                // splitting thread finish a probe against a stale window.
                if split_ok {
                    self.abort = fan_stop.clone();
                }
                self.nn.ix_apply(acc, pos, flipped, mover);
                let raw = self.negamax(&nb, acc, depth - 1, -(alpha + PVS_EPS), -alpha);
                self.nn.ix_undo(acc, pos, flipped, mover);
                if split_ok {
                    self.abort = outer.clone();
                }
                if raw == ABORTED {
                    // Told to stop by an ancestor: the node is dead. Told to
                    // stop by *this* fan-out: only the probe is dead, and the
                    // move stays unsettled for the next round. A node that
                    // cannot split has no fan-out of its own, so any abort
                    // reaching it came from above.
                    if !split_ok || outer.as_ref().is_some_and(|o| o.stopped()) {
                        aborted = true;
                    }
                    break;
                }
                let g = -raw;
                if g > best {
                    best = g;
                    best_move = pos.index();
                }
                if g > alpha {
                    next_alpha = next_alpha.max(g);
                    stop_fanout = true;
                    if let Some(f) = &fan_stop {
                        f.raise();
                    }
                    research.push(i);
                } else {
                    settled[i] = true;
                    n_settled += 1;
                }
            }

            // Collect this fan-out. Tasks that were stopped stay unsettled and
            // go into the next round against the improved alpha.
            for (i, slot) in pending {
                self.pool.unwrap().wait(&slot);
                self.nodes += slot.nodes.load(std::sync::atomic::Ordering::Relaxed);
                let raw = f32::from_bits(slot.bits.load(std::sync::atomic::Ordering::Relaxed));
                if raw == ABORTED || aborted {
                    continue;
                }
                let g = -raw;
                if g > best {
                    best = g;
                    best_move = kids[i].0.index();
                }
                if g > alpha {
                    next_alpha = next_alpha.max(g);
                    research.push(i);
                } else {
                    settled[i] = true;
                    n_settled += 1;
                }
            }

            // An ancestor cut off while this fan-out was running: every task
            // came back ABORTED, so there is nothing to restart and nothing
            // worth storing.
            if outer.as_ref().is_some_and(|o| o.stopped()) {
                aborted = true;
            }
            if aborted {
                break 'fanout;
            }
            if research.is_empty() {
                // Nobody beat alpha: either every child is settled, or the
                // eldest already produced a cutoff.
                break 'fanout;
            }
            if next_alpha >= beta {
                break 'fanout;
            }

            // A null-window probe that beat its alpha is only a lower bound, so
            // the winners are re-searched with the real window before the rest
            // are fanned out again.
            alpha = next_alpha;
            for &i in &research {
                let (pos, flipped, _) = kids[i];
                let mut nb = *b;
                nb.apply_flips(pos, flipped);
                self.nn.ix_apply(acc, pos, flipped, mover);
                let raw = self.negamax(&nb, acc, depth - 1, -beta, -alpha);
                self.nn.ix_undo(acc, pos, flipped, mover);
                if raw == ABORTED {
                    aborted = true;
                    break 'fanout;
                }
                settled[i] = true;
                n_settled += 1;
                let g = -raw;
                if g > best {
                    best = g;
                    best_move = pos.index();
                }
                if best > alpha {
                    alpha = best;
                    if alpha >= beta {
                        break;
                    }
                }
            }
            if alpha >= beta || n_settled == n_kids {
                break 'fanout;
            }
        }

        if aborted {
            // A truncated subtree carries no value. Returning here — and
            // crucially *not* writing the table — keeps the abort from
            // poisoning entries that other threads will trust. (Storing it
            // was measured as a total collapse: 0% score, -59 discs.)
            return ABORTED;
        }

        self.tt.put(
            h,
            depth as u8,
            self.mpc_relax as u8,
            first_alpha,
            first_beta,
            best,
            best_move,
        );
        best
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    /// 期限を過ぎたら、そこまでに終わった深さの答えを返す。
    /// (深い指定でも待たされない = 反復深化が段ごとに畳まれている)
    #[test]
    fn deadline_cuts_the_search_short() {
        // 重みは読まない (テストに weights/ を要求しない)。値は無意味だが、
        // 段が畳まれるかどうかの確認には要らない。quantize は必須 (忘れると
        // SIMD 経路が未初期化領域を読む)
        let mut nn0 = Nnue::new(crate::pattern::EGAROUCID_PATTERNS);
        nn0.quantize();
        let nn: &'static Nnue = Box::leak(Box::new(nn0));
        let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(18)));
        let mut s = NnueSearch::new(nn, tt);
        s.threads = 1;
        s.set_stop(Some(StopHandle::new()));
        let b = Board::new();
        let t0 = std::time::Instant::now();
        let dl = t0 + std::time::Duration::from_millis(120);
        // 深さ 40 は本来数分かかる。期限で切り上がること
        let (pos, _v, reached) = s.best_move_deadline(&b, 40, Some(dl));
        let el = t0.elapsed();
        assert!(pos.is_some(), "浅くても手は返る");
        assert!(reached >= 1, "少なくとも 1 段は終わっている");
        assert!(reached < 40, "期限で切り上がる (到達 {reached})");
        assert!(
            el < std::time::Duration::from_secs(3),
            "期限を大きく超えない ({el:?})"
        );
    }
}
