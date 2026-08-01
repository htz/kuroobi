//! Kifu-based training CLI (the Rust counterpart of Go's cmd/train).
//!
//! Usage:
//!   train [OPTIONS] <data-file>...
//!
//! Data files may be binary (.data, 17-byte records) or text (.txt,
//! "<64 board chars> <score>" per line); the format is chosen by extension.
//!
//! Options:
//!   --epochs <n>      Number of passes over all examples (default 10)
//!   --lr <f>          Adam learning rate (default 0.01)
//!   --weights <path>  Weight file to load (if it exists) and save
//!                     (default weights.bin, saved after every epoch)
//!   --patterns <set>  Pattern library: egaroucid | edax (default egaroucid)
//!   --limit <n>       Use at most n examples per file (default all)
//!   --max-examples <n> Examples held in RAM at once (default 64M, 0 = all)
//!   --log <path>      Append per-epoch stage losses as CSV
//!
//! Data too large to fit in RAM is trained in **shards**: whole files are
//! grouped up to `--max-examples`, and each epoch walks every shard, loading
//! and dropping one at a time.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use kuroobi::evaluator::{AdamOptimizer, Evaluator, Optimizer, SgdOptimizer, STAGE_COUNT};
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::trainer::{
    count_examples_binary, load_examples_binary_into, load_examples_text_into, EpochStats, Example,
    Trainer,
};

/// Examples kept in RAM at once when `--max-examples` is not given.
/// An `Example` is 24 bytes, so this caps the sample buffer at ~1.5 GB —
/// small enough to leave room for the 150 MB weight tables and the OS, and
/// large enough that a shard is many minutes of training, not seconds.
const DEFAULT_MAX_EXAMPLES: usize = 64_000_000;

/// Bytes per `Example` in memory, for the reported budget.
const EXAMPLE_BYTES: usize = std::mem::size_of::<Example>();

struct Args {
    epochs: usize,
    learning_rate: f32,
    decay: f32,
    optimizer: OptimizerKind,
    weights_path: PathBuf,
    patterns: &'static str,
    limit: Option<usize>,
    max_examples: Option<usize>,
    log_path: Option<PathBuf>,
    threads: usize,
    swa: bool,
    swa_start: usize,
    val_files: Vec<PathBuf>,
    data_files: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq)]
enum OptimizerKind {
    Sgd,
    Adam,
}

const USAGE: &str = "\
Usage: train [OPTIONS] <data-file>...

Train the pattern evaluator on labeled positions (kifu-derived data).
Files ending in .txt are parsed as text; anything else as 17-byte binary
records (kifu-converter output).

Options:
  --epochs <n>      Passes over all examples (default 10)
  --threads <n>     Parallel workers (default 1). SGD only: the workers
                    share the weights without locking, which is sound because
                    each example touches only a handful of cells
  --optimizer <o>   sgd | adam (default sgd; sgd's error-proportional step
                    converges much faster on this linear model)
  --lr <f>          Learning rate (default: sgd 0.002, adam 0.01)
  --decay <f>       SGD per-epoch lr decay factor (default 0.95)
  --weights <path>  Weight file to load/save (default weights.bin)
  --patterns <set>  egaroucid | edax | egaroucid-plus (default egaroucid)
  --limit <n>       Max examples per file
  --max-examples <n>
                    Examples held in RAM at once (default 64000000, 0 = all).
                    Datasets larger than this are split into shards of whole
                    files; every epoch walks all shards, loading one at a time
  --log <path>      Append per-epoch stage losses as CSV
  --val <path>      Held-out file scored after every epoch (repeatable).
                    The in-epoch training MSE is measured while the weights
                    are still moving, so it is a poor stopping signal; the
                    val MSE is the honest one. When given, the weights with
                    the best val MSE are also saved to <weights>.best
  --swa             Stochastic Weight Averaging: keep a running mean of the
                    per-epoch weights and save it to <weights>.swa. On this
                    convex (linear-model) loss the SGD iterates bounce around
                    the true minimum; their average sits closer to it than any
                    single epoch, so <weights>.swa usually beats <weights>.best
  --swa-start <n>   First epoch folded into the SWA mean (default 2; earlier
                    epochs are still settling from the start point)
  -h, --help        Show this help";

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        epochs: 10,
        learning_rate: f32::NAN, // resolved after optimizer choice
        decay: 0.95,
        optimizer: OptimizerKind::Sgd,
        threads: 1,
        weights_path: PathBuf::from("weights/weights.bin"),
        patterns: "egaroucid",
        limit: None,
        max_examples: Some(DEFAULT_MAX_EXAMPLES),
        log_path: None,
        swa: false,
        swa_start: 2,
        val_files: Vec::new(),
        data_files: Vec::new(),
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} requires a value"));
        match arg.as_str() {
            "--epochs" => {
                args.epochs = value("--epochs")?
                    .parse()
                    .map_err(|e| format!("--epochs: {e}"))?
            }
            "--lr" => {
                args.learning_rate = value("--lr")?.parse().map_err(|e| format!("--lr: {e}"))?
            }
            "--decay" => {
                args.decay = value("--decay")?
                    .parse()
                    .map_err(|e| format!("--decay: {e}"))?
            }
            "--threads" => {
                args.threads = value("--threads")?
                    .parse()
                    .map_err(|e| format!("--threads: {e}"))?
            }
            "--optimizer" => {
                let v = value("--optimizer")?;
                args.optimizer = match v.as_str() {
                    "sgd" => OptimizerKind::Sgd,
                    "adam" => OptimizerKind::Adam,
                    other => return Err(format!("unknown optimizer: {other}")),
                };
            }
            "--weights" => args.weights_path = PathBuf::from(value("--weights")?),
            "--patterns" => {
                let v = value("--patterns")?;
                match v.as_str() {
                    "egaroucid" => args.patterns = "egaroucid",
                    "egaroucid-plus" => args.patterns = "egaroucid-plus",
                    "edax" => args.patterns = "edax",
                    other => return Err(format!("unknown pattern set: {other}")),
                }
            }
            "--limit" => {
                args.limit = Some(
                    value("--limit")?
                        .parse()
                        .map_err(|e| format!("--limit: {e}"))?,
                )
            }
            "--max-examples" => {
                let n: usize = value("--max-examples")?
                    .parse()
                    .map_err(|e| format!("--max-examples: {e}"))?;
                args.max_examples = (n > 0).then_some(n);
            }
            "--log" => args.log_path = Some(PathBuf::from(value("--log")?)),
            "--val" => args.val_files.push(PathBuf::from(value("--val")?)),
            "--swa" => args.swa = true,
            "--swa-start" => {
                args.swa_start = value("--swa-start")?
                    .parse()
                    .map_err(|e| format!("--swa-start: {e}"))?
            }
            "-h" | "--help" => return Err(USAGE.to_string()),
            other if other.starts_with('-') => {
                return Err(format!("unknown option: {other}\n\n{USAGE}"))
            }
            file => args.data_files.push(PathBuf::from(file)),
        }
    }

    // Optimizer-appropriate default learning rate.
    // SGD: each of the 64 active cells moves by lr*err and is read back, so
    // the per-step contraction is (1 - 64*lr); lr = 0.002 gives a stable,
    // fast 12.8%/step pull. (This matches the Go trainer's regime: lr 0.01
    // there is applied WITHOUT symmetry-augmented repeat visits per sample.)
    if args.learning_rate.is_nan() {
        args.learning_rate = match args.optimizer {
            OptimizerKind::Sgd => 0.002,
            OptimizerKind::Adam => 0.01,
        };
    }

    if args.data_files.is_empty() {
        return Err(format!("no data files given\n\n{USAGE}"));
    }
    Ok(args)
}

fn is_text(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "txt")
}

/// Append one file's examples to `out`, returning how many were added.
fn load_file_into(
    path: &Path,
    limit: Option<usize>,
    out: &mut Vec<Example>,
) -> std::io::Result<usize> {
    if is_text(path) {
        load_examples_text_into(path, out, limit)
    } else {
        load_examples_binary_into(path, out, limit)
    }
}

/// The dataset, sized but not loaded.
///
/// Counts start as size-derived estimates so shards can be planned without
/// reading 16 GB first; each file's entry is replaced by the true count once
/// that file has actually been loaded.
struct DataPlan {
    files: Vec<PathBuf>,
    counts: Vec<usize>,
}

/// A group of whole files that fit in the memory budget together.
struct Shard {
    files: Vec<usize>,
    examples: usize,
}

impl DataPlan {
    fn new(files: Vec<PathBuf>, limit: Option<usize>) -> std::io::Result<DataPlan> {
        let mut counts = Vec::with_capacity(files.len());
        for f in &files {
            // Binary records are fixed-width so the count is exact. A text
            // line is 64 board chars + separator + score + newline, i.e. at
            // least 67 bytes, so dividing by 67 over-estimates — the safe
            // direction for a memory budget.
            let est = if is_text(f) {
                std::fs::metadata(f)?.len() as usize / 67
            } else {
                count_examples_binary(f)?
            };
            counts.push(limit.map_or(est, |l| est.min(l)));
        }
        Ok(DataPlan { files, counts })
    }

    fn total(&self) -> usize {
        self.counts.iter().sum()
    }

    /// Group files into shards no larger than `max` examples.
    ///
    /// `order` lets the caller vary which files share a shard between epochs,
    /// so a fixed grouping does not become a fixed correlation in the data.
    fn shards(&self, order: &[usize], max: Option<usize>) -> Vec<Shard> {
        let mut shards: Vec<Shard> = Vec::new();
        let mut cur = Shard {
            files: Vec::new(),
            examples: 0,
        };
        for &i in order {
            let n = self.counts[i];
            if !cur.files.is_empty() && max.is_some_and(|m| cur.examples + n > m) {
                shards.push(std::mem::replace(
                    &mut cur,
                    Shard {
                        files: Vec::new(),
                        examples: 0,
                    },
                ));
            }
            cur.files.push(i);
            cur.examples += n;
        }
        if !cur.files.is_empty() {
            shards.push(cur);
        }
        shards
    }
}

/// xorshift64* — a deterministic stream, seeded per epoch and shard so runs
/// reproduce exactly while the ordering still differs between passes.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        // Any nonzero state works; mixing in a constant keeps seed 0 usable.
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// In-place Fisher-Yates.
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

fn fmt_count(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn fmt_bytes(n: usize) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    if n as f64 >= GB {
        format!("{:.1} GB", n as f64 / GB)
    } else {
        format!("{:.0} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn append_log(
    path: &Path,
    epoch: usize,
    stats: &kuroobi::trainer::EpochStats,
) -> std::io::Result<()> {
    use std::io::Write;
    let new_file = !path.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    if new_file {
        writeln!(f, "epoch,stage,samples,loss_sum,loss_avg")?;
    }
    for stage in 0..STAGE_COUNT {
        if stats.samples[stage] > 0 {
            writeln!(
                f,
                "{},{},{},{:.6},{:.6}",
                epoch,
                stage,
                stats.samples[stage],
                stats.loss_sum[stage],
                stats.stage_mse(stage)
            )?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let patterns = match args.patterns {
        "edax" => EDAX_PATTERNS,
        "egaroucid-plus" => EGAROUCID_PLUS_PATTERNS,
        _ => EGAROUCID_PATTERNS,
    };

    // Size the dataset from file metadata only. Loading it all up front used
    // to be the "read once, reuse across epochs" optimization, but at 16 GB
    // on disk (and ~1.4x that in RAM) the machine dies before epoch 1; past
    // a few GB the re-read is a rounding error next to the training itself.
    let mut plan = match DataPlan::new(args.data_files.clone(), args.limit) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to size input files: {e}");
            return ExitCode::FAILURE;
        }
    };
    let identity: Vec<usize> = (0..plan.files.len()).collect();
    let shard_count = plan.shards(&identity, args.max_examples).len();
    println!(
        "data: {} files, ~{} examples ({} in RAM if loaded at once)",
        plan.files.len(),
        fmt_count(plan.total()),
        fmt_bytes(plan.total() * EXAMPLE_BYTES),
    );
    match args.max_examples {
        Some(max) if shard_count > 1 => println!(
            "shards: {shard_count} per epoch, up to {} examples ({}) resident",
            fmt_count(max),
            fmt_bytes(max * EXAMPLE_BYTES),
        ),
        _ => println!("shards: 1 (whole dataset fits the budget)"),
    }
    // A file bigger than the budget cannot be split — shards are whole files.
    if let Some(max) = args.max_examples {
        for (f, &n) in plan.files.iter().zip(&plan.counts) {
            if n > max {
                eprintln!(
                    "warning: {} alone holds ~{} examples, over the {} budget; \
                     it will be loaded whole",
                    f.display(),
                    fmt_count(n),
                    fmt_count(max),
                );
            }
        }
    }

    // Held-out set for an honest per-epoch signal, loaded once and kept
    // resident (it is small relative to training). The in-epoch training MSE
    // is measured on moving weights and is a poor stopping signal.
    let mut val: Vec<Example> = Vec::new();
    for f in &args.val_files {
        if let Err(e) = load_file_into(f, None, &mut val) {
            eprintln!("failed to load val {}: {e}", f.display());
            return ExitCode::FAILURE;
        }
    }
    if !args.val_files.is_empty() {
        println!("val: {} held-out examples", fmt_count(val.len()));
    }

    // Evaluator: resume from an existing weight file when present
    let mut evaluator = Evaluator::new(patterns);
    if args.weights_path.exists() {
        match evaluator.load_weights(&args.weights_path) {
            Ok(()) => println!("resumed weights from {}", args.weights_path.display()),
            Err(e) => {
                eprintln!("failed to load {}: {e}", args.weights_path.display());
                return ExitCode::FAILURE;
            }
        }
    } else {
        println!("starting from zero weights");
    }

    // Ctrl-C requests a graceful stop: the current epoch finishes, weights
    // are saved, and the loop exits cleanly. A second Ctrl-C force-quits.
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let flag = interrupted.clone();
        if let Err(e) = ctrlc_handler(move || {
            if flag.swap(true, Ordering::SeqCst) {
                eprintln!("\nsecond interrupt: exiting immediately");
                std::process::exit(130);
            }
            eprintln!("\ninterrupt received: finishing current epoch, then saving...");
        }) {
            eprintln!("warning: could not install Ctrl-C handler: {e}");
        }
    }

    match args.optimizer {
        OptimizerKind::Sgd => {
            println!(
                "optimizer: sgd (lr {}, decay {})",
                args.learning_rate, args.decay
            );
            let trainer =
                Trainer::new(evaluator, SgdOptimizer::new(args.learning_rate, args.decay));
            run_epochs(trainer, &args, &mut plan, &val, &interrupted)
        }
        OptimizerKind::Adam => {
            println!("optimizer: adam (lr {})", args.learning_rate);
            let trainer = Trainer::new(evaluator, AdamOptimizer::new(args.learning_rate));
            run_epochs(trainer, &args, &mut plan, &val, &interrupted)
        }
    }
}

/// Render an in-place progress bar on stderr:
/// `epoch 3/100 shard 2/15 [=====>    ]  45.2%  12.3M/25.5M  33k pos/s  ETA 6m32s`
///
/// `done`/`total` count the whole epoch, not the current shard, so the bar
/// advances monotonically across shard boundaries.
fn draw_progress(
    epoch: usize,
    epochs: usize,
    shard: (usize, usize),
    done: usize,
    total: usize,
    started: &Instant,
) {
    const BAR_WIDTH: usize = 24;

    // Text-file counts are estimates, so `done` can overshoot `total`.
    let frac = if total == 0 {
        1.0
    } else {
        (done as f64 / total as f64).min(1.0)
    };
    let filled = (frac * BAR_WIDTH as f64) as usize;
    let mut bar = String::with_capacity(BAR_WIDTH);
    for i in 0..BAR_WIDTH {
        bar.push(match i.cmp(&filled) {
            std::cmp::Ordering::Less => '=',
            std::cmp::Ordering::Equal => '>',
            std::cmp::Ordering::Greater => ' ',
        });
    }

    let elapsed = started.elapsed().as_secs_f64();
    let per_sec = if elapsed > 0.0 {
        done as f64 / elapsed
    } else {
        0.0
    };
    let eta_secs = if per_sec > 0.0 {
        (total.saturating_sub(done) as f64 / per_sec) as u64
    } else {
        0
    };
    let eta = if eta_secs >= 60 {
        format!("{}m{:02}s", eta_secs / 60, eta_secs % 60)
    } else {
        format!("{eta_secs}s")
    };

    let shard_label = if shard.1 > 1 {
        format!(" shard {}/{}", shard.0, shard.1)
    } else {
        String::new()
    };

    eprint!(
        "\repoch {epoch}/{epochs}{shard_label} [{bar}] {:5.1}%  {}/{}  {:.0}k pos/s  ETA {eta}   ",
        frac * 100.0,
        fmt_count(done),
        fmt_count(total),
        per_sec / 1e3,
    );
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

/// Mean squared error of `evaluator` over a held-out set, sharded across
/// `threads`. This scores the frozen weights (no updates), so unlike the
/// in-epoch training MSE it is a clean early-stopping signal.
fn val_mse(evaluator: &Evaluator, val: &[Example], threads: usize) -> f64 {
    if val.is_empty() {
        return f64::NAN;
    }
    let sum: f64 = if threads > 1 {
        let chunk = val.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let handles: Vec<_> = val
                .chunks(chunk)
                .map(|part| {
                    scope.spawn(move || {
                        part.iter()
                            .map(|ex| {
                                let e = ex.score as f64 - evaluator.eval(&ex.board()) as f64;
                                e * e
                            })
                            .sum::<f64>()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).sum()
        })
    } else {
        val.iter()
            .map(|ex| {
                let e = ex.score as f64 - evaluator.eval(&ex.board()) as f64;
                e * e
            })
            .sum()
    };
    sum / val.len() as f64
}

/// Running mean of the per-epoch weight files, for Stochastic Weight
/// Averaging. Folds each epoch's serialized weights (the `save_weights`
/// output) so it never has to reach into the evaluator's internals: the
/// header is captured once and the f32 payload is averaged in f64.
struct SwaAccumulator {
    header: Vec<u8>,
    sum: Vec<f64>,
    count: u64,
    out: PathBuf,
}

impl SwaAccumulator {
    /// Fold the current `weights_path` file into the mean.
    fn fold(&mut self, weights_path: &Path) -> std::io::Result<()> {
        let bytes = std::fs::read(weights_path)?;
        if self.header.is_empty() {
            // Header = magic(8) + stage u32 + pattern u32 + table_size u32
            // each; capture it once and average the f32 payload thereafter.
            let pattern_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
            let header_len = 16 + 4 * pattern_count;
            self.header = bytes[..header_len].to_vec();
            self.sum = vec![0.0; (bytes.len() - header_len) / 4];
        }
        let payload = &bytes[self.header.len()..];
        debug_assert_eq!(payload.len() / 4, self.sum.len());
        for (i, chunk) in payload.chunks_exact(4).enumerate() {
            self.sum[i] += f32::from_le_bytes(chunk.try_into().unwrap()) as f64;
        }
        self.count += 1;
        Ok(())
    }

    /// Write the mean to `<weights>.swa`. Returns false if nothing was folded.
    fn write(&self) -> std::io::Result<bool> {
        if self.count == 0 {
            return Ok(false);
        }
        let inv = 1.0 / self.count as f64;
        let mut buf = self.header.clone();
        buf.reserve(self.sum.len() * 4);
        for &s in &self.sum {
            buf.extend_from_slice(&((s * inv) as f32).to_le_bytes());
        }
        let tmp = self.out.with_extension("swa.tmp");
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, &self.out)?;
        Ok(true)
    }
}

fn run_epochs<O: Optimizer>(
    mut trainer: Trainer<O>,
    args: &Args,
    plan: &mut DataPlan,
    val: &[Example],
    interrupted: &AtomicBool,
) -> ExitCode {
    let parallel = args.threads > 1 && args.optimizer == OptimizerKind::Sgd;
    // One buffer for the whole run: after the first shard it already holds
    // enough capacity, so later shards reuse the allocation instead of
    // handing the allocator a multi-gigabyte free/alloc pair every time.
    let mut examples: Vec<Example> = Vec::new();
    // Best val MSE seen and where its weights live (<weights>.best).
    let mut best_val = f64::INFINITY;
    let best_path = {
        let mut p = args.weights_path.clone().into_os_string();
        p.push(".best");
        PathBuf::from(p)
    };
    let mut swa = args.swa.then(|| {
        let mut p = args.weights_path.clone().into_os_string();
        p.push(".swa");
        SwaAccumulator {
            header: Vec::new(),
            sum: Vec::new(),
            count: 0,
            out: PathBuf::from(p),
        }
    });

    for epoch in 1..=args.epochs {
        let t = Instant::now();

        // Vary which files share a shard between epochs: within a shard the
        // examples are shuffled, but a fixed grouping would still mean two
        // files never mix, and these sources differ systematically.
        let mut order: Vec<usize> = (0..plan.files.len()).collect();
        if epoch > 1 {
            Rng::new(epoch as u64).shuffle(&mut order);
        }
        let shards = plan.shards(&order, args.max_examples);
        let epoch_total: usize = shards.iter().map(|s| s.examples).sum();

        let mut stats = EpochStats::default();
        let mut done = 0usize;
        for (si, shard) in shards.iter().enumerate() {
            examples.clear();
            for &fi in &shard.files {
                let before = examples.len();
                match load_file_into(&plan.files[fi], args.limit, &mut examples) {
                    // Replace the size-derived estimate with the true count,
                    // so the progress bar stops guessing from epoch 2 on.
                    Ok(_) => plan.counts[fi] = examples.len() - before,
                    Err(e) => {
                        eprintln!("\nfailed to load {}: {e}", plan.files[fi].display());
                        return ExitCode::FAILURE;
                    }
                }
            }
            // SGD assumes IID sample order, but these datasets are grossly
            // ordered (per-opening-depth files, per-source directories);
            // training in file order makes the model drift toward whichever
            // source came last and the epoch loss creep upward.
            Rng::new((epoch as u64) << 32 | si as u64).shuffle(&mut examples);

            let progress = |n: usize, _total: usize| {
                draw_progress(
                    epoch,
                    args.epochs,
                    (si + 1, shards.len()),
                    done + n,
                    epoch_total,
                    &t,
                );
            };
            let shard_stats = if parallel {
                // The decay schedule lives in the optimizer; mirror it here so
                // the parallel path follows the same curve.
                let lr = args.learning_rate * args.decay.powi(epoch as i32 - 1);
                trainer.train_epoch_parallel(&examples, lr, args.threads, progress)
            } else {
                // `train_pass`, not `train_epoch_*`: the lr schedule advances
                // once per epoch, not once per shard.
                trainer.train_pass(&examples, progress)
            };
            stats.add(&shard_stats);
            done += examples.len();

            if interrupted.load(Ordering::SeqCst) {
                eprint!("\r{:width$}\r", "", width = 90);
                if let Err(e) = trainer.evaluator.save_weights(&args.weights_path) {
                    eprintln!("failed to save {}: {e}", args.weights_path.display());
                    return ExitCode::FAILURE;
                }
                println!(
                    "stopped during epoch {epoch} (shard {}/{}); weights saved to {}",
                    si + 1,
                    shards.len(),
                    args.weights_path.display()
                );
                return ExitCode::SUCCESS;
            }
        }
        if !parallel {
            trainer.optimizer.next_epoch();
        }

        // Clear the progress line before printing the epoch summary
        eprint!("\r{:width$}\r", "", width = 90);
        let elapsed = t.elapsed().as_secs_f32();
        let per_sec = done as f32 / elapsed;
        // The held-out score, if a val set was given. This is the honest
        // signal; `stats.mse()` (train MSE) is measured on moving weights.
        let vm = if val.is_empty() {
            f64::NAN
        } else {
            val_mse(&trainer.evaluator, val, args.threads)
        };
        let is_best = vm < best_val; // false when vm is NaN (no val set)
        let val_line = if val.is_empty() {
            String::new()
        } else {
            format!("  val {vm:.4}{}", if is_best { " *best" } else { "" })
        };
        println!(
            "epoch {:>3}/{}: mse {:.4}{}  ({:.1}s, {:.0} pos/s)",
            epoch,
            args.epochs,
            stats.mse(),
            val_line,
            elapsed,
            per_sec
        );

        if let Some(log) = &args.log_path {
            if let Err(e) = append_log(log, epoch, &stats) {
                eprintln!("failed to write log {}: {e}", log.display());
                return ExitCode::FAILURE;
            }
        }

        // Save after every epoch (atomic replace) so long runs are
        // interruption-safe: a kill mid-epoch loses at most that epoch.
        if let Err(e) = trainer.evaluator.save_weights(&args.weights_path) {
            eprintln!("failed to save {}: {e}", args.weights_path.display());
            return ExitCode::FAILURE;
        }
        // Keep the best-by-val weights separately: the last epoch is not
        // necessarily the best, and this run overwrites `weights_path`.
        if is_best {
            best_val = vm;
            if let Err(e) = trainer.evaluator.save_weights(&best_path) {
                eprintln!("failed to save {}: {e}", best_path.display());
                return ExitCode::FAILURE;
            }
        }
        // Fold this epoch into the SWA mean (past the warmup) and refresh the
        // .swa file so it is available even if the run is interrupted.
        if let Some(acc) = &mut swa {
            if epoch >= args.swa_start {
                if let Err(e) = acc
                    .fold(&args.weights_path)
                    .and_then(|_| acc.write().map(|_| ()))
                {
                    eprintln!("failed to update SWA: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }

        if interrupted.load(Ordering::SeqCst) {
            if let Some(acc) = &swa {
                if acc.count > 0 {
                    println!(
                        "SWA mean of {} epochs saved to {}",
                        acc.count,
                        acc.out.display()
                    );
                }
            }
            println!(
                "stopped after epoch {epoch}; weights saved to {}",
                args.weights_path.display()
            );
            return ExitCode::SUCCESS;
        }
    }

    println!("weights saved to {}", args.weights_path.display());
    if best_val.is_finite() {
        println!(
            "best val mse {:.4} saved to {}",
            best_val,
            best_path.display()
        );
    }
    if let Some(acc) = &swa {
        if acc.count > 0 {
            println!(
                "SWA mean of {} epochs saved to {}",
                acc.count,
                acc.out.display()
            );
        }
    }
    ExitCode::SUCCESS
}

/// Minimal SIGINT handler installation without external crates.
fn ctrlc_handler<F: FnMut() + Send + 'static>(handler: F) -> std::io::Result<()> {
    use std::sync::Mutex;
    static HANDLER: Mutex<Option<Box<dyn FnMut() + Send>>> = Mutex::new(None);

    extern "C" fn trampoline(_: libc::c_int) {
        // Best-effort: if the lock is contended we skip (async-signal safety
        // is approximated; acceptable for a training CLI).
        if let Ok(mut guard) = HANDLER.try_lock() {
            if let Some(h) = guard.as_mut() {
                h();
            }
        }
    }

    *HANDLER.lock().unwrap() = Some(Box::new(handler));
    // SAFETY: installing a signal handler with a valid extern "C" fn.
    let prev = unsafe { libc::signal(libc::SIGINT, trampoline as libc::sighandler_t) };
    if prev == libc::SIG_ERR {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
