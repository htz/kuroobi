//! Endgame solver: PVS (principal variation search) with a Zobrist-keyed
//! transposition table, move ordering, and specialized last-4/3/2/1 fast
//! paths.
//!
//! Scores are disc differences from the current player's perspective, with
//! the empty-square bonus applied to the winner (Board::score semantics).

use crate::bitboard;
use crate::board::Board;
use crate::evaluator::Evaluator;
use crate::pattern_index::{PatternIndexer, PatternIndices};
use crate::position::Position;
use crate::zobrist;

/// Search depth thresholds (empties remaining) for switching strategies.
const PVS_LIMIT: u8 = 12;
const MOVE_ORDERING_LIMIT: u8 = 7;
/// Below this many empties the ordered search goes to a cache-resident
/// table instead of the main one. The main table holds 2^26 entries of 24
/// bytes, so a probe there is a guaranteed DRAM round trip; at these
/// depths the tree is wide enough that the latency, not the hit rate,
/// decides.
const MID_TT_EMPTIES: u8 = 10;
/// Entry count of that table, as a power of two. 2^19 * 24 B = 12.6 MB.
const MID_TT_BITS: u32 = 19;
/// From this many empties upward, an evaluator (when provided) orders
/// moves instead of the static heuristic.
const EVAL_ORDER_EMPTIES: u8 = 14;
/// From this many empties upward, ordering refines the evaluation with a
/// one-ply lookahead (max over the opponent's replies).
const DEEP_ORDER_EMPTIES: u8 = 16;
/// Terminal score with the empty-square bonus awarded to the winner
/// (FFO convention; also what the game pipeline records). The old
/// plain disc difference disagreed whenever a game ended with empties left
/// — last1 already awarded its single empty, the general terminals did not.
#[inline]
fn final_score(board: &Board) -> i32 {
    let diff = board.score();
    let empties = board.empty_count() as i32;
    match diff.cmp(&0) {
        std::cmp::Ordering::Greater => diff + empties,
        std::cmp::Ordering::Less => diff - empties,
        std::cmp::Ordering::Equal => 0,
    }
}

/// Wipeout: a side with no discs can never move again, so the game is
/// over the moment it happens (worth handling explicitly; without it
/// the search still terminates but only after mobility churn).
#[inline]
fn wipeout_score(board: &Board) -> Option<i32> {
    if board.opponent_bb() == 0 {
        return Some(64);
    }
    if board.player_bb() == 0 {
        return Some(-64);
    }
    None
}

/// Stability-cut alpha thresholds, indexed by empty count:
/// the stability bound is only worth computing once the window has come
/// this far. Below it the fixed point almost never cuts, and the popcount
/// gate alone lets through eight wasted computations per cut. 99 disables
/// the test outright (too many empties for any disc to be stable yet).
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

/// Stability cutoff precondition: the bound 64 - 2*S can only cut when the
/// opponent has at least ceil((64-alpha)/2) stable discs, so their total
/// disc count (cheap popcount) must reach that first.
#[inline]
fn stability_cut(board: &Board, alpha: i32, beta: i32) -> Option<i32> {
    let threshold = STABILITY_THRESHOLD[board.empty_count() as usize];
    // Upper bound via the opponent's stable discs (fail low)
    let need = (64 - alpha + 1) / 2;
    if alpha >= threshold && need <= 32 && (board.opponent_bb().count_ones() as i32) >= need {
        let bound =
            64 - 2 * crate::stability::stable_count(board.opponent_bb(), board.player_bb()) as i32;
        if bound <= alpha {
            return Some(bound);
        }
    }
    // Lower bound via our own stable discs (fail high). Nearly free on
    // balanced positions thanks to the popcount gate, and it collapses
    // one-sided positions (e.g. FFO#59) that otherwise explode.
    // Mirror of the guard above, with the threshold test flipped to the
    // beta side.
    let need = (64 + beta + 1) / 2;
    if beta <= -threshold && need <= 32 && (board.player_bb().count_ones() as i32) >= need {
        let bound =
            2 * crate::stability::stable_count(board.player_bb(), board.opponent_bb()) as i32 - 64;
        if bound >= beta {
            return Some(bound);
        }
    }
    None
}
/// From this many empties upward, ordering uses a two-ply lookahead.
const DEEP2_ORDER_EMPTIES: u8 = 19;
/// Enhanced transposition cutoff: from this many empties upward, probe
/// every child's hash entry before searching — a proven fail-high there
/// cuts this node without any search.
const ETC_EMPTIES: u8 = 12;
/// Ordering weight of one opponent reply, in eighths of a disc (the same
/// scale the evaluation term uses).
const MOBILITY_ORDER_WEIGHT: i32 = 12;
/// Splitting only pays off when each sibling subtree is substantial: below
/// this many empties the hand-off costs more than the subtree.
const PARALLEL_MIN_EMPTIES: u8 = 16;
/// Helpers a single node may recruit. Capping it
/// keeps one wide node from starving the rest of the tree.
const SPLIT_MAX_SLAVES: usize = 1;

/// Cutoff signal for a split node, chained to its enclosing splits.
///
/// When one sibling proves a cutoff, every thread still working inside that
/// node's subtree is doing work that can no longer matter. Setting the flag
/// lets them unwind instead of running their subtree to completion. The
/// parent link means an outer cutoff also stops searches started by nested
/// splits.
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
        self.flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// True if this split, or any enclosing one, has been cut off. The chain
    /// is as deep as splits are nested, which is a handful of links.
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

/// Nodes between abort checks. Walking the chain on every node would cost
/// more than the work it saves; a few hundred nodes is a short enough leash.
const ABORT_CHECK_INTERVAL: u64 = 512;

/// Sentinel returned by a search that unwound early. It is never stored in
/// the table and never compared as a real score.
const ABORTED: i32 = i32::MIN + 1;

/// Pool of spare search threads, shared by every node that wants to split.
/// Nodes reserve helpers before spawning and hand them back afterwards, so
/// the total number of live search threads never exceeds the configured
/// budget no matter how deeply splits nest.
struct ThreadBudget {
    free: std::sync::atomic::AtomicUsize,
}

impl ThreadBudget {
    fn new(extra_threads: usize) -> ThreadBudget {
        ThreadBudget {
            free: std::sync::atomic::AtomicUsize::new(extra_threads),
        }
    }

    /// Take up to `want` helpers; returns how many were actually granted.
    fn reserve(&self, want: usize) -> usize {
        use std::sync::atomic::Ordering;
        let mut cur = self.free.load(Ordering::Relaxed);
        loop {
            let take = cur.min(want);
            if take == 0 {
                return 0;
            }
            match self.free.compare_exchange_weak(
                cur,
                cur - take,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return take,
                Err(actual) => cur = actual,
            }
        }
    }

    fn release(&self, n: usize) {
        self.free
            .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }

}
/// Slack below the node's alpha for the ordering lookahead's window.
const SORT_ALPHA_DELTA: i32 = 12;
/// Ordering weight of one stable edge disc, in eighths of a disc.
const EDGE_STABILITY_ORDER_WEIGHT: i32 = 1;
/// Half-width of the first aspiration window around the warm-up score.
const ASPIRATION_WIDTH: i32 = 6;
/// Half-width used by the warm-up passes, whose centre is only an
/// evaluation estimate rather than a searched score.
const WARM_ASPIRATION_WIDTH: i32 = 6;
/// Depth of the evaluation search that centres the first warm-up window.
const ESTIMATE_DEPTH: u8 = 6;
/// Warm-up passes only prune at this many empties or more.
const SELECTIVE_MIN_EMPTIES: u8 = 14;
/// Depth of the evaluation probe used by a warm-up pass.
const SELECTIVE_PROBE_DEPTH: u8 = 4;
/// Roots this deep run the warm-up ladder before the exact pass. Below 20
/// empties the exact search is cheap enough that the pass cannot pay for
/// itself (measured on FFO1-19: +6% nodes and +24% time at 16, +14%/+62%
/// at 14), while from 20 up it pays for itself several times over.
const SELECTIVE_PASS_MIN_EMPTIES: u8 = 24;
/// Confidence levels (standard deviations) of the warm-up passes, from
/// most selective to least. Each pass is aspirated around the previous
/// pass's score, so the estimate handed to the exact search converges —
/// an estimate that is off by even two discs makes the exact search pay
/// for a failed window, which is the dominant cost on hard positions.
const SELECTIVE_LADDER: [f32; 2] = [1.1, 1.8];

/// Plies of ordering lookahead by empty count, stepped one ply at a time
/// as the position opens up.
const SORT_DEPTH_LADDER: [u8; 64] = {
    let mut t = [0u8; 64];
    let mut e = 0usize;
    while e < 64 {
        t[e] = if e >= 28 {
            3
        } else if e >= DEEP2_ORDER_EMPTIES as usize {
            2
        } else if e >= DEEP_ORDER_EMPTIES as usize {
            1
        } else {
            0
        };
        e += 1;
    }
    t
};

/// Squares adjacent to each square (used to skip moves that cannot flip:
/// a legal move must touch at least one opponent disc).
/// Index = file-major square; built at first use.
fn neighbour_bit(sq: u8) -> u64 {
    // Compute the 8-neighbourhood mask in file-major layout.
    // (Small enough to compute on the fly; the compiler folds it well.)
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

/// Precomputed neighbour masks for all 64 squares.
struct NeighbourTable([u64; 64]);

impl NeighbourTable {
    fn new() -> Self {
        let mut t = [0u64; 64];
        for (sq, slot) in t.iter_mut().enumerate() {
            *slot = neighbour_bit(sq as u8);
        }
        NeighbourTable(t)
    }

    #[inline]
    fn get(&self, sq: u8) -> u64 {
        self.0[sq as usize]
    }
}

// ---------------------------------------------------------------------------
// Quadrant parity (odd/even number of empties per board quadrant).
// Endgame heuristic: prefer moves in quadrants with an odd number of empties.
// ---------------------------------------------------------------------------

/// `final_score` on the raw bitboards the leaf routines carry: the side to
/// move stays implicit in the argument order rather than in a colour field,
/// so the last plies never touch a `Board` (and never pay for its
/// runtime-indexed `player_bb()`).
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

/// Quadrant parity of the empty squares, from the occupancy.
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
    let file = sq / 8;
    let rank = sq % 8;
    match (file < 4, rank < 4) {
        (true, true) => 1,
        (false, true) => 2,
        (true, false) => 4,
        (false, false) => 8,
    }
}

/// Board mask of each quadrant (file-major), indexed by quadrant_id bit.
const QUADRANT_MASKS: [(u8, u64); 4] = [
    (1, 0x0000_0000_0F0F_0F0F), // files 0-3, ranks 0-3
    (2, 0x0F0F_0F0F_0000_0000), // files 4-7, ranks 0-3
    (4, 0x0000_0000_F0F0_F0F0), // files 0-3, ranks 4-7
    (8, 0xF0F0_F0F0_0000_0000), // files 4-7, ranks 4-7
];

/// Squares lying in quadrants with an odd number of empties, straight from
/// the empty squares: a quadrant is odd when its popcount is. An alternative
/// is carrying the parity down the search incrementally (one XOR per move);
/// four popcounts get there without the per-square walk this replaces, and
/// with a parity bitmask in hand the union of its quadrant masks is a single
/// table load.
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


// ---------------------------------------------------------------------------
// Transposition table
// ---------------------------------------------------------------------------

const VALUE_INF: i32 = i32::MAX / 2;

/// Compact 24-byte entry: a 2-way bucket fits in one cache line, so a probe
/// costs a single memory access. `(black, white, player)` fully identifies
/// the position — no separate hash key is needed (the hash only picks the
/// bucket). Scores are stored as i8 with MIN/MAX as -/+infinity sentinels.
#[derive(Clone, Copy)]
struct HashEntry {
    black: u64,
    white: u64,
    lower8: i8,
    upper8: i8,
    depth: u8,
    /// Square index of the best move; 255 = none.
    best8: u8,
    /// Bit 0: used. Bit 1: player is White.
    flags: u8,
    /// Which warm-up generation wrote this entry. Anything older than the
    /// table's current generation counts as a seed, which turns demoting a
    /// pass's results into a single increment.
    date: u8,
    _pad: [u8; 2],
}

impl HashEntry {
    const EMPTY: HashEntry = HashEntry {
        black: 0,
        white: 0,
        lower8: i8::MIN,
        upper8: i8::MAX,
        depth: 0,
        best8: 255,
        flags: 0,
        date: 0,
        _pad: [0; 2],
    };

    #[inline]
    fn used(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Seed entries come from the evaluation-guided pre-search: their move
    /// is a good ordering hint but their bounds are heuristic and must
    /// never cut the exact search.
    #[inline]
    fn is_seed(&self, now: u8) -> bool {
        self.flags & 4 != 0 || self.date != now
    }

    #[inline]
    fn lower(&self) -> i32 {
        if self.lower8 == i8::MIN { -VALUE_INF } else { self.lower8 as i32 }
    }

    #[inline]
    fn upper(&self) -> i32 {
        if self.upper8 == i8::MAX { VALUE_INF } else { self.upper8 as i32 }
    }

    #[inline]
    fn best(&self) -> Option<Position> {
        (self.best8 < 64).then_some(Position(self.best8))
    }

    #[inline]
    fn matches(&self, board: &Board) -> bool {
        self.used()
            && self.black == board.black
            && self.white == board.white
            && (self.flags >> 1) & 1 == board.player as u8
    }
}

/// A test-and-set spin lock. The transposition table is shared by every
/// search thread and its entries (24 bytes) are far too wide for a single
/// atomic write, so writes are serialized. Locks are *striped*: a small
/// array of them is indexed by hash bits, so threads working in different
/// regions of the table almost never collide.
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

/// Releases the lock on drop, so an unwind cannot leave the table wedged.
struct SpinGuard<'a>(&'a SpinLock);

impl Drop for SpinGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        self.0 .0.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Number of stripes. Generous relative to any core count so that lock
/// collisions between threads are rare even under heavy write traffic.
const HASH_LOCK_STRIPES: usize = 4096;

struct HashTable {
    mask: u64,
    /// Current generation; see `HashEntry::date`.
    date: std::sync::atomic::AtomicU8,
    /// Set while more than one search thread is live. When it is clear the
    /// table is private to one thread and every access can skip its lock.
    shared: std::sync::atomic::AtomicBool,
    /// Interior mutability so search threads can share one table. Every
    /// access goes through `stripe()`; see `get`/`update` for the protocol.
    entries: std::cell::UnsafeCell<Vec<HashEntry>>,
    locks: Vec<SpinLock>,
}

// SAFETY: all interior mutation happens while holding the entry's stripe
// lock, and `HashEntry` is a plain `Copy` value with no interior pointers.
unsafe impl Sync for HashTable {}

impl HashTable {
    /// 2-way associative: same total entry count, half as many buckets.
    /// A deep entry no longer gets evicted just because a shallow position
    /// happens to share its bucket.
    fn new(bit_size: u32) -> HashTable {
        let size = 1usize << bit_size;
        let mut locks = Vec::with_capacity(HASH_LOCK_STRIPES);
        locks.resize_with(HASH_LOCK_STRIPES, SpinLock::new);
        HashTable {
            mask: ((size >> 1) - 1) as u64,
            date: std::sync::atomic::AtomicU8::new(1),
            shared: std::sync::atomic::AtomicBool::new(false),
            entries: std::cell::UnsafeCell::new(vec![HashEntry::EMPTY; size]),
            locks,
        }
    }

    #[inline]
    fn date(&self) -> u8 {
        self.date.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Declare whether search threads other than the owner are running.
    fn set_shared(&self, shared: bool) {
        self.shared
            .store(shared, std::sync::atomic::Ordering::Relaxed);
    }

    #[inline]
    fn is_shared(&self) -> bool {
        self.shared.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Stripe guarding the bucket this hash maps to.
    #[inline]
    fn stripe(&self, hash: u64) -> &SpinLock {
        &self.locks[(hash as usize) & (HASH_LOCK_STRIPES - 1)]
    }

    /// Exclusive view of the entries. Callers must hold the relevant stripe
    /// lock, or have exclusive access to the table (`&mut self`).
    #[inline]
    #[allow(clippy::mut_from_ref)]
    unsafe fn slots(&self) -> &mut Vec<HashEntry> {
        &mut *self.entries.get()
    }

    /// Pull an entry's cache line in before it is needed, so the miss
    /// overlaps the work in between. Ours covers the ETC sweep, which
    /// touches every child's entry a moment after the moves are generated.
    #[inline(always)]
    fn prefetch(&self, hash: u64) {
        let base = ((hash & self.mask) as usize) << 1;
        // SAFETY: `base` indexes an allocated slot (mask is len/2 - 1), and
        // a prefetch has no effect beyond the cache.
        unsafe {
            let p = (*self.entries.get()).as_ptr().add(base);
            // `core::arch::aarch64::_prefetch` is still unstable, so emit the
            // hint directly. `pldl1keep` = prefetch for read into L1, keep.
            #[cfg(target_arch = "aarch64")]
            std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) p, options(nostack, preserves_flags));
            #[cfg(not(target_arch = "aarch64"))]
            let _ = p;
        }
    }

    fn clear(&mut self) {
        self.entries.get_mut().fill(HashEntry::EMPTY);
    }

    /// Returns a *copy* of the matching entry. Copying (24 bytes) rather
    /// than borrowing keeps probes valid once the table is shared between
    /// search threads, where the slot may be overwritten at any moment.
    /// Probe. The position test runs without taking the lock — probes miss
    /// far more often than they hit, and a torn read can only fabricate a
    /// *mismatch*, never a false hit, because the match is re-checked under
    /// the lock before the entry is copied out.
    #[inline]
    fn get(&self, board: &Board, hash: u64) -> Option<HashEntry> {
        // `mask` is `entries.len() / 2 - 1`, so `base + 1` is always the
        // second half of a valid bucket. Saying so removes a bounds check
        // from a path that is otherwise a single load: probing is 8.3% of a
        // solve, and the branch sits right in front of the memory access.
        let base = ((hash & self.mask) as usize) << 1;
        // SAFETY: with helpers running this read may observe a torn entry,
        // which the locked re-check below discards; no reference escapes.
        // The indices are in range by the invariant above.
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
            // Sole owner: nothing can be rewriting the slot.
            return Some(hit);
        }
        let _guard = self.stripe(hash).lock();
        // SAFETY: the stripe lock is held for the duration of the read.
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
    ) {
        let best8 = best.map_or(255, |p| p.index());
        let now = self.date();
        let base = ((hash & self.mask) as usize) << 1;
        let _guard = self.is_shared().then(|| self.stripe(hash).lock());
        // SAFETY: the stripe lock is held for the whole read-modify-write.
        let entries = unsafe { self.slots() };
        let slot = if entries[base].matches(board) {
            base
        } else if entries[base + 1].matches(board) {
            base + 1
        } else {
            // Evict the shallower slot of the pair
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
                    flags: 1 | ((board.player as u8) << 1),
                    date: self.date(),
                    _pad: [0; 2],
                };
            }
            return;
        };
        let entry = &mut entries[slot];
        if entry.is_seed(now) {
            // Heuristic bounds from the pre-search: replace, don't tighten
            entry.lower8 = if value > alpha { value as i8 } else { i8::MIN };
            entry.upper8 = if value < beta { value as i8 } else { i8::MAX };
            entry.depth = board.empty_count();
            entry.flags = 1 | ((board.player as u8) << 1);
            entry.best8 = best8;
            entry.date = now;
            return;
        }
        if value < beta && (value as i8) < entry.upper8 {
            entry.upper8 = value as i8;
        }
        if value > alpha && (value as i8) > entry.lower8 {
            entry.lower8 = value as i8;
        }
        entry.best8 = best8;
        entry.date = now;
    }

    /// Store a seed entry (evaluation-guided pre-search): best move plus
    /// heuristic bounds, flagged so the exact search only uses the move.
    /// Mark every live entry as a seed: its best move stays usable for
    /// ordering while its bounds stop being treated as proven.
    /// Mark every live entry as a seed: its best move stays usable for
    /// ordering while its bounds stop being treated as proven. Takes `&self`
    /// because workers only hold a shared borrow; callers must ensure no
    /// other thread is searching (warm-up rungs are separated by a join).
    fn demote_to_seed_shared(&self) {
        // Everything written so far belongs to the previous generation, so
        // opening a new one demotes all of it at once. The sweep this
        // replaces walked all 1.6 GB of the table, three times per solve,
        // evicting every level of cache on the way through.
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
        // SAFETY: the stripe lock is held for the whole read-modify-write.
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
            // Same position already solved exactly: keep it
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

// ---------------------------------------------------------------------------
// Solver
// ---------------------------------------------------------------------------

/// Search objective for [`Solver::solve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndSolverMode {
    /// Win/Loss/Draw: only the sign of the result is exact.
    WinLossDraw,
    /// Win vs not-win.
    WinDraw,
    /// Draw vs loss.
    DrawLoss,
    /// Exact final score.
    Perfect,
}

/// Result of an endgame search.
#[derive(Debug, Clone)]
pub struct EndSolverResult {
    /// Best move (None = current player must pass).
    pub best_move: Option<Position>,
    /// Score (disc difference with empty bonus) from the current player's view.
    pub value: i32,
    /// Empty squares at the root.
    pub empty: u8,
    /// Nodes visited.
    pub nodes: u64,
}

pub struct Solver {
    /// Shared between all search threads: the transposition table (its
    /// entries are guarded by striped locks) and the read-only neighbour
    /// masks.
    hash_table: HashTable,
    neighbours: NeighbourTable,
    /// Results of the last solve, copied back from the worker.
    nodes: u64,
    best: Option<Position>,
    /// Search threads to split the root across (1 = sequential).
    threads: usize,
}

/// One search thread's private state plus borrows of the shared tables.
/// Every search routine lives here, so spawning another thread is just
/// another `Worker` over the same `HashTable`.
struct Worker<'a> {
    tt: &'a HashTable,
    neighbours: &'a NeighbourTable,
    nodes: u64,
    best: Option<Position>,
    /// Score returned by the warm-up pass, used to centre the exact
    /// search's aspiration window (carrying the score between passes —
    /// that, not the warmed table, is the main benefit).
    warm_score: Option<i32>,
    /// When set, the search is *selective*: probable cutoffs are taken at
    /// this confidence (standard deviations of the evaluation's prediction
    /// error). Only the warm-up passes run this way; their table entries
    /// are demoted so the exact pass trusts their moves but not their bounds.
    selective_t: Option<f32>,
    /// Small dedicated table for the shallow region (5-6 empties), where
    /// transpositions are dense but entries would lose the depth-preferred
    /// replacement race in the main table. Private to the thread.
    shallow_table: HashTable,
    /// Same idea one band up, for the ordered search below
    /// `MID_TT_EMPTIES`. Those layers carry most of the tree, so every
    /// probe into the multi-gigabyte main table is a cold miss; a table
    /// that fits in cache trades a little hit rate for a lot of latency.
    mid_table: HashTable,
    /// Spare threads this search may recruit when splitting a node.
    budget: &'a ThreadBudget,
    /// Cutoff signal for the split this worker is searching under.
    abort: &'a AbortFlag<'a>,
    /// Node counter for throttling abort checks.
    abort_countdown: u64,
}

impl<'a> Worker<'a> {
    fn new(
        tt: &'a HashTable,
        neighbours: &'a NeighbourTable,
        budget: &'a ThreadBudget,
        abort: &'a AbortFlag<'a>,
    ) -> Worker<'a> {
        Worker {
            tt,
            neighbours,
            nodes: 0,
            best: None,
            warm_score: None,
            selective_t: None,
            shallow_table: HashTable::new(16),
            mid_table: HashTable::new(MID_TT_BITS),
            budget,
            abort,
            abort_countdown: ABORT_CHECK_INTERVAL,
        }
    }

    /// Transposition table serving a node with this many empties: the
    /// cache-resident one below `MID_TT_EMPTIES`, the main one above.
    #[inline(always)]
    fn table(&self, empties: u8) -> &HashTable {
        if empties < MID_TT_EMPTIES {
            &self.mid_table
        } else {
            self.tt
        }
    }

    /// Poll the cutoff signal, amortized over many nodes.
    #[inline]
    fn should_abort(&mut self) -> bool {
        self.abort_countdown -= 1;
        if self.abort_countdown != 0 {
            return false;
        }
        self.abort_countdown = ABORT_CHECK_INTERVAL;
        self.abort.aborted()
    }
}

impl Solver {
    /// `bit_size`: transposition table has 2^bit_size entries
    /// (each entry is ~48 bytes; 20 -> ~50 MB).
    pub fn new(bit_size: u32) -> Solver {
        Solver {
            hash_table: HashTable::new(bit_size),
            neighbours: NeighbourTable::new(),
            nodes: 0,
            best: None,
            threads: 1,
        }
    }

    /// Set how many threads the root may split its siblings across.
    /// The transposition table is shared; every other piece of search state
    /// is per-thread.
    pub fn set_threads(&mut self, threads: usize) {
        self.threads = threads.max(1);
    }


    /// Solve the endgame for `board` under the given mode.
    pub fn solve(&mut self, mode: EndSolverMode, board: &Board) -> EndSolverResult {
        self.solve_with_eval(mode, board, None)
    }


    /// Solve with an optional evaluator used purely for move ordering in
    /// the upper (many-empties) region of the tree. Ordering never changes
    /// the exact result — only the node count.
    pub fn solve_with_eval(
        &mut self,
        mode: EndSolverMode,
        board: &Board,
        ev: Option<&Evaluator>,
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

        // A single-threaded solve owns the table outright and can skip every
        // lock; with helpers in play the probes must synchronize.
        self.hash_table.set_shared(self.threads > 1);

        self.hash_table.clear();

        let value = {
            let budget = ThreadBudget::new(self.threads.saturating_sub(1));
            let root_abort = AbortFlag::root();
            let mut w = Worker::new(&self.hash_table, &self.neighbours, &budget, &root_abort);
            let mut b = *board;

            // Warm-up ladder (iterative selectivity): solve the same
            // full-depth endgame selectively first, leaving real full-depth
            // best moves in the table for the exact pass that follows.
            if let Some(e) = ev {
                if board.empty_count() >= SELECTIVE_PASS_MIN_EMPTIES {
                    let mut guess = w.estimate_score(board, Some(e));
                    for t in SELECTIVE_LADDER {
                        w.selective_t = Some(t);
                        let mut sb = *board;
                        let s = w.aspiration_width(&mut sb, guess, WARM_ASPIRATION_WIDTH, Some(e));
                        w.selective_t = None;
                        guess = s - (s & 1);
                        // Each rung must re-derive its own bounds: an entry
                        // stored by a more aggressive (less reliable) pass
                        // would otherwise be taken at face value, and the
                        // ladder would just re-confirm the first pass's error
                        // instead of converging. Best moves survive demotion.
                        w.tt.demote_to_seed_shared();
                        w.warm_score = Some(s - (s & 1));
                    }
                    w.tt.demote_to_seed_shared();
                }
            }

            let v = match mode {
                EndSolverMode::WinLossDraw => w.pvs_root(&mut b, -1, 1, ev),
                EndSolverMode::WinDraw => w.pvs_root(&mut b, 0, 1, ev),
                EndSolverMode::DrawLoss => w.pvs_root(&mut b, -1, 0, ev),
                EndSolverMode::Perfect => w.perfect(&mut b, ev),
            };
            self.nodes = w.nodes;
            self.best = w.best;
            v
        };

        EndSolverResult {
            best_move: self.best,
            value,
            empty: board.empty_count(),
            nodes: self.nodes,
        }
    }
}

impl Worker<'_> {
    /// Evaluation-guided iterative pre-search: before the
    /// exact passes, run shallow alpha-beta over the evaluator at
    /// increasing depths, storing best moves as flagged seed entries.
    /// The exact search then starts with near-complete move ordering.
    #[allow(clippy::too_many_arguments)]
    fn seed_search(
        &mut self,
        board: &Board,
        hash: u64,
        ev: &Evaluator,
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
            // Exactly solved already: the true value beats any estimate
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
                &child, zobrist::board_hash(board.opponent_bb(), board.player_bb()), ev, ix, indices, depth,
                -beta, -alpha, store,
            );
        }

        // Order children: TT/seed move first, then 0-ply evaluation
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
                child, *child_hash, ev, ix, indices, depth - 1, -beta, -alpha, store,
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
            self.tt.seed_update(board, hash, depth, lower8, upper8, best_move);
        }
        best_val
    }

    /// Exact score. With a warm-up score available the window is a narrow
    /// aspiration around it, widened only
    /// on the side that failed. Without one, fall back to the win/loss
    /// probe and a +-8 band.
    fn perfect(&mut self, board: &mut Board, ev: Option<&Evaluator>) -> i32 {
        if let Some(score) = self.warm_score {
            return self.aspiration(board, score, ev);
        }

        let mut val = self.pvs_root(board, -1, 1, ev);
        if val > 0 {
            let bound = val + 8;
            val = self.pvs_root(board, val, bound, ev);
            if val >= bound {
                val = self.pvs_root(board, val, 64, ev);
            }
        } else if val < 0 {
            let bound = val - 8;
            val = self.pvs_root(board, bound, val, ev);
            if val <= bound {
                val = self.pvs_root(board, -64, val, ev);
            }
        }
        val
    }

    /// Cheap evaluation-based guess of the final score, on the even grid
    /// that terminal scores live on.
    fn estimate_score(&mut self, board: &Board, ev: Option<&Evaluator>) -> i32 {
        let Some(e) = ev else { return 0 };
        let ix = e.indexer();
        let mut indices = ix.init(board.black, board.white);
        let hash = zobrist::board_hash(board.player_bb(), board.opponent_bb());
        let depth = ESTIMATE_DEPTH.min(board.empty_count());
        let v = self.seed_search(
            board, hash, e, ix, &mut indices, depth, f32::NEG_INFINITY, f32::INFINITY, false,
        );
        let rounded = (v / 2.0).round() as i32 * 2;
        rounded.clamp(-62, 62)
    }

    /// Aspiration around a prior score: search a narrow window, and on a
    /// failure re-centre on the failing bound and double that side only.
    fn aspiration(&mut self, board: &mut Board, score: i32, ev: Option<&Evaluator>) -> i32 {
        self.aspiration_width(board, score, ASPIRATION_WIDTH, ev)
    }

    /// Aspiration with an explicit starting half-width: the warm-up passes
    /// start from a rougher estimate than the exact pass does.
    fn aspiration_width(
        &mut self,
        board: &mut Board,
        mut score: i32,
        width: i32,
        ev: Option<&Evaluator>,
    ) -> i32 {
        let mut left = width;
        let mut right = width;
        for _ in 0..12 {
            let lo = (score - left).max(-64);
            let hi = (score + right).min(64);
            if lo >= hi || (lo <= -64 && hi >= 64) {
                break;
            }
            let val = self.pvs_root(board, lo, hi, ev);
            if val <= lo && lo > -64 {
                score = val;
                left = (left * 2).min(128);
                right = 0;
            } else if val >= hi && hi < 64 {
                score = val;
                left = 0;
                right = (right * 2).min(128);
            } else {
                return val;
            }
        }
        self.pvs_root(board, -64, 64, ev)
    }

    fn pvs_root(&mut self, board: &mut Board, alpha: i32, beta: i32, ev: Option<&Evaluator>) -> i32 {
        let mut lower = alpha;
        let upper = beta;
        // Root computes the hash from scratch once; children update it
        // incrementally (flipped discs + placed disc + player swap).
        let hash = zobrist::board_hash(board.player_bb(), board.opponent_bb());

        self.nodes += 1;

        let mut moves = MoveBuf::new();
        let tt_best = self.tt.get(board, hash).and_then(|e| e.best());
        self.scored_moves(board, tt_best, ev, &mut moves);
        if moves.is_empty() {
            return final_score(board);
        }
        moves.sort_by_key(|m| m.value);

        let mut best = moves[0].pos;
        let mut max;

        // First move: full window
        {
            let mut child = moves[0].child(board);
            max = -self.pvs(&mut child, moves[0].hash, -upper, -lower, false, ev);
            if max > lower {
                lower = max;
            }
        }

        // Young Brothers Wait: the eldest child above has proved a bound, so
        // the remaining siblings are independent enough to share out. Only
        // worth the hand-off when the subtrees are large.
        if moves.len() > 2 && board.empty_count() >= PARALLEL_MIN_EMPTIES && lower < upper {
            if let Some((val, bpos)) = self.split_siblings(board, &moves[1..], lower, upper, ev) {
                if val > max {
                    max = val;
                    best = bpos;
                }
                self.tt.update(board, hash, alpha, beta, max, Some(best));
                self.best = Some(best);
                return max;
            }
        }

        // Remaining moves: null-window probe, re-search on fail-high
        for m in &moves[1..] {
            if lower >= upper {
                break;
            }
            let mut child = m.child(board);
            let mut val = -self.pvs(&mut child, m.hash, -lower - 1, -lower, false, ev);
            if lower < val && val < upper {
                val = -self.pvs(&mut child, m.hash, -upper, -val, false, ev);
            }
            if val > max {
                max = val;
                best = m.pos;
                if max > lower {
                    lower = max;
                }
            }
        }

        self.tt
            .update(board, hash, alpha, beta, max, Some(best));
        self.best = Some(best);
        max
    }

    /// Search `siblings` in parallel, all threads sharing the one
    /// transposition table. Returns the best (value, move), or `None` if no
    /// helper threads were available and the caller should search normally.
    ///
    /// The alpha bound is shared and only ever rises, so a thread that reads
    /// a slightly stale value simply searches a wider window — correct, just
    /// marginally more work. Each helper keeps its own node counter and
    /// shallow table; results are folded back in at the join.
    fn split_siblings(
        &mut self,
        parent: &Board,
        siblings: &[ScoredMove],
        lower: i32,
        upper: i32,
        ev: Option<&Evaluator>,
    ) -> Option<(i32, Position)> {
        use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
        use std::sync::Mutex;

        let helpers = self
            .budget
            .reserve(SPLIT_MAX_SLAVES.min(siblings.len() - 1));
        if helpers == 0 {
            return None;
        }

        let next = AtomicUsize::new(0);
        let shared_lower = AtomicI32::new(lower);
        let best = Mutex::new((i32::MIN, siblings[0].pos));
        let tt = self.tt;
        let neighbours = self.neighbours;
        let budget = self.budget;
        let selective_t = self.selective_t;
        let extra_nodes = AtomicUsize::new(0);
        // Cutting off this node must also stop work already under way in the
        // siblings, so the helpers search under a flag chained to ours.
        let group = AbortFlag::child(self.abort);
        let group = &group;

        std::thread::scope(|scope| {
            for _ in 0..helpers {
                scope.spawn(|| {
                    let mut w = Worker::new(tt, neighbours, budget, group);
                    w.selective_t = selective_t;
                    w.run_siblings(parent, siblings, upper, ev, &next, &shared_lower, &best, group);
                    extra_nodes.fetch_add(w.nodes as usize, Ordering::Relaxed);
                });
            }
            // The splitting thread takes its share too, then joins at the
            // end of the scope.
            self.run_siblings(parent, siblings, upper, ev, &next, &shared_lower, &best, group);
        });

        self.budget.release(helpers);
        self.nodes += extra_nodes.load(Ordering::Relaxed) as u64;
        let out = *best.lock().unwrap();
        Some(out)
    }

    /// Claim siblings one at a time and search each with the current shared
    /// alpha, mirroring the sequential null-window-then-re-search logic.
    fn run_siblings(
        &mut self,
        parent: &Board,
        siblings: &[ScoredMove],
        upper: i32,
        ev: Option<&Evaluator>,
        next: &std::sync::atomic::AtomicUsize,
        shared_lower: &std::sync::atomic::AtomicI32,
        best: &std::sync::Mutex<(i32, Position)>,
        group: &AbortFlag<'_>,
    ) {
        use std::sync::atomic::Ordering;
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= siblings.len() {
                return;
            }
            let cur = shared_lower.load(Ordering::Relaxed);
            if cur >= upper || group.aborted() {
                return; // this node is already settled
            }
            let m = &siblings[i];
            let mut child = m.child(parent);
            let mut val = -self.pvs(&mut child, m.hash, -cur - 1, -cur, false, ev);
            if val == -ABORTED {
                return; // unwound; the value proves nothing
            }
            if cur < val && val < upper {
                val = -self.pvs(&mut child, m.hash, -upper, -val, false, ev);
                if val == -ABORTED {
                    return;
                }
            }
            let mut g = best.lock().unwrap();
            if val > g.0 {
                *g = (val, m.pos);
            }
            drop(g);
            shared_lower.fetch_max(val, Ordering::Relaxed);
            if val >= upper {
                // Proved the cutoff: wake the siblings still searching.
                group.abort();
                return;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pvs(&mut self, board: &mut Board, hash: u64, alpha: i32, beta: i32, passed: bool, ev: Option<&Evaluator>) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, board.empty_count());
        let mut lower = alpha;
        let mut upper = beta;

        self.nodes += 1;
        if self.should_abort() {
            return ABORTED;
        }

        if let Some(v) = wipeout_score(board) {
            return v;
        }

        // One probe serves both purposes, carried through the node: the
        // bounds below and the ordering move further
        // down used to cost two lookups of the same entry.
        let entry = {
            let _p = layer_profile::Scope::new(layer_profile::TT, board.empty_count());
            self.tt.get(board, hash)
        };
        if let Some(v) = entry {
            if !v.is_seed(self.tt.date()) {
                if v.lower() >= v.upper() {
                    return v.lower();
                }
                if upper > v.upper() {
                    upper = v.upper();
                    if upper <= lower {
                        return upper;
                    }
                }
                if lower < v.lower() {
                    lower = v.lower();
                    if lower >= upper {
                        return lower;
                    }
                }
            }
        }

        // Stability cutoff: the opponent's stable discs can never be
        // flipped, so our final score is at most 64 - 2*|opp stable|.
        {
            let _p = layer_profile::Scope::new(layer_profile::STAB, board.empty_count());
            if let Some(bound) = stability_cut(board, lower, upper) {
                return bound;
            }
        }

        // Warm-up (selective) pass: take probable cutoffs from a shallow
        // evaluation search instead of proving them.
        if let (Some(t), Some(e)) = (self.selective_t, ev) {
            if board.empty_count() >= SELECTIVE_MIN_EMPTIES && upper - lower <= 1 {
                if let Some(v) = self.selective_cut(board, hash, e, t, lower, upper) {
                    return v;
                }
            }
        }

        // Children are generated without their (expensive) ordering values:
        // a node whose transposition-table move already cuts must not pay
        // for pattern evaluations and lookaheads it never uses.
        let mut moves = MoveBuf::new();
        self.gen_moves(board, &mut moves);

        if moves.is_empty() {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.pvs(board, zobrist::board_hash(board.opponent_bb(), board.player_bb()), -upper, -lower, true, ev);
            board.pass();
            self.tt.update(board, hash, alpha, beta, val, None);
            return val;
        }

        // A move that wipes out the opponent ends the game at exactly +64,
        // which is the maximum possible score — so it *is* this node's value.
        if moves.iter().any(|m| m.child(board).player_bb() == 0) {
            self.tt.update(board, hash, alpha, beta, 64, None);
            return 64;
        }

        // Enhanced transposition cutoff: a child whose stored upper bound
        // already proves our value >= upper ends this node for free.
        if board.empty_count() >= ETC_EMPTIES {
            let _p = layer_profile::Scope::new(layer_profile::ETC, board.empty_count());
            // The comparison accounting counts each ETC probe as a node;
            // tallied locally so the instrumentation costs one atomic per
            // node rather than one per probe.
            let mut probed = 0u64;
            for m in moves.iter() {
                probed += 1;
                if let Some(e) = self.tt.get(&m.child(board), m.hash) {
                    if !e.is_seed(self.tt.date()) && -e.upper() >= upper {
                        node_accounting::etc(probed);
                        return -e.upper();
                    }
                }
            }
            node_accounting::etc(probed);
        }

        let mut max = i32::MIN;
        let mut best = None;

        // Stage 1: the transposition-table move, searched before any
        // ordering work is done. Most cut nodes end here.
        let tt_best = entry.and_then(|e| e.best());
        if let Some(bpos) = tt_best {
            if let Some(idx) = moves.iter().position(|m| m.pos == bpos) {
                let m = moves.swap_remove(idx);
                let mut child = m.child(board);
                max = self.descend(&mut child, m.hash, -upper, -lower, ev);
                best = Some(m.pos);
                if max > lower {
                    lower = max;
                }
                if lower >= upper {
                    self.tt.update(board, hash, alpha, beta, max, best);
                    return max;
                }
            }
        }

        // Stage 2: the rest, now ordered properly.
        self.score_moves(board, &mut moves, None, ev, lower);

        let mut next = 0usize;
        if max == i32::MIN {
            // No table move: the best-ordered child takes the full window.
            Self::select_next(&mut moves, 0);
            let m = moves[0];
            next = 1;
            let mut child = m.child(board);
            max = self.descend(&mut child, m.hash, -upper, -lower, ev);
            if max == ABORTED {
                return ABORTED;
            }
            best = Some(m.pos);
            if max > lower {
                lower = max;
            }
        }

        // Young Brothers Wait: an elder sibling has proved a bound, so the
        // rest are independent enough to share out. Splitting at any node
        // deep enough, not only at the root, is what
        // keeps every core busy when one subtree dominates.
        if moves.len() - next > 1 && board.empty_count() >= PARALLEL_MIN_EMPTIES && lower < upper {
            // Helpers take the siblings in order, so this path pays for the
            // full sort that the sequential path avoids.
            moves[next..].sort_by_key(|m| m.value);
            if let Some((val, bpos)) = self.split_siblings(board, &moves[next..], lower, upper, ev) {
                if val > max {
                    max = val;
                    best = Some(bpos);
                }
                self.tt.update(board, hash, alpha, beta, max, best);
                return max;
            }
        }

        // Remaining moves: null-window probe, re-search on fail-high
        while next < moves.len() {
            if lower >= upper {
                break;
            }
            Self::select_next(&mut moves, next);
            let m = moves[next];
            next += 1;
            let mut child = m.child(board);
            let val = self.descend_null_window(&mut child, m.hash, lower, upper, ev);
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

        // A truncated search proves nothing, so its bound must never reach
        // the table — a wrong bound there would corrupt later searches.
        self.tt.update(board, hash, alpha, beta, max, best);
        max
    }

    /// Full-window recursive descent picking the right strategy by depth.
    #[inline]
    fn descend(&mut self, child: &mut Board, hash: u64, alpha: i32, beta: i32, ev: Option<&Evaluator>) -> i32 {
        if child.empty_count() >= PVS_LIMIT {
            let v = self.pvs(child, hash, alpha, beta, false, ev);
            // The child unwound; keep the sentinel recognizable after the
            // usual negation.
            if v == ABORTED {
                return ABORTED;
            }
            -v
        } else if child.empty_count() >= MOVE_ORDERING_LIMIT {
            -self.alpha_beta_ordered(child, hash, alpha, beta, false, ev)
        } else {
            let parity = parity_of(child);
            -self.alpha_beta(child, hash, alpha, beta, false, parity)
        }
    }

    /// Null-window probe then re-search, at the strategy for this depth.
    #[inline]
    fn descend_null_window(&mut self, child: &mut Board, hash: u64, lower: i32, upper: i32, ev: Option<&Evaluator>) -> i32 {
        let mut val = self.descend(child, hash, -lower - 1, -lower, ev);
        if val == ABORTED {
            return ABORTED;
        }
        if lower < val && val < upper {
            val = self.descend(child, hash, -upper, -val, ev);
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
        ev: Option<&Evaluator>,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, board.empty_count());
        let mut alpha = alpha;
        let mut beta = beta;

        self.nodes += 1;

        if let Some(v) = wipeout_score(board) {
            return v;
        }

        // Issue the table read early but do not wait on it: the order is
        // prefetch -> stability cutoff -> table probe,
        // so the miss latency is covered by the stability computation
        // instead of stalling the node. The parent prefetched this line
        // during its own move generation, but for every child except the
        // first that prefetch is long evicted by the sibling subtrees.
        self.table(board.empty_count()).prefetch(hash);

        {
            let _p = layer_profile::Scope::new(layer_profile::STAB, board.empty_count());
            if let Some(bound) = stability_cut(board, alpha, beta) {
                return bound;
            }
        }

        // Single probe, reused for the ordering move below (see `pvs`).
        let entry = {
            let _p = layer_profile::Scope::new(layer_profile::TT, board.empty_count());
            self.table(board.empty_count()).get(board, hash)
        };
        let mut narrowed = false;
        if let Some(v) = entry {
            if !v.is_seed(self.table(board.empty_count()).date()) {
                if v.lower() >= v.upper() {
                    return v.lower();
                }
                if beta > v.upper() {
                    beta = v.upper();
                    narrowed = true;
                    if beta <= alpha {
                        return beta;
                    }
                }
                if alpha < v.lower() {
                    alpha = v.lower();
                    narrowed = true;
                    if alpha >= beta {
                        return alpha;
                    }
                }
            }
        }

        // The window the table just handed us is tighter than the one the
        // stability cutoff saw, and its guards are threshold tests on
        // alpha/beta — so a cut that was out of reach a moment ago may be
        // available now. Retrying only on an actual narrowing keeps this off
        // the common path while preserving every cutoff the old ordering had.
        if narrowed {
            let _p = layer_profile::Scope::new(layer_profile::STAB, board.empty_count());
            if let Some(bound) = stability_cut(board, alpha, beta) {
                return bound;
            }
        }

        let orig_alpha = alpha;
        let mut moves = MoveBuf::new();
        self.gen_moves(board, &mut moves);

        if moves.is_empty() {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.alpha_beta_ordered(
                board,
                zobrist::board_hash(board.opponent_bb(), board.player_bb()),
                -beta,
                -alpha,
                true,
                ev,
            );
            board.pass();
            self.table(board.empty_count())
                .update(board, hash, orig_alpha, beta, val, None);
            return val;
        }

        // A forced move has nothing to order and nothing worth remembering:
        // re-deriving it costs one move generation, far less than the entry
        // it would evict, so skip the evaluation and the store.
        if moves.len() == 1 {
            let m = moves[0];
            let mut child = m.child(board);
            return if child.empty_count() >= MOVE_ORDERING_LIMIT {
                -self.alpha_beta_ordered(&mut child, m.hash, -beta, -alpha, false, ev)
            } else {
                {
                    let cp = parity_of(&child);
                    -self.alpha_beta(&mut child, m.hash, -beta, -alpha, false, cp)
                }
            };
        }

        self.score_moves(board, &mut moves, entry.and_then(|e| e.best()), ev, i32::MIN / 2);

        let mut best = None;
        for i in 0..moves.len() {
            Self::select_next(&mut moves, i);
            let m = moves[i];
            let mut child = m.child(board);
            let val = if child.empty_count() >= MOVE_ORDERING_LIMIT {
                -self.alpha_beta_ordered(&mut child, m.hash, -beta, -alpha, false, ev)
            } else {
                {
                    let cp = parity_of(&child);
                    -self.alpha_beta(&mut child, m.hash, -beta, -alpha, false, cp)
                }
            };
            if val > alpha {
                alpha = val;
            }
            if alpha >= beta {
                best = Some(m.pos);
                break;
            }
        }

        self.table(board.empty_count())
            .update(board, hash, orig_alpha, beta, alpha, best);
        alpha
    }

    /// Plain alpha-beta over the empty list, with the last-4 fast path and
    /// a dedicated shallow transposition table (5-6 empties).
    fn alpha_beta(
        &mut self,
        board: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, board.empty_count());
        self.nodes += 1;

        if let Some(v) = wipeout_score(board) {
            return v;
        }

        {
            let _p = layer_profile::Scope::new(layer_profile::STAB, board.empty_count());
            if let Some(bound) = stability_cut(board, alpha, beta) {
                return bound;
            }
        }

        if board.empty_count() == 4 {
            let mut e = board.empty();
            let p1 = e.trailing_zeros() as u8;
            e &= e - 1;
            let p2 = e.trailing_zeros() as u8;
            e &= e - 1;
            let p3 = e.trailing_zeros() as u8;
            e &= e - 1;
            let p4 = e.trailing_zeros() as u8;
            return self.last4(
                board.player_bb(),
                board.opponent_bb(),
                p1,
                p2,
                p3,
                p4,
                alpha,
                beta,
                passed,
                parity,
            );
        }

        let mut lower = alpha;
        let mut upper = beta;
        if let Some(v) = self.shallow_table.get(board, hash) {
            if v.lower() >= v.upper() {
                return v.lower();
            }
            if upper > v.upper() {
                upper = v.upper();
                if upper <= lower {
                    return upper;
                }
            }
            if lower < v.lower() {
                lower = v.lower();
                if lower >= upper {
                    return lower;
                }
            }
        }

        let orig_lower = lower;
        let mut best = lower;
        let mut any = false;
        let mut cut = false;
        let player_bb = board.player_bb();
        let opponent_bb = board.opponent_bb();
        // Children of a 5-empty node dispatch to last4 and never probe, so
        // their hashes are only needed one level up.
        let need_child_hash = board.empty_count() > 5;

        // Odd-quadrant moves first (quadrant parity: filling the last
        // empty of a region tends to keep the tempo).
        let empties = board.empty();
        let odd = PARITY_ODD_MASK[parity as usize];
        'passes: for pass_mask in [empties & odd, empties & !odd] {
            let mut e = pass_mask;
            while e != 0 {
                let sq = e.trailing_zeros() as u8;
                e &= e - 1;

                if self.neighbours.get(sq) & opponent_bb == 0 {
                    continue;
                }
                let pos = Position(sq);
                let pos_bit = pos.to_bit();
                let flips = bitboard::flippable(player_bb, opponent_bb, pos_bit);
                if flips == 0 {
                    continue;
                }

                any = true;
                let mut child = *board;
                child.apply_flips(pos, flips);
                let child_hash = if need_child_hash {
                    zobrist::board_hash(opponent_bb ^ flips, player_bb | flips | pos_bit)
                } else {
                    0
                };
                let val = -self.alpha_beta(
                    &mut child,
                    child_hash,
                    -upper,
                    -best.max(orig_lower),
                    false,
                    parity ^ quadrant_id(sq),
                );

                if val > best {
                    best = val;
                }
                if best >= upper {
                    cut = true;
                    break 'passes;
                }
            }
        }
        let _ = cut;

        if !any {
            if passed {
                return final_score(board);
            }
            board.pass();
            let val = -self.alpha_beta(
                board,
                zobrist::board_hash(board.opponent_bb(), board.player_bb()),
                -upper,
                -orig_lower,
                true,
                parity,
            );
            board.pass();
            return val;
        }

        self.shallow_table.update(board, hash, orig_lower, upper, best, None);
        best
    }

    /// Specialized 4-empties search with quadrant-parity move ordering.
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

        // Parity ordering: squares in odd quadrants first (odd sorts before
        // even because !odd is false < true). The caller carries the parity
        // down the tree, so this level does not recompute it.
        let odd = |sq: u8| parity & quadrant_id(sq) != 0;
        let mut arr = [p1, p2, p3, p4];
        arr.sort_by_key(|&s| !odd(s));
        let [p1, p2, p3, p4] = arr;

        let mut alpha = alpha;
        let mut any = false;

        for (a, rest) in [
            (p1, [p2, p3, p4]),
            (p2, [p1, p3, p4]),
            (p3, [p1, p2, p4]),
            (p4, [p1, p2, p3]),
        ] {
            if self.neighbours.get(a) & opponent == 0 {
                continue;
            }
            let pos_bit = 1u64 << a;
            let flips = bitboard::flippable(player, opponent, pos_bit);
            if flips == 0 {
                continue;
            }
            any = true;
            let val = -self.last3(
                opponent ^ flips,
                player | flips | pos_bit,
                rest[0],
                rest[1],
                rest[2],
                -beta,
                -alpha,
                false,
                parity ^ quadrant_id(a),
            );
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }

        if !any {
            if passed {
                return final_score_bb(player, opponent);
            }
            return -self.last4(opponent, player, p1, p2, p3, p4, -beta, -alpha, true, parity);
        }

        alpha
    }

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

        let odd = |sq: u8| parity & quadrant_id(sq) != 0;
        let mut arr = [p1, p2, p3];
        arr.sort_by_key(|&s| !odd(s));
        let [p1, p2, p3] = arr;

        let mut alpha = alpha;
        let mut any = false;

        for (a, rest) in [(p1, [p2, p3]), (p2, [p1, p3]), (p3, [p1, p2])] {
            if self.neighbours.get(a) & opponent == 0 {
                continue;
            }
            let pos_bit = 1u64 << a;
            let flips = bitboard::flippable(player, opponent, pos_bit);
            if flips == 0 {
                continue;
            }
            any = true;
            let val = -self.last2(
                opponent ^ flips,
                player | flips | pos_bit,
                rest[0],
                rest[1],
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

        if !any {
            if passed {
                return final_score_bb(player, opponent);
            }
            return -self.last3(opponent, player, p1, p2, p3, -beta, -alpha, true, parity);
        }

        alpha
    }

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

        for (a, b) in [(p1, p2), (p2, p1)] {
            if self.neighbours.get(a) & opponent == 0 {
                continue;
            }
            let pos_bit = 1u64 << a;
            let flips = bitboard::flippable(player, opponent, pos_bit);
            if flips == 0 {
                continue;
            }
            any = true;
            let val = -self.last1(opponent ^ flips, player | flips | pos_bit, b);
            if val >= beta {
                return val;
            }
            if val > alpha {
                alpha = val;
            }
        }

        if !any {
            if passed {
                return final_score_bb(player, opponent);
            }
            return -self.last2(opponent, player, p1, p2, -beta, -alpha, true);
        }

        alpha
    }

    /// Exactly one empty square left: resolve without recursion.
    fn last1(&mut self, player: u64, opponent: u64, p1: u8) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, 1);
        let pos_bit = 1u64 << p1;
        let diff = player.count_ones() as i32 - opponent.count_ones() as i32;

        self.nodes += 1;
        // Current player fills the last square
        if self.neighbours.get(p1) & opponent != 0 {
            let n = bitboard::flippable(player, opponent, pos_bit).count_ones() as i32;
            if n > 0 {
                return diff + 2 * n + 1;
            }
        }

        self.nodes += 1;
        // Current player passes; opponent fills the last square
        if self.neighbours.get(p1) & player != 0 {
            let n = bitboard::flippable(opponent, player, pos_bit).count_ones() as i32;
            if n > 0 {
                return diff - 2 * n - 1;
            }
        }

        // Nobody can play the last square: empty goes to the winner
        match diff.cmp(&0) {
            std::cmp::Ordering::Greater => diff + 1,
            std::cmp::Ordering::Less => diff - 1,
            std::cmp::Ordering::Equal => 0,
        }
    }

    /// Probable cutoff for a warm-up pass: a shallow evaluation search that
    /// clears the window by a confidence margin stands in for the exact
    /// result. Never reached by the final (exact) pass.
    fn selective_cut(
        &mut self,
        board: &Board,
        hash: u64,
        ev: &Evaluator,
        t: f32,
        lower: i32,
        upper: i32,
    ) -> Option<i32> {
        let empties = board.empty_count();
        let error = t * crate::search::mpc_sigma(empties as u32, empties, SELECTIVE_PROBE_DEPTH);
        let ix = ev.indexer();
        let mut indices = ix.init(board.black, board.white);

        let hi = upper as f32 + error;
        if hi < 64.0 {
            let v = self.seed_search(
                board, hash, ev, ix, &mut indices, SELECTIVE_PROBE_DEPTH, hi - 1.0, hi, false,
            );
            if v >= hi {
                return Some(upper);
            }
        }
        let lo = lower as f32 - error;
        if lo > -64.0 {
            let v = self.seed_search(
                board, hash, ev, ix, &mut indices, SELECTIVE_PROBE_DEPTH, lo, lo + 1.0, false,
            );
            if v <= lo {
                return Some(lower);
            }
        }
        None
    }

    /// Generate children sorted by an endgame move-ordering heuristic
    /// (fastest-first: minimize opponent mobility, corner stability, parity).
    /// The transposition table's best move, when present, is searched first.
    /// Each child carries its incrementally-updated Zobrist hash.
    fn scored_moves(
        &self,
        board: &Board,
        tt_best: Option<Position>,
        ev: Option<&Evaluator>,
        out: &mut MoveBuf,
    ) {
        self.gen_moves(board, out);
        self.score_moves(board, out, tt_best, ev, i32::MIN / 2);
    }

    /// Generate children with their incremental hashes but WITHOUT the
    /// expensive ordering evaluation. Wipeout moves are flagged: they end
    /// the game at +64, so a node that has one needs no search at all.
    fn gen_moves(&self, board: &Board, out: &mut MoveBuf) {
        let _prof = layer_profile::Scope::new(layer_profile::GEN, board.empty_count());
        out.len = 0;
        let mut m = board.movable();
        while m != 0 {
            let sq = m.trailing_zeros() as u8;
            m &= m - 1;
            let pos = Position(sq);
            let flipped = bitboard::flippable(board.player_bb(), board.opponent_bb(), pos.to_bit());
            // The child's bitboards, without materializing a Board: the
            // mover gains the flipped discs plus the square it played.
            let child_hash = zobrist::board_hash(
                board.opponent_bb() ^ flipped,
                board.player_bb() | flipped | pos.to_bit(),
            );
            self.table(board.empty_count() - 1).prefetch(child_hash);
            out.push(ScoredMove { pos, flipped, hash: child_hash, value: 0 });
        }
    }

    /// Fill in ordering values (the expensive part: pattern evaluation and
    /// pruned lookaheads). Split from generation so that a node whose
    /// transposition-table move already cuts never pays for it. Callers
    /// either sort the result or draw from it with `select_next`.
    fn score_moves(
        &self,
        board: &Board,
        moves: &mut [ScoredMove],
        tt_best: Option<Position>,
        ev: Option<&Evaluator>,
        alpha: i32,
    ) {
        let _prof = layer_profile::Scope::new(layer_profile::ORDER, board.empty_count());
        node_accounting::sorted(moves.len() as u64);
        let eval_order = board.empty_count() >= EVAL_ORDER_EMPTIES;
        // Incremental pattern indices for ordering evaluation: initialized
        // once per node, then updated per candidate move — far cheaper than
        // recomputing every pattern from scratch inside the lookahead.
        let mut order_ix = if eval_order {
            ev.map(|e| (e.indexer(), e.indexer().init(board.black, board.white)))
        } else {
            None
        };
        // Only the static path consults the parity, and computing it walks
        // every empty square — pure waste at the deep nodes that use the
        // evaluator instead.
        let parity = if eval_order { 0 } else { parity_of(board) };
        let mover = board.player();
        // The ordering lookahead only has to distinguish moves that could
        // matter at this node, so it is bounded from below by the node's own
        // alpha (less a margin); a full window wastes work
        // proving exact values for moves that are already hopeless.
        // The lookahead runs from the child's point of view, so the node's
        // alpha becomes an upper bound there.
        let sort_hi = if alpha <= i32::MIN / 4 {
            f32::INFINITY
        } else {
            -(alpha - SORT_ALPHA_DELTA) as f32
        };

        // The ladder raises the sort depth one ply at a time as the position
        // opens up (0 at 15-17 empties, 1 at 18-20, 2 at 21-23, more beyond).
        // The old one jumped 0 -> 2 -> 4, and since each extra ply of this
        // pruned search costs several times the previous one, the deep
        // problems drowned in
        // ordering work: on FFO56 the lookahead alone was 17% of the solve.
        let sort_depth = SORT_DEPTH_LADDER[(board.empty_count() as usize).min(63)];
        let player_bb = board.player_bb();
        let opponent_bb = board.opponent_bb();
        for sm in moves.iter_mut() {
            let pos = sm.pos;
            let flipped = sm.flipped;
            // The child as bare bitboards; only the evaluator path below
            // needs a real `Board`.
            let cp = opponent_bb ^ flipped;
            let co = player_bb | flipped | pos.to_bit();
            sm.value = if cp == 0 {
                // Wiping out the opponent ends the game at +64: try first
                i32::MIN
            } else if Some(pos) == tt_best {
                i32::MIN + 1
            } else if let (Some(e), Some((ix, indices))) = (ev, order_ix.as_mut()) {
                let child = sm.child(board);
                // Pattern evaluation of the child (opponent view: lower =
                // better for the mover). Far stronger ordering than the
                // static heuristic in the many-empties region; the topmost
                // region refines it with a pruned lookahead.
                // Restoring the snapshot beats undoing: `undo` walks the
                // flipped discs again and scatters an update through the CSR
                // table for each, while the indices are 160 bytes of
                // contiguous u16 that copy in a handful of vector moves.
                let saved = *indices;
                ix.apply(indices, pos, flipped, mover);
                let v = if sort_depth > 0 {
                    shallow_search(&child, e, ix, indices, sort_depth, f32::NEG_INFINITY, sort_hi)
                } else {
                    e.eval_order_bb(cp, co, mover.opponent(), indices)
                };
                *indices = saved;
                // Mobility weighs as heavily as the evaluation itself:
                // the evaluator alone is blind to
                // how many replies it leaves the opponent. One reply counts
                // for about one disc.
                // Stability the move gains is credited too. An exact edge
                // table would be sharper in this band, but the corner-only
                // count is
                // a few bit tests instead of four table lookups plus the rank
                // gather, and measured better on both node count and time —
                // the extra edge discs it misses are rarely what decides the
                // order. `co` is our own discs after the move.
                let edge = corner_stability_bb(co) * 8;
                (v * 8.0) as i32 + weighted_mobility(cp, co) * MOBILITY_ORDER_WEIGHT
                    - edge * EDGE_STABILITY_ORDER_WEIGHT
            } else {
                move_ordering_value(pos, cp, co, parity)
            };
        }

    }

    /// Swap the best-ordered of `moves[i..]` into `moves[i]`.
    ///
    /// Extracting the next best on demand instead of
    /// sorting the list: at a cut node, where the first move usually suffices,
    /// one O(n) scan replaces a full sort.
    #[inline]
    fn select_next(moves: &mut [ScoredMove], i: usize) {
        let mut best = i;
        for j in i + 1..moves.len() {
            if moves[j].value < moves[best].value {
                best = j;
            }
        }
        moves.swap(i, best);
    }
}

#[derive(Clone, Copy)]
struct ScoredMove {
    pos: Position,
    /// Discs this move flips. The child position is the parent with these
    /// recoloured, so storing the mask instead of a whole `Board` keeps the
    /// move list — and therefore every `pvs` stack frame — much smaller.
    flipped: u64,
    hash: u64,
    value: i32,
}

impl ScoredMove {
    /// Rebuild the position after this move from its parent.
    #[inline(always)]
    fn child(&self, parent: &Board) -> Board {
        let mut b = *parent;
        b.apply_flips(self.pos, self.flipped);
        b
    }
}

/// A position has at most 32 legal moves; 34 gives slack. Move lists live
/// in this stack buffer instead of a heap `Vec` — allocation showed up as
/// ~3% of solver time in deep-search profiles, and every node builds one.
/// The backing store is uninitialized (`MaybeUninit`) so there is no
/// per-node memset; only slots `0..len` are ever read.
const MAX_MOVES: usize = 34;

struct MoveBuf {
    buf: [std::mem::MaybeUninit<ScoredMove>; MAX_MOVES],
    len: usize,
}

impl MoveBuf {
    #[inline]
    fn new() -> MoveBuf {
        MoveBuf {
            // SAFETY: an array of MaybeUninit needs no initialization.
            buf: unsafe { std::mem::MaybeUninit::uninit().assume_init() },
            len: 0,
        }
    }

    #[inline]
    fn push(&mut self, m: ScoredMove) {
        self.buf[self.len].write(m);
        self.len += 1;
    }

    /// Remove by index, moving the last element into the hole. `ScoredMove`
    /// is `Copy`, so nothing needs dropping.
    #[inline]
    fn swap_remove(&mut self, i: usize) -> ScoredMove {
        // SAFETY: callers pass i < len, and 0..len are initialized.
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
        // SAFETY: slots 0..len were initialized by push.
        unsafe { std::slice::from_raw_parts(self.buf.as_ptr() as *const ScoredMove, self.len) }
    }
}

impl std::ops::DerefMut for MoveBuf {
    #[inline]
    fn deref_mut(&mut self) -> &mut [ScoredMove] {
        // SAFETY: as above.
        unsafe { std::slice::from_raw_parts_mut(self.buf.as_mut_ptr() as *mut ScoredMove, self.len) }
    }
}

/// Static square-visit priority for the ordering lookahead (file-major):
/// corners first, C/X squares last. Trying good squares first improves the
/// lookahead's alpha-beta cutoffs; because it only reorders sibling moves,
/// the value the lookahead returns is unchanged (and so is the resulting
/// move order and the main search's node count).
#[rustfmt::skip]
const SHALLOW_ORDER: [u8; 64] = {
    // priority weight per square, rank-major, then transposed to file-major
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

/// Shallow alpha-beta refinement for ordering: the position's value from
/// its own player's view, looking `depth` replies ahead with the
/// evaluator. Pruned — same root value as a full-width lookahead at a
/// fraction of the cost, which buys deeper (= better-sorted) lookaheads.
#[allow(clippy::too_many_arguments)]
fn shallow_search(
    board: &Board,
    ev: &Evaluator,
    ix: &PatternIndexer,
    indices: &mut PatternIndices,
    depth: u8,
    alpha: f32,
    beta: f32,
) -> f32 {
    let _prof = layer_profile::Scope::new(layer_profile::LOOKAHEAD, board.empty_count());
    node_accounting::lookahead();
    if depth == 0 {
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
        // A pass leaves the discs (and thus the indices) unchanged
        return -shallow_search(&p, ev, ix, indices, depth, -beta, -alpha);
    }
    let mut alpha = alpha;
    let mut best = f32::NEG_INFINITY;
    let mover = board.player();

    // Collect legal squares and visit them corner-first. Reordering
    // siblings cannot change the value alpha-beta returns, only how quickly
    // it prunes.
    let mut sqs = [0u8; MAX_MOVES];
    let mut n = 0usize;
    let mut m = moves;
    while m != 0 {
        sqs[n] = m.trailing_zeros() as u8;
        n += 1;
        m &= m - 1;
    }
    let sqs = &mut sqs[..n];
    sqs.sort_unstable_by_key(|&sq| SHALLOW_ORDER[sq as usize]);

    for &sq in sqs.iter() {
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
                break;
            }
        }
    }
    best
}

/// Move-ordering heuristic (lower = searched earlier): after the move, the
/// board is from the opponent's perspective, so high opponent mobility is
/// bad for us.
/// 8-neighbourhood dilation (file-major layout).
#[inline]
fn dilate(x: u64) -> u64 {
    let v = x | ((x & 0x7F7F_7F7F_7F7F_7F7F) << 1) | ((x & 0xFEFE_FEFE_FEFE_FEFE) >> 1);
    v | (v << 8) | (v >> 8)
}

/// Opponent replies, counting a corner reply twice: a corner is worth far
/// more than an ordinary reply, and the extra AND and
/// popcount are free next to the move generation itself.
#[inline]
fn weighted_mobility(cp: u64, co: u64) -> i32 {
    const CORNERS: u64 = 0x8100_0000_0000_0081;
    let m = bitboard::mobility(cp, co, !(cp | co));
    (m.count_ones() + (m & CORNERS).count_ones()) as i32
}

/// Ordering value of a child, given as raw bitboards (side to move, other)
/// so that no `Board` — and no runtime-indexed `player_bb()` — is built for
/// a move the search may never reach.
///
/// The terms sit on widely separated scales:
/// real mobility dominates, corner stability is a sixteenth of
/// it, potential mobility a thousandth, and the square table and parity are
/// tie-breaks. An earlier version blended the same ingredients on a flat
/// scale, which
/// let a corner-stability difference outweigh a reply.
fn move_ordering_value(pos: Position, cp: u64, co: u64, parity: u8) -> i32 {
    const W_MOBILITY: i32 = 1 << 15;
    const W_CORNER_STABILITY: i32 = 1 << 11;
    const W_POTENTIAL: i32 = 1 << 5;
    const W_PARITY: i32 = 1 << 3;

    let empty = !(cp | co);
    // Potential mobility counts the *opponent's* potential moves: the empty
    // squares next to
    // our discs. An earlier version counted our own frontier discs, which is
    // related but not the same quantity.
    let potential = (dilate(co) & empty).count_ones() as i32;
    let mut score = SQUARE_VALUE[pos.index() as usize] as i32;
    if parity & quadrant_id(pos.index()) != 0 {
        score += W_PARITY;
    }
    score += (36 - potential) * W_POTENTIAL;
    score += corner_stability_bb(co) * W_CORNER_STABILITY;
    score += (36 - weighted_mobility(cp, co)) * W_MOBILITY;
    // The score builds ascending-is-better; the solver draws the smallest
    // value first, hence the negation.
    -score
}

/// JCW square values. The table is
/// symmetric in both axes, so it needs no transposing for our file-major
/// layout.
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

/// Corner stability count: corners owned plus adjacent edge discs.
fn corner_stability_bb(bb: u64) -> i32 {
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

/// Where the search spends its time, split by empty count and by phase.
///
/// A node stores its layer and phase into one atomic on entry and restores
/// the caller's on the way out; a sampling thread reads that word. Unlike a
/// stack profiler this attributes time to the *logical* phase, which is what
/// told apart "the leaf routines are slow" from "the ordering is slow" when
/// comparing against Edax layer by layer.
///
/// Enable with `--features layer-profile`; off, every call folds away.
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
    pub const PHASE_NAMES: [&str; PHASES] = [
        "search", "order", "look", "warm", "gen", "etc", "tt", "stab",
    ];

    pub static STATE: AtomicU32 = AtomicU32::new(0);
    /// Search nodes entered at each empty count. Note that a node dispatching
    /// straight to a leaf routine is counted by both, so the leaf layers are
    /// double-counted; compare totals, not per-layer averages, across them.
    pub static NODES: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
    pub const ENABLED: bool = cfg!(feature = "layer-profile");

    /// Sets the current (phase, empties) and restores the previous on drop.
    /// The saved value is unread when the feature is off, which is the point.
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

    /// The current bucket as (phase, empties).
    pub fn sample() -> (usize, usize) {
        let v = STATE.load(Relaxed);
        (((v >> 8) as usize).min(PHASES - 1), (v & 0xFF) as usize & 63)
    }
}

/// Node accounting on Edax's terms, so node counts compare fairly.
///
/// Our search counter only counts nodes the search itself visits. Edax also
/// counts every move it scores while ordering, every
/// enhanced-transposition probe and every node of the
/// shallow searches ordering launches — which for us adds about 90% on top
/// of the search count, so node comparisons are meaningless until both
/// sides count the same things.
///
/// Off by default: the counters cost a few percent of search time. Enable
/// with `--features node-accounting`.
pub mod node_accounting {
    #[cfg(feature = "node-accounting")]
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    #[cfg(feature = "node-accounting")]
    static SORTED: AtomicU64 = AtomicU64::new(0);
    #[cfg(feature = "node-accounting")]
    static ETC: AtomicU64 = AtomicU64::new(0);
    #[cfg(feature = "node-accounting")]
    static LOOKAHEAD: AtomicU64 = AtomicU64::new(0);

    /// Moves scored for ordering at one node.
    #[inline(always)]
    pub(crate) fn sorted(n: u64) {
        let _ = n;
        #[cfg(feature = "node-accounting")]
        SORTED.fetch_add(n, Relaxed);
    }

    /// Enhanced-transposition probes made at one node.
    #[inline(always)]
    pub(crate) fn etc(n: u64) {
        let _ = n;
        #[cfg(feature = "node-accounting")]
        ETC.fetch_add(n, Relaxed);
    }

    /// One node of the move-ordering lookahead.
    #[inline(always)]
    pub(crate) fn lookahead() {
        #[cfg(feature = "node-accounting")]
        LOOKAHEAD.fetch_add(1, Relaxed);
    }

    /// Ordering nodes counted so far: (moves scored, ETC probes, lookahead).
    /// All zero unless the `node-accounting` feature is on.
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

    /// Whether the counters are compiled in.
    pub const ENABLED: bool = cfg!(feature = "node-accounting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    /// Brute-force negamax over full boards for cross-checking the solver.
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

    /// Play a deterministic game until `empties` squares remain.
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
        // Cross-check exact scores against brute force at several depths
        for empties in [4u8, 6, 8] {
            let board = position_with_empties(empties);
            if board.is_game_over() {
                continue;
            }
            let expected = negamax(&board, false);
            let mut solver = Solver::new(14);
            let result = solver.solve(EndSolverMode::Perfect, &board);
            assert_eq!(
                result.value, expected,
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
        // A board where the current player cannot move at all
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
        // Fill a game down to exactly 1 empty and compare against negamax
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
        let t = NeighbourTable::new();
        // A1 (corner, sq 0): neighbours are A2(1), B1(8), B2(9)
        assert_eq!(t.get(0), (1u64 << 1) | (1u64 << 8) | (1u64 << 9));
        // Center square E5 (sq 36) has 8 neighbours
        assert_eq!(t.get(36).count_ones(), 8);
    }

    #[test]
    fn test_quadrant_parity() {
        let b = Board::new();
        // 60 empties, 15 per quadrant (each quadrant has 16 squares, minus
        // one initial disc each) -> every quadrant parity is odd
        assert_eq!(parity_of(&b), 0b1111);
    }
}
