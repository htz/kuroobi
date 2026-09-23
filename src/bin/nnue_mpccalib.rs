//! Measures the ProbCut sigma of one NNUE model and writes it into that model's own weights file.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use kuroobi::midgame::{mpc_reduced_depth, NnueSearch, SharedTt};
use kuroobi::nnue::{MpcSigma, Nnue};
use kuroobi::trainer::load_examples_binary;

const DISC_LIMIT: f32 = 64.0;

const LEGACY: [f32; MpcSigma::LEN] = [-0.068941, 0.368775, -0.713476, 0.010223, 0.647219, 4.050545];

struct Cell {
    empties: f64,
    e_lo: u32,
    e_hi: u32,
    depth: u32,
    pc_depth: u32,
    n: usize,
    rms: f32,
}

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

const NP: usize = 5;

fn model(t: &[f64; NP], e: f64, d: f64, p: f64) -> (f64, [f64; NP]) {
    let s = e + t[0] * d + t[1] * p;
    let f = t[2] * s * s + t[3] * s + t[4];
    let ds = 2.0 * t[2] * s + t[3];
    (f, [ds * d, ds * p, s * s, s, 1.0])
}

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

struct Args {
    paths: Vec<PathBuf>,
    stride: usize,
    threads: usize,
    max_positions: usize,
    max_depth: u32,
    depths: Vec<u32>,
    min_empties: u32,
    max_empties: u32,
    min_cell: usize,
    empties_bin: u32,
    csv: Option<PathBuf>,
    from_csv: Option<PathBuf>,
    show_cells: bool,
    write: bool,
    legacy_sigma: bool,
    which: String,
    spec: Option<PathBuf>,
}

fn parse_args() -> Result<Args, ExitCode> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut stride = 997usize; // prime stride decorrelates from file order
    let mut max_positions = 2000usize;
    let mut threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let mut max_depth = 12u32;
    let mut depths: Vec<u32> = Vec::new();
    let mut min_empties = 20u32;
    let mut max_empties = 58u32;
    let mut min_cell = 20usize;
    let mut empties_bin = 3u32;
    let mut csv: Option<PathBuf> = None;
    let mut from_csv: Option<PathBuf> = None;
    let mut show_cells = false;
    let mut write = false;
    let mut legacy_sigma = false;
    let mut which = String::from("nnue");
    let mut spec: Option<PathBuf> = None;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--patterns" => which = it.next().unwrap_or(which),
            "--patterns-file" => spec = it.next().map(PathBuf::from),
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
    Ok(Args {
        paths,
        stride,
        threads,
        max_positions,
        max_depth,
        depths,
        min_empties,
        max_empties,
        min_cell,
        empties_bin,
        csv,
        from_csv,
        show_cells,
        write,
        legacy_sigma,
        which,
        spec,
    })
}

fn main() -> ExitCode {
    let Args {
        mut paths,
        stride,
        threads,
        max_positions,
        max_depth,
        mut depths,
        min_empties,
        max_empties,
        min_cell,
        empties_bin,
        csv,
        from_csv,
        show_cells,
        write,
        legacy_sigma,
        which,
        spec,
    } = match parse_args() {
        Ok(a) => a,
        Err(code) => return code,
    };
    let want_data = !legacy_sigma && from_csv.is_none();
    if paths.is_empty() || (want_data && paths.len() < 2) {
        eprintln!(
            "usage: nnue_mpccalib [--threads N] [--stride N] [--max N] [--max-depth 12] \
             [--depths a,b,c] [--patterns nnue|linear] [--patterns-file <spec>] [--min-empties 20] [--max-empties 58] \
             [--min-cell 20] [--empties-bin 3] [--csv out.csv] [--cells] [--write] \
             <nnue.bin> <data-file>..."
        );
        return ExitCode::FAILURE;
    }
    let nnue_path = paths.remove(0);

    if legacy_sigma {
        return write_legacy_sigma(&nnue_path, &which, spec.as_deref());
    }
    let Some(pairs) = depth_pairs(max_depth, &mut depths) else {
        return ExitCode::FAILURE;
    };
    let patterns = match kuroobi::pattern::resolve(&which, spec.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let rows = match &from_csv {
        Some(path) => read_csv(path, &mut depths),
        None => measure(
            &nnue_path,
            patterns,
            &paths,
            &depths,
            Scan {
                stride,
                min_empties,
                max_empties,
                max_positions,
                threads,
            },
        ),
    };
    let Ok(rows) = rows else {
        return ExitCode::FAILURE;
    };
    if let Some(path) = &csv {
        write_csv(path, &depths, &rows);
    }
    let kept = keep_finite(&rows);
    if kept.is_empty() {
        eprintln!("nothing left to fit");
        return ExitCode::FAILURE;
    }
    let cells = build_cells(&kept, &pairs, &depths, empties_bin, min_cell);
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
    report_pairs(&cells, &pairs, &sigma, coef, rmse);
    if show_cells {
        report_cells(&cells, &sigma);
    }
    report_span(&pairs, &sigma);

    if !write {
        println!(
            "not written (pass --write to store this in {})",
            nnue_path.display()
        );
        return ExitCode::SUCCESS;
    }
    if from_csv.is_some() {
        eprintln!(
            "warning: these values were measured by an earlier run, not by this one -- \
             writing them is only correct if {} holds those same weights",
            nnue_path.display()
        );
    }
    store_sigma(&nnue_path, patterns, sigma)
}

struct Scan {
    stride: usize,
    min_empties: u32,
    max_empties: u32,
    max_positions: usize,
    threads: usize,
}

fn write_legacy_sigma(nnue_path: &Path, which: &str, spec: Option<&Path>) -> ExitCode {
    let patterns = match kuroobi::pattern::resolve(which, spec) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut nn = Nnue::new(patterns);
    if let Err(err) = nn.load(nnue_path) {
        eprintln!("failed to load {}: {err}", nnue_path.display());
        return ExitCode::FAILURE;
    }
    nn.set_mpc_sigma(Some(MpcSigma::from_array(LEGACY)));
    if let Err(err) = nn.save(nnue_path) {
        eprintln!("failed to write {}: {err}", nnue_path.display());
        return ExitCode::FAILURE;
    }
    println!(
        "wrote the linear linear's constants into {} -- for A/B only, \
         this is not a measurement of this model",
        nnue_path.display()
    );
    ExitCode::SUCCESS
}

/// The (depth, pc_depth) pairs to fit, and the depths they need searched.
fn depth_pairs(max_depth: u32, depths: &mut Vec<u32>) -> Option<Vec<(u32, u32)>> {
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
        return None;
    }
    Some(pairs)
}

fn read_csv(path: &Path, depths: &mut Vec<u32>) -> Result<Vec<(u32, Vec<f32>)>, ()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(err) => {
            eprintln!("failed to read {}: {err}", path.display());
            return Err(());
        }
    };
    let mut it = text.lines();
    let Some(header) = it.next() else {
        eprintln!("{} is empty", path.display());
        return Err(());
    };
    *depths = header
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
    Ok(out)
}

fn measure(
    nnue_path: &Path,
    patterns: &'static [kuroobi::pattern::Pattern],
    paths: &[PathBuf],
    depths: &[u32],
    Scan {
        stride,
        min_empties,
        max_empties,
        max_positions,
        threads,
    }: Scan,
) -> Result<Vec<(u32, Vec<f32>)>, ()> {
    let mut nn = Nnue::new(patterns);
    if let Err(err) = nn.load(nnue_path) {
        eprintln!("failed to load {}: {err}", nnue_path.display());
        return Err(());
    }
    nn.quantize();
    let nn = std::sync::Arc::new(nn);

    let mut boards = Vec::new();
    'outer: for path in paths {
        let examples = match load_examples_binary(path) {
            Ok(ex) => ex,
            Err(err) => {
                eprintln!("failed to load {}: {err}", path.display());
                return Err(());
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
    Ok(std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (next, boards, depths) = (&next, &boards, &depths);
                let nn = nn.clone();
                s.spawn(move || {
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
    }))
}

fn write_csv(path: &Path, depths: &[u32], rows: &[(u32, Vec<f32>)]) {
    use std::io::Write;
    match std::fs::File::create(path) {
        Ok(f) => {
            let mut w = std::io::BufWriter::new(f);
            let _ = write!(w, "empties");
            for d in depths {
                let _ = write!(w, ",d{d}");
            }
            let _ = writeln!(w);
            for (e, v) in rows {
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

/// Rows holding a terminal value would dominate the fit.
fn keep_finite(rows: &[(u32, Vec<f32>)]) -> Vec<&(u32, Vec<f32>)> {
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
    kept
}

fn build_cells(
    kept: &[&(u32, Vec<f32>)],
    pairs: &[(u32, u32)],
    depths: &[u32],
    empties_bin: u32,
    min_cell: usize,
) -> Vec<Cell> {
    let idx = |d: u32| depths.iter().position(|&x| x == d).unwrap();
    struct Acc {
        n: usize,
        sq: f64,
        e_sum: f64,
        e_lo: u32,
        e_hi: u32,
    }
    let mut acc: std::collections::BTreeMap<(u32, u32, u32), Acc> = Default::default();
    for (e, v) in kept {
        for &(d, p) in pairs {
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
    cells
}

fn report_pairs(
    cells: &[Cell],
    pairs: &[(u32, u32)],
    sigma: &MpcSigma,
    coef: [f32; MpcSigma::LEN],
    rmse: f64,
) {
    let legacy = MpcSigma::from_array(LEGACY);
    println!(
        "{:>5} {:>4} {:>8} {:>9} {:>8} {:>8} {:>7}",
        "depth", "pc", "n", "empties", "measured", "fitted", "vs old"
    );
    for &(d, p) in pairs {
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
}

fn report_cells(cells: &[Cell], sigma: &MpcSigma) {
    println!();
    println!(
        "{:>5} {:>4} {:>9} {:>8} {:>8} {:>8} {:>8}",
        "depth", "pc", "empties", "n", "measured", "fitted", "err"
    );
    for c in cells {
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

fn report_span(pairs: &[(u32, u32)], sigma: &MpcSigma) {
    let (mut worst_lo, mut worst_hi) = (f32::MAX, 0.0f32);
    let mut floored = 0usize;
    for e in 1..=60u32 {
        for &(d, p) in pairs {
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
}

fn store_sigma(
    nnue_path: &Path,
    patterns: &'static [kuroobi::pattern::Pattern],
    sigma: MpcSigma,
) -> ExitCode {
    let mut out = Nnue::new(patterns);
    if let Err(err) = out.load(nnue_path) {
        eprintln!("failed to re-read {}: {err}", nnue_path.display());
        return ExitCode::FAILURE;
    }
    out.set_mpc_sigma(Some(sigma));
    if let Err(err) = out.save(nnue_path) {
        eprintln!("failed to write {}: {err}", nnue_path.display());
        return ExitCode::FAILURE;
    }
    println!("sigma written to {}", nnue_path.display());
    ExitCode::SUCCESS
}
