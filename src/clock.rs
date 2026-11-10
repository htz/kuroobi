//! The search for games on a clock: one alpha-beta search from the first move to the solve.
//! Iterative deepening by two plies up to the full depth, then the same full-depth search again
//! with ever wider ProbCut margins until only the exact solve is left. Integer disc scores,
//! helper threads over one shared table, and a time rule from the clock alone.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::bitboard;
use crate::midgame::StopHandle;
use crate::nnue::{MpcSigma, Nnue};
use crate::pattern_index::PatternIndices;
use crate::solver::final_score_bb;
use crate::zobrist;
use crate::{Board, Position};

const MIN: i32 = -127;
const MAX: i32 = 127;
const NO_MOVE: u8 = 64;
/// ProbCut probe depth for each search depth (-1: no probe); none from depth 35 up.
const PROBE_DEPTH: [i8; 35] = [
    -1, -1, 0, 1, 2, 1, 2, 3, 4, 3, 4, 3, 4, 5, 4, 5, 4, 5, 6, 7, 8, 9, 10, 9, 10, 9, 10, 9, 10, 9,
    10, 9, 10, 9, 10,
];
/// ProbCut margins in half-sigmas, widened one step per full-depth pass; after the last, the solve.
const MARGINS: [i32; 5] = [3, 4, 5, 6, 8];
/// Past this many discs on the board the rest is searched exactly.
const EXACT_DISCS: i32 = 60;
/// Nodes this deep share their siblings after the first; 8 threads, depth 24: 7 / 8 / 10 gave 2.03 / 1.87 / 1.89 s.
const SPLIT_MIN: i32 = 8;
const HISTORY_DECAY: i32 = 8;
const STATIC_BONUS: [i32; 64] = {
    let mut t = [0i32; 64];
    let mut i = 0;
    while i < 64 {
        let (f, r) = (i % 8, i / 8);
        let corner = (f == 0 || f == 7) && (r == 0 || r == 7);
        let near = (f <= 1 || f >= 6) && (r <= 1 || r >= 6);
        t[i] = if corner {
            1_000_000
        } else if near {
            -100_000
        } else {
            0
        };
        i += 1;
    }
    t
};

/// Kept back from the clock for move transmission.
const CLOCK_MARGIN: Duration = Duration::from_secs(2);
const MIN_REMAINING: Duration = Duration::from_millis(200);
/// Budget per move: `BUDGET_SHARE * remaining / max(MOVES_LEFT_FLOOR, PACE_DISCS - discs)`.
const BUDGET_SHARE: f64 = 1.8;
const PACE_DISCS: f64 = 37.0;
const MOVES_LEFT_FLOOR: f64 = 4.8;
const MIN_BUDGET_SECS: f64 = 0.02;
/// The first move searched in a game has an empty table and gets this much more.
const FIRST_MOVE_EXTRA: f64 = 1.2;
const FIRST_MOVE_EXTRA_DISCS: i32 = 40;
/// Past this share of the budget no new pass starts unless the best move just changed or failed low.
const SETTLED_SHARE: f64 = 0.5;
/// A move is played at once when every other move fails low at its score minus this many discs.
const SEP_THRESHOLD: i32 = 5;
const SEP_MIN_DEPTH: i32 = 20;
/// Passes in a row that must keep the best move before the separation test runs.
const SEP_STABLE_PASSES: u32 = 2;
/// A move within this many discs of the best makes the position hard, once per move from this many empties.
const EXT_CLOSE: i32 = 1;
const EXT_MIN_EMPTIES: i32 = 38;
/// A hard position adds this many average move shares to its budget, up to half the clock.
const EXT_SHARE: f64 = 1.8;
const EXT_MAX_SHARE_OF_CLOCK: f64 = 0.5;

/// Two-way buckets of (key ^ data, data), so a torn write fails the key check.
struct Table {
    slots: Vec<(AtomicU64, AtomicU64)>,
    mask: usize,
    generation: AtomicU64,
}

#[derive(Clone, Copy)]
struct Entry {
    lower: i32,
    upper: i32,
    mv: u8,
    depth: u8,
    margin: u8,
    generation: u8,
}

impl Entry {
    fn pack(self) -> u64 {
        (self.lower as i8 as u8 as u64)
            | (self.upper as i8 as u8 as u64) << 8
            | (self.mv as u64) << 16
            | (self.depth as u64) << 24
            | (self.margin as u64) << 32
            | (self.generation as u64) << 40
            | 1 << 48
    }

    fn unpack(d: u64) -> Entry {
        Entry {
            lower: d as u8 as i8 as i32,
            upper: (d >> 8) as u8 as i8 as i32,
            mv: (d >> 16) as u8,
            depth: (d >> 24) as u8,
            margin: (d >> 32) as u8,
            generation: (d >> 40) as u8,
        }
    }

    /// Replaces `old` for the same position or a same-generation slot.
    fn beats(&self, old: &Entry) -> bool {
        self.depth >= old.depth || self.margin > old.margin
    }
}

impl Table {
    fn new(bits: u32) -> Table {
        let n = 1usize << bits.max(1);
        Table {
            slots: (0..n)
                .map(|_| (AtomicU64::new(0), AtomicU64::new(0)))
                .collect(),
            mask: n - 2,
            generation: AtomicU64::new(0),
        }
    }

    fn clear(&self) {
        for s in &self.slots {
            s.0.store(0, Relaxed);
            s.1.store(0, Relaxed);
        }
    }

    fn bucket(&self, h: u64) -> &[(AtomicU64, AtomicU64)] {
        let i = (h as usize) & self.mask;
        &self.slots[i..i + 2]
    }

    #[inline]
    fn prefetch(&self, h: u64) {
        crate::prefetch::l2(self.slots.as_ptr().wrapping_add((h as usize) & self.mask));
    }

    fn get(&self, h: u64) -> Option<Entry> {
        self.bucket(h).iter().find_map(|s| {
            let (k, d) = (s.0.load(Relaxed), s.1.load(Relaxed));
            (d != 0 && k ^ d == h).then(|| Entry::unpack(d))
        })
    }

    fn put(&self, h: u64, e: Entry) {
        let e = Entry {
            generation: self.generation.load(Relaxed) as u8,
            ..e
        };
        let write = |s: &(AtomicU64, AtomicU64)| {
            let p = e.pack();
            s.1.store(p, Relaxed);
            s.0.store(h ^ p, Relaxed);
        };
        let bucket = self.bucket(h);
        for s in bucket {
            let (k, d) = (s.0.load(Relaxed), s.1.load(Relaxed));
            if d != 0 && k ^ d == h {
                if e.beats(&Entry::unpack(d)) {
                    write(s);
                }
                return;
            }
        }
        for s in bucket {
            let d = s.1.load(Relaxed);
            let old = Entry::unpack(d);
            if d == 0 || old.generation != e.generation || e.beats(&old) {
                write(s);
                return;
            }
        }
    }
}

struct Shared {
    nn: Arc<Nnue>,
    table: Table,
    /// Without a measured sigma the model searches without ProbCut.
    sigma: Option<MpcSigma>,
    /// Set at the deadline or by `stop`: everything unwinds and the pass is dropped.
    timeout: AtomicBool,
    deadline_ns: AtomicU64,
    epoch: Instant,
    stop: Option<StopHandle>,
}

/// A stop that also trips when any ancestor's does.
struct CStop {
    flag: AtomicBool,
    parent: Option<Arc<CStop>>,
}

impl CStop {
    fn new(parent: Option<Arc<CStop>>) -> Arc<CStop> {
        Arc::new(CStop {
            flag: AtomicBool::new(false),
            parent,
        })
    }

    fn raise(&self) {
        self.flag.store(true, Relaxed);
    }

    fn stopped(&self) -> bool {
        let mut c = Some(self);
        while let Some(s) = c {
            if s.flag.load(Relaxed) {
                return true;
            }
            c = s.parent.as_deref();
        }
        false
    }

    fn descends_from(&self, a: &CStop) -> bool {
        let mut c = Some(self);
        while let Some(s) = c {
            if std::ptr::eq(s, a) {
                return true;
            }
            c = s.parent.as_deref();
        }
        false
    }
}

const KID_ABORTED: i32 = i32::MIN;

struct CKid {
    sq: u8,
    taken: AtomicBool,
    /// 0 open, 1 being written, 2 written.
    state: std::sync::atomic::AtomicU8,
    val: std::sync::atomic::AtomicI32,
    running: std::sync::atomic::AtomicU32,
    taken_over: AtomicBool,
    stop: Arc<CStop>,
}

/// One node's siblings after the first, open to every idle thread.
struct CFan {
    sh: *const Shared,
    board: Board,
    acc: PatternIndices,
    depth: i32,
    alpha: i32,
    /// On a PV node a fail high is re-searched, not a cut, so the others go on.
    pv: bool,
    margin: i32,
    history: [[i32; 64]; 2],
    kids_all: bool,
    kids: Vec<CKid>,
    left: std::sync::atomic::AtomicUsize,
    helpers: std::sync::atomic::AtomicUsize,
    nodes: AtomicU64,
    stop: Arc<CStop>,
}

// The owner does not leave the node before every search of a kid has returned, so `sh` outlives the fan's use.
unsafe impl Send for CFan {}
unsafe impl Sync for CFan {}

impl CFan {
    fn take(&self, k: usize) -> bool {
        if self.kids[k]
            .taken
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return false;
        }
        self.left.fetch_sub(1, Relaxed);
        true
    }

    fn open(&self) -> bool {
        self.left.load(Relaxed) > 0 && !self.stop.stopped()
    }

    fn settled(&self) -> bool {
        use std::sync::atomic::Ordering::Acquire;
        self.kids
            .iter()
            .all(|k| k.running.load(Acquire) == 0 || k.state.load(Acquire) == 2)
    }

    fn search_kid(&self, w: &mut Worker, k: usize) {
        use std::sync::atomic::Ordering::{AcqRel, Acquire, Release};
        let kid = &self.kids[k];
        kid.running.fetch_add(1, AcqRel);
        let pos = Position(kid.sq);
        let mut nb = self.board;
        let flipped = nb.make_move_bits(pos);
        let mut child = self.acc;
        w.sh.nn
            .ix_apply(&mut child, pos, flipped, self.board.player());
        let saved = w.stop.replace(kid.stop.clone());
        w.expect_all = self.kids_all;
        let r = w.search(&nb, &child, self.depth, -self.alpha - 1, -self.alpha, false);
        w.stop = saved;
        if kid.state.compare_exchange(0, 1, AcqRel, Acquire).is_ok() {
            let v = match r {
                Some((v, _)) => {
                    if -v > self.alpha && !self.pv {
                        self.stop.raise();
                    }
                    -v
                }
                None => KID_ABORTED,
            };
            kid.val.store(v, Relaxed);
            kid.state.store(2, Release);
            kid.stop.raise();
        }
        kid.running.fetch_sub(1, AcqRel);
    }

    fn help(&self, pool: &Arc<CPool>) -> bool {
        let sh = unsafe { &*self.sh };
        let mut w = Worker {
            sh,
            nodes: 0,
            history: self.history,
            margin: self.margin,
            root_mask: 0,
            root_depth: -1,
            stop: None,
            pool: Some(pool.clone()),
            expect_all: false,
        };
        let mut ran = false;
        for k in 0..self.kids.len() {
            if self.stop.stopped() {
                break;
            }
            if self.take(k) {
                ran = true;
                self.search_kid(&mut w, k);
            }
        }
        self.nodes.fetch_add(w.nodes, Relaxed);
        ran
    }

    fn take_over(&self, pool: &Arc<CPool>) -> bool {
        use std::sync::atomic::Ordering::{AcqRel, Acquire};
        for (k, kid) in self.kids.iter().enumerate() {
            if kid.running.load(Acquire) > 0
                && kid.state.load(Acquire) == 0
                && !kid.taken_over.swap(true, AcqRel)
            {
                let sh = unsafe { &*self.sh };
                let mut w = Worker {
                    sh,
                    nodes: 0,
                    history: self.history,
                    margin: self.margin,
                    root_mask: 0,
                    root_depth: -1,
                    stop: None,
                    pool: Some(pool.clone()),
                    expect_all: false,
                };
                self.search_kid(&mut w, k);
                self.nodes.fetch_add(w.nodes, Relaxed);
                return true;
            }
        }
        false
    }
}

struct CPool {
    fans: std::sync::RwLock<Vec<Arc<CFan>>>,
    n_fans: std::sync::atomic::AtomicUsize,
    m: std::sync::Mutex<()>,
    cv: std::sync::Condvar,
    idle: std::sync::atomic::AtomicUsize,
    shutdown: AtomicBool,
}

impl CPool {
    fn start(workers: usize) -> Arc<CPool> {
        let p = Arc::new(CPool {
            fans: std::sync::RwLock::new(Vec::new()),
            n_fans: std::sync::atomic::AtomicUsize::new(0),
            m: std::sync::Mutex::new(()),
            cv: std::sync::Condvar::new(),
            idle: std::sync::atomic::AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
        });
        for _ in 0..workers {
            let q = p.clone();
            std::thread::spawn(move || q.worker_loop());
        }
        p
    }

    fn open_fan(&self, f: &Arc<CFan>) {
        self.fans.write().unwrap().push(f.clone());
        self.n_fans.fetch_add(1, Relaxed);
        if self.idle.load(Relaxed) > 0 {
            let _g = self.m.lock().unwrap();
            self.cv.notify_all();
        }
    }

    fn fan_open(&self) -> bool {
        self.n_fans.load(Relaxed) > 0 && self.fans.read().unwrap().iter().any(|f| f.open())
    }

    fn help(self: &Arc<Self>, under: Option<&CStop>) -> bool {
        if self.n_fans.load(Relaxed) == 0 {
            return false;
        }
        let pick = {
            let fans = self.fans.read().unwrap();
            fans.iter()
                .filter(|f| f.open() && under.is_none_or(|a| f.stop.descends_from(a)))
                .max_by_key(|f| f.depth)
                .map(|f| {
                    f.helpers.fetch_add(1, Relaxed);
                    f.clone()
                })
        };
        let Some(f) = pick else {
            return false;
        };
        let ran = f.help(self);
        f.helpers.fetch_sub(1, Relaxed);
        ran
    }

    fn close_fan(self: &Arc<Self>, f: &Arc<CFan>) {
        use std::sync::atomic::Ordering::Acquire;
        for k in 0..f.kids.len() {
            f.take(k);
        }
        self.fans.write().unwrap().retain(|x| !Arc::ptr_eq(x, f));
        self.n_fans.fetch_sub(1, Relaxed);
        let mut idle = 0u32;
        while f.helpers.load(Acquire) > 0 && !f.settled() {
            if self.help(Some(&f.stop)) || f.take_over(self) {
                idle = 0;
                continue;
            }
            idle += 1;
            if idle < 64 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
        // A helper may still be between taking nothing and leaving.
        while f.helpers.load(Acquire) > 0 {
            std::hint::spin_loop();
        }
    }

    fn worker_loop(self: Arc<Self>) {
        loop {
            if self.shutdown.load(Relaxed) {
                return;
            }
            if self.fan_open() {
                self.help(None);
                continue;
            }
            self.idle.fetch_add(1, Relaxed);
            let g = self.m.lock().unwrap();
            if !self.fan_open() && !self.shutdown.load(Relaxed) {
                drop(self.cv.wait(g).unwrap());
            }
            self.idle.fetch_sub(1, Relaxed);
        }
    }

    fn stop_all(&self) {
        self.shutdown.store(true, Relaxed);
        let _g = self.m.lock().unwrap();
        self.cv.notify_all();
    }
}

struct Worker<'a> {
    sh: &'a Shared,
    /// The kid this worker searches for a fan, when it does.
    stop: Option<Arc<CStop>>,
    pool: Option<Arc<CPool>>,
    /// The next search's node is expected to fail low on every move; taken at entry.
    expect_all: bool,
    nodes: u64,
    history: [[i32; 64]; 2],
    margin: i32,
    /// Moves searched at the root (all when 0); a narrowed root is not stored, its bound is not the position's.
    root_mask: u64,
    root_depth: i32,
}

impl Worker<'_> {
    #[inline]
    fn aborted(&mut self) -> bool {
        if self.sh.timeout.load(Relaxed) {
            return true;
        }
        if self.stop.as_ref().is_some_and(|s| s.stopped()) {
            return true;
        }
        if self.nodes & 1023 == 0 {
            let late =
                self.sh.epoch.elapsed().as_nanos() as u64 > self.sh.deadline_ns.load(Relaxed);
            if late || self.sh.stop.as_ref().is_some_and(|s| s.is_stopped()) {
                self.sh.timeout.store(true, Relaxed);
                return true;
            }
        }
        false
    }

    fn eval(&self, b: &Board, acc: &PatternIndices) -> i32 {
        (self.sh.nn.eval_from_indices(acc, b).round() as i32).clamp(-64, 64)
    }

    fn exact(&mut self, p: u64, o: u64, mut alpha: i32, beta: i32, passed: bool) -> i32 {
        let moves = bitboard::mobility(p, o, !(p | o));
        if moves == 0 {
            if passed {
                return final_score_bb(p, o);
            }
            return -self.exact(o, p, -beta, -alpha, true);
        }
        let mut best = MIN;
        let mut m = moves;
        while m != 0 {
            let bit = m.isolate_lowest_one();
            m &= m - 1;
            let f = bitboard::flippable(p, o, bit);
            self.nodes += 1;
            let v = -self.exact(o ^ f, p | f | bit, -beta, -alpha, false);
            if v > best {
                best = v;
                if v > alpha {
                    alpha = v;
                    if alpha >= beta {
                        break;
                    }
                }
            }
        }
        best
    }

    fn sort(&self, b: &Board, list: &mut [u8]) {
        let side = b.player() as usize & 1;
        list.sort_by_key(|&m| -(self.history[side][m as usize] + STATIC_BONUS[m as usize]));
    }

    /// (score, best move), `None` once the search was stopped.
    fn search(
        &mut self,
        b: &Board,
        acc: &PatternIndices,
        depth: i32,
        mut alpha: i32,
        mut beta: i32,
        pv: bool,
    ) -> Option<(i32, u8)> {
        self.nodes += 1;
        let all_node = std::mem::take(&mut self.expect_all);
        if self.aborted() {
            return None;
        }
        let empties = b.empty_count() as i32;
        if 64 - empties > EXACT_DISCS {
            let v = self.exact(b.player_bb(), b.opponent_bb(), alpha, beta, false);
            return Some((v, NO_MOVE));
        }
        let mut moves = b.movable();
        if moves == 0 {
            let mut nb = *b;
            nb.pass();
            if nb.movable() == 0 {
                return Some((final_score_bb(b.player_bb(), b.opponent_bb()), NO_MOVE));
            }
            self.expect_all = !all_node;
            let (v, _) = self.search(&nb, acc, depth, -beta, -alpha, pv)?;
            return Some((-v, NO_MOVE));
        }
        if depth <= 0 {
            return Some((self.eval(b, acc), NO_MOVE));
        }

        // Each move takes a ply off the depth, so only the root is searched at the root depth.
        let narrowed = self.root_mask != 0 && depth == self.root_depth;
        if narrowed {
            moves &= self.root_mask;
        }
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        let mut tt_move = NO_MOVE;
        let mut tt_depth = -1;
        if let Some(e) = self.sh.table.get(h) {
            tt_move = e.mv;
            tt_depth = e.depth as i32;
            if tt_depth >= depth && e.margin as i32 >= self.margin {
                if e.upper < beta {
                    beta = e.upper;
                    if beta <= alpha {
                        return Some((beta, tt_move));
                    }
                }
                // A lower bound never raises alpha on the PV: the move must be searched to be played.
                if !pv && e.lower > alpha {
                    alpha = e.lower;
                    if alpha >= beta {
                        return Some((alpha, tt_move));
                    }
                }
            }
        }

        let probe = PROBE_DEPTH.get(depth as usize).filter(|&&d| d >= 0);
        if let (false, Some(sigma), Some(&d1)) = (pv, self.sh.sigma, probe) {
            let sigma = sigma.value(empties as u32, depth as u32, d1 as u32);
            let s = ((sigma * self.margin as f32) / 2.0).round().max(1.0) as i32;
            self.expect_all = all_node;
            let (v, mv) = self.search(b, acc, d1 as i32, alpha - s, beta + s, false)?;
            if v >= beta + s {
                return Some((beta, mv));
            }
            if v <= alpha - s {
                return Some((alpha, mv));
            }
            if mv < NO_MOVE && (tt_move >= NO_MOVE || d1 as i32 >= tt_depth) {
                tt_move = mv;
            }
        }

        let mut list = [0u8; 32];
        let mut n = 0usize;
        let mut m = moves;
        while m != 0 {
            list[n] = m.trailing_zeros() as u8;
            m &= m - 1;
            n += 1;
        }
        let mut sorted = n;
        if tt_move < NO_MOVE && moves >> tt_move & 1 == 1 {
            let i = list[..n].iter().position(|&x| x == tt_move).unwrap();
            list.swap(0, i);
            sorted = 1;
        } else if n > 2 {
            self.sort(b, &mut list[..n]);
        }

        let mover = b.player();
        let side = mover as usize & 1;
        let alpha0 = alpha;
        let mut best = MIN;
        let mut best_move = NO_MOVE;
        // The kids are leaves outside the exact zone: their values do not depend on the window.
        let leaf_kids = depth == 1 && 64 - (empties - 1) <= EXACT_DISCS;
        let split = self.pool.is_some() && depth >= SPLIT_MIN;
        let kid_all = !pv && !all_node;
        let mut j = 0usize;
        while j < n {
            if j == sorted && n > j + 2 {
                self.sort(b, &mut list[j..n]);
            }
            // After the first kid, or at once where every kid is expected to fail low.
            if split && n - j >= 2 && (j >= 1 || (all_node && !pv)) {
                let (rest, tried) = (&list[j..n], &list[..j]);
                return self.split(
                    b, acc, h, rest, tried, depth, alpha, alpha0, beta, pv, best, best_move,
                    kid_all,
                );
            }
            let sq = list[j];
            j += 1;
            // The next kid's table entry loads while this one is searched.
            if !leaf_kids && j < n {
                let mut nb = *b;
                nb.make_move_bits(Position(list[j]));
                self.sh
                    .table
                    .prefetch(zobrist::board_hash(nb.player_bb(), nb.opponent_bb()));
            }
            let v = if leaf_kids {
                self.child1(b, acc, sq, alpha, beta, j == 1)?
            } else {
                self.child(b, acc, sq, depth, alpha, beta, pv, j == 1, kid_all)?
            };
            if v > best {
                best = v;
                best_move = sq;
                if v > alpha {
                    alpha = v;
                    if alpha >= beta {
                        self.history[side][sq as usize] += depth;
                        for &k in &list[..j - 1] {
                            self.history[side][k as usize] -= depth;
                        }
                        self.store(h, alpha0, beta, best, best_move, depth);
                        return Some((best, best_move));
                    }
                }
            }
        }
        self.store(h, alpha0, beta, best, best_move, depth);
        Some((best, best_move))
    }

    /// The siblings after the first, searched by every idle thread at once; the node's result.
    #[allow(clippy::too_many_arguments)]
    fn split(
        &mut self,
        b: &Board,
        acc: &PatternIndices,
        h: u64,
        rest: &[u8],
        tried: &[u8],
        depth: i32,
        mut alpha: i32,
        alpha0: i32,
        beta: i32,
        pv: bool,
        mut best: i32,
        mut best_move: u8,
        kid_all: bool,
    ) -> Option<(i32, u8)> {
        use std::sync::atomic::Ordering::Acquire;
        let pool = self.pool.clone().unwrap();
        let fan_stop = CStop::new(self.stop.clone());
        let kids = rest
            .iter()
            .map(|&sq| CKid {
                sq,
                taken: AtomicBool::new(false),
                state: std::sync::atomic::AtomicU8::new(0),
                val: std::sync::atomic::AtomicI32::new(0),
                running: std::sync::atomic::AtomicU32::new(0),
                taken_over: AtomicBool::new(false),
                stop: CStop::new(Some(fan_stop.clone())),
            })
            .collect();
        let fan = Arc::new(CFan {
            sh: self.sh as *const Shared,
            board: *b,
            acc: *acc,
            depth: depth - 1,
            alpha,
            pv,
            margin: self.margin,
            history: self.history,
            kids_all: kid_all,
            kids,
            left: std::sync::atomic::AtomicUsize::new(rest.len()),
            helpers: std::sync::atomic::AtomicUsize::new(0),
            nodes: AtomicU64::new(0),
            stop: fan_stop,
        });
        pool.open_fan(&fan);
        for k in 0..rest.len() {
            if fan.stop.stopped() {
                break;
            }
            if fan.take(k) {
                fan.search_kid(self, k);
            }
        }
        pool.close_fan(&fan);
        self.nodes += fan.nodes.load(Relaxed);
        if self.sh.timeout.load(Relaxed) || self.stop.as_ref().is_some_and(|s| s.stopped()) {
            return None;
        }
        let side = b.player() as usize & 1;
        let mut high = [0u8; 32];
        let mut nh = 0usize;
        for (k, kid) in fan.kids.iter().enumerate() {
            if kid.state.load(Acquire) != 2 {
                continue;
            }
            let g = kid.val.load(Relaxed);
            if g == KID_ABORTED {
                continue;
            }
            if g > best {
                best = g;
                best_move = kid.sq;
            }
            if g > alpha {
                if !pv {
                    self.history[side][kid.sq as usize] += depth;
                    for &t in tried.iter().chain(&rest[..k]) {
                        self.history[side][t as usize] -= depth;
                    }
                    self.store(h, alpha0, beta, g, kid.sq, depth);
                    return Some((g, kid.sq));
                }
                high[nh] = k as u8;
                nh += 1;
            }
        }
        for &k in &high[..nh] {
            let sq = rest[k as usize];
            let v = self.child(b, acc, sq, depth, alpha, beta, pv, false, kid_all)?;
            if v > best {
                best = v;
                best_move = sq;
            }
            if v > alpha {
                alpha = v;
                if alpha >= beta {
                    self.history[side][sq as usize] += depth;
                    self.store(h, alpha0, beta, best, best_move, depth);
                    return Some((best, best_move));
                }
            }
        }
        self.store(h, alpha0, beta, best, best_move, depth);
        Some((best, best_move))
    }

    /// A leaf's value as `search` at depth 0 gives it, counting the same nodes.
    #[inline]
    fn leaf(&mut self, nb: &Board, acc: &PatternIndices) -> Option<i32> {
        self.nodes += 1;
        if self.aborted() {
            return None;
        }
        if nb.movable() != 0 {
            return Some(self.eval(nb, acc));
        }
        let mut p = *nb;
        p.pass();
        if p.movable() == 0 {
            return Some(final_score_bb(nb.player_bb(), nb.opponent_bb()));
        }
        self.nodes += 1;
        if self.aborted() {
            return None;
        }
        Some(-self.eval(&p, acc))
    }

    /// [`Worker::child`] for a leaf kid: the null-window search and its re-search give the same value.
    #[inline]
    fn child1(
        &mut self,
        b: &Board,
        acc: &PatternIndices,
        sq: u8,
        alpha: i32,
        beta: i32,
        first: bool,
    ) -> Option<i32> {
        let pos = Position(sq);
        let mut nb = *b;
        let flipped = nb.make_move_bits(pos);
        self.sh.nn.prefetch_eval(&nb);
        let mut child = *acc;
        self.sh.nn.ix_apply(&mut child, pos, flipped, b.player());
        let v = -self.leaf(&nb, &child)?;
        if !first && v > alpha && v < beta {
            self.leaf(&nb, &child)?;
        }
        Some(v)
    }

    /// The first move gets the full window; the rest a null window and a re-search on a fail high.
    #[allow(clippy::too_many_arguments)]
    fn child(
        &mut self,
        b: &Board,
        acc: &PatternIndices,
        sq: u8,
        depth: i32,
        alpha: i32,
        beta: i32,
        pv: bool,
        first: bool,
        kid_all: bool,
    ) -> Option<i32> {
        let pos = Position(sq);
        let mut nb = *b;
        let flipped = nb.make_move_bits(pos);
        let mut child = *acc;
        self.sh.nn.ix_apply(&mut child, pos, flipped, b.player());
        self.expect_all = kid_all;
        if first {
            return Some(-self.search(&nb, &child, depth - 1, -beta, -alpha, pv)?.0);
        }
        let mut v = -self
            .search(&nb, &child, depth - 1, -alpha - 1, -alpha, false)?
            .0;
        if v > alpha && v < beta {
            // A null-window fail high by one is exactly alpha + 1: re-search from alpha.
            if v == alpha + 1 {
                v = alpha;
            }
            v = -self.search(&nb, &child, depth - 1, -beta, -v, true)?.0;
        }
        Some(v)
    }

    fn store(&self, h: u64, alpha0: i32, beta: i32, score: i32, mv: u8, depth: i32) {
        if self.root_mask != 0 && depth == self.root_depth {
            return;
        }
        let (lower, upper) = if score >= beta {
            (score, 64)
        } else if score > alpha0 {
            (score, score)
        } else {
            (-64, score)
        };
        self.sh.table.put(
            h,
            Entry {
                lower: lower.clamp(-64, 64),
                upper: upper.clamp(-64, 64),
                mv,
                depth: depth.clamp(0, 255) as u8,
                margin: self.margin as u8,
                generation: 0,
            },
        );
    }
}

/// One completed pass, for progress display.
#[derive(Clone, Copy, Debug)]
pub struct Pass {
    pub depth: i32,
    /// ProbCut margin in half-sigmas.
    pub margin: i32,
    pub mv: Position,
    pub score: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Choice {
    pub mv: Option<Position>,
    pub score: i32,
    pub depth: i32,
    /// The ProbCut margin of the last completed pass, in half-sigmas.
    pub margin: i32,
    /// Only the exact solve is left; the caller runs it until `deadline`.
    pub solve: bool,
    pub deadline: Instant,
    pub nodes: u64,
}

pub struct ClockSearch {
    sh: Arc<Shared>,
    threads: usize,
    /// Threads beyond the first, waiting for fans to help with.
    pool: Option<Arc<CPool>>,
    history: [[i32; 64]; 2],
    /// Index into `MARGINS` (its length: the solve), carried over while searches reach the full depth.
    step: usize,
    first_move: bool,
}

impl Drop for ClockSearch {
    fn drop(&mut self) {
        if let Some(p) = self.pool.take() {
            p.stop_all();
        }
    }
}

impl ClockSearch {
    pub fn new(
        nn: Arc<Nnue>,
        table_bits: u32,
        threads: usize,
        stop: Option<StopHandle>,
    ) -> ClockSearch {
        let sigma = nn.mpc_sigma();
        ClockSearch {
            sh: Arc::new(Shared {
                nn,
                table: Table::new(table_bits),
                sigma,
                timeout: AtomicBool::new(false),
                deadline_ns: AtomicU64::new(u64::MAX),
                epoch: Instant::now(),
                stop,
            }),
            threads: threads.max(1),
            pool: (threads > 1).then(|| CPool::start(threads - 1)),
            history: [[0; 64]; 2],
            step: 0,
            first_move: true,
        }
    }

    pub fn set_threads(&mut self, threads: usize) {
        let threads = threads.max(1);
        if threads == self.threads {
            return;
        }
        if let Some(p) = self.pool.take() {
            p.stop_all();
        }
        self.threads = threads;
        self.pool = (threads > 1).then(|| CPool::start(threads - 1));
    }

    pub fn table_move(&self, b: &Board) -> Option<Position> {
        let e = self
            .sh
            .table
            .get(zobrist::board_hash(b.player_bb(), b.opponent_bb()))?;
        (e.mv < NO_MOVE).then_some(Position(e.mv))
    }

    /// Puts `pos` first in the ordering at `b` unless the table already holds a move there.
    pub fn seed_move(&self, b: &Board, pos: Position) {
        let h = zobrist::board_hash(b.player_bb(), b.opponent_bb());
        if self.sh.table.get(h).is_none() {
            self.sh.table.put(
                h,
                Entry {
                    lower: -64,
                    upper: 64,
                    mv: pos.index(),
                    depth: 0,
                    margin: 0,
                    generation: 0,
                },
            );
        }
    }

    pub fn new_game(&mut self) {
        self.sh.table.clear();
        self.history = [[0; 64]; 2];
        self.step = 0;
        self.first_move = true;
    }

    /// One pass at `depth` over [alpha, beta] on all threads; `None` if it was stopped.
    /// `root_mask` narrows the root moves (0: all of them).
    fn pass(
        &mut self,
        b: &Board,
        depth: i32,
        alpha: i32,
        beta: i32,
        root_mask: u64,
    ) -> (Option<(i32, u8)>, u64) {
        let sh = &*self.sh;
        let acc = sh.nn.indices(b.black, b.white);
        let mut w = Worker {
            sh,
            nodes: 0,
            history: self.history,
            margin: MARGINS[self.step],
            root_mask,
            root_depth: depth,
            stop: None,
            pool: self.pool.clone(),
            expect_all: false,
        };
        let r = w.search(b, &acc, depth, alpha, beta, true);
        let (nodes, history) = (w.nodes, w.history);
        self.history = history;
        (r, nodes)
    }

    /// `clock` is the time left for the whole game; `on_pass` sees every completed pass.
    pub fn choose(&mut self, b: &Board, clock: Duration, mut on_pass: impl FnMut(Pass)) -> Choice {
        let t0 = Instant::now();
        let moves = b.movable();
        let discs = 64 - b.empty_count() as i32;
        let remaining = clock.saturating_sub(CLOCK_MARGIN).max(MIN_REMAINING);
        let extra = if self.first_move && discs < FIRST_MOVE_EXTRA_DISCS {
            FIRST_MOVE_EXTRA
        } else {
            1.0
        };
        self.first_move = false;
        let moves_left = (PACE_DISCS - discs as f64).max(MOVES_LEFT_FLOOR);
        let mut budget =
            (BUDGET_SHARE * remaining.as_secs_f64() / moves_left).max(MIN_BUDGET_SECS) * extra;
        let mut deadline = t0 + Duration::from_secs_f64(budget);
        let mut out = Choice {
            mv: None,
            score: 0,
            depth: 0,
            margin: 0,
            solve: false,
            deadline,
            nodes: 0,
        };
        // No move is a pass (mv None); anything else falls back to the 64th square, which does not exist.
        if moves == 0 {
            return out;
        }
        // A forced move still needs its value (it is reported and graphed), so it gets one pass.
        let forced = moves.count_ones() == 1;
        let sh = self.sh.clone();
        sh.table.generation.fetch_add(1, Relaxed);
        for v in self.history.iter_mut().flatten() {
            *v /= HISTORY_DECAY;
        }
        sh.timeout.store(false, Relaxed);
        sh.deadline_ns
            .store((deadline - sh.epoch).as_nanos() as u64, Relaxed);

        let full = 64 - discs;
        let mut depth = (6 + discs % 2).min(full);
        if depth < full {
            self.step = 0;
        }
        let (mut alpha, mut beta) = (MIN, MAX);
        let mut best = NO_MOVE;
        let mut stable = 0u32;
        let mut hard_checked = false;
        loop {
            if self.step == MARGINS.len() {
                out.solve = true;
                break;
            }
            let mut fail_low = false;
            let mut result = None;
            loop {
                let (r, nodes) = self.pass(b, depth, alpha, beta, 0);
                out.nodes += nodes;
                let Some((s, mv)) = r else { break };
                result = Some((s, mv));
                if s > alpha && s < beta {
                    fail_low = false;
                    break;
                }
                fail_low = s <= alpha;
                alpha = alpha.min(s - 2);
                beta = beta.max(s + 2);
                if s <= MIN + 1 || s >= MAX - 1 {
                    alpha = MIN;
                    beta = MAX;
                }
            }
            let timeout = sh.timeout.load(Relaxed);
            let mut changed = false;
            if let Some((s, mv)) = result {
                if !fail_low && !timeout && mv < NO_MOVE && moves >> mv & 1 == 1 {
                    changed = best < NO_MOVE && best != mv;
                    stable = if best == mv { stable + 1 } else { 0 };
                    best = mv;
                    out.score = s;
                    out.depth = depth;
                    out.margin = MARGINS[self.step];
                    on_pass(Pass {
                        depth,
                        margin: out.margin,
                        mv: Position(mv),
                        score: s,
                    });
                }
                alpha = s - 2;
                beta = s + 2;
            }
            let elapsed = t0.elapsed().as_secs_f64();
            if timeout || elapsed >= budget || (forced && best < NO_MOVE) {
                break;
            }
            if !fail_low && depth >= SEP_MIN_DEPTH && depth < full && moves.count_ones() > 1 {
                let others = moves & !(1u64 << best);
                if stable >= SEP_STABLE_PASSES {
                    let a = out.score - SEP_THRESHOLD;
                    let (r, nodes) = self.pass(b, depth, a, a + 1, others);
                    out.nodes += nodes;
                    if r.is_some_and(|(v, _)| v <= a) {
                        break;
                    }
                }
                if !hard_checked && full >= EXT_MIN_EMPTIES && !sh.timeout.load(Relaxed) {
                    hard_checked = true;
                    let a = out.score - EXT_CLOSE - 1;
                    let (r, nodes) = self.pass(b, depth, a, a + 1, others);
                    out.nodes += nodes;
                    if r.is_some_and(|(v, _)| v > a) {
                        let cap = remaining.as_secs_f64() * EXT_MAX_SHARE_OF_CLOCK;
                        let extra = EXT_SHARE * remaining.as_secs_f64() / moves_left;
                        budget = (budget + extra).min(cap).max(budget);
                        deadline = t0 + Duration::from_secs_f64(budget);
                        out.deadline = deadline;
                        sh.deadline_ns
                            .store((deadline - sh.epoch).as_nanos() as u64, Relaxed);
                    }
                }
                if sh.timeout.load(Relaxed) {
                    break;
                }
            }
            if !changed && !fail_low && t0.elapsed().as_secs_f64() > budget * SETTLED_SHARE {
                break;
            }
            if depth + 2 < full - 5 {
                depth += 2;
            } else {
                if depth >= full {
                    self.step = (self.step + 1).min(MARGINS.len());
                }
                depth = full;
            }
        }
        if best == NO_MOVE {
            best = moves.trailing_zeros() as u8;
        }
        out.mv = Some(Position(best));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::NNUE_PATTERNS;

    fn search() -> ClockSearch {
        let mut nn = Nnue::new(NNUE_PATTERNS);
        nn.quantize();
        ClockSearch::new(Arc::new(nn), 16, 1, None)
    }

    #[test]
    fn plays_a_legal_move_within_the_budget() {
        let mut s = search();
        let b = Board::new();
        let t0 = Instant::now();
        let c = s.choose(&b, Duration::from_secs(3), |_| {});
        let mv = c.mv.expect("no move");
        assert_eq!(b.movable() >> mv.index() & 1, 1);
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_forced_move_is_played_with_its_value() {
        let b = Board::from_string(
            "XOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOOO- X",
        )
        .unwrap();
        let mut after = b;
        after.make_move(Position(63)).unwrap();
        let mine = after.opponent_bb().count_ones() as i32;
        let theirs = after.player_bb().count_ones() as i32;
        let mut s = search();
        let t0 = Instant::now();
        let c = s.choose(&b, Duration::from_secs(60), |_| {});
        assert_eq!(c.mv.map(|p| p.index()), Some(63));
        // Either scored here or handed to the exact solve, never a bare 0.
        assert!(
            c.solve || (c.depth > 0 && c.score == mine - theirs),
            "{c:?}"
        );
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn no_legal_move_is_a_pass() {
        // O to move with the last empty square, which only X can take.
        let b = Board::from_string(
            "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXO- O",
        )
        .unwrap();
        assert_eq!(b.movable(), 0);
        let mut s = search();
        let c = s.choose(&b, Duration::from_secs(60), |_| {});
        assert_eq!(c.mv, None);
    }

    #[test]
    fn the_last_empties_are_scored_exactly() {
        let b = Board::from_string(
            "OOOOOOOOXOOXXX--XXOOXOO-XOXOOO--XOOOOX--XOOXOO--XOOOOO--XXXX---- O",
        )
        .unwrap();
        let s = search();
        let full = b.empty_count() as i32;
        let acc = s.sh.nn.indices(b.black, b.white);
        let mut w = Worker {
            sh: &s.sh,
            nodes: 0,
            history: [[0; 64]; 2],
            margin: 0,
            root_mask: 0,
            root_depth: 0,
            stop: None,
            pool: None,
            expect_all: false,
        };
        let (v, _) = w.search(&b, &acc, full, MIN, MAX, true).unwrap();
        let exact = w.exact(b.player_bb(), b.opponent_bb(), MIN, MAX, false);
        assert_eq!(v, exact);
    }
}
