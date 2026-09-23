//! Endgame solver: PVS (principal variation search) with a Zobrist-keyed transposition table, move ordering, and specialized last-4/3/2/1 fast paths.

#![allow(clippy::too_many_arguments)]

use crate::bitboard;
use crate::board::Board;
use crate::linear::Linear;
use crate::pattern_index::{PatternIndexer, PatternIndices};
use crate::position::Position;
use crate::zobrist;

const PVS_LIMIT: u8 = 12;

#[cfg(feature = "tunable")]
fn pvs_limit() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("PVS_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(PVS_LIMIT)
    })
}
#[cfg(not(feature = "tunable"))]
const fn pvs_limit() -> u8 {
    PVS_LIMIT
}
const MOVE_ORDERING_LIMIT: u8 = 7;
const TT_MIN_EMPTIES_DEFAULT: u8 = 11;

#[cfg(feature = "tunable")]
fn tt_min_empties() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("TT_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(TT_MIN_EMPTIES_DEFAULT)
    })
}
#[cfg(not(feature = "tunable"))]
const fn tt_min_empties() -> u8 {
    TT_MIN_EMPTIES_DEFAULT
}

#[cfg(feature = "tunable")]
fn etc_empties() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("ETC_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(ETC_EMPTIES)
    })
}
#[cfg(not(feature = "tunable"))]
const fn etc_empties() -> u8 {
    ETC_EMPTIES
}

const MID_TT_EMPTIES: u8 = 10;
const MID_TT_BITS: u32 = 19;
const SHALLOW_TT_BITS: u32 = 13;

#[inline(always)]
#[cfg(feature = "tunable")]
fn mid_tt_empties() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("TT_MID_EMPTIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MID_TT_EMPTIES)
    })
}
#[cfg(not(feature = "tunable"))]
fn mid_tt_empties() -> u8 {
    MID_TT_EMPTIES
}

fn private_tt_bits() -> (u32, u32) {
    static V: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        let read = |k: &str, d: u32| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        (
            read("TT_SHALLOW_BITS", SHALLOW_TT_BITS),
            read("TT_MID_BITS", MID_TT_BITS),
        )
    })
}
pub static ORDER_MISMATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static ORDER_MISMATCH_UNSEEDED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub static ORDER_MISMATCH_ZERO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const EVAL_ORDER_EMPTIES: u8 = 14;

#[cfg(feature = "tunable")]
fn eval_order_empties() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EVAL_ORDER_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(EVAL_ORDER_EMPTIES)
    })
}
#[cfg(not(feature = "tunable"))]
const fn eval_order_empties() -> u8 {
    EVAL_ORDER_EMPTIES
}
const DEEP_ORDER_EMPTIES: u8 = 17;
#[inline]
pub fn final_score(board: &Board) -> i32 {
    let diff = board.score();
    let empties = board.empty_count() as i32;
    match diff.cmp(&0) {
        std::cmp::Ordering::Greater => diff + empties,
        std::cmp::Ordering::Less => diff - empties,
        std::cmp::Ordering::Equal => 0,
    }
}

#[inline]
fn wipeout_score(board: &Board) -> Option<i32> {
    wipeout_score_bb(board.player_bb(), board.opponent_bb())
}

/// Narrows `alpha..beta` to a probed bound; `Some` is an immediate return.
#[inline(always)]
fn narrow(alpha: &mut i32, beta: &mut i32, narrowed: &mut bool, lo: i32, hi: i32) -> Option<i32> {
    if lo >= hi {
        return Some(lo);
    }
    if *beta > hi {
        *beta = hi;
        *narrowed = true;
        if *beta <= *alpha {
            return Some(*beta);
        }
    }
    if *alpha < lo {
        *alpha = lo;
        *narrowed = true;
        if *alpha >= *beta {
            return Some(*alpha);
        }
    }
    None
}

#[inline]
fn child_bb(player: u64, opponent: u64, m: ScoredMove) -> (u64, u64) {
    (opponent ^ m.flipped, player | m.flipped | m.pos.to_bit())
}

#[inline]
fn board_of(player: u64, opponent: u64, to_move: crate::color::Color) -> Board {
    let (black, white) = if to_move == crate::color::Color::Black {
        (player, opponent)
    } else {
        (opponent, player)
    };
    Board {
        black,
        white,
        player: to_move,
        empty_count: bitboard::empty_bb(player, opponent).count_ones() as u8,
    }
}

#[inline]
fn wipeout_score_bb(player: u64, opponent: u64) -> Option<i32> {
    if opponent == 0 {
        return Some(64);
    }
    if player == 0 {
        return Some(-64);
    }
    None
}

const STABILITY_THRESHOLD: [i32; 64] = [
    99, 99, 99, 99, 6, 8, 10, 12, //
    14, 16, 20, 22, 24, 26, 28, 30, //
    32, 34, 36, 38, 40, 42, 44, 46, //
    48, 48, 50, 50, 52, 52, 54, 54, //
    56, 56, 58, 58, 60, 60, 62, 62, //
    64, 64, 64, 64, 64, 64, 64, 64, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99,
];

const STABILITY_CUT_MARGIN: i32 = 8;

const STAB_MIN_EMPTIES: u8 = 4;

#[cfg(feature = "tunable")]
fn stab_min_empties() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EXACT_STAB_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(STAB_MIN_EMPTIES)
    })
}
#[cfg(not(feature = "tunable"))]
fn stab_min_empties() -> u8 {
    STAB_MIN_EMPTIES
}

#[inline]
fn stability_cut(board: &Board, alpha: i32, beta: i32) -> Option<i32> {
    stability_cut_bb(
        board.player_bb(),
        board.opponent_bb(),
        board.empty_count(),
        alpha,
        beta,
    )
}

#[inline(always)]
fn stability_cut_bb(player: u64, opponent: u64, empties: u8, alpha: i32, beta: i32) -> Option<i32> {
    if empties < stab_min_empties() {
        return None;
    }
    #[cfg(feature = "layer-profile")]
    ab_stats::STAB_GATE_A.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if alpha >= STABILITY_THRESHOLD[(empties & 63) as usize] {
        let need = (64 - alpha + 1) / 2;
        if need <= 32 && (opponent.count_ones() as i32) >= need + STABILITY_CUT_MARGIN {
            #[cfg(feature = "layer-profile")]
            ab_stats::STAB_FULL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(bound) = stability_sweep_low(player, opponent, need, alpha) {
                return Some(bound);
            }
        }
    }
    stability_cut_high(player, opponent, empties, beta)
}

#[inline(never)]
fn stability_sweep_low(player: u64, opponent: u64, need: i32, alpha: i32) -> Option<i32> {
    let bound =
        64 - 2 * crate::stability::stable_count_at_least(opponent, player, need as u32) as i32;
    (bound <= alpha).then_some(bound)
}

#[inline(always)]
fn stability_cut_high(player: u64, opponent: u64, empties: u8, beta: i32) -> Option<i32> {
    {
        let _ = (player, opponent, empties, beta);
        #[cfg(feature = "layer-profile")]
        ab_stats::STAB_GATE_B.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        None
    }
}

const DEEP2_ORDER_EMPTIES: u8 = 20;
const DEEP3_ORDER_EMPTIES: u8 = 29;
const ETC_EMPTIES: u8 = 13;
const MOBILITY_ORDER_WEIGHT: i32 = 12;
const PARALLEL_MIN_EMPTIES: u8 = 16;

#[cfg(feature = "tunable")]
fn parallel_min_empties() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("PAR_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(PARALLEL_MIN_EMPTIES)
    })
}
#[cfg(not(feature = "tunable"))]
fn parallel_min_empties() -> u8 {
    PARALLEL_MIN_EMPTIES
}

pub static SPLITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static HANDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static WARMUP_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static EXACT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static WARMUP_NODES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static EXACT_NODES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static CLEAR_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub static ABORT_FIRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(feature = "tunable")]
fn solver_abort() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SOLVER_ABORT").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn solver_abort() -> bool {
    true
}

#[cfg(feature = "tunable")]
fn chaos_every() -> u64 {
    static V: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("SOLVER_CHAOS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    })
}
#[cfg(not(feature = "tunable"))]
fn chaos_every() -> u64 {
    0
}

pub static TASK_ABORTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static TASK_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static WAIT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct AbortFlag<'p> {
    flag: std::sync::atomic::AtomicBool,
    parent: Option<&'p AbortFlag<'p>>,
}

impl<'p> AbortFlag<'p> {
    const fn root() -> AbortFlag<'static> {
        AbortFlag {
            flag: std::sync::atomic::AtomicBool::new(false),
            parent: None,
        }
    }

    fn child(parent: &'p AbortFlag<'p>) -> AbortFlag<'p> {
        AbortFlag {
            flag: std::sync::atomic::AtomicBool::new(false),
            parent: Some(parent),
        }
    }

    fn abort(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn aborted(&self) -> bool {
        let mut cur = Some(self);
        while let Some(f) = cur {
            if f.flag.load(std::sync::atomic::Ordering::Relaxed) {
                return true;
            }
            cur = f.parent;
        }
        false
    }
}

const ABORTED: i32 = i32::MIN + 1;

struct ThreadBudget {
    pool: EndPool,
    scratch: std::sync::Mutex<Vec<Scratch>>,
}

struct Scratch {
    shallow: HashTable,
    mid: HashTable,
    l4: Vec<L4Entry>,
    l56: Vec<L4Entry>,
    l78: Vec<L4Entry>,
    tag: u32,
}

impl ThreadBudget {
    fn new(extra_threads: usize) -> ThreadBudget {
        ThreadBudget {
            pool: EndPool::new(extra_threads),
            scratch: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn take_scratch(&self, tag: u32) -> Scratch {
        let popped = self.scratch.lock().unwrap().pop();
        match popped {
            Some(mut s) => {
                if s.tag != tag {
                    s.shallow.clear(1);
                    s.mid.clear(1);
                    s.l4.fill(L4_EMPTY);
                    s.l56.fill(L4_EMPTY);
                    s.l78.fill(L4_EMPTY);
                    s.tag = tag;
                }
                s
            }
            None => {
                let (sb, mb) = private_tt_bits();
                Scratch {
                    shallow: HashTable::new(sb),
                    mid: HashTable::new(mb),
                    l4: Vec::new(),
                    l56: Vec::new(),
                    l78: Vec::new(),
                    tag,
                }
            }
        }
    }

    fn give_scratch(&self, s: Scratch) {
        self.scratch.lock().unwrap().push(s);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_one_sibling(
    tt: &HashTable,
    budget: &ThreadBudget,
    group: &AbortFlag<'_>,
    selective_t: Option<f32>,
    nnue: Option<NnueProbe>,
    sigma_scale: f32,
    parent: &Board,
    m: ScoredMove,
    upper: i32,
    shared_lower: &std::sync::atomic::AtomicI32,
    slot: &TaskSlot,
    ev: Option<&Linear>,
) {
    use std::sync::atomic::Ordering;
    let t_live = std::time::Instant::now();
    let cur = shared_lower.load(Ordering::Relaxed);
    if group.aborted() || cur >= upper {
        slot.finish(ABORTED, 0, false);
        return;
    }
    let tag = selective_t.map_or(0, f32::to_bits);
    let mut s = budget.take_scratch(tag);
    let mut w = Worker::with_tables(tt, budget, group, s.shallow, s.mid);
    if !s.l4.is_empty() {
        w.l4 = std::mem::take(&mut s.l4);
    }
    if !s.l56.is_empty() {
        w.l56 = std::mem::take(&mut s.l56);
    }
    if !s.l78.is_empty() {
        w.l78 = std::mem::take(&mut s.l78);
    }
    w.selective_t = selective_t;
    w.nnue = nnue;
    w.sigma_scale = sigma_scale;

    let mut child = m.child(parent);
    if let Some(e) = ev.filter(|_| child.empty_count() >= eval_order_empties()) {
        w.order_ix = order_indexer(e).init(child.black, child.white);
        w.order_seeded = true;
    }
    let ch = child_hash_of(&child);
    let mut val = -w.pvs(&mut child, ch, -cur - 1, -cur, false, true, ev);
    if val == -ABORTED {
        val = ABORTED;
    } else if cur < val && val < upper {
        let ch = child_hash_of(&child);
        let re = -w.pvs(&mut child, ch, -upper, -val, false, false, ev);
        val = if re == -ABORTED { ABORTED } else { re };
    }
    if val != ABORTED && val > cur {
        shared_lower.fetch_max(val, Ordering::Relaxed);
        if val >= upper {
            ABORT_FIRED.fetch_add(1, Ordering::Relaxed);
            if solver_abort() {
                group.abort();
            }
        }
    }
    if chaos_every() > 0
        && TASK_ABORTED
            .fetch_add(0, Ordering::Relaxed)
            .is_multiple_of(7)
    {
        let n = ABORT_FIRED.fetch_add(1, Ordering::Relaxed);
        if n.is_multiple_of(chaos_every()) {
            group.abort();
        }
    }

    let nodes = w.nodes;
    budget.give_scratch(Scratch {
        shallow: w.shallow_table,
        mid: w.mid_table,
        l4: w.l4,
        l56: w.l56,
        l78: w.l78,
        tag,
    });
    TASK_NS.fetch_add(t_live.elapsed().as_nanos() as u64, Ordering::Relaxed);
    slot.finish(val, nodes, val != ABORTED && val > cur);
}

struct SplitPoint {
    board: Board,
    moves: *const ScoredMove,
    n_moves: usize,
    upper: i32,
    selective_t: Option<f32>,
    nnue: Option<NnueProbe>,
    sigma_scale: f32,
    tt: *const HashTable,
    ev: *const (),
    group: *const (),
    budget: *const (),
    cursor: std::sync::atomic::AtomicUsize,
    shared_lower: std::sync::atomic::AtomicI32,
    active: std::sync::atomic::AtomicUsize,
    merge: std::sync::Mutex<SplitMerge>,
    nodes: std::sync::atomic::AtomicU64,
    waiter: std::thread::Thread,
}

unsafe impl Send for SplitPoint {}
unsafe impl Sync for SplitPoint {}

#[derive(Clone, Copy)]
struct SplitMerge {
    max: i32,
    best_val: i32,
    best: Option<Position>,
    aborted: bool,
}

#[cfg(feature = "tunable")]
fn split_v2() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SPLIT_V2").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn split_v2() -> bool {
    true
}

fn split_merge(sp: &SplitPoint, val: i32, cur: i32, pos: Position) {
    let mut m = sp.merge.lock().unwrap();
    if val == ABORTED {
        m.aborted = true;
        TASK_ABORTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    if val > m.max {
        m.max = val;
    }
    if val > cur && val > m.best_val {
        m.best_val = val;
        m.best = Some(pos);
    }
}

unsafe fn help_split(sp: &SplitPoint) -> bool {
    use std::sync::atomic::Ordering;
    let t_live = std::time::Instant::now();
    let (tt, group, budget) = unsafe {
        (
            &*sp.tt,
            &*(sp.group as *const AbortFlag<'static>),
            &*(sp.budget as *const ThreadBudget),
        )
    };
    let ev: Option<&Linear> = if sp.ev.is_null() {
        None
    } else {
        Some(unsafe { &*(sp.ev as *const Linear) })
    };
    let moves = unsafe { std::slice::from_raw_parts(sp.moves, sp.n_moves) };
    let tag = sp.selective_t.map_or(0, f32::to_bits);
    let mut scratch: Option<Scratch> = None;
    let mut nodes = 0u64;
    loop {
        if group.aborted() {
            break;
        }
        let cur = sp.shared_lower.load(Ordering::Relaxed);
        if cur >= sp.upper {
            break;
        }
        let idx = sp.cursor.fetch_add(1, Ordering::Relaxed);
        if idx >= sp.n_moves {
            break;
        }
        HANDED.fetch_add(1, Ordering::Relaxed);
        let m = moves[idx];
        let mut sc = scratch.take().unwrap_or_else(|| budget.take_scratch(tag));
        let mut w = Worker::with_tables(tt, budget, group, sc.shallow, sc.mid);
        if !sc.l4.is_empty() {
            w.l4 = std::mem::take(&mut sc.l4);
        }
        if !sc.l56.is_empty() {
            w.l56 = std::mem::take(&mut sc.l56);
        }
        if !sc.l78.is_empty() {
            w.l78 = std::mem::take(&mut sc.l78);
        }
        w.selective_t = sp.selective_t;
        w.nnue = sp.nnue.clone();
        w.sigma_scale = sp.sigma_scale;
        let mut child = m.child(&sp.board);
        if let Some(e) = ev.filter(|_| child.empty_count() >= eval_order_empties()) {
            w.order_ix = order_indexer(e).init(child.black, child.white);
            w.order_seeded = true;
        }
        let ch = child_hash_of(&child);
        let mut val = -w.pvs(&mut child, ch, -cur - 1, -cur, false, true, ev);
        if val == -ABORTED {
            val = ABORTED;
        } else if cur < val && val < sp.upper {
            let ch = child_hash_of(&child);
            let re = -w.pvs(&mut child, ch, -sp.upper, -val, false, false, ev);
            val = if re == -ABORTED { ABORTED } else { re };
        }
        if val != ABORTED && val > cur {
            sp.shared_lower.fetch_max(val, Ordering::Relaxed);
            if val >= sp.upper {
                ABORT_FIRED.fetch_add(1, Ordering::Relaxed);
                if solver_abort() {
                    group.abort();
                }
            }
        }
        if chaos_every() > 0
            && TASK_ABORTED
                .fetch_add(0, Ordering::Relaxed)
                .is_multiple_of(7)
        {
            let n = ABORT_FIRED.fetch_add(1, Ordering::Relaxed);
            if n.is_multiple_of(chaos_every()) {
                group.abort();
            }
        }
        nodes += w.nodes;
        scratch = Some(Scratch {
            shallow: w.shallow_table,
            mid: w.mid_table,
            l4: w.l4,
            l56: w.l56,
            l78: w.l78,
            tag,
        });
        split_merge(sp, val, cur, m.pos);
    }
    if let Some(sc) = scratch {
        budget.give_scratch(sc);
        sp.nodes.fetch_add(nodes, Ordering::Relaxed);
        TASK_NS.fetch_add(t_live.elapsed().as_nanos() as u64, Ordering::Relaxed);
        return true;
    }
    false
}

struct TaskSlot {
    done: std::sync::atomic::AtomicBool,
    beat: std::sync::atomic::AtomicBool,
    value: std::sync::atomic::AtomicI32,
    nodes: std::sync::atomic::AtomicU64,
    waiter: std::thread::Thread,
}

impl TaskSlot {
    fn new(waiter: std::thread::Thread) -> TaskSlot {
        TaskSlot {
            done: std::sync::atomic::AtomicBool::new(false),
            beat: std::sync::atomic::AtomicBool::new(false),
            value: std::sync::atomic::AtomicI32::new(ABORTED),
            nodes: std::sync::atomic::AtomicU64::new(0),
            waiter,
        }
    }

    fn finish(&self, value: i32, nodes: u64, beat: bool) {
        use std::sync::atomic::Ordering;
        self.beat.store(beat, Ordering::Relaxed);
        self.value.store(value, Ordering::Relaxed);
        self.nodes.store(nodes, Ordering::Relaxed);
        let waiter = self.waiter.clone();
        self.done.store(true, Ordering::Release);
        waiter.unpark();
    }

    fn result(&self) -> (i32, u64) {
        use std::sync::atomic::Ordering;
        (
            self.value.load(Ordering::Relaxed),
            self.nodes.load(Ordering::Relaxed),
        )
    }
}

const SPLIT_SLOTS: usize = 32;

const SPEC_SPLIT_MAX_EMPTIES: u8 = 24;

#[cfg(feature = "tunable")]
fn spec_split_max() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("SPEC_MAX")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(SPEC_SPLIT_MAX_EMPTIES)
    })
}
#[cfg(not(feature = "tunable"))]
fn spec_split_max() -> u8 {
    SPEC_SPLIT_MAX_EMPTIES
}

#[cfg(feature = "tunable")]
fn join_deepest() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("JOIN_DEEP").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn join_deepest() -> bool {
    true
}

#[cfg(feature = "tunable")]
fn join_deepest_big() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("JOIN_DEEP").map_or(true, |v| v != "1"))
}
#[cfg(not(feature = "tunable"))]
fn join_deepest_big() -> bool {
    true
}

#[cfg(feature = "tunable")]
fn max_join() -> usize {
    static V: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("MAX_JOIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    })
}
#[cfg(not(feature = "tunable"))]
fn max_join() -> usize {
    0
}

#[cfg(feature = "tunable")]
fn spec_split() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SPEC_SPLIT").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn spec_split() -> bool {
    true
}

struct EndPool {
    q: std::sync::Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send + 'static>>>,
    cv: std::sync::Condvar,
    split_ptr: [std::sync::atomic::AtomicPtr<SplitPoint>; SPLIT_SLOTS],
    split_guard: [std::sync::atomic::AtomicUsize; SPLIT_SLOTS],
    idle: std::sync::atomic::AtomicUsize,
    queued: std::sync::atomic::AtomicUsize,
    workers: usize,
    stop: std::sync::atomic::AtomicBool,
}

impl EndPool {
    fn new(workers: usize) -> EndPool {
        EndPool {
            q: std::sync::Mutex::new(std::collections::VecDeque::new()),
            cv: std::sync::Condvar::new(),
            split_ptr: [const { std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()) };
                SPLIT_SLOTS],
            split_guard: [const { std::sync::atomic::AtomicUsize::new(0) }; SPLIT_SLOTS],
            idle: std::sync::atomic::AtomicUsize::new(0),
            queued: std::sync::atomic::AtomicUsize::new(0),
            workers,
            stop: std::sync::atomic::AtomicBool::new(false),
        }
    }

    unsafe fn try_push<'t>(&self, f: impl FnOnce() + Send + 't) -> bool {
        use std::sync::atomic::Ordering;
        if self.workers == 0 {
            return false;
        }
        let free = |queued: usize| self.idle.load(Ordering::Relaxed) > queued;
        if !free(self.queued.load(Ordering::Relaxed)) {
            return false;
        }
        let boxed: Box<dyn FnOnce() + Send + 't> = Box::new(f);
        let boxed: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(boxed) };
        let mut q = self.q.lock().unwrap();
        if !free(q.len()) {
            return false;
        }
        q.push_back(boxed);
        self.queued.store(q.len(), Ordering::Relaxed);
        drop(q);
        self.cv.notify_one();
        true
    }

    fn register_split(&self, sp: &SplitPoint) -> Option<usize> {
        use std::sync::atomic::Ordering;
        let p = sp as *const SplitPoint as *mut SplitPoint;
        for i in 0..SPLIT_SLOTS {
            if self.split_ptr[i]
                .compare_exchange(std::ptr::null_mut(), p, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                drop(self.q.lock().unwrap());
                self.cv.notify_all();
                return Some(i);
            }
        }
        None
    }

    fn unregister_split(&self, i: usize, sp: &SplitPoint) {
        use std::sync::atomic::Ordering;
        let p = sp as *const SplitPoint as *mut SplitPoint;
        let _ = self.split_ptr[i].compare_exchange(
            p,
            std::ptr::null_mut(),
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
        while self.split_guard[i].load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
    }

    fn try_help_splits(&self) -> bool {
        use std::sync::atomic::Ordering;
        let mut order: [(u8, u8); SPLIT_SLOTS] = [(u8::MAX, 0); SPLIT_SLOTS];
        if join_deepest() {
            for (i, o) in order.iter_mut().enumerate() {
                self.split_guard[i].fetch_add(1, Ordering::AcqRel);
                let p = self.split_ptr[i].load(Ordering::Acquire);
                if !p.is_null() {
                    let e = unsafe { (*p).board.empty_count() };
                    let key = if join_deepest_big() { 64 - e } else { e };
                    *o = (key, i as u8);
                }
                self.split_guard[i].fetch_sub(1, Ordering::Release);
            }
            order.sort_unstable();
        } else {
            for (i, o) in order.iter_mut().enumerate() {
                *o = (0, i as u8);
            }
        }
        for &(_, slot) in order.iter() {
            let i = slot as usize;
            self.split_guard[i].fetch_add(1, Ordering::AcqRel);
            let p = self.split_ptr[i].load(Ordering::Acquire);
            if p.is_null() {
                self.split_guard[i].fetch_sub(1, Ordering::Release);
                continue;
            }
            let sp = unsafe { &*p };
            let cap = max_join();
            if cap != 0 && sp.active.load(Ordering::Relaxed) >= cap {
                self.split_guard[i].fetch_sub(1, Ordering::Release);
                continue;
            }
            sp.active.fetch_add(1, Ordering::AcqRel);
            self.split_guard[i].fetch_sub(1, Ordering::Release);
            let ran = unsafe { help_split(sp) };
            if sp.active.fetch_sub(1, Ordering::AcqRel) == 1 {
                sp.waiter.unpark();
            }
            if ran {
                return true;
            }
        }
        false
    }

    fn try_run_one(&self) -> bool {
        use std::sync::atomic::Ordering;
        let task = {
            let mut q = self.q.lock().unwrap();
            let t = q.pop_front();
            self.queued.store(q.len(), Ordering::Relaxed);
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

    fn help_until(&self, done: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering;
        while !done.load(Ordering::Acquire) {
            if self.try_run_one() {
                continue;
            }
            if done.load(Ordering::Acquire) {
                return;
            }
            std::thread::park();
        }
    }

    fn worker_loop(&self) {
        use std::sync::atomic::Ordering;
        let mut q = self.q.lock().unwrap();
        self.idle.fetch_add(1, Ordering::Relaxed);
        loop {
            while q.is_empty() {
                if self.stop.load(Ordering::Relaxed) {
                    self.idle.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
                drop(q);
                self.idle.fetch_sub(1, Ordering::Relaxed);
                let helped = self.try_help_splits();
                self.idle.fetch_add(1, Ordering::Relaxed);
                q = self.q.lock().unwrap();
                if helped || !q.is_empty() {
                    continue;
                }
                if self.stop.load(Ordering::Relaxed) {
                    self.idle.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
                q = self.cv.wait(q).unwrap();
            }
            let t = q.pop_front().unwrap();
            self.queued.store(q.len(), Ordering::Relaxed);
            self.idle.fetch_sub(1, Ordering::Relaxed);
            drop(q);
            t();
            q = self.q.lock().unwrap();
            self.idle.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn shutdown(&self) {
        let _q = self.q.lock().unwrap();
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        drop(_q);
        self.cv.notify_all();
    }
}

const SORT_ALPHA_DELTA: i32 = 12;
const EDGE_STABILITY_ORDER_WEIGHT: i32 = 1;
const ASPIRATION_WIDTH: i32 = 6;

fn dbg_asp() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("DBG_ASP").is_ok_and(|v| v != "0"))
}
const WARM_ASPIRATION_WIDTH: i32 = 6;

#[cfg(feature = "tunable")]
fn warm_aspiration_width() -> i32 {
    static V: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("WARM_ASP_WIDTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(WARM_ASPIRATION_WIDTH)
    })
}
#[cfg(not(feature = "tunable"))]
fn warm_aspiration_width() -> i32 {
    WARM_ASPIRATION_WIDTH
}

const ESTIMATE_DEPTH: u8 = 6;
const SELECTIVE_MIN_EMPTIES: u8 = 10;

fn selective_ladder() -> Vec<f32> {
    static V: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        std::env::var("SEL_LADDER")
            .ok()
            .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
            .filter(|v: &Vec<f32>| !v.is_empty())
            .unwrap_or_else(|| SELECTIVE_LADDER.to_vec())
    })
    .clone()
}

#[cfg(feature = "tunable")]
fn selective_pass_min_empties() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("SEL_PASS_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(SELECTIVE_PASS_MIN_EMPTIES)
    })
}
#[cfg(not(feature = "tunable"))]
fn selective_pass_min_empties() -> u8 {
    SELECTIVE_PASS_MIN_EMPTIES
}

#[cfg(feature = "tunable")]
fn selective_min_empties() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("SEL_MIN_EMPTIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(SELECTIVE_MIN_EMPTIES)
    })
}
#[cfg(not(feature = "tunable"))]
fn selective_min_empties() -> u8 {
    SELECTIVE_MIN_EMPTIES
}

const SELECTIVE_PROBE_DEPTH: u8 = 2;

fn selective_sigma(empties: u8, pc: u8) -> f32 {
    if legacy_sigma() {
        return legacy_mpc_sigma(empties as u32, empties, pc);
    }

    let e = (empties as f32).clamp(14.0, 30.0);
    let p = (pc as f32).clamp(2.0, 10.0);
    let s = 7.246577 + 0.020516 * e - 0.438464 * p - 0.004024 * e * e
        + 0.000167 * p * p
        + 0.009864 * e * p;
    s.max(1.0)
}

#[inline]
fn legacy_mpc_sigma(empties: u32, depth: u8, pc_depth: u8) -> f32 {
    const A: f32 = -0.068941;
    const B: f32 = 0.368775;
    const C: f32 = -0.713476;
    const QA: f32 = 0.010223;
    const QB: f32 = 0.647219;
    const QC: f32 = 4.050545;
    let s = A * empties as f32 + B * depth as f32 + C * pc_depth as f32;
    QA * s * s + QB * s + QC
}

#[cfg(feature = "tunable")]
fn legacy_sigma() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SEL_SIGMA").is_ok_and(|v| v == "old"))
}
#[cfg(not(feature = "tunable"))]
fn legacy_sigma() -> bool {
    false
}

fn selective_probe_depth(empties: u8) -> u8 {
    static V: std::sync::OnceLock<Option<u8>> = std::sync::OnceLock::new();
    if let Some(d) = *V.get_or_init(|| {
        std::env::var("SEL_PROBE_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
    }) {
        return d;
    }
    static S: std::sync::OnceLock<Option<(u8, u8)>> = std::sync::OnceLock::new();
    if let Some((at, d)) = *S.get_or_init(|| {
        std::env::var("SEL_PROBE_STEP").ok().and_then(|v| {
            let (a, b) = v.split_once(':')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
    }) {
        if empties >= at {
            return d;
        }
        return SELECTIVE_PROBE_DEPTH;
    }
    let step = PROBE_STEP.load(std::sync::atomic::Ordering::Relaxed);
    if step != 0 {
        if empties >= (step >> 8) as u8 {
            return (step & 0xff) as u8;
        }
        return SELECTIVE_PROBE_DEPTH;
    }
    if selective_probe_scaled() {
        ((empties / 3) & !1) + (empties & 1)
    } else {
        SELECTIVE_PROBE_DEPTH
    }
}

static PROBE_STEP: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

#[cfg(feature = "tunable")]
static PROBE_SCALED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(u8::MAX);

#[cfg(feature = "tunable")]
fn selective_probe_scaled() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let mut v = PROBE_SCALED.load(Relaxed);
    if v == u8::MAX {
        v = std::env::var("SEL_PROBE_SCALED").is_ok_and(|s| s != "0") as u8;
        PROBE_SCALED.store(v, Relaxed);
    }
    v == 1
}
#[cfg(not(feature = "tunable"))]
fn selective_probe_scaled() -> bool {
    false
}

fn selective_gate_offset() -> Option<f32> {
    static V: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *V.get_or_init(|| match std::env::var("SEL_GATE").ok().as_deref() {
        Some("off") | Some("-") => None,
        Some(v) => v.parse().ok().filter(|v: &f32| *v >= 0.0).or(Some(4.0)),
        None => Some(4.0),
    })
}
const SELECTIVE_PASS_MIN_EMPTIES: u8 = 18;
const SELECTIVE_LADDER: [f32; 2] = [1.1, 1.8];

const SORT_DEPTH_LADDER: [u8; 64] =
    build_sort_ladder([DEEP_ORDER_EMPTIES, DEEP2_ORDER_EMPTIES, DEEP3_ORDER_EMPTIES]);

const fn build_sort_ladder(steps: [u8; 3]) -> [u8; 64] {
    let mut t = [0u8; 64];
    let mut e = 0usize;
    while e < 64 {
        t[e] = if e >= steps[2] as usize {
            3
        } else if e >= steps[1] as usize {
            2
        } else if e >= steps[0] as usize {
            1
        } else {
            0
        };
        e += 1;
    }
    t
}

fn sort_depth_ladder() -> &'static [u8; 64] {
    static V: std::sync::OnceLock<[u8; 64]> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let Ok(spec) = std::env::var("SORT_LADDER") else {
            return SORT_DEPTH_LADDER;
        };
        let mut steps = [DEEP_ORDER_EMPTIES, DEEP2_ORDER_EMPTIES, DEEP3_ORDER_EMPTIES];
        for (slot, field) in steps.iter_mut().zip(spec.split(',')) {
            if let Ok(v) = field.trim().parse() {
                *slot = v;
            }
        }
        build_sort_ladder(steps)
    })
}

#[inline(always)]
fn child_hash_of(child: &Board) -> u64 {
    zobrist::board_hash(child.player_bb(), child.opponent_bb())
}

#[cfg(test)]
fn neighbour_bit(sq: u8) -> u64 {
    let file = (sq / 8) as i32;
    let rank = (sq % 8) as i32;
    let mut mask = 0u64;
    for df in -1..=1i32 {
        for dr in -1..=1i32 {
            if df == 0 && dr == 0 {
                continue;
            }
            let f = file + df;
            let r = rank + dr;
            if (0..8).contains(&f) && (0..8).contains(&r) {
                mask |= 1u64 << (f * 8 + r);
            }
        }
    }
    mask
}

#[inline]
fn final_score_bb(player: u64, opponent: u64) -> i32 {
    let diff = player.count_ones() as i32 - opponent.count_ones() as i32;
    let empties = 64 - (player | opponent).count_ones() as i32;
    match diff.cmp(&0) {
        std::cmp::Ordering::Greater => diff + empties,
        std::cmp::Ordering::Less => diff - empties,
        std::cmp::Ordering::Equal => 0,
    }
}

#[inline]
fn parity_of_bb(occupied: u64) -> u8 {
    let empty = !occupied;
    let mut parity = 0u8;
    for (id, mask) in QUADRANT_MASKS {
        if (empty & mask).count_ones() & 1 != 0 {
            parity |= id;
        }
    }
    parity
}

#[inline]
fn quadrant_id(sq: u8) -> u8 {
    1u8 << (((sq >> 5) & 1) | (((sq >> 2) & 1) << 1))
}

const QUADRANT_MASKS: [(u8, u64); 4] = [
    (1, 0x0000_0000_0F0F_0F0F), // files 0-3, ranks 0-3
    (2, 0x0F0F_0F0F_0000_0000), // files 4-7, ranks 0-3
    (4, 0x0000_0000_F0F0_F0F0), // files 0-3, ranks 4-7
    (8, 0xF0F0_F0F0_0000_0000), // files 4-7, ranks 4-7
];

const PARITY_ODD_MASK: [u64; 16] = {
    let mut t = [0u64; 16];
    let mut p = 0usize;
    while p < 16 {
        let mut m = 0u64;
        let mut i = 0usize;
        while i < 4 {
            let (id, mask) = QUADRANT_MASKS[i];
            if p & id as usize != 0 {
                m |= mask;
            }
            i += 1;
        }
        t[p] = m;
        p += 1;
    }
    t
};

#[inline]
fn parity_of(board: &Board) -> u8 {
    parity_of_bb(board.black | board.white)
}

const VALUE_INF: i32 = i32::MAX / 2;

#[derive(Clone, Copy)]
#[repr(C, align(32))]
struct HashEntry {
    black: u64,
    white: u64,
    lower8: i8,
    upper8: i8,
    depth: u8,
    best8: u8,
    flags: u8,
    date: u8,
    _pad: [u8; 2],
}

impl HashEntry {
    const EMPTY: HashEntry = HashEntry {
        black: 0,
        white: 0,
        lower8: 0,
        upper8: 0,
        depth: 0,
        best8: 0,
        flags: 0,
        date: 0,
        _pad: [0; 2],
    };

    #[inline]
    fn used(&self) -> bool {
        self.flags & 1 != 0
    }

    #[inline]
    fn is_seed(&self, now: u8) -> bool {
        if self.flags & 4 != 0 {
            return true;
        }
        if self.date == now {
            return false;
        }
        if self.flags & Self::PROVEN != 0 {
            return false;
        }
        self.depth >= exact_keep_floor()
    }

    const PROVEN: u8 = 8;

    fn proven_entry(&self) -> bool {
        self.flags & Self::PROVEN != 0 || self.depth < structural_proof_floor()
    }

    #[inline]
    fn lower(&self) -> i32 {
        if self.lower8 == i8::MIN {
            -VALUE_INF
        } else {
            self.lower8 as i32
        }
    }

    #[inline]
    fn upper(&self) -> i32 {
        if self.upper8 == i8::MAX {
            VALUE_INF
        } else {
            self.upper8 as i32
        }
    }

    #[inline]
    fn best(&self) -> Option<Position> {
        (self.best8 < 64).then_some(Position(self.best8))
    }

    #[inline]
    fn matches(&self, board: &Board) -> bool {
        let want = 1 | ((board.player as u8) << 1);
        ((self.black ^ board.black) | (self.white ^ board.white)) == 0 && (self.flags & 3) == want
    }
}

struct SpinLock(std::sync::atomic::AtomicBool);

impl SpinLock {
    const fn new() -> SpinLock {
        SpinLock(std::sync::atomic::AtomicBool::new(false))
    }

    #[inline]
    fn lock(&self) -> SpinGuard<'_> {
        use std::sync::atomic::Ordering;
        while self
            .0
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.0.load(Ordering::Relaxed) {
                std::hint::spin_loop();
            }
        }
        SpinGuard(self)
    }
}

struct SpinGuard<'a>(&'a SpinLock);

impl Drop for SpinGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        self.0 .0.store(false, std::sync::atomic::Ordering::Release);
    }
}

const HASH_LOCK_STRIPES: usize = 4096;

struct HashTable {
    mask: u64,
    date: std::sync::atomic::AtomicU8,
    shared: std::sync::atomic::AtomicBool,
    entries: std::cell::UnsafeCell<Vec<HashEntry>>,
    locks: Vec<SpinLock>,
}

unsafe impl Sync for HashTable {}

impl HashTable {
    fn new(bit_size: u32) -> HashTable {
        let size = 1usize << bit_size;
        let mut locks = Vec::with_capacity(HASH_LOCK_STRIPES);
        locks.resize_with(HASH_LOCK_STRIPES, SpinLock::new);
        HashTable {
            mask: ((size >> 1) - 1) as u64,
            date: std::sync::atomic::AtomicU8::new(1),
            shared: std::sync::atomic::AtomicBool::new(false),
            entries: std::cell::UnsafeCell::new(zeroed_vec::<HashEntry>(size)),
            locks,
        }
    }

    #[inline]
    fn date(&self) -> u8 {
        self.date.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set_shared(&self, shared: bool) {
        self.shared
            .store(shared, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    fn is_shared(&self) -> bool {
        self.shared.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline]
    fn stripe(&self, hash: u64) -> &SpinLock {
        &self.locks[(hash as usize) & (HASH_LOCK_STRIPES - 1)]
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    unsafe fn slots(&self) -> &mut Vec<HashEntry> {
        &mut *self.entries.get()
    }

    #[inline(always)]
    fn prefetch(&self, hash: u64) {
        let base = ((hash & self.mask) as usize) << 1;
        unsafe {
            let p = (*self.entries.get()).as_ptr().add(base);
            #[cfg(target_arch = "aarch64")]
            std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) p, options(nostack, preserves_flags));
            #[cfg(not(target_arch = "aarch64"))]
            let _ = p;
        }
    }

    fn clear(&mut self, threads: usize) {
        let entries = self.entries.get_mut();
        if threads <= 1 {
            entries.fill(HashEntry::EMPTY);
            return;
        }
        let chunk = entries.len().div_ceil(threads);
        std::thread::scope(|scope| {
            for slice in entries.chunks_mut(chunk) {
                scope.spawn(|| slice.fill(HashEntry::EMPTY));
            }
        });
    }

    #[inline]
    fn get(&self, board: &Board, hash: u64) -> Option<HashEntry> {
        let base = ((hash & self.mask) as usize) << 1;
        let e = unsafe { self.slots() };
        let (e0, e1) = unsafe { (*e.get_unchecked(base), *e.get_unchecked(base + 1)) };
        let hit = if e0.matches(board) {
            e0
        } else if e1.matches(board) {
            e1
        } else {
            return None;
        };
        if !self.is_shared() {
            return Some(hit);
        }
        let _guard = self.stripe(hash).lock();
        let e = unsafe { self.slots() };
        if e[base].matches(board) {
            return Some(e[base]);
        }
        if e[base + 1].matches(board) {
            return Some(e[base + 1]);
        }
        None
    }

    fn update(
        &self,
        board: &Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        value: i32,
        best: Option<Position>,
        proven: bool,
    ) {
        let proven_bit = if proven && exact_proof() {
            HashEntry::PROVEN
        } else {
            0
        };
        let best8 = best.map_or(255, |p| p.index());
        let now = self.date();
        let base = ((hash & self.mask) as usize) << 1;
        let _guard = self.is_shared().then(|| self.stripe(hash).lock());
        let entries = unsafe { self.slots() };
        let slot = if entries[base].matches(board) {
            base
        } else if entries[base + 1].matches(board) {
            base + 1
        } else {
            let victim = if entries[base].depth <= entries[base + 1].depth {
                base
            } else {
                base + 1
            };
            let entry = &mut entries[victim];
            if !entry.used() || entry.depth <= board.empty_count() {
                *entry = HashEntry {
                    black: board.black,
                    white: board.white,
                    lower8: if value > alpha { value as i8 } else { i8::MIN },
                    upper8: if value < beta { value as i8 } else { i8::MAX },
                    depth: board.empty_count(),
                    best8,
                    flags: 1 | ((board.player as u8) << 1) | proven_bit,
                    date: self.date(),
                    _pad: [0; 2],
                };
            }
            return;
        };
        let entry = &mut entries[slot];
        if entry.is_seed(now) {
            entry.lower8 = if value > alpha { value as i8 } else { i8::MIN };
            entry.upper8 = if value < beta { value as i8 } else { i8::MAX };
            entry.depth = board.empty_count();
            entry.flags = 1 | ((board.player as u8) << 1) | proven_bit;
            entry.best8 = best8;
            entry.date = now;
            return;
        }
        if proven_bit == 0 {
            entry.flags &= !HashEntry::PROVEN;
        }
        if value < beta && (value as i8) < entry.upper8 {
            entry.upper8 = value as i8;
        }
        if value > alpha && (value as i8) > entry.lower8 {
            entry.lower8 = value as i8;
        }
        if best8 != 255 {
            entry.best8 = best8;
        }
        entry.date = now;
    }

    fn demote_to_seed_shared(&self) {
        let next = self.date().wrapping_add(1).max(1);
        self.date.store(next, std::sync::atomic::Ordering::Relaxed);
    }

    fn seed_update(
        &self,
        board: &Board,
        hash: u64,
        depth: u8,
        lower8: i8,
        upper8: i8,
        best: Option<Position>,
    ) {
        let base = ((hash & self.mask) as usize) << 1;
        let now = self.date();
        let _guard = self.stripe(hash).lock();
        let entries = unsafe { self.slots() };
        let slot = if entries[base].matches(board) {
            base
        } else if entries[base + 1].matches(board) {
            base + 1
        } else if !entries[base].used() {
            base
        } else if !entries[base + 1].used() {
            base + 1
        } else if entries[base].is_seed(now) && entries[base].depth <= depth {
            base
        } else if entries[base + 1].is_seed(now) && entries[base + 1].depth <= depth {
            base + 1
        } else {
            return; // never evict a real entry for a seed
        };
        let entry = &mut entries[slot];
        if entry.used() && !entry.is_seed(now) {
            return;
        }
        *entry = HashEntry {
            black: board.black,
            white: board.white,
            lower8,
            upper8,
            depth,
            best8: best.map_or(255, |p| p.index()),
            flags: 1 | ((board.player as u8) << 1) | 4,
            date: self.date(),
            _pad: [0; 2],
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndSolverMode {
    WinLossDraw,
    WinDraw,
    DrawLoss,
    Perfect,
}

#[derive(Debug, Clone)]
pub struct EndSolverResult {
    pub best_move: Option<Position>,
    pub value: i32,
    pub empty: u8,
    pub nodes: u64,
}

pub static ORDER_NNUE: std::sync::OnceLock<std::sync::Arc<crate::nnue::Nnue>> =
    std::sync::OnceLock::new();

fn order_indexer(e: &crate::linear::Linear) -> &crate::pattern_index::PatternIndexer {
    match order_nnue() {
        Some(nn) => nn.indexer(),
        None => e.indexer(),
    }
}

pub fn order_nnue() -> Option<&'static crate::nnue::Nnue> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var("KUROOBI_NNUE_ORDER").is_ok()) {
        ORDER_NNUE.get().map(|nn| &**nn)
    } else {
        None
    }
}

pub type NnueProbe = (
    std::sync::Arc<crate::nnue::Nnue>,
    std::sync::Arc<crate::midgame::SharedTt>,
);

#[cfg(feature = "tunable")]
fn sel_nnue_probe() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SEL_NNUE_PROBE").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn sel_nnue_probe() -> bool {
    true
}

#[cfg(feature = "tunable")]
fn sel_nnue_warm() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SEL_NNUE_WARM").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn sel_nnue_warm() -> bool {
    true
}

#[cfg(feature = "tunable")]
fn exact_keep_floor() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        if std::env::var("EXACT_KEEP_SHALLOW").is_ok_and(|v| v == "0") {
            return 0;
        }
        structural_proof_floor()
    })
}
#[cfg(not(feature = "tunable"))]
fn exact_keep_floor() -> u8 {
    structural_proof_floor()
}

fn exact_reopen_clamp() -> Option<i32> {
    static V: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EXACT_REOPEN_CLAMP")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|d: &i32| *d > 0)
    })
}

#[cfg(feature = "tunable")]
fn l4_cache() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("EXACT_L4_CACHE").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn l4_cache() -> bool {
    true
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct L4Entry {
    player: u64,
    opponent: u64,
    lower: i8,
    upper: i8,
    best8: u8,
}

const ENTRY_BYTES: usize = std::mem::size_of::<L4Entry>();
const _: () = assert!(ENTRY_BYTES == 19);

#[cfg(feature = "layer-profile")]
pub mod ab_stats {
    use std::sync::atomic::AtomicU64;
    pub static NODES: AtomicU64 = AtomicU64::new(0);
    pub static L4_PROBE: AtomicU64 = AtomicU64::new(0);
    pub static L4_HIT: AtomicU64 = AtomicU64::new(0);
    pub static L4_STORE: AtomicU64 = AtomicU64::new(0);
    pub static L56_PROBE: AtomicU64 = AtomicU64::new(0);
    pub static L56_HIT: AtomicU64 = AtomicU64::new(0);
    pub static CHILD: AtomicU64 = AtomicU64::new(0);
    pub static STAB4: AtomicU64 = AtomicU64::new(0);
    pub static STAB4_CUT: AtomicU64 = AtomicU64::new(0);
    pub static STAB_FULL: AtomicU64 = AtomicU64::new(0);

    pub static STAB_GATE_A: AtomicU64 = AtomicU64::new(0);
    pub static STAB_GATE_B: AtomicU64 = AtomicU64::new(0);
    pub static O_NODES: AtomicU64 = AtomicU64::new(0);

    pub static O_SCORED: AtomicU64 = AtomicU64::new(0);
    pub static O_TT_PROBE: AtomicU64 = AtomicU64::new(0);
    pub static O_TT_CUT: AtomicU64 = AtomicU64::new(0);
    pub static O_CHILD: AtomicU64 = AtomicU64::new(0);
}
#[cfg(feature = "layer-profile")]
macro_rules! abst {
    ($c:ident) => {
        ab_stats::$c.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    };
}
#[cfg(not(feature = "layer-profile"))]
macro_rules! abst {
    ($c:ident) => {
        ()
    };
}

const L4_EMPTY: L4Entry = L4Entry {
    player: 0,
    opponent: 0,
    lower: 0,
    upper: 0,
    best8: 0,
};

fn zeroed_vec<T: Copy>(len: usize) -> Vec<T> {
    let layout = std::alloc::Layout::array::<T>(len).expect("table size overflow");
    unsafe {
        let p = std::alloc::alloc_zeroed(layout) as *mut T;
        assert!(!p.is_null(), "table allocation failed");
        Vec::from_raw_parts(p, len, len)
    }
}

#[cfg(feature = "tunable")]
fn l4_bits() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EXACT_L4_BITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14)
    })
}
#[cfg(not(feature = "tunable"))]
fn l4_bits() -> u32 {
    14
}

#[cfg(feature = "tunable")]
fn no_shallow56() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NO_SHALLOW56").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn no_shallow56() -> bool {
    false
}

#[cfg(feature = "tunable")]
fn new_shallow() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NEW_SHALLOW").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn new_shallow() -> bool {
    true
}

#[cfg(feature = "tunable")]
fn new_mid() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NEW_MID").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn new_mid() -> bool {
    false
}

#[cfg(feature = "tunable")]
fn new_mid_bits() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NEW_MID_BITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(18)
    })
}
#[cfg(not(feature = "tunable"))]
fn new_mid_bits() -> u32 {
    18
}

#[cfg(feature = "tunable")]
fn new_78() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NEW_78").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn new_78() -> bool {
    true
}

#[cfg(feature = "tunable")]
fn l78_bits() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("L78_BITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15)
    })
}
#[cfg(not(feature = "tunable"))]
fn l78_bits() -> u32 {
    15
}

#[cfg(feature = "tunable")]
fn new_shallow_bits() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("NEW_SHALLOW_BITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14)
    })
}
#[cfg(not(feature = "tunable"))]
fn new_shallow_bits() -> u32 {
    14
}

#[cfg(feature = "tunable")]
fn l56_cache() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("EXACT_L56_CACHE").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn l56_cache() -> bool {
    false
}

#[cfg(feature = "tunable")]
fn exact_proof() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("EXACT_PROOF").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn exact_proof() -> bool {
    false
}

#[cfg(feature = "tunable")]
fn structural_proof_floor() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| pvs_limit().max(selective_min_empties()))
}
#[cfg(not(feature = "tunable"))]
fn structural_proof_floor() -> u8 {
    pvs_limit().max(SELECTIVE_MIN_EMPTIES)
}

pub struct Solver {
    hash_table: HashTable,
    nodes: u64,
    best: Option<Position>,
    threads: usize,
    nnue: Option<NnueProbe>,
    scratch: Option<(HashTable, HashTable, Vec<L4Entry>)>,
    budget: Option<std::sync::Arc<ThreadBudget>>,
    budget_threads: usize,
    budget_handles: Vec<std::thread::JoinHandle<()>>,
    stop: Option<crate::midgame::StopHandle>,
}

struct Worker<'a> {
    nodes: u64,
    l4: Vec<L4Entry>,
    l56: Vec<L4Entry>,
    l78: Vec<L4Entry>,
    g_l4_cache: bool,
    g_l56_cache: bool,
    g_new_shallow: bool,
    g_no_shallow56: bool,
    g_new_mid: bool,
    mid_empties: u8,
    order_ix: crate::pattern_index::PatternIndices,
    order_seeded: bool,
    tt: &'a HashTable,
    abort: &'a AbortFlag<'a>,
    budget: &'a ThreadBudget,
    stop: Option<&'a crate::midgame::StopHandle>,
    best: Option<Position>,
    warm_score: Option<i32>,
    warm_window: Option<(i32, i32)>,
    selective_t: Option<f32>,
    shallow_table: HashTable,
    mid_table: HashTable,
    nnue: Option<NnueProbe>,
    taint: u64,
    l9: Vec<L4Entry>,
    sigma_scale: f32,
}

impl<'a> Worker<'a> {
    fn with_tables(
        tt: &'a HashTable,
        budget: &'a ThreadBudget,
        abort: &'a AbortFlag<'a>,
        shallow_table: HashTable,
        mid_table: HashTable,
    ) -> Worker<'a> {
        Worker {
            tt,
            stop: None,
            nodes: 0,
            best: None,
            warm_score: None,
            warm_window: None,
            selective_t: None,
            shallow_table,
            mid_table,
            mid_empties: mid_tt_empties(),
            order_ix: crate::pattern_index::PatternIndices::ZERO,
            order_seeded: false,
            nnue: None,
            taint: 0,
            l56: if new_shallow() {
                zeroed_vec::<L4Entry>(1usize << new_shallow_bits())
            } else {
                Vec::new()
            },
            l9: if new_mid() {
                zeroed_vec::<L4Entry>(1usize << new_mid_bits())
            } else {
                Vec::new()
            },
            l78: if new_78() {
                zeroed_vec::<L4Entry>(1usize << l78_bits())
            } else {
                Vec::new()
            },
            l4: if l4_cache() || l56_cache() {
                let threads = budget.pool.workers + 1;
                let bits = l4_bits().saturating_sub(threads.ilog2()).max(13);
                zeroed_vec::<L4Entry>(1usize << bits)
            } else {
                Vec::new()
            },
            sigma_scale: 1.0,
            g_l4_cache: l4_cache(),
            g_l56_cache: l56_cache(),
            g_new_shallow: new_shallow(),
            g_no_shallow56: no_shallow56(),
            g_new_mid: new_mid(),
            budget,
            abort,
        }
    }

    #[inline(always)]
    fn l78_prefetch(&self, hash: u64) {
        if self.l78.is_empty() {
            return;
        }
        let idx = (hash as usize) & (self.l78.len() - 1);
        #[cfg(target_arch = "aarch64")]
        unsafe {
            std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) self.l78.as_ptr().add(idx), options(nostack, preserves_flags));
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = idx;
    }

    #[inline]
    fn l4_prefetch(&self, hash: u64) {
        if self.l4.is_empty() {
            return;
        }
        let idx = (hash as usize) & (self.l4.len() - 1);
        unsafe {
            let p = self.l4.as_ptr().add(idx);
            #[cfg(target_arch = "aarch64")]
            std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) p, options(nostack, preserves_flags));
            #[cfg(not(target_arch = "aarch64"))]
            let _ = p;
        }
    }

    #[inline]
    #[allow(clippy::type_complexity)]
    fn l78_probe(
        &self,
        hash: u64,
        player: u64,
        opponent: u64,
    ) -> Option<(i32, i32, Option<Position>)> {
        let e = unsafe {
            self.l78
                .get_unchecked((hash as usize) & (self.l78.len() - 1))
        };
        if e.player == player && e.opponent == opponent {
            let lo = if e.lower == i8::MIN {
                -VALUE_INF
            } else {
                e.lower as i32
            };
            let hi = if e.upper == i8::MAX {
                VALUE_INF
            } else {
                e.upper as i32
            };
            let best = if e.best8 == 0 {
                None
            } else {
                Some(Position(e.best8 - 1))
            };
            return Some((lo, hi, best));
        }
        None
    }

    #[inline]
    fn l78_store(
        &mut self,
        hash: u64,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
        score: i32,
        best: Option<Position>,
    ) {
        let idx = (hash as usize) & (self.l78.len() - 1);
        let e = unsafe { self.l78.get_unchecked_mut(idx) };
        if e.player != player || e.opponent != opponent {
            *e = L4Entry {
                player,
                opponent,
                lower: i8::MIN,
                upper: i8::MAX,
                best8: 0,
            };
        }
        if score > alpha {
            e.lower = e.lower.max(score as i8);
        }
        if score < beta {
            e.upper = e.upper.min(score as i8);
        }
        if let Some(b) = best {
            e.best8 = b.index() + 1;
        }
    }

    #[inline]
    #[allow(clippy::type_complexity)]
    fn l9_probe(
        &self,
        hash: u64,
        player: u64,
        opponent: u64,
    ) -> Option<(i32, i32, Option<Position>)> {
        let e = unsafe { self.l9.get_unchecked((hash as usize) & (self.l9.len() - 1)) };
        if e.player == player && e.opponent == opponent {
            let lo = if e.lower == i8::MIN {
                -VALUE_INF
            } else {
                e.lower as i32
            };
            let hi = if e.upper == i8::MAX {
                VALUE_INF
            } else {
                e.upper as i32
            };
            let best = if e.best8 == 0 {
                None
            } else {
                Some(Position(e.best8 - 1))
            };
            return Some((lo, hi, best));
        }
        None
    }

    #[inline]
    fn l9_store(
        &mut self,
        hash: u64,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
        score: i32,
        best: Option<Position>,
    ) {
        let idx = (hash as usize) & (self.l9.len() - 1);
        let e = unsafe { self.l9.get_unchecked_mut(idx) };
        if e.player != player || e.opponent != opponent {
            *e = L4Entry {
                player,
                opponent,
                lower: i8::MIN,
                upper: i8::MAX,
                best8: 0,
            };
        }
        if score > alpha {
            e.lower = e.lower.max(score as i8);
        }
        if score < beta {
            e.upper = e.upper.min(score as i8);
        }
        if let Some(b) = best {
            e.best8 = b.index() + 1;
        }
    }

    #[inline]
    fn l56_probe(
        &self,
        hash: u64,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
    ) -> Option<i32> {
        let e = unsafe {
            self.l56
                .get_unchecked((hash as usize) & (self.l56.len() - 1))
        };
        if e.player == player && e.opponent == opponent {
            if (e.lower as i32) >= beta {
                return Some(e.lower as i32);
            }
            if (e.upper as i32) <= alpha {
                return Some(e.upper as i32);
            }
        }
        None
    }

    #[inline]
    fn l56_store(
        &mut self,
        hash: u64,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
        score: i32,
    ) {
        let idx = (hash as usize) & (self.l56.len() - 1);
        let e = unsafe { self.l56.get_unchecked_mut(idx) };
        if e.player != player || e.opponent != opponent {
            *e = L4Entry {
                player,
                opponent,
                lower: i8::MIN,
                upper: i8::MAX,
                best8: 0,
            };
        }
        if score > alpha {
            e.lower = e.lower.max(score as i8);
        }
        if score < beta {
            e.upper = e.upper.min(score as i8);
        }
    }

    #[inline]
    fn l4_probe(
        &self,
        hash: u64,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
    ) -> Option<i32> {
        let e = unsafe { self.l4.get_unchecked((hash as usize) & (self.l4.len() - 1)) };
        if e.player == player && e.opponent == opponent {
            if (e.lower as i32) >= beta {
                return Some(e.lower as i32);
            }
            if (e.upper as i32) <= alpha {
                return Some(e.upper as i32);
            }
        }
        None
    }

    #[inline]
    fn l4_store(
        &mut self,
        hash: u64,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
        score: i32,
    ) {
        let idx = (hash as usize) & (self.l4.len() - 1);
        let e = unsafe { self.l4.get_unchecked_mut(idx) };
        if e.player != player || e.opponent != opponent {
            *e = L4Entry {
                player,
                opponent,
                lower: i8::MIN,
                upper: i8::MAX,
                best8: 0,
            };
        }
        if score > alpha {
            e.lower = e.lower.max(score as i8);
        }
        if score < beta {
            e.upper = e.upper.min(score as i8);
        }
    }

    #[inline(always)]
    fn table(&self, empties: u8) -> &HashTable {
        if empties < self.mid_empties {
            &self.mid_table
        } else {
            self.tt
        }
    }

    #[inline]
    fn should_abort(&mut self) -> bool {
        if self.abort.aborted() {
            return true;
        }
        self.stop.is_some_and(|s| s.is_stopped())
    }
}

impl Drop for Solver {
    fn drop(&mut self) {
        self.drop_budget();
    }
}

impl Solver {
    pub fn new(bit_size: u32) -> Solver {
        Solver {
            hash_table: HashTable::new(bit_size),
            nodes: 0,
            best: None,
            threads: 1,
            nnue: None,
            scratch: None,
            budget: None,
            budget_threads: 0,
            budget_handles: Vec::new(),
            stop: None,
        }
    }

    fn ensure_budget(&mut self, extra: usize) -> std::sync::Arc<ThreadBudget> {
        if self.budget.is_none() || self.budget_threads != extra {
            self.drop_budget();
            let b = std::sync::Arc::new(ThreadBudget::new(extra));
            for _ in 0..extra {
                let bc = std::sync::Arc::clone(&b);
                self.budget_handles
                    .push(std::thread::spawn(move || bc.pool.worker_loop()));
            }
            self.budget = Some(b);
            self.budget_threads = extra;
        }
        std::sync::Arc::clone(self.budget.as_ref().unwrap())
    }

    fn drop_budget(&mut self) {
        if let Some(b) = self.budget.take() {
            b.pool.shutdown();
            for h in self.budget_handles.drain(..) {
                let _ = h.join();
            }
            drop(b);
        }
        self.budget_threads = 0;
    }

    pub fn set_nnue(
        &mut self,
        nn: std::sync::Arc<crate::nnue::Nnue>,
        tt: std::sync::Arc<crate::midgame::SharedTt>,
    ) {
        self.nnue = Some((nn, tt));
    }

    pub fn set_stop(&mut self, stop: Option<crate::midgame::StopHandle>) {
        self.stop = stop;
    }

    pub fn set_threads(&mut self, threads: usize) {
        self.threads = threads.max(1);
    }

    pub fn solve(&mut self, mode: EndSolverMode, board: &Board) -> EndSolverResult {
        self.solve_with_eval(mode, board, None)
    }

    pub fn solve_with_eval(
        &mut self,
        mode: EndSolverMode,
        board: &Board,
        ev: Option<&Linear>,
    ) -> EndSolverResult {
        self.solve_impl(mode, board, ev, None)
    }

    pub fn solve_selective(
        &mut self,
        board: &Board,
        ev: Option<&Linear>,
        t: f32,
    ) -> EndSolverResult {
        self.solve_impl(EndSolverMode::Perfect, board, ev, Some(t))
    }

    fn solve_impl(
        &mut self,
        mode: EndSolverMode,
        board: &Board,
        ev: Option<&Linear>,
        selective: Option<f32>,
    ) -> EndSolverResult {
        self.nodes = 0;
        self.best = None;

        if !board.check_all() {
            return EndSolverResult {
                best_move: None,
                value: 0,
                empty: board.empty_count(),
                nodes: 0,
            };
        }

        {
            static DEEP_ROOT: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
            let n = *DEEP_ROOT.get_or_init(|| {
                std::env::var("SEL_DEEP_ROOT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(25)
            });
            let step = if selective.is_some() {
                0
            } else if n != 0 && board.empty_count() >= n {
                (20 << 8) | 6
            } else {
                3
            };
            PROBE_STEP.store(step, std::sync::atomic::Ordering::Relaxed);
        }

        self.hash_table.set_shared(self.threads > 1);

        let t_clear = std::time::Instant::now();
        self.hash_table.clear(self.threads);
        let (mut r_shallow, mut r_mid, mut r_l4) = self.scratch.take().unwrap_or_else(|| {
            let (sb, mb) = private_tt_bits();
            let l4 = if l4_cache() || l56_cache() {
                zeroed_vec::<L4Entry>(1usize << l4_bits())
            } else {
                Vec::new()
            };
            (HashTable::new(sb), HashTable::new(mb), l4)
        });
        r_shallow.clear(1);
        r_mid.clear(1);
        r_l4.fill(L4_EMPTY);
        if let Some(b) = self.budget.as_ref() {
            b.scratch.lock().unwrap().clear();
        }
        CLEAR_NS.fetch_add(
            t_clear.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        let extra = self.threads.saturating_sub(1);
        let budget_arc = self.ensure_budget(extra);
        let budget: &ThreadBudget = &budget_arc;
        let tt = &self.hash_table;
        let stop_ref = self.stop.as_ref();
        let root_abort = AbortFlag::root();
        let watching = std::sync::atomic::AtomicBool::new(true);
        let (value, nodes, best, r_shallow_back, r_mid_back, r_l4_back) =
            std::thread::scope(|scope| {
                struct StopWatch<'a>(&'a std::sync::atomic::AtomicBool);
                impl Drop for StopWatch<'_> {
                    fn drop(&mut self) {
                        self.0.store(false, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                let _watch = StopWatch(&watching);
                if stop_ref.is_some() {
                    let (stop, abort, watching) = (stop_ref, &root_abort, &watching);
                    scope.spawn(move || {
                        while watching.load(std::sync::atomic::Ordering::Relaxed) {
                            if stop.is_some_and(|s| s.is_stopped()) {
                                abort.abort();
                                return;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                    });
                }
                let mut w = Worker::with_tables(tt, budget, &root_abort, r_shallow, r_mid);
                w.l4 = r_l4;
                w.stop = stop_ref;
                w.nnue = if selective.is_some() || sel_nnue_warm() {
                    self.nnue.clone()
                } else {
                    None
                };
                w.sigma_scale = std::env::var("SEL_SIGMA_SCALE")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(if selective.is_some() { 0.6 } else { 1.0 });
                let mut b = *board;

                let t_warm = std::time::Instant::now();
                let mut rungs: Vec<f32> = selective_ladder();
                if let Some(t) = selective {
                    if !std::env::var("SEL_ONE_RUNG").is_ok_and(|v| v != "0") {
                        rungs.retain(|&r| r < t);
                        rungs.truncate(1);
                    } else {
                        rungs.clear();
                    }
                    rungs.push(t);
                }
                let mut last_selective: Option<i32> = None;
                if let Some(e) = ev {
                    if selective.is_some() || board.empty_count() >= selective_pass_min_empties() {
                        let mut guess = w.estimate_score(board, Some(e));
                        if dbg_asp() {
                            eprintln!("[warm] estimate {guess}");
                        }
                        for t in rungs {
                            w.selective_t = Some(t);
                            let mut sb = *board;
                            let n0 = w.nodes;
                            let s = w.aspiration_width(
                                &mut sb,
                                guess,
                                warm_aspiration_width(),
                                Some(e),
                            );
                            if dbg_asp() {
                                eprintln!(
                                    "[warm] rung t={t} -> {s}, best={:?} ({}M nodes)",
                                    w.best,
                                    (w.nodes - n0) / 1_000_000
                                );
                            }
                            w.selective_t = None;
                            guess = s - (s & 1);
                            last_selective = Some(s);
                            w.tt.demote_to_seed_shared();
                            w.warm_score = Some(s - (s & 1));
                        }
                        w.tt.demote_to_seed_shared();
                    }
                }

                WARMUP_NS.fetch_add(
                    t_warm.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                let warm_nodes = w.nodes;
                WARMUP_NODES.fetch_add(warm_nodes, std::sync::atomic::Ordering::Relaxed);
                if let Some(v) = last_selective.filter(|_| selective.is_some()) {
                    return (v, w.nodes, w.best, w.shallow_table, w.mid_table, w.l4);
                }
                let t_exact = std::time::Instant::now();
                let v = match mode {
                    EndSolverMode::WinLossDraw => w.pvs_root(&mut b, -1, 1, ev),
                    EndSolverMode::WinDraw => w.pvs_root(&mut b, 0, 1, ev),
                    EndSolverMode::DrawLoss => w.pvs_root(&mut b, -1, 0, ev),
                    EndSolverMode::Perfect => w.perfect(&mut b, ev),
                };
                EXACT_NS.fetch_add(
                    t_exact.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                EXACT_NODES.fetch_add(w.nodes - warm_nodes, std::sync::atomic::Ordering::Relaxed);
                (v, w.nodes, w.best, w.shallow_table, w.mid_table, w.l4)
            });
        self.scratch = Some((r_shallow_back, r_mid_back, r_l4_back));
        self.nodes = nodes;
        self.best = best;

        EndSolverResult {
            best_move: self.best,
            value,
            empty: board.empty_count(),
            nodes: self.nodes,
        }
    }
}

impl Worker<'_> {
    #[allow(clippy::too_many_arguments)]
    fn seed_search(
        &mut self,
        board: &Board,
        hash: u64,
        ev: &Linear,
        ix: &PatternIndexer,
        indices: &mut PatternIndices,
        depth: u8,
        alpha: f32,
        beta: f32,
        store: bool,
    ) -> f32 {
        let _prof = layer_profile::Scope::new(layer_profile::WARMUP, board.empty_count());
        self.nodes += 1;

        if let Some(v) = wipeout_score(board) {
            return v as f32;
        }
        if depth == 0 {
            return ev.eval_indices(board, indices);
        }

        let mut tt_move: Option<Position> = None;
        if let Some(e) = self.tt.get(board, hash) {
            tt_move = e.best();
            if !e.is_seed(self.tt.date()) && e.lower() >= e.upper() {
                return e.lower() as f32;
            }
        }

        let moves = board.movable();
        if moves == 0 {
            let mut child = *board;
            child.pass();
            if child.movable() == 0 {
                return final_score(board) as f32;
            }
            return -self.seed_search(
                &child,
                zobrist::board_hash(board.opponent_bb(), board.player_bb()),
                ev,
                ix,
                indices,
                depth,
                -beta,
                -alpha,
                store,
            );
        }

        let mover = board.player();
        let mut children = [(Position(0), *board, 0u64, 0u64, 0i32); 34];
        let mut n = 0usize;
        let mut m = moves;
        while m != 0 {
            let sq = m.trailing_zeros() as u8;
            m &= m - 1;
            let pos = Position(sq);
            let mut child = *board;
            let flipped = child.make_move_bits(pos);
            let child_hash = zobrist::board_hash(
                board.opponent_bb() ^ flipped,
                board.player_bb() | flipped | pos.to_bit(),
            );
            let key = if child.player_bb() == 0 || Some(pos) == tt_move {
                i32::MIN
            } else if depth >= 2 {
                let saved = *indices;
                ix.apply(indices, pos, flipped, mover);
                let v = (ev.eval_indices(&child, indices) * 8.0) as i32;
                *indices = saved;
                v
            } else {
                0
            };
            children[n] = (pos, child, child_hash, flipped, key);
            n += 1;
        }
        let children = &mut children[..n];
        if depth >= 2 || tt_move.is_some() {
            children.sort_unstable_by_key(|c| c.4);
        }

        let mut alpha = alpha;
        let mut best_val = f32::NEG_INFINITY;
        let mut best_move = None;
        let orig_alpha = alpha;
        for (pos, child, child_hash, flipped, _) in children.iter() {
            let saved = *indices;
            ix.apply(indices, *pos, *flipped, mover);
            let v = -self.seed_search(
                child,
                *child_hash,
                ev,
                ix,
                indices,
                depth - 1,
                -beta,
                -alpha,
                store,
            );
            *indices = saved;
            if v > best_val {
                best_val = v;
                best_move = Some(*pos);
                if v > alpha {
                    alpha = v;
                }
            }
            if alpha >= beta {
                break;
            }
        }

        if store {
            let v8 = best_val.round().clamp(-64.0, 64.0) as i8;
            let lower8 = if best_val > orig_alpha { v8 } else { i8::MIN };
            let upper8 = if best_val < beta { v8 } else { i8::MAX };
            self.tt
                .seed_update(board, hash, depth, lower8, upper8, best_move);
        }
        best_val
    }

    fn perfect(&mut self, board: &mut Board, ev: Option<&Linear>) -> i32 {
        if std::env::var("SEL_EXACT_FULLWIN").is_ok_and(|v| v != "0") && self.warm_score.is_some() {
            return self.pvs_root(board, -64, 64, ev);
        }
        if let Some(score) = self.warm_score {
            let recenter = std::env::var("SEL_ASP_RECENTER").is_ok_and(|v| v != "0");
            if let Some((mut lo, mut hi)) = self.warm_window.filter(|_| !recenter) {
                if lo < score && score < hi {
                    if let Some(d) = exact_reopen_clamp() {
                        lo = lo.max(score - d);
                        hi = hi.min(score + d);
                    }
                    if dbg_asp() {
                        eprintln!("[exact] reopening warm window [{lo},{hi}]");
                    }
                    let val = self.pvs_root(board, lo, hi, ev);
                    if val == ABORTED {
                        return ABORTED;
                    }
                    if lo < val && val < hi {
                        return val;
                    }
                    return self.aspiration(board, val, ev);
                }
            }
            return self.aspiration(board, score, ev);
        }

        let mut val = self.pvs_root(board, -1, 1, ev);
        if val == ABORTED {
            return ABORTED;
        }
        if val > 0 {
            let bound = val + 8;
            val = self.pvs_root(board, val, bound, ev);
            if val == ABORTED {
                return ABORTED;
            }
            if val >= bound {
                val = self.pvs_root(board, val, 64, ev);
            }
        } else if val < 0 {
            let bound = val - 8;
            val = self.pvs_root(board, bound, val, ev);
            if val == ABORTED {
                return ABORTED;
            }
            if val <= bound {
                val = self.pvs_root(board, -64, val, ev);
            }
        }
        val
    }

    fn estimate_score(&mut self, board: &Board, ev: Option<&Linear>) -> i32 {
        let Some(e) = ev else { return 0 };
        let ix = e.indexer();
        let mut indices = ix.init(board.black, board.white);
        let hash = zobrist::board_hash(board.player_bb(), board.opponent_bb());
        let depth = ESTIMATE_DEPTH.min(board.empty_count());
        let v = self.seed_search(
            board,
            hash,
            e,
            ix,
            &mut indices,
            depth,
            f32::NEG_INFINITY,
            f32::INFINITY,
            false,
        );
        let rounded = (v / 2.0).round() as i32 * 2;
        rounded.clamp(-62, 62)
    }

    fn aspiration(&mut self, board: &mut Board, score: i32, ev: Option<&Linear>) -> i32 {
        self.aspiration_width(board, score, ASPIRATION_WIDTH, ev)
    }

    fn aspiration_width(
        &mut self,
        board: &mut Board,
        mut score: i32,
        width: i32,
        ev: Option<&Linear>,
    ) -> i32 {
        let mut left = width;
        let mut right = width;
        for _ in 0..12 {
            let lo = (score - left).max(-64);
            let hi = (score + right).min(64);
            if lo >= hi || (lo <= -64 && hi >= 64) {
                break;
            }
            let n0 = self.nodes;
            let val = self.pvs_root(board, lo, hi, ev);
            if val == ABORTED {
                return ABORTED;
            }
            if dbg_asp() {
                eprintln!(
                    "[asp] window [{lo},{hi}] -> {val} ({}M nodes)",
                    (self.nodes - n0) / 1_000_000
                );
            }
            if val <= lo && lo > -64 {
                score = val;
                left = (left * 2).min(128);
                right = 0;
            } else if val >= hi && hi < 64 {
                score = val;
                left = 0;
                right = (right * 2).min(128);
            } else {
                self.warm_window = Some((lo, hi));
                return val;
            }
        }
        self.warm_window = Some((-64, 64));
        self.pvs_root(board, -64, 64, ev)
    }

    fn pvs_root(&mut self, board: &mut Board, alpha: i32, beta: i32, ev: Option<&Linear>) -> i32 {
        let mut lower = alpha;
        let upper = beta;
        let hash = zobrist::board_hash(board.player_bb(), board.opponent_bb());

        self.nodes += 1;

        if let Some(e) = ev.filter(|_| board.empty_count() >= eval_order_empties()) {
            self.order_ix = order_indexer(e).init(board.black, board.white);
            self.order_seeded = true;
        }
        let root_mover = board.player();

        let mut moves = MoveBuf::new();
        let tt_best = self.tt.get(board, hash).and_then(|e| e.best());
        self.scored_moves(board, tt_best, ev, &mut moves);
        if moves.is_empty() {
            return final_score(board);
        }
        moves.sort_by_key(|m| m.value);

        let mut best = moves[0].pos;
        let mut max;

        {
            let n0 = self.nodes;
            let m0 = moves[0];
            let mut child = m0.child(board);
            let ch = child_hash_of(&child);
            max = -self.pvs_ordered(&mut child, ch, -upper, -lower, false, ev, m0, root_mover);
            if max == -ABORTED {
                return ABORTED;
            }
            if dbg_asp() {
                eprintln!(
                    "[root] eldest {:?} [{lower},{upper}] -> {max} ({}M)",
                    moves[0].pos,
                    (self.nodes - n0) / 1_000_000
                );
            }
            if max > lower {
                lower = max;
            }
        }

        if moves.len() > 2 && board.empty_count() >= parallel_min_empties() && lower < upper {
            if let Some((val, bpos)) = self.split_siblings(board, &moves[1..], lower, upper, ev) {
                if val == ABORTED {
                    return ABORTED;
                }
                if val > max {
                    max = val;
                    if let Some(p) = bpos {
                        best = p;
                    }
                }
                let clean = self.selective_t.is_none();
                self.tt
                    .update(board, hash, alpha, beta, max, Some(best), clean);
                self.best = Some(best);
                return max;
            }
        }

        for m in &moves[1..] {
            if lower >= upper {
                break;
            }
            let mut child = m.child(board);
            let ch = child_hash_of(&child);
            let mut val =
                -self.pvs_ordered(&mut child, ch, -lower - 1, -lower, true, ev, *m, root_mover);
            if val == -ABORTED {
                return ABORTED;
            }
            if lower < val && val < upper {
                let ch = child_hash_of(&child);
                val = -self.pvs_ordered(&mut child, ch, -upper, -val, false, ev, *m, root_mover);
                if val == -ABORTED {
                    return ABORTED;
                }
            }
            if val > max {
                max = val;
                best = m.pos;
                if max > lower {
                    lower = max;
                }
            }
        }

        let clean = self.selective_t.is_none();
        self.tt
            .update(board, hash, alpha, beta, max, Some(best), clean);
        self.best = Some(best);
        max
    }

    fn split_siblings_v2(
        &mut self,
        parent: &Board,
        siblings: &[ScoredMove],
        lower: i32,
        upper: i32,
        ev: Option<&Linear>,
    ) -> Option<(i32, Option<Position>)> {
        use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};

        let pool = &self.budget.pool;
        SPLITS.fetch_add(1, Ordering::Relaxed);
        let group = AbortFlag::child(self.abort);
        let sp = SplitPoint {
            board: *parent,
            moves: siblings.as_ptr(),
            n_moves: siblings.len(),
            upper,
            selective_t: self.selective_t,
            nnue: self.nnue.clone(),
            sigma_scale: self.sigma_scale,
            tt: self.tt as *const HashTable,
            ev: ev.map_or(std::ptr::null(), |e| e as *const Linear as *const ()),
            group: &group as *const AbortFlag as *const (),
            budget: self.budget as *const ThreadBudget as *const (),
            cursor: AtomicUsize::new(0),
            shared_lower: AtomicI32::new(lower),
            active: AtomicUsize::new(0),
            merge: std::sync::Mutex::new(SplitMerge {
                max: i32::MIN,
                best_val: i32::MIN,
                best: None,
                aborted: false,
            }),
            nodes: AtomicU64::new(0),
            waiter: std::thread::current(),
        };
        let slot = pool.register_split(&sp);

        let mut unwound = false;
        let mut cut_short = false;
        loop {
            if group.aborted() {
                cut_short = true;
                break;
            }
            let cur = sp.shared_lower.load(Ordering::Relaxed);
            if cur >= upper {
                break;
            }
            let idx = sp.cursor.fetch_add(1, Ordering::Relaxed);
            if idx >= sp.n_moves {
                break;
            }
            let m = siblings[idx];
            let mut child = m.child(parent);
            let ch = child_hash_of(&child);
            let mover = parent.player();
            let mut val = -self.pvs_ordered(&mut child, ch, -cur - 1, -cur, true, ev, m, mover);
            if val == -ABORTED {
                unwound = true;
                break;
            }
            if cur < val && val < upper {
                let ch = child_hash_of(&child);
                val = -self.pvs_ordered(&mut child, ch, -upper, -val, false, ev, m, mover);
                if val == -ABORTED {
                    unwound = true;
                    break;
                }
            }
            if val > cur {
                sp.shared_lower.fetch_max(val, Ordering::Relaxed);
            }
            split_merge(&sp, val, cur, m.pos);
            if val >= upper {
                ABORT_FIRED.fetch_add(1, Ordering::Relaxed);
                if solver_abort() {
                    group.abort();
                }
                break;
            }
        }
        sp.cursor.store(sp.n_moves, Ordering::Release);
        if let Some(i) = slot {
            pool.unregister_split(i, &sp);
        }
        let t_wait = std::time::Instant::now();
        while sp.active.load(Ordering::Acquire) != 0 {
            std::thread::park();
        }
        WAIT_NS.fetch_add(t_wait.elapsed().as_nanos() as u64, Ordering::Relaxed);

        self.nodes += sp.nodes.load(Ordering::Relaxed);
        let mg = *sp.merge.lock().unwrap();
        if unwound {
            return Some((ABORTED, mg.best));
        }
        if (mg.aborted || cut_short) && mg.max < upper {
            return Some((ABORTED, mg.best));
        }
        Some((mg.max, mg.best))
    }

    fn split_siblings(
        &mut self,
        parent: &Board,
        siblings: &[ScoredMove],
        lower: i32,
        upper: i32,
        ev: Option<&Linear>,
    ) -> Option<(i32, Option<Position>)> {
        use std::sync::atomic::{AtomicI32, Ordering};

        let pool = &self.budget.pool;
        if pool.workers == 0 {
            return None;
        }
        if split_v2() {
            return self.split_siblings_v2(parent, siblings, lower, upper, ev);
        }
        SPLITS.fetch_add(1, Ordering::Relaxed);

        let shared_lower = AtomicI32::new(lower);
        let tt = self.tt;
        let budget = self.budget;
        let selective_t = self.selective_t;
        let nnue = self.nnue.clone();
        let sigma_scale = self.sigma_scale;
        let group = AbortFlag::child(self.abort);
        let group = &group;
        let waiter = std::thread::current();
        let slots: Vec<TaskSlot> = siblings
            .iter()
            .map(|_| TaskSlot::new(waiter.clone()))
            .collect();
        let board = *parent;

        let mut handed: Vec<usize> = Vec::new();
        let mut max = i32::MIN;
        let mut best_val = i32::MIN;
        let mut best: Option<Position> = None;
        let mut unwound = false;
        let mut cut_short = false;

        for (i, m) in siblings.iter().enumerate() {
            if group.aborted() {
                cut_short = true;
                break;
            }
            let cur = shared_lower.load(Ordering::Relaxed);
            if cur >= upper {
                break;
            }
            if i + 1 < siblings.len() {
                let m = *m;
                let slot = &slots[i];
                let shared = &shared_lower;
                let nnue = nnue.clone();
                let pushed = unsafe {
                    pool.try_push(move || {
                        run_one_sibling(
                            tt,
                            budget,
                            group,
                            selective_t,
                            nnue,
                            sigma_scale,
                            &board,
                            m,
                            upper,
                            shared,
                            slot,
                            ev,
                        );
                    })
                };
                if pushed {
                    handed.push(i);
                    HANDED.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                REFUSED.fetch_add(1, Ordering::Relaxed);
            }
            let n0 = self.nodes;
            let mut child = m.child(parent);
            let ch = child_hash_of(&child);
            let mover = parent.player();
            let mut val = -self.pvs_ordered(&mut child, ch, -cur - 1, -cur, true, ev, *m, mover);
            if val == -ABORTED {
                unwound = true;
                break;
            }
            if cur < val && val < upper {
                let ch = child_hash_of(&child);
                val = -self.pvs_ordered(&mut child, ch, -upper, -val, false, ev, *m, mover);
                if val == -ABORTED {
                    unwound = true;
                    break;
                }
            }
            if dbg_asp() && parent.empty_count() >= 10 {
                eprintln!(
                    "[split e{}] inline {:?} probe@{cur} -> {val} ({}M)",
                    parent.empty_count(),
                    m.pos,
                    (self.nodes - n0) / 1_000_000
                );
            }
            if val > max {
                max = val;
                shared_lower.fetch_max(val, Ordering::Relaxed);
            }
            if val > cur && val > best_val {
                best_val = val;
                best = Some(m.pos);
            }
            if val >= upper {
                break;
            }
        }

        let t_wait = std::time::Instant::now();
        for &i in &handed {
            pool.help_until(&slots[i].done);
        }
        WAIT_NS.fetch_add(t_wait.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let mut any_aborted = false;
        for &i in &handed {
            let (val, nodes) = slots[i].result();
            let beat = slots[i].beat.load(Ordering::Relaxed);
            self.nodes += nodes;
            if val == ABORTED {
                any_aborted = true;
                TASK_ABORTED.fetch_add(1, Ordering::Relaxed);
            }
            if dbg_asp() && parent.empty_count() >= 10 {
                eprintln!(
                    "[split e{}] handed {:?} -> {val} (beat {beat}) ({}M)",
                    parent.empty_count(),
                    siblings[i].pos,
                    nodes / 1_000_000
                );
            }
            if val != ABORTED {
                if val > max {
                    max = val;
                }
                if beat && val > best_val {
                    best_val = val;
                    best = Some(siblings[i].pos);
                }
            }
        }

        if unwound {
            return Some((ABORTED, best));
        }
        if (any_aborted || cut_short) && max < upper {
            return Some((ABORTED, best));
        }
        Some((max, best))
    }

    #[allow(clippy::too_many_arguments)]
    /// Stores a result, marked clean when no selective cut fed into it.
    #[inline(always)]
    fn store(
        &mut self,
        board: &Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        val: i32,
        best: Option<Position>,
        taint0: u64,
    ) {
        let clean = self.selective_t.is_none() || self.taint == taint0;
        self.tt.update(board, hash, alpha, beta, val, best, clean);
    }

    /// Enhanced transposition cutoff: a child the table already proves good enough.
    #[inline(always)]
    fn etc_cut(&mut self, board: &Board, moves: &MoveBuf, upper: i32) -> Option<i32> {
        let _p = layer_profile::Scope::new(layer_profile::ETC, board.empty_count());
        let mut probed = 0u64;
        for m in moves.iter() {
            probed += 1;
            let child = m.child(board);
            if let Some(e) = self.tt.get(&child, child_hash_of(&child)) {
                if !e.is_seed(self.tt.date()) && -e.upper() >= upper {
                    if self.selective_t.is_some() && !e.proven_entry() {
                        self.taint += 1;
                    }
                    node_accounting::etc(probed);
                    return Some(-e.upper());
                }
            }
        }
        node_accounting::etc(probed);
        None
    }

    fn pvs(
        &mut self,
        board: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        cut_node: bool,
        ev: Option<&Linear>,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, board.empty_count());
        let mut lower = alpha;
        let mut upper = beta;

        self.nodes += 1;
        if self.should_abort() {
            return ABORTED;
        }
        let mover = board.player();
        let taint0 = self.taint;

        if let Some(v) = wipeout_score(board) {
            return v;
        }

        let entry = {
            let _p = layer_profile::Scope::new(layer_profile::TT, board.empty_count());
            self.tt.get(board, hash)
        };
        if let Some(e) = entry {
            if !e.is_seed(self.tt.date()) {
                if self.selective_t.is_some() && !e.proven_entry() {
                    self.taint += 1;
                }
                let mut unused = false;
                if let Some(v) = narrow(&mut lower, &mut upper, &mut unused, e.lower(), e.upper()) {
                    return v;
                }
            }
        }

        {
            let _p = layer_profile::Scope::new(layer_profile::STAB, board.empty_count());
            if let Some(bound) = stability_cut(board, lower, upper) {
                return bound;
            }
        }

        if let (Some(t), Some(e)) = (self.selective_t, ev) {
            if board.empty_count() >= selective_min_empties() && upper - lower <= 1 {
                if let Some(v) = self.selective_cut(board, hash, e, t, lower, upper) {
                    self.taint += 1;
                    return v;
                }
            }
        }

        let mut moves = MoveBuf::new();
        self.gen_moves(board, &mut moves);

        if moves.is_empty() {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.pvs(
                board,
                zobrist::board_hash(board.opponent_bb(), board.player_bb()),
                -upper,
                -lower,
                true,
                !cut_node,
                ev,
            );
            board.pass();
            if val == -ABORTED {
                return ABORTED;
            }
            self.store(board, hash, alpha, beta, val, None, taint0);
            return val;
        }

        if moves.iter().any(|m| m.child(board).player_bb() == 0) {
            self.tt.update(board, hash, alpha, beta, 64, None, true);
            return 64;
        }

        if board.empty_count() >= etc_empties() {
            if let Some(v) = self.etc_cut(board, &moves, upper) {
                return v;
            }
        }

        let mut max = i32::MIN;
        let mut best = None;

        let tt_best = entry.and_then(|e| e.best());

        if spec_split()
            && !cut_node
            && upper - lower == 1
            && board.empty_count() >= parallel_min_empties()
            && board.empty_count() <= spec_split_max()
            && moves.len() > 2
            && self.budget.pool.workers > 0
        {
            self.score_moves(board, &mut moves, tt_best, ev, lower, parity_of(board));
            moves.sort_unstable_by_key(|m| m.value);
            if let Some((val, bpos)) = self.split_siblings(board, &moves, lower, upper, ev) {
                if val == ABORTED {
                    return ABORTED;
                }
                if self.selective_t.is_some() {
                    self.taint += 1;
                }
                self.store(board, hash, alpha, beta, val, bpos, taint0);
                return val;
            }
        }

        if let Some(bpos) = tt_best {
            if let Some(idx) = moves.iter().position(|m| m.pos == bpos) {
                let m = moves.swap_remove(idx);
                let mut child = m.child(board);
                let ch = child_hash_of(&child);
                max = self.descend_ordered(&mut child, ch, -upper, -lower, !cut_node, ev, m, mover);
                best = Some(m.pos);
                if max > lower {
                    lower = max;
                }
                if lower >= upper {
                    self.store(board, hash, alpha, beta, max, best, taint0);
                    node_accounting::cut_at(board.empty_count(), Some(0));
                    return max;
                }
            }
        }

        self.score_moves(board, &mut moves, None, ev, lower, parity_of(board));

        let mut next = 0usize;
        let tt_searched = max != i32::MIN;
        if max == i32::MIN {
            Self::select_next(&mut moves, 0);
            let m = moves[0];
            next = 1;
            let mut child = m.child(board);
            let ch = child_hash_of(&child);
            max = self.descend_ordered(&mut child, ch, -upper, -lower, !cut_node, ev, m, mover);
            if max == ABORTED {
                return ABORTED;
            }
            best = Some(m.pos);
            if max > lower {
                lower = max;
            }
        }

        if moves.len() - next > 1 && board.empty_count() >= parallel_min_empties() && lower < upper
        {
            moves[next..].sort_unstable_by_key(|m| m.value);
            if let Some((val, bpos)) = self.split_siblings(board, &moves[next..], lower, upper, ev)
            {
                if val == ABORTED {
                    return ABORTED;
                }
                if val > max {
                    max = val;
                    if let Some(p) = bpos {
                        best = Some(p);
                    }
                }
                if self.selective_t.is_some() {
                    self.taint += 1;
                }
                self.store(board, hash, alpha, beta, max, best, taint0);
                return max;
            }
        }

        while next < moves.len() {
            if lower >= upper {
                break;
            }
            Self::select_next(&mut moves, next);
            let m = moves[next];
            next += 1;
            let mut child = m.child(board);
            let val = {
                let ch = child_hash_of(&child);
                match ev.filter(|_| child.empty_count() >= eval_order_empties()) {
                    Some(e) => {
                        let saved = self.order_ix;
                        order_indexer(e).apply(&mut self.order_ix, m.pos, m.flipped, mover);
                        let v = self.descend_null_window(&mut child, ch, lower, upper, true, ev);
                        self.order_ix = saved;
                        v
                    }
                    None => self.descend_null_window(&mut child, ch, lower, upper, true, ev),
                }
            };
            if val == ABORTED {
                return ABORTED;
            }
            if val > max {
                max = val;
                best = Some(m.pos);
                if max > lower {
                    lower = max;
                }
            }
        }

        if moves.len() + usize::from(tt_searched) >= 2 {
            let searched = next + usize::from(tt_searched);
            node_accounting::cut_at(
                board.empty_count(),
                (lower >= upper).then_some(searched.saturating_sub(1)),
            );
        }

        self.store(board, hash, alpha, beta, max, best, taint0);
        max
    }

    #[allow(clippy::too_many_arguments)]
    fn pvs_ordered(
        &mut self,
        child: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        cut_node: bool,
        ev: Option<&Linear>,
        m: ScoredMove,
        mover: crate::color::Color,
    ) -> i32 {
        let Some(e) = ev.filter(|_| child.empty_count() >= eval_order_empties()) else {
            return self.pvs(child, hash, alpha, beta, false, cut_node, ev);
        };
        let saved = self.order_ix;
        order_indexer(e).apply(&mut self.order_ix, m.pos, m.flipped, mover);
        let v = self.pvs(child, hash, alpha, beta, false, cut_node, ev);
        self.order_ix = saved;
        v
    }

    #[allow(clippy::too_many_arguments)]
    fn descend_ordered(
        &mut self,
        child: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        cut_node: bool,
        ev: Option<&Linear>,
        m: ScoredMove,
        mover: crate::color::Color,
    ) -> i32 {
        let Some(e) = ev.filter(|_| child.empty_count() >= eval_order_empties()) else {
            return self.descend(child, hash, alpha, beta, cut_node, ev);
        };
        let saved = self.order_ix;
        order_indexer(e).apply(&mut self.order_ix, m.pos, m.flipped, mover);
        let v = self.descend(child, hash, alpha, beta, cut_node, ev);
        self.order_ix = saved;
        v
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn descend(
        &mut self,
        child: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        cut_node: bool,
        ev: Option<&Linear>,
    ) -> i32 {
        if child.empty_count() >= pvs_limit() {
            let dbg = dbg_asp() && child.empty_count() >= 23;
            let n0 = self.nodes;
            let v = self.pvs(child, hash, alpha, beta, false, cut_node, ev);
            if dbg {
                eprintln!(
                    "[d{}] h{:04x} [{alpha},{beta}] -> {v} ({}M)",
                    child.empty_count(),
                    hash & 0xffff,
                    (self.nodes - n0) / 1_000_000
                );
            }
            if v == ABORTED {
                return ABORTED;
            }
            -v
        } else {
            let parity = parity_of(child);
            if child.empty_count() >= MOVE_ORDERING_LIMIT {
                -self.alpha_beta_ordered(child, hash, alpha, beta, false, parity, ev)
            } else {
                -self.alpha_beta(child, hash, alpha, beta, false, parity)
            }
        }
    }

    #[inline(always)]
    fn descend_null_window(
        &mut self,
        child: &mut Board,
        hash: u64,
        lower: i32,
        upper: i32,
        cut_node: bool,
        ev: Option<&Linear>,
    ) -> i32 {
        let mut val = self.descend(child, hash, -lower - 1, -lower, cut_node, ev);
        if val == ABORTED {
            return ABORTED;
        }
        if lower < val && val < upper {
            val = self.descend(child, hash, -upper, -val, false, ev);
            if val == ABORTED {
                return ABORTED;
            }
        }
        val
    }

    #[allow(clippy::too_many_arguments)]
    fn alpha_beta_ordered(
        &mut self,
        board: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
        ev: Option<&Linear>,
    ) -> i32 {
        self.alpha_beta_ordered_bb(
            board.player_bb(),
            board.opponent_bb(),
            board.empty_count(),
            board.player,
            hash,
            alpha,
            beta,
            passed,
            parity,
            ev,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn ordered_store(
        &mut self,
        use_l9: bool,
        use_tt: bool,
        use_l78: bool,
        n_empties: u8,
        to_move: crate::color::Color,
        hash: u64,
        player: u64,
        opponent: u64,
        orig_alpha: i32,
        beta: i32,
        value: i32,
        best: Option<Position>,
    ) {
        if use_l9 {
            self.l9_store(hash, player, opponent, orig_alpha, beta, value, best);
        } else if use_tt {
            self.table(n_empties).update(
                &board_of(player, opponent, to_move),
                hash,
                orig_alpha,
                beta,
                value,
                best,
                true,
            );
        } else if use_l78 {
            self.l78_store(hash, player, opponent, orig_alpha, beta, value, best);
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// One child at the negated window: ordered search above the limit, plain below.
    #[inline(always)]
    fn child_value(
        &mut self,
        player: u64,
        opponent: u64,
        n_empties: u8,
        to_move: crate::color::Color,
        m: ScoredMove,
        alpha: i32,
        beta: i32,
        parity: u8,
        ev: Option<&Linear>,
    ) -> i32 {
        let (cbp, cbo) = child_bb(player, opponent, m);
        let cp = parity ^ quadrant_id(m.pos.index());
        let ch = zobrist::board_hash(cbp, cbo);
        let ce = n_empties - 1;
        if ce >= MOVE_ORDERING_LIMIT {
            -self.alpha_beta_ordered_bb(
                cbp,
                cbo,
                ce,
                to_move.opponent(),
                ch,
                -beta,
                -alpha,
                false,
                cp,
                ev,
            )
        } else {
            -self.alpha_beta_bb(cbp, cbo, ce, ch, -beta, -alpha, false, cp)
        }
    }

    fn alpha_beta_ordered_bb(
        &mut self,
        player: u64,
        opponent: u64,
        n_empties: u8,
        to_move: crate::color::Color,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
        ev: Option<&Linear>,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, n_empties);
        debug_assert!(alpha < beta, "alpha_beta_ordered needs a non-empty window");
        let mut alpha = alpha;
        let mut beta = beta;

        self.nodes += 1;

        if let Some(v) = wipeout_score_bb(player, opponent) {
            return v;
        }

        let use_tt = n_empties >= tt_min_empties();
        if use_tt {
            self.table(n_empties).prefetch(hash);
        }

        {
            let _p = layer_profile::Scope::new(layer_profile::STAB, n_empties);
            if let Some(bound) = stability_cut_bb(player, opponent, n_empties, alpha, beta) {
                return bound;
            }
        }

        abst!(O_NODES);
        let use_l9 = use_tt && self.g_new_mid && n_empties < self.mid_empties;
        let use_l78 = !use_tt && !self.l78.is_empty();
        let mut l9_best: Option<Position> = None;
        let mut l78_best: Option<Position> = None;
        let entry = {
            let _p = layer_profile::Scope::new(layer_profile::TT, n_empties);
            if use_tt && !use_l9 {
                abst!(O_TT_PROBE);
                self.table(n_empties)
                    .get(&board_of(player, opponent, to_move), hash)
            } else {
                None
            }
        };
        let mut narrowed = false;
        if use_l9 {
            if let Some((lo, hi, best)) = self.l9_probe(hash, player, opponent) {
                l9_best = best;
                if let Some(v) = narrow(&mut alpha, &mut beta, &mut narrowed, lo, hi) {
                    return v;
                }
            }
        } else if let Some(e) = entry {
            if !e.is_seed(self.table(n_empties).date()) {
                if let Some(v) = narrow(&mut alpha, &mut beta, &mut narrowed, e.lower(), e.upper())
                {
                    return v;
                }
            }
        }

        if use_l78 {
            if let Some((lo, hi, best)) = self.l78_probe(hash, player, opponent) {
                l78_best = best;
                if let Some(v) = narrow(&mut alpha, &mut beta, &mut narrowed, lo, hi) {
                    return v;
                }
            }
        }

        if narrowed {
            let _p = layer_profile::Scope::new(layer_profile::STAB, n_empties);
            if let Some(bound) = stability_cut_bb(player, opponent, n_empties, alpha, beta) {
                return bound;
            }
        }

        let orig_alpha = alpha;
        let mut moves = MoveBuf::new();
        self.gen_moves_bb(player, opponent, n_empties, &mut moves);

        if moves.is_empty() {
            if passed {
                return final_score_bb(player, opponent);
            }
            let val = -self.alpha_beta_ordered_bb(
                opponent,
                player,
                n_empties,
                to_move.opponent(),
                zobrist::board_hash(player, opponent),
                -beta,
                -alpha,
                true,
                parity,
                ev,
            );
            if use_l9 {
                self.l9_store(hash, player, opponent, orig_alpha, beta, val, None);
            } else if use_tt {
                self.table(n_empties).update(
                    &board_of(player, opponent, to_move),
                    hash,
                    orig_alpha,
                    beta,
                    val,
                    None,
                    true,
                );
            } else if use_l78 {
                self.l78_store(hash, player, opponent, orig_alpha, beta, val, None);
            }
            return val;
        }

        if moves.len() == 1 {
            let m = moves.at(0);
            return self.child_value(
                player, opponent, n_empties, to_move, m, alpha, beta, parity, ev,
            );
        }

        let tt_best = if use_l9 {
            l9_best
        } else {
            entry.and_then(|e| e.best())
        }
        .or(l78_best);
        let mut best = None;
        let mut scored_from = 0usize;
        if let Some(bpos) = tt_best {
            if let Some(idx) = moves.iter().position(|m| m.pos == bpos) {
                abst!(O_CHILD);
                moves.swap(0, idx);
                scored_from = 1;
                let m = moves.at(0);
                let val = self.child_value(
                    player, opponent, n_empties, to_move, m, alpha, beta, parity, ev,
                );
                if val >= beta {
                    alpha = val;
                    abst!(O_TT_CUT);
                    node_accounting::cut_at(n_empties, Some(0));
                    best = Some(m.pos);
                    self.ordered_store(
                        use_l9, use_tt, use_l78, n_empties, to_move, hash, player, opponent,
                        orig_alpha, beta, alpha, best,
                    );
                    return alpha;
                }
                alpha = alpha.max(val);
            }
        }

        for _ in scored_from..moves.len() {
            abst!(O_SCORED);
        }
        if n_empties >= eval_order_empties() {
            let scratch = board_of(player, opponent, to_move);
            self.score_moves(
                &scratch,
                moves.tail_mut(scored_from),
                None,
                ev,
                i32::MIN / 2,
                parity,
            );
        } else {
            self.score_moves_static_bb(
                player,
                opponent,
                n_empties,
                moves.tail_mut(scored_from),
                None,
                parity,
            );
        }

        for i in scored_from..moves.len() {
            Self::select_next(&mut moves, i);
            let m = moves.at(i);
            let val = self.child_value(
                player, opponent, n_empties, to_move, m, alpha, beta, parity, ev,
            );
            if val >= beta {
                alpha = val;
                best = Some(m.pos);
                node_accounting::cut_at(n_empties, Some(i));
                break;
            }
            alpha = alpha.max(val);
        }
        if best.is_none() {
            node_accounting::cut_at(n_empties, None);
        }

        self.ordered_store(
            use_l9, use_tt, use_l78, n_empties, to_move, hash, player, opponent, orig_alpha, beta,
            alpha, best,
        );
        alpha
    }

    fn alpha_beta(
        &mut self,
        board: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
    ) -> i32 {
        self.alpha_beta_bb(
            board.player_bb(),
            board.opponent_bb(),
            board.empty_count(),
            hash,
            alpha,
            beta,
            passed,
            parity,
        )
    }

    #[allow(clippy::too_many_arguments)]
    /// The five-empty child: cached four-empty search, or the plain one.
    #[inline(always)]
    fn child5(
        &mut self,
        player_bb: u64,
        opponent_bb: u64,
        pos_bit: u64,
        flips: u64,
        empties: u64,
        rest: u64,
        alpha: i32,
        beta: i32,
        parity: u8,
    ) -> i32 {
        let cp = opponent_bb ^ flips;
        let co = player_bb | flips | pos_bit;
        if !self.g_l4_cache {
            return -self.search4(cp, co, empties & !pos_bit, alpha, beta, parity);
        }
        let child_hash = zobrist::board_hash(cp, co);
        if rest != 0 {
            let jb = rest.isolate_lowest_one();
            let jf = bitboard::flippable(player_bb, opponent_bb, jb);
            if jf != 0 {
                self.l4_prefetch(zobrist::board_hash(opponent_bb ^ jf, player_bb | jf | jb));
            }
        }
        abst!(L4_PROBE);
        if let Some(h) = self.l4_probe(child_hash, cp, co, alpha, beta) {
            abst!(L4_HIT);
            self.nodes += 1;
            return -h;
        }
        let mut trivial = false;
        let r = self.search4_t(
            cp,
            co,
            empties & !pos_bit,
            alpha,
            beta,
            parity,
            &mut trivial,
        );
        if !trivial {
            abst!(L4_STORE);
            self.l4_store(child_hash, cp, co, alpha, beta, r);
        }
        -r
    }

    #[inline(always)]
    fn shallow_store(
        &mut self,
        player: u64,
        opponent: u64,
        hash: u64,
        lower: i32,
        upper: i32,
        best: i32,
    ) {
        if self.g_l56_cache {
            self.l4_store(hash, player, opponent, lower, upper, best);
        }
        if self.g_new_shallow {
            self.l56_store(hash, player, opponent, lower, upper, best);
        } else if !self.g_no_shallow56 {
            self.shallow_table.update(
                &board_of(player, opponent, crate::color::Color::Black),
                hash,
                lower,
                upper,
                best,
                None,
                true,
            );
        }
    }

    fn alpha_beta_bb(
        &mut self,
        player: u64,
        opponent: u64,
        n_empties: u8,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
    ) -> i32 {
        if n_empties == 4 {
            if let Some(v) = wipeout_score_bb(player, opponent) {
                self.nodes += 1;
                return v;
            }
            {
                let _p = layer_profile::Scope::new(layer_profile::STAB, 4);
                if let Some(bound) = stability_cut_bb(player, opponent, 4, alpha, beta) {
                    self.nodes += 1;
                    return bound;
                }
            }
            return self.last4_bb(player, opponent, alpha, beta, passed, parity);
        }

        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, n_empties);
        self.nodes += 1;

        if let Some(v) = wipeout_score_bb(player, opponent) {
            return v;
        }

        {
            let _p = layer_profile::Scope::new(layer_profile::STAB, n_empties);
            if let Some(bound) = stability_cut_bb(player, opponent, n_empties, alpha, beta) {
                return bound;
            }
        }

        let mut lower = alpha;
        let mut upper = beta;
        if self.g_l56_cache {
            if let Some(v) = self.l4_probe(hash, player, opponent, lower, upper) {
                return v;
            }
        }
        abst!(NODES);
        if self.g_new_shallow {
            abst!(L56_PROBE);
            if let Some(v) = self.l56_probe(hash, player, opponent, lower, upper) {
                abst!(L56_HIT);
                return v;
            }
        } else if !self.g_no_shallow56 {
            if let Some(v) = self.shallow_table.get(
                &board_of(player, opponent, crate::color::Color::Black),
                hash,
            ) {
                let mut unused = false;
                if let Some(x) = narrow(&mut lower, &mut upper, &mut unused, v.lower(), v.upper()) {
                    return x;
                }
            }
        }

        let orig_lower = lower;
        let mut best = lower;
        let mut any = false;
        let mut cut = false;
        let player_bb = player;
        let opponent_bb = opponent;
        let five_empty = n_empties == 5;
        let need_child_hash = !five_empty;

        let empties = bitboard::empty_bb(player, opponent);
        let odd = PARITY_ODD_MASK[parity as usize];
        let legal = bitboard::mobility(player_bb, opponent_bb, empties);
        let odd_moves = legal & odd;
        let even_moves = legal & !odd;
        let classes = [
            odd_moves & CORNER_MASK,
            odd_moves & !CORNER_MASK,
            even_moves & CORNER_MASK,
            even_moves & !CORNER_MASK,
        ];
        let mut class_ix = 0usize;
        let mut cur = classes[0];
        'passes: loop {
            {
                while cur == 0 {
                    class_ix += 1;
                    if class_ix == 4 {
                        break 'passes;
                    }
                    cur = unsafe { *classes.get_unchecked(class_ix) };
                }
                let sq = cur.trailing_zeros() as u8;
                cur &= cur - 1;
                let pos = Position(sq);
                let pos_bit = pos.to_bit();
                let flips = bitboard::flippable(player_bb, opponent_bb, pos_bit);

                any = true;
                abst!(CHILD);
                let child_parity = parity ^ quadrant_id(sq);
                let val = if five_empty {
                    self.child5(
                        player_bb,
                        opponent_bb,
                        pos_bit,
                        flips,
                        empties,
                        cur,
                        -upper,
                        -best.max(orig_lower),
                        child_parity,
                    )
                } else {
                    let cp = opponent_bb ^ flips;
                    let co = player_bb | flips | pos_bit;
                    let child_hash = if need_child_hash {
                        zobrist::board_hash(cp, co)
                    } else {
                        0
                    };
                    -self.alpha_beta_bb(
                        cp,
                        co,
                        n_empties - 1,
                        child_hash,
                        -upper,
                        -best.max(orig_lower),
                        false,
                        child_parity,
                    )
                };

                if val >= upper {
                    best = val;
                    cut = true;
                    break 'passes;
                }
                if val > best {
                    best = val;
                }
            }
        }
        let _ = cut;

        if !any {
            if passed {
                return final_score_bb(player, opponent);
            }
            return -self.alpha_beta_bb(
                opponent,
                player,
                n_empties,
                zobrist::board_hash(player, opponent),
                -upper,
                -orig_lower,
                true,
                parity,
            );
        }

        self.shallow_store(player, opponent, hash, orig_lower, upper, best);
        best
    }

    fn search4(
        &mut self,
        player: u64,
        opponent: u64,
        empties: u64,
        alpha: i32,
        beta: i32,
        parity: u8,
    ) -> i32 {
        let mut trivial = false;
        self.search4_t(player, opponent, empties, alpha, beta, parity, &mut trivial)
    }

    #[allow(clippy::too_many_arguments)]
    fn search4_t(
        &mut self,
        player: u64,
        opponent: u64,
        empties: u64,
        alpha: i32,
        beta: i32,
        parity: u8,
        trivial: &mut bool,
    ) -> i32 {
        if player == 0 {
            *trivial = true;
            self.nodes += 1;
            return -64;
        }
        {
            let _p = layer_profile::Scope::new(layer_profile::STAB, 4);
            abst!(STAB4);
            if let Some(bound) = stability_cut_bb(player, opponent, 4, alpha, beta) {
                abst!(STAB4_CUT);
                *trivial = true;
                self.nodes += 1;
                return bound;
            }
        }
        let mut e = empties;
        let p1 = e.trailing_zeros() as u8;
        e &= e - 1;
        let p2 = e.trailing_zeros() as u8;
        e &= e - 1;
        let p3 = e.trailing_zeros() as u8;
        e &= e - 1;
        let p4 = e.trailing_zeros() as u8;
        self.last4(player, opponent, p1, p2, p3, p4, alpha, beta, false, parity)
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn last4_bb(
        &mut self,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
    ) -> i32 {
        let mut e = bitboard::empty_bb(player, opponent);
        let p1 = e.trailing_zeros() as u8;
        e &= e - 1;
        let p2 = e.trailing_zeros() as u8;
        e &= e - 1;
        let p3 = e.trailing_zeros() as u8;
        e &= e - 1;
        let p4 = e.trailing_zeros() as u8;
        self.last4(
            player, opponent, p1, p2, p3, p4, alpha, beta, passed, parity,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn last4(
        &mut self,
        player: u64,
        opponent: u64,
        p1: u8,
        p2: u8,
        p3: u8,
        p4: u8,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, 4);
        self.nodes += 1;

        let (p1, p2, p3, p4) = if parity != 0 {
            let m = ((parity & quadrant_id(p1) != 0) as u8)
                | (((parity & quadrant_id(p2) != 0) as u8) << 1)
                | (((parity & quadrant_id(p3) != 0) as u8) << 2)
                | (((parity & quadrant_id(p4) != 0) as u8) << 3);
            match m {
                0b0010 => (p2, p1, p3, p4),
                0b0100 => (p3, p1, p2, p4),
                0b0101 => (p1, p3, p2, p4),
                0b0110 => (p2, p3, p1, p4),
                0b1000 => (p4, p1, p2, p3),
                0b1001 => (p1, p4, p2, p3),
                0b1010 => (p2, p4, p1, p3),
                0b1011 => (p1, p2, p4, p3),
                0b1100 => (p3, p4, p1, p2),
                0b1101 => (p1, p3, p4, p2),
                0b1110 => (p2, p3, p4, p1),
                _ => (p1, p2, p3, p4),
            }
        } else {
            (p1, p2, p3, p4)
        };

        let mut alpha = alpha;
        let mut any = false;

        let (f1, f2, f3, f4) = bitboard::flippable4(player, opponent, p1, p2, p3, p4);

        if f1 != 0 {
            any = true;
            let val = -self.last3(
                opponent ^ f1,
                player | f1 | (1u64 << p1),
                p2,
                p3,
                p4,
                -beta,
                -alpha,
                false,
                parity ^ quadrant_id(p1),
            );
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }
        if f2 != 0 {
            any = true;
            let val = -self.last3(
                opponent ^ f2,
                player | f2 | (1u64 << p2),
                p1,
                p3,
                p4,
                -beta,
                -alpha,
                false,
                parity ^ quadrant_id(p2),
            );
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }
        if f3 != 0 {
            any = true;
            let val = -self.last3(
                opponent ^ f3,
                player | f3 | (1u64 << p3),
                p1,
                p2,
                p4,
                -beta,
                -alpha,
                false,
                parity ^ quadrant_id(p3),
            );
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }
        if f4 != 0 {
            let val = -self.last3(
                opponent ^ f4,
                player | f4 | (1u64 << p4),
                p1,
                p2,
                p3,
                -beta,
                -alpha,
                false,
                parity ^ quadrant_id(p4),
            );
            return alpha.max(val);
        }

        if !any {
            if passed {
                return final_score_bb(player, opponent);
            }
            return -self.last4(
                opponent, player, p1, p2, p3, p4, -beta, -alpha, true, parity,
            );
        }

        alpha
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn last3(
        &mut self,
        player: u64,
        opponent: u64,
        p1: u8,
        p2: u8,
        p3: u8,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, 3);
        self.nodes += 1;

        let (p1, p2, p3) = {
            let m = ((parity & quadrant_id(p1) != 0) as u8)
                | (((parity & quadrant_id(p2) != 0) as u8) << 1)
                | (((parity & quadrant_id(p3) != 0) as u8) << 2);
            match m {
                0b010 => (p2, p1, p3),
                0b100 => (p3, p1, p2),
                0b101 => (p1, p3, p2),
                0b110 => (p2, p3, p1),
                _ => (p1, p2, p3),
            }
        };

        let mut alpha = alpha;
        let mut any = false;

        let (f1, f2, f3) = bitboard::flippable3(player, opponent, p1, p2, p3);

        if f1 != 0 {
            any = true;
            let val = -self.last2(
                opponent ^ f1,
                player | f1 | (1u64 << p1),
                p2,
                p3,
                -beta,
                -alpha,
                false,
            );
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }
        if f2 != 0 {
            any = true;
            let val = -self.last2(
                opponent ^ f2,
                player | f2 | (1u64 << p2),
                p1,
                p3,
                -beta,
                -alpha,
                false,
            );
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }
        if f3 != 0 {
            let val = -self.last2(
                opponent ^ f3,
                player | f3 | (1u64 << p3),
                p1,
                p2,
                -beta,
                -alpha,
                false,
            );
            return alpha.max(val);
        }

        if !any {
            if passed {
                return final_score_bb(player, opponent);
            }
            return -self.last3(opponent, player, p1, p2, p3, -beta, -alpha, true, parity);
        }

        alpha
    }

    #[inline(always)]
    fn last2(
        &mut self,
        player: u64,
        opponent: u64,
        p1: u8,
        p2: u8,
        alpha: i32,
        beta: i32,
        passed: bool,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, 2);
        self.nodes += 1;

        let mut alpha = alpha;
        let mut any = false;

        let (f1, f2) = bitboard::flippable2(player, opponent, p1, p2);
        if f1 != 0 {
            any = true;
            let (v, c) = Self::last1_value(opponent ^ f1, p2);
            self.nodes += c;
            let val = -v;
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }
        if f2 != 0 {
            let (v, c) = Self::last1_value(opponent ^ f2, p1);
            self.nodes += c;
            return alpha.max(-v);
        }

        if !any {
            if passed {
                return final_score_bb(player, opponent);
            }
            return -self.last2(opponent, player, p1, p2, -beta, -alpha, true);
        }

        alpha
    }

    #[inline(always)]
    fn last1_value(player: u64, p1: u8) -> (i32, u64) {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, 1);
        let diff = 2 * player.count_ones() as i32 - 63;
        let (mine, theirs) = bitboard::count_last_flips(player, p1);

        if mine > 0 {
            return (diff + 2 * mine as i32 + 1, 1);
        }
        if theirs > 0 {
            return (diff - 2 * theirs as i32 - 1, 2);
        }
        let v = match diff.cmp(&0) {
            std::cmp::Ordering::Greater => diff + 1,
            std::cmp::Ordering::Less => diff - 1,
            std::cmp::Ordering::Equal => 0,
        };
        (v, 2)
    }

    fn selective_cut(
        &mut self,
        board: &Board,
        hash: u64,
        ev: &Linear,
        t: f32,
        lower: i32,
        upper: i32,
    ) -> Option<i32> {
        let empties = board.empty_count();
        let pd = selective_probe_depth(empties);
        let error = t * selective_sigma(empties, pd) * self.sigma_scale;

        if sel_nnue_probe() {
            if let Some((nn, mtt)) = self.nnue.clone() {
                const EPS: f32 = 0.01;
                let mut acc = nn.indices(board.black, board.white);
                let gate = selective_gate_offset()
                    .map(|off| (nn.eval_from_indices(&acc, board), (error - off).max(1.0)));
                let mut ms = crate::midgame::NnueSearch::new(nn, mtt);
                let hi = upper as f32 + error;
                if hi < 64.0 && gate.is_none_or(|(d0, e0)| d0 >= upper as f32 + e0) {
                    let v = ms.negamax(board, &mut acc, pd as u32, hi - EPS, hi);
                    if v >= hi {
                        self.nodes += ms.nodes;
                        return Some(upper);
                    }
                }
                let lo = lower as f32 - error;
                if lo > -64.0 && gate.is_none_or(|(d0, e0)| d0 <= lower as f32 - e0) {
                    let v = ms.negamax(board, &mut acc, pd as u32, lo, lo + EPS);
                    if v <= lo {
                        self.nodes += ms.nodes;
                        return Some(lower);
                    }
                }
                self.nodes += ms.nodes;
                return None;
            }
        }

        let ix = ev.indexer();
        let mut indices = ix.init(board.black, board.white);

        let gate = selective_gate_offset().map(|off| {
            let d0 = self.seed_search(
                board,
                hash,
                ev,
                ix,
                &mut indices,
                0,
                f32::NEG_INFINITY,
                f32::INFINITY,
                false,
            );
            (d0, (error - off).max(1.0))
        });

        let hi = upper as f32 + error;
        if hi < 64.0 && gate.is_none_or(|(d0, e0)| d0 >= upper as f32 + e0) {
            let v = self.seed_search(board, hash, ev, ix, &mut indices, pd, hi - 1.0, hi, false);
            if v >= hi {
                return Some(upper);
            }
        }
        let lo = lower as f32 - error;
        if lo > -64.0 && gate.is_none_or(|(d0, e0)| d0 <= lower as f32 - e0) {
            let v = self.seed_search(board, hash, ev, ix, &mut indices, pd, lo, lo + 1.0, false);
            if v <= lo {
                return Some(lower);
            }
        }
        None
    }

    fn scored_moves(
        &self,
        board: &Board,
        tt_best: Option<Position>,
        ev: Option<&Linear>,
        out: &mut MoveBuf,
    ) {
        self.gen_moves(board, out);
        self.score_moves(board, out, tt_best, ev, i32::MIN / 2, parity_of(board));
    }

    fn gen_moves(&self, board: &Board, out: &mut MoveBuf) {
        self.gen_moves_bb(
            board.player_bb(),
            board.opponent_bb(),
            board.empty_count(),
            out,
        )
    }

    fn gen_moves_bb(&self, p: u64, o: u64, n_empties: u8, out: &mut MoveBuf) {
        let _prof = layer_profile::Scope::new(layer_profile::GEN, n_empties);
        out.len = 0;

        let n = {
            let mut m = bitboard::mobility(p, o, bitboard::empty_bb(p, o));
            let mut n = 0usize;
            while m != 0 {
                let s0 = m.trailing_zeros() as u8;
                m &= m - 1;
                if m == 0 {
                    out.write_at(
                        n,
                        ScoredMove {
                            pos: Position(s0),
                            flipped: bitboard::flippable(p, o, 1u64 << s0),
                            value: 0,
                        },
                    );
                    n += 1;
                    break;
                }
                let s1 = m.trailing_zeros() as u8;
                m &= m - 1;
                if m == 0 {
                    let (f0, f1) = bitboard::flippable2(p, o, s0, s1);
                    out.write_at(
                        n,
                        ScoredMove {
                            pos: Position(s0),
                            flipped: f0,
                            value: 0,
                        },
                    );
                    out.write_at(
                        n + 1,
                        ScoredMove {
                            pos: Position(s1),
                            flipped: f1,
                            value: 0,
                        },
                    );
                    n += 2;
                    break;
                }
                let s2 = m.trailing_zeros() as u8;
                m &= m - 1;
                if m == 0 {
                    let (f0, f1) = bitboard::flippable2(p, o, s0, s1);
                    let f2 = bitboard::flippable(p, o, 1u64 << s2);
                    for (k, (sq, fl)) in [(s0, f0), (s1, f1), (s2, f2)].into_iter().enumerate() {
                        out.write_at(
                            n + k,
                            ScoredMove {
                                pos: Position(sq),
                                flipped: fl,
                                value: 0,
                            },
                        );
                    }
                    n += 3;
                    break;
                }
                let s3 = m.trailing_zeros() as u8;
                m &= m - 1;
                let (f0, f1, f2, f3) = bitboard::flippable4(p, o, s0, s1, s2, s3);
                for (k, (sq, fl)) in [(s0, f0), (s1, f1), (s2, f2), (s3, f3)]
                    .into_iter()
                    .enumerate()
                {
                    out.write_at(
                        n + k,
                        ScoredMove {
                            pos: Position(sq),
                            flipped: fl,
                            value: 0,
                        },
                    );
                }
                n += 4;
            }
            n
        };

        out.len = n;
        let child_empties = n_empties - 1;
        let eager = child_empties >= tt_min_empties();
        if eager {
            for i in 0..n {
                let m = out[i];
                let child_hash = zobrist::board_hash(o ^ m.flipped, p | m.flipped | m.pos.to_bit());
                if child_empties >= tt_min_empties() {
                    self.table(child_empties).prefetch(child_hash);
                } else if child_empties >= MOVE_ORDERING_LIMIT {
                    self.l78_prefetch(child_hash);
                }
            }
        }
    }

    fn score_moves_static(
        &self,
        board: &Board,
        moves: &mut [ScoredMove],
        tt_best: Option<Position>,
        parity: u8,
    ) {
        self.score_moves_static_bb(
            board.player_bb(),
            board.opponent_bb(),
            board.empty_count(),
            moves,
            tt_best,
            parity,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn score_moves_static_bb(
        &self,
        player_bb: u64,
        opponent_bb: u64,
        n_empties: u8,
        moves: &mut [ScoredMove],
        tt_best: Option<Position>,
        parity: u8,
    ) {
        let _prof = layer_profile::Scope::new(layer_profile::ORDER, n_empties);
        node_accounting::sorted(moves.len() as u64);
        let pot = order_pot();
        match tt_best {
            None => {
                for sm in moves.iter_mut() {
                    let pos = sm.pos;
                    let flipped = sm.flipped;
                    let cp = opponent_bb ^ flipped;
                    let co = player_bb | flipped | pos.to_bit();
                    sm.value = if cp == 0 {
                        i32::MIN
                    } else {
                        move_ordering_value(pos, cp, co, parity, pot)
                    };
                }
            }
            Some(best) => {
                for sm in moves.iter_mut() {
                    let pos = sm.pos;
                    let flipped = sm.flipped;
                    let cp = opponent_bb ^ flipped;
                    let co = player_bb | flipped | pos.to_bit();
                    sm.value = if cp == 0 {
                        i32::MIN
                    } else if pos == best {
                        i32::MIN + 1
                    } else {
                        move_ordering_value(pos, cp, co, parity, pot)
                    };
                }
            }
        }
    }

    fn score_moves(
        &self,
        board: &Board,
        moves: &mut [ScoredMove],
        tt_best: Option<Position>,
        ev: Option<&Linear>,
        alpha: i32,
        parity: u8,
    ) {
        let _prof = layer_profile::Scope::new(layer_profile::ORDER, board.empty_count());
        node_accounting::sorted(moves.len() as u64);
        let pot = order_pot();
        let eval_order = board.empty_count() >= eval_order_empties();
        if !eval_order || ev.is_none() {
            return self.score_moves_static(board, moves, tt_best, parity);
        }
        if std::env::var_os("KUROOBI_ORDER_CHECK").is_some() {
            if let Some(e) = ev {
                let want = order_indexer(e).init(board.black, board.white);
                if want != self.order_ix {
                    ORDER_MISMATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !self.order_seeded {
                        ORDER_MISMATCH_UNSEEDED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    if self.order_ix == crate::pattern_index::PatternIndices::ZERO {
                        ORDER_MISMATCH_ZERO.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }
        }
        let mut order_ix = if eval_order {
            ev.map(|e| (order_indexer(e), self.order_ix))
        } else {
            None
        };
        let mover = board.player();
        let sort_hi = if alpha <= i32::MIN / 4 {
            f32::INFINITY
        } else {
            -(alpha - SORT_ALPHA_DELTA) as f32
        };

        let sort_depth = sort_depth_ladder()[(board.empty_count() as usize).min(63)];
        let player_bb = board.player_bb();
        let opponent_bb = board.opponent_bb();
        for sm in moves.iter_mut() {
            let pos = sm.pos;
            let flipped = sm.flipped;
            let cp = opponent_bb ^ flipped;
            let co = player_bb | flipped | pos.to_bit();
            sm.value = if cp == 0 {
                i32::MIN
            } else if Some(pos) == tt_best {
                i32::MIN + 1
            } else if let (Some(e), Some((ix, indices))) = (ev, order_ix.as_mut()) {
                let child = sm.child(board);
                let saved = *indices;
                ix.apply(indices, pos, flipped, mover);
                let v = if sort_depth > 0 {
                    shallow_search(
                        &child,
                        e,
                        ix,
                        indices,
                        sort_depth,
                        f32::NEG_INFINITY,
                        sort_hi,
                    )
                } else if let Some(nn) = order_nnue() {
                    nn.eval_from_indices(indices, &child)
                } else {
                    e.eval_order_bb(cp, co, mover.opponent(), indices)
                };
                *indices = saved;
                let edge = corner_stability_bb(co) * 8;
                (v * 8.0) as i32 + weighted_mobility(cp, co) * MOBILITY_ORDER_WEIGHT
                    - edge * EDGE_STABILITY_ORDER_WEIGHT
            } else {
                move_ordering_value(pos, cp, co, parity, pot)
            };
        }
    }

    #[inline]
    fn select_next(moves: &mut [ScoredMove], i: usize) {
        let mut best = i;
        let n = moves.len();
        for j in i + 1..n {
            unsafe {
                if moves.get_unchecked(j).value < moves.get_unchecked(best).value {
                    best = j;
                }
            }
        }
        moves.swap(i, best);
    }
}

#[derive(Clone, Copy)]
struct ScoredMove {
    pos: Position,
    flipped: u64,
    value: i32,
}

impl ScoredMove {
    #[inline(always)]
    fn child(&self, parent: &Board) -> Board {
        let mut b = *parent;
        b.apply_flips(self.pos, self.flipped);
        b
    }
}

const MAX_MOVES: usize = 34;

struct MoveBuf {
    buf: [std::mem::MaybeUninit<ScoredMove>; MAX_MOVES],
    len: usize,
}

impl MoveBuf {
    #[inline]
    fn new() -> MoveBuf {
        MoveBuf {
            buf: [const { std::mem::MaybeUninit::uninit() }; MAX_MOVES],
            len: 0,
        }
    }

    #[inline(always)]
    fn write_at(&mut self, i: usize, m: ScoredMove) {
        debug_assert!(i < MAX_MOVES);
        unsafe { self.buf.get_unchecked_mut(i).write(m) };
    }

    #[inline(always)]
    fn at(&self, i: usize) -> ScoredMove {
        debug_assert!(i < self.len);
        unsafe { self.buf.get_unchecked(i).assume_init() }
    }

    #[inline(always)]
    fn tail_mut(&mut self, from: usize) -> &mut [ScoredMove] {
        debug_assert!(from <= self.len);
        unsafe {
            std::slice::from_raw_parts_mut(
                self.buf.as_mut_ptr().add(from) as *mut ScoredMove,
                self.len - from,
            )
        }
    }

    #[inline]
    fn swap_remove(&mut self, i: usize) -> ScoredMove {
        let out = unsafe { self.buf[i].assume_init() };
        self.len -= 1;
        self.buf[i] = self.buf[self.len];
        out
    }
}

impl std::ops::Deref for MoveBuf {
    type Target = [ScoredMove];
    #[inline]
    fn deref(&self) -> &[ScoredMove] {
        unsafe { std::slice::from_raw_parts(self.buf.as_ptr() as *const ScoredMove, self.len) }
    }
}

impl std::ops::DerefMut for MoveBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut [ScoredMove] {
        unsafe {
            std::slice::from_raw_parts_mut(self.buf.as_mut_ptr() as *mut ScoredMove, self.len)
        }
    }
}

#[rustfmt::skip]
const SHALLOW_ORDER: [u8; 64] = {
    let rm: [u8; 64] = [
         0,  9,  3,  5,  5,  3,  9,  0,
         9, 12,  7,  8,  8,  7, 12,  9,
         3,  7,  1,  4,  4,  1,  7,  3,
         5,  8,  4,  6,  6,  4,  8,  5,
         5,  8,  4,  6,  6,  4,  8,  5,
         3,  7,  1,  4,  4,  1,  7,  3,
         9, 12,  7,  8,  8,  7, 12,  9,
         0,  9,  3,  5,  5,  3,  9,  0,
    ];
    let mut t = [0u8; 64];
    let mut f = 0;
    while f < 8 { let mut r = 0; while r < 8 { t[f*8+r] = rm[r*8+f]; r += 1; } f += 1; }
    t
};

const SHALLOW_TIERS: [u64; 13] = {
    let mut t = [0u64; 13];
    let mut sq = 0usize;
    while sq < 64 {
        t[SHALLOW_ORDER[sq] as usize] |= 1u64 << sq;
        sq += 1;
    }
    t
};

#[allow(clippy::too_many_arguments)]
fn shallow_search(
    board: &Board,
    ev: &Linear,
    ix: &PatternIndexer,
    indices: &mut PatternIndices,
    depth: u8,
    alpha: f32,
    beta: f32,
) -> f32 {
    let _prof = layer_profile::Scope::new(layer_profile::LOOKAHEAD, board.empty_count());
    node_accounting::lookahead();
    if depth == 0 {
        if let Some(nn) = order_nnue() {
            return nn.eval_from_indices(indices, board);
        }
        return ev.eval_order_bb(
            board.player_bb(),
            board.opponent_bb(),
            board.player(),
            indices,
        );
    }
    let moves = board.movable();
    if moves == 0 {
        let mut p = *board;
        p.pass();
        if p.movable() == 0 {
            return final_score(board) as f32 * 1000.0;
        }
        return -shallow_search(&p, ev, ix, indices, depth, -beta, -alpha);
    }
    let mut alpha = alpha;
    let mut best = f32::NEG_INFINITY;
    let mover = board.player();

    'classes: for tier in SHALLOW_TIERS {
        let mut m = moves & tier;
        while m != 0 {
            let sq = m.trailing_zeros() as u8;
            m &= m - 1;
            let pos = Position(sq);
            let mut child = *board;
            let flipped = child.make_move_bits(pos);
            let saved = *indices;
            ix.apply(indices, pos, flipped, mover);
            let v = -shallow_search(&child, ev, ix, indices, depth - 1, -beta, -alpha);
            *indices = saved;
            if v > best {
                best = v;
                if v > alpha {
                    alpha = v;
                }
                if alpha >= beta {
                    break 'classes;
                }
            }
        }
    }
    best
}

#[inline]
fn dilate(x: u64) -> u64 {
    let v = x | ((x & 0x7F7F_7F7F_7F7F_7F7F) << 1) | ((x & 0xFEFE_FEFE_FEFE_FEFE) >> 1);
    v | (v << 8) | (v >> 8)
}

#[inline]
fn weighted_mobility(cp: u64, co: u64) -> i32 {
    const CORNERS: u64 = 0x8100_0000_0000_0081;
    let m = bitboard::mobility(cp, co, !(cp | co));
    (m.count_ones() + (m & CORNERS).count_ones()) as i32
}

#[cfg(feature = "tunable")]
fn order_pot() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("ORDER_POT").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn order_pot() -> bool {
    false
}

fn move_ordering_value(pos: Position, cp: u64, co: u64, parity: u8, pot: bool) -> i32 {
    const W_MOBILITY: i32 = 1 << 15;
    const W_CORNER_STABILITY: i32 = 1 << 11;
    const W_POTENTIAL: i32 = 1 << 5;
    const W_PARITY: i32 = 1 << 3;

    if !pot {
        let mut score = SQUARE_VALUE[(pos.index() & 63) as usize] as i32;
        score += corner_stability_bb(co) * W_CORNER_STABILITY;
        score += (36 - weighted_mobility(cp, co)) * W_MOBILITY;
        return -score;
    }
    let empty = !(cp | co);
    let potential = (dilate(co) & empty).count_ones() as i32;
    let mut score = SQUARE_VALUE[(pos.index() & 63) as usize] as i32;
    if parity & quadrant_id(pos.index()) != 0 {
        score += W_PARITY;
    }
    score += (36 - potential) * W_POTENTIAL;
    score += corner_stability_bb(co) * W_CORNER_STABILITY;
    score += (36 - weighted_mobility(cp, co)) * W_MOBILITY;
    -score
}

const CORNER_MASK: u64 = 0x8100_0000_0000_0081;

const SQUARE_VALUE: [u8; 64] = [
    18, 4, 16, 12, 12, 16, 4, 18, //
    4, 2, 6, 8, 8, 6, 2, 4, //
    16, 6, 14, 10, 10, 14, 6, 16, //
    12, 8, 10, 0, 0, 10, 8, 12, //
    12, 8, 10, 0, 0, 10, 8, 12, //
    16, 6, 14, 10, 10, 14, 6, 16, //
    4, 2, 6, 8, 8, 6, 2, 4, //
    18, 4, 16, 12, 12, 16, 4, 18,
];

#[inline]
fn corner_stability_bb(bb: u64) -> i32 {
    const RANK1_CORNERS: u64 = 0x0100_0000_0000_0001;
    const RANK8_CORNERS: u64 = 0x8000_0000_0000_0080;
    const FILE_A_CORNERS: u64 = 0x0000_0000_0000_0081;
    const FILE_H_CORNERS: u64 = 0x8100_0000_0000_0000;
    const CORNERS: u64 = 0x8100_0000_0000_0081;

    let stable = (((RANK1_CORNERS & bb) << 1)
        | ((RANK8_CORNERS & bb) >> 1)
        | ((FILE_A_CORNERS & bb) << 8)
        | ((FILE_H_CORNERS & bb) >> 8)
        | CORNERS)
        & bb;
    stable.count_ones() as i32
}

pub mod layer_profile {
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

    pub const SEARCH: u32 = 0;
    pub const ORDER: u32 = 1;
    pub const LOOKAHEAD: u32 = 2;
    pub const WARMUP: u32 = 3;
    pub const GEN: u32 = 4;
    pub const ETC: u32 = 5;
    pub const TT: u32 = 6;
    pub const STAB: u32 = 7;
    pub const PHASES: usize = 8;

    pub static STATE: AtomicU32 = AtomicU32::new(0);
    pub static NODES: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
    pub const ENABLED: bool = cfg!(feature = "layer-profile");

    pub struct Scope(#[allow(dead_code)] u32);

    impl Scope {
        #[inline(always)]
        pub fn new(phase: u32, empties: u8) -> Self {
            #[cfg(feature = "layer-profile")]
            {
                let prev = STATE.load(Relaxed);
                STATE.store((phase << 8) | empties as u32, Relaxed);
                if phase == SEARCH {
                    NODES[(empties as usize) & 63].fetch_add(1, Relaxed);
                }
                return Scope(prev);
            }
            #[cfg(not(feature = "layer-profile"))]
            {
                let _ = (phase, empties);
                Scope(0)
            }
        }
    }

    impl Drop for Scope {
        #[inline(always)]
        fn drop(&mut self) {
            #[cfg(feature = "layer-profile")]
            STATE.store(self.0, Relaxed);
        }
    }

    pub fn sample() -> (usize, usize) {
        let v = STATE.load(Relaxed);
        (
            ((v >> 8) as usize).min(PHASES - 1),
            (v & 0xFF) as usize & 63,
        )
    }
}

pub mod node_accounting {
    #[cfg(feature = "node-accounting")]
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    #[cfg(feature = "node-accounting")]
    static SORTED: AtomicU64 = AtomicU64::new(0);
    #[cfg(feature = "node-accounting")]
    static ETC: AtomicU64 = AtomicU64::new(0);
    #[cfg(feature = "node-accounting")]
    static LOOKAHEAD: AtomicU64 = AtomicU64::new(0);
    #[cfg(feature = "node-accounting")]
    static CUT_AT: [[AtomicU64; 4]; 64] = [const { [const { AtomicU64::new(0) }; 4] }; 64];

    #[inline(always)]
    pub(crate) fn cut_at(empties: u8, at: Option<usize>) {
        let _ = (empties, at);
        #[cfg(feature = "node-accounting")]
        {
            let slot = at.map_or(3, |i| i.min(2));
            CUT_AT[(empties as usize) & 63][slot].fetch_add(1, Relaxed);
        }
    }

    #[inline(always)]
    pub(crate) fn sorted(n: u64) {
        let _ = n;
        #[cfg(feature = "node-accounting")]
        SORTED.fetch_add(n, Relaxed);
    }

    #[inline(always)]
    pub(crate) fn etc(n: u64) {
        let _ = n;
        #[cfg(feature = "node-accounting")]
        ETC.fetch_add(n, Relaxed);
    }

    #[inline(always)]
    pub(crate) fn lookahead() {
        #[cfg(feature = "node-accounting")]
        LOOKAHEAD.fetch_add(1, Relaxed);
    }

    pub fn totals() -> (u64, u64, u64) {
        #[cfg(feature = "node-accounting")]
        {
            (
                SORTED.load(Relaxed),
                ETC.load(Relaxed),
                LOOKAHEAD.load(Relaxed),
            )
        }
        #[cfg(not(feature = "node-accounting"))]
        {
            (0, 0, 0)
        }
    }

    pub const ENABLED: bool = cfg!(feature = "node-accounting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    #[test]
    fn shallow_subsets_cover_the_same_moves_as_the_scan() {
        let mut seed = 0x1234_5678_9abc_def1u64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let mut empty = 0u64;
            while empty.count_ones() < 6 {
                empty |= 1u64 << (rand() % 64);
            }
            let player = !empty & rand();
            let opponent = !empty & !player;
            if player == 0 || opponent == 0 {
                continue;
            }

            let mut scanned = 0u64;
            let mut e = empty;
            while e != 0 {
                let sq = e.trailing_zeros() as u8;
                e &= e - 1;
                if bitboard::flippable(player, opponent, 1u64 << sq) != 0 {
                    scanned |= 1u64 << sq;
                }
            }

            let moves = bitboard::mobility(player, opponent, empty);
            assert_eq!(moves, scanned, "mobility disagrees with the flip scan");

            for parity in 0..16u8 {
                const CORNERS: u64 = 0x8100_0000_0000_0081;
                let odd = PARITY_ODD_MASK[parity as usize];
                let (om, em) = (moves & odd, moves & !odd);
                let subsets = [om & CORNERS, om & !CORNERS, em & CORNERS, em & !CORNERS];
                let mut union = 0u64;
                let mut total = 0u32;
                for m in subsets {
                    union |= m;
                    total += m.count_ones();
                }
                assert_eq!(union, moves, "subsets lose a move");
                assert_eq!(total, moves.count_ones(), "subsets repeat a move");
            }
        }
    }

    #[test]
    fn sort_ladder_steps_one_ply_at_a_time() {
        for (e, &d) in SORT_DEPTH_LADDER.iter().enumerate() {
            let e = e as u8;
            let want = if e >= DEEP3_ORDER_EMPTIES {
                3
            } else if e >= DEEP2_ORDER_EMPTIES {
                2
            } else if e >= DEEP_ORDER_EMPTIES {
                1
            } else {
                0
            };
            assert_eq!(d, want, "sort depth at {e} empties");
        }
        const _: () = assert!(DEEP_ORDER_EMPTIES < DEEP2_ORDER_EMPTIES);
        const _: () = assert!(DEEP2_ORDER_EMPTIES < DEEP3_ORDER_EMPTIES);
    }

    fn corner_stability_loop(bb: u64) -> i32 {
        use crate::pattern::sq::*;
        let has = |s: u8| bb & (1u64 << s) != 0;
        let mut count = 0;
        for (corner, edge1, edge2) in [(A1, B1, A2), (A8, B8, A7), (H1, G1, H2), (H8, G8, H7)] {
            if has(corner) {
                count += 1;
                if has(edge1) {
                    count += 1;
                }
                if has(edge2) {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn corner_stability_matches_the_loop() {
        const RELEVANT: [u8; 12] = [0, 1, 8, 7, 6, 15, 56, 57, 48, 63, 62, 55];
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let relevant_mask: u64 = RELEVANT.iter().fold(0, |m, s| m | (1u64 << s));
        for pattern in 0..4096u32 {
            let mut bits = 0u64;
            for (i, sq) in RELEVANT.iter().enumerate() {
                if pattern >> i & 1 != 0 {
                    bits |= 1u64 << sq;
                }
            }
            for _ in 0..64 {
                let bb = bits | (next() & !relevant_mask);
                assert_eq!(
                    corner_stability_bb(bb),
                    corner_stability_loop(bb),
                    "bb={bb:#018x}"
                );
            }
        }
    }

    fn negamax(board: &Board, passed: bool) -> i32 {
        let moves = board.movable();
        if moves == 0 {
            if passed {
                return final_score(board);
            }
            let mut b = *board;
            b.pass();
            return -negamax(&b, true);
        }
        let mut best = -VALUE_INF;
        let mut m = moves;
        while m != 0 {
            let bit = m.trailing_zeros();
            m &= m - 1;
            let mut child = *board;
            child.make_move_unchecked(Position::from_index(bit).unwrap());
            let v = -negamax(&child, false);
            if v > best {
                best = v;
            }
        }
        best
    }

    fn position_with_empties(empties: u8) -> Board {
        let mut b = Board::new();
        let mut ply = 0usize;
        while b.empty_count() > empties {
            let moves = b.movable();
            if moves == 0 {
                let mut p = b;
                p.pass();
                if p.movable() == 0 {
                    break; // game ended early
                }
                b = p;
                continue;
            }
            let bit = if ply.is_multiple_of(2) {
                moves.trailing_zeros()
            } else {
                63 - moves.leading_zeros()
            };
            b.make_move_unchecked(Position::from_index(bit).unwrap());
            ply += 1;
        }
        b
    }

    #[test]
    fn test_solver_matches_negamax_shallow() {
        for empties in [4u8, 6, 8] {
            let board = position_with_empties(empties);
            if board.is_game_over() {
                continue;
            }
            let expected = negamax(&board, false);
            let mut solver = Solver::new(14);
            let result = solver.solve(EndSolverMode::Perfect, &board);
            assert_eq!(
                result.value,
                expected,
                "perfect solve mismatch at {} empties",
                board.empty_count()
            );
        }
    }

    #[test]
    fn test_solver_wld_sign_matches_perfect() {
        let board = position_with_empties(10);
        let mut solver = Solver::new(14);
        let perfect = solver.solve(EndSolverMode::Perfect, &board);
        let wld = solver.solve(EndSolverMode::WinLossDraw, &board);
        assert_eq!(
            wld.value.signum(),
            perfect.value.signum(),
            "WLD sign must match the exact result"
        );
    }

    #[test]
    fn test_solver_best_move_is_legal() {
        let board = position_with_empties(12);
        let mut solver = Solver::new(14);
        let result = solver.solve(EndSolverMode::Perfect, &board);
        let best = result.best_move.expect("a legal move exists");
        assert!(
            board.movable() & best.to_bit() != 0,
            "solver's best move must be legal"
        );
        assert!(result.nodes > 0);
    }

    #[test]
    fn test_solver_pass_position() {
        let mut b = Board::new();
        b.black = 1u64 << 0; // A1
        b.white = 1u64 << 8; // B1
        b.empty_count = 62;
        b.player = crate::color::Color::White; // White has no move
        let mut solver = Solver::new(10);
        let result = solver.solve(EndSolverMode::Perfect, &b);
        assert_eq!(result.best_move, None, "pass position returns no move");
    }

    #[test]
    fn test_last1_exact() {
        let board = position_with_empties(1);
        if board.empty_count() == 1 && !board.is_game_over() {
            let expected = negamax(&board, false);
            let mut solver = Solver::new(10);
            let result = solver.solve(EndSolverMode::Perfect, &board);
            assert_eq!(result.value, expected, "last1 exact score");
        }
    }

    #[test]
    fn test_neighbour_table() {
        assert_eq!(neighbour_bit(0), (1u64 << 1) | (1u64 << 8) | (1u64 << 9));
        assert_eq!(neighbour_bit(36).count_ones(), 8);
    }

    #[test]
    fn test_quadrant_parity() {
        let b = Board::new();
        assert_eq!(parity_of(&b), 0b1111);
    }
}

#[cfg(test)]
mod quadrant_id_tests {
    use super::quadrant_id;

    #[test]
    fn matches_the_file_rank_split() {
        for sq in 0u8..64 {
            let (file, rank) = (sq / 8, sq % 8);
            let want = match (file < 4, rank < 4) {
                (true, true) => 1,
                (false, true) => 2,
                (true, false) => 4,
                (false, false) => 8,
            };
            assert_eq!(quadrant_id(sq), want, "square {sq}");
        }
    }
}
