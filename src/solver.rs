//! Endgame solver: PVS (principal variation search) with a Zobrist-keyed
//! transposition table, move ordering, and specialized last-4/3/2/1 fast
//! paths.
//!
//! Scores are disc differences from the current player's perspective, with
//! the empty-square bonus applied to the winner (Board::score semantics).

// Indexed loops here iterate in an order that matters (contiguous scans,
// SIMD-style unrolling), so the iterator lints are not taken. Search
// functions keep their long argument lists: bundling them into a struct
// would add per-call construction on a hot path.
#![allow(clippy::too_many_arguments)]

use crate::bitboard;
use crate::board::Board;
use crate::evaluator::Evaluator;
use crate::pattern_index::{PatternIndexer, PatternIndices};
use crate::position::Position;
use crate::zobrist;

/// Search depth thresholds (empties remaining) for switching strategies.
const PVS_LIMIT: u8 = 12;

/// `PVS_MIN` overrides [`PVS_LIMIT`] for sweeps.
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
/// Lowest empty count at which the ordered stage consults a transposition
/// table at all. Below it the stage searches without probing or storing.
///
/// Seven and eight empties are the busiest layers of the exact pass, and a
/// probe there reaches the 12.6 MB mid table — a cache miss whose latency
/// the hit rate does not repay, the same trade that made shrinking the
/// shallow table a win. Skipping both: band22 -4.5%, fix22 -5.6%,
/// fix24 -3.9%, ffo40-49 -4.7%, for trees only 0.6-2.9% larger. Nine still
/// earns its probe (skipping it too costs 2.3-13.2% against this), and
/// pushing the whole ordered stage up one layer instead
/// (`MOVE_ORDERING_LIMIT` 8) loses on every set, at +22-30% nodes.
/// Lowest empty count that consults a transposition table proper.
///
/// Below this the bands have their own direct-mapped bound caches. `9` is
/// what this engine grew: 9 on the private mid table, 10-12 on the shared
/// one. `ec-band` builds the alternative where nothing below 13 touches the
/// main table and 7-12 share one thread-private bound-only cache; it costs
/// 2.6% of the tree, so it is not the default.
#[cfg(feature = "ec-band")]
const TT_MIN_EMPTIES_DEFAULT: u8 = 13;
#[cfg(not(feature = "ec-band"))]
const TT_MIN_EMPTIES_DEFAULT: u8 = 11;

/// `TT_MIN` overrides the boundary for sweeps.
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

/// `ETC_MIN` overrides the enhanced-transposition-cutoff floor for sweeps.
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

/// Highest empty count the bound cache covers under `l78-wide`.
#[cfg(feature = "l78-wide")]
const TT_MID_WIDE: u8 = 13;
/// Below this many empties the ordered search goes to a cache-resident
/// table instead of the main one. The main table holds 2^26 entries of 24
/// bytes, so a probe there is a guaranteed DRAM round trip; at these
/// depths the tree is wide enough that the latency, not the hit rate,
/// decides.
const MID_TT_EMPTIES: u8 = 10;
/// Entry count of that table, as a power of two. 2^19 * 24 B = 12.6 MB.
const MID_TT_BITS: u32 = 19;
/// Entry count of the 5-6 empties table, as a power of two.
/// 2^13 * 24 B = 192 KB. The 1.57 MB it used to be was chosen for hit rate;
/// hit rate is not what this table is for. Shrinking it costs ~1% more nodes
/// and buys far more than that back in latency, on every set measured
/// (band22 -9.7%, FFO40-49 -6.7%, 24 empties -8.5%, 26 empties -7.4%, two
/// rounds each, minima, with a steady six-thread job on the other cores).
/// The curve is monotone from 16 down to 13 and flat below: 2^12 is another
/// 0.3-0.7%, inside the run-to-run spread.
const SHALLOW_TT_BITS: u32 = 13;

/// Highest empty count still served by the private mid table, exclusive.
/// `TT_MID_EMPTIES` overrides it for sweeps.
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

/// Sizes of the two private tables, in entries as a power of two.
/// `TT_SHALLOW_BITS` / `TT_MID_BITS` override them for sweeps.
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
/// From this many empties upward, an evaluator (when provided) orders
/// moves instead of the static heuristic.
const EVAL_ORDER_EMPTIES: u8 = 14;

/// `EVAL_ORDER_MIN` overrides [`EVAL_ORDER_EMPTIES`] for sweeps.
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
/// From this many empties upward, ordering refines the evaluation with a
/// one-ply lookahead (max over the opponent's replies).
///
/// The three rungs moved out by one when the pattern indices stopped being
/// rebuilt at every node: the lookahead's own per-move snapshot did not get
/// cheaper, so the evaluation it competes with did, and the balance moved
/// against it. 17/20/29 is -2.1% against 16/19/28 for 2.6% more nodes;
/// 15/18/27, 18/20/29 and 17/21/29 are all worse.
const DEEP_ORDER_EMPTIES: u8 = 17;
/// Terminal score with the empty-square bonus awarded to the winner
/// (FFO convention; also what the game pipeline records). The old
/// plain disc difference disagreed whenever a game ended with empties left
/// — last1 already awarded its single empty, the general terminals did not.
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

/// Wipeout: a side with no discs can never move again, so the game is
/// over the moment it happens (worth handling explicitly; without it
/// the search still terminates but only after mobility churn).
#[inline]
fn wipeout_score(board: &Board) -> Option<i32> {
    wipeout_score_bb(board.player_bb(), board.opponent_bb())
}

/// A child's bitboard pair, straight from the parent's and the move's flip
/// mask - the `Board` this replaces was copied, mutated and colour-flipped
/// once per move just to be read back as two words.
#[inline]
fn child_bb(player: u64, opponent: u64, m: ScoredMove) -> (u64, u64) {
    (opponent ^ m.flipped, player | m.flipped | m.pos.to_bit())
}

/// Rebuild a `Board` for the few places that still key on one: the shared
/// transposition table and the legacy 5-6 table both fold the colour into
/// their match, so it has to be carried down rather than assumed.
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

/// The same test from raw bitboards, for the bands that never build a
/// `Board`.
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

/// Extra discs the popcount gate demands beyond the bare minimum, so the
/// stable-disc sweep is skipped where the surplus is too thin to cut.
///
/// `stable_count` returns far fewer stable discs than the raw disc count, so
/// a position that only just clears `need` almost never proves the bound and
/// the whole sweep is wasted. Charging eight discs of headroom pays for
/// itself on every set measured (exact pass, one thread, alternating runs,
/// minima, solutions identical, against margin 0):
///
/// | set | time | tree |
/// |---|---|---|
/// | band22 | -4.4% | +0.5% |
/// | FFO40-49 | -2.0% | +1.9% |
/// | fix24 | -6.8% | +0.5% |
/// | deep26 | -2.5% | +0.4% |
///
/// Swept over 0/4/8/12/16. Eight is the largest margin that still leaves the
/// tree essentially untouched. Twelve and sixteen are a shade faster on
/// band22 and fix24, but they buy that time by giving up cuts rather than by
/// skipping futile sweeps: on FFO40-49 the tree grows 9.1% at twelve and
/// 26.2% at sixteen, and sixteen is already 2.4% slower there outright.
///
/// **Do not re-tune this on a subset.** band22 and fix24 on their own would
/// have chosen twelve or sixteen; FFO40-49 is the set that vetoes them, and
/// the tree trend says the loss keeps growing as positions get deeper. Same
/// trap as the ordering-band and sort-ladder step points before it.
const STABILITY_CUT_MARGIN: i32 = 8;

/// Lowest empty count at which a stability cut is attempted. At four
/// empties the cut is reached on every node of the busiest layer and costs
/// 6.1% of wall clock there, but the subtree it prunes is four plies deep —
/// far too small to pay for a `stable_count` on both colours. Measured with
/// the cut disabled below 5: band22 -2.9%, fix22 -2.8%, fix24 -3.0%, and
/// neutral on the two sets where the tree grows most (fix20 +0.2% for +17%
/// nodes, ffo40-49 +0.1% for +24.7%). Disabling it below 6 or 7 raises
/// nodes/s further still — to 29.2M/s on ffo40-49 against 20.5M/s here —
/// while wall clock gets worse by 1.7% and 3.1%; the rate rises only
/// because the nodes left behind are the cheap ones.
/// 4, not the 5 it was while wall clock was the KPI: cutting at four
/// empties shrinks the tree by ~19% on FFO40-49 (the four-empty cache and
/// the cut compound - fresh subtrees die to the cut, revisits to the
/// cache), at some per-node cost. `EXACT_STAB_MIN` overrides for sweeps.
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

/// Stability cutoff precondition: the bound 64 - 2*S can only cut when the
/// opponent has at least ceil((64-alpha)/2) stable discs, so their total
/// disc count (cheap popcount) must reach that first.
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

/// The same cut from raw bitboards, for callers that hold a child's occupancy
/// but never build the `Board` — the 5-empty move loop, which hands its
/// children straight to the 4-empty routine.
///
/// Gate and sweep are split so the gate can inline: seven call sites reach
/// it 127.6M times on FFO40-49 and only 29.0M get past the two cheap tests,
/// so keeping the whole routine out of line charged a call, a frame and the
/// register traffic to a hundred million threshold compares.
#[inline(always)]
fn stability_cut_bb(player: u64, opponent: u64, empties: u8, alpha: i32, beta: i32) -> Option<i32> {
    if empties < stab_min_empties() {
        return None;
    }
    #[cfg(feature = "layer-profile")]
    ab_stats::STAB_GATE_A.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Upper bound via the opponent's stable discs (fail low)
    if alpha >= STABILITY_THRESHOLD[(empties & 63) as usize] {
        let need = (64 - alpha + 1) / 2;
        // `count_ones` is a round trip through a vector register on
        // aarch64, so it stays behind the threshold test.
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

/// The fail-low sweep. Out of line: fewer than a quarter of the gate's
/// callers reach it, and it is the only part big enough to pay for a call.
#[inline(never)]
fn stability_sweep_low(player: u64, opponent: u64, need: i32, alpha: i32) -> Option<i32> {
    let bound =
        64 - 2 * crate::stability::stable_count_at_least(opponent, player, need as u32) as i32;
    (bound <= alpha).then_some(bound)
}

/// The fail-high half - a lower bound from our own stable discs - is
/// off. It ran 26.7M of the 66.7M full stability computations and bought
/// 0.5% to 2.4% of the tree for 2% to 4% of the clock: measured over four
/// sets, six shuffled rounds each, hard20 -3.26%, band22 -2.12%,
/// 18-empty roots -4.44%, FFO40-49 -3.30%, total -3.17%.
///
/// It was adopted on the argument that the popcount gate makes it nearly
/// free and that it collapses one-sided positions like FFO#59. The gate
/// is indeed cheap; what it does not gate is that a position passing it
/// still pays the full stable-disc sweep, and the sweep is the cost.
/// `stab-fail-high` restores it.
#[inline(always)]
fn stability_cut_high(player: u64, opponent: u64, empties: u8, beta: i32) -> Option<i32> {
    #[cfg(not(feature = "stab-fail-high"))]
    {
        let _ = (player, opponent, empties, beta);
        #[cfg(feature = "layer-profile")]
        ab_stats::STAB_GATE_B.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        None
    }
    #[cfg(feature = "stab-fail-high")]
    {
        let threshold = STABILITY_THRESHOLD[(empties & 63) as usize];
        let need = (64 + beta + 1) / 2;
        #[cfg(feature = "layer-profile")]
        ab_stats::STAB_GATE_B.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if beta <= -threshold
            && need <= 32
            && (player.count_ones() as i32) >= need + STABILITY_CUT_MARGIN
        {
            #[cfg(feature = "layer-profile")]
            ab_stats::STAB_BETA.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let bound = 2 * crate::stability::stable_count_at_least(player, opponent, need as u32)
                as i32
                - 64;
            if bound >= beta {
                return Some(bound);
            }
        }
        None
    }
}

/// From this many empties upward, ordering uses a two-ply lookahead.
const DEEP2_ORDER_EMPTIES: u8 = 20;
/// From this many empties upward, ordering uses a three-ply lookahead.
const DEEP3_ORDER_EMPTIES: u8 = 29;
/// Enhanced transposition cutoff: from this many empties upward, probe
/// every child's hash entry before searching — a proven fail-high there
/// cuts this node without any search.
const ETC_EMPTIES: u8 = 13;
/// Ordering weight of one opponent reply, in eighths of a disc (the same
/// scale the evaluation term uses).
const MOBILITY_ORDER_WEIGHT: i32 = 12;
/// Splitting only pays off when each sibling subtree is substantial: below
/// this many empties the hand-off costs more than the subtree.
const PARALLEL_MIN_EMPTIES: u8 = 16;

/// `PAR_MIN=<n>` overrides [`PARALLEL_MIN_EMPTIES`] for sweeps: the floor
/// was tuned for momentary hand-offs, and persistent split points change
/// what a shallow split is worth.
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

/// How many nodes split, and how many young brothers that handed to the pool.
pub static SPLITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static HANDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Wall-nanoseconds in the selective warm-up ladder, and in the exact pass
/// that follows. The two scale differently: on FFO40-49 and 10 threads the
/// ladder gets 3.21x against the exact pass's 5.28x, so its share of the solve
/// grows from 20% to 28%.
///
/// Three candidate causes were measured and ruled out. `estimate_score` — the
/// sequential depth-6 evaluation search that centres the first window — is
/// 0.008s of the 4.3s. The ladder does not thrash the aspiration either: the
/// whole ten-problem run makes 30 root searches against the 20 that two rungs
/// plus the exact pass need. And it is not the choice of rungs (see
/// `SELECTIVE_LADDER`).
///
/// Nor is it something Lazy SMP can fill. Extra whole-search copies on idle
/// workers only pay off for searches shallow enough to have no split points
/// at all, where the workers would otherwise sit idle. Our rungs are full-depth
/// selective searches at 20-26 empties, which YBWC can and does split, so
/// copies only compete with it: measured on FFO40-49, min of 3, 10 threads
/// 4.90s -> 5.25 / 5.43 / 5.53 / 5.69s for 1 / 2 / 3 / 5 copies, and the
/// warm-up phase itself got *slower* (1.43s -> 1.73s). Node count went
/// 597M -> 726M while nps went 122M -> 138M: idle time turned into duplicated
/// work rather than useful work. **Lazy SMP earns its keep in a phase we do
/// not have.**
///
/// What is left is that a selective tree is a smaller tree, and small trees do
/// not fill ten threads — the same law the midgame ran into.
pub static WARMUP_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static EXACT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nodes searched in each phase, to tell a phase that is idle apart from a
/// phase that is doing wasted work: wall time alone cannot separate them.
pub static WARMUP_NODES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static EXACT_NODES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Wall-nanoseconds spent emptying the table before a solve. Broken out
/// because it is neither search nor parallel: one thread writes 2^`bit_size`
/// entries while the rest of the pool has nothing to do, so counting it as
/// search time understates the speedup and the thread occupancy alike.
pub static CLEAR_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// A young brother was offered to the pool and refused: no worker was idle.
pub static REFUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Number of abort events (siblings stopped after a proven cutoff) and
/// tasks killed by them. Needed to verify a test set actually exercises
/// this path — "values matched" is meaningless on a set that never aborts.
pub static ABORT_FIRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Force aborts (`SOLVER_CHAOS=n` fires one every n). Race-only bugs
/// appear intermittently; without a way to make aborts dense you cannot
/// even tell whether a fix worked.
/// Whether proven cutoffs abort siblings; on by default (`SOLVER_ABORT=0` disables).
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
/// Thread-nanoseconds spent inside handed-off tasks.
pub static TASK_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Thread-nanoseconds a thread spent waiting for its own fan-out. This is the
/// "play" in the schedule: the thread is alive but has nothing to do.
pub static WAIT_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
        self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
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

// There is deliberately no cap on how many young brothers one node may have
// outstanding. Capping measured worse here at every setting — FFO40-49,
// min of 3, 10 threads: cap 1 11.85s, 2 8.44s, 3 6.85s, 5 6.71s, none 4.89s.
// Cap 1 searches the *fewest* nodes of the lot (550M against 595M) and is still
// 2.4x slower: holding back the speculation costs more parallelism than it
// saves work. Edax caps at one, which is why it stops scaling past 6 threads.

/// Sentinel returned by a search that unwound early. It is never stored in
/// the table and never compared as a real score.
const ABORTED: i32 = i32::MIN + 1;

/// Pool of spare search threads, shared by every node that wants to split.
/// Nodes reserve helpers before spawning and hand them back afterwards, so
/// the total number of live search threads never exceeds the configured
/// budget no matter how deeply splits nest.
struct ThreadBudget {
    pool: EndPool,
    /// Private tables handed back by finished helpers. A helper's two tables
    /// are 14 MB of `HashEntry`, and allocating them means zero-filling all of
    /// it: at tens of thousands of splits that moves more memory than the
    /// search itself. Recycling makes a hand-off cost nothing — the effect a
    /// persistent pool gets by building per-thread state once at startup.
    scratch: std::sync::Mutex<Vec<Scratch>>,
}

/// A helper's two private tables, plus the selectivity they were filled under.
struct Scratch {
    shallow: HashTable,
    mid: HashTable,
    l4: Vec<L4Entry>,
    l56: Vec<L4Entry>,
    l78: Vec<L4Entry>,
    /// `selective_t` bits (0 = exact). Entries are keyed by the full board, so
    /// a recycled table can only be hit by a search of the very same position
    /// — and for those, a selective pass's bounds must not be read as exact.
    /// Same setting: keep the entries, they are a warm cache.
    tag: u32,
}

impl ThreadBudget {
    fn new(extra_threads: usize) -> ThreadBudget {
        ThreadBudget {
            pool: EndPool::new(extra_threads),
            scratch: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// A recycled table pair if one is free, otherwise a fresh one.
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

/// Search one handed-off young brother and publish the result.
///
/// A fresh search state over the shared table, one null-window pass at the
/// alpha that held when the task was queued, and a re-search on the spot if
/// it lands inside the window. Re-searching here rather than back in the
/// parent keeps that work parallel.
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
    ev: Option<&Evaluator>,
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
    let ch = child_hash_of(&child);
    let mut val = -w.pvs(&mut child, ch, -cur - 1, -cur, false, true, ev);
    if val == -ABORTED {
        val = ABORTED;
    } else if cur < val && val < upper {
        let ch = child_hash_of(&child);
        let re = -w.pvs(&mut child, ch, -upper, -val, false, false, ev);
        val = if re == -ABORTED { ABORTED } else { re };
    }
    /* A move that fails low in a null window only yields an upper bound:
    after `(-cur-1, -cur)` with `val <= cur` the true value may be lower.
    Feeding that into `shared_lower` gives siblings a wrong window and
    lets bounds leak into best-move selection (observed: a 19-empties
    position picked a -20 move labelled +14 in parallel). Only treat the
    value as exact when it beats the window. */
    if val != ABORTED && val > cur {
        shared_lower.fetch_max(val, Ordering::Relaxed);
        if val >= upper {
            /* Cutoff proven: stop the remaining siblings. Their results
            are discarded, but the discard has to be careful — an aborted
            task returns `ABORTED` and the collector must treat it as
            "unseen", not "bad", or the best move can be lost.
            `SOLVER_ABORT=0` disables this for bisection. */
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

/// A published split point: one node's remaining siblings, claimable move
/// by move by any worker until the cursor drains (late join). The owner
/// registers it, competes for moves like everyone else, and returns only
/// after every joined helper has left, so the borrows behind the raw
/// pointers outlive every use. `SPLIT_V2=0` restores the task-queue path.
struct SplitPoint {
    board: Board,
    moves: *const ScoredMove,
    n_moves: usize,
    upper: i32,
    selective_t: Option<f32>,
    nnue: Option<NnueProbe>,
    sigma_scale: f32,
    tt: *const HashTable,
    /// `Option<&Evaluator>` with the lifetime erased; null = `None`.
    ev: *const (),
    /// `&AbortFlag` with the lifetime erased.
    group: *const (),
    /// `&ThreadBudget` with the lifetime erased.
    budget: *const (),
    cursor: std::sync::atomic::AtomicUsize,
    shared_lower: std::sync::atomic::AtomicI32,
    /// Helpers currently joined (plus one per claim in flight).
    active: std::sync::atomic::AtomicUsize,
    merge: std::sync::Mutex<SplitMerge>,
    nodes: std::sync::atomic::AtomicU64,
    waiter: std::thread::Thread,
}

// SAFETY: every raw pointer targets the owner's frame or longer-lived
// state, and the owner does not return before `active` drains and the
// registry guard clears (see `unregister_split`).
unsafe impl Send for SplitPoint {}
unsafe impl Sync for SplitPoint {}

#[derive(Clone, Copy)]
struct SplitMerge {
    max: i32,
    best_val: i32,
    best: Option<Position>,
    aborted: bool,
}

/// `SPLIT_V2=0` restores the momentary task-queue hand-off.
#[cfg(feature = "tunable")]
fn split_v2() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SPLIT_V2").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn split_v2() -> bool {
    true
}

/// Fold one finished sibling into the split's running result. The rules are
/// the task-queue path's, verbatim: an aborted sibling is unseen, not bad;
/// a fail-low bound may raise the value but never picks the move.
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

/// Claim and run moves from a published split point until it drains, cuts
/// or aborts. Returns whether at least one move was searched. The caller
/// has already reserved its presence in `sp.active`.
///
/// # Safety
/// `sp` and everything behind its pointers must stay alive; the owner's
/// wait on `active` guarantees it as long as the caller's reservation
/// stands.
unsafe fn help_split(sp: &SplitPoint) -> bool {
    use std::sync::atomic::Ordering;
    let t_live = std::time::Instant::now();
    // SAFETY: owner-frame borrows, alive per the function contract.
    let (tt, group, budget) = unsafe {
        (
            &*sp.tt,
            &*(sp.group as *const AbortFlag<'static>),
            &*(sp.budget as *const ThreadBudget),
        )
    };
    let ev: Option<&Evaluator> = if sp.ev.is_null() {
        None
    } else {
        // SAFETY: same contract.
        Some(unsafe { &*(sp.ev as *const Evaluator) })
    };
    // SAFETY: same contract.
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
        w.nnue = sp.nnue;
        w.sigma_scale = sp.sigma_scale;
        let mut child = m.child(&sp.board);
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

/// One handed-off sibling's result, written by whoever ran it.
struct TaskSlot {
    done: std::sync::atomic::AtomicBool,
    /// Whether the result beat the window it was handed. A value that
    /// didn't is only an upper bound and cannot become the best move.
    /// The writer records the verdict here because carrying the original
    /// window in `handed` would double the per-split Vec width (2.5M
    /// splits make the allocation visible).
    beat: std::sync::atomic::AtomicBool,
    value: std::sync::atomic::AtomicI32,
    nodes: std::sync::atomic::AtomicU64,
    /// The thread that will wait for this slot, recorded when the slot is
    /// built so the task can wake it. Parking beats spinning here: a thread
    /// waiting on a split with nothing left to steal would otherwise burn a
    /// core, and measured that way it cost more than the idleness it hid.
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

    /// Publish the result. `done` is released before the wake so a waiter that
    /// the unpark reaches always sees the value and node count.
    fn finish(&self, value: i32, nodes: u64, beat: bool) {
        use std::sync::atomic::Ordering;
        self.beat.store(beat, Ordering::Relaxed);
        self.value.store(value, Ordering::Relaxed);
        self.nodes.store(nodes, Ordering::Relaxed);
        // Take the handle *before* publishing. The slots live in the splitting
        // thread's stack frame, so the moment `done` becomes visible that
        // thread may leave `split_siblings` and destroy the whole `Vec` — and
        // then `self.waiter` is freed memory. Storing first and unparking
        // second is a use-after-free that segfaults inside `Thread::unpark`
        // roughly once every few thousand solves; it needs a fan-out finishing
        // in the same instant the parent stops waiting, which is why the
        // ten-position FFO runs never showed it and a few hundred games did.
        // `Thread` is refcounted, so the clone keeps the parked thread
        // reachable no matter what happens to the slot.
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

/// Task pool for one solve.
///
/// The unit of parallel work is **one young brother**. A splitting node tries
/// to hand over every child it is about to search; the only gate is whether a
/// worker is idle *right now* (checked unlocked, then again under the lock),
/// and the refusal is reported back. On refusal the parent searches that child
/// itself.
///
/// That granularity is what the previous scheme lacked. Reserving helpers up
/// front and giving them a whole sibling list means a helper stays committed to
/// one node until the list drains, and a node that cannot reserve searches its
/// siblings alone even while workers elsewhere go idle. Here a worker rejoins
/// the idle set after a single subtree and is free to serve any node in the
/// tree.
const SPLIT_SLOTS: usize = 32;

/// Ceiling for the speculative All-node fan-out: above it a mispredicted
/// All node wastes too large a subtree.
const SPEC_SPLIT_MAX_EMPTIES: u8 = 24;

/// `SPEC_MAX=<n>` overrides [`SPEC_SPLIT_MAX_EMPTIES`] for sweeps.
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

/// Split-scan order: default prefers the biggest remaining subtree (a
/// helper buys the most work per join, and joins fewest times per second -
/// measured -3.8% against index order, while smallest-first measured +12%).
/// `JOIN_DEEP=0` restores index order, `1` smallest-first, for sweeps.
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

/// `MAX_JOIN=<n>` caps how many helpers may crowd one split point
/// (0 = unlimited).
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

/// The speculative All-node fan-out (`SPEC_SPLIT=0` disables). A loaded
/// window measured it neutral; the quiet-window judgment run has it a
/// consistent -2.3% at 8 threads (minima 3.136s vs 3.210s over four
/// alternating rounds), so it is on.
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
    /// Published split points, scanned by idle workers (late join). The
    /// guard counter brackets the load-and-reserve window so an owner can
    /// prove no worker still holds a just-unregistered pointer.
    split_ptr: [std::sync::atomic::AtomicPtr<SplitPoint>; SPLIT_SLOTS],
    split_guard: [std::sync::atomic::AtomicUsize; SPLIT_SLOTS],
    /// Workers blocked waiting for work.
    idle: std::sync::atomic::AtomicUsize,
    /// Queue length, readable without the lock. Nearly every hand-off attempt
    /// is a refusal, and taking the mutex just to discover that funnels every
    /// searching thread through one lock.
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

    /// Hand `f` to an idle worker, or refuse and let the caller do the work.
    ///
    /// # Safety
    ///
    /// `f` may borrow the caller's stack, and the box is erased to `'static` to
    /// get it into the queue. The caller must not release those borrows until
    /// the task has run. Every caller here waits on the task's
    /// [`TaskSlot::done`] before returning, and the workers are joined by the
    /// `thread::scope` that owns the solve, so a queued task can never outlive
    /// what it points at.
    unsafe fn try_push<'t>(&self, f: impl FnOnce() + Send + 't) -> bool {
        use std::sync::atomic::Ordering;
        if self.workers == 0 {
            return false;
        }
        // Unlocked first, exactly as `push_task` does.
        let free = |queued: usize| self.idle.load(Ordering::Relaxed) > queued;
        if !free(self.queued.load(Ordering::Relaxed)) {
            return false;
        }
        let boxed: Box<dyn FnOnce() + Send + 't> = Box::new(f);
        // SAFETY: the caller's contract above.
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

    /// Publish a split point for late joiners; wakes every parked worker.
    fn register_split(&self, sp: &SplitPoint) -> Option<usize> {
        use std::sync::atomic::Ordering;
        let p = sp as *const SplitPoint as *mut SplitPoint;
        for i in 0..SPLIT_SLOTS {
            if self.split_ptr[i]
                .compare_exchange(std::ptr::null_mut(), p, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // Taking the queue lock orders this against a worker that is
                // about to park: it either sees the slot or gets the notify.
                drop(self.q.lock().unwrap());
                self.cv.notify_all();
                return Some(i);
            }
        }
        None
    }

    /// Retract a split point. On return no worker holds the pointer without
    /// also holding a reservation in `sp.active`.
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

    /// Join a published split point and work it until it drains. Returns
    /// whether any search work was done. Slots are tried shallowest board
    /// first (fewest empties): those subtrees finish soonest, so less
    /// in-flight work is thrown away when a cutoff lands (`JOIN_DEEP=0`
    /// restores index order).
    fn try_help_splits(&self) -> bool {
        use std::sync::atomic::Ordering;
        let mut order: [(u8, u8); SPLIT_SLOTS] = [(u8::MAX, 0); SPLIT_SLOTS];
        if join_deepest() {
            for (i, o) in order.iter_mut().enumerate() {
                self.split_guard[i].fetch_add(1, Ordering::AcqRel);
                let p = self.split_ptr[i].load(Ordering::Acquire);
                if !p.is_null() {
                    // SAFETY: the guard keeps the owner from freeing the
                    // split point while this peek dereferences it.
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
            // SAFETY: the guard keeps the owner from freeing `sp` between
            // the load above and the `active` reservation below.
            let sp = unsafe { &*p };
            let cap = max_join();
            if cap != 0 && sp.active.load(Ordering::Relaxed) >= cap {
                self.split_guard[i].fetch_sub(1, Ordering::Release);
                continue;
            }
            sp.active.fetch_add(1, Ordering::AcqRel);
            self.split_guard[i].fetch_sub(1, Ordering::Release);
            // SAFETY: the reservation keeps `sp` alive until released.
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

    /// Run one queued task if there is one. Used by a thread that is waiting
    /// for a task of its own: without it, every worker could end up blocked on
    /// sub-tasks that only a worker could run.
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

    /// Wait for `done`: first try to run something from the queue, and only
    /// park once there is nothing left to steal.
    ///
    /// Stealing before parking is what keeps nesting deadlock-free — without
    /// it every worker could end up waiting on sub-tasks that only a worker
    /// could run. Parking is safe because a task is only ever accepted when a
    /// worker is idle to take it, so anything queued does get picked up, and
    /// `unpark` before `park` is remembered rather than lost.
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
                // A published split point is work too; only park when
                // neither the queue nor the registry has any. The register
                // path takes the queue lock before notifying, so a slot
                // published after this scan cannot slip past the wait.
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

/// Slack below the node's alpha for the ordering lookahead's window.
const SORT_ALPHA_DELTA: i32 = 12;
/// Ordering weight of one stable edge disc, in eighths of a disc.
const EDGE_STABILITY_ORDER_WEIGHT: i32 = 1;
/// Half-width of the first aspiration window around the warm-up score.
const ASPIRATION_WIDTH: i32 = 6;

/// `DBG_ASP=1` traces the warm-up rungs and every aspiration window to
/// stderr — the tool that told apart "warm score wobbles" from "same score,
/// different tree" when chasing the bimodal FFO49 solve.
fn dbg_asp() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("DBG_ASP").is_ok_and(|v| v != "0"))
}
/// Half-width used by the warm-up passes, whose centre is only an
/// evaluation estimate rather than a searched score.
const WARM_ASPIRATION_WIDTH: i32 = 6;

/// `WARM_ASP_WIDTH=<n>` overrides [`WARM_ASPIRATION_WIDTH`] for sweeps.
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

/// Depth of the evaluation search that centres the first warm-up window.
const ESTIMATE_DEPTH: u8 = 6;
/// Warm-up passes only prune at this many empties or more.
///
/// Fourteen leaves the whole bottom of the tree — where nearly all the nodes
/// are — searched exactly, so a "selective" solve at 30 empties is only
/// selective in its top sixteen plies.
// 14 -> 10 (2026-07-30): with the static-eval gate below, the probes at
// 10-13 empties are cheap enough that pruning there finally pays — measured
// on 12 fixed 29-empty positions: 2,798M -> 2,061M nodes, 112.9s -> 95.4s.
// Without the gate the same setting *loses* time (133.7s), which is why it
// was rejected before; the two knobs only work together.
const SELECTIVE_MIN_EMPTIES: u8 = 10;

/// Roots at or above this many empties run the warm-up ladder before the exact
/// pass. Tuned to 24 under the old sigma; the measured sigma halves the rungs'
/// margins, so where the ladder starts paying for itself needs re-measuring.
/// The warm-up rungs, overridable for sweeps (`SEL_LADDER=1.55` or `1.1,1.8`).
/// Two rungs, tuned on FFO positions deep enough that both pay. Where the
/// ladder starts and how many rungs it has are the same trade against the
/// same clock, so they are tuned together.
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

/// Depth of the evaluation probe used by a warm-up pass.
///
/// 2, not the 4 this used to be. A warm-up rung exists to fill the table, not
/// to answer, so the probe only has to order moves and bound them well enough
/// for the exact pass that follows; at depth 4 it was spending 4.7x as many
/// nodes per probe as the answer needed. Probe searches alone sampled at 12.2%
/// of wall clock on FFO40-49 and 29.2% on a 22-empty set.
///
/// Sweep of 2/3/4 at one thread, minima of alternating runs (negative is
/// faster than the old 4):
///
/// ```text
///   set              pd2      pd3
///   18 empties      0.0%     0.0%
///   20 empties     +1.4%    +2.9%
///   22 empties    -27.7%   -25.9%
///   24 empties    -14.3%   -19.5%
///   26 empties     -2.4%    -7.5%
///   band22        -17.1%   -15.5%
///   FFO40-49      -12.4%    +1.8%
/// ```
///
/// Why 2 and not 3: 3 is the better value on the 24- and 26-empty sets, but it
/// is *slower than the old 4* on FFO40-49, the set closest to real work. The
/// knob is not monotone — probe parity matters, so these must be swept rather
/// than interpolated — and 3 wins or loses by set rather than being better.
/// 2's worst case anywhere is +1.4% (20 empties) while it wins the sets that
/// matter, so a flat 2 is the honest choice; a depth that varies with the
/// empty count may beat it but is a structural change needing its own case.
///
/// This does not touch exactness: the rungs only warm the table and the exact
/// pass still decides. It is also inside the `selective_sigma` fit (2-10
/// plies), and the depth-2 margins measure *more* conservative than depth 4's
/// against exact solves (14.4% of probes outside 1.1 sigma against 16.7%, on
/// a 90-position sample), so the shallower probe is not buying its speed by
/// pruning harder than the calibration supports.
const SELECTIVE_PROBE_DEPTH: u8 = 2;

/// Standard deviation of `exact - probe` for a warm-up probe of `pc` plies at
/// `empties` empty squares — *measured*, not extrapolated.
///
/// The margin a warm-up pass prunes against is `t * sigma`, so sigma decides
/// whether a selective solve is selective at all. It used to borrow
/// `search::mpc_sigma`, which is fitted on midgame searches of a fixed depth
/// and grows about a quarter of a disc per ply without bound. Extended to an
/// endgame search, where the depth *is* the empty count, that model is wrong in
/// both directions: at 30 empties it claimed 8.4 against a measured 4.2, so the
/// margin was 15 discs and essentially nothing was ever cut (2.8 G nodes for
/// one position against Egaroucid's 85 M); at 14 empties with a 10-ply probe it
/// claimed 2.2 against a measured 3.2, and cut things it should not have.
///
/// Fitted on 1,550 positions at 14-30 empties, probes of 2-10 plies, each
/// against an exact solve of the same position (`--sigma-calib`). Worst cell is
/// 18% high, most are inside 6%. Outside that range the fit is not evidence, so
/// it is clamped rather than extrapolated.
fn selective_sigma(empties: u8, pc: u8) -> f32 {
    if legacy_sigma() {
        return crate::search::mpc_sigma(empties as u32, empties, pc);
    }

    let e = (empties as f32).clamp(14.0, 30.0);
    let p = (pc as f32).clamp(2.0, 10.0);
    let s = 7.246577 + 0.020516 * e - 0.438464 * p - 0.004024 * e * e
        + 0.000167 * p * p
        + 0.009864 * e * p;
    // A margin cannot sensibly go below a disc, and the fit is only linear-ish
    // near its edges.
    s.max(1.0)
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

/// Depth of a warm-up pass's probe, optionally scaled with the position.
///
/// The scaled shape probes at `depth / 3 + parity`, so a solve at 29 empties
/// asks a 9-ply question. The flat shape asks a 4-ply one at every depth: the
/// deeper the position, the more the probe is guessing, and the wider
/// `t * sigma` has to be to stay honest. That flat 4 was chosen for the exact
/// solve's ladder, where the rungs exist to fill the table rather than to
/// answer, so it stays the default until measured.
fn selective_probe_depth(empties: u8) -> u8 {
    // SEL_PROBE_DEPTH=<n> pins a flat probe depth for sweeps.
    static V: std::sync::OnceLock<Option<u8>> = std::sync::OnceLock::new();
    if let Some(d) = *V.get_or_init(|| {
        std::env::var("SEL_PROBE_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
    }) {
        return d;
    }
    // SEL_PROBE_STEP=<empties>:<depth> probes deeper from that many empties
    // up, keeping the flat depth below (sweep switch: a deep node's flat
    // probe is guessing, but paying scaled depth everywhere loses on time).
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
    // Per-solve step, armed by `solve_impl` for deep roots (`SEL_DEEP_ROOT`).
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

/// Per-solve probe-depth step (`at << 8 | depth`, 0 = off), armed by
/// `solve_impl` when the root is at least `SEL_DEEP_ROOT` empties: a flat
/// depth-2 probe pays everywhere except on deep solves, whose upper region
/// needs a deeper question to cut anything.
static PROBE_STEP: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// `u8::MAX` until resolved: the environment supplies the default, and a caller
/// scoring both probe shapes on one position overrides it between solves.
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

/// Choose the probe shape for the solves that follow, overriding the
/// environment. For measuring both on the same position.
pub fn set_selective_probe_scaled(on: bool) {
    PROBE_SCALED.store(on as u8, std::sync::atomic::Ordering::Relaxed);
}

/// How far outside the window the static evaluation must already be before a
/// warm-up pass pays for its probe search, as a discount off the probe's own
/// margin.
///
/// Without the gate every node in the band buys up to two probe searches
/// whether or not one could possibly cut; the static pre-check is the
/// cheapest half of a ProbCut.
fn selective_gate_offset() -> Option<f32> {
    // On by default at 4 (measured best of 2..6 here); SEL_GATE=off
    // disables, SEL_GATE=<n> overrides.
    static V: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *V.get_or_init(|| match std::env::var("SEL_GATE").ok().as_deref() {
        Some("off") | Some("-") => None,
        Some(v) => v.parse().ok().filter(|v: &f32| *v >= 0.0).or(Some(4.0)),
        None => Some(4.0),
    })
}
/// Roots this deep run the warm-up ladder before the exact pass. Below 20
/// empties the exact search is cheap enough that the pass cannot pay for
/// itself (measured on FFO1-19: +6% nodes and +24% time at 16, +14%/+62%
/// at 14), while from 20 up it pays for itself several times over.
///
/// 22 rather than 24: on game-reached 22-empty positions the ladder cuts the
/// exact tree from 42M to 24M nodes a position and 14% of the time, and it is
/// most of why the tree matched Egaroucid's on FFO positions deep enough to
/// run it (without it, 22-23-empty FFO problems were 3x its node count).
/// FFO40-49 is unchanged (5.33s vs 5.45s minima over five rounds). Measured
/// with the fitted `selective_sigma`; the FFO1-19 numbers above predate it.
///
/// 18 rather than 22 (2026-08-27): the floor above was set with *linear*
/// probes. With the NNUE lent to the warm-up the ladder pays two rungs
/// lower — fixed-20-empty positions: -24% nodes, -15% wall; fixed-18:
/// -32% nodes, -3.9% wall (quiet window, alternating minima). The FFO1-19
/// numbers above predate the NNUE probes; 16 has not been re-measured.
const SELECTIVE_PASS_MIN_EMPTIES: u8 = 18;
/// Confidence levels (standard deviations) of the warm-up passes, from
/// most selective to least. Each pass is aspirated around the previous
/// pass's score, so the estimate handed to the exact search converges —
/// an estimate that is off by even two discs makes the exact search pay
/// for a failed window, which is the dominant cost on hard positions.
/// The two rungs are optimal at every thread count, not just sequentially.
/// FFO40-49, 2 rounds, min: `[1.1, 1.8]` 21.28s / 4.88s (1 / 10 threads),
/// `[1.8]` 21.80 / 5.17, `[1.1]` 22.98 / 5.01, `[1.1, 1.5, 1.8]` 21.94 / 5.16,
/// `[0.8, 1.4, 2.0]` 22.30 / 5.09. Worth re-checking because the ladder
/// parallelises worse than the exact pass it feeds (3.21x against 5.28x on 10
/// threads, so its share of the solve grows from 20% to 28%) — but the fix is
/// not fewer rungs.
const SELECTIVE_LADDER: [f32; 2] = [1.1, 1.8];

/// Plies of ordering lookahead by empty count, stepped one ply at a time
/// as the position opens up.
const SORT_DEPTH_LADDER: [u8; 64] =
    build_sort_ladder([DEEP_ORDER_EMPTIES, DEEP2_ORDER_EMPTIES, DEEP3_ORDER_EMPTIES]);

/// Empty counts at which the lookahead gains its first, second and third ply.
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

/// `SORT_LADDER=<a>,<b>,<c>` replaces the three step points for sweeps.
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

/// One square's flip under the selected arm.
///
/// The two modifiers are orthogonal, so the features compose rather than
/// naming arms: `flip-noshare` drops the shared board broadcast and pays
/// `BoardCtx::new` per square, `flip-guard` adds back the adjacency test a
/// non-empty flip implies anyway. Against the batched default that gives
/// five points, and the fifth is what makes the decomposition checkable:
///
/// | arm | features | setup | timing | guard |
/// |---|---|---|---|---|
/// | A | (none) | shared | eager | no |
/// | D | `flip-lazy` | shared | lazy | no |
/// | E | `flip-noshare` | per square | lazy | no |
/// | B | `flip-guard` | shared | lazy | yes |
/// | C | `flip-guard,flip-noshare` | per square | lazy | yes |
///
/// `A - D` is the cost of computing flips a cutoff never needs, `D - B` the
/// guard's, and sharing the setup is worth `D - E` without the guard and
/// `B - C` with it. Those last two agreeing is what says the three effects
/// are additive - and a single recorded number for all three only
/// generalizes if they are.
#[cfg(all(
    any(
        feature = "flip-lazy",
        feature = "flip-guard",
        feature = "flip-noshare"
    ),
    not(feature = "flip-noshare")
))]
#[inline(always)]
fn arm_flip(ctx: &bitboard::FlipCtx, _p: u64, o: u64, sq: u8) -> u64 {
    if cfg!(feature = "flip-guard") && o & bitboard::neighbours(sq) == 0 {
        return 0;
    }
    ctx.flip(sq)
}

#[cfg(feature = "flip-noshare")]
#[inline(always)]
fn arm_flip(_ctx: &bitboard::FlipCtx, p: u64, o: u64, sq: u8) -> u64 {
    if cfg!(feature = "flip-guard") && o & bitboard::neighbours(sq) == 0 {
        return 0;
    }
    bitboard::flippable(p, o, 1u64 << sq)
}

/// The child's hash, computed now if `gen_moves` deferred it.
///
/// Deferring is the default: the eager hash exists to feed a prefetch, and
/// at seven, eight and nine empties there is none to feed. Measured on
/// FFO40-49 at one thread, ten shuffled rounds, tree byte-identical:
/// -0.85%, and the deferred build was faster in all ten. `gen-eager-hash`
/// restores the old shape for re-measurement.
#[inline(always)]
fn child_hash_of(child: &Board) -> u64 {
    zobrist::board_hash(child.player_bb(), child.opponent_bb())
}

/// Squares adjacent to each square. The 5-6 loop's prefilter that used
/// this is gone (batched flips subsume it); the test below keeps the mask
/// definition honest for any future reader.
#[cfg(test)]
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
    // Branchless: the file crosses the middle exactly when bit 5 of the
    // square is set (file = sq >> 3, so file >= 4 is sq >= 32), and the
    // rank exactly when bit 2 is. The pair indexes the four quadrant bits
    // in the order the old match spelled out: (near, near) = 1,
    // (far, near) = 2, (near, far) = 4, (far, far) = 8. Called on every
    // descent through the last four layers and again inside their parity
    // permutations, so the divide, the modulo and the four-way match were
    // all on the hottest path in the search.
    1u8 << (((sq >> 5) & 1) | (((sq >> 2) & 1) << 1))
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
#[repr(C, align(32))]
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
    /// All-zero, so a fresh table is calloc's lazily-zeroed pages instead
    /// of a 12 MB pattern fill per solve. Safe because every read is gated
    /// on `used()` / a board match, and every write fills the whole entry.
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

    /// Seed entries come from the evaluation-guided pre-search: their move
    /// is a good ordering hint but their bounds are heuristic and must
    /// never cut the exact search.
    #[inline]
    fn is_seed(&self, now: u8) -> bool {
        if self.flags & 4 != 0 {
            return true;
        }
        if self.date == now {
            return false;
        }
        // A bound whose subtree never took a probabilistic cut is an exact
        // alpha-beta fact, so generation demotion may spare it: either the
        // whole layer is structurally probe-free (`exact_keep_floor`) or the
        // search proved this entry clean (`PROVEN`, see `EXACT_PROOF`).
        if self.flags & Self::PROVEN != 0 {
            return false;
        }
        self.depth >= exact_keep_floor()
    }

    /// Flag bit: the stored bounds come from a subtree in which no selective
    /// probe fired and no unproven table bound was consumed.
    const PROVEN: u8 = 8;

    /// Whether this entry's bounds are exact facts independent of the rung
    /// that produced them: proven dynamically, or below the layer where the
    /// selective probes can fire at all.
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
        // Branchless: fold both board words and the used/colour bits into
        // one comparison, so a probe is three loads and a single test
        // instead of a chain of compares.
        let want = 1 | ((board.player as u8) << 1);
        ((self.black ^ board.black) | (self.white ^ board.white)) == 0 && (self.flags & 3) == want
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
        // Probing is confined to the low part of the table while the
        // allocation - and therefore the per-position wipe - stays full
        // size. This is the control that separates the two things a
        // smaller table does at once: probes land in less memory (a real
        // win) and the wipe touches less memory (which evicts less of
        // everything else, and lands on the *next* position's timed run
        // rather than this one's). Shrinking the table gets both; this
        // gets only the first. If it reproduces the whole gain, capacity
        // is what to cut; if it does not, the wipe is, and the fix is to
        // stop wiping - `HashEntry` already carries a generation.
        #[cfg(feature = "tt-probe-clamp")]
        const CLAMP_BUCKET_BITS: u32 = 22;
        HashTable {
            #[cfg(feature = "tt-probe-clamp")]
            mask: (((size >> 1) - 1) as u64).min((1u64 << CLAMP_BUCKET_BITS) - 1),
            #[cfg(not(feature = "tt-probe-clamp"))]
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

    /// Empty the table, splitting the write across `threads` threads.
    ///
    /// At the sizes an endgame needs this is not a bookkeeping detail: 2^26
    /// entries is 2.1 GB, and writing it on one thread took 41 ms per position
    /// — 1.9% of a one-thread solve but 8.1% of a ten-thread one, with the
    /// other nine threads necessarily idle throughout.
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
                    flags: 1 | ((board.player as u8) << 1) | proven_bit,
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
            entry.flags = 1 | ((board.player as u8) << 1) | proven_bit;
            entry.best8 = best8;
            entry.date = now;
            return;
        }
        // Tightening mixes the stored bounds with the new ones, so the entry
        // stays proven only when both sides are.
        if proven_bit == 0 {
            entry.flags &= !HashEntry::PROVEN;
        }
        if value < beta && (value as i8) < entry.upper8 {
            entry.upper8 = value as i8;
        }
        if value > alpha && (value as i8) > entry.lower8 {
            entry.lower8 = value as i8;
        }
        // A fail-low store carries no move; keep the one already there
        // rather than erasing it: a stale-but-real move orders better than
        // none.
        if best8 != 255 {
            entry.best8 = best8;
        }
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

/// The NNUE midgame searcher lent to the endgame for its selective probes:
/// the network and the shared midgame table it searches through.
/// Move ordering through the network instead of the linear table, for
/// measurement only.
///
/// Ordering calls the evaluator once per child, so its cost is multiplied by
/// the branching factor -- which is why it reads the 8-bit ordering tables
/// and not the full evaluation. Whether the network would order *better* is
/// a separate question from whether it could afford to, and node counts
/// answer the first one on its own. Gated on an environment variable so one
/// binary measures both arms; nothing reads it unless it is set.
pub static ORDER_NNUE: std::sync::OnceLock<&'static crate::nnue::Nnue> = std::sync::OnceLock::new();

/// The indexer the carried ordering indices follow: the network's own
/// pattern set when the ordering arm reads the network, else the linear
/// evaluator's. The two sets differ, so feeding one's indices to the other
/// would score noise.
fn order_indexer(e: &crate::evaluator::Evaluator) -> &crate::pattern_index::PatternIndexer {
    match order_nnue() {
        Some(nn) => nn.indexer(),
        None => e.indexer(),
    }
}

pub fn order_nnue() -> Option<&'static crate::nnue::Nnue> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var("KUROOBI_NNUE_ORDER").is_ok()) {
        ORDER_NNUE.get().copied()
    } else {
        None
    }
}

pub type NnueProbe = (
    &'static crate::nnue::Nnue,
    &'static crate::midgame::SharedTt,
);

/// The selective probes run an (unpruned) NNUE search instead of the linear
/// seed search whenever the solver has been lent one (`set_nnue`). Measured
/// on 12 fixed 29-empty positions (1T): 2,061M -> 1,829M nodes and 95.4s ->
/// 66.7s — the probe is both a little more accurate (sigma ~0.9x) and much
/// cheaper per call. Deeper NNUE probes (5/6/8/depth-3) all lose on time.
/// `SEL_NNUE_PROBE=0` restores the linear probes.
#[cfg(feature = "tunable")]
fn sel_nnue_probe() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SEL_NNUE_PROBE").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn sel_nnue_probe() -> bool {
    true
}

/// The exact solve's warm-up ladder probes with the NNUE too (`SEL_NNUE_WARM=0`
/// restores the linear probes). This lost under the H=16 model; re-measured
/// under H=32 it wins on wall clock (quiet window, alternating minima):
/// band22 -13.2%, fix20 -15.1%, fix24 -4.7%, FFO40-49 -5.9%, with fix22 +4.1%
/// the one loss. The probe depth question is separate — see `SEL_DEEP_ROOT`.
#[cfg(feature = "tunable")]
fn sel_nnue_warm() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("SEL_NNUE_WARM").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn sel_nnue_warm() -> bool {
    true
}

/// Entries below the structural probe floor are spared the between-rung
/// demotion: a probe can only fire in `pvs` at `selective_min_empties` or
/// more, so bounds proved below that are selectivity-independent facts the
/// exact pass may reuse. Worth -1% (FFO) to -8% (deep26) of the total tree
/// on top of the NNUE warm-up probes. `EXACT_KEEP_SHALLOW=0` restores the
/// blanket demotion.
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

/// `EXACT_REOPEN_CLAMP=<d>` caps the exact pass's reopened aspiration window
/// at `warm score +/- d` (sweep switch; unset keeps the full converged
/// window).
fn exact_reopen_clamp() -> Option<i32> {
    static V: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EXACT_REOPEN_CLAMP")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|d: &i32| *d > 0)
    })
}

/// Probe/store a dedicated four-empty cache in the five-empty loop
/// (`EXACT_L4_CACHE=0` disables). Dedicated because the shallow table's
/// depth-preferred replacement never lets a four-empty entry evict a 5-6
/// one, so routing them there measured zero hits. This one is direct-mapped,
/// overwrites on position mismatch, and merges a (lower, upper) pair per
/// board so one entry serves many windows. FFO40-49 nodes -7.3% alone;
/// combined with the revived four-empty stability cut, total -26% against
/// the pre-cache tree (solutions identical on all seven sets).
#[cfg(feature = "tunable")]
fn l4_cache() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("EXACT_L4_CACHE").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn l4_cache() -> bool {
    true
}

/// One four-empty cache entry: the position (side-to-move relative, like
/// every board below the colour boundary) and its proven bound pair.
/// Packed to its 19 content bytes rather than padded out to a 32-byte
/// slot. The slot used to be aligned so that an entry never straddled a
/// cache line, on the theory that a straddle doubles a random probe's
/// latency. That buys single-line probes at the price of 40% of the
/// table: at 19 bytes the same footprint holds 1.68x the entries, and
/// this band is footprint-bound, not latency-bound - the 7-8 layer
/// measured 256KB beating 2MB because the larger table's stores evicted
/// everything around them. Fewer bytes for the same entries pushes on
/// exactly that axis. Unaligned loads are cheap on aarch64.
/// Three layouts, because this is a two-sided question and neither side has
/// measured it: `entry-align32` restores the padded slot, `entry-natural`
/// takes the 24-byte natural alignment as a middle arm, and the default is
/// packed. Build all three and sweep; do not assume.
#[derive(Clone, Copy)]
#[cfg_attr(feature = "entry-align32", repr(C, align(32)))]
#[cfg_attr(feature = "entry-natural", repr(C))]
#[cfg_attr(
    not(any(feature = "entry-align32", feature = "entry-natural")),
    repr(C, packed)
)]
struct L4Entry {
    player: u64,
    opponent: u64,
    lower: i8,
    upper: i8,
    /// Best move + 1; 0 = none, so calloc's zero pages decode as "none".
    best8: u8,
}

/// The whole point of each layout is its byte count, so state it where a
/// stray field or a repr change would trip over it.
///
/// The table sizes are knobs in *entries* (`l4_bits` and friends), and the
/// index is a mask, so an entry count is always a power of two. That fixes
/// what a comparison between these layouts means: at equal `bits` the arms
/// hold the same entries and differ only in footprint (packed is 59% of
/// the padded slot). The other reading - equal footprint, 1.68x the
/// entries - is not reachable by a mask-indexed table, since 1.68x is not
/// a power of two; it needs the byte-budget-plus-multiply-index shape, and
/// that changes the index function too, which moves the tree. Keep the two
/// questions apart: sweep `bits` per layout, and read the curves against
/// each other rather than reading one point.
const ENTRY_BYTES: usize = std::mem::size_of::<L4Entry>();
const _: () = assert!(if cfg!(feature = "entry-align32") {
    ENTRY_BYTES == 32
} else if cfg!(feature = "entry-natural") {
    ENTRY_BYTES == 24
} else {
    ENTRY_BYTES == 19
});

/// A zeroed slot matches no real position (both sides empty), and its bound
/// bytes are never read before a store rewrites the whole entry - so the
/// table can come straight from calloc's zero pages.
#[allow(dead_code)]
/// Composition counters for the 5-6 band (layer-profile builds only):
/// how many of each unit a node pays for, to divide the band's ns/node.
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
    pub static STAB_BETA: AtomicU64 = AtomicU64::new(0);
    pub static STAB_GATE_A: AtomicU64 = AtomicU64::new(0);
    pub static STAB_GATE_B: AtomicU64 = AtomicU64::new(0);
    // 7-11 band (alpha_beta_ordered)
    pub static O_NODES: AtomicU64 = AtomicU64::new(0);
    pub static O_GEN_MOVES: AtomicU64 = AtomicU64::new(0);
    pub static O_SCORED: AtomicU64 = AtomicU64::new(0);
    pub static O_TT_PROBE: AtomicU64 = AtomicU64::new(0);
    pub static O_TT_CUT: AtomicU64 = AtomicU64::new(0);
    pub static O_CHILD: AtomicU64 = AtomicU64::new(0);
    pub static O_STAB: AtomicU64 = AtomicU64::new(0);
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

/// An all-zero-initialised Vec straight from `alloc_zeroed`: the kernel
/// hands back zero pages lazily, so a table the search only partially
/// touches never pays a full memset.
fn zeroed_vec<T: Copy>(len: usize) -> Vec<T> {
    let layout = std::alloc::Layout::array::<T>(len).expect("table size overflow");
    // SAFETY: T is Copy (no drop), the layout matches len elements, and
    // alloc_zeroed's bytes are a valid all-zero T for the entry types used
    // here (plain integers behind repr(C)).
    unsafe {
        let p = std::alloc::alloc_zeroed(layout) as *mut T;
        assert!(!p.is_null(), "table allocation failed");
        Vec::from_raw_parts(p, len, len)
    }
}

/// Index bits of the four-empty cache (2^16 x 24 B = 1.5 MiB per worker;
/// `EXACT_L4_BITS` overrides for sweeps. 14 gives back ~1% of the nodes).

#[cfg(feature = "tunable")]
fn l4_bits() -> u32 {
    static V: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("EXACT_L4_BITS")
            .ok()
            .and_then(|v| v.parse().ok())
            // 2^14 x 32 B = 512 KiB. 2^16 held 0.64% more of the tree but
            // the extra misses cost more than the nodes: FFO40-49 -2.1%
            // wall clock, deep26 -4.6% at this size. The thread scaling
            // below still clamps 8 threads to 2^13, so only 1-4 threads
            // change.
            .unwrap_or(14)
    })
}
#[cfg(not(feature = "tunable"))]
fn l4_bits() -> u32 {
    14
}

/// Also probe/store the overwrite-always cache at 5-6 empty entries: the
/// shallow table's depth-preferred replacement never lets a 5 evict a 6
/// either (`EXACT_L56_CACHE=0` disables).
/// `NO_SHALLOW56=1` drops the shallow table's probe and store at 5-6
/// empties, leaving the unified overwrite cache as the band's only layer
/// (consolidation experiment; pass condition is no node increase).
#[cfg(feature = "tunable")]
fn no_shallow56() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NO_SHALLOW56").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn no_shallow56() -> bool {
    false
}

/// `NEW_SHALLOW=1` swaps the 5-6 band's depth-preferred 2-way shallow table
/// for a direct-mapped overwrite cache with merged bound pairs (the shape
/// that won at four empties).
#[cfg(feature = "tunable")]
fn new_shallow() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NEW_SHALLOW").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn new_shallow() -> bool {
    true
}

/// `NEW_MID=1` swaps the 9-empty layer's 2-way mid table for the same
/// direct-mapped overwrite shape (experiment; node ceiling applies).
#[cfg(feature = "tunable")]
fn new_mid() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NEW_MID").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn new_mid() -> bool {
    false
}

/// Index bits of the 9-empty cache (`NEW_MID_BITS`, default 18 = 8 MiB).
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

/// `NEW_78=1` (default): give the 7-8 layers the same one-way pair table
/// the 5-6 band has. They sit below `tt_min_empties()` and had no
/// transposition reuse at all, which showed up as a swollen 7-11 layer on
/// the positions with the worst node ratio (FFO#44).
#[cfg(feature = "tunable")]
fn new_78() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("NEW_78").map_or(true, |v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn new_78() -> bool {
    true
}

/// Index bits of the bound cache below `TT_MIN_EMPTIES` (`L78_BITS`,
/// default 15 = 1 MiB).
///
/// It was 13 while the cache covered two layers, where 2^16 found 1.7x the
/// node savings for +5.8% wall clock - the store traffic evicted the L2
/// lines the leaf machinery lives on. The band now runs to ten empties, so
/// four layers share it, and the sweep moved with them: 15 is -1.0% and
/// -0.6% nodes against 13, with 14, 16 and 17 all worse.
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
#[cfg(all(not(feature = "tunable"), feature = "ec-band"))]
fn l78_bits() -> u32 {
    // Six layers to cover instead of two, so 4 MiB rather than the 256 KiB
    // that suits 7-8 alone.
    18
}
#[cfg(all(not(feature = "tunable"), not(feature = "ec-band")))]
fn l78_bits() -> u32 {
    15
}

/// Index bits of the 5-6 cache (`NEW_SHALLOW_BITS`, default 14 = 512 KiB).
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
    // Default off since the four-empty table shrank to 512 KiB: 5-6 entries
    // rooming in it evict the four-empty entries that earn more, so the
    // shared tenancy now *adds* nodes (-0.27% FFO40-49, -0.17% fix22,
    // -0.15% deep26 without it) and its entry probe was paid at every 5-6
    // node. The dedicated NEW_SHALLOW table keeps covering 5-6.
    *V.get_or_init(|| std::env::var("EXACT_L56_CACHE").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn l56_cache() -> bool {
    false
}

/// `EXACT_PROOF=1` tracks, per warm-up node, whether any selective cut (or
/// any bound derived from one) entered its subtree; untainted results are
/// stored with [`HashEntry::PROVEN`] and survive the between-rung demotion
/// as exact facts. Off by default (sweep switch).
#[cfg(feature = "tunable")]
fn exact_proof() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("EXACT_PROOF").is_ok_and(|v| v != "0"))
}
#[cfg(not(feature = "tunable"))]
fn exact_proof() -> bool {
    false
}

/// The layer below which a selective probe can never fire: probes run only in
/// `pvs` (>= `pvs_limit()` empties) and only at `selective_min_empties` or more,
/// so every entry below this depth is probe-free by construction.
#[cfg(feature = "tunable")]
fn structural_proof_floor() -> u8 {
    static V: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *V.get_or_init(|| pvs_limit().max(selective_min_empties()))
}
#[cfg(not(feature = "tunable"))]
fn structural_proof_floor() -> u8 {
    pvs_limit().max(SELECTIVE_MIN_EMPTIES)
}

/// Layout probe: where the hot fields actually sit. `repr(Rust)` orders by
/// size and alignment, not by how often a field is touched, so the node
/// counter and the bound caches can end up behind the two inline hash
/// tables - and every access is then a large offset from one base pointer.
#[cfg(feature = "layer-profile")]
pub fn worker_layout() -> Vec<(&'static str, usize)> {
    let tt = HashTable::new(10);
    let budget = ThreadBudget::new(0);
    let abort = AbortFlag::root();
    let w = Worker::new(&tt, &budget, &abort);
    let base = &w as *const _ as usize;
    vec![
        ("size_of", std::mem::size_of::<Worker>()),
        ("nodes", (&w.nodes as *const _ as usize) - base),
        ("l4", (&w.l4 as *const _ as usize) - base),
        ("l56", (&w.l56 as *const _ as usize) - base),
        ("l78", (&w.l78 as *const _ as usize) - base),
        (
            "shallow_table",
            (&w.shallow_table as *const _ as usize) - base,
        ),
        ("mid_table", (&w.mid_table as *const _ as usize) - base),
        ("tt", (&w.tt as *const _ as usize) - base),
        ("g_l4_cache", (&w.g_l4_cache as *const _ as usize) - base),
    ]
}

pub struct Solver {
    /// Shared between all search threads: the transposition table (its
    /// entries are guarded by striped locks).
    hash_table: HashTable,
    /// Results of the last solve, copied back from the worker.
    nodes: u64,
    best: Option<Position>,
    /// Search threads to split the root across (1 = sequential).
    threads: usize,
    /// See [`NnueProbe`]; `None` keeps the linear probes.
    nnue: Option<NnueProbe>,
    /// The root worker's private tables, kept across solves so their
    /// allocation and per-solve clearing sit outside the timed search — the
    /// same convention as the main table (and as the engines we compare
    /// against, which allocate their thread-local caches once at startup).
    scratch: Option<(HashTable, HashTable, Vec<L4Entry>)>,
    /// The worker pool, spawned once and woken per solve, so neither thread
    /// creation nor teardown is billed to (or serialised into) a solve.
    budget: Option<std::sync::Arc<ThreadBudget>>,
    budget_threads: usize,
    budget_handles: Vec<std::thread::JoinHandle<()>>,
    /// External (UI) stop handle; when set the search returns immediately.
    stop: Option<crate::midgame::StopHandle>,
}

/// One search thread's private state plus borrows of the shared tables.
/// Every search routine lives here, so spawning another thread is just
/// another `Worker` over the same `HashTable`.
/// Field order here is not the layout: `repr(Rust)` sorts by size and
/// alignment, which puts the node counter at offset 272 and the cache flags
/// at 322, behind two inline hash tables the exact search barely touches.
///
/// Pinning the layout with `repr(C)` and hoisting the hot fields to the
/// front was measured and is worse: it moved `nodes` to 0 and `l4` to 8,
/// but grew the struct from 328 to 336 bytes, and the four sets came out
/// +4.46%, +0.79%, +0.61%, +0.58% (total +0.90%). The packing `repr(Rust)`
/// finds is worth more than the smaller offsets.
struct Worker<'a> {
    nodes: u64,
    /// Four-empty cache (`EXACT_L4_CACHE`), empty when the switch is off.
    l4: Vec<L4Entry>,
    /// Direct-mapped overwrite cache for 5-6-empty entries (`NEW_SHALLOW`),
    /// replacing the depth-preferred shallow table's probe there.
    l56: Vec<L4Entry>,
    l78: Vec<L4Entry>,
    /// Env-gate snapshots. The `OnceLock` getters cost an acquire load and
    /// an init-check branch per call; the hot paths read these plain bools
    /// instead (several per node at 5-6 empties).
    g_l4_cache: bool,
    g_l56_cache: bool,
    g_new_shallow: bool,
    g_no_shallow56: bool,
    g_new_mid: bool,
    /// Highest empty count still served by `mid_table`, exclusive.
    mid_empties: u8,
    /// Pattern indices for the evaluation-based move ordering, carried down
    /// the tree instead of rebuilt at every node. Building them from the
    /// bitboards reads every cell of all 64 masks and measured 333 ns, once
    /// per node of the band that orders by evaluation - more than the
    /// ordering evaluation it feeds. A move changes them by the same CSR
    /// walk the ordering already does per candidate, so carrying them is a
    /// snapshot and a restore around each child that needs them.
    order_ix: crate::pattern_index::PatternIndices,
    tt: &'a HashTable,
    /// Cutoff signal for the split this worker is searching under.
    abort: &'a AbortFlag<'a>,
    /// Spare threads this search may recruit when splitting a node.
    budget: &'a ThreadBudget,
    /// External stop (UI), checked together with the per-node abort flag.
    stop: Option<&'a crate::midgame::StopHandle>,
    best: Option<Position>,
    /// Score returned by the warm-up pass, used to centre the exact
    /// search's aspiration window (carrying the score between passes —
    /// that, not the warmed table, is the main benefit).
    warm_score: Option<i32>,
    /// The window the last aspiration converged in. The exact pass reopens
    /// *this* window rather than re-centring on the warm score: the seed
    /// ordering the warm-up left describes the proof tree of its own window,
    /// and a window shifted even 2 discs makes deep nodes fail the other way
    /// than their seeds prepared for — measured 8x on FFO49 (67M -> 530M)
    /// whenever the warm rung landed exactly on the true value.
    warm_window: Option<(i32, i32)>,
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
    /// See [`NnueProbe`]; copied from the solver into every worker.
    nnue: Option<NnueProbe>,
    /// Bumped whenever a warm-up node takes a selective cut or consumes an
    /// unproven table bound; a subtree whose search left it unchanged is an
    /// exact alpha-beta fact (see `EXACT_PROOF`).
    taint: u64,
    /// Same shape one band up (`NEW_MID`): the 9-empty layer, bounds plus a
    /// best-move byte, replacing the 2-way mid table's probe there.
    l9: Vec<L4Entry>,
    /// Margin multiplier for the selective cuts. 1.0 for exact solves; a
    /// selective *answer* runs at 0.6 — calibrated-sigma margins turned out
    /// to buy far more certainty than the answer needs (measured on two
    /// out-of-sample 12-position sets at 29 empties: 0.6 matches
    /// Egaroucid's 93% answer error at 0.85-0.94x their time, while 1.0 is
    /// 4x slower for 1-4 discs less error). `SEL_SIGMA_SCALE` overrides.
    sigma_scale: f32,
}

impl<'a> Worker<'a> {
    fn new(tt: &'a HashTable, budget: &'a ThreadBudget, abort: &'a AbortFlag<'a>) -> Worker<'a> {
        let (sb, mb) = private_tt_bits();
        Worker::with_tables(tt, budget, abort, HashTable::new(sb), HashTable::new(mb))
    }

    /// Build a worker over tables it does not own the allocation of, so a
    /// helper thread can adopt a recycled pair instead of zero-filling 14 MB.
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
                // Per-worker caches share one L2, so divide the table by
                // the thread count. 2^16 x 32 B at
                // one thread; 8 threads at 2^16 measured 3.35s on FFO40-49
                // against 2.83s at 2^13 (-15.5%).
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

    /// Warm the 7-12 cache slot a child will probe.
    #[inline(always)]
    fn l78_prefetch(&self, hash: u64) {
        if self.l78.is_empty() {
            return;
        }
        let idx = (hash as usize) & (self.l78.len() - 1);
        #[cfg(target_arch = "aarch64")]
        // SAFETY: `idx` is masked into the table, and a prefetch of a valid
        // address has no architectural effect.
        unsafe {
            std::arch::asm!("prfm pldl1keep, [{0}]", in(reg) self.l78.as_ptr().add(idx), options(nostack, preserves_flags));
        }
        #[cfg(not(target_arch = "aarch64"))]
        let _ = idx;
    }

    /// Warm the four-empty cache line for `hash` (no-op off aarch64).
    #[inline]
    fn l4_prefetch(&self, hash: u64) {
        if self.l4.is_empty() {
            return;
        }
        let idx = (hash as usize) & (self.l4.len() - 1);
        // SAFETY: in-range index; a prefetch has no effect beyond the cache.
        unsafe {
            let p = self.l4.as_ptr().add(idx);
            #[cfg(target_arch = "aarch64")]
            std::arch::asm!("prfm pldl1keep, [{p}]", p = in(reg) p, options(nostack, preserves_flags));
            #[cfg(not(target_arch = "aarch64"))]
            let _ = p;
        }
    }

    /// Probe the 7-8 cache: same one-way (lower, upper, best) shape as the
    /// 9-empty cache, for the two layers below `tt_min_empties()` that had no
    /// transposition reuse at all.
    #[inline]
    #[allow(clippy::type_complexity)]
    fn l78_probe(
        &self,
        hash: u64,
        player: u64,
        opponent: u64,
    ) -> Option<(i32, i32, Option<Position>)> {
        // SAFETY: the table length is a power of two (it comes from
        // `zeroed_vec(1 << bits)`), so the mask lands inside it; a
        // checked index here is a branch on every probe.
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
        // SAFETY: `idx` is masked into a power-of-two length table.
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

    /// Probe the 9-empty cache: bounds handled like a table entry, plus the
    /// stored best move for ordering. `None` = no matching entry.
    #[inline]
    #[allow(clippy::type_complexity)]
    fn l9_probe(
        &self,
        hash: u64,
        player: u64,
        opponent: u64,
    ) -> Option<(i32, i32, Option<Position>)> {
        // SAFETY: masked into a power-of-two length table, as above.
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
        // SAFETY: `idx` is masked into a power-of-two length table.
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

    /// Cut from the 5-6 cache, or `None`.
    #[inline]
    fn l56_probe(
        &self,
        hash: u64,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
    ) -> Option<i32> {
        // SAFETY: the table length is a power of two (it comes from
        // `zeroed_vec(1 << bits)`), so the mask lands inside it; a
        // checked index here is a branch on every probe.
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
        // SAFETY: `idx` is masked into a power-of-two length table.
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

    /// Cut from the four-empty cache, or `None`. `alpha`/`beta` are the
    /// child's own window.
    #[inline]
    fn l4_probe(
        &self,
        hash: u64,
        player: u64,
        opponent: u64,
        alpha: i32,
        beta: i32,
    ) -> Option<i32> {
        // SAFETY: the table length is a power of two (it comes from
        // `zeroed_vec(1 << bits)`), so the mask lands inside it; a
        // checked index here is a branch on every probe.
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

    /// Merge a solved four-empty result into the cache: same position
    /// tightens the pair, a different one overwrites unconditionally.
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
        // Overwrite a stale slot, then fold the score into the bounds -
        // rather than assembling the whole entry and storing it once, which
        // measured +0.47% over four sets. The move generator gains from a
        // single store because every entry there is new; here the common
        // case is a hit, and a hit only has to write the two bound bytes.
        let idx = (hash as usize) & (self.l4.len() - 1);
        // SAFETY: `idx` is masked into a power-of-two length table.
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

    /// Transposition table serving a node with this many empties: the
    /// cache-resident one below `MID_TT_EMPTIES`, the main one above.
    #[inline(always)]
    fn table(&self, empties: u8) -> &HashTable {
        if empties < self.mid_empties {
            &self.mid_table
        } else {
            self.tt
        }
    }

    /// Poll the cutoff signal, at every node.
    ///
    /// This used to run on a 512-node leash on the theory that walking the
    /// ancestor chain per node would cost more than the work it saves. It does
    /// not: measured on FFO40-49, leashes of 512, 64, 8 and 1 are
    /// indistinguishable in both throughput and node count (nps stays at 92M
    /// on 6 threads, 120M on 10). At one thread the chain is a single link, so
    /// there is nothing to walk.
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
    /// `bit_size`: transposition table has 2^bit_size entries
    /// (each entry is ~48 bytes; 20 -> ~50 MB).
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

    /// The persistent pool with `extra` workers, (re)spawned only when the
    /// thread count changes.
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

    /// Lend the NNUE searcher to the selective probes (see `NnueProbe`).
    pub fn set_nnue(
        &mut self,
        nn: &'static crate::nnue::Nnue,
        tt: &'static crate::midgame::SharedTt,
    ) {
        self.nnue = Some((nn, tt));
    }

    /// Set how many threads the root may split its siblings across.
    /// The transposition table is shared; every other piece of search state
    /// is per-thread.
    /// Install the external stop handle (the UI stop button).
    pub fn set_stop(&mut self, stop: Option<crate::midgame::StopHandle>) {
        self.stop = stop;
    }

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
        self.solve_impl(mode, board, ev, None)
    }

    /// Solve to the end of the game *selectively*: run the warm-up ladder up to
    /// `t` standard deviations and answer with that, skipping the exact pass.
    ///
    /// This is the band between a midgame search and an exact solve: once
    /// empties drop far enough, the search depth becomes the empty count —
    /// the whole rest of the game — answered selectively rather than proved.
    /// Reading to
    /// the end beats estimating with any evaluator, and reading to the end
    /// *selectively* costs a fraction of proving it: measured here, moving the
    /// exact threshold from 24 to 26 empties bought 0.9 points of win rate for
    /// 1.6x the time, because an exact solve is the wrong tool for a band
    /// where a 93% answer is worth nearly as much.
    pub fn solve_selective(
        &mut self,
        board: &Board,
        ev: Option<&Evaluator>,
        t: f32,
    ) -> EndSolverResult {
        self.solve_impl(EndSolverMode::Perfect, board, ev, Some(t))
    }

    /// The value a warm-up probe of `depth` plies reports for `board` — the
    /// left-hand side of the ProbCut inequality, exposed so the margin it is
    /// compared against can be *measured* rather than extrapolated.
    ///
    /// `MPC_T * mpc_sigma(..)` is only honest if sigma is the standard
    /// deviation of `exact - probe` for the search that actually runs. Ours is
    /// fitted on midgame searches and grows about a quarter of a disc per ply
    /// without bound, so by 30 empties it claims an 8-disc error — twice what
    /// direct measurement shows. Pair this with an exact solve of the same
    /// position to find out which is true.
    /// NNUE probe value at `depth`, for sigma calibration (None if no NNUE).
    pub fn probe_value_nnue(&mut self, board: &Board, depth: u8) -> Option<f32> {
        let (nn, mtt) = self.nnue?;
        let mut ms = crate::midgame::NnueSearch::new(nn, mtt);
        let mut acc = nn.indices(board.black, board.white);
        Some(ms.negamax(
            board,
            &mut acc,
            depth as u32,
            f32::NEG_INFINITY,
            f32::INFINITY,
        ))
    }

    /// Paired-harness bench of the static move scoring: ns per scored move
    /// (generation + the 5-term ordering value, the 7-13-empty path).
    pub fn bench_order(cases: &[(u64, u64)], rounds: usize) -> (f64, u64) {
        let mut best = f64::INFINITY;
        let mut moves_total = 0u64;
        let mut sink = 0u64;
        for _ in 0..5 {
            moves_total = 0;
            let t0 = std::time::Instant::now();
            for _ in 0..rounds {
                for &(p, o) in cases {
                    let parity = {
                        let mut e = !(p | o);
                        let mut par = 0u8;
                        while e != 0 {
                            let sq = e.trailing_zeros() as u8;
                            e &= e - 1;
                            par ^= quadrant_id(sq);
                        }
                        par
                    };
                    let mut m = bitboard::mobility(p, o, !(p | o));
                    while m != 0 {
                        let sq = m.trailing_zeros() as u8;
                        m &= m - 1;
                        let f = bitboard::flippable(p, o, 1u64 << sq);
                        let cp = o ^ f;
                        let co = p | f | (1u64 << sq);
                        sink ^=
                            move_ordering_value(Position(sq), cp, co, parity, order_pot()) as u64;
                        moves_total += 1;
                    }
                }
            }
            let ns = t0.elapsed().as_nanos() as f64 / moves_total as f64;
            best = best.min(ns);
        }
        std::hint::black_box(sink);
        (best, moves_total / rounds as u64)
    }

    /// Paired-harness bench of the main table: (store, hit, miss) ns/op.
    /// One store per case, then a hit probe and a probed miss on a
    /// disturbed board.
    /// Times the leaf solvers over a shared corpus, so a second
    /// implementation can reproduce the same work and the ratio can be read
    /// from paired numbers rather than from sampling attribution - which
    /// moves with inlining and has twice overstated a gap here.
    ///
    /// Each case is played down to `n` empties by the corpus generator, so
    /// the squares handed in are the position's own empties.
    pub fn bench_leaves(cases: &[(u64, u64)], rounds: usize) -> (f64, f64, f64, u64) {
        let tt = HashTable::new(16);
        let budget = ThreadBudget::new(0);
        let root_abort = AbortFlag::root();
        let mut out = [0f64; 3];
        let mut sink = 0i64;
        let mut n_cases = 0u64;
        for (slot, n_empty) in [2usize, 3, 4].into_iter().enumerate() {
            let cs: Vec<(u64, u64, [u8; 4])> = cases
                .iter()
                .filter_map(|&(p, o)| {
                    let mut e = !(p | o);
                    if e.count_ones() as usize != n_empty {
                        return None;
                    }
                    let mut sq = [0u8; 4];
                    for s in sq.iter_mut().take(n_empty) {
                        *s = e.trailing_zeros() as u8;
                        e &= e - 1;
                    }
                    Some((p, o, sq))
                })
                .collect();
            if cs.is_empty() {
                continue;
            }
            if slot == 0 {
                n_cases = cs.len() as u64;
            }
            // The worker owns two private tables; building one per round
            // would time their allocation, not the leaves.
            let mut w = Worker::new(&tt, &budget, &root_abort);
            let mut best = f64::INFINITY;
            for _ in 0..5 {
                let t0 = std::time::Instant::now();
                for _ in 0..rounds {
                    for &(p, o, sq) in &cs {
                        sink = sink.wrapping_add(match n_empty {
                            // Null window, as the search itself uses and as the paired
                            // implementation's `solve*` build internally: a full
                            // window would search every child and time work
                            // neither engine actually does.
                            2 => w.last2(p, o, sq[0], sq[1], 0, 1, false) as i64,
                            3 => w.last3(p, o, sq[0], sq[1], sq[2], 0, 1, false, 0) as i64,
                            _ => w.last4(p, o, sq[0], sq[1], sq[2], sq[3], 0, 1, false, 0) as i64,
                        });
                    }
                }
                let ns = t0.elapsed().as_nanos() as f64 / (rounds * cs.len()) as f64;
                best = best.min(ns);
            }
            out[slot] = best;
        }
        std::hint::black_box(sink);
        (out[0], out[1], out[2], n_cases)
    }

    pub fn bench_tt(cases: &[(u64, u64)], rounds: usize) -> (f64, f64, f64) {
        let table = HashTable::new(22);
        let time = |f: &mut dyn FnMut() -> u64, n: usize| -> f64 {
            let mut best = f64::INFINITY;
            let mut sink = 0u64;
            for _ in 0..5 {
                let t0 = std::time::Instant::now();
                for _ in 0..rounds {
                    sink ^= f();
                }
                best = best.min(t0.elapsed().as_nanos() as f64 / (rounds * n) as f64);
            }
            std::hint::black_box(sink);
            best
        };
        let boards: Vec<(Board, u64)> = cases
            .iter()
            .map(|&(p, o)| {
                let b = Board {
                    black: p,
                    white: o,
                    player: crate::color::Color::Black,
                    empty_count: (64 - (p | o).count_ones()) as u8,
                };
                let h = zobrist::board_hash(p, o);
                (b, h)
            })
            .collect();
        let store = time(
            &mut || {
                let mut acc = 0u64;
                for (b, h) in &boards {
                    table.update(b, *h, -3, 3, 1, None, true);
                    acc ^= *h;
                }
                acc
            },
            boards.len(),
        );
        let hit = time(
            &mut || {
                let mut acc = 0u64;
                for (b, h) in &boards {
                    if let Some(e) = table.get(b, *h) {
                        acc ^= e.lower() as u64;
                    }
                }
                acc
            },
            boards.len(),
        );
        let miss = time(
            &mut || {
                let mut acc = 0u64;
                for (b, h) in &boards {
                    let hb = h ^ 0x9e37_79b9;
                    if let Some(e) = table.get(b, hb) {
                        acc ^= e.upper() as u64;
                    }
                    acc = acc.wrapping_add(1);
                }
                acc
            },
            boards.len(),
        );
        (store, hit, miss)
    }

    pub fn probe_value(&mut self, board: &Board, ev: &Evaluator, depth: u8) -> f32 {
        let tt = &self.hash_table;
        let budget = ThreadBudget::new(0);
        let root_abort = AbortFlag::root();
        let mut w = Worker::new(tt, &budget, &root_abort);
        let ix = ev.indexer();
        let mut indices = ix.init(board.black, board.white);
        let hash = zobrist::board_hash(board.player_bb(), board.opponent_bb());
        w.seed_search(
            board,
            hash,
            ev,
            ix,
            &mut indices,
            depth,
            f32::NEG_INFINITY,
            f32::INFINITY,
            false,
        )
    }

    fn solve_impl(
        &mut self,
        mode: EndSolverMode,
        board: &Board,
        ev: Option<&Evaluator>,
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

        // The exact solve's probe-depth shape, chosen per root. Flat 3 from
        // 18-24 empties (-0.7% nodes against flat 2); from 25 up, 6 at
        // 20+-empty nodes and 2 below (`SEL_DEEP_ROOT=<n>` moves the switch,
        // 0 disables) - a deep root's flat probe stops cutting and the
        // warm-up degrades to re-search. Selective answers keep the
        // selective tuning untouched.
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

        // A single-threaded solve owns the table outright and can skip every
        // lock; with helpers in play the probes must synchronize.
        self.hash_table.set_shared(self.threads > 1);

        let t_clear = std::time::Instant::now();
        self.hash_table.clear(self.threads);
        // The root worker's private tables are cleared here, inside the
        // subtracted interval like the main table, and reused across solves:
        // neither their allocation nor their wiping is billed to the search.
        // Positions stay independent - the tables are empty either way.
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
        // The helper scratch pool persists with the budget now; drop its
        // contents here (outside the timed interval) so positions stay
        // independent, like every other table. Fresh ones are lazy pages.
        if let Some(b) = self.budget.as_ref() {
            b.scratch.lock().unwrap().clear();
        }
        CLEAR_NS.fetch_add(
            t_clear.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        // The pool lives for the whole solve, so a hand-off is a queue push
        // rather than an OS thread creation. A process-wide pool would go
        // further; one per solve is close enough at seconds per move and
        // keeps the borrows scoped.
        let extra = self.threads.saturating_sub(1);
        let budget_arc = self.ensure_budget(extra);
        let budget: &ThreadBudget = &budget_arc;
        let tt = &self.hash_table;
        let stop_ref = self.stop.as_ref();
        // Borrowed by the watcher thread, so it must outlive the scope.
        let root_abort = AbortFlag::root();
        let watching = std::sync::atomic::AtomicBool::new(true);
        let (value, nodes, best, r_shallow_back, r_mid_back, r_l4_back) =
            std::thread::scope(|scope| {
                // Route the external stop through the sibling-abort flag.
                // Workers don't carry the stop handle: every worker already
                // polls the abort flag each node and `aborted()` walks up the
                // parents, so raising the root reaches everyone without adding
                // a check to the search inner loop. The watcher is a dedicated
                // thread (polling inside the search either dulls the response
                // or slows the search), and it must exit via Drop or the
                // scoped join would never return.
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
                // The NNUE probes serve both the selective answer (-30% there)
                // and, since 2026-08-27, the exact solve's warm-up ladder (see
                // `sel_nnue_warm` — the earlier +10% loss was the H=16 model).
                w.nnue = if selective.is_some() || sel_nnue_warm() {
                    self.nnue
                } else {
                    None
                };
                w.sigma_scale = std::env::var("SEL_SIGMA_SCALE")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(if selective.is_some() { 0.6 } else { 1.0 });
                let mut b = *board;

                // Warm-up ladder (iterative selectivity): solve the same
                // full-depth endgame selectively first, leaving real full-depth
                // best moves in the table for the exact pass that follows.
                let t_warm = std::time::Instant::now();
                // The rungs to climb. A selective solve still walks the cheaper
                // rungs first — they cost little and leave the table ordered for
                // the one that answers ([[warmup-rung-value-is-the-table]]: the
                // value of a rung is the table it leaves, not its score).
                let mut rungs: Vec<f32> = selective_ladder();
                if let Some(t) = selective {
                    // A selective solve answers with its top rung, so the cheaper
                    // ones below it are pure ordering warm-up — and their bounds
                    // are demoted between rungs, so ordering is *all* they leave.
                    if !std::env::var("SEL_ONE_RUNG").is_ok_and(|v| v != "0") {
                        rungs.retain(|&r| r < t);
                        // For a selective *answer* only the cheapest rung earns its
                        // keep: a second rung near t re-searches nearly the same
                        // tree (measured at 29 empties: [1.1,1.8]+t 67.8s,
                        // [1.1]+t 62.3s, [1.4]+t 72.9s, t alone 89.3s). The exact
                        // solve's warm-up keeps the full ladder — there the rungs
                        // exist for ordering and the second one pays (2.1x).
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

                WARMUP_NS.fetch_add(
                    t_warm.elapsed().as_nanos() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                // Helper nodes are folded into `w.nodes` by the split that waited
                // for them, so this running total already covers the whole phase.
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
                // Every task has been waited for by the split that queued it, so
                // the queue is empty here and the workers only need releasing.
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

    /// Exact score. With a warm-up score available the window is a narrow
    /// aspiration around it, widened only
    /// on the side that failed. Without one, fall back to the win/loss
    /// probe and a +-8 band.
    fn perfect(&mut self, board: &mut Board, ev: Option<&Evaluator>) -> i32 {
        // Experiment knob: run the exact pass on the full window and let
        // PVS narrow it from the seeds; no aspiration to mis-centre.
        if std::env::var("SEL_EXACT_FULLWIN").is_ok_and(|v| v != "0") && self.warm_score.is_some() {
            return self.pvs_root(board, -64, 64, ev);
        }
        if let Some(score) = self.warm_score {
            // Reopen the window the last warm rung converged in (see
            // `warm_window`). Re-centring on the rung's *score* instead moves
            // the window whenever the rung lands on the true value — and a
            // shifted window fights the seed ordering instead of using it.
            // `SEL_ASP_RECENTER=1` restores the old re-centring for A/B runs.
            let recenter = std::env::var("SEL_ASP_RECENTER").is_ok_and(|v| v != "0");
            if let Some((mut lo, mut hi)) = self.warm_window.filter(|_| !recenter) {
                if lo < score && score < hi {
                    // `EXACT_REOPEN_CLAMP=<d>` narrows the reopened window to
                    // score +/- d (an edge result falls into `aspiration`).
                    // The rung's converged window can be wide after fail
                    // doublings; the score is a better centre than its span.
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

    /// Cheap evaluation-based guess of the final score, on the even grid
    /// that terminal scores live on.
    fn estimate_score(&mut self, board: &Board, ev: Option<&Evaluator>) -> i32 {
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
            let n0 = self.nodes;
            let val = self.pvs_root(board, lo, hi, ev);
            /* ABORTED is not a value. It is `i32::MIN + 1`; letting it
            into window arithmetic (`val - 8`) overflows and corrupts the
            next window. Deadline expiry takes this same path, so this
            could produce a wrong move in a real game. */
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

    fn pvs_root(
        &mut self,
        board: &mut Board,
        alpha: i32,
        beta: i32,
        ev: Option<&Evaluator>,
    ) -> i32 {
        let mut lower = alpha;
        let upper = beta;
        // Root computes the hash from scratch once; children update it
        // incrementally (flipped discs + placed disc + player swap).
        let hash = zobrist::board_hash(board.player_bb(), board.opponent_bb());

        self.nodes += 1;

        // Seed the carried ordering indices once. Every node below rebuilt
        // them from the bitboards before this; see `Worker::order_ix`.
        if let Some(e) = ev.filter(|_| board.empty_count() >= eval_order_empties()) {
            self.order_ix = order_indexer(e).init(board.black, board.white);
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

        // First move: full window
        {
            let n0 = self.nodes;
            let m0 = moves[0];
            let mut child = m0.child(board);
            let ch = child_hash_of(&child);
            max = -self.pvs_ordered(&mut child, ch, -upper, -lower, false, ev, m0, root_mover);
            /* Never use an aborted search's value. `ABORTED` is
            `i32::MIN + 1`; negation turns it into `i32::MAX`, which flows
            through `max` into the table (`i32::MAX as i8` is -1, i.e. a
            bogus lower bound of -1). A poisoned table later sank a true +16
            move to +1 and the engine played a -20 move as best. */
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

        // Young Brothers Wait: the eldest child above has proved a bound, so
        // the remaining siblings are independent enough to share out. Only
        // worth the hand-off when the subtrees are large.
        if moves.len() > 2 && board.empty_count() >= parallel_min_empties() && lower < upper {
            if let Some((val, bpos)) = self.split_siblings(board, &moves[1..], lower, upper, ev) {
                if val == ABORTED {
                    return ABORTED;
                }
                if val > max {
                    max = val;
                    // Only moves with proven values become best; a
                    // fail-low upper bound may raise the value but its
                    // move must not be selected.
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

        // Remaining moves: null-window probe, re-search on fail-high
        for m in &moves[1..] {
            if lower >= upper {
                break;
            }
            let mut child = m.child(board);
            let ch = child_hash_of(&child);
            let mut val =
                -self.pvs_ordered(&mut child, ch, -lower - 1, -lower, true, ev, *m, root_mover);
            // Aborts are not values (same reason as the eldest branch).
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

    /// Fan the young brothers out over the pool.
    ///
    /// One child is one task. Each is offered to the pool as the loop reaches
    /// it; if the pool refuses — no worker is idle right now — this thread
    /// searches that child itself and moves on. Nothing is reserved in advance,
    /// so a node never holds a worker it is not using, and a worker never sits
    /// committed to a node that has run out of siblings.
    ///
    /// Returns the best (value, move), `None` when there are no workers at all
    /// and the caller should search normally, or `ABORTED` as the value if the
    /// search unwound — a truncated search proves nothing and its bound must
    /// not reach the table.
    ///
    /// The alpha bound is shared and only ever rises, so a brother that reads a
    /// slightly stale value simply searches a wider window — correct, just
    /// marginally more work.
    ///
    /// A more aggressive variant does not pay off for us: probe every brother
    /// at the alpha the round
    /// started with, stop the whole fan-out the moment *any* of them beats it
    /// (not only on a proven cutoff), re-search the improvers with the full
    /// window at the raised alpha, and recurse on whatever the abort
    /// left unsearched. Measured here, that was slower on FFO40-49: min of
    /// 3, 10 threads 4.98s -> 6.10s with nodes 599M -> 752M; 6 threads 6.25s ->
    /// 6.32s. Killing the fan-out throws away nine part-built subtrees, and
    /// because an unwound `pvs` never reaches its `tt.update` their work is not
    /// in the table either — the next round pays for all of it again. The two
    /// schemes are identical at null-window nodes (there `val > lower` *is* the
    /// cutoff), so the whole difference is the wide-window nodes at the top of
    /// the tree, which are exactly the expensive ones to redo.
    /// Publish the siblings as a persistent split point and work it like
    /// any helper; late-idle workers join through the registry until the
    /// cursor drains. Same window/merge semantics as the task-queue path.
    fn split_siblings_v2(
        &mut self,
        parent: &Board,
        siblings: &[ScoredMove],
        lower: i32,
        upper: i32,
        ev: Option<&Evaluator>,
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
            nnue: self.nnue,
            sigma_scale: self.sigma_scale,
            tt: self.tt as *const HashTable,
            ev: ev.map_or(std::ptr::null(), |e| e as *const Evaluator as *const ()),
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
            let mut val = -self.pvs(&mut child, ch, -cur - 1, -cur, false, true, ev);
            if val == -ABORTED {
                unwound = true;
                break;
            }
            if cur < val && val < upper {
                let ch = child_hash_of(&child);
                val = -self.pvs(&mut child, ch, -upper, -val, false, false, ev);
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
                // Helpers may still be inside their siblings; the proven
                // cutoff makes their results moot, so stop them the same
                // way a helper's own cutoff would.
                ABORT_FIRED.fetch_add(1, Ordering::Relaxed);
                if solver_abort() {
                    group.abort();
                }
                break;
            }
        }
        // Close the cursor so nothing joins what the owner has abandoned.
        sp.cursor.store(sp.n_moves, Ordering::Release);
        if let Some(i) = slot {
            pool.unregister_split(i, &sp);
        }
        let t_wait = std::time::Instant::now();
        // Helping other split points while waiting here measured neutral
        // (8T FFO minima 6.62s vs 6.55s): after the registry the wait is
        // already only ~8% of thread time. Park and keep the stack flat.
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
        ev: Option<&Evaluator>,
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
        let nnue = self.nnue;
        let sigma_scale = self.sigma_scale;
        // Cutting off this node must also stop work already under way in the
        // siblings, so the tasks search under a flag chained to ours.
        let group = AbortFlag::child(self.abort);
        let group = &group;
        let waiter = std::thread::current();
        let slots: Vec<TaskSlot> = siblings
            .iter()
            .map(|_| TaskSlot::new(waiter.clone()))
            .collect();
        let board = *parent;

        let mut handed: Vec<usize> = Vec::new();
        /* Track value and move separately. A fail-low upper bound feeds
        the node value (fail-soft) but must never pick the move; tracking
        both in one variable once made a -20 move look like a +16 best. */
        let mut max = i32::MIN;
        let mut best_val = i32::MIN;
        let mut best: Option<Position> = None;
        let mut unwound = false;
        // Did an ancestor abort leave siblings unseen?
        let mut cut_short = false;

        for (i, m) in siblings.iter().enumerate() {
            if group.aborted() {
                /* Stopped by an ancestor: siblings remain unseen, so
                `max` is not a full result. Returning it would let the
                caller store it as exact and mislead later searches.
                Parallel-only and intermittent. */
                cut_short = true;
                break;
            }
            let cur = shared_lower.load(Ordering::Relaxed);
            if cur >= upper {
                break;
            }
            // Never hand off the youngest brother: this thread has to wait
            // for the fan-out
            // anyway, so it may as well be the one to search the last child.
            if i + 1 < siblings.len() {
                let m = *m;
                let slot = &slots[i];
                let shared = &shared_lower;
                // SAFETY: the task borrows `group`, `shared_lower` and `slot`,
                // all locals of this frame. The `help_until` loop below waits
                // for every task in `handed` before this frame returns, so the
                // borrows outlive the task.
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
            // Refused, or the youngest brother: search it here.
            let n0 = self.nodes;
            let mut child = m.child(parent);
            let ch = child_hash_of(&child);
            let mut val = -self.pvs(&mut child, ch, -cur - 1, -cur, false, true, ev);
            if val == -ABORTED {
                unwound = true;
                break;
            }
            if cur < val && val < upper {
                let ch = child_hash_of(&child);
                val = -self.pvs(&mut child, ch, -upper, -val, false, false, ev);
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
            /* Value and move are handled separately: a fail-low in
            `(cur, cur+1)` is an upper bound — fine for the fail-soft node
            value, never for move selection. */
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

        // Everything handed over has to be accounted for before the borrows
        // above die, aborted or not.
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
            // Split value from move (same reason as the inline path).
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
        /* With aborted siblings the remainder cannot prove a value.
        A proven cutoff (`max >= upper`) suffices, but an aborted sibling
        is "unseen", not "bad": taking the max of what's left once dropped
        a +16 move and returned +14 as best. */
        if (any_aborted || cut_short) && max < upper {
            return Some((ABORTED, best));
        }
        Some((max, best))
    }

    #[allow(clippy::too_many_arguments)]
    fn pvs(
        &mut self,
        board: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        cut_node: bool,
        ev: Option<&Evaluator>,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, board.empty_count());
        let mut lower = alpha;
        let mut upper = beta;

        self.nodes += 1;
        if self.should_abort() {
            return ABORTED;
        }
        // The side to move, for advancing the carried ordering indices
        // across a child's move.
        let mover = board.player();
        // Everything the taint counter moves past this point makes the
        // node's result rung-dependent (see `EXACT_PROOF`).
        let taint0 = self.taint;

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
                if self.selective_t.is_some() && !v.proven_entry() {
                    self.taint += 1;
                }
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
            if board.empty_count() >= selective_min_empties() && upper - lower <= 1 {
                if let Some(v) = self.selective_cut(board, hash, e, t, lower, upper) {
                    self.taint += 1;
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
            // The child of an expected All node is a Cut node.
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
            // Aborts are not values; keep them out of the table too.
            if val == -ABORTED {
                return ABORTED;
            }
            let clean = self.selective_t.is_none() || self.taint == taint0;
            self.tt.update(board, hash, alpha, beta, val, None, clean);
            return val;
        }

        // A move that wipes out the opponent ends the game at exactly +64,
        // which is the maximum possible score — so it *is* this node's value.
        if moves.iter().any(|m| m.child(board).player_bb() == 0) {
            self.tt.update(board, hash, alpha, beta, 64, None, true);
            return 64;
        }

        // Enhanced transposition cutoff: a child whose stored upper bound
        // already proves our value >= upper ends this node for free.
        if board.empty_count() >= etc_empties() {
            let _p = layer_profile::Scope::new(layer_profile::ETC, board.empty_count());
            // The comparison accounting counts each ETC probe as a node;
            // tallied locally so the instrumentation costs one atomic per
            // node rather than one per probe.
            let mut probed = 0u64;
            for m in moves.iter() {
                probed += 1;
                let probe_child = m.child(board);
                if let Some(e) = self.tt.get(&probe_child, child_hash_of(&probe_child)) {
                    if !e.is_seed(self.tt.date()) && -e.upper() >= upper {
                        if self.selective_t.is_some() && !e.proven_entry() {
                            self.taint += 1;
                        }
                        node_accounting::etc(probed);
                        return -e.upper();
                    }
                }
            }
            node_accounting::etc(probed);
        }

        let mut max = i32::MIN;
        let mut best = None;

        let tt_best = entry.and_then(|e| e.best());

        // Speculative fan-out at an expected All node: every child gets
        // searched whatever the order, so serialising the eldest to prove a
        // bound first buys nothing - publish the whole list at once. Null
        // windows only (a wide-window fan-out re-derives the rejected wide
        // split), and only where the subtrees are big enough to feed on
        // (`SPEC_SPLIT=0` disables).
        if spec_split()
            && !cut_node
            && upper - lower == 1
            && board.empty_count() >= parallel_min_empties()
            && board.empty_count() <= spec_split_max()
            && moves.len() > 2
            && self.budget.pool.workers > 0
        {
            self.score_moves(board, &mut moves, tt_best, ev, lower, parity_of(board));
            // Unstable: helpers only need an order, and the stable sort
            // allocates a scratch `Vec` to get a tie-break nothing reads.
            moves.sort_unstable_by_key(|m| m.value);
            if let Some((val, bpos)) = self.split_siblings(board, &moves, lower, upper, ev) {
                if val == ABORTED {
                    return ABORTED;
                }
                if self.selective_t.is_some() {
                    self.taint += 1;
                }
                let clean = self.selective_t.is_none() || self.taint == taint0;
                self.tt.update(board, hash, alpha, beta, val, bpos, clean);
                return val;
            }
        }

        // Stage 1: the transposition-table move, searched before any
        // ordering work is done. Most cut nodes end here.
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
                    let clean = self.selective_t.is_none() || self.taint == taint0;
                    self.tt.update(board, hash, alpha, beta, max, best, clean);
                    node_accounting::cut_at(board.empty_count(), Some(0));
                    return max;
                }
            }
        }

        // Stage 2: the rest, now ordered properly.
        self.score_moves(board, &mut moves, None, ev, lower, parity_of(board));

        let mut next = 0usize;
        // Whether the table move was searched (accounting only): it left
        // `moves`, so it is not in `next`.
        let tt_searched = max != i32::MIN;
        if max == i32::MIN {
            // No table move: the best-ordered child takes the full window.
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

        // Young Brothers Wait: an elder sibling has proved a bound, so the
        // rest are independent enough to share out. Splitting at any node
        // deep enough, not only at the root, is what
        // keeps every core busy when one subtree dominates.
        if moves.len() - next > 1 && board.empty_count() >= parallel_min_empties() && lower < upper
        {
            // Helpers take the siblings in order, so this path pays for the
            // full sort that the sequential path avoids.
            moves[next..].sort_unstable_by_key(|m| m.value);
            if let Some((val, bpos)) = self.split_siblings(board, &moves[next..], lower, upper, ev)
            {
                // A truncated search proves nothing: its bound must not reach
                // the table, so unwind instead of storing a partial `max`.
                if val == ABORTED {
                    return ABORTED;
                }
                if val > max {
                    max = val;
                    if let Some(p) = bpos {
                        best = Some(p);
                    }
                }
                // Helper taint is invisible from here, so a split result is
                // never proven during a warm-up rung.
                if self.selective_t.is_some() {
                    self.taint += 1;
                }
                let clean = self.selective_t.is_none() || self.taint == taint0;
                self.tt.update(board, hash, alpha, beta, max, best, clean);
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

        // A truncated search proves nothing, so its bound must never reach
        // the table — a wrong bound there would corrupt later searches.
        let clean = self.selective_t.is_none() || self.taint == taint0;
        self.tt.update(board, hash, alpha, beta, max, best, clean);
        max
    }

    /// `pvs`, with the carried ordering indices advanced across `m` first
    /// and restored after. See `descend_ordered`.
    #[allow(clippy::too_many_arguments)]
    fn pvs_ordered(
        &mut self,
        child: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        cut_node: bool,
        ev: Option<&Evaluator>,
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

    /// `descend`, with the carried ordering indices advanced across `m`
    /// first and restored after.
    ///
    /// Only the band that orders by evaluation reads them, so the snapshot
    /// is skipped for a child below it: the field is left as the parent had
    /// it, which is what the parent will find on the way back up anyway.
    #[allow(clippy::too_many_arguments)]
    fn descend_ordered(
        &mut self,
        child: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        cut_node: bool,
        ev: Option<&Evaluator>,
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

    /// Full-window recursive descent picking the right strategy by depth.
    ///
    /// Forced inline: the call graph showed this between every pair of `pvs`
    /// frames, so the search paid two calls a ply for one dispatch.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn descend(
        &mut self,
        child: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        cut_node: bool,
        ev: Option<&Evaluator>,
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
            // The child unwound; keep the sentinel recognizable after the
            // usual negation.
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

    /// Null-window probe then re-search, at the strategy for this depth.
    /// Forced inline for the reason given on `descend`.
    #[inline(always)]
    fn descend_null_window(
        &mut self,
        child: &mut Board,
        hash: u64,
        lower: i32,
        upper: i32,
        cut_node: bool,
        ev: Option<&Evaluator>,
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
    /// `parity` is the quadrant parity of this position, carried down
    /// rather than recomputed. Every node needs it - the static ordering
    /// reads it, and so does every child that lands in the 5-6 band - and
    /// deriving it costs four masked popcounts against the single XOR that
    /// updates it across a move.
    ///
    /// `&mut Board` entry point; the band runs on raw bitboards below, for
    /// the reason `alpha_beta` gives. Only the shared transposition table
    /// still wants a `Board`, and it gets one built on the spot at the two
    /// places that touch it.
    fn alpha_beta_ordered(
        &mut self,
        board: &mut Board,
        hash: u64,
        alpha: i32,
        beta: i32,
        passed: bool,
        parity: u8,
        ev: Option<&Evaluator>,
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

    /// Whichever of the three tables this band is using for the position.
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

    /// See `alpha_beta_ordered`.
    #[allow(clippy::too_many_arguments)]
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
        ev: Option<&Evaluator>,
    ) -> i32 {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, n_empties);
        // The move loops below test `val >= beta` rather than re-testing
        // `alpha >= beta` after every update, which is the same cutoff only
        // while the window is non-empty. An empty window here would make the
        // old order break on the first child and this one run the list out,
        // so pin the precondition: if the tree ever moves, look here first.
        debug_assert!(alpha < beta, "alpha_beta_ordered needs a non-empty window");
        let mut alpha = alpha;
        let mut beta = beta;

        self.nodes += 1;

        if let Some(v) = wipeout_score_bb(player, opponent) {
            return v;
        }

        // Issue the table read early but do not wait on it: the order is
        // prefetch -> stability cutoff -> table probe,
        // so the miss latency is covered by the stability computation
        // instead of stalling the node. The parent prefetched this line
        // during its own move generation, but for every child except the
        // first that prefetch is long evicted by the sibling subtrees.
        //
        // Only above `TT_MIN_EMPTIES`: the band below it reads a megabyte
        // of bound cache that stays in L2, and warming the shared table
        // instead - which is what this used to do at every depth - pulled in
        // a line the node never touches. Warming the right one measured
        // -0.02% +/- 0.14%, so the cache does not need warming at all.
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
        // Single probe, reused for the ordering move below (see `pvs`).
        let use_l9 = use_tt && self.g_new_mid && n_empties < self.mid_empties;
        // The bound cache runs beside the transposition table rather than
        // instead of it. Moving the band wholesale to 7-12 cost 2.7% of the
        // tree, because 10-12 lost the shared table - but that measured a
        // replacement, not an addition. A cut here is
        // worth far more tree than one at four empties, and the two lookups
        // answer different questions: the table remembers a proven window,
        // the cache remembers a bound pair keyed on the position alone.
        #[cfg(feature = "l78-wide")]
        let use_l78 = n_empties < TT_MID_WIDE && !self.l78.is_empty();
        #[cfg(not(feature = "l78-wide"))]
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
                if lo >= hi {
                    return lo;
                }
                if beta > hi {
                    beta = hi;
                    narrowed = true;
                    if beta <= alpha {
                        return beta;
                    }
                }
                if alpha < lo {
                    alpha = lo;
                    narrowed = true;
                    if alpha >= beta {
                        return alpha;
                    }
                }
            }
        } else if let Some(v) = entry {
            if !v.is_seed(self.table(n_empties).date()) {
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

        if use_l78 {
            if let Some((lo, hi, best)) = self.l78_probe(hash, player, opponent) {
                l78_best = best;
                if lo >= hi {
                    return lo;
                }
                if beta > hi {
                    beta = hi;
                    narrowed = true;
                    if beta <= alpha {
                        return beta;
                    }
                }
                if alpha < lo {
                    alpha = lo;
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
            // The pre-pass pair, as the `Board` version hashed it after
            // swapping the sides.
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

        // A forced move has nothing to order and nothing worth remembering:
        // re-deriving it costs one move generation, far less than the entry
        // it would evict, so skip the evaluation and the store.
        if moves.len() == 1 {
            let m = moves.at(0);
            let (cbp, cbo) = child_bb(player, opponent, m);
            let cp = parity ^ quadrant_id(m.pos.index());
            let ch = zobrist::board_hash(cbp, cbo);
            let ce = n_empties - 1;
            return if ce >= MOVE_ORDERING_LIMIT {
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
            };
        }

        // Stage 1: the table move, searched before any ordering work - the
        // ordering below puts it first anyway, so the searched order (and
        // the tree) is identical; a cut here just skips the scoring. Same
        // shape as `pvs`.
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
                let (cbp, cbo) = child_bb(player, opponent, m);
                let cp = parity ^ quadrant_id(m.pos.index());
                let ch = zobrist::board_hash(cbp, cbo);
                let ce = n_empties - 1;
                let val = if ce >= MOVE_ORDERING_LIMIT {
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
                };
                // Fail-high first, as in the 5-6 band and the leaf routines:
                // the cutoff test predicts not-taken, and the alpha update
                // drops off the cutoff path. `val >= beta` implies
                // `val > alpha` (the window is never empty), so assigning
                // here is the same store the old order made.
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
                // Branchless off the cutoff path: this layer sets `best`
                // only on a fail high, so nothing else depends on knowing
                // whether alpha moved.
                alpha = alpha.max(val);
            }
        }

        for _ in scored_from..moves.len() {
            abst!(O_SCORED);
        }
        // This band sits entirely below `EVAL_ORDER_EMPTIES`, so the
        // scoring is always the static one; going straight to it is what
        // lets the whole node run without a `Board`.
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
            let (cbp, cbo) = child_bb(player, opponent, m);
            let cp = parity ^ quadrant_id(m.pos.index());
            let ch = zobrist::board_hash(cbp, cbo);
            let ce = n_empties - 1;
            let val = if ce >= MOVE_ORDERING_LIMIT {
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
            };
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

    /// Plain alpha-beta over the empty list, with the last-4 fast path and
    /// a dedicated shallow transposition table (5-6 empties).
    ///
    /// `&mut Board` entry point, for the callers that still hold one. The
    /// band itself runs on raw bitboards: a `Board` carries a colour field,
    /// so every read of the side to move is a select, and the struct is
    /// wide enough that it spills and reloads around the child searches.
    /// The leaf routines have taken bitboards for a while; this extends the
    /// same shape up through the 5-6 band, and lets the ordered stage above
    /// hand over the child it already has as two words instead of building
    /// a `Board` for it.
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

    /// See `alpha_beta`.
    #[allow(clippy::too_many_arguments)]
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
        // A 4-empty entry is a tail dispatch on the *same* position: `last4`
        // counts it, so counting it here as well counted one position twice —
        // 6.4% of the FFO40-49 node total, 8.3% of band22. Run the two guards
        // (they prune, so dropping them grows the tree) but count only when
        // one of them returns, which is exactly the case `last4` never sees,
        // and skip the rest of the prologue, which a 4-empty node has no use
        // for. The hot caller — the 5-empty move loop below — bypasses this
        // entry entirely via `search4`; what reaches here is the rare
        // shallow-root path through `descend`.
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

        // The stability cut runs before the cache probe by default: it is
        // arithmetic on values already in registers, where the probe is a
        // random load. `stab-after-probe` runs them the other way, which
        // saves the sweep on a hit and pays a miss on every cut.
        #[cfg(not(feature = "stab-after-probe"))]
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

        #[cfg(feature = "stab-after-probe")]
        {
            let _p = layer_profile::Scope::new(layer_profile::STAB, n_empties);
            if let Some(bound) = stability_cut_bb(player, opponent, n_empties, lower, upper) {
                return bound;
            }
        }

        let orig_lower = lower;
        let mut best = lower;
        let mut any = false;
        let mut cut = false;
        let player_bb = player;
        let opponent_bb = opponent;
        // Children of a 5-empty node dispatch to last4 and never probe, so
        // their hashes are only needed one level up.
        let five_empty = n_empties == 5;
        let need_child_hash = !five_empty;

        // Odd-quadrant moves first (quadrant parity: filling the last
        // empty of a region tends to keep the tempo).
        let empties = bitboard::empty_bb(player, opponent);
        let odd = PARITY_ODD_MASK[parity as usize];
        // Walking every empty square and treating a zero flip as illegal,
        // rather than generating the legal moves first. Both were measured.
        // Paying for one mobility here and iterating only legal squares is a
        // wash at a bit-identical tree (band22 -0.2%, FFO40-49 -0.2%, 24
        // empties +0.4%, 26 empties +0.3%): the mobility costs what the
        // wasted flips cost. Splitting that mobility further into
        // parity x corner subsets does order better - 1.9% fewer nodes on
        // band22, 3.9% on FFO40-49, 2.5% at 26 empties - but the masking and
        // the extra passes eat exactly that much time (-0.5% to +0.9%, all
        // inside the spread), so the plain scan stays.
        // Flips for every empty square in two batch calls: the batch
        // kernels share one board broadcast, so this is cheaper than 5-6
        // scalar `flippable` calls even when a cutoff would have skipped
        // the tail, and it makes the neighbour prefilter redundant (a zero
        // flip is skipped below). Values, order and tree are unchanged.
        // Uninitialised, like `gen_moves`' scratch: both are written in
        // full for `0..n_empty` before anything reads them, and zeroing
        // them first is 54 bytes of `memset` on every node of the band
        // that carries the most nodes in the search.
        // Legal moves as a bitboard, split into the four visit classes by
        // masking - no index arrays, no per-square legality probe.
        //
        // The scan this replaces built `sqs`, `fls` and `ord` and walked the
        // empties four times: once to split corners from the rest, once per
        // square to find a flip (a zero flip meaning illegal), and twice
        // more to bucket by parity. Two of those passes exist only to
        // produce an order that four AND masks give directly, and the flips
        // for squares a cutoff never reaches are computed either way.
        //
        // Both halves of this were measured before, each bolted onto the
        // array machinery, and each came out a wash - "the masking and the
        // extra passes eat exactly what the better order buys". They are
        // not extra passes when they *are* the iteration.
        //
        // The order is unchanged: odd-parity corners, odd-parity rest,
        // even-parity corners, even-parity rest, which is exactly what the
        // corner-first `sqs` split fed to the two parity passes.
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
                    // SAFETY: the test above leaves `class_ix` below 4,
                    // which the bounds check could not see.
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
                // A 5-empty node's children have four empties, which is
                // `last4`'s territory. Routing them back through this
                // function's entry counted the same position twice and paid
                // for a prologue a leaf routine cannot use. `search4` keeps
                // the two guards that prune and drops the rest; the child
                // board is never materialized either, because the four
                // remaining empties come straight off the parent's mask and
                // the hash is unused below 6 empties.
                let val = if five_empty {
                    // `EXACT_L4_CACHE=1`: probe the four-empty cache before
                    // solving the child — a hit resolves the whole subtree
                    // (its 1-3-empty expansion is where the tree is widest)
                    // for one counted node, the same charge the solve would
                    // have made.
                    if self.g_l4_cache {
                        let ca = -upper;
                        let cb = -best.max(orig_lower);
                        let cp = opponent_bb ^ flips;
                        let co = player_bb | flips | pos_bit;
                        let child_hash = zobrist::board_hash(cp, co);
                        // Warm the next legal child's slot while this one
                        // solves: the probe is a random load into a table
                        // bigger than L1, and one-ahead wastes at most one
                        // hash per node (the all-up-front variant wasted
                        // 3-4 and measured -1.2%).
                        // Warm the next legal child's slot while this one
                        // solves. `cur` already has this move cleared, so
                        // its low bit is the next square of this class; the
                        // class boundary is not crossed, which only changes
                        // when a prefetch is issued, never what is searched.
                        // Warming the next child's slot needs that child's
                        // hash, and the hash needs its flip - a whole kernel
                        // and a Zobrist per child, paid to hide the latency
                        // of one probe. It was nearly free when the flips
                        // for every empty square were already computed in a
                        // batch; the bitboard iteration that replaced those
                        // arrays left this recomputing them.
                        //
                        // Priced afterwards, it still breaks even: four sets,
                        // six shuffled rounds, dropping it is +0.12%, +0.74%,
                        // -0.62%, -0.25%, total -0.07%. The recomputed flip
                        // costs what the hidden latency saves, so the shape
                        // stays as it was. `l4-no-prefetch` drops it.
                        #[cfg(not(feature = "l4-no-prefetch"))]
                        if cur != 0 {
                            let jb = cur.isolate_lowest_one();
                            let jf = bitboard::flippable(player_bb, opponent_bb, jb);
                            if jf != 0 {
                                self.l4_prefetch(zobrist::board_hash(
                                    opponent_bb ^ jf,
                                    player_bb | jf | jb,
                                ));
                            }
                        }
                        abst!(L4_PROBE);
                        if let Some(h) = self.l4_probe(child_hash, cp, co, ca, cb) {
                            abst!(L4_HIT);
                            self.nodes += 1;
                            -h
                        } else {
                            let mut trivial = false;
                            let r = self.search4_t(
                                cp,
                                co,
                                empties & !pos_bit,
                                ca,
                                cb,
                                child_parity,
                                &mut trivial,
                            );
                            if !trivial {
                                abst!(L4_STORE);
                                self.l4_store(child_hash, cp, co, ca, cb, r);
                            }
                            -r
                        }
                    } else {
                        -self.search4(
                            opponent_bb ^ flips,
                            player_bb | flips | pos_bit,
                            empties & !pos_bit,
                            -upper,
                            -best.max(orig_lower),
                            child_parity,
                        )
                    }
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

                // Fail-high first: the cutoff test is the predicted-not-
                // taken branch, and the max drops off the cutoff path.
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
                // The pre-pass pair, which is what the `Board` version
                // hashed after swapping the sides: the caches verify the
                // bitboards themselves, so this only picks the bucket.
                zobrist::board_hash(player, opponent),
                -upper,
                -orig_lower,
                true,
                parity,
            );
        }

        if self.g_l56_cache {
            self.l4_store(hash, player, opponent, orig_lower, upper, best);
        }
        if self.g_new_shallow {
            self.l56_store(hash, player, opponent, orig_lower, upper, best);
        } else if !self.g_no_shallow56 {
            self.shallow_table.update(
                &board_of(player, opponent, crate::color::Color::Black),
                hash,
                orig_lower,
                upper,
                best,
                None,
                true,
            );
        }
        best
    }

    /// A 4-empty child of a 5-empty node, straight from the parent's
    /// registers.
    ///
    /// This is what the tail dispatch through `alpha_beta` used to do, minus
    /// the node count (`last4` counts the position itself) and minus a
    /// prologue whose transposition probe and move loop a 4-empty node never
    /// reaches. The two guards below stay: they prune, and dropping them
    /// grows the tree.
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

    /// `search4` that also reports whether a guard resolved the child in a
    /// single node - such results are not worth a cache store (measured
    /// 5.1% hit rate; a 1-node subtree cannot repay the store).
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
        // The opponent always holds the disc just played, so only the player
        // side can be wiped out here.
        //
        // The counting rule is one node per position entered, so the two
        // early returns below have to count: they are the entries that never
        // reach `last4`, which counts the rest. Leaving them out undercounted
        // FFO40-49 by 11.5M nodes (2.8%) — lopsided positions cut here far
        // more often than balanced ones, so the bias is set-dependent.
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

    /// `last4` from raw bitboards, unpacking the four empty squares.
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

    /// Specialized 4-empties search with quadrant-parity move ordering.
    #[cfg_attr(feature = "last4-inline", inline(always))]
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
        //
        // With all quadrants even (parity == 0) every key ties and the
        // stable sort is the identity — skip the whole quadrant lookup.
        // A large share of 4-empty positions land here.
        #[cfg(feature = "last4-ifsort")]
        let (p1, p2, p3, p4) = if parity != 0 {
            let o1 = parity & quadrant_id(p1) != 0;
            let o2 = parity & quadrant_id(p2) != 0;
            let o3 = parity & quadrant_id(p3) != 0;
            let o4 = parity & quadrant_id(p4) != 0;
            if o1 {
                if o2 {
                    if o3 || !o4 {
                        (p1, p2, p3, p4)
                    } else {
                        (p1, p2, p4, p3)
                    }
                } else if o3 {
                    if o4 {
                        (p1, p3, p4, p2)
                    } else {
                        (p1, p3, p2, p4)
                    }
                } else if o4 {
                    (p1, p4, p2, p3)
                } else {
                    (p1, p2, p3, p4)
                }
            } else if o2 {
                if o3 {
                    if o4 {
                        (p2, p3, p4, p1)
                    } else {
                        (p2, p3, p1, p4)
                    }
                } else if o4 {
                    (p2, p4, p1, p3)
                } else {
                    (p2, p1, p3, p4)
                }
            } else if o3 {
                if o4 {
                    (p3, p4, p1, p2)
                } else {
                    (p3, p1, p2, p4)
                }
            } else if o4 {
                (p4, p1, p2, p3)
            } else {
                (p1, p2, p3, p4)
            }
        } else {
            (p1, p2, p3, p4)
        };

        #[cfg(not(feature = "last4-ifsort"))]
        let (p1, p2, p3, p4) = if parity != 0 {
            // Stable partition via a permutation match, as in `last3`.
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

        // All four flips up front, sharing one board broadcast. A non-zero
        // flip already implies an adjacent opponent disc, so the neighbour
        // guard the loop used to carry is subsumed by `flips == 0` and the
        // node count is unchanged. Eager flips lose against a scalar flip
        // routine (the cut-off moves are computed for nothing); against the
        // vector one they win 1.5% on band22.
        #[cfg(not(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        )))]
        let (f1, f2, f3, f4) = bitboard::flippable4(player, opponent, p1, p2, p3, p4);
        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let fctx = bitboard::FlipCtx::new(player, opponent);

        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let f1 = arm_flip(&fctx, player, opponent, p1);
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
        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let f2 = arm_flip(&fctx, player, opponent, p2);
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
        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let f3 = arm_flip(&fctx, player, opponent, p3);
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
        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let f4 = arm_flip(&fctx, player, opponent, p4);
        // Last move: no cutoff left to prove, so the answer is the better
        // of the two either way.
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

    /// Folded into its caller: the leaf chain is four routines deep and a
    /// node here costs tens of nanoseconds, so the call itself is a
    /// measurable share.
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

        // Stable partition (odd quadrants first, original order within each
        // class) via an explicit permutation: the same order the stable
        // 3-sort produced, without the sort. This runs 47M times per
        // FFO40-49; the generic sort and its key closure were measurable.
        // `last3-noorder` prices the ordering itself: the permutation match
        // runs 45M times on FFO40-49, and this layer is the most expensive
        // per node of the leaf chain.
        #[cfg(not(feature = "last3-noorder"))]
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

        // Straight-line children: the tuple-array loop this replaces built
        // its rest-arrays on the stack on every entry.
        #[cfg(not(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        )))]
        let (f1, f2, f3) = bitboard::flippable3(player, opponent, p1, p2, p3);
        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let fctx = bitboard::FlipCtx::new(player, opponent);

        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let f1 = arm_flip(&fctx, player, opponent, p1);
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
        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let f2 = arm_flip(&fctx, player, opponent, p2);
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
        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let f3 = arm_flip(&fctx, player, opponent, p3);
        // Last move: see `last2`.
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

    /// Folded into its caller: the leaf chain is four routines deep and a
    /// node here costs tens of nanoseconds, so the call itself is a
    /// measurable share.
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
        #[cfg(any(
            feature = "flip-lazy",
            feature = "flip-guard",
            feature = "flip-noshare"
        ))]
        let f2 = arm_flip(&fctx, player, opponent, p2);
        // The last move has nothing left to cut off: whether `val` clears
        // beta or not, the node's answer is the better of the two.
        // Measured neutral on its own (four sets, six shuffled rounds:
        // -0.45%, -0.66%, -0.38%, +0.36%, total +0.05%) - kept because the
        // branch is genuinely dead, not because it pays.
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

    /// Exactly one empty square left, as a value and the rule-U count it
    /// owes.
    ///
    /// Free-standing rather than a method, splitting the pure kernel from
    /// its counting wrapper: a `&mut self` call in the
    /// middle of the hottest move loop in the search carries a store the
    /// optimizer must assume can alias anything the loop holds. On its own
    /// this measured neutral (12.417s against 12.432s); it is kept because
    /// the shape is a precondition for the other leaf changes, not because
    /// it pays by itself.
    #[inline(always)]
    fn last1_value(player: u64, p1: u8) -> (i32, u64) {
        let _prof = layer_profile::Scope::new(layer_profile::SEARCH, 1);
        // The board is full but for `p1`, so the two sides hold 63 discs
        // between them and one popcount gives the difference:
        // `p - (63 - p)`.
        let diff = 2 * player.count_ones() as i32 - 63;
        // Both sides' counts share the four line gathers, so the pass branch
        // below costs nothing extra to have ready.
        let (mine, theirs) = bitboard::count_last_flips(player, p1);

        // Current player fills the last square: one node.
        if mine > 0 {
            return (diff + 2 * mine as i32 + 1, 1);
        }
        // Current player passes; the pass re-entry is the second node.
        if theirs > 0 {
            return (diff - 2 * theirs as i32 - 1, 2);
        }
        // Nobody can play the last square: empty goes to the winner.
        let v = match diff.cmp(&0) {
            std::cmp::Ordering::Greater => diff + 1,
            std::cmp::Ordering::Less => diff - 1,
            std::cmp::Ordering::Equal => 0,
        };
        (v, 2)
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
        let pd = selective_probe_depth(empties);
        let error = t * selective_sigma(empties, pd) * self.sigma_scale;

        // NNUE probes: same depth and margins, but the probe search runs the
        // (unpruned) midgame NNUE engine instead of the linear seed search.
        // Off by default until its own sigma is calibrated.
        if sel_nnue_probe() {
            if let Some((nn, mtt)) = self.nnue {
                const EPS: f32 = 0.01;
                let mut ms = crate::midgame::NnueSearch::new(nn, mtt);
                let mut acc = nn.indices(board.black, board.white);
                let gate = selective_gate_offset()
                    .map(|off| (nn.eval_from_indices(&acc, board), (error - off).max(1.0)));
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

        // Static-eval gate: one static evaluation decides whether either
        // probe can reach its bound at all. A depth-0 seed search *is* that
        // evaluation, and it costs a fraction of the probe it guards.
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
        self.score_moves(board, out, tt_best, ev, i32::MIN / 2, parity_of(board));
    }

    /// Generate children with their incremental hashes but WITHOUT the
    /// expensive ordering evaluation. Wipeout moves are flagged: they end
    /// the game at +64, so a node that has one needs no search at all.
    fn gen_moves(&self, board: &Board, out: &mut MoveBuf) {
        self.gen_moves_bb(
            board.player_bb(),
            board.opponent_bb(),
            board.empty_count(),
            out,
        )
    }

    /// See `gen_moves`. The band above runs on raw bitboards, so the colour
    /// select a `Board` would need on every read is not paid here either.
    fn gen_moves_bb(&self, p: u64, o: u64, n_empties: u8, out: &mut MoveBuf) {
        let _prof = layer_profile::Scope::new(layer_profile::GEN, n_empties);
        out.len = 0;
        // `board.movable()` re-derives `p` and `o` through the colour select
        // and rebuilds the empty mask, but passing the ones already in hand
        // measured +0.15% over four sets - the optimizer had removed the
        // duplication already.
        #[cfg(feature = "gen-scalar")]
        let n = {
            // One flip kernel per square. `gen-scalar` prices the batched
            // kernels below against it: they share one board broadcast
            // across two or four squares, which is only a win if the
            // broadcast is a real share of the kernel.
            let mut m = bitboard::mobility(p, o, bitboard::empty_bb(p, o));
            let mut n = 0usize;
            while m != 0 {
                let s0 = m.trailing_zeros() as u8;
                m &= m - 1;
                out.write_at(
                    n,
                    ScoredMove {
                        pos: Position(s0),
                        flipped: bitboard::flippable(p, o, 1u64 << s0),
                        value: 0,
                    },
                );
                n += 1;
            }
            n
        };

        #[cfg(not(feature = "gen-scalar"))]
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
                // Padding the tail out to four so this kernel could run
                // unconditionally measured +0.30% over four sets: at an average
                // of 4.4 moves a node the padded flips are a large share, and
                // they cost more than the three-way branch they replace.
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
        // The prefetch below only fires for children at or above
        // `tt_min_empties()` - the 5-6 band has its own cache and the ordered
        // stage below nine consults no main table - so a node with seven,
        // eight or nine empties issues none at all, and the hash it would
        // compute per move exists only to feed a prefetch that never
        // happens. Those layers skip this pass entirely and let the descent
        // that needs a hash compute it. The condition is the same for every
        // move, so it is tested once, not per move.
        let child_empties = n_empties - 1;
        // Only the shared table is worth warming: the band below it now
        // fits a megabyte and sits in L2, where the load the prefetch would
        // hide costs less than the hash it needs. `gen-prefetch-all` keeps
        // the old shape, which warmed both.
        #[cfg(all(not(feature = "gen-eager-hash"), not(feature = "gen-prefetch-all")))]
        let eager = child_empties >= tt_min_empties();
        #[cfg(all(not(feature = "gen-eager-hash"), feature = "gen-prefetch-all"))]
        let eager = child_empties >= MOVE_ORDERING_LIMIT.min(tt_min_empties());
        #[cfg(feature = "gen-eager-hash")]
        let eager = true;
        if eager && !cfg!(feature = "gen-no-prefetch") {
            for i in 0..n {
                let m = out[i];
                let child_hash = zobrist::board_hash(o ^ m.flipped, p | m.flipped | m.pos.to_bit());
                // Prefetching every generated move, not just the ones the
                // node reaches, is the shape that measured best: gating it
                // by position in the move list costs 1.8% at four, 3.1% at
                // two and 5.1% with none at all.
                if child_empties >= tt_min_empties() {
                    self.table(child_empties).prefetch(child_hash);
                } else if child_empties >= MOVE_ORDERING_LIMIT {
                    self.l78_prefetch(child_hash);
                }
            }
        }
    }

    /// Fill in ordering values (the expensive part: pattern evaluation and
    /// pruned lookaheads). Split from generation so that a node whose
    /// transposition-table move already cuts never pays for it. Callers
    /// either sort the result or draw from it with `select_next`.
    /// The static half of `score_moves`, split out so the layers that use
    /// it do not carry the other half's frame.
    ///
    /// The evaluator path keeps 160 bytes of pattern indices plus its
    /// snapshot alive across the loop, and LLVM reserves that space in the
    /// prologue whether or not the branch is taken - 496 bytes of locals on
    /// every call, including the seven-to-thirteen-empty nodes that never
    /// touch them and are most of the nodes in the search. Splitting the
    /// two gives that band a frame it actually uses.
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

    /// See `score_moves_static`. The ordered band never builds a `Board`,
    /// and this is the only thing it used to need one for.
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
        // Split on the table move instead of testing for it per move. Most
        // nodes reach here without one - the staged table move is searched
        // and cut before any scoring happens - so the comparison is a
        // branch every scored move pays to answer "no". The wipeout test
        // stays: it is a compare against a value already in hand.
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
        ev: Option<&Evaluator>,
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
        // The carried pattern indices (see `Worker::order_ix`), updated per
        // candidate move and restored after. Building them here from the
        // bitboards - which is what this did - reads every cell of all 64
        // masks once per node, and measured more than the evaluation it
        // feeds.
        let mut order_ix = if eval_order {
            ev.map(|e| (order_indexer(e), self.order_ix))
        } else {
            None
        };
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
        let sort_depth = sort_depth_ladder()[(board.empty_count() as usize).min(63)];
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
                move_ordering_value(pos, cp, co, parity, pot)
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
        let n = moves.len();
        for j in i + 1..n {
            // SAFETY: `i <= best < n` holds on entry and is preserved, and
            // `j < n` is the loop bound. The checks this drops sat on the
            // inner comparison of the ordering pass.
            unsafe {
                if moves.get_unchecked(j).value < moves.get_unchecked(best).value {
                    best = j;
                }
            }
        }
        moves.swap(i, best);
    }
}

/// 16 bytes, not 24: the child hash used to live here so a prefetch could
/// be issued for it, but that prefetch only fires above `tt_min_empties()`
/// and the layers that carry most of the moves defer the hash to the
/// descent anyway. Carrying it for all of them cost a third of the move
/// buffer's width - and the buffer is the hottest structure in the search.
#[derive(Clone, Copy)]
struct ScoredMove {
    pos: Position,
    /// Discs this move flips. The child position is the parent with these
    /// recoloured, so storing the mask instead of a whole `Board` keeps the
    /// move list — and therefore every `pvs` stack frame — much smaller.
    flipped: u64,
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
            // Per-element `uninit()`, not `assume_init()` on the whole
            // array: the latter builds an `undef` aggregate that the
            // backend materializes, and it did — every `alpha_beta_ordered`
            // node called `_bzero` on all 552 bytes right before handing
            // the buffer to `gen_moves`, which writes only `0..len`.
            buf: [const { std::mem::MaybeUninit::uninit() }; MAX_MOVES],
            len: 0,
        }
    }

    /// Write one generated move. Unchecked: the caller's index counts
    /// legal moves, of which a position has at most 32, and the bounds
    /// check sat inside move generation's own loop.
    #[inline(always)]
    fn write_at(&mut self, i: usize, m: ScoredMove) {
        debug_assert!(i < MAX_MOVES);
        // SAFETY: `i < MAX_MOVES` by the legal-move bound above.
        unsafe { self.buf.get_unchecked_mut(i).write(m) };
    }

    /// One generated move by index. Unchecked: every caller has already
    /// established the index against `len`, and the checks sat in the
    /// ordered stage's move loop.
    #[inline(always)]
    fn at(&self, i: usize) -> ScoredMove {
        debug_assert!(i < self.len);
        // SAFETY: `0..len` were written by `write_at`.
        unsafe { self.buf.get_unchecked(i).assume_init() }
    }

    /// The moves from `from` on, for the scoring pass.
    #[inline(always)]
    fn tail_mut(&mut self, from: usize) -> &mut [ScoredMove] {
        debug_assert!(from <= self.len);
        // SAFETY: `0..len` were written by `write_at`, and `from <= len`.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.buf.as_mut_ptr().add(from) as *mut ScoredMove,
                self.len - from,
            )
        }
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
        unsafe {
            std::slice::from_raw_parts_mut(self.buf.as_mut_ptr() as *mut ScoredMove, self.len)
        }
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

/// `SHALLOW_ORDER` as a partition of the board: one mask per priority
/// weight, ascending. Walking these in turn visits the legal squares in
/// priority order without materializing or sorting a list.
const SHALLOW_TIERS: [u64; 13] = {
    let mut t = [0u64; 13];
    let mut sq = 0usize;
    while sq < 64 {
        t[SHALLOW_ORDER[sq] as usize] |= 1u64 << sq;
        sq += 1;
    }
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
        // A pass leaves the discs (and thus the indices) unchanged
        return -shallow_search(&p, ev, ix, indices, depth, -beta, -alpha);
    }
    let mut alpha = alpha;
    let mut best = f32::NEG_INFINITY;
    let mover = board.player();

    // Visit the legal squares in priority order by walking one priority
    // class at a time. Collecting the squares into an array and sorting
    // them produced the same order for a `memset` of the array, a scan, and
    // an insertion sort on every node of the ordering lookahead; the classes
    // are a compile-time partition of the board, so masking gives the order
    // directly. Reordering siblings cannot change the value alpha-beta
    // returns, only how quickly it prunes.
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
/// `ORDER_POT=0` drops the ordering to its three strongest terms.
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

    // The potential-mobility and parity terms are off by default now:
    // `ORDER_POT=1` restores them. The shared-corpus bench prices them at
    // 2.83 ns of the 12.51 ns a scored move costs, and they buy 0.05-0.94%
    // of the tree. Timed over four sets, six shuffled
    // rounds each: hard20 -0.75%, band22 -1.69%, 18-empty roots -0.80%,
    // FFO40-49 -1.54%, total -1.45%.
    //
    // An earlier round judged this on FFO40-49 alone and called it a wash
    // (-0.07%); it was measured before the move generator stopped writing
    // each entry two and three times, which is what made the ordering the
    // dominant term. The gate is hoisted to the caller: this runs 185M
    // times per FFO40-49 and the OnceLock read paid its acquire load on
    // every one of them.
    if !pot {
        let mut score = SQUARE_VALUE[(pos.index() & 63) as usize] as i32;
        score += corner_stability_bb(co) * W_CORNER_STABILITY;
        score += (36 - weighted_mobility(cp, co)) * W_MOBILITY;
        return -score;
    }
    let empty = !(cp | co);
    // Potential mobility counts the *opponent's* potential moves: the empty
    // squares next to
    // our discs. An earlier version counted our own frontier discs, which is
    // related but not the same quantity.
    let potential = (dilate(co) & empty).count_ones() as i32;
    let mut score = SQUARE_VALUE[(pos.index() & 63) as usize] as i32;
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

/// The four corner squares, used by the shallow visit order.
const CORNER_MASK: u64 = 0x8100_0000_0000_0081;

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
///
/// Branchless: each shift moves the owned corners of one board edge onto
/// their two neighbours, so a neighbour survives the final `& bb` only when
/// both it and its corner are ours. The loop form this replaces ran up to
/// twelve dependent test-and-branch pairs on a value the ordering needs for
/// every move; this is four shifts, five ORs, one AND and one popcount.
/// Layout is file-major, so `<< 1` steps a rank and `<< 8` steps a file.
#[inline]
fn corner_stability_bb(bb: u64) -> i32 {
    // Corners grouped by the board edge whose neighbour they own:
    // rank 1 (A1, H1), rank 8 (A8, H8), file A (A1, A8), file H (H1, H8).
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
        (
            ((v >> 8) as usize).min(PHASES - 1),
            (v & 0xFF) as usize & 63,
        )
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
    /// Per empties: which searched child produced the cut at nodes with
    /// two or more moves — first, second, later, or none (fail low).
    #[cfg(feature = "node-accounting")]
    static CUT_AT: [[AtomicU64; 4]; 64] = [const { [const { AtomicU64::new(0) }; 4] }; 64];

    /// Record where a multi-move node cut: `Some(i)` = the i-th child
    /// searched, `None` = no cut.
    #[inline(always)]
    pub(crate) fn cut_at(empties: u8, at: Option<usize>) {
        let _ = (empties, at);
        #[cfg(feature = "node-accounting")]
        {
            let slot = at.map_or(3, |i| i.min(2));
            CUT_AT[(empties as usize) & 63][slot].fetch_add(1, Relaxed);
        }
    }

    /// The cut distribution per empties, `[first, second, later, none]`.
    pub fn cut_dist() -> Vec<(u8, [u64; 4])> {
        #[cfg(feature = "node-accounting")]
        {
            (0..64u8)
                .map(|e| {
                    let c = &CUT_AT[e as usize];
                    (e, [0, 1, 2, 3].map(|i| c[i].load(Relaxed)))
                })
                .filter(|(_, c)| c.iter().sum::<u64>() > 0)
                .collect()
        }
        #[cfg(not(feature = "node-accounting"))]
        {
            Vec::new()
        }
    }

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

    /// The 5-6 band walks every empty square and calls a zero flip illegal.
    /// Generating the legal moves instead was measured (see the comment on
    /// that loop) and was a wash, but the equivalence it rests on is worth
    /// pinning: mobility must agree with the flip scan square for square,
    /// and the parity x corner subsets must partition it.
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
            // A full board minus a handful of squares, split between the
            // two sides: the shape this band actually sees.
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

    /// The ladder builder must gain exactly one ply at each of its three
    /// steps, wherever those steps are set.
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
        // The steps are ordered and distinct, so each one is worth a ply.
        const _: () = assert!(DEEP_ORDER_EMPTIES < DEEP2_ORDER_EMPTIES);
        const _: () = assert!(DEEP2_ORDER_EMPTIES < DEEP3_ORDER_EMPTIES);
    }

    /// The branch-per-corner form `corner_stability_bb` replaced.
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
        // Only twelve squares can contribute, so enumerate all of them
        // exhaustively and let random noise fill the rest of the board.
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
        // A1 (corner, sq 0): neighbours are A2(1), B1(8), B2(9)
        assert_eq!(neighbour_bit(0), (1u64 << 1) | (1u64 << 8) | (1u64 << 9));
        // Center square E5 (sq 36) has 8 neighbours
        assert_eq!(neighbour_bit(36).count_ones(), 8);
    }

    #[test]
    fn test_quadrant_parity() {
        let b = Board::new();
        // 60 empties, 15 per quadrant (each quadrant has 16 squares, minus
        // one initial disc each) -> every quadrant parity is odd
        assert_eq!(parity_of(&b), 0b1111);
    }
}

#[cfg(test)]
mod quadrant_id_tests {
    use super::quadrant_id;

    /// The branchless form must agree with the file/rank split it replaced
    /// on every square, not just the ones a game happens to reach.
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
