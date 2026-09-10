//! Measures the ProbCut sigma of one NNUE model and writes it into that
//! model's own weights file.
//!
//! Sigma is the standard deviation of `search(depth) - search(pc_depth)`,
//! and it belongs to the evaluator: a model that evaluates differently
//! misses by a different amount. From 2026-07-20 to 2026-09-10 the NNUE
//! searcher pruned against constants fitted for the *linear* evaluator, on
//! a "safer anyway" argument, and the endgame sigma had made the same
//! argument and turned out 2x too large.
//!
//! Measured, the borrowed constants were 1.15-1.28x wide -- conservative,
//! as advertised -- and 300 games at 200 ms/move could not separate them
//! from the measured ones (50.5%, 95% CI 44.8..56.2). So the value here is
//! not a recovered loss; it is that the next model's margins will be its
//! own, whether or not it happens to resemble this one.
//!
//! One run does the whole job: search, discard the terminal values, fit the
//! six coefficients, write them back into the file it loaded. There is no
//! way to write a sigma into a file other than the one it was measured
//! against, which is the only failure this tool could otherwise cause.
//!
//! Positions run in parallel, searches stay sequential (Lazy SMP changes
//! the tree; we measure values, not speed).
//!
//! `--patterns` selects the NNUE's pattern set (default `nnue`, the
//! 297,432-row one). Without `--write` nothing is written and the fit is
//! only reported.
//!
//! Usage:
//!   mpccalib_nnue [--threads N] [--stride N] [--max N] [--max-depth 12]
//!                 [--depths a,b,c] [--patterns nnue|egaroucid|compact]
//!                 [--min-empties 20] [--max-empties 58] [--min-cell 8] [--csv out.csv] [--write]
//!                 <nnue.bin> <data-file>...

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use kuroobi::midgame::{mpc_reduced_depth, NnueSearch, SharedTt};
use kuroobi::nnue::{MpcSigma, Nnue};
use kuroobi::pattern::{COMPACT_PATTERNS, EGAROUCID_PATTERNS, NNUE_PATTERNS};
use kuroobi::trainer::load_examples_binary;

/// Values above this are the solver's terminal encoding, not a disc
/// difference. Leaving them in is not a small error: they inflate the fitted
/// sigma by three orders of magnitude.
const DISC_LIMIT: f32 = 64.0;

/// The coefficients the search used before any model carried its own. Kept
/// as the fit's starting point -- the shape is right and only the scale is
/// another evaluator's -- and as the ratio the report prints against.
const LEGACY: [f32; MpcSigma::LEN] = [-0.068941, 0.368775, -0.713476, 0.010223, 0.647219, 4.050545];

/// One (empties, depth, pc_depth) cell of the measurement.
struct Cell {
    /// Mean empty count of the samples in this cell. Empties are binned
    /// (`--empties-bin`) because a cell keyed on the exact count holds a
    /// handful of samples and its RMS is mostly noise; the surface is
    /// smooth in empties, so the mean of a narrow bin costs nothing.
    empties: f64,
    e_lo: u32,
    e_hi: u32,
    depth: u32,
    pc_depth: u32,
    n: usize,
    /// RMS of `search(depth) - search(pc_depth)` about zero, not about its
    /// own mean: the margin is applied symmetrically, so a systematic bias
    /// at this cell has to be paid for out of the same budget.
    rms: f32,
}

/// Solve `a x = b` for a small dense system, in place. `None` if singular.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| {
            a[i][col]
                .abs()
                .partial_cmp(&a[j][col].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in (col + 1)..n {
            let f = a[row][col] / a[col][col];
            let (above, from_row) = a.split_at_mut(row);
            for (x, y) in from_row[0].iter_mut().zip(above[col].iter()).skip(col) {
                *x -= f * y;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let mut s = b[row];
        for k in (row + 1)..n {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
    }
    Some(x)
}

/// Free parameters of the fit: `b, c, qa, qb, qc`, with `a` held at one.
///
/// The stored form `qa*s^2 + qb*s + qc`, `s = a*e + b*d + c*p`, has a
/// redundant scale: multiplying `s` by k and dividing `qa` by k^2 and `qb`
/// by k describes the identical surface. Left free, the fit drifts along
/// that direction and lands on whatever the damping happened to allow --
/// which is how a first attempt produced `qa = -0.0716` for a surface that
/// curves upward. Pinning `a = 1` removes the redundancy and costs no
/// expressiveness, since `a` divides straight out into the other five.
const NP: usize = 5;

/// `qa*s^2 + qb*s + qc` with `s = e + b*d + c*p`, unfloored -- the fit works
/// on the raw surface; the floor is the search's safety net, not the model's
/// shape. Returns the value and the gradient in the free parameters.
fn model(t: &[f64; NP], e: f64, d: f64, p: f64) -> (f64, [f64; NP]) {
    let s = e + t[0] * d + t[1] * p;
    let f = t[2] * s * s + t[3] * s + t[4];
    let ds = 2.0 * t[2] * s + t[3];
    (f, [ds * d, ds * p, s * s, s, 1.0])
}

/// The stored six from the fitted five.
fn to_coefficients(t: &[f64; NP]) -> [f32; MpcSigma::LEN] {
    [
        1.0,
        t[0] as f32,
        t[1] as f32,
        t[2] as f32,
        t[3] as f32,
        t[4] as f32,
    ]
}

/// The fitted five from a stored six, for use as a starting point.
fn to_free(v: [f32; MpcSigma::LEN]) -> [f64; NP] {
    let a = v[0] as f64;
    [
        v[1] as f64 / a,
        v[2] as f64 / a,
        v[3] as f64 * a * a,
        v[4] as f64 * a,
        v[5] as f64,
    ]
}

/// Levenberg-Marquardt on the six coefficients, weighted by `sqrt(n)`: the
/// uncertainty of an RMS estimate falls as `1/sqrt(n)`, so a cell built from
/// four samples should not outvote one built from four hundred.
fn fit(cells: &[Cell], start: [f32; MpcSigma::LEN]) -> ([f32; MpcSigma::LEN], f64) {
    let mut t = to_free(start);
    let cost = |t: &[f64; NP]| -> f64 {
        cells
            .iter()
            .map(|c| {
                let w = (c.n as f64).sqrt();
                let (f, _) = model(t, c.empties, c.depth as f64, c.pc_depth as f64);
                let r = w * (f - c.rms as f64);
                r * r
            })
            .sum()
    };
    let mut best = cost(&t);
    let mut lambda = 1e-3f64;
    for _ in 0..500 {
        let mut ata = vec![vec![0.0f64; NP]; NP];
        let mut atb = vec![0.0f64; NP];
        for c in cells {
            let w = (c.n as f64).sqrt();
            let (f, g) = model(&t, c.empties, c.depth as f64, c.pc_depth as f64);
            let r = w * (c.rms as f64 - f);
            for i in 0..NP {
                atb[i] += w * g[i] * r;
                for j in 0..NP {
                    ata[i][j] += w * g[i] * w * g[j];
                }
            }
        }
        let mut improved = false;
        for _ in 0..12 {
            let mut damped = ata.clone();
            for (i, row) in damped.iter_mut().enumerate() {
                row[i] += lambda * (1.0 + ata[i][i]);
            }
            let Some(step) = solve(damped, atb.clone()) else {
                lambda *= 10.0;
                continue;
            };
            let mut cand = t;
            for i in 0..NP {
                cand[i] += step[i];
            }
            let c = cost(&cand);
            if c.is_finite() && c < best {
                t = cand;
                best = c;
                lambda = (lambda * 0.3).max(1e-12);
                improved = true;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    let wsum: f64 = cells.iter().map(|c| c.n as f64).sum();
    (to_coefficients(&t), (best / wsum.max(1.0)).sqrt())
}

fn main() -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut stride = 997usize; // prime stride decorrelates from file order
    let mut max_positions = 2000usize;
    let mut threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let mut max_depth = 12u32;
    let mut depths: Vec<u32> = Vec::new();
    let mut min_empties = 20u32;
    // The midgame search runs from the opening down to the solve threshold,
    // so the fit has to cover that whole span. A cap at 45 left the opening
    // out and let the quadratic extrapolate there unchecked.
    let mut max_empties = 58u32;
    let mut min_cell = 20usize;
    let mut empties_bin = 3u32;
    let mut csv: Option<PathBuf> = None;
    // Re-fit an earlier run's raw values instead of searching again. The
    // measurement is the expensive half and the fit is the half worth
    // iterating on, so they are separable -- but only in this direction:
    // there is still no way to write a sigma into a file other than the
    // one named on the command line.
    let mut from_csv: Option<PathBuf> = None;
    let mut show_cells = false;
    let mut write = false;
    // Stamp the pre-2026-09-10 constants into a file without measuring
    // anything. Asking "was the borrowed sigma worse?" means playing the
    // two against each other, and that needs an opponent built the old
    // way. (Asked on 2026-09-10: 50.5% over 300 games, no difference.)
    let mut legacy_sigma = false;
    // The tool predates the 297,432-row set; a weight file belongs to the
    // pattern set it was trained on, so the set has to be selectable.
    let mut which = String::from("nnue");

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--patterns" => which = it.next().unwrap_or(which),
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(threads),
            "--stride" => stride = it.next().and_then(|v| v.parse().ok()).unwrap_or(stride),
            "--max" => {
                max_positions = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(max_positions)
            }
            "--max-depth" => {
                max_depth = it.next().and_then(|v| v.parse().ok()).unwrap_or(max_depth)
            }
            "--min-empties" => {
                min_empties = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(min_empties)
            }
            "--max-empties" => {
                max_empties = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(max_empties)
            }
            "--min-cell" => min_cell = it.next().and_then(|v| v.parse().ok()).unwrap_or(min_cell),
            "--empties-bin" => {
                empties_bin = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n| n >= 1)
                    .unwrap_or(empties_bin)
            }
            "--csv" => csv = it.next().map(PathBuf::from),
            "--from-csv" => from_csv = it.next().map(PathBuf::from),
            "--cells" => show_cells = true,
            "--write" => write = true,
            "--legacy-sigma" => legacy_sigma = true,
            "--depths" => {
                if let Some(v) = it.next() {
                    depths = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                }
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    // A data file is only needed when something is actually searched;
    // `--legacy-sigma` and `--from-csv` both skip the search.
    let want_data = !legacy_sigma && from_csv.is_none();
    if paths.is_empty() || (want_data && paths.len() < 2) {
        eprintln!(
            "usage: mpccalib_nnue [--threads N] [--stride N] [--max N] [--max-depth 12] \
             [--depths a,b,c] [--patterns nnue|egaroucid|compact] [--min-empties 20] [--max-empties 58] \
             [--min-cell 20] [--empties-bin 3] [--csv out.csv] [--cells] [--write] \
             <nnue.bin> <data-file>..."
        );
        return ExitCode::FAILURE;
    }
    let nnue_path = paths.remove(0);

    if legacy_sigma {
        let patterns = match which.as_str() {
            "compact" => COMPACT_PATTERNS,
            "nnue" => NNUE_PATTERNS,
            "egaroucid" => EGAROUCID_PATTERNS,
            other => {
                eprintln!("unknown pattern set {other}");
                return ExitCode::FAILURE;
            }
        };
        let mut nn = Nnue::new(patterns);
        if let Err(err) = nn.load(&nnue_path) {
            eprintln!("failed to load {}: {err}", nnue_path.display());
            return ExitCode::FAILURE;
        }
        nn.set_mpc_sigma(Some(MpcSigma::from_array(LEGACY)));
        if let Err(err) = nn.save(&nnue_path) {
            eprintln!("failed to write {}: {err}", nnue_path.display());
            return ExitCode::FAILURE;
        }
        println!(
            "wrote the linear evaluator's constants into {} -- for A/B only, \
             this is not a measurement of this model",
            nnue_path.display()
        );
        return ExitCode::SUCCESS;
    }

    /* The pairs the search will actually use, and nothing else: fitting a
    surface over depths ProbCut never probes at spends the samples on cells
    that cannot affect a game. */
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for d in kuroobi::midgame::mpc_min_depth()..=max_depth {
        let p = mpc_reduced_depth(d);
        if p >= 1 && p < d {
            pairs.push((d, p));
        }
    }
    if depths.is_empty() {
        for &(d, p) in &pairs {
            depths.push(d);
            depths.push(p);
        }
        depths.sort_unstable();
        depths.dedup();
    }
    let pairs: Vec<(u32, u32)> = pairs
        .into_iter()
        .filter(|(d, p)| depths.contains(d) && depths.contains(p))
        .collect();
    if pairs.is_empty() {
        eprintln!("no (depth, pc_depth) pair is covered by --depths");
        return ExitCode::FAILURE;
    }

    let patterns = match which.as_str() {
        "compact" => COMPACT_PATTERNS,
        "nnue" => NNUE_PATTERNS,
        "egaroucid" => EGAROUCID_PATTERNS,
        other => {
            eprintln!("unknown pattern set {other}");
            return ExitCode::FAILURE;
        }
    };
    /* The whole measurement, or an earlier one read back. Searching is the
    expensive half; the fit is the half worth iterating on, so they are
    separable. A re-fit never loads the model, which is a gigabyte. */
    let rows: Vec<(u32, Vec<f32>)> = if let Some(path) = &from_csv {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(err) => {
                eprintln!("failed to read {}: {err}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let mut it = text.lines();
        let Some(header) = it.next() else {
            eprintln!("{} is empty", path.display());
            return ExitCode::FAILURE;
        };
        // The header names the depths, so a re-fit uses the depths that
        // were actually searched rather than whatever the flags now say.
        depths = header
            .split(',')
            .skip(1)
            .filter_map(|c| c.trim().strip_prefix('d')?.parse().ok())
            .collect();
        let mut out = Vec::new();
        for line in it {
            let mut f = line.split(',');
            let Some(Ok(e)) = f.next().map(str::parse::<u32>) else {
                continue;
            };
            let vals: Vec<f32> = f.filter_map(|v| v.trim().parse().ok()).collect();
            if vals.len() == depths.len() {
                out.push((e, vals));
            }
        }
        eprintln!(
            "re-fitting {} rows from {} at depths {:?}",
            out.len(),
            path.display(),
            depths
        );
        out
    } else {
        let mut nn = Nnue::new(patterns);
        if let Err(err) = nn.load(&nnue_path) {
            eprintln!("failed to load {}: {err}", nnue_path.display());
            return ExitCode::FAILURE;
        }
        // Skipping this makes the SIMD path read uninitialized memory.
        nn.quantize();
        let nn = std::sync::Arc::new(nn);

        // Collect positions first for parallel dispatch.
        let mut boards = Vec::new();
        'outer: for path in &paths {
            let examples = match load_examples_binary(path) {
                Ok(ex) => ex,
                Err(err) => {
                    eprintln!("failed to load {}: {err}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            for ex in examples.iter().step_by(stride) {
                let board = ex.board();
                let empties = 64 - (board.black | board.white).count_ones();
                if !(min_empties..=max_empties).contains(&empties) || board.movable() == 0 {
                    continue;
                }
                boards.push(board);
                if boards.len() >= max_positions {
                    break 'outer;
                }
            }
        }
        eprintln!(
            "measuring {} positions at depths {:?} on {threads} threads",
            boards.len(),
            depths
        );

        let next = AtomicUsize::new(0);
        // One row per position: empties, then one value per measured depth.
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let (next, boards, depths) = (&next, &boards, &depths);
                    let nn = nn.clone();
                    s.spawn(move || {
                        /* Per-thread tables: sharing lets one position's
                        results help another and breaks independence. 18 bits
                        is ~4 MB per thread. */
                        let tt = std::sync::Arc::new(SharedTt::new(18));
                        let mut search = NnueSearch::new(nn, tt.clone());
                        search.threads = 1; // sequential search keeps the tree fixed
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(board) = boards.get(i) else { break };
                            let empties = 64 - (board.black | board.white).count_ones();
                            let vals = depths
                                .iter()
                                .map(|&d| {
                                    tt.clear();
                                    search.best_move_deadline(board, d, None).1
                                })
                                .collect();
                            out.push((empties, vals));
                        }
                        out
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        })
    };

    if let Some(path) = &csv {
        use std::io::Write;
        match std::fs::File::create(path) {
            Ok(f) => {
                let mut w = std::io::BufWriter::new(f);
                let _ = write!(w, "empties");
                for d in &depths {
                    let _ = write!(w, ",d{d}");
                }
                let _ = writeln!(w);
                for (e, v) in &rows {
                    let _ = write!(w, "{e}");
                    for x in v {
                        let _ = write!(w, ",{x:.3}");
                    }
                    let _ = writeln!(w);
                }
                eprintln!("raw values written to {}", path.display());
            }
            Err(err) => eprintln!("could not write {}: {err}", path.display()),
        }
    }

    /* Terminal values first. A position with fewer empties than the search
    depth is solved, and the solver's win/loss encoding is not a disc
    difference; a single one of those in a cell moves its RMS by hundreds. */
    let mut dropped = 0usize;
    let kept: Vec<&(u32, Vec<f32>)> = rows
        .iter()
        .filter(|(_, v)| {
            let ok = v.iter().all(|x| x.abs() <= DISC_LIMIT);
            if !ok {
                dropped += 1;
            }
            ok
        })
        .collect();
    eprintln!(
        "excluded {dropped} of {} rows holding a terminal value (|v| > {DISC_LIMIT:.0}), {} left",
        rows.len(),
        kept.len()
    );
    if kept.is_empty() {
        eprintln!("nothing left to fit");
        return ExitCode::FAILURE;
    }

    let idx = |d: u32| depths.iter().position(|&x| x == d).unwrap();
    struct Acc {
        n: usize,
        sq: f64,
        e_sum: f64,
        e_lo: u32,
        e_hi: u32,
    }
    let mut acc: std::collections::BTreeMap<(u32, u32, u32), Acc> = Default::default();
    for (e, v) in &kept {
        for &(d, p) in &pairs {
            let err = (v[idx(d)] - v[idx(p)]) as f64;
            let slot = acc.entry((*e / empties_bin, d, p)).or_insert(Acc {
                n: 0,
                sq: 0.0,
                e_sum: 0.0,
                e_lo: u32::MAX,
                e_hi: 0,
            });
            slot.n += 1;
            slot.sq += err * err;
            slot.e_sum += *e as f64;
            slot.e_lo = slot.e_lo.min(*e);
            slot.e_hi = slot.e_hi.max(*e);
        }
    }
    let cells: Vec<Cell> = acc
        .iter()
        .filter(|(_, a)| a.n >= min_cell)
        .map(|(&(_, depth, pc_depth), a)| Cell {
            empties: a.e_sum / a.n as f64,
            e_lo: a.e_lo,
            e_hi: a.e_hi,
            depth,
            pc_depth,
            n: a.n,
            rms: (a.sq / a.n as f64).sqrt() as f32,
        })
        .collect();
    eprintln!(
        "{} cells with at least {min_cell} samples, over {} (depth, pc_depth) pairs",
        cells.len(),
        pairs.len()
    );
    if cells.len() < 12 {
        eprintln!("too few cells to fit six coefficients; measure more positions");
        return ExitCode::FAILURE;
    }

    let (coef, rmse) = fit(&cells, LEGACY);
    let sigma = MpcSigma::from_array(coef);
    let legacy = MpcSigma::from_array(LEGACY);

    // Per-pair summary: what was measured, what the fit says, and how it
    // compares with what the search used to prune against.
    println!(
        "{:>5} {:>4} {:>8} {:>9} {:>8} {:>8} {:>7}",
        "depth", "pc", "n", "empties", "measured", "fitted", "vs old"
    );
    for &(d, p) in &pairs {
        let sel: Vec<&Cell> = cells
            .iter()
            .filter(|c| c.depth == d && c.pc_depth == p)
            .collect();
        if sel.is_empty() {
            continue;
        }
        let n: usize = sel.iter().map(|c| c.n).sum();
        let sq: f64 = sel
            .iter()
            .map(|c| (c.rms as f64).powi(2) * c.n as f64)
            .sum();
        let meas = (sq / n as f64).sqrt();
        let lo = sel.iter().map(|c| c.e_lo).min().unwrap();
        let hi = sel.iter().map(|c| c.e_hi).max().unwrap();
        let mid = (lo + hi) / 2;
        let fitted = sigma.value(mid, d, p);
        let old = legacy.value(mid, d, p);
        println!(
            "{d:>5} {p:>4} {n:>8} {:>9} {meas:>8.3} {fitted:>8.3} {:>6.2}x",
            format!("{lo}-{hi}"),
            fitted / old
        );
    }
    println!(
        "a={:.6} b={:.6} c={:.6} qa={:.6} qb={:.6} qc={:.6}   (weighted rmse {rmse:.4})",
        coef[0], coef[1], coef[2], coef[3], coef[4], coef[5]
    );

    if show_cells {
        println!();
        println!(
            "{:>5} {:>4} {:>9} {:>8} {:>8} {:>8} {:>8}",
            "depth", "pc", "empties", "n", "measured", "fitted", "err"
        );
        for c in &cells {
            let f = sigma.value(c.empties.round() as u32, c.depth, c.pc_depth);
            println!(
                "{:>5} {:>4} {:>9} {:>8} {:>8.3} {:>8.3} {:>+8.3}",
                c.depth,
                c.pc_depth,
                format!("{}-{}", c.e_lo, c.e_hi),
                c.n,
                c.rms,
                f,
                f - c.rms
            );
        }
    }

    /* The fit is a quadratic, so outside the empty counts it saw it can
    turn down through zero or run away upward. Both are survivable -- the
    floor catches one and a huge margin only means no pruning -- but a
    silent one is not, so the whole deployed domain gets walked. */
    let (mut worst_lo, mut worst_hi) = (f32::MAX, 0.0f32);
    let mut floored = 0usize;
    for e in 1..=60u32 {
        for &(d, p) in &pairs {
            let sv = e as f32 * sigma.a + d as f32 * sigma.b + p as f32 * sigma.c;
            let raw = sigma.qa * sv * sv + sigma.qb * sv + sigma.qc;
            if raw < 1.0 {
                floored += 1;
            }
            worst_lo = worst_lo.min(raw);
            worst_hi = worst_hi.max(raw);
        }
    }
    println!(
        "over 1..60 empties x {} pairs: raw sigma spans {worst_lo:.2}..{worst_hi:.2}, {floored} \
         of {} cells hit the 1.0 floor",
        pairs.len(),
        60 * pairs.len()
    );

    if !write {
        println!(
            "not written (pass --write to store this in {})",
            nnue_path.display()
        );
        return ExitCode::SUCCESS;
    }
    if from_csv.is_some() {
        /* `--from-csv` is the one hole in "a sigma can only be written to
        the file it was measured against": the values came from whatever
        model produced the csv, which nothing here can check. Say so. */
        eprintln!(
            "warning: these values were measured by an earlier run, not by this one -- \
             writing them is only correct if {} holds those same weights",
            nnue_path.display()
        );
    }
    /* Write back into the very file that was measured. Loading again rather
    than reusing the quantized copy keeps the saved weights byte-identical to
    what was on disk: this run must change the sigma and nothing else. */
    let mut out = Nnue::new(patterns);
    if let Err(err) = out.load(&nnue_path) {
        eprintln!("failed to re-read {}: {err}", nnue_path.display());
        return ExitCode::FAILURE;
    }
    out.set_mpc_sigma(Some(sigma));
    if let Err(err) = out.save(&nnue_path) {
        eprintln!("failed to write {}: {err}", nnue_path.display());
        return ExitCode::FAILURE;
    }
    println!("sigma written to {}", nnue_path.display());
    ExitCode::SUCCESS
}
