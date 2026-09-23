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
    pub auto_band: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub depth: u32,
    pub solve: u8,
    pub band: u8,
    pub cap: Option<Duration>,
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
    pub solve_ref: SolveRef,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SolveRef {
    Fixed(u8),
    Auto,
}

impl SolveRef {
    pub fn parse(s: &str) -> SolveRef {
        if s == "auto" {
            return SolveRef::Auto;
        }
        s.parse()
            .map(SolveRef::Fixed)
            .unwrap_or(SolveRef::Fixed(SOLVE_REF))
    }

    fn value(self, clock_secs: u64, nps: Option<f64>, threads: usize) -> u8 {
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

const SOLVE_SAFETY: f64 = 3.0;

const SOLVE_TOTAL_FACTOR: f64 = 4.0 / 3.0;

pub fn solve_secs(empties: u8, nps: f64, threads: usize) -> f64 {
    if nps <= 0.0 {
        return f64::INFINITY;
    }
    let nodes = SOLVE_NODES_A * (SOLVE_NODES_B * empties as f64).exp();
    nodes * parallel_overhead(threads, empties) / (nps * DEEP_NPS_RATIO)
        * SOLVE_SAFETY
        * SOLVE_TOTAL_FACTOR
}

pub fn solve_entry(budget_secs: f64, nps: f64, threads: usize, max: u8) -> u8 {
    (0..=max)
        .rev()
        .find(|&e| solve_secs(e, nps, threads) <= budget_secs)
        .unwrap_or(0)
}

const SOLVE_CEILING: u8 = 32;

const DEPTH_BY_CLOCK: u32 = 60;

const SOLVE_REF: u8 = 18;

pub fn auto_solve_ref(clock_secs: u64, nps: Option<f64>, threads: usize) -> u8 {
    let Some(nps) = nps else {
        return SOLVE_REF;
    };
    let budget = clock_secs as f64 * SOLVE_REF_SHARE;
    solve_entry(budget, nps, threads, SOLVE_REF_MAX).max(SOLVE_REF)
}

const SOLVE_REF_SHARE: f64 = 0.05;

const SOLVE_REF_MAX: u8 = 30;

fn band_for(budget: f64) -> u8 {
    if budget < 12.0 {
        0
    } else if budget < 60.0 {
        6
    } else {
        8
    }
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

pub fn plan(s: Situation, base: Levels, pace: Pace) -> Plan {
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
    if s.in_overtime {
        let moves = ((s.empties as f64 / 2.0).ceil() as u64).max(1);
        let pool = secs.saturating_sub(OVERTIME_RESERVE) as f64;
        let per = (pool / moves as f64).min(OVERTIME_MAX_SECS);
        return Plan {
            depth: DEPTH_BY_CLOCK,
            solve: base.solve.min(OVERTIME_SOLVE),
            band: 0,
            cap: Some(Duration::from_secs_f64(per.max(0.05))),
        };
    }
    let avail = secs;
    let solve_ref = s.solve_ref.value(avail, s.nps, s.threads);
    let my_moves = ((s.empties.saturating_sub(solve_ref) as f64 / 2.0).ceil() as u64).max(1);
    let want = if s.grace_secs == 0 {
        s.reserve_secs * NO_GRACE_RESERVE_MUL
    } else {
        s.reserve_secs
    };
    let reserve = want.min(avail / 2);
    let pool = avail.saturating_sub(reserve) as f64;
    let even = pool / my_moves as f64;
    let root = (my_moves as f64).sqrt();
    let budget = match pace {
        Pace::Fast => even * (0.6 + 0.4 / root),
        Pace::Tail(a) => even * (a + (1.0 - a) / root),
        _ => even,
    };
    let budget = budget * effective_budget_use(s.budget_use);
    let budget = budget.min(pool);
    let budget = if s.max_move_secs > 0 {
        budget.min(s.max_move_secs as f64)
    } else {
        budget
    };
    let solve = match s.nps {
        Some(nps) => {
            let b = (budget * SOLVE_GREED).min(avail as f64 * SOLVE_MAX_SHARE);
            solve_entry(b, nps, s.threads, SOLVE_CEILING)
        }
        None if avail < 20 => base.solve.min(14),
        None if avail < 60 => base.solve.min(20),
        None => base.solve,
    };
    let band = if base.auto_band {
        band_for(budget)
    } else {
        if budget >= 12.0 {
            base.band
        } else {
            0
        }
    };
    Plan {
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
        let two_left = SOLVE_REF + 4;
        let late = cap_secs(600, two_left, Pace::Fast);
        assert!((cap_secs(600, two_left, Pace::Tail(1.0)) - late).abs() < 1e-9);
    }

    #[test]
    fn the_reference_follows_the_machine() {
        let live = auto_solve_ref(900, Some(130e6), 8);
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

    #[test]
    fn the_reference_never_goes_below_the_old_default() {
        for (clock, nps, threads) in [(3u64, 1e6, 1), (10, 5e5, 1), (60, 1e5, 2)] {
            assert_eq!(auto_solve_ref(clock, Some(nps), threads), SOLVE_REF);
        }
    }

    #[test]
    fn without_calibration_the_reference_is_the_old_default() {
        assert_eq!(auto_solve_ref(900, None, 8), SOLVE_REF);
    }

    #[test]
    fn the_reference_is_capped() {
        assert!(auto_solve_ref(60, Some(130e6), 8) <= auto_solve_ref(1800, Some(130e6), 8));
        assert!(auto_solve_ref(36_000, Some(130e6), 8) <= SOLVE_REF_MAX);
    }

    #[test]
    fn smaller_tail_is_thinner_in_the_opening() {
        let a = cap_secs(600, 60, Pace::Tail(0.6));
        let b = cap_secs(600, 60, Pace::Tail(0.25));
        assert!(b < a, "0.25 {b} < 0.6 {a}");
    }

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
        assert_ne!(with(1.0), good, "1.0 must differ from the default");
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
        assert!(with(1.0) < with(2.0));
        assert!(with(2.0) < with(3.0));
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
        auto_band: false,
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
