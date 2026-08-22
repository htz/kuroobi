//! Clock budgeting.
//!
//! Moved out of the GUI into the engine so the same allocator serves
//! both `arena` self-play and GGS — allocation quality can only be
//! measured in timed games, and inside the GUI it could not be.
//!
//! To add a scheme, extend [`Pace`] and branch in [`plan`]. Never change
//! the existing formulas: they are the baselines all measurements
//! compare against.

use std::time::Duration;

/// Clock allocation scheme.
///
/// Reduced to what measurement justified: against `even` (equal split
/// over remaining moves), `slow` scored 0.0% at fast controls while
/// `fast` never lost to `even` at any control — so `fast` is the only
/// scheme worth offering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pace {
    /// Short early moves, saving time for the endgame. Default.
    Fast,
    /// Read to the configured depth, ignoring the clock; the caller owns
    /// time management. Study only — guaranteed flag fall in timed games,
    /// so GGS does not offer it.
    Depth,
    /// Direct tail coefficient: `a + (1-a)/sqrt(moves_left)`.
    /// `a = 1.0` is the equal split, `a = 0.6` equals [`Pace::Fast`].
    /// Kept so the slope can be re-examined against the same formula
    /// family in self-play.
    Tail(f64),
}

impl Pace {
    /// Parse the GUI/GGS string; unknown words fall to the default
    /// ([`Pace::Fast`]). Deliberately not `FromStr`, so it cannot fail —
    /// this comes from settings, and the removed `slow`/`even` in an old
    /// config file must not resurrect a harmful scheme.
    pub fn parse(s: &str) -> Pace {
        // A coefficient may be passed as `tail:0.4` (measurement).
        if let Some(a) = s.strip_prefix("tail:") {
            if let Ok(v) = a.parse::<f64>() {
                return Pace::Tail(v.clamp(0.0, 1.0));
            }
        }
        match s {
            "depth" => Pace::Depth,
            _ => Pace::Fast,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Pace::Fast => "fast",
            Pace::Depth => "depth",
            Pace::Tail(_) => "tail",
        }
    }
}

/// Strength settings (midgame depth / solve entry / selective band).
#[derive(Debug, Clone, Copy)]
pub struct Levels {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
    /// Whether the selective band derives from the budget (true in
    /// timed play); false uses `band` as-is (fixed depth, legacy comparison).
    pub auto_band: bool,
}

/// Plan for one move.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
    /// Deadline for this move; `None` = no clock (fixed depth).
    pub cap: Option<Duration>,
}

/// Inputs for the decision.
#[derive(Debug, Clone, Copy)]
pub struct Situation {
    /// Our remaining clock in seconds; `None` = untimed. Overtime shows
    /// here too — GGS runs one clock and adds the grace onto it.
    pub clock_secs: Option<u64>,
    /// Whether we are in overtime. Once there the game is already lost
    /// and the only goal is avoiding the wipeout (`timeout_hard`). Not
    /// derivable from `clock_secs` (post-addition it looks healthy);
    /// the caller sets it after seeing the clock jump.
    pub in_overtime: bool,
    /// Grace time as configured (GGS display's third field), not the
    /// remainder. 0 means main-time expiry is an immediate wipeout.
    pub grace_secs: u64,
    /// Empty squares on the board.
    pub empties: u8,
    /// Per-move cap in seconds (0 = none).
    pub max_move_secs: u64,
    /// Seconds reserved for the perfect solve.
    pub reserve_secs: u64,
    /// How aggressively to spend the clock; 1.0 = exactly as allocated.
    ///
    /// Iterative deepening returns once the next iteration would not
    /// fit, spending ~47% of its deadline, so 1.0 uses only about half
    /// the allocation. Default 2.5 was measured in 15-minute synchro
    /// games (45% -> 84% utilization); the added time turns into depth
    /// and no deadline was exceeded. Non-positive or NaN falls back to
    /// the default — this value comes from settings.
    pub budget_use: f64,
    /// Calibrated solver nodes/sec
    /// ([`crate::engine::Engine::measure_solve_nps`]); `None` drops the
    /// solve entry to the fixed ladder.
    pub nps: Option<f64>,
    /// Search threads; needed to estimate the parallel node overhead.
    pub threads: usize,
    /// Solve reference used to count remaining moves:
    /// `(empties - this) / 2` is "how many more moves we make". Default
    /// is fixed; the machine-derived [`SolveRef::Auto`] is opt-in until
    /// measured.
    pub solve_ref: SolveRef,
}

/// How the move-count reference is chosen.
///
/// Fixed by default: a similar calibration-driven change once cost 20pt
/// of win rate at fast controls (see
/// `calibration_does_not_move_the_move_budget`), so [`SolveRef::Auto`]
/// stays opt-in until self-play confirms it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SolveRef {
    /// Fixed value (default [`SOLVE_REF`]).
    Fixed(u8),
    /// Derived from machine speed, threads and clock ([`auto_solve_ref`]).
    Auto,
}

impl SolveRef {
    /// Parse from UI/args; unknown words fall to the fixed default.
    pub fn parse(s: &str) -> SolveRef {
        if s == "auto" {
            return SolveRef::Auto;
        }
        s.parse()
            .map(SolveRef::Fixed)
            .unwrap_or(SolveRef::Fixed(SOLVE_REF))
    }

    fn value(self, clock_secs: u64, nps: Option<f64>, threads: usize) -> u8 {
        /* Escape hatch for live trials: the self-play bench cannot
        reproduce live conditions (endgame cost share differs 50x), so
        adoption can only be judged in GGS games. `SOLVE_REF=auto`
        toggles it before it earns a GUI setting; same treatment as
        `budget_use`. */
        static ENV: std::sync::OnceLock<Option<SolveRef>> = std::sync::OnceLock::new();
        let env = *ENV.get_or_init(|| std::env::var("SOLVE_REF").ok().map(|v| SolveRef::parse(&v)));
        match env.unwrap_or(self) {
            SolveRef::Fixed(v) => v,
            SolveRef::Auto => auto_solve_ref(clock_secs, nps, threads),
        }
    }
}

impl Default for Situation {
    fn default() -> Situation {
        Situation {
            clock_secs: None,
            in_overtime: false,
            grace_secs: 0,
            empties: 60,
            max_move_secs: 0,
            reserve_secs: 20,
            budget_use: 2.5,
            nps: None,
            threads: 1,
            solve_ref: SolveRef::Fixed(SOLVE_REF),
        }
    }
}

/* ---- Deriving the solve entry point from the clock --------------------

A solve cannot be sliced: `Engine::choose_within`'s deadline only works
on iterative deepening, so the cost must be predicted before entering.
The fixed ladder knows nothing about machine speed — a value right for
the dev box flags on one half as fast.

The estimate is layered:

    time = base_nodes(empties) x parallel_factor(threads, empties) / nps

- base_nodes: single-thread measurement (110 positions), machine-invariant
- parallel factor: tree growth per thread count
- nps: the only machine-dependent part (`Engine::measure_solve_nps`)

Split so a machine change re-measures one number instead of the whole
(empties x threads) grid. */

/// Median solve nodes (1 thread) as `A * exp(B * empties)`, fitted to
/// 110 measured positions at 14-30 empties; `exp(B) = 1.999`, i.e. a
/// branching factor of exactly 2.0.
const SOLVE_NODES_A: f64 = 2.82;
const SOLVE_NODES_B: f64 = 0.693;

/// Extra nodes searched in parallel. Measured 1.15 at 2T / 1.22 at 5T
/// (22-30 empties); proportional to log2(threads) fits. Shallow
/// positions show no overhead, so it ramps in over 14-22 empties.
fn parallel_overhead(threads: usize, empties: u8) -> f64 {
    if threads <= 1 {
        return 1.0;
    }
    let ramp = ((empties as f64 - 14.0) / 8.0).clamp(0.0, 1.0);
    1.0 + 0.09 * (threads as f64).log2() * ramp
}

/// Ratio converting calibrated nps to deep-position nps. Calibration
/// runs at 22 empties (sub-second); deeper positions overflow the table
/// and lose nps (24.9M -> 20.5M single-threaded, 22 -> 30 empties).
const DEEP_NPS_RATIO: f64 = 0.9;

/// Safety factor over the median: same-empties positions spread up to
/// 5.7x. The loss is asymmetric — entering late just means a selective
/// move, entering and not finishing means a flag fall.
const SOLVE_SAFETY: f64 = 3.0;

/// Total solve time through the end of the game over the first solve.
/// Entering at E commits to E-2, E-4, ... too; with branching factor 2
/// the series sums to ~4/3, so the decision is priced for all of them.
const SOLVE_TOTAL_FACTOR: f64 = 4.0 / 3.0;

/// Estimated seconds to solve at `empties` (safety factor included).
pub fn solve_secs(empties: u8, nps: f64, threads: usize) -> f64 {
    if nps <= 0.0 {
        return f64::INFINITY;
    }
    let nodes = SOLVE_NODES_A * (SOLVE_NODES_B * empties as f64).exp();
    nodes * parallel_overhead(threads, empties) / (nps * DEEP_NPS_RATIO)
        * SOLVE_SAFETY
        * SOLVE_TOTAL_FACTOR
}

/// Largest solvable empties within `budget_secs` (capped at `max`);
/// 0 if none fits (= don't enter the solve).
pub fn solve_entry(budget_secs: f64, nps: f64, threads: usize, max: u8) -> u8 {
    (0..=max)
        .rev()
        .find(|&e| solve_secs(e, nps, threads) <= budget_secs)
        .unwrap_or(0)
}

/// Upper bound on the timed-play solve entry. Not capped by the
/// configured value — calibration decides; this is the physical wall
/// (32 empties exceeds 10 minutes single-threaded).
const SOLVE_CEILING: u8 = 32;

/// Midgame depth in timed play: uncapped, the deadline is the only
/// stopper. A cap wasted 90% of the budget (5- and 10-minute games both
/// used 28 seconds, both stuck at depth 24). To play weaker, play
/// untimed — a clock means full strength.
const DEPTH_BY_CLOCK: u32 = 60;

/// Solve reference for counting remaining moves:
/// `(empties - ref) / 2` = our moves left.
///
/// Never the time-derived entry — deepening the entry would shrink the
/// move count and fatten every budget (a 9% fatter budget cost 20pt at
/// 3-second controls). Not the configured entry either, or the strength
/// setting would leak into the budget. A fixed yardstick is fine.
const SOLVE_REF: u8 = 18;

/// The empties where solving becomes effectively free; the move-count
/// reference derives from here.
///
/// The fixed 18 was a legacy default that ignored machine speed,
/// threads and clock, yet the measured solve cost differs 50x between
/// live play (15 min / 8T: 0.13% of the clock) and the bench
/// (60s / 1T: 6.7%) — the same number is right in one and wrong in the
/// other (a fixed 28 flagged 10 of 120 bench games). So derive it:
/// the largest empties whose estimated solve ([`solve_secs`]) fits
/// [`SOLVE_REF_SHARE`] of the clock — 28 at 15min/8T, 21 at 60s/1T.
/// Floor = the legacy value; ceiling = [`SOLVE_REF_MAX`].
pub fn auto_solve_ref(clock_secs: u64, nps: Option<f64>, threads: usize) -> u8 {
    let Some(nps) = nps else {
        // Uncalibrated = unknown machine speed; the legacy value beats
        // a guess.
        return SOLVE_REF;
    };
    let budget = clock_secs as f64 * SOLVE_REF_SHARE;
    solve_entry(budget, nps, threads, SOLVE_REF_MAX).max(SOLVE_REF)
}

/// Clock share allowed for one solve — only for picking the reference,
/// not the entry decision (that is [`SOLVE_GREED`] / [`SOLVE_MAX_SHARE`]).
/// 5% of 15 minutes is 45s; fine, since a solve reads the whole rest of
/// the game in one move.
const SOLVE_REF_SHARE: f64 = 0.05;

/// Reference ceiling, kept below the physical wall ([`SOLVE_CEILING`]):
/// as the reference nears the empty count, the move count collapses to
/// 1 and pacing differences vanish.
const SOLVE_REF_MAX: u8 = 30;

/// Derive the selective band from the per-move budget. The band extends
/// probabilistic solving above the entry point
/// ([`crate::midgame::selective_band`]); with the entry derived from
/// time, the band should be too. Steps mirror the legacy settings.
fn band_for(budget: f64) -> u8 {
    if budget < 12.0 {
        0
    } else if budget < 60.0 {
        6
    } else {
        8
    }
}

/// Solve time allowance as a multiple of the move budget.
///
/// The solve gets no separate budget: an independent "N% of remaining"
/// once decided 24s was fine in a 30-second game and left 0.9s on the
/// clock. Tied to the move budget, the decision tightens automatically
/// as time runs down; the multiplier exceeds 1 because a solve sees the
/// game to the end.
const SOLVE_GREED: f64 = 10.0;

/// Cap on remaining time a solve may take. The multiplier alone let a
/// 1-move-left budget balloon past the remaining time (a 50s allowance
/// with 20s left); capping at the reserve limit (`remaining/2`) keeps
/// the decision consistent with what is actually available.
const SOLVE_MAX_SHARE: f64 = 0.5;

/// Correction matching allocation to actual usage. Deepening returns at
/// ~47% of its deadline, so the deadline is stretched by the inverse;
/// 2.0 turns 47% into ~94%. No added flag risk: the watcher enforces
/// the deadline (measured 1.02x) and every reserve scales with the
/// remaining clock.
/// Validate the setting; the environment variable wins (for sweeps).
fn effective_budget_use(from_setting: f64) -> f64 {
    static ENV: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    let env = *ENV.get_or_init(|| {
        std::env::var("BUDGET_USE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
    });
    if let Some(v) = env {
        return v;
    }
    if from_setting.is_finite() && from_setting > 0.0 {
        from_setting
    } else {
        2.5
    }
}

/* ---------- Playing in overtime ----------

The game is already lost; the only job is finishing it without a
wipeout. Depth has no value (the result is capped at
`min(minimal_loss, board_score)`), so every constant leans toward
"reliably finish" over "read". */

/// Seconds kept free in overtime — estimate misses, round-trips,
/// game-end handling. Crossing it is `timeout_hard`, the maximal loss.
const OVERTIME_RESERVE: u64 = 5;

/// Per-move cap in overtime, applied after the per-move split — a long
/// think cannot change the outcome.
const OVERTIME_MAX_SECS: f64 = 1.5;

/// Solve entry allowed in overtime. A missed estimate overruns by an
/// order of magnitude, and this is exactly the difference between a
/// minimal loss and a -64 wipeout.
const OVERTIME_SOLVE: u8 = 12;

/// Reserve multiplier for graceless games: without grace, main-time
/// expiry is a wipeout rather than a minimal loss — 60 discs apart —
/// so the same reserve is not balanced. Only matters when grace is off.
const NO_GRACE_RESERVE_MUL: u64 = 2;

/// Plan one move. Our remaining moves are roughly half the empties
/// (passes push it down); the budget-to-depth ladder is measured and
/// leans safe because deeper settings cost sharply more per move.
pub fn plan(s: Situation, base: Levels, pace: Pace) -> Plan {
    // Fixed depth: read as configured, ignore the clock.
    if pace == Pace::Depth {
        return Plan {
            depth: base.depth,
            solve: base.solve,
            band: base.band,
            cap: None,
        };
    }
    let Some(secs) = s.clock_secs else {
        return Plan {
            depth: base.depth,
            solve: base.solve,
            band: base.band,
            cap: None,
        };
    };
    /* ---------- Overtime (GGS) ----------

    Entering overtime means the game is already lost. GGS runs a single
    clock; on main-time expiry the server sets `timeout_soft` and adds
    the grace back onto it. Othello is a soft-timeout game, so from that
    point the result is capped at `min(minimal_loss, board_score)` — a
    win is unreachable. Overtime is, per COsClock, "additional time to
    avoid a wipeout"; exhausting it too is `timeout_hard`, the maximal
    loss. The only job here is to reliably finish the remaining moves.

    Note `ext_secs` is the configured grace, not the remainder: the
    display's third field never moves; the remainder shows in the first
    field even during overtime. */
    if s.in_overtime {
        let moves = ((s.empties as f64 / 2.0).ceil() as u64).max(1);
        let pool = secs.saturating_sub(OVERTIME_RESERVE) as f64;
        let per = (pool / moves as f64).min(OVERTIME_MAX_SECS);
        return Plan {
            depth: DEPTH_BY_CLOCK,
            // A solve reads everything at once; a miss here falls all
            // the way to the wipeout. Keep the entry shallow.
            solve: base.solve.min(OVERTIME_SOLVE),
            band: 0,
            cap: Some(Duration::from_secs_f64(per.max(0.05))),
        };
    }
    let avail = secs;
    /* Our remaining moves (at least 1); the solve reads everything in
    one move, so only the span above the entry gets budgeted.

    Count from the fixed reference, never the time-derived entry:
    deepening the entry would shrink the count and fatten every budget
    (9% fatter cost 20pt at 3-second controls). Pacing slope belongs to
    [`Pace`] alone. `reserve` stays uncalibrated for the same reason —
    a calibrated reserve would thin the midgame allocation by 20% in a
    900-second game. */
    let solve_ref = s.solve_ref.value(avail, s.nps, s.threads);
    let my_moves = ((s.empties.saturating_sub(solve_ref) as f64 / 2.0).ceil() as u64).max(1);
    /* Reserve one solve's worth, then allocate the midgame. Graceless
    games reserve more: the same one-second overrun costs 60 more discs
    without grace. */
    let want = if s.grace_secs == 0 {
        s.reserve_secs * NO_GRACE_RESERVE_MUL
    } else {
        s.reserve_secs
    };
    let reserve = want.min(avail / 2);
    let pool = avail.saturating_sub(reserve) as f64;
    let even = pool / my_moves as f64;
    /* Pacing. `even` is no longer selectable but stays as the baseline
    the coefficients multiply; rewriting the formula would orphan every
    past measurement. */
    let root = (my_moves as f64).sqrt();
    let budget = match pace {
        Pace::Fast => even * (0.6 + 0.4 / root),
        Pace::Tail(a) => even * (a + (1.0 - a) / root),
        // Depth returned earlier.
        _ => even,
    };
    /* Compensate for deepening's underspend: the search returns at
    ~47% of its deadline, which left 6-9 of 15 minutes unused in rated
    games (we 40-46%, opponents 98-99%). Stretching the deadline scales
    that 47% proportionally — it buys depth, not waiting. */
    let budget = budget * effective_budget_use(s.budget_use);
    /* Never promise more than remains: with `BUDGET_USE`, one move
    left could budget twice the available time. Cap at "everything
    allocatable" — a fractional cap (remaining x 0.25) kicked in too
    early in realistic games and erased the pacing differences. */
    let budget = budget.min(pool);
    let budget = if s.max_move_secs > 0 {
        budget.min(s.max_move_secs as f64)
    } else {
        budget
    };
    // Depth is passed as a cap (the deadline decides the reach); only
    // the solve entry must be derived from the clock.
    let solve = match s.nps {
        /* Calibrated: derive the solvable empties from the remaining
        clock, uncapped by the configured value — with a clock, this is
        not a human's number to set (a 30-minute game once stayed stuck
        at the configured 26). Ceiling is the physical wall. */
        Some(nps) => {
            let b = (budget * SOLVE_GREED).min(avail as f64 * SOLVE_MAX_SHARE);
            solve_entry(b, nps, s.threads, SOLVE_CEILING)
        }
        // Uncalibrated: the fixed ladder. A guess — on slow machines
        // the configured entry passes through and can flag.
        None if avail < 20 => base.solve.min(14),
        None if avail < 60 => base.solve.min(20),
        None => base.solve,
    };
    let band = if base.auto_band {
        band_for(budget)
    } else {
        // Legacy: the configured band, only when there is budget.
        if budget >= 12.0 {
            base.band
        } else {
            0
        }
    };
    Plan {
        // No depth cap: the deadline decides (solves are abortable now).
        depth: DEPTH_BY_CLOCK,
        solve,
        band,
        cap: Some(Duration::from_secs_f64(budget.max(0.05))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Levels = Levels {
        depth: 22,
        solve: 26,
        band: 6,
        auto_band: false,
    };

    fn cap_secs(secs: u64, empties: u8, pace: Pace) -> f64 {
        plan(
            Situation {
                clock_secs: Some(secs),
                grace_secs: 120,
                empties,
                ..Situation::default()
            },
            BASE,
            pace,
        )
        .cap
        .unwrap()
        .as_secs_f64()
    }

    /// Fixed depth carries no deadline.
    #[test]
    fn depth_has_no_deadline() {
        let p = plan(
            Situation {
                clock_secs: Some(30),
                grace_secs: 120,
                empties: 40,
                ..Situation::default()
            },
            BASE,
            Pace::Depth,
        );
        assert!(p.cap.is_none());
        assert_eq!(p.depth, BASE.depth);
    }

    /// The default allocates thin early moves: it must stay below the
    /// equal split (`Tail(1.0)`), or it would equal the removed `even`.
    /// Thicker measured weaker (a 1.18x-thicker `slow` scored 0.0%).
    #[test]
    fn the_default_is_thin_in_the_opening() {
        let even = cap_secs(600, 60, Pace::Tail(1.0));
        let fast = cap_secs(600, 60, Pace::Fast);
        assert!(
            fast < even,
            "default {fast} must be below equal split {even}"
        );
    }

    /// Removed words fall to the default; an old config with `slow`
    /// must not resurrect a harmful scheme.
    #[test]
    fn dropped_names_fall_back_to_the_default() {
        for s in ["slow", "even", "", "something"] {
            assert_eq!(Pace::parse(s), Pace::Fast, "{s:?}");
        }
        assert_eq!(Pace::parse("depth"), Pace::Depth);
        assert_eq!(Pace::parse("tail:0.4"), Pace::Tail(0.4));
    }

    /// At zero on the clock, move immediately even with grace
    /// configured: grace is added by the server, and thinking here
    /// falls toward the wipeout before it arrives.
    #[test]
    fn a_zero_clock_moves_at_once() {
        let p = plan(
            Situation {
                clock_secs: Some(0),
                grace_secs: 120,
                empties: 20,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        );
        assert!(p.cap.unwrap() <= Duration::from_millis(100));
    }

    /// A shrinking clock never budgets zero — zero would return no move.
    #[test]
    fn budget_never_reaches_zero() {
        for secs in [1, 2, 5, 10] {
            assert!(cap_secs(secs, 60, Pace::Fast) >= 0.05);
        }
    }

    /// `Tail(0.6)` must equal the default, pinning the coefficient
    /// family to the same formula; `Tail(1.0)` remains the equal-split
    /// baseline.
    #[test]
    fn tail_is_continuous_with_the_default() {
        /* Low-empties cases excluded: with 1-2 moves left the
        never-promise-more-than-remains cap flattens every scheme to
        the same value — the cap winning, not the formula breaking. */
        for empties in [60u8, 44] {
            let f = cap_secs(600, empties, Pace::Fast);
            assert!((cap_secs(600, empties, Pace::Tail(0.6)) - f).abs() < 1e-9);
            // Equal split is thicker than the default (the removed
            // baseline formula still works).
            assert!(cap_secs(600, empties, Pace::Tail(1.0)) > f);
        }
        /* With 2 moves left the cap wins regardless of scheme. Count
        empties from SOLVE_REF (the fixed reference): reference + 4
        empties = 2 moves. */
        let two_left = SOLVE_REF + 4;
        let late = cap_secs(600, two_left, Pace::Fast);
        assert!((cap_secs(600, two_left, Pace::Tail(1.0)) - late).abs() < 1e-9);
    }

    /// The reference derives from machine and clock; the same number is
    /// right in one condition and wrong in another (a fixed 28 flagged
    /// 10 of 120 bench games at 60s/1T).
    #[test]
    fn the_reference_follows_the_machine() {
        // Live GGS: 15 min, 8 threads, 130M nodes/sec.
        let live = auto_solve_ref(900, Some(130e6), 8);
        // Bench: 60s, 1 thread, 13.8M nodes/sec.
        let bench = auto_solve_ref(60, Some(13.8e6), 1);
        assert!(
            live > bench,
            "live play should afford a deeper reference ({live} vs {bench})"
        );
        assert!(
            (26..=30).contains(&live),
            "live reference out of range: {live}"
        );
        assert!(
            (18..=24).contains(&bench),
            "bench reference out of range: {bench}"
        );
    }

    /// Never thinner than the legacy value: on slow machines a lower
    /// reference would shrink every move budget below what it was.
    #[test]
    fn the_reference_never_goes_below_the_old_default() {
        for (clock, nps, threads) in [(3u64, 1e6, 1), (10, 5e5, 1), (60, 1e5, 2)] {
            assert_eq!(auto_solve_ref(clock, Some(nps), threads), SOLVE_REF);
        }
    }

    /// Uncalibrated machines get no guess (legacy value stands).
    #[test]
    fn without_calibration_the_reference_is_the_old_default() {
        assert_eq!(auto_solve_ref(900, None, 8), SOLVE_REF);
    }

    /// Longer clocks raise the reference, but never past the ceiling.
    #[test]
    fn the_reference_is_capped() {
        assert!(auto_solve_ref(60, Some(130e6), 8) <= auto_solve_ref(1800, Some(130e6), 8));
        assert!(auto_solve_ref(36_000, Some(130e6), 8) <= SOLVE_REF_MAX);
    }

    /// Smaller coefficients allocate thinner early moves.
    #[test]
    fn smaller_tail_is_thinner_in_the_opening() {
        let a = cap_secs(600, 60, Pace::Tail(0.6));
        let b = cap_secs(600, 60, Pace::Tail(0.25));
        assert!(b < a, "0.25 {b} < 0.6 {a}");
    }

    /// Calibration may move only the solve entry. Reaching into the
    /// move budget changes the pacing slope and contaminates the
    /// measurement — it did: counting moves from the calibrated entry
    /// fattened budgets 9% and cost 20pt at 3-second controls.
    #[test]
    fn calibration_does_not_move_the_move_budget() {
        for secs in [3u64, 10, 30, 600] {
            for empties in [60u8, 40, 30] {
                let sit = |nps| Situation {
                    clock_secs: Some(secs),
                    empties,
                    threads: 5,
                    nps,
                    ..Situation::default()
                };
                assert_eq!(
                    plan(sit(None), BASE, Pace::Fast).cap,
                    plan(sit(Some(90e6)), BASE, Pace::Fast).cap,
                    "move budget moved at {secs}s, {empties} empties"
                );
            }
        }
    }

    /// Never approve a solve whose estimate exceeds the remaining
    /// clock — that is the whole point. The multiplier-only version
    /// failed here (a 50s allowance with 20s left).
    #[test]
    fn never_promises_more_than_the_clock() {
        for &nps in &[6e6, 23e6, 90e6] {
            for threads in [1usize, 5] {
                for secs in [3u64, 10, 30, 60, 300] {
                    for empties in [60u8, 44, 30, 26] {
                        let p = plan(
                            Situation {
                                clock_secs: Some(secs),
                                empties,
                                threads,
                                nps: Some(nps),
                                ..Situation::default()
                            },
                            BASE,
                            Pace::Fast,
                        );
                        if p.solve == 0 {
                            continue;
                        }
                        let need = solve_secs(p.solve, nps, threads);
                        assert!(
                            need <= secs as f64,
                            "nps {nps:e}, {threads}T, {secs}s, {empties} empties: \
                             entering a solve estimated at {need:.1}s for {} empties",
                            p.solve
                        );
                    }
                }
            }
        }
    }

    /// The entry gets shallower as the clock shrinks — proof it tracks
    /// the budget.
    #[test]
    fn the_entry_follows_the_clock() {
        let at = |secs| {
            plan(
                Situation {
                    clock_secs: Some(secs),
                    empties: 40,
                    threads: 1,
                    nps: Some(23e6),
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .solve
        };
        assert!(at(3) <= at(10), "3s {} <= 10s {}", at(3), at(10));
        assert!(at(10) <= at(60), "10s {} <= 60s {}", at(10), at(60));
        // Not capped by the configured entry: with time, go deeper.
        assert!(
            at(600) > BASE.solve,
            "600s {} > configured {} (calibration decides)",
            at(600),
            BASE.solve
        );
        assert!(at(600) <= SOLVE_CEILING, "never past the physical wall");
    }

    /// With a clock the depth cap comes off; only the deadline stops
    /// the search (a cap once left 5- and 10-minute games both using
    /// 28 seconds).
    #[test]
    fn a_clock_lifts_the_depth_cap() {
        let timed = plan(
            Situation {
                clock_secs: Some(600),
                empties: 44,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        );
        assert!(timed.depth > BASE.depth, "{} > {}", timed.depth, BASE.depth);

        // Untimed: as configured (the way to play weaker).
        let untimed = plan(Situation::default(), BASE, Pace::Fast);
        assert_eq!(untimed.depth, BASE.depth);
        assert!(untimed.cap.is_none());

        // Fixed depth: as configured (study).
        let fixed = plan(
            Situation {
                clock_secs: Some(600),
                ..Situation::default()
            },
            BASE,
            Pace::Depth,
        );
        assert_eq!(fixed.depth, BASE.depth);
    }

    /// A per-move cap caps the budget.
    #[test]
    fn max_move_caps_the_budget() {
        let p = plan(
            Situation {
                clock_secs: Some(600),
                empties: 60,
                max_move_secs: 3,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        );
        assert!(p.cap.unwrap() <= Duration::from_secs(3));
    }

    /// One overtime move (the remainder shows in the clock's first
    /// field; the third field is the configured grace).
    fn ot_plan(left: u64, empties: u8) -> Plan {
        plan(
            Situation {
                clock_secs: Some(left),
                in_overtime: true,
                grace_secs: 120,
                empties,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        )
    }

    /// No deep thinking in overtime — the result is capped, depth buys
    /// nothing. The same remaining seconds on main time may be spent
    /// freely; the two must clearly differ.
    #[test]
    fn overtime_is_far_cheaper_than_main_time() {
        let ot = ot_plan(120, 40).cap.unwrap();
        let main = plan(
            Situation {
                clock_secs: Some(120),
                empties: 40,
                ..Situation::default()
            },
            BASE,
            Pace::Fast,
        )
        .cap
        .unwrap();
        assert!(ot <= Duration::from_secs_f64(OVERTIME_MAX_SECS));
        assert!(ot < main, "overtime spends like main time");
    }

    /// Never fully trust setting-sourced values; running on the default
    /// beats stopping the game on a broken value.
    #[test]
    fn a_broken_budget_use_falls_back_to_the_default() {
        let with = |v: f64| {
            plan(
                Situation {
                    clock_secs: Some(600),
                    empties: 44,
                    budget_use: v,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .cap
            .unwrap()
        };
        let good = with(2.5);
        assert_eq!(with(0.0), good, "0 must fall to the default");
        assert_ne!(with(1.0), good, "1.0 must differ from the default");
        assert_eq!(with(-1.0), good, "negatives must fall to the default");
        assert_eq!(with(f64::NAN), good, "NaN must fall to the default");
        assert_eq!(
            with(f64::INFINITY),
            good,
            "infinity must fall to the default"
        );
    }

    /// Larger values budget longer moves.
    #[test]
    fn a_larger_budget_use_thinks_longer() {
        let with = |v: f64| {
            plan(
                Situation {
                    clock_secs: Some(600),
                    empties: 44,
                    budget_use: v,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .cap
            .unwrap()
        };
        assert!(with(1.0) < with(2.0));
        assert!(with(2.0) < with(3.0));
    }

    /// No wipeout: playing out every remaining move must not exhaust
    /// the grace (`timeout_hard` costs 60 discs more than a minimal loss).
    #[test]
    fn overtime_finishes_the_game_without_a_wipeout() {
        for grace in [120u64, 60, 30] {
            let mut left = grace as f64;
            // From 60 empties one move at a time, worst case: we play
            // every move ourselves.
            for e in (0..=60u8).rev() {
                let used = ot_plan(left.max(0.0) as u64, e).cap.unwrap().as_secs_f64();
                left -= used;
                assert!(left > 0.0, "grace {grace}s exhausted at {e} empties");
            }
        }
    }

    /// Moves shorten as the grace runs down.
    #[test]
    fn overtime_shrinks_as_the_grace_runs_down() {
        let much = ot_plan(120, 40).cap.unwrap();
        let little = ot_plan(8, 40).cap.unwrap();
        assert!(little < much);
    }

    /// Graceless games allocate cautiously: main-time expiry is a
    /// wipeout, not a minimal loss.
    #[test]
    fn no_grace_means_a_thicker_reserve() {
        let budget = |grace: u64| {
            plan(
                Situation {
                    clock_secs: Some(60),
                    grace_secs: grace,
                    empties: 40,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .cap
            .unwrap()
        };
        assert!(
            budget(0) < budget(120),
            "grace on/off must change the allocation"
        );
    }

    /// A solve reads everything at once; overtime keeps the entry shallow.
    #[test]
    fn overtime_keeps_the_solver_shallow() {
        let p = ot_plan(120, 20);
        assert!(p.solve <= OVERTIME_SOLVE);
        assert_eq!(p.band, 0);
    }
}

#[cfg(test)]
mod clock_usage_tests {
    use super::*;

    const BASE: Levels = Levels {
        depth: 22,
        solve: 26,
        band: 6,
        auto_band: false,
    };

    /// Whole-game clock utilization. Deepening returns at ~47% of its
    /// deadline; simulate a full 15-minute game at that rate and check
    /// both utilization and that time never runs out (rated games used
    /// to finish at 40-46% vs opponents' 98-99%; `BUDGET_USE` closes
    /// that gap).
    fn play_out(use_ratio: f64) -> (f64, bool) {
        let mut left = 900.0_f64;
        let mut empties = 48u8;
        let mut spent = 0.0;
        let mut ran_out = false;
        /* Only the span above the solve entry matters: past it one
        solve makes the rest nearly free, and BUDGET_USE only acts on
        this span. */
        while empties > BASE.solve {
            let p = plan(
                Situation {
                    clock_secs: Some(left as u64),
                    empties,
                    nps: Some(60e6),
                    threads: 4,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            );
            let take = p.cap.map_or(0.0, |c| c.as_secs_f64()) * use_ratio;
            left -= take;
            spent += take;
            if left <= 0.0 {
                ran_out = true;
                break;
            }
            // One move each for us and the opponent.
            empties = empties.saturating_sub(2);
        }
        (spent / 900.0, ran_out)
    }

    #[test]
    fn the_clock_is_actually_used() {
        let (rate, out) = play_out(0.47);
        println!("  midgame spend rate (measured 47%): {:.0}%", rate * 100.0);
        println!(
            "  equivalent before BUDGET_USE:      {:.0}%",
            play_out(0.235).0 * 100.0
        );
        println!(
            "  if deadlines were fully used:      {:.0}%",
            play_out(1.0).0 * 100.0
        );
        assert!(!out, "ran out of clock");
        assert!(
            rate > 0.60,
            "only {:.0}% of the clock used over a game",
            rate * 100.0
        );
    }

    /// Even spending every deadline in full must not flag.
    #[test]
    fn even_full_use_does_not_run_out() {
        let (_, out) = play_out(1.0);
        assert!(!out, "using full deadlines flags");
    }
}
