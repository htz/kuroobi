//! Measure this machine's solve speed and write it to `resources.conf`.
//!
//! Deriving the solve entry from the clock ([`kuroobi::timectl`]) needs
//! a nodes-to-seconds factor; of the three layers only nps is
//! machine-dependent, so it is measured here. The GUI settings can do
//! the same; this is for CLI-only use and re-measurement checks.
//!
//! The benefit shows as avoided breakdowns, not strength: 1400
//! self-play games moved no win rate but raised the worst-case leftover
//! clock from 5.0s to 8.9s in 30-second games.
//!
//! ```sh
//! calibnps                      # measure at the configured threads, save
//! calibnps --threads 8          # measure at a given thread count
//! calibnps --show               # show saved values and derived entries
//! calibnps --no-save            # measure only (no write)
//! calibnps --depths             # also measure budget-reachable depth (minutes)
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::resources::Resources;

/// Config path — the same one the GUI uses, so either side's
/// calibration serves both.
fn resources_path() -> PathBuf {
    if let Ok(d) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(d).join("kuroobi").join("resources.conf");
    }
    let home = std::env::var("HOME").unwrap_or_default();
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(home).join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let base = PathBuf::from(home).join(".config");
    base.join("kuroobi").join("resources.conf")
}

/// Default thread count (half the cores, same as the GUI's auto).
fn auto_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(1)
}

/// Move budget and solve entry for a given clock and empties.
fn at(nps: f64, threads: usize, secs: u64, empties: u8, solve_cap: u8) -> (f64, u8) {
    let p = kuroobi::timectl::plan(
        kuroobi::timectl::Situation {
            clock_secs: Some(secs),
            empties,
            nps: Some(nps),
            threads,
            ..Default::default()
        },
        kuroobi::timectl::Levels {
            depth: 22,
            solve: solve_cap,
            band: 6,
            auto_band: true,
        },
        kuroobi::timectl::Pace::Fast,
    );
    (p.cap.map_or(0.0, |c| c.as_secs_f64()), p.solve)
}

/// Print per-clock allocations from the calibration — what it changes
/// reads better than one number. Two solve caps are shown so a limiting
/// strength setting is visible.
fn report(nps: f64, threads: usize) {
    println!("読切 {:.1}M ノード/秒 ({threads} スレッド)", nps / 1e6);
    println!();
    println!("  持ち時間 |  1 手の予算 (空き 60 / 44 / 30) | 読切の入り口 (上限 26 / 30)");
    println!("  ---------+-------------------------------+---------------------------");
    for secs in [300u64, 600, 900, 1200, 1800] {
        let (b60, s26) = at(nps, threads, secs, 60, 26);
        let (b44, _) = at(nps, threads, secs, 44, 26);
        let (b30, _) = at(nps, threads, secs, 30, 26);
        let (_, s30) = at(nps, threads, secs, 60, 30);
        let t = kuroobi::timectl::solve_secs(s30, nps, threads);
        println!(
            "  {:>4} 分 | {b60:>7.1}s {b44:>8.1}s {b30:>8.1}s | {s26:>7} {s30:>10}  (空き {s30} は {t:.0} 秒の見込み)",
            secs / 60
        );
    }
}

/// Measure how deep a move budget actually reaches in the midgame.
///
/// The configured depth is only a cap; time decides the reach. Too low
/// wastes budget, too high drains it (depth 34 once cut the leftover to
/// 0.5s at 47% win rate) — the balance point must be measured. Positions
/// come from random openings across several empties.
fn depth_table(engine: &mut Engine, threads_list: &[usize]) {
    use kuroobi::{Board, Position};
    // Deterministic PRNG (identical openings every run).
    let mut st = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        st ^= st >> 12;
        st ^= st << 25;
        st ^= st >> 27;
        st.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let opening = |plies: usize, next: &mut dyn FnMut() -> u64| {
        let mut b = Board::new();
        for _ in 0..plies {
            let m = b.movable();
            if m == 0 {
                b.pass();
                continue;
            }
            let pick = (next() % m.count_ones() as u64) as u32;
            let mut x = m;
            for _ in 0..pick {
                x &= x - 1;
            }
            b.make_move_unchecked(Position::from_index(x.trailing_zeros()).unwrap());
        }
        b
    };
    let keep = engine.config().threads;
    println!();
    println!("1 手の予算で届く中盤の深さとノード数 (局面 3 つの中央値)");
    println!("  **同じ局面を同じ予算で、スレッド数だけ変えて測る。** 並列で深さが");
    println!("  伸びないのに ノードが増えていれば、木が太っただけ (重複)。");
    println!();
    print!("  空き  予算 |");
    for t in threads_list {
        print!(" {t:>3}T 深さ  ノード");
    }
    println!();
    println!("  -----------+{}", "-".repeat(threads_list.len() * 17));
    for plies in [10usize, 16, 24, 30] {
        let boards: Vec<Board> = (0..3).map(|_| opening(plies, &mut next)).collect();
        for b in [2u64, 10, 40] {
            print!("  {:>4} {b:>4}s |", boards[0].empty_count());
            for &t in threads_list {
                engine.set_threads(t);
                let mut rows: Vec<(u32, u64)> = boards
                    .iter()
                    .map(|bd| {
                        engine.clear_tables();
                        let n0 = engine.nodes();
                        let t0 = std::time::Instant::now();
                        let d = engine
                            .choose_within(bd, Some(t0 + std::time::Duration::from_secs(b)))
                            .depth;
                        (d, engine.nodes() - n0)
                    })
                    .collect();
                rows.sort_unstable();
                print!(" {:>7} {:>8.0}M", rows[1].0, rows[1].1 as f64 / 1e6);
            }
            println!();
        }
    }
    engine.set_threads(keep);
}

fn main() -> ExitCode {
    let mut threads: Option<usize> = None;
    let mut save = true;
    let mut show = false;
    let mut depths = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()),
            "--no-save" => save = false,
            "--show" => show = true,
            "--depths" => depths = true,
            other => {
                eprintln!("unknown option {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let path = resources_path();
    let mut res = Resources::load(&path);
    let threads = threads.or(res.threads).unwrap_or_else(auto_threads);

    if show {
        if res.nps.is_empty() {
            println!("まだ較正していない ({} に nps が無い)", path.display());
            return ExitCode::SUCCESS;
        }
        println!("{}", path.display());
        for (t, nps) in &res.nps {
            report(*nps, *t);
            println!();
        }
        if res.nps_for(threads).is_none() {
            println!("(いま設定されているのは {threads} スレッド — その数では測っていない)");
        }
        return ExitCode::SUCCESS;
    }

    let cfg = EngineConfig {
        weights: res.weights_path(),
        nnue: res.nnue_path(),
        book: res.book_path(),
        threads,
        // Measure with the configured tables; defaults would record a
        // value for the wrong table size (22 -> 24 is 8.9% faster).
        midgame_hash_bits: res.hash_mid_bits(),
        solver_hash_bits: res.hash_end_bits(),
        ..Default::default()
    };
    let mut engine = match Engine::new(cfg) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("エンジンを用意できない: {e}");
            return ExitCode::FAILURE;
        }
    };
    let nps = engine.measure_solve_nps();
    if nps <= 0.0 {
        eprintln!("測れなかった (較正局面が解けていない)");
        return ExitCode::FAILURE;
    }
    report(nps, threads);
    if depths {
        engine.set_levels(60, 0, 0); // uncap depth; let time decide
        engine.set_use_book(false);
        depth_table(&mut engine, &[1, 4, 8]);
    }

    if save {
        res.set_nps(threads, nps);
        if let Err(e) = res.save(&path) {
            eprintln!("保存できない {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("{} に保存した", path.display());
    }
    ExitCode::SUCCESS
}
