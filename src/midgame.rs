//! Midgame NNUE search — the engine that plays matches.

#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]

use crate::fan::{AbortChain, HasKids, Kids};
use crate::nnue::Nnue;
use crate::pattern_index::PatternIndices;
use crate::zobrist;
use crate::{Board, Position};

const PVS_EPS: f32 = 0.01;

#[inline(always)]
fn is_null_window(alpha: f32, beta: f32) -> bool {
    beta - alpha <= PVS_EPS * 1.5
}

type Kid = (Position, u64, i32);

/// A finished game, scaled far beyond any evaluation.
fn final_value(b: &Board) -> f32 {
    crate::solver::final_score(b) as f32 * 1000.0
}

#[inline]
fn order_key(x: f32) -> i32 {
    let b = x.to_bits() as i32;
    b ^ (((b >> 31) as u32) >> 1) as i32
}

const MAX_KIDS: usize = 34;

/// Up to `MAX_KIDS` kids, never initialised past the last one pushed: zeroing all of them would cost more than scoring.
struct KidStack<T: Copy> {
    items: [std::mem::MaybeUninit<T>; MAX_KIDS],
    len: usize,
}

impl<T: Copy> KidStack<T> {
    #[inline(always)]
    fn new() -> Self {
        KidStack {
            items: [const { std::mem::MaybeUninit::uninit() }; MAX_KIDS],
            len: 0,
        }
    }

    #[inline(always)]
    fn push(&mut self, t: T) {
        self.items[self.len].write(t);
        self.len += 1;
    }

    #[inline(always)]
    fn as_slice(&self) -> &[T] {
        // Every item below `len` was written by `push`.
        unsafe { std::slice::from_raw_parts(self.items.as_ptr() as *const T, self.len) }
    }
}
const NO_SLOT: u8 = u8::MAX;

const EVAL_ORDER_DEPTH: u32 = 3;

const ABORTED: f32 = f32::NEG_INFINITY;

const MPC_RELAX_STEP: f32 = 1.18;

/// 2 discs: on 7 contested replays depth 31.00 against 30.71 with it off, same moves; 1 disc bought none.
const DECIDED: f32 = 2.0;

/// On 8 threads 0.3 left too few iterations to check: 0.1 stopped 9 of 20 live positions at 30 s (0.3: 3), same moves.
const DECIDED_AT: f32 = 0.1;

const DECIDED_DEPTH: u32 = 28;

/// Estimating it from the observed growth alone changed nothing (same nodes at 4 / 8 / 16 s).
const NEXT_PASS_FACTOR: f32 = 2.0;

/// Live 5-minute games tore down the unfinished depth on almost every midgame move at a fixed 2x.
const NEXT_PASS_MAX_GROWTH: f32 = 6.0;

/// The next depth's time at the growth the last two depths showed, never below the fixed factor.
fn next_pass_eta(last: std::time::Duration, prev: std::time::Duration) -> std::time::Duration {
    let growth = if prev.is_zero() {
        0.0
    } else {
        last.as_secs_f32() / prev.as_secs_f32()
    };
    last.mul_f32(growth.clamp(NEXT_PASS_FACTOR, NEXT_PASS_MAX_GROWTH))
}

knob!(fn ybwc_min_depth() -> u32 = "YBWC_MIN" or 6);
const ABORT_CHECK_INTERVAL: u64 = 512;

pub const MPC_MIN_DEPTH: u32 = 4;

const MPC_T: f32 = 1.1;

knob!(pub fn mpc_t() -> f32 = "MPC_T" or MPC_T);

const MPC_D0_OFFSET: f32 = 4.0;

const MPC_MAX_LEVEL: u32 = 3;

/// Off where the reduced search reaches the end: left on, it erred 6.4 discs against exact solves at 30 empties.
const MPC_NESTED_GAP: u32 = 6;

fn mpc_pv_factor(dev: u32) -> f32 {
    1.05 / (1.0 + 0.05 * dev as f32)
}

pub fn mpc_reduced_depth(depth: u32) -> u32 {
    2 * (depth / 4) + (depth & 1)
}

const STATIC_CELL_PRIORITY: [u64; 4] = [
    0x8100000000000081, // corner
    0x00003C24243C0000, // box
    0x3C3CC3C3C3C33C3C, // block
    0x42C300000000C342, // X, C
];

const CORNERS: u64 = 0x8100000000000081;

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

knob!(fn smp_min_depth() -> u32 = "SMP_MIN_DEPTH" or 1);
knob!(fn smp_spread() -> u32 = "SMP_SPREAD" or 2);
knob!(fn smp_sharpen_max() -> u32 = "SMP_SHARPEN_MAX" or 2);
knob!(fn smp_max_depth() -> u32 = "SMP_MAX_DEPTH" or 10);

const TT_WAYS: usize = 4;

#[inline]
fn tt_level(depth: u8, relax: u8) -> u32 {
    ((depth as u32) << 8) | relax as u32
}

pub struct SharedTt {
    buckets: std::cell::UnsafeCell<Vec<TtBucket>>,
    mask: u64,
    epoch: std::sync::atomic::AtomicU8,
}
unsafe impl Sync for SharedTt {}

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct TtBucket([TtEntry; TT_WAYS]);

impl SharedTt {
    /// The bytes the table takes at `bits`.
    pub fn bytes(bits: u32) -> usize {
        (1usize << bits.saturating_sub(2)) * std::mem::size_of::<TtBucket>()
    }

    pub fn new(bits: u32) -> SharedTt {
        let n = 1usize << bits.saturating_sub(2);
        SharedTt {
            buckets: std::cell::UnsafeCell::new(vec![TtBucket([TtEntry::EMPTY; TT_WAYS]); n]),
            mask: (n - 1) as u64,
            epoch: std::sync::atomic::AtomicU8::new(1),
        }
    }

    #[inline]
    fn epoch(&self) -> u8 {
        self.epoch.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[allow(clippy::mut_from_ref)]
    #[inline]
    fn bucket(&self, hash: u64) -> &mut TtBucket {
        unsafe {
            let v = &mut *self.buckets.get();
            v.get_unchecked_mut((hash & self.mask) as usize)
        }
    }

    #[allow(clippy::mut_from_ref)]
    #[inline]
    fn slot(&self, hash: u64, i: u64) -> &mut TtEntry {
        unsafe { self.bucket(hash).0.get_unchecked_mut(i as usize) }
    }

    #[inline]
    fn prefetch(&self, hash: u64) {
        crate::prefetch::l2(self.bucket(hash));
    }

    #[inline]
    fn get(&self, hash: u64) -> TtEntry {
        let b = self.bucket(hash);
        let now = self.epoch();
        for e in b.0.iter() {
            if e.key == hash && e.flag != 0 && e.epoch == now {
                return *e;
            }
        }
        TtEntry::EMPTY
    }

    pub fn best_move(&self, hash: u64) -> Option<u8> {
        let e = self.get(hash);
        (e.flag != 0 && e.best < 64).then_some(e.best)
    }

    pub fn seed_move(&self, hash: u64, best_move: u8) {
        if best_move >= 64 {
            return;
        }
        if self.best_move(hash).is_some() {
            return;
        }
        self.put(hash, 0, 0, f32::INFINITY, f32::NEG_INFINITY, 0.0, best_move);
    }

    #[inline]
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
        let now = self.epoch();
        for i in 0..TT_WAYS {
            let slot = self.slot(hash, i as u64);
            if slot.key == hash && slot.flag != 0 && slot.epoch == now {
                let slot_level = tt_level(slot.depth, slot.relax);
                if slot_level > level {
                    return;
                }
                if slot_level == level {
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
            if slot.flag == 0 || slot.epoch != now || tt_level(slot.depth, slot.relax) <= level {
                *slot = TtEntry {
                    key: hash,
                    lower,
                    upper,
                    best: best_move,
                    best2: 64,
                    depth,
                    flag: 1,
                    relax,
                    epoch: now,
                };
                return;
            }
        }
    }

    pub fn clear(&self) {
        let next = self.epoch().wrapping_add(1);
        if next != 0 {
            self.epoch.store(next, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        unsafe {
            for b in (*self.buckets.get()).iter_mut() {
                for e in b.0.iter_mut() {
                    *e = TtEntry::EMPTY;
                }
            }
        }
        self.epoch.store(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[derive(Clone, Copy)]
struct TtEntry {
    key: u64,
    lower: f32,
    upper: f32,
    best: u8,
    best2: u8,
    depth: u8,
    flag: u8,
    relax: u8,
    epoch: u8,
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
        epoch: 0,
    };
}

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
#[derive(Clone, Default)]
pub struct StopHandle(std::sync::Arc<std::sync::atomic::AtomicBool>);

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
impl StopHandle {
    pub fn new() -> StopHandle {
        StopHandle(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        )))
    }
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

#[derive(Default)]
struct Slot {
    done: std::sync::atomic::AtomicBool,
    nodes: std::sync::atomic::AtomicU64,
}

impl Slot {
    fn finish(&self, nodes: u64) {
        use std::sync::atomic::Ordering;
        self.nodes.store(nodes, Ordering::Relaxed);
        self.done.store(true, Ordering::Release);
    }
}

/// One node's unsearched siblings, left open for any idle thread to take.
struct Fan {
    kids: Kids,
    moves: Vec<(Position, u64)>,
    board: Board,
    alpha: f32,
    /// The search each kid starts from: the owner's settings, one deviation deeper.
    proto: NnueSearch,
}

impl HasKids for Fan {
    fn kids(&self) -> &Kids {
        &self.kids
    }
}

impl Fan {
    fn result(&self, k: usize) -> Option<f32> {
        self.kids.result(k).map(f32::from_bits)
    }

    fn search_kid(&self, k: usize) {
        use std::sync::atomic::Ordering;
        let (pos, flipped) = self.moves[k];
        let search = |stop: &std::sync::Arc<AbortChain>| {
            let mut child = self.board;
            child.apply_flips(pos, flipped);
            let mut w = self.proto.clone();
            w.abort = Some(stop.clone());
            w.abort_countdown = 1;
            let mut acc = w.nn.indices(child.black, child.white);
            let (lo, hi) = (-(self.alpha + PVS_EPS), -self.alpha);
            let v = w.negamax(&child, &mut acc, self.kids.depth(), lo, hi);
            self.kids.nodes.fetch_add(w.nodes, Ordering::Relaxed);
            v.to_bits()
        };
        // `ABORTED` is written like any value but never cuts: the kid only failed to finish.
        let cuts = |bits: u32| {
            let v = f32::from_bits(bits);
            v != ABORTED && -v > self.alpha
        };
        self.kids.run(k, search, cuts);
    }

    fn help(&self) -> bool {
        self.kids.search_free(|k| self.search_kid(k))
    }

    /// Searches, alongside its helper, a kid whose helper has not finished; true if there was one.
    fn take_over(&self) -> bool {
        let Some(k) = self.kids.take_over() else {
            return false;
        };
        self.search_kid(k);
        true
    }
}

struct Pool {
    fans: crate::fan::FanList<Fan>,
    q: std::sync::Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send + 'static>>>,
    cv: std::sync::Condvar,
    idle: std::sync::atomic::AtomicUsize,
    queued: std::sync::atomic::AtomicUsize,
}

impl Pool {
    fn new() -> Pool {
        use std::sync::atomic::AtomicUsize;
        Pool {
            q: std::sync::Mutex::new(std::collections::VecDeque::new()),
            cv: std::sync::Condvar::new(),
            fans: crate::fan::FanList::new(),
            idle: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
        }
    }

    fn push(&self, f: impl FnOnce() + Send + 'static) {
        use std::sync::atomic::Ordering;
        let mut q = self.q.lock().unwrap();
        q.push_back(Box::new(f));
        self.queued.fetch_add(1, Ordering::Relaxed);
        drop(q);
        self.cv.notify_one();
    }

    fn open_fan(&self, f: &std::sync::Arc<Fan>) {
        use std::sync::atomic::Ordering;
        self.fans.add(f);
        if self.idle.load(Ordering::Relaxed) > 0 {
            let _q = self.q.lock().unwrap();
            self.cv.notify_all();
        }
    }

    fn close_fan(&self, f: &std::sync::Arc<Fan>) {
        self.fans
            .close(f, |under| self.help(Some(under)), || f.take_over());
    }

    fn fan_open(&self) -> bool {
        self.fans.any_open()
    }

    fn help(&self, under: Option<&AbortChain>) -> bool {
        self.fans.help(under, |f| f.help())
    }

    fn run_one(&self) -> bool {
        use std::sync::atomic::Ordering;
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

    fn wait(&self, slot: &Slot) {
        use std::sync::atomic::Ordering;
        let mut idle = 0u32;
        while !slot.done.load(Ordering::Acquire) {
            if self.run_one() {
                idle = 0;
                continue;
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
    }

    fn worker_loop(&self) {
        use std::sync::atomic::Ordering;
        loop {
            self.idle.fetch_add(1, Ordering::Relaxed);
            let task = {
                let mut q = self.q.lock().unwrap();
                loop {
                    if let Some(t) = q.pop_front() {
                        self.queued.fetch_sub(1, Ordering::Relaxed);
                        break t;
                    }
                    drop(q);
                    if self.fan_open() {
                        self.idle.fetch_sub(1, Ordering::Relaxed);
                        self.help(None);
                        self.idle.fetch_add(1, Ordering::Relaxed);
                        q = self.q.lock().unwrap();
                        continue;
                    }
                    q = self.q.lock().unwrap();
                    if !q.is_empty() || self.fan_open() {
                        continue;
                    }
                    q = self.cv.wait(q).unwrap();
                }
            };
            self.idle.fetch_sub(1, Ordering::Relaxed);
            task();
        }
    }
}

/// Trips `stop` once `at` passes; storing `true` in the returned flag ends the watch.
#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
pub fn watch_deadline(
    at: crate::time::Instant,
    stop: StopHandle,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watching = done.clone();
    std::thread::spawn(move || {
        while !watching.load(std::sync::atomic::Ordering::Relaxed) {
            let now = crate::time::Instant::now();
            if now >= at {
                stop.stop();
                return;
            }
            std::thread::sleep((at - now).min(std::time::Duration::from_millis(20)));
        }
    });
    done
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub use single_thread_stop::{watch_deadline, StopHandle};

/// A browser runs the search on one thread, so nothing beside it can trip a deadline: the stop checks the clock itself.
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
mod single_thread_stop {
    use crate::time::Instant;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
    use std::sync::{Arc, Mutex};

    /// Checks between clock reads: a JS call each, and a solve asks every node.
    const POLLS_PER_READ: u32 = 1024;

    #[derive(Default)]
    struct State {
        stopped: AtomicBool,
        polls: AtomicU32,
        /// The deadline being watched, until its caller sets the flag beside it.
        watch: Mutex<Option<(Instant, Arc<AtomicBool>)>>,
    }

    #[derive(Clone, Default)]
    pub struct StopHandle(Arc<State>);

    impl StopHandle {
        pub fn new() -> StopHandle {
            StopHandle::default()
        }
        pub fn stop(&self) {
            self.0.stopped.store(true, Relaxed);
        }
        pub fn reset(&self) {
            self.0.stopped.store(false, Relaxed);
            *self.0.watch.lock().unwrap() = None;
        }
        #[inline]
        pub fn is_stopped(&self) -> bool {
            if self.0.stopped.load(Relaxed) {
                return true;
            }
            if !self
                .0
                .polls
                .fetch_add(1, Relaxed)
                .is_multiple_of(POLLS_PER_READ)
            {
                return false;
            }
            let mut watch = self.0.watch.lock().unwrap();
            let Some((at, done)) = watch.as_ref() else {
                return false;
            };
            if done.load(Relaxed) {
                *watch = None;
                return false;
            }
            if Instant::now() < *at {
                return false;
            }
            *watch = None;
            self.0.stopped.store(true, Relaxed);
            true
        }
    }

    /// Trips `stop` once `at` passes; storing `true` in the returned flag ends the watch.
    pub fn watch_deadline(at: Instant, stop: StopHandle) -> Arc<AtomicBool> {
        let done = Arc::new(AtomicBool::new(false));
        *stop.0.watch.lock().unwrap() = Some((at, done.clone()));
        done
    }
}

fn out_of_time(
    deadline: Option<crate::time::Instant>,
    reached: u32,
    last_pass: std::time::Duration,
    prev_pass: std::time::Duration,
) -> bool {
    deadline.is_some_and(|dl| {
        let now = crate::time::Instant::now();
        now >= dl || (reached > 0 && now + next_pass_eta(last_pass, prev_pass) >= dl)
    })
}

#[derive(Clone)]
pub struct NnueSearch {
    pub nn: std::sync::Arc<Nnue>,
    pub tt: std::sync::Arc<SharedTt>,
    pub threads: usize,
    pub nodes: u64,
    pub mpc: bool,
    probcut_level: u32,
    expect_all: bool,
    /// Times the current line left the first-searched move (0 on the principal variation).
    dev: u32,
    abort: Option<std::sync::Arc<AbortChain>>,
    stop: Option<StopHandle>,
    root_move: Option<Position>,
    root_hash: u64,
    progress: Option<std::sync::Arc<crate::engine::Progress>>,
    abort_countdown: u64,
    done: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    my_gen: u32,
    pool: Option<&'static Pool>,
    mpc_relax: u32,
    split_min: u32,
}

impl NnueSearch {
    pub fn new(nn: std::sync::Arc<Nnue>, tt: std::sync::Arc<SharedTt>) -> Self {
        NnueSearch {
            nn,
            tt,
            threads: 1,
            nodes: 0,
            mpc: false,
            probcut_level: 0,
            expect_all: false,
            dev: 0,
            abort: None,
            stop: None,
            root_move: None,
            root_hash: 0,
            progress: None,
            abort_countdown: ABORT_CHECK_INTERVAL,
            done: None,
            my_gen: 0,
            pool: None,
            mpc_relax: 0,
            split_min: ybwc_min_depth(),
        }
    }

    fn shared_pool(&self, workers: usize) -> Option<&'static Pool> {
        if workers == 0 {
            return None;
        }
        static POOL: std::sync::OnceLock<&'static Pool> = std::sync::OnceLock::new();
        Some(*POOL.get_or_init(|| {
            let pool: &'static Pool = Box::leak(Box::new(Pool::new()));
            for _ in 0..workers {
                std::thread::spawn(move || pool.worker_loop());
            }
            pool
        }))
    }

    fn worker(&self) -> NnueSearch {
        NnueSearch {
            mpc: self.mpc,
            abort: self.abort.clone(),
            stop: self.stop.clone(),
            done: self.done.clone(),
            my_gen: self.my_gen,
            pool: self.pool,
            mpc_relax: self.mpc_relax,
            split_min: self.split_min,
            ..NnueSearch::new(self.nn.clone(), self.tt.clone())
        }
    }

    #[inline]
    fn stopped(&self) -> bool {
        self.done
            .as_ref()
            .is_some_and(|g| g.load(std::sync::atomic::Ordering::Relaxed) != self.my_gen)
    }

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

    pub fn set_progress(&mut self, p: Option<std::sync::Arc<crate::engine::Progress>>) {
        self.progress = p;
    }

    pub fn set_stop(&mut self, stop: Option<StopHandle>) {
        self.stop = stop;
    }

    #[inline]
    fn should_stop_or_done(&mut self) -> bool {
        if self.should_stop() {
            return true;
        }
        self.stopped()
    }

    /// A sibling cut keeps what was searched; a teardown does not.
    #[inline]
    fn torn_down(&self) -> bool {
        self.stop.as_ref().is_some_and(|s| s.is_stopped()) || self.stopped()
    }

    pub fn clear(&mut self) {
        self.tt.clear();
    }

    pub fn best_move_deadline(
        &mut self,
        b: &Board,
        depth: u32,
        deadline: Option<crate::time::Instant>,
    ) -> (Option<Position>, f32, u32) {
        if b.movable() == 0 {
            // The side to move passes; the game goes on unless the opponent cannot move either.
            let mut passed = *b;
            passed.pass();
            if passed.movable() == 0 {
                return (None, final_value(b), 0);
            }
            let (_, v, reached) = self.best_move_deadline(&passed, depth, deadline);
            return (None, -v, reached);
        }
        let watcher = deadline.and_then(|dl| Some(watch_deadline(dl, self.stop.clone()?)));
        let r = self.iterate(b, depth, deadline);
        if let Some(d) = watcher {
            d.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        r
    }

    fn iterate(
        &mut self,
        b: &Board,
        depth: u32,
        deadline: Option<crate::time::Instant>,
    ) -> (Option<Position>, f32, u32) {
        if self.threads > 1 && depth >= 2 {
            return self.lazy_smp(b, depth, deadline);
        }
        let mut acc = self.nn.indices(b.black, b.white);
        let mut value = f32::NAN;
        let mut best = None;
        let mut last_best = None;
        let mut two_ago = None;
        let mut reached = 0;
        let mut last_pass = std::time::Duration::ZERO;
        let mut prev_pass = std::time::Duration::ZERO;
        let clock = deadline.map(|dl| (crate::time::Instant::now(), dl));
        for d in 1..=depth {
            if out_of_time(deadline, reached, last_pass, prev_pass) {
                break;
            }
            let t0 = crate::time::Instant::now();
            self.root_hash = zobrist::board_hash(b.player_bb(), b.opponent_bb());
            self.root_move = None;
            let v = self.negamax(b, &mut acc, d, f32::NEG_INFINITY, f32::INFINITY);
            if v == ABORTED || self.torn_down() {
                break;
            }
            prev_pass = last_pass;
            last_pass = t0.elapsed();
            value = v;
            best = self.root_move.or_else(|| self.root_best(b, &mut acc));
            reached = d;
            if self.settle(b, &acc, best, last_best, two_ago, d, value, clock) {
                break;
            }
            two_ago = last_best;
            last_best = best;
            if let Some(p) = self.progress.as_ref() {
                p.reached(d, best, v);
            }
        }
        (best.or_else(|| self.root_best(b, &mut acc)), value, reached)
    }

    fn settle(
        &mut self,
        b: &Board,
        acc: &PatternIndices,
        best: Option<Position>,
        last_best: Option<Position>,
        two_ago: Option<Position>,
        depth: u32,
        value: f32,
        clock: Option<(crate::time::Instant, crate::time::Instant)>,
    ) -> bool {
        let Some((start, soft)) = clock else {
            return false;
        };
        let budget = soft.saturating_duration_since(start).as_secs_f32();
        if start.elapsed().as_secs_f32() < budget * DECIDED_AT {
            return false;
        }
        // Live early stops at depth 19 and 25 each lost 2 discs; those at 28-39 lost none.
        let shallow = depth < DECIDED_DEPTH;
        // On live positions the best move alternated every depth from 24 to 29, so two back agrees too.
        !shallow
            && (best == last_best || best == two_ago)
            && self.others_behind(b, acc, best, depth, value, DECIDED) == Some(true)
    }

    /// `None` when torn down: no verdict either way.
    fn others_behind(
        &mut self,
        b: &Board,
        acc: &PatternIndices,
        best: Option<Position>,
        depth: u32,
        value: f32,
        margin: f32,
    ) -> Option<bool> {
        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0); MAX_KIDS];
        let mut scratch = *acc;
        let n = self.ordered_into(b, &mut scratch, 1, b.movable(), &mut kids, false, 64, 64);
        let want = depth.saturating_sub(1);
        let bar = value - margin;
        for pos in kids[..n].iter().map(|k| k.0).filter(|p| Some(*p) != best) {
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            let h = zobrist::board_hash(nb.player_bb(), nb.opponent_bb());
            let e = self.tt.get(h);
            let v = if e.key == h
                && e.flag != 0
                && e.lower.is_finite()
                && e.depth as u32 >= want
                && e.relax >= self.mpc_relax as u8
                && -e.lower < bar
            {
                -e.lower
            } else {
                let mut child = *acc;
                self.nn.ix_apply(&mut child, pos, flipped, b.player());
                let raw = self.negamax(&nb, &mut child, want, -bar, -bar + PVS_EPS);
                if raw == ABORTED || self.torn_down() {
                    return None;
                }
                -raw
            };
            if v >= bar {
                return Some(false);
            }
        }
        Some(true)
    }

    fn root_best(&self, b: &Board, acc: &mut PatternIndices) -> Option<Position> {
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let e = self.tt.get(h);
        if e.key == h && e.best < 64 {
            return Position::from_index(e.best as u32);
        }
        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0); MAX_KIDS];
        let n = self.ordered_into(b, acc, 1, b.movable(), &mut kids, false, 64, 64);
        (n > 0).then(|| kids[0].0)
    }

    fn lazy_smp(
        &mut self,
        b: &Board,
        depth: u32,
        deadline: Option<crate::time::Instant>,
    ) -> (Option<Position>, f32, u32) {
        use std::sync::atomic::Ordering;
        let mut nodes = 0;
        let mut acc = self.nn.indices(b.black, b.white);
        let mut value = f32::NAN;
        let mut best = None;
        let mut last_best = None;
        let mut two_ago = None;
        let mut reached = 0;
        let mut last_pass = std::time::Duration::ZERO;
        let mut prev_pass = std::time::Duration::ZERO;
        let clock = deadline.map(|dl| (crate::time::Instant::now(), dl));

        let workers = self.threads - 1;
        let pool = self.shared_pool(workers);
        // Two threads lose to the extra split points one ply down; three and more gain.
        self.split_min = ybwc_min_depth() - u32::from(self.threads >= 3);

        for main_depth in 1..=depth {
            if out_of_time(deadline, reached, last_pass, prev_pass) {
                break;
            }
            let t0 = crate::time::Instant::now();
            let lazy = main_depth >= smp_min_depth() && main_depth <= smp_max_depth();
            let gen = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(main_depth));
            let mut slots = Vec::new();
            let root_task = |mut w: NnueSearch, d: u32| {
                let slot = std::sync::Arc::new(Slot::default());
                w.done = Some(gen.clone());
                w.my_gen = main_depth;
                let task_slot = slot.clone();
                let root = *b;
                pool.unwrap().push(move || {
                    let mut wacc = w.nn.indices(root.black, root.white);
                    w.negamax(&root, &mut wacc, d, f32::NEG_INFINITY, f32::INFINITY);
                    task_slot.finish(w.nodes);
                });
                slot
            };
            if pool.is_some() && lazy {
                for idx in 0..workers {
                    let mut w = self.worker();
                    let ahead = (idx as u32 + 1).trailing_zeros() * smp_spread();
                    w.mpc_relax = (idx as u32 / 2).min(smp_sharpen_max());
                    slots.push(root_task(w, (main_depth + ahead).min(depth)));
                }
            } else {
                // One worker starts the next depth and splits it too: 8 threads 29.9 -> 24.9 s; two threads lose, 62.7 -> 64.6 s.
                if pool.is_some() && workers >= 2 && main_depth < depth {
                    let mut w = self.worker();
                    w.pool = pool;
                    slots.push(root_task(w, main_depth + 1));
                }
                self.pool = pool;
            }

            self.root_hash = zobrist::board_hash(b.player_bb(), b.opponent_bb());
            self.root_move = None;
            let v = self.negamax(b, &mut acc, main_depth, f32::NEG_INFINITY, f32::INFINITY);
            self.pool = None;

            gen.store(u32::MAX, Ordering::Relaxed);
            for slot in slots {
                pool.unwrap().wait(&slot);
                nodes += slot.nodes.load(Ordering::Relaxed);
            }

            if v == ABORTED || self.torn_down() {
                break;
            }
            prev_pass = last_pass;
            last_pass = t0.elapsed();
            value = v;
            best = self.root_move.or_else(|| self.root_best(b, &mut acc));
            reached = main_depth;
            if let Some(p) = self.progress.as_ref() {
                p.reached(main_depth, best, v);
            }
            if self.settle(b, &acc, best, last_best, two_ago, main_depth, value, clock) {
                break;
            }
            two_ago = last_best;
            last_best = best;
        }
        self.nodes += nodes;
        let out = best.or_else(|| self.root_best(b, &mut acc));
        (
            out,
            value,
            reached.max(if deadline.is_none() { depth } else { 0 }),
        )
    }

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
        // 0.25 measured: 17/7, the ratio that suits a disc-scaled pattern evaluation, searched 1.5x the nodes.
        let (w_mob, w_pm, w_val) = if null_window {
            (0.25, 0.0, 1.0)
        } else {
            (35.0 / 269.0, 17.0 / 269.0, 1.0)
        };
        let eval_order = depth >= EVAL_ORDER_DEPTH;
        let mut pend = KidStack::<(usize, Board, PatternIndices)>::new();
        let mut keys = [0f32; MAX_KIDS];
        let mut n = 0usize;
        let mut m = moves;
        while m != 0 {
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            m &= m - 1;
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            let key = if pos.index() == tt_move {
                1.0e9
            } else if pos.index() == tt_move2 {
                1.0e8
            } else {
                let legal = nb.movable();
                let mob = 38.0 - (legal.count_ones() * 2 + (legal & CORNERS).count_ones()) as f32;
                let mut k = mob * w_mob;
                if w_pm != 0.0 {
                    let empties = !(nb.black | nb.white);
                    k += (38.0 - potential_mobility(nb.opponent_bb(), empties) as f32) * w_pm;
                }
                if eval_order {
                    self.nn.prefetch_eval(&nb);
                    let mut child = *acc;
                    self.nn.ix_apply(&mut child, pos, flipped, mover);
                    pend.push((n, nb, child));
                }
                k
            };
            self.tt
                .prefetch(zobrist::board_hash(nb.player_bb(), nb.opponent_bb()));
            keys[n] = key;
            out[n] = (pos, flipped, 0);
            n += 1;
        }
        let pend = pend.as_slice();
        for pair in pend.chunks(2) {
            if let [a, b] = pair {
                let (x, y) = self.nn.eval_pair_from_indices((&a.2, &a.1), (&b.2, &b.1));
                keys[a.0] += -x * w_val;
                keys[b.0] += -y * w_val;
            } else {
                let a = &pair[0];
                keys[a.0] += -self.nn.eval_from_indices(&a.2, &a.1) * w_val;
            }
        }
        for i in 0..n {
            out[i].2 = order_key(keys[i]);
        }
        out[..n].sort_unstable_by_key(|k| core::cmp::Reverse(k.2));
        n
    }

    fn eval1_nws(&mut self, b: &Board, acc: &mut PatternIndices, alpha: f32) -> f32 {
        self.nodes += 1;
        let moves = b.movable();
        if moves == 0 {
            let mut nb = *b;
            nb.pass();
            if nb.movable() == 0 {
                return final_value(b);
            }
            let raw = self.eval1_nws(&nb, acc, -alpha - PVS_EPS);
            return -raw;
        }
        let mover = b.player();
        let mut v = f32::NEG_INFINITY;
        // The first child cuts 70% of the time; when it does not, the rest are scored in pairs.
        let mut rest = KidStack::<(Board, PatternIndices)>::new();
        let mut first = true;
        for mask in STATIC_CELL_PRIORITY {
            let mut l = moves & mask;
            while l != 0 {
                let pos = Position::from_index(l.trailing_zeros()).unwrap();
                l &= l - 1;
                let mut nb = *b;
                let flipped = nb.make_move_bits(pos);
                self.nn.prefetch_eval(&nb);
                let mut child = *acc;
                self.nn.ix_apply(&mut child, pos, flipped, mover);
                if !first {
                    rest.push((nb, child));
                    continue;
                }
                first = false;
                self.nodes += 1;
                let g = -self.nn.eval_from_indices(&child, &nb);
                if g > v {
                    if g > alpha {
                        return g;
                    }
                    v = g;
                }
            }
        }
        let rest = rest.as_slice();
        let n_rest = rest.len();
        let mut i = 0;
        while i < n_rest {
            let (ga, gb) = if i + 1 < n_rest {
                let (a, b) = (&rest[i], &rest[i + 1]);
                let (x, y) = self.nn.eval_pair_from_indices((&a.1, &a.0), (&b.1, &b.0));
                (-x, Some(-y))
            } else {
                (-self.nn.eval_from_indices(&rest[i].1, &rest[i].0), None)
            };
            for g in std::iter::once(ga).chain(gb) {
                self.nodes += 1;
                if g > v {
                    if g > alpha {
                        return g;
                    }
                    v = g;
                }
            }
            i += 2;
        }
        v
    }

    /// Opens the unsettled kids from `from` on to the pool; `None` when fewer than two remain.
    #[allow(clippy::too_many_arguments)]
    fn open_fan(
        &self,
        b: &Board,
        kids: &[Kid],
        settled: &[bool],
        from: usize,
        depth: u32,
        alpha: f32,
        stop: std::sync::Arc<AbortChain>,
        kids_all: bool,
        slot_of: &mut [u8; MAX_KIDS],
    ) -> Option<std::sync::Arc<Fan>> {
        let mut moves = Vec::new();
        for j in from..kids.len() {
            if !settled[j] {
                slot_of[j] = moves.len() as u8;
                moves.push((kids[j].0, kids[j].1));
            }
        }
        if moves.len() < 2 {
            *slot_of = [NO_SLOT; MAX_KIDS];
            return None;
        }
        let pool = self.pool.unwrap();
        let mut proto = self.worker();
        proto.abort = None;
        proto.dev = self.dev + 1;
        proto.expect_all = kids_all;
        let fan = std::sync::Arc::new(Fan {
            kids: Kids::new(moves.len(), depth, stop),
            moves,
            board: *b,
            alpha,
            proto,
        });
        pool.open_fan(&fan);
        Some(fan)
    }

    /// Pass, or the final score when neither side can move.
    #[inline(always)]
    fn after_pass(
        &mut self,
        b: &Board,
        acc: &mut PatternIndices,
        depth: u32,
        alpha: f32,
        beta: f32,
    ) -> f32 {
        let mut nb = *b;
        nb.pass();
        if nb.movable() == 0 {
            return final_value(b);
        }
        if depth == 0 {
            return self.nn.eval_from_indices(acc, b);
        }
        let raw = self.negamax(&nb, acc, depth, -beta, -alpha);
        if raw == ABORTED {
            ABORTED
        } else {
            -raw
        }
    }

    /// `Some` is an immediate cutoff; also returns the entry's two best moves (64 for none).
    #[inline(always)]
    fn tt_probe(
        &self,
        h: u64,
        depth: u32,
        alpha: &mut f32,
        beta: &mut f32,
    ) -> (Option<f32>, u8, u8) {
        let e = self.tt.get(h);
        if e.key != h || e.flag == 0 {
            return (None, 64, 64);
        }
        let usable = !(self.root_hash != 0 && h == self.root_hash)
            && e.depth as u32 >= depth
            && e.relax >= self.mpc_relax as u8;
        let mut cut = None;
        if usable {
            if e.upper <= *alpha || e.upper == e.lower {
                cut = Some(e.upper);
            } else if *beta <= e.lower {
                cut = Some(e.lower);
            } else {
                if *alpha < e.lower {
                    *alpha = e.lower;
                }
                if e.upper < *beta {
                    *beta = e.upper;
                }
            }
        }
        (cut, e.best, e.best2)
    }

    /// Multi-ProbCut: `Some` is a cutoff, or `ABORTED`.
    #[inline(always)]
    fn mpc_cut(
        &mut self,
        b: &Board,
        acc: &mut PatternIndices,
        depth: u32,
        alpha: f32,
        beta: f32,
    ) -> Option<f32> {
        if !(self.mpc
            && depth >= MPC_MIN_DEPTH
            && self.probcut_level < MPC_MAX_LEVEL
            && alpha.is_finite()
            && beta.is_finite()
            && is_null_window(alpha, beta))
        {
            return None;
        }
        let sigma = self.nn.mpc_sigma()?;
        let pd = mpc_reduced_depth(depth);
        if pd < 1 || pd >= depth {
            return None;
        }
        let mut t = mpc_t() * MPC_RELAX_STEP.powi(self.mpc_relax as i32) * mpc_pv_factor(self.dev);
        if let Some(a) = self.nn.mpc_alpha() {
            let mut o = *b;
            o.pass();
            t *= a.value(
                depth,
                b.empty_count() as u32,
                b.movable_count() as u32,
                o.movable_count() as u32,
            );
        }
        let margin = t * sigma.value(b.empty_count() as u32, depth, pd);
        let d0 = self.nn.eval_from_indices(acc, b);
        let gate = (margin - MPC_D0_OFFSET).max(1.0);
        let (try_high, try_low) = (d0 >= beta + gate, d0 <= alpha - gate);
        if try_high || try_low {
            self.probcut_level += 1;
            let saved_mpc = self.mpc;
            if (b.empty_count() as u32) < depth + MPC_NESTED_GAP {
                self.mpc = false;
            }
            let mut cut = None;
            let mut aborted = false;
            if try_high {
                let hi = beta + margin;
                self.expect_all = false;
                let high = self.negamax(b, acc, pd, hi - PVS_EPS, hi);
                if high == ABORTED {
                    aborted = true;
                } else if high >= hi {
                    cut = Some(beta);
                }
            }
            if !aborted && cut.is_none() && try_low {
                let lo = alpha - margin;
                self.expect_all = true;
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
                return Some(ABORTED);
            }
            if cut.is_some() {
                return cut;
            }
        }
        None
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
        let all_node = std::mem::take(&mut self.expect_all);
        if self.should_stop_or_done() {
            return ABORTED;
        }
        let moves = b.movable();
        if moves == 0 {
            self.expect_all = !all_node;
            let v = self.after_pass(b, acc, depth, alpha, beta);
            self.expect_all = false;
            return v;
        }
        if depth == 0 {
            return self.nn.eval_from_indices(acc, b);
        }

        if depth == 1 && is_null_window(alpha, beta) {
            return self.eval1_nws(b, acc, alpha);
        }

        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let (first_alpha, first_beta) = (alpha, beta);
        let mut beta = beta;
        let (cut, tt_move, tt_move2) = self.tt_probe(h, depth, &mut alpha, &mut beta);
        if let Some(v) = cut {
            return v;
        }

        if let Some(v) = self.mpc_cut(b, acc, depth, alpha, beta) {
            return v;
        }

        let mover = b.player();
        let mut best = f32::NEG_INFINITY;
        let mut best_val = f32::NEG_INFINITY;
        let mut best_move = 64u8;
        let mut moves = moves;

        let narrow = is_null_window(alpha, beta);
        let kids_all = narrow && !all_node;
        let will_split = self.pool.is_some() && depth >= self.split_min;
        // Every move of an expected All node gets searched, so its siblings need not wait for the first.
        let early = will_split && narrow && all_node;

        let mut pre_searched = false;
        if !early
            && tt_move < 64
            && depth >= 2
            && moves.count_ones() > 1
            && moves >> tt_move & 1 == 1
        {
            let pos = Position::from_index(tt_move as u32).unwrap();
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            let mut child = *acc;
            self.nn.ix_apply(&mut child, pos, flipped, mover);
            self.expect_all = kids_all;
            let raw = self.negamax(&nb, &mut child, depth - 1, -beta, -alpha);
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
        let null_window = is_null_window(alpha, beta);

        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0); MAX_KIDS];
        let ordered = depth >= 2 && moves.count_ones() > 1;
        let n_kids = if ordered {
            let first_key = if early { tt_move } else { 64 };
            self.ordered_into(
                b,
                acc,
                depth,
                moves,
                &mut kids,
                null_window,
                first_key,
                tt_move2,
            )
        } else {
            let mut n = 0usize;
            let mut m = moves;
            while m != 0 {
                let pos = Position::from_index(m.trailing_zeros()).unwrap();
                m &= m - 1;
                let mut nb = *b;
                kids[n] = (pos, nb.make_move_bits(pos), 0);
                n += 1;
            }
            n
        };
        if !ordered && tt_move < 64 {
            if let Some(i) = kids[..n_kids].iter().position(|k| k.0.index() == tt_move) {
                kids.swap(0, i);
            }
        }

        let at_root = self.root_hash != 0 && h == self.root_hash;
        let mut aborted = false;
        let mut settled = [false; MAX_KIDS];
        let mut n_settled = 0usize;
        let mut first = !pre_searched;

        let outer = if will_split { self.abort.clone() } else { None };
        'fanout: loop {
            let fan_stop = if will_split {
                Some(AbortChain::child(outer.clone()))
            } else {
                None
            };
            let mut stop_fanout = false;
            let mut fan: Option<std::sync::Arc<Fan>> = None;
            let mut fan_tried = false;
            let mut slot_of = [NO_SLOT; MAX_KIDS];
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
                let mut nb = *b;
                nb.apply_flips(pos, flipped);

                if first {
                    if early {
                        let stop = fan_stop.clone().unwrap();
                        fan = self.open_fan(
                            b,
                            &kids[..n_kids],
                            &settled,
                            i + 1,
                            depth - 1,
                            alpha,
                            stop,
                            kids_all,
                            &mut slot_of,
                        );
                        fan_tried = true;
                        self.abort = fan_stop.clone();
                    }
                    let mut child = *acc;
                    self.nn.ix_apply(&mut child, pos, flipped, mover);
                    self.expect_all = kids_all;
                    let raw = self.negamax(&nb, &mut child, depth - 1, -beta, -alpha);
                    if early {
                        self.abort = outer.clone();
                    }
                    if raw == ABORTED {
                        if !early || self.torn_down() || outer.as_ref().is_some_and(|o| o.stopped())
                        {
                            aborted = true;
                        }
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
                        if let Some(f) = &fan_stop {
                            f.raise();
                        }
                        break;
                    }
                    continue;
                }

                if will_split && !fan_tried {
                    let stop = fan_stop.clone().unwrap();
                    fan = self.open_fan(
                        b,
                        &kids[..n_kids],
                        &settled,
                        i,
                        depth - 1,
                        alpha,
                        stop,
                        kids_all,
                        &mut slot_of,
                    );
                    fan_tried = true;
                }
                if let Some(f) = fan.as_ref().filter(|_| slot_of[i] != NO_SLOT) {
                    if !f.kids.take(slot_of[i] as usize) {
                        continue;
                    }
                }

                if will_split {
                    self.abort = fan_stop.clone();
                }
                let mut child = *acc;
                self.nn.ix_apply(&mut child, pos, flipped, mover);
                self.dev += 1;
                self.expect_all = kids_all;
                let raw = self.negamax(&nb, &mut child, depth - 1, -(alpha + PVS_EPS), -alpha);
                self.dev -= 1;
                if will_split {
                    self.abort = outer.clone();
                }
                if raw == ABORTED {
                    if !will_split
                        || self.torn_down()
                        || outer.as_ref().is_some_and(|o| o.stopped())
                    {
                        aborted = true;
                    }
                    break;
                }
                let g = -raw;
                if g > best {
                    best = g;
                }
                if g > alpha && g > best_val {
                    best_val = g;
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

            let helped = fan.take();
            if let Some(f) = &helped {
                self.pool.unwrap().close_fan(f);
                self.nodes += f.kids.nodes.load(std::sync::atomic::Ordering::Relaxed);
            }
            for i in 0..n_kids {
                let Some(raw) = helped
                    .as_ref()
                    .filter(|_| slot_of[i] != NO_SLOT)
                    .and_then(|f| f.result(slot_of[i] as usize))
                else {
                    continue;
                };
                if raw == ABORTED || aborted {
                    if raw == ABORTED && self.torn_down() {
                        aborted = true;
                    }
                    continue;
                }
                let g = -raw;
                if g > best {
                    best = g;
                }
                if g > alpha && g > best_val {
                    best_val = g;
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

            if outer.as_ref().is_some_and(|o| o.stopped()) {
                aborted = true;
            }
            if aborted {
                break 'fanout;
            }
            if research.is_empty() {
                break 'fanout;
            }
            if next_alpha >= beta {
                break 'fanout;
            }

            alpha = next_alpha;
            for &i in &research {
                let (pos, flipped, _) = kids[i];
                let mut nb = *b;
                nb.apply_flips(pos, flipped);
                let mut child = *acc;
                self.nn.ix_apply(&mut child, pos, flipped, mover);
                self.dev += 1;
                let raw = self.negamax(&nb, &mut child, depth - 1, -beta, -alpha);
                self.dev -= 1;
                if raw == ABORTED {
                    aborted = true;
                    break 'fanout;
                }
                settled[i] = true;
                n_settled += 1;
                let g = -raw;
                if g > best {
                    best = g;
                }
                if g > best_val {
                    best_val = g;
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

        // `ABORTED` equals `best`'s start (-inf): storing it would poison the table.
        if aborted || best == ABORTED {
            return ABORTED;
        }

        if at_root && best_move < 64 {
            self.root_move = Position::from_index(best_move as u32);
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

    #[test]
    fn deadline_cuts_the_search_short() {
        let mut nn0 = Nnue::new(crate::nnue::test_patterns());
        nn0.quantize();
        let nn = std::sync::Arc::new(nn0);
        let tt = std::sync::Arc::new(SharedTt::new(18));
        let mut s = NnueSearch::new(nn, tt);
        s.threads = 1;
        s.set_stop(Some(StopHandle::new()));
        let b = Board::new();
        let t0 = crate::time::Instant::now();
        let dl = t0 + std::time::Duration::from_millis(120);
        let (pos, _v, reached) = s.best_move_deadline(&b, 40, Some(dl));
        let el = t0.elapsed();
        assert!(pos.is_some(), "a move returns even when shallow");
        assert!(reached >= 1, "at least one iteration completes");
        assert!(
            reached < 40,
            "the deadline cuts deepening (reached {reached})"
        );
        assert!(
            el < std::time::Duration::from_secs(3),
            "must not overshoot the deadline by much ({el:?})"
        );
    }

    #[test]
    fn a_cut_search_leaves_no_sentinel_bound() {
        let mut nn0 = Nnue::new(crate::nnue::test_patterns());
        nn0.quantize();
        let nn = std::sync::Arc::new(nn0);
        let tt = std::sync::Arc::new(SharedTt::new(18));
        let mut s = NnueSearch::new(nn, tt);
        s.threads = 4;
        s.set_stop(Some(StopHandle::new()));
        let b = Board::new();
        let dl = crate::time::Instant::now() + std::time::Duration::from_millis(400);
        let (_pos, _v, reached) = s.best_move_deadline(&b, 40, Some(dl));
        assert!(
            reached < 40,
            "the deadline has to bite for this to mean anything"
        );

        let mut boards = vec![b];
        let mut m = b.movable();
        while m != 0 {
            let pos = Position::from_index(m.trailing_zeros()).unwrap();
            m &= m - 1;
            let mut child = b;
            child.make_move_bits(pos);
            boards.push(child);
        }
        for c in boards {
            let e =
                s.tt.get(zobrist::board_hash(c.player_bb(), c.opponent_bb()));
            if e.flag == 0 {
                continue;
            }
            assert!(
                e.upper != f32::NEG_INFINITY && e.lower != f32::INFINITY,
                "stored a sentinel bound: {} .. {}",
                e.lower,
                e.upper
            );
        }
    }

    #[test]
    fn deadline_cuts_the_search_short_in_parallel() {
        let mut nn0 = Nnue::new(crate::nnue::test_patterns());
        nn0.quantize();
        let nn = std::sync::Arc::new(nn0);
        let tt = std::sync::Arc::new(SharedTt::new(18));
        let mut s = NnueSearch::new(nn, tt);
        s.threads = 4;
        s.set_stop(Some(StopHandle::new()));
        let b = Board::new();
        let t0 = crate::time::Instant::now();
        let dl = t0 + std::time::Duration::from_millis(200);
        let (pos, _v, reached) = s.best_move_deadline(&b, 40, Some(dl));
        let el = t0.elapsed();
        assert!(pos.is_some(), "a move returns");
        assert!(
            reached < 40,
            "the deadline cuts deepening (reached {reached})"
        );
        assert!(
            el < std::time::Duration::from_secs(3),
            "parallel search must not overshoot the deadline ({el:?})"
        );
    }
}

#[cfg(test)]
mod seed_tests {
    use super::*;

    #[test]
    fn a_seeded_move_carries_no_bounds() {
        let tt = SharedTt::new(16);
        let b = Board::new();
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        assert_eq!(tt.best_move(h), None, "empty at first");

        tt.seed_move(h, 19);
        assert_eq!(tt.best_move(h), Some(19), "the move is stored");
        let e = tt.get(h);
        assert!(
            e.lower.is_infinite() && e.lower < 0.0,
            "lower bound must be -inf"
        );
        assert!(
            e.upper.is_infinite() && e.upper > 0.0,
            "upper bound must be +inf"
        );
        let (alpha, beta) = (-5.0f32, 5.0f32);
        assert!(e.upper > alpha, "would cut on the upper bound");
        assert!(
            beta < e.lower || e.lower.is_infinite(),
            "would cut on the lower bound"
        );
        assert!(e.upper != e.lower, "would be read as exact");
    }

    #[test]
    fn a_seed_never_overwrites_a_real_result() {
        let tt = SharedTt::new(16);
        let b = Board::new();
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        tt.put(h, 8, 0, -1.0, 1.0, 0.5, 20);
        assert_eq!(tt.best_move(h), Some(20));
        tt.seed_move(h, 19);
        assert_eq!(tt.best_move(h), Some(20), "the real entry was lost");
    }

    #[test]
    fn an_out_of_range_seed_is_ignored() {
        let tt = SharedTt::new(16);
        let b = Board::new();
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        tt.seed_move(h, 64);
        assert_eq!(tt.best_move(h), None);
    }
}

#[cfg(test)]
mod order_key_tests {
    use super::order_key;

    fn agrees(a: f32, b: f32) -> bool {
        a.total_cmp(&b) == order_key(a).cmp(&order_key(b))
    }

    #[test]
    fn it_matches_total_cmp_on_ordinary_values() {
        let vs = [
            -1.0e9, -269.0, -38.0, -1.0, -0.5, -1e-30, 0.0, 1e-30, 0.5, 1.0, 38.0, 269.0, 1.0e8,
            1.0e9,
        ];
        for &a in &vs {
            for &b in &vs {
                assert!(agrees(a, b), "{a} vs {b}");
            }
        }
    }

    #[test]
    fn it_keeps_the_table_moves_on_top() {
        assert!(order_key(1.0e9) > order_key(1.0e8));
        assert!(order_key(1.0e8) > order_key(269.0));
    }

    #[test]
    fn it_handles_the_sign_boundary() {
        let mut x = -64.0f32;
        while x <= 64.0 {
            let mut y = -64.0f32;
            while y <= 64.0 {
                assert!(agrees(x, y), "{x} vs {y}");
                y += 0.25;
            }
            x += 0.25;
        }
    }

    #[test]
    fn it_matches_on_zeros_and_infinities() {
        let vs = [-0.0f32, 0.0, f32::NEG_INFINITY, f32::INFINITY, -1.0, 1.0];
        for &a in &vs {
            for &b in &vs {
                assert!(agrees(a, b), "{a} vs {b}");
            }
        }
    }
}
