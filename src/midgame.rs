//! Midgame NNUE search — the engine that plays matches.

#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]

use crate::nnue::Nnue;
use crate::pattern_index::PatternIndices;
use crate::zobrist;
use crate::{Board, Position};

const PVS_EPS: f32 = 0.01;

type Kid = (Position, u64, i32);

#[inline]
fn order_key(x: f32) -> i32 {
    let b = x.to_bits() as i32;
    b ^ (((b >> 31) as u32) >> 1) as i32
}

const MAX_KIDS: usize = 34;

fn eval_order_depth() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EVAL_ORDER_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3)
    })
}

const ABORTED: f32 = f32::NEG_INFINITY;

pub static ROOT_DIFF: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static ROOT_TOTAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const MPC_RELAX_STEP: f32 = 1.18;

fn next_pass_factor() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NEXT_PASS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2.0)
    })
}

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

const MPC_MIN_DEPTH: u32 = 4;

const MPC_T: f32 = 1.1;

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

const MPC_D0_OFFSET: f32 = 4.0;

fn mpc_old() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("MPC_OLD").is_ok_and(|v| v != "0"))
}

const MPC_MAX_LEVEL: u32 = 3;

pub fn mpc_reduced_depth(depth: u32) -> u32 {
    2 * (depth / 4) + (depth & 1)
}

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

fn pool_slack() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("POOL_SLACK")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2)
    })
}

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
        #[cfg(target_arch = "aarch64")]
        {
            let p = self.bucket(hash) as *const TtBucket as *const u8;
            unsafe {
                std::arch::asm!("prfm pldl2keep, [{0}]", in(reg) p, options(nostack, readonly));
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = hash;
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
            return; // never clobber a real entry
        }
        self.put(hash, 0, 0, f32::INFINITY, f32::NEG_INFINITY, 0.0, best_move);
    }

    #[inline]
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

pub static SPLIT_TRIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static WORKER_BUSY_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static POOL_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static SPLIT_DONE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static WAIT_IDLE_MAIN_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static WAIT_IDLE_WORKER_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static BUSY_HIST: [std::sync::atomic::AtomicU64; 17] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 17];
pub static SEARCH_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    static IS_POOL_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SPLIT_TRIED_LOCAL: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Default)]
pub struct StopHandle(std::sync::Arc<std::sync::atomic::AtomicBool>);

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

struct Pool {
    q: std::sync::Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send + 'static>>>,
    cv: std::sync::Condvar,
    idle: std::sync::atomic::AtomicUsize,
    queued: std::sync::atomic::AtomicUsize,
    workers: usize,
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

    fn try_push(&self, f: impl FnOnce() + Send + 'static) -> bool {
        use std::sync::atomic::Ordering;
        if self.workers == 0 {
            return false;
        }
        SPLIT_TRIED_LOCAL.with(|c| {
            let n = c.get() + 1;
            if n >= 4096 {
                c.set(0);
                SPLIT_TRIED.fetch_add(n, Ordering::Relaxed);
            } else {
                c.set(n);
            }
        });
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
            self.idle.fetch_add(1, Ordering::Relaxed);
            #[allow(clippy::never_loop)]
            let task = 'get: loop {
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

pub struct NnueSearch {
    pub nn: std::sync::Arc<Nnue>,
    pub tt: std::sync::Arc<SharedTt>,
    pub threads: usize,
    pub nodes: u64,
    pub mpc: bool,
    probcut_level: u32,
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
        }
    }

    fn shared_pool(&self, workers: usize) -> Option<&'static Pool> {
        if workers == 0 {
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
            nn: self.nn.clone(),
            tt: self.tt.clone(),
            threads: 1,
            nodes: 0,
            mpc: self.mpc,
            probcut_level: 0,
            abort: self.abort.clone(),
            stop: self.stop.clone(),
            root_move: None,
            root_hash: 0,
            abort_countdown: ABORT_CHECK_INTERVAL,
            done: self.done.clone(),
            my_gen: self.my_gen,
            pool: self.pool,
            mpc_relax: self.mpc_relax,
            progress: None,
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
        self.done.is_some() && self.stopped()
    }

    pub fn clear(&mut self) {
        self.tt.clear();
    }

    pub fn best_move(&mut self, b: &Board, depth: u32) -> Option<Position> {
        self.best_move_valued(b, depth).0
    }

    pub fn best_move_valued(&mut self, b: &Board, depth: u32) -> (Option<Position>, f32) {
        let (p, v, _) = self.best_move_deadline(b, depth, None);
        (p, v)
    }

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
        if self.threads > 1 && depth >= 2 {
            return self.lazy_smp(b, depth, deadline);
        }
        let mut acc = self.nn.indices(b.black, b.white);
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
                if reached > 0 && now + last_pass.mul_f32(next_pass_factor()) >= dl {
                    break;
                }
            }
            let t0 = std::time::Instant::now();
            self.root_hash = zobrist::board_hash(b.player_bb(), b.opponent_bb());
            self.root_move = None;
            let v = self.negamax(b, &mut acc, d, f32::NEG_INFINITY, f32::INFINITY);
            if v == ABORTED || self.stop.as_ref().is_some_and(|s| s.is_stopped()) {
                break;
            }
            last_pass = t0.elapsed();
            value = v;
            best = self.root_move.or_else(|| self.root_best(b, &mut acc));
            reached = d;
            if let Some(p) = self.progress.as_ref() {
                p.reached(d, best, v);
            }
        }
        (best.or_else(|| self.root_best(b, &mut acc)), value, reached)
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
        deadline: Option<std::time::Instant>,
    ) -> (Option<Position>, f32, u32) {
        use std::sync::atomic::{AtomicU64, Ordering};
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

        fn smp_max_depth() -> u32 {
            static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
            *V.get_or_init(|| env_u32("SMP_MAX_DEPTH", 10))
        }

        let nodes = AtomicU64::new(0);
        let mut acc = self.nn.indices(b.black, b.white);
        let mut value = f32::NAN;
        let mut best = None;
        let mut reached = 0;
        let mut last_pass = std::time::Duration::ZERO;

        let workers = self.threads - 1;
        let pool = self.shared_pool(workers);

        {
            for main_depth in 1..=depth {
                if let Some(dl) = deadline {
                    let now = std::time::Instant::now();
                    if now >= dl {
                        break;
                    }
                    if reached > 0 && now + last_pass.mul_f32(next_pass_factor()) >= dl {
                        break;
                    }
                }
                let t0 = std::time::Instant::now();
                let lazy = main_depth >= smp_min_depth() && main_depth <= smp_max_depth();
                let gen = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(main_depth));
                let mut slots = Vec::new();
                if let Some(pool) = pool.filter(|_| lazy) {
                    for idx in 0..workers {
                        let slot = std::sync::Arc::new(Slot::new());
                        let mut w = self.worker();
                        w.done = Some(gen.clone());
                        w.my_gen = main_depth;
                        let ahead = (idx as u32 + 1).trailing_zeros() * smp_spread();
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
                    self.pool = pool;
                }

                self.root_hash = zobrist::board_hash(b.player_bb(), b.opponent_bb());
                self.root_move = None;
                let v = self.negamax(b, &mut acc, main_depth, f32::NEG_INFINITY, f32::INFINITY);
                self.pool = None;

                gen.store(u32::MAX, Ordering::Relaxed);
                for slot in slots {
                    pool.unwrap().wait(&slot);
                    nodes.fetch_add(slot.nodes.load(Ordering::Relaxed), Ordering::Relaxed);
                }

                if v == ABORTED || self.stop.as_ref().is_some_and(|s| s.is_stopped()) {
                    break;
                }
                last_pass = t0.elapsed();
                value = v;
                best = self.root_move.or_else(|| self.root_best(b, &mut acc));
                reached = main_depth;
                if let Some(p) = self.progress.as_ref() {
                    p.reached(main_depth, best, v);
                }
                if std::env::var("ROOT_TRACE").is_ok() {
                    eprintln!(
                        "  iter [{:016x}] {main_depth:2} {v:+8.2} {:?} {:.1}s",
                        self.root_hash,
                        best,
                        t0.elapsed().as_secs_f32()
                    );
                }
            }
        }
        self.nodes += nodes.load(Ordering::Relaxed);
        let out = best.or_else(|| self.root_best(b, &mut acc));
        if std::env::var("ROOT_DIFF").is_ok() {
            let from_tt = self.root_best(b, &mut acc);
            ROOT_TOTAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if from_tt != out {
                ROOT_DIFF.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                eprintln!(
                    "  returning {:?} / table {:?} (depth {reached})",
                    out.map(|p| p.index()),
                    from_tt.map(|p| p.index())
                );
            }
        }
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
                    let mut child = *acc;
                    self.nn.ix_apply(&mut child, pos, flipped, mover);
                    k += -self.nn.eval_from_indices(&child, &nb) * w_val;
                }
                k
            };
            self.tt
                .prefetch(zobrist::board_hash(nb.player_bb(), nb.opponent_bb()));
            out[n] = (pos, flipped, order_key(key));
            n += 1;
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
                let mut child = *acc;
                self.nn.ix_apply(&mut child, pos, flipped, mover);
                let g = -self.nn.eval_from_indices(&child, &nb);
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

    /// Hands one child to the pool at a null window; `None` when it is full.
    fn spawn_kid(
        &self,
        child: Board,
        depth: u32,
        alpha: f32,
        stop: std::sync::Arc<AbortChain>,
    ) -> Option<std::sync::Arc<Slot>> {
        let pool = self.pool.unwrap();
        let slot = std::sync::Arc::new(Slot::new());
        let (nn, tt, mpc, relax) = (self.nn.clone(), self.tt.clone(), self.mpc, self.mpc_relax);
        let (gen, my_gen) = (self.done.clone(), self.my_gen);
        let ext_stop = self.stop.clone();
        let task_slot = slot.clone();
        let pushed = pool.try_push(move || {
            if stop.stopped() {
                task_slot.set(ABORTED, 0);
                return;
            }
            let mut w = NnueSearch::new(nn, tt);
            w.mpc = mpc;
            w.mpc_relax = relax;
            w.pool = Some(pool);
            w.abort = Some(stop.clone());
            w.stop = ext_stop;
            w.abort_countdown = 1;
            w.done = gen;
            w.my_gen = my_gen;
            let mut cacc = w.nn.indices(child.black, child.white);
            let v = w.negamax(&child, &mut cacc, depth, -(alpha + PVS_EPS), -alpha);
            if v != ABORTED && -v > alpha {
                stop.raise();
            }
            task_slot.set(v, w.nodes);
        });
        pushed.then_some(slot)
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
        if raw == ABORTED {
            ABORTED
        } else {
            -raw
        }
    }

    /// Narrows the window from the table; `Some` is an immediate cutoff.
    /// Also returns the entry's two best moves (64 for none).
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
            && depth >= mpc_min_depth()
            && self.probcut_level < MPC_MAX_LEVEL
            && alpha.is_finite()
            && beta.is_finite()
            && beta - alpha <= PVS_EPS * 1.5)
        {
            return None;
        }
        let sigma = self.nn.mpc_sigma()?;
        let pd = mpc_reduced_depth(depth);
        if pd < 1 || pd >= depth {
            return None;
        }
        let t = mpc_t() * MPC_RELAX_STEP.powi(self.mpc_relax as i32);
        let margin = t * sigma.value(b.empty_count() as u32, depth, pd);
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
        if self.should_stop_or_done() {
            return ABORTED;
        }
        let moves = b.movable();
        if moves == 0 {
            return self.after_pass(b, acc, depth, alpha, beta);
        }
        if depth == 0 {
            return self.nn.eval_from_indices(acc, b);
        }

        if depth == 1 && beta - alpha <= PVS_EPS * 1.5 {
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

        let mut pre_searched = false;
        if tt_move < 64 && depth >= 2 && moves.count_ones() > 1 && moves >> tt_move & 1 == 1 {
            let pos = Position::from_index(tt_move as u32).unwrap();
            let mut nb = *b;
            let flipped = nb.make_move_bits(pos);
            let mut child = *acc;
            self.nn.ix_apply(&mut child, pos, flipped, mover);
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
        let null_window = beta - alpha <= PVS_EPS * 1.5;

        let mut kids: [Kid; MAX_KIDS] = [(Position(0), 0, 0); MAX_KIDS];
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

        let split_ok = will_split;

        let mut aborted = false;
        let mut settled = [false; MAX_KIDS];
        let mut n_settled = 0usize;
        let mut first = !pre_searched;

        let outer = if split_ok { self.abort.clone() } else { None };
        'fanout: loop {
            let fan_stop = if split_ok {
                Some(AbortChain::child(outer.clone()))
            } else {
                None
            };
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
                let mut nb = *b;
                nb.apply_flips(pos, flipped);

                if first {
                    let mut child = *acc;
                    self.nn.ix_apply(&mut child, pos, flipped, mover);
                    let raw = self.negamax(&nb, &mut child, depth - 1, -beta, -alpha);
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

                let is_last = (i + 1..n_kids).all(|j| settled[j]);
                if split_ok && !is_last {
                    let stop = fan_stop.clone().unwrap();
                    if let Some(slot) = self.spawn_kid(nb, depth - 1, alpha, stop) {
                        pending.push((i, slot));
                        continue;
                    }
                }

                if split_ok {
                    self.abort = fan_stop.clone();
                }
                let mut child = *acc;
                self.nn.ix_apply(&mut child, pos, flipped, mover);
                let raw = self.negamax(&nb, &mut child, depth - 1, -(alpha + PVS_EPS), -alpha);
                if split_ok {
                    self.abort = outer.clone();
                }
                if raw == ABORTED {
                    if !split_ok || outer.as_ref().is_some_and(|o| o.stopped()) {
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
                let raw = self.negamax(&nb, &mut child, depth - 1, -beta, -alpha);
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

        if aborted {
            return ABORTED;
        }

        if self.root_hash != 0 && h == self.root_hash && best_move < 64 {
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
        let t0 = std::time::Instant::now();
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
    fn deadline_cuts_the_search_short_in_parallel() {
        let mut nn0 = Nnue::new(crate::nnue::test_patterns());
        nn0.quantize();
        let nn = std::sync::Arc::new(nn0);
        let tt = std::sync::Arc::new(SharedTt::new(18));
        let mut s = NnueSearch::new(nn, tt);
        s.threads = 4;
        s.set_stop(Some(StopHandle::new()));
        let b = Board::new();
        let t0 = std::time::Instant::now();
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
