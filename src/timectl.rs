//! Clock budgeting.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pace {
    Fast,
    Depth,
    Tail(f64),
}

impl Pace {
    pub fn parse(s: &str) -> Pace {
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

#[derive(Debug, Clone, Copy)]
pub struct Levels {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
    pub cap: Option<Duration>,
    /// Comes out of the moves after it: the next plan reads the clock actually left.
    pub extend: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
pub struct Situation {
    pub clock_secs: Option<u64>,
    pub in_overtime: bool,
    pub grace_secs: u64,
    pub empties: u8,
    pub max_move_secs: u64,
    pub reserve_secs: u64,
    pub budget_use: f64,
    pub nps: Option<f64>,
    pub threads: usize,
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
        }
    }
}

const SOLVE_NODES_A: f64 = 2.82;
const SOLVE_NODES_B: f64 = 0.693;

fn parallel_overhead(threads: usize, empties: u8) -> f64 {
    if threads <= 1 {
        return 1.0;
    }
    let ramp = ((empties as f64 - 14.0) / 8.0).clamp(0.0, 1.0);
    1.0 + 0.09 * (threads as f64).log2() * ramp
}

const DEEP_NPS_RATIO: f64 = 0.9;

/// Typical cost: 40 live 29-30-empty solves (tables warmed 5 s) took a median 0.8x this.
pub fn solve_secs(empties: u8, nps: f64, threads: usize) -> f64 {
    if nps <= 0.0 {
        return f64::INFINITY;
    }
    let nodes = SOLVE_NODES_A * (SOLVE_NODES_B * empties as f64).exp();
    nodes * parallel_overhead(threads, empties) / (nps * DEEP_NPS_RATIO)
}

/// The same 40 solves: 90th percentile 3.4x `solve_secs`, slowest 4.7x.
const SOLVE_SLOW: f64 = 3.4;

/// What a solve must be allowed so that it is rarely cut.
fn solve_slow_secs(empties: u8, nps: f64, threads: usize) -> f64 {
    solve_secs(empties, nps, threads) * SOLVE_SLOW
}

pub fn solve_entry(budget_secs: f64, nps: f64, threads: usize, max: u8) -> u8 {
    (0..=max)
        .rev()
        .find(|&e| solve_slow_secs(e, nps, threads) <= budget_secs)
        .unwrap_or(0)
}

const SOLVE_CEILING: u8 = 32;

const DEPTH_BY_CLOCK: u32 = 60;

const SOLVE_REF: u8 = 18;

/// The earliest solve the clock affords when it costs no more than one of the midgame moves
/// still ahead; a fixed share of the clock shrank the solve's reserve as the clock ran down.
fn solve_ref_and_moves(
    avail: u64,
    empties: u8,
    nps: Option<f64>,
    threads: usize,
) -> (u8, u64, f64) {
    let moves_above = |e: u8| (empties.saturating_sub(e) as f64 / 2.0).ceil() as u64;
    let endgame = |e: u8, nps: f64| solve_slow_secs(e, nps, threads);
    let Some(nps) = nps else {
        return (SOLVE_REF, moves_above(SOLVE_REF).max(1), 0.0);
    };
    let e = (SOLVE_REF..=SOLVE_CEILING)
        .rev()
        .find(|&e| endgame(e, nps) <= avail as f64 / (moves_above(e) + 1) as f64)
        .unwrap_or(SOLVE_REF);
    (e, moves_above(e).max(1), endgame(e, nps))
}

const SOLVE_GREED: f64 = 10.0;

const SOLVE_MAX_SHARE: f64 = 0.5;

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

const OVERTIME_RESERVE: u64 = 5;

const OVERTIME_MAX_SECS: f64 = 1.5;

const OVERTIME_SOLVE: u8 = 12;

const NO_GRACE_RESERVE_MUL: u64 = 2;

/// A 900 s clock once ran to 901 s and lost a 16-disc win; from ~24 empties a solve takes 0.3 s.
const ENDGAME_FLOOR: u64 = 10;

/// Enough for a solve (0.3 s measured).
const FLOOR_MOVE_SECS: f64 = 0.25;

/// At 3.0, 33 of 45 extended moves ran to the limit while games ended with 1-2 minutes unspent (live, 5 minutes).
const EXTEND_MAX: f64 = 5.0;

const EXTEND_POOL_SHARE: f64 = 0.25;

/// Every move ran its full even share (58 of 63 cut at the deadline), leaving nothing to extend with;
/// at 0.7 72% of midgame moves still took 25-45 s.
const MOVE_BASE: f64 = 0.5;

pub fn plan(s: Situation, base: Levels, pace: Pace) -> Plan {
    if pace == Pace::Depth {
        return Plan {
            depth: base.depth,
            solve: base.solve,
            band: base.band,
            cap: None,
            extend: None,
        };
    }
    let Some(secs) = s.clock_secs else {
        return Plan {
            depth: base.depth,
            solve: base.solve,
            band: base.band,
            cap: None,
            extend: None,
        };
    };
    if s.in_overtime {
        let moves = ((s.empties as f64 / 2.0).ceil() as u64).max(1);
        let pool = secs.saturating_sub(OVERTIME_RESERVE) as f64;
        let per = (pool / moves as f64).min(OVERTIME_MAX_SECS);
        return Plan {
            depth: DEPTH_BY_CLOCK,
            solve: base.solve.min(OVERTIME_SOLVE),
            band: 0,
            cap: Some(Duration::from_secs_f64(per.max(0.05))),
            extend: None,
        };
    }
    let avail = secs;
    let (solve_ref, my_moves, endgame) = solve_ref_and_moves(avail, s.empties, s.nps, s.threads);
    let want = if s.grace_secs == 0 {
        s.reserve_secs * NO_GRACE_RESERVE_MUL
    } else {
        s.reserve_secs
    };
    // Without the solve's own time held back, the auto reference let the opening take 140 of 300 s.
    let reserve = want.min(avail / 2).max(ENDGAME_FLOOR.min(avail));
    // Held at a full midgame share: holding only the estimate let each move overspend, the reference slid
    // from 31 to 28 and the selective moves were left 2-10 s (live, 5 minutes).
    let reserve = if s.empties > solve_ref {
        let share = avail as f64 / (my_moves + 1) as f64;
        reserve.max((endgame.max(share).ceil() as u64).min(avail * 9 / 10))
    } else {
        reserve
    };
    let pool = avail.saturating_sub(reserve) as f64;
    let solve_due = s.empties <= solve_ref;
    let even = pool / my_moves as f64;
    let root = (my_moves as f64).sqrt();
    let budget = match pace {
        Pace::Fast => even * (0.6 + 0.4 / root),
        Pace::Tail(a) => even * (a + (1.0 - a) / root),
        _ => even,
    };
    let budget = budget * effective_budget_use(s.budget_use);
    let budget = budget.min(pool).min(even * MOVE_BASE);
    let budget = if s.max_move_secs > 0 {
        budget.min(s.max_move_secs as f64)
    } else {
        budget
    };
    // A solve ends the game, so price it against the clock, not the empty pool.
    let last = pool <= 0.0;
    let solve = match s.nps {
        Some(nps) => {
            let b = if solve_due {
                avail as f64 * SOLVE_MAX_SHARE
            } else {
                (budget * SOLVE_GREED).min(avail as f64 * SOLVE_MAX_SHARE)
            };
            let from_share = solve_entry(b, nps, s.threads, SOLVE_CEILING);
            if last {
                from_share.max(solvable_now(s, nps))
            } else {
                from_share
            }
        }
        None if avail < 20 => base.solve.min(14),
        None if avail < 60 => base.solve.min(20),
        None => base.solve,
    };
    let budget = if (last || solve_due) && solve >= s.empties {
        let need = s
            .nps
            .map(|nps| solve_slow_secs(s.empties, nps, s.threads))
            .unwrap_or(0.0);
        let share = if last {
            SOLVE_FINAL_SHARE
        } else {
            SOLVE_MAX_SHARE
        };
        budget.max(need.min(avail as f64 * share))
    } else {
        budget
    };
    // Only for a due solve the clock cannot afford: on 22 live losing selective moves a midgame search repeated 14.
    let band = match s.empties.checked_sub(solve) {
        Some(gap) if solve_due && gap > 0 && gap <= base.band => gap,
        _ => 0,
    };
    let cap = budget.max(if avail > 0 { FLOOR_MOVE_SECS } else { 0.05 });
    let extend = (!last).then(|| {
        (cap * EXTEND_MAX)
            .min(cap + (pool - cap).max(0.0) * EXTEND_POOL_SHARE)
            .max(cap)
    });
    Plan {
        depth: DEPTH_BY_CLOCK,
        solve,
        band,
        cap: Some(Duration::from_secs_f64(cap)),
        extend: extend.map(Duration::from_secs_f64),
    }
}

/// Nothing comes after a terminal solve.
const SOLVE_FINAL_SHARE: f64 = 0.9;

/// Via the share, the floor zeroed the pool and refused a 0.4 ms solve at 10 empties.
fn solvable_now(s: Situation, nps: f64) -> u8 {
    let avail = s.clock_secs.unwrap_or(0) as f64;
    if solve_slow_secs(s.empties, nps, s.threads) <= avail * SOLVE_FINAL_SHARE {
        s.empties
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Levels = Levels {
        depth: 22,
        solve: 26,
        band: 6,
    };

    fn cap_secs(secs: u64, empties: u8, pace: Pace) -> f64 {
        plan(
            Situation {
                clock_secs: Some(secs),
                grace_secs: 120,
                empties,
                // Below MOVE_BASE, so the pace still shapes the budget.
                budget_use: 0.5,
                ..Situation::default()
            },
            BASE,
            pace,
        )
        .cap
        .unwrap()
        .as_secs_f64()
    }

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

    #[test]
    fn the_default_is_thin_in_the_opening() {
        let even = cap_secs(600, 60, Pace::Tail(1.0));
        let fast = cap_secs(600, 60, Pace::Fast);
        assert!(
            fast < even,
            "default {fast} must be below equal split {even}"
        );
    }

    #[test]
    fn dropped_names_fall_back_to_the_default() {
        for s in ["slow", "even", "", "something"] {
            assert_eq!(Pace::parse(s), Pace::Fast, "{s:?}");
        }
        assert_eq!(Pace::parse("depth"), Pace::Depth);
        assert_eq!(Pace::parse("tail:0.4"), Pace::Tail(0.4));
    }

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

    #[test]
    fn the_floor_keeps_the_tail_unspent() {
        let at = |left| {
            plan(
                Situation {
                    clock_secs: Some(left),
                    grace_secs: 120,
                    empties: 26,
                    budget_use: 5.8,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
            .cap
            .unwrap()
            .as_secs_f64()
        };
        for left in [ENDGAME_FLOOR, 8, 5, 2, 1] {
            assert!(
                at(left) <= 0.3,
                "{left}s left must buy a solve, not a search: {}",
                at(left)
            );
        }
        assert!(
            at(ENDGAME_FLOOR + 4) <= 4.0,
            "above the floor only the pool is spendable: {}",
            at(ENDGAME_FLOOR + 4)
        );
    }

    #[test]
    fn the_floor_still_buys_the_solve_it_reserved_for() {
        let at = |empties, left| {
            plan(
                Situation {
                    clock_secs: Some(left),
                    grace_secs: 120,
                    empties,
                    budget_use: 6.0,
                    nps: Some(30e6),
                    threads: 8,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
        };
        for (empties, left) in [(10u8, 7u64), (10, 1), (18, 5), (24, 10)] {
            let p = at(empties, left);
            assert!(
                p.solve >= empties,
                "{empties} empties on {left}s must be solved ({:.4}s of work), got solve {}",
                solve_secs(empties, 30e6, 8),
                p.solve
            );
            assert!(
                p.cap.unwrap().as_secs_f64() >= solve_secs(empties, 30e6, 8) - 1e-6,
                "the deadline must let the solve finish: cap {:?}",
                p.cap.unwrap()
            );
        }
        // Out of reach stays out of reach: no pretending the clock can solve it.
        assert!(at(34, 54).solve < 34);
    }

    #[test]
    fn an_extension_stays_inside_what_the_clock_can_give() {
        let at = |empties, left| {
            plan(
                Situation {
                    clock_secs: Some(left),
                    grace_secs: 120,
                    empties,
                    budget_use: 6.0,
                    nps: Some(30e6),
                    threads: 8,
                    ..Situation::default()
                },
                BASE,
                Pace::Fast,
            )
        };
        for (empties, left) in [(46u8, 900u64), (40, 600), (36, 200), (32, 60)] {
            let p = at(empties, left);
            let (cap, ext) = (p.cap.unwrap(), p.extend.unwrap());
            assert!(
                ext >= cap,
                "{empties}/{left}s: extension {ext:?} below cap {cap:?}"
            );
            assert!(
                ext.as_secs_f64() <= cap.as_secs_f64() * EXTEND_MAX + 1e-9,
                "{empties}/{left}s: extension {ext:?} past {EXTEND_MAX}x cap {cap:?}"
            );
            assert!(
                ext.as_secs_f64() < left as f64,
                "{empties}/{left}s: {ext:?} spends the clock"
            );
        }
        assert!(at(10, 7).extend.is_none(), "the last move before the floor");
    }

    #[test]
    fn budget_never_reaches_zero() {
        for secs in [1, 2, 5, 10] {
            assert!(cap_secs(secs, 60, Pace::Fast) >= 0.05);
        }
    }

    #[test]
    fn tail_is_continuous_with_the_default() {
        for empties in [60u8, 44] {
            let f = cap_secs(600, empties, Pace::Fast);
            assert!((cap_secs(600, empties, Pace::Tail(0.6)) - f).abs() < 1e-9);
            assert!(cap_secs(600, empties, Pace::Tail(1.0)) > f);
        }
        let last_move = SOLVE_REF + 2;
        let late = cap_secs(600, last_move, Pace::Fast);
        assert!((cap_secs(600, last_move, Pace::Tail(1.0)) - late).abs() < 1e-9);
    }

    fn reference(clock: u64, nps: Option<f64>, threads: usize) -> u8 {
        solve_ref_and_moves(clock, 44, nps, threads).0
    }

    #[test]
    fn the_reference_follows_the_machine() {
        let live = reference(900, Some(130e6), 8);
        let bench = reference(60, Some(13.8e6), 1);
        assert!(live > bench, "live {live} vs bench {bench}");
        assert!(live <= SOLVE_CEILING && bench >= SOLVE_REF);
    }

    #[test]
    fn without_calibration_the_reference_is_the_old_default() {
        assert_eq!(reference(900, None, 8), SOLVE_REF);
    }

    #[test]
    fn the_reference_grows_with_the_clock() {
        assert!(reference(60, Some(130e6), 8) <= reference(1800, Some(130e6), 8));
        assert!(reference(36_000, Some(130e6), 8) <= SOLVE_CEILING);
    }

    #[test]
    fn every_clock_keeps_the_solve_its_time() {
        let (nps, threads) = (221e6, 8);
        for clock in [60u64, 120, 300, 600, 900, 1800] {
            let mut left = clock as f64;
            let mut first = true;
            for e in (2..=44u8).rev().step_by(2) {
                let p = plan(
                    Situation {
                        clock_secs: Some(left as u64),
                        grace_secs: 120,
                        empties: e,
                        budget_use: 6.0,
                        nps: Some(nps),
                        threads,
                        ..Situation::default()
                    },
                    Levels {
                        depth: 60,
                        solve: 26,
                        band: 6,
                    },
                    Pace::Fast,
                );
                let cap = p.cap.unwrap().as_secs_f64();
                let used = if e <= p.solve && first {
                    first = false;
                    let need = solve_secs(e, nps, threads);
                    assert!(
                        cap >= need - 1e-6,
                        "{clock}s: the first solve at {e} got {cap:.1}s of {need:.1}s"
                    );
                    need / 2.0
                } else if e <= p.solve {
                    1.0
                } else {
                    cap
                };
                left -= used.min(cap);
                assert!(left >= 0.0, "{clock}s ran out at {e} empties");
            }
            assert!(!first, "{clock}s never solved");
        }
    }

    #[test]
    fn smaller_tail_is_thinner_in_the_opening() {
        let a = cap_secs(600, 60, Pace::Tail(0.6));
        let b = cap_secs(600, 60, Pace::Tail(0.25));
        assert!(b < a, "0.25 {b} < 0.6 {a}");
    }

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
        assert!(
            at(600) > BASE.solve,
            "600s {} > configured {} (calibration decides)",
            at(600),
            BASE.solve
        );
        assert!(at(600) <= SOLVE_CEILING, "never past the physical wall");
    }

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

        let untimed = plan(Situation::default(), BASE, Pace::Fast);
        assert_eq!(untimed.depth, BASE.depth);
        assert!(untimed.cap.is_none());

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
        assert_ne!(with(0.3), good, "0.3 must differ from the default");
        assert_eq!(with(-1.0), good, "negatives must fall to the default");
        assert_eq!(with(f64::NAN), good, "NaN must fall to the default");
        assert_eq!(
            with(f64::INFINITY),
            good,
            "infinity must fall to the default"
        );
    }

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
        assert!(with(0.2) < with(0.4));
        assert!(with(0.4) < with(0.6));
    }

    #[test]
    fn overtime_finishes_the_game_without_a_wipeout() {
        for grace in [120u64, 60, 30] {
            let mut left = grace as f64;
            for e in (0..=60u8).rev() {
                let used = ot_plan(left.max(0.0) as u64, e).cap.unwrap().as_secs_f64();
                left -= used;
                assert!(left > 0.0, "grace {grace}s exhausted at {e} empties");
            }
        }
    }

    #[test]
    fn overtime_shrinks_as_the_grace_runs_down() {
        let much = ot_plan(120, 40).cap.unwrap();
        let little = ot_plan(8, 40).cap.unwrap();
        assert!(little < much);
    }

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
    };

    fn play_out(use_ratio: f64) -> (f64, bool) {
        let mut left = 900.0_f64;
        let mut empties = 48u8;
        let mut spent = 0.0;
        let mut ran_out = false;
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

    #[test]
    fn even_full_use_does_not_run_out() {
        let (_, out) = play_out(1.0);
        assert!(!out, "using full deadlines flags");
    }
}
