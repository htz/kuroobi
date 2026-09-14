//! Trainer for the linear pattern evaluator ([`kuroobi::linear`]).
//!
//! Usage:
//!   linear_train [OPTIONS] <data-file>...
//!
//! Data files are `kuroobi::record` files (kifu2data and gendata output).
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
//!   --gpu             Train on the GPU (build with --features gpu)
//!   --minibatch <n>   Positions per GPU step (default 8192)
//!   --cosine          Anneal the rate to ~0 over the run, stepped per shard
//!   --search-value-to-ply <n>  Take the search value up to ply n, the
//!                     game's result after (the NNUE corpus's teacher)
//!   --drop-random     Drop positions whose move was random
//!   --keep-above-ply <n>  From this ply on, keep everything
//!   --min-ply <n>     Drop positions before this ply
//!
//! Data too large to fit in RAM is trained in **shards**: whole files are
//! grouped up to `--max-examples`, and each epoch walks every shard, loading
//! and dropping one at a time.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use kuroobi::linear::{AdamOptimizer, Linear, Optimizer, SgdOptimizer, STAGE_COUNT};
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::record::{Filter, TeacherPolicy};
use kuroobi::trainer::{
    count_examples_binary, load_examples_filtered_into, EpochStats, Example, Trainer,
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
    cosine: bool,
    gpu: bool,
    minibatch: usize,
    search_value_to_ply: Option<u8>,
    drop_random: bool,
    keep_above_ply: Option<u8>,
    min_ply: u8,
    swa: bool,
    swa_start: usize,
    per_stage_best: bool,
    select_by: String,
    cell_lr: bool,
    restore_on_halve: bool,
    min_appear: u32,
    lr_smooth: bool,
    stages_lo: usize,
    stages_hi: usize,
    warmup: usize,
    patience: usize,
    plateau: usize,
    plateau_factor: f32,
    plateau_min: f32,
    plateau_frac: f64,
    val_files: Vec<PathBuf>,
    data_files: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq)]
enum OptimizerKind {
    Sgd,
    Adam,
}

const USAGE: &str = "\
Usage: linear_train [OPTIONS] <data-file>...

Train the pattern linear on `kuroobi::record` files (kifu2data and
gendata output).

Options:
  --epochs <n>      Passes over all examples (default 10)
  --stage <n>       The one stage this run trains. Required for sgd: a run
                    covers one stage, and several stages means several
                    processes, not several threads
  --threads <n>     Workers for scoring the val set, and for adam's pass.
                    Sgd trains one stage on one thread; see --stage
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
  --gpu             Train on the GPU (needs a build with --features gpu).
                    Honours --stage / --stages: a run aimed at one stage
                    carries only that stage's tables, 12 MB against 712,
                    so eight of them share a device comfortably.
                    Minibatch Adam instead of the CPU path's per-example
                    steps, which is what makes the work parallel.
  --minibatch <n>   Positions per GPU step (default 8192). Each is eight
                    rows, one per symmetric form.
  --cosine          Anneal the rate from --lr to ~0 across the whole run,
                    stepped once per shard. Matches the NNUE trainer's
                    schedule, which is what makes the two comparable.
  --search-value-to-ply <n>
                    Up to and including ply n take the record's search
                    value; past it take the game's final disc difference.
                    Without it every record reads as the game result,
                    a different target from the one the NNUE was fitted
                    to on the same files.
  --drop-random     Drop positions whose move was chosen at random
  --keep-above-ply <n>
                    From this ply on, keep everything the two above drop
  --min-ply <n>     Drop positions before this ply
  --val <path>      Held-out file scored after every epoch (repeatable).
                    The in-epoch training MSE is measured while the weights
                    are still moving, so it is a poor stopping signal; the
                    val MSE is the honest one. When given, the weights with
                    the best val MSE are also saved to <weights>.best
  --plateau <n>     Halve the learning rate after n epochs in which no stage
                    improved its best. With --per-stage-best, whether an
                    epoch helped is a per-stage question: an epoch that
                    moved even one of the 61 counts as progress, and the
                    rate drops only when the whole model has stopped.
                    Stops when the rate falls below --plateau-min (1e-6).
                    Overrides --decay, which lowers the rate on a fixed
                    schedule whether or not the model is still learning
  --stages <a[-b]>  Train only these stages (0-60, by moves played, so stage
                    40 is 20 empties). Everything else is left exactly as it
                    was loaded. The stages are independent tables, so a run
                    can be pointed at whichever few are still not converging
                    instead of paying for all 61 to find out
  --warmup <n>      Start at a fifth of the rate and climb to it over n
                    epochs. A start point that was moved by hand -- a merge,
                    a smoothing pass -- sits off its own minimum, so the
                    first step is large exactly where the model is already
                    good
  --patience <n>    Stop a stage after n epochs that neither beat its
                    incumbent nor improved on the epoch before. A descent
                    that has not yet reached the incumbent still counts as
                    progress, so the clock only runs while the stage is
                    going nowhere. Without any of this a finished stage keeps
                    walking away from its best weights for the rest of the
                    run: 58 of 60 stages unable to return after three epochs
  --cell-lr         Divide each cell's step by how often that cell appears
                    in the training data, and skip cells that never appear.
                    A shared rate starves the rare cells and overshoots the
                    common ones, and the rate that suits neither gets halved
                    again and again -- 16 halvings in 29 epochs here without
                    the stage converging
  --restore-on-halve  On a halving, put the stage back on the weights that
                    scored its best before continuing. Halving alone only
                    slows a drift: training resumes from the weights that
                    just failed, so a stage started at too large a rate has
                    to walk all the way back. Measured here, empties 14 went
                    6.087 -> 6.375 over three epochs and two halvings without
                    once returning
  --min-appear N    With --cell-lr, leave cells seen N times or fewer alone.
                    Their step is the largest under per-cell scaling and the
                    least supported by data
  --lr-smooth       After each halving pass, replace every stage's rate with
                    the geometric mean of itself (weighted double) and its two
                    neighbours. Adjacent stages differ by one ply and want
                    nearly the same rate, but independent halving lets one
                    epoch of val noise leave a stage at twice its neighbour
  --plateau-frac <f>    Share of stages that must improve for an epoch to
                    count as progress (default 0.2). Below it the epoch is a
                    stall, however many stages crept
  --plateau-factor <f>  Multiplier applied on a stall (default 0.5)
  --plateau-min <f>     Rate floor; the run ends below it (default 1e-6)
  --select-by M     Which per-stage number decides a snapshot: mae
                    (default), mse, or spread -- the error with the stage's
                    constant offset taken out. See `stage_stats`
  --per-stage-best  Score the val set separately for each of the 61 stages
                    every epoch, keep each stage's best-scoring weights, and
                    save the assembled model to <weights>.stagebest. The
                    stages are independent tables, so one pooled val number
                    lets a stage that improved and a stage that rotted cancel
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
        cosine: false,
        gpu: false,
        minibatch: 8192,
        search_value_to_ply: None,
        drop_random: false,
        keep_above_ply: None,
        min_ply: 0,
        per_stage_best: false,
        select_by: String::from("mae"),
        cell_lr: false,
        restore_on_halve: false,
        min_appear: 0,
        lr_smooth: false,
        stages_lo: 0,
        stages_hi: STAGE_COUNT - 1,
        warmup: 0,
        patience: 0,
        plateau: 0,
        plateau_factor: 0.5,
        plateau_min: 1e-6,
        plateau_frac: 0.2,
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
            /* The NNUE runs anneal the rate to zero over the whole run
            rather than decaying it geometrically, and a linear model
            trained beside one has to be on the same schedule for the two
            to be comparable. */
            "--cosine" => args.cosine = true,
            /* The CPU path applies 512 sequential Adam steps per position
            on one core; the GPU sums a batch's touches of a cell and takes
            one step, which is the only shape of this work a GPU can run. */
            "--gpu" => args.gpu = true,
            "--minibatch" => {
                args.minibatch = value("--minibatch")?
                    .parse()
                    .map_err(|e| format!("--minibatch: {e}"))?
            }
            // The teacher the NNUE corpus is read under; without these the
            // two models fit different targets on the same files.
            "--search-value-to-ply" => {
                args.search_value_to_ply = Some(
                    value("--search-value-to-ply")?
                        .parse()
                        .map_err(|e| format!("--search-value-to-ply: {e}"))?,
                )
            }
            "--drop-random" => args.drop_random = true,
            "--keep-above-ply" => {
                args.keep_above_ply = Some(
                    value("--keep-above-ply")?
                        .parse()
                        .map_err(|e| format!("--keep-above-ply: {e}"))?,
                )
            }
            "--min-ply" => {
                args.min_ply = value("--min-ply")?
                    .parse()
                    .map_err(|e| format!("--min-ply: {e}"))?
            }
            "--swa" => args.swa = true,
            "--per-stage-best" => args.per_stage_best = true,
            "--select-by" => args.select_by = it.next().unwrap_or_default(),
            "--lr-smooth" => args.lr_smooth = true,
            "--cell-lr" => args.cell_lr = true,
            "--restore-on-halve" => args.restore_on_halve = true,
            "--min-appear" => {
                args.min_appear = value("--min-appear")?
                    .parse()
                    .map_err(|e| format!("--min-appear: {e}"))?
            }
            "--stages" => {
                let v = value("--stages")?;
                let (a, b) = v.split_once('-').unwrap_or((v.as_str(), v.as_str()));
                args.stages_lo = a.parse().map_err(|e| format!("--stages: {e}"))?;
                args.stages_hi = b.parse().map_err(|e| format!("--stages: {e}"))?;
                if args.stages_hi >= STAGE_COUNT || args.stages_lo > args.stages_hi {
                    return Err(format!("--stages: out of range 0-{}", STAGE_COUNT - 1));
                }
            }
            "--stage" => {
                let n: usize = value("--stage")?
                    .parse()
                    .map_err(|e| format!("--stage: {e}"))?;
                if n >= STAGE_COUNT {
                    return Err(format!("--stage: {n} is past the last stage"));
                }
                args.stages_lo = n;
                args.stages_hi = n;
            }
            "--warmup" => {
                args.warmup = value("--warmup")?
                    .parse()
                    .map_err(|e| format!("--warmup: {e}"))?
            }
            "--patience" => {
                args.patience = value("--patience")?
                    .parse()
                    .map_err(|e| format!("--patience: {e}"))?
            }
            "--plateau" => {
                args.plateau = value("--plateau")?
                    .parse()
                    .map_err(|e| format!("--plateau: {e}"))?
            }
            "--plateau-factor" => {
                args.plateau_factor = value("--plateau-factor")?
                    .parse()
                    .map_err(|e| format!("--plateau-factor: {e}"))?
            }
            "--plateau-frac" => {
                args.plateau_frac = value("--plateau-frac")?
                    .parse()
                    .map_err(|e| format!("--plateau-frac: {e}"))?
            }
            "--plateau-min" => {
                args.plateau_min = value("--plateau-min")?
                    .parse()
                    .map_err(|e| format!("--plateau-min: {e}"))?
            }
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
    if args.gpu && args.optimizer == OptimizerKind::Sgd {
        return Err(String::from(
            "--gpu is the Adam path; pass --optimizer adam (the GPU sums a\n             batch's touches of a cell, which sgd's per-example step is not)",
        ));
    }
    if args.optimizer == OptimizerKind::Sgd {
        // One run, one stage, one thread. The stages are independent tables,
        // so a run aimed at several of them is several runs sharing a process
        // -- and it is worse than several processes: the rate schedule, the
        // stall counters and the retirement clock all advance on whichever
        // stage happens to finish an epoch, and every epoch re-reads examples
        // for stages it is not training.
        if args.stages_lo != args.stages_hi {
            return Err(format!(
                "one run trains one stage: pass --stage N (got {}-{}). \
                 To cover several, run one process per stage.",
                args.stages_lo, args.stages_hi
            ));
        }
    }
    Ok(args)
}

/// The records to keep, from the run's flags.
fn filter_of(args: &Args) -> Filter {
    Filter {
        min_ply: args.min_ply,
        max_score_diff: None,
        drop_random: args.drop_random,
        keep_above_ply: args.keep_above_ply,
    }
}

/// Which teacher to read, from the run's flags.
fn policy_of(args: &Args) -> TeacherPolicy {
    TeacherPolicy {
        search_value_to_ply: args.search_value_to_ply,
    }
}

/// Append one file's examples to `out`, returning how many were added.
fn load_file_into(
    path: &Path,
    limit: Option<usize>,
    out: &mut Vec<Example>,
    filter: &Filter,
    policy: &TeacherPolicy,
) -> std::io::Result<usize> {
    load_examples_filtered_into(path, out, limit, filter, policy)
}

/// The dataset, sized but not loaded.
///
/// Counts come from file sizes so shards can be planned without
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
            // Records are fixed-width so the count is exact.
            let est = count_examples_binary(f)?;
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

/// The stage size the default rates were tuned at, used to normalise the
/// rate for stages holding more or fewer examples. A midgame stage of this
/// corpus; nothing about the number matters except that it is fixed, so that
/// a rate means the same amount of movement whichever stage a run is given.
const REFERENCE_EXAMPLES: f64 = 12_500_000.0;

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
    /* The corpus holds both a search value and the game's result per
    record; which one is the teacher, and which records are dropped, has to
    be the same for val as for training or the number measures nothing. */
    let filter = filter_of(&args);
    let policy = policy_of(&args);
    println!("teacher: {}", policy.describe());
    let mut val: Vec<Example> = Vec::new();
    for f in &args.val_files {
        if let Err(e) = load_file_into(f, None, &mut val, &filter, &policy) {
            eprintln!("failed to load val {}: {e}", f.display());
            return ExitCode::FAILURE;
        }
    }
    /* A run aimed at part of the board scores only that part, but the
    held-out set covers all sixty-one stages and every epoch walked all of
    it. On the opening stages that was the whole epoch: fourteen training
    positions against 211k scored ones. */
    if args.stages_lo > 0 || args.stages_hi < STAGE_COUNT - 1 {
        let before = val.len();
        val.retain(|e| {
            let st = Linear::stage(&e.board());
            st >= args.stages_lo && st <= args.stages_hi
        });
        println!(
            "val: {} of {} held-out examples are in stages {}-{}",
            fmt_count(val.len()),
            fmt_count(before),
            args.stages_lo,
            args.stages_hi
        );
    }
    if !args.val_files.is_empty() {
        println!("val: {} held-out examples", fmt_count(val.len()));
    }

    // Linear: resume from an existing weight file when present
    let mut linear = Linear::new(patterns);
    if args.weights_path.exists() {
        match linear.load_weights(&args.weights_path) {
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
            let trainer = Trainer::new(linear, SgdOptimizer::new(args.learning_rate, args.decay));
            run_epochs(trainer, &args, &mut plan, &val, &interrupted)
        }
        OptimizerKind::Adam => {
            println!("optimizer: adam (lr {})", args.learning_rate);
            let trainer = Trainer::new(linear, AdamOptimizer::new(args.learning_rate));
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

/// One stage's trainable parameters: the pattern tables and the disc-count
/// table, as [`Linear::stage_weights`] hands them over.
type StageSnapshot = (Vec<Vec<f32>>, Vec<f32>);

/// Held-out error broken down by game stage: `[count, sum_sq, sum_abs]`.
///
/// The stages are independent tables, so a single pooled number can hide one
/// stage improving while another rots -- they net out. Scoring each stage on
/// its own is what lets the best epoch be chosen per stage.
fn val_by_stage(linear: &Linear, val: &[Example]) -> Vec<[f64; 4]> {
    let mut acc = vec![[0.0f64; 4]; STAGE_COUNT];
    for ex in val {
        let board = ex.board();
        // Prediction minus truth, so a positive mean reads as "this model
        // scores positions high".
        let e = linear.eval(&board) as f64 - ex.score as f64;
        let a = &mut acc[Linear::stage(&board)];
        a[0] += 1.0;
        a[1] += e * e;
        a[2] += e.abs();
        a[3] += e;
    }
    acc
}

/// What a stage's error looks like once its constant part is separated out.
///
/// A stage's mean error is a constant offset on every position it scores.
/// Move ordering never sees it: the moves compared at a node are all one
/// ply deeper, hence all in the same stage, so a per-stage constant cancels
/// in the argmax. What is left -- the spread around that constant -- is the
/// part that can reorder moves.
///
/// Measured, the difference is not academic. Against the same positions the
/// H=64 network scores MAE 5.493 to the linear model's 4.621 and *loses* on
/// that number, yet it carries a +3.75 offset: take the offset out and its
/// spread is 5.356 against 6.131, and it wins 63-28 over the board. Ranked
/// by spread, five evaluators came out in exactly their head-to-head order;
/// ranked by MAE, the strongest of them placed fourth.
///
/// The offset is not free everywhere, which is why it is reported rather
/// than discarded: a midgame score meets an exact endgame value at the
/// solver boundary, aspiration windows carry a bound across depths, and the
/// selective-search margins are calibrated in discs. All three compare
/// numbers that a per-stage offset does move.
fn select_score(which: &str, a: &[f64; 4]) -> f64 {
    let (mse, mae, _, sd) = stage_stats(a);
    match which {
        "mse" => mse,
        "spread" => sd,
        _ => mae,
    }
}

fn stage_stats(a: &[f64; 4]) -> (f64, f64, f64, f64) {
    let n = a[0];
    let mse = a[1] / n;
    let mae = a[2] / n;
    let bias = a[3] / n;
    (mse, mae, bias, (mse - bias * bias).max(0.0).sqrt())
}

/// Mean squared error of `evaluator` over a held-out set, sharded across
/// `threads`. This scores the frozen weights (no updates), so unlike the
/// in-epoch training MSE it is a clean early-stopping signal.
fn val_mse(linear: &Linear, val: &[Example], threads: usize) -> f64 {
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
                                let e = ex.score as f64 - linear.eval(&ex.board()) as f64;
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
                let e = ex.score as f64 - linear.eval(&ex.board()) as f64;
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
        for (i, chunk) in payload.as_chunks::<4>().0.iter().enumerate() {
            self.sum[i] += f32::from_le_bytes(*chunk) as f64;
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

/// The rate for one shard under `--cosine`, or the flat rate without it.
///
/// Stepped per shard rather than per epoch: with seventeen shards an epoch,
/// a per-epoch step would hold one rate for hours and then jump.
fn cosine_lr(args: &Args, epoch: usize, si: usize, n_shards: usize) -> f32 {
    if !args.cosine {
        return args.learning_rate;
    }
    const ETA_MIN: f32 = 1e-8;
    let t = (epoch as f32 - 1.0 + si as f32 / n_shards as f32) / args.epochs as f32;
    ETA_MIN
        + (args.learning_rate - ETA_MIN) * 0.5 * (1.0 + (std::f32::consts::PI * t.min(1.0)).cos())
}

fn run_epochs<O: Optimizer>(
    mut trainer: Trainer<O>,
    args: &Args,
    plan: &mut DataPlan,
    val: &[Example],
    interrupted: &AtomicBool,
) -> ExitCode {
    // SGD always goes through the per-stage path, one thread, one stage.
    //
    // There used to be two: a threaded one that read per-stage rates and
    // per-cell counts, and a single-threaded one that read neither. Anything
    // added to the first was silently inert under `--threads 1` -- per-cell
    // scaling was, and the raw rate it then trained at read as the scaling
    // diverging. Parallelism belongs between processes here anyway: a run
    // aimed at one stage only ever wakes one worker, so eight stages want
    // eight processes, not eight threads.
    let parallel = args.optimizer == OptimizerKind::Sgd;
    let filter = filter_of(args);
    let policy = policy_of(args);
    #[cfg(feature = "gpu")]
    let mut gpu = args.gpu.then(|| {
        kuroobi::linear::gpu::LinearGpu::new(
            &trainer.linear,
            args.minibatch,
            (args.stages_lo, args.stages_hi),
        )
    });
    #[cfg(not(feature = "gpu"))]
    let gpu: Option<()> = None;
    #[cfg(not(feature = "gpu"))]
    if args.gpu {
        eprintln!("--gpu needs a build with `--features gpu`");
        return ExitCode::FAILURE;
    }
    // One buffer for the whole run: after the first shard it already holds
    // enough capacity, so later shards reuse the allocation instead of
    // handing the allocator a multi-gigabyte free/alloc pair every time.
    let mut examples: Vec<Example> = Vec::new();
    // Best val MSE seen and where its weights live (<weights>.best).
    let mut best_val = f64::INFINITY;
    // Best held-out MSE seen per stage, and the weights that scored it.
    // Assembling these at the end gives a model whose every stage is the
    // best that stage reached, which no single epoch need be.
    // Running rate for `--plateau`: held while the model still moves and
    // halved when it stops, rather than decayed on a schedule that has no
    // way of knowing whether there was anything left to learn.
    let mut cur_lr = args.learning_rate;
    let mut stale = 0usize;
    // One rate and one stall counter per stage. A stage that stopped moving
    // has finished with its rate whether or not the others have: they are
    // independent tables fed by different numbers of examples, and a single
    // shared rate is necessarily wrong for most of them.
    let mut stage_lr = vec![args.learning_rate; STAGE_COUNT];
    let mut stage_stale = vec![0usize; STAGE_COUNT];
    // Stages that have stopped improving for `--patience` epochs. They keep
    // the weights that scored their best and take no further updates.
    let mut stage_done = [false; STAGE_COUNT];
    let mut stage_since_best = vec![0usize; STAGE_COUNT];
    // Epochs since the best moved, never reset by anything but a new best.
    // `stage_since_best` forgives a descent; this one does not, so a score
    // that merely wobbles still runs the clock down.
    let mut stage_age = vec![0usize; STAGE_COUNT];
    // `stage_age` when the rate was last halved, so the ceiling below fires
    // once per interval rather than every epoch after it is first crossed.
    let mut stage_age_at_halve = vec![0usize; STAGE_COUNT];
    // Last epoch's score per stage, for deciding whether the rate is still
    // doing work.
    let mut stage_prev = vec![f64::INFINITY; STAGE_COUNT];
    let mut counted_shards: Vec<usize> = Vec::new();
    // Scale each stage's rate by how many examples it gets.
    //
    // One SGD epoch moves a stage by roughly (examples * rate), and the
    // stages differ by orders of magnitude in how many examples they see --
    // a corpus opened with N random plies has nothing before stage N, and
    // the opening stages exist in only a handful of distinct positions. A
    // shared rate therefore overshoots the thin stages and barely moves the
    // thick ones, which is exactly what the logs showed: empties 47 swinging
    // 5.1 -> 8.5 while empties 3 crept by 0.03 an epoch. Normalising by the
    // count equalises the per-epoch movement, so the rates start in the
    // right place instead of being searched for by halving.
    let mut stage_lr_seeded = false;
    // Stage counts accumulated over a whole epoch. One shard cannot show the
    // imbalance -- every shard holds roughly the same mix -- so the rates are
    // seeded at the end of epoch 1, once the full corpus has been counted.
    let mut stage_counts = vec![0usize; STAGE_COUNT];
    let mut stage_best = vec![f64::INFINITY; STAGE_COUNT];
    let mut stage_snap: Vec<Option<StageSnapshot>> = vec![None; STAGE_COUNT];
    // Enter the starting weights as each stage's incumbent.
    //
    // Without this the first epoch wins every stage by default, however much
    // worse it is, and the assembly can only be compared to where it started
    // by pooling the stages back into one number -- which is the comparison
    // per-stage selection exists to get away from. Seeded this way a stage
    // keeps what it had unless an epoch actually beats it, so the assembly is
    // at least as good as the start point *in every stage*, and there is
    // nothing left to check globally.
    if args.per_stage_best && !val.is_empty() {
        let acc = val_by_stage(&trainer.linear, val);
        let mut seeded = 0usize;
        for (st, a) in acc.iter().enumerate() {
            if a[0] == 0.0 {
                continue;
            }
            stage_best[st] = select_score(&args.select_by, a);
            stage_snap[st] = Some(trainer.linear.stage_weights(st));
            seeded += 1;
        }
        println!("baseline: {seeded} stages seeded from the starting weights");
        println!(
            "  stage  empties       n      MSE      MAE     bias   spread  (selecting on {})",
            args.select_by
        );
        for (st, a) in acc.iter().enumerate() {
            if a[0] == 0.0 || st < args.stages_lo || st > args.stages_hi {
                continue;
            }
            let (mse, mae, bias, sd) = stage_stats(a);
            println!(
                "  {:>5}  {:>7}  {:>6}  {:>7.3}  {:>7.3}  {:>+7.3}  {:>7.3}",
                st,
                60 - st,
                a[0] as u64,
                mse,
                mae,
                bias,
                sd
            );
        }
    }
    let best_path = {
        let mut p = args.weights_path.clone().into_os_string();
        p.push(".best");
        PathBuf::from(p)
    };
    let stagebest_path = {
        let mut p = args.weights_path.clone().into_os_string();
        p.push(".stagebest");
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
                match load_file_into(&plan.files[fi], args.limit, &mut examples, &filter, &policy) {
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

            // Counts belong to the data, not to a threading mode: the
            // single-threaded path calls the same `train_shared`. Counting
            // only inside the parallel branch left `--threads 1 --cell-lr`
            // silently training at the raw rate -- which is what turned a
            // rate of 0.1 into NaN and looked like per-cell scaling
            // diverging.
            if args.cell_lr && !counted_shards.contains(&si) {
                trainer
                    .linear
                    .count_appearances(examples.iter().map(|e| e.board()));
                counted_shards.push(si);
                trainer.linear.set_min_appear(args.min_appear);
                for st in args.stages_lo..=args.stages_hi.min(STAGE_COUNT - 1) {
                    if let Some((seen, unseen, q)) = trainer.linear.appearance_spread(st) {
                        println!(
                            "  cell counts stage {st}: {seen} seen, {unseen} unseen, \
                             min {} p1 {} median {} p99 {} max {}",
                            q[0], q[1], q[2], q[3], q[4]
                        );
                    }
                }
            }

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
                // One stage, this thread. `--stage` is required for sgd, so
                // `stages_lo` is the stage and nothing else is touched.
                let st = args.stages_lo;
                if !stage_lr_seeded {
                    stage_counts[st] += examples
                        .iter()
                        .filter(|e| Linear::stage(&e.board()) == st)
                        .count();
                }
                let mut lr = if args.plateau > 0 {
                    stage_lr[st]
                } else {
                    args.learning_rate * args.decay.powi(epoch as i32 - 1)
                };
                if stage_done[st] {
                    lr = 0.0;
                } else if args.warmup > 0 && epoch <= args.warmup {
                    // Climb from a fifth of the rate to all of it.
                    lr *= 0.2 + 0.8 * (epoch as f32 / args.warmup as f32);
                }
                trainer.train_stage_epoch(&examples, st, lr, progress)
            } else if gpu.is_some() {
                #[cfg(feature = "gpu")]
                {
                    let g = gpu.as_mut().unwrap();
                    /* Under --plateau the rate is the stage's own, cut
                    when its held-out score stops moving and restored to
                    its best if asked. A run aimed at one stage has one
                    such rate, which is the whole point of aiming at one
                    stage: the middlegame is still descending long after
                    the endgame has finished. */
                    let mut lr_fn = || {
                        if args.plateau > 0 {
                            stage_lr[args.stages_lo]
                        } else {
                            cosine_lr(args, epoch, si, shards.len())
                        }
                    };
                    let sq = g.train_shard(&examples, &mut lr_fn);
                    // The evaluator's own tables are stale until this; val
                    // and the save both read them, so pull them back every
                    // shard.
                    g.download(&mut trainer.linear);
                    let mut st = EpochStats::default();
                    // Eight rows per position, so the row count is what the
                    // sum of squares was taken over.
                    st.loss_sum[0] = sq;
                    st.samples[0] = (examples.len() * kuroobi::linear::gpu::FORMS) as u64;
                    st
                }
                #[cfg(not(feature = "gpu"))]
                unreachable!("the flag is refused without the feature")
            } else {
                /* Anneal across the whole run, stepped per shard rather
                than per epoch: with fifteen shards an epoch, a per-epoch
                step would hold one rate for two hours and then jump. The
                floor matches nnue_train's so the two sweeps end alike. */
                if args.cosine {
                    trainer
                        .optimizer
                        .set_lr(cosine_lr(args, epoch, si, shards.len()));
                }
                // `train_pass`, not `train_stage_epoch`: the lr schedule
                // advances once per epoch, not once per shard.
                trainer.train_pass(&examples, progress)
            };
            stats.add(&shard_stats);
            done += examples.len();

            if interrupted.load(Ordering::SeqCst) {
                eprint!("\r{:width$}\r", "", width = 90);
                if let Err(e) = trainer.linear.save_weights(&args.weights_path) {
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
            val_mse(&trainer.linear, val, args.threads)
        };
        let is_best = vm < best_val; // false when vm is NaN (no val set)
        if !stage_lr_seeded && args.plateau > 0 {
            // Scale the rate against a fixed reference, not against the other
            // stages in this run -- there are none. One SGD epoch moves a
            // stage by roughly (examples * rate), and the stages differ by
            // several times in how many examples they hold: measured here,
            // stage 19 has 12.5M and stage 51 has 44M. At one shared rate the
            // endgame stages take three times the step, which is what sent
            // empties 15 and 14 from 6.192 and 6.087 up to 6.300 and 6.361 in
            // their first epoch while empties 21 came down.
            //
            // This used to normalise against the median stage of the run,
            // which said something only because a run covered many stages.
            // One stage per run makes that median the stage itself and the
            // whole step a no-op.
            let st = args.stages_lo;
            let c = stage_counts[st];
            if c > 0 {
                // Clamp the ratio: a stage seen a hundred times must not be
                // handed a rate a thousand times the base.
                let ratio = (REFERENCE_EXAMPLES / c as f64).clamp(0.05, 20.0);
                stage_lr[st] = args.learning_rate * ratio as f32;
                println!(
                    "stage {st} lr seeded from {c} examples: {} x{:.2} -> {}",
                    args.learning_rate, ratio, stage_lr[st]
                );
            }
            stage_lr_seeded = true;
        }
        let mut improved = 0usize;
        let mut stages_seen = 0usize;
        if args.per_stage_best && !val.is_empty() {
            let acc = val_by_stage(&trainer.linear, val);
            println!("  stage  empties       n      MSE      MAE     bias   spread");
            for (st, a) in acc.iter().enumerate() {
                if a[0] == 0.0 {
                    continue;
                }
                // Only the stages this run is training are worth printing or
                // counting: the rest are pinned to the weights they loaded
                // with and cannot move.
                let reported = st >= args.stages_lo && st <= args.stages_hi;
                if reported {
                    stages_seen += 1;
                }
                let (mse, mae, bias, sd) = stage_stats(a);
                // The default is MAE, not MSE. Squared error is dominated by
                // the thinly-sampled opening stages (empties 40-50 sit at
                // 70-90 where the endgame sits at 8-40), so a lucky epoch
                // there outweighs a real loss elsewhere: measured over 11
                // epochs the MSE-selected assembly scored *worse* on
                // stage-balanced MAE than a single epoch of it did.
                // `--select-by spread` is for a model that carries an offset;
                // see `stage_stats`.
                let score = select_score(&args.select_by, a);
                let mut restored = false;
                let better = score < stage_best[st];
                if better {
                    stage_best[st] = score;
                    stage_snap[st] = Some(trainer.linear.stage_weights(st));
                    if reported {
                        improved += 1;
                    }
                    stage_stale[st] = 0;
                    stage_since_best[st] = 0;
                    stage_age[st] = 0;
                    stage_age_at_halve[st] = 0;
                } else {
                    stage_since_best[st] += 1;
                    stage_age[st] += 1;
                    // A stage still coming down has not finished, whatever
                    // the count says. Retiring on "epochs since the best" cuts
                    // off a descent that has not reached the incumbent yet:
                    // measured here, a stage went 6.995 -> 6.085 -> 5.929
                    // against an incumbent of 5.634, closing to 0.295 with
                    // the clock about to run out. Only a rise is evidence
                    // that the stage is done.
                    //
                    // But the reset cannot be unconditional. A score that
                    // oscillates falls against the previous epoch about half
                    // the time, so an unconditional reset stops the clock
                    // from ever running: measured here, eight stages sat past
                    // 30 epochs with `--patience 6` and a best that had not
                    // moved in twenty, because every other epoch happened to
                    // tick down. Credit a descent only while the stage is
                    // still above its own best -- that is the approach the
                    // reset exists for. Once it has been there, oscillating
                    // around it is not progress.
                    if score < stage_prev[st] && score > stage_best[st] {
                        stage_since_best[st] = 0;
                    }
                    // And a hard ceiling regardless: however the score wobbles,
                    // a best that has not moved in this many epochs is done.
                    let stalled_out = stage_age[st] >= args.patience * 3;
                    // Still descending? Then the rate is doing its job.
                    //
                    // Halving on "did not beat the incumbent" cuts the rate
                    // in the middle of a descent: measured here, a stage went
                    // 7.787 -> 5.797 in one epoch -- two discs of progress --
                    // and was halved anyway because it had not yet passed a
                    // start point that was already good, then bounced back to
                    // 7.811. The rate is only wrong when the step stops
                    // buying anything, which is what a rise over the previous
                    // epoch shows.
                    // An identical score is not evidence either way. It
                    // means the epoch moved the weights nowhere the held-out
                    // set can see -- not that the rate is too large. Counting
                    // it as a rise spends a halving on nothing; the stale
                    // clock simply holds.
                    let flat = score == stage_prev[st];
                    let descending = score < stage_prev[st];
                    if reported
                        && args.patience > 0
                        && (stage_since_best[st] >= args.patience || stalled_out)
                        && !stage_done[st]
                    {
                        // Put the stage back on the weights that scored its
                        // best before retiring it, so the live model and the
                        // assembly agree from here on.
                        if let Some((w, num)) = &stage_snap[st] {
                            trainer.linear.set_stage_weights(st, w, num);
                        }
                        stage_done[st] = true;
                        println!("  stage {st} (空き {}) 収束、学習終了", 60 - st);
                    }
                    if args.plateau > 0 {
                        if descending {
                            stage_stale[st] = 0;
                        } else if !flat {
                            stage_stale[st] += 1;
                            // The descent reset above is as blind to
                            // oscillation as the retirement clock was: a
                            // score that wobbles falls against the previous
                            // epoch about every other epoch, so stale never
                            // reaches the threshold and the rate never moves.
                            // Measured here, empties 11 and 9 sat five epochs
                            // at the rate that put them 0.11 above their own
                            // best, alternating rise and fall the whole time.
                            // So halve on the same unforgiving clock too:
                            // whatever the score does between epochs, a best
                            // that has not moved this long says the step is
                            // too large.
                            let ceiling = args.plateau * 3;
                            let overdue = stage_age[st] >= stage_age_at_halve[st] + ceiling;
                            if stage_stale[st] >= args.plateau || overdue {
                                stage_lr[st] *= args.plateau_factor;
                                stage_stale[st] = 0;
                                stage_age_at_halve[st] = stage_age[st];
                                if args.restore_on_halve {
                                    if let Some((w, num)) = &stage_snap[st] {
                                        trainer.linear.set_stage_weights(st, w, num);
                                        restored = true;
                                    }
                                }
                            }
                        }
                    }
                }
                // After a restore the next epoch starts from the best, so
                // that is the score it has to be compared against -- not the
                // one belonging to the weights just thrown away.
                stage_prev[st] = if restored { stage_best[st] } else { score };
                if reported {
                    println!(
                        "  {:>5}  {:>7}  {:>6}  {:>7.3}  {:>7.3}  {:>+7.3}  {:>7.3}{}",
                        st,
                        60 - st,
                        a[0] as u64,
                        mse,
                        mae,
                        bias,
                        sd,
                        if better { " *" } else { "" }
                    );
                }
            }
            if args.plateau > 0 && args.lr_smooth {
                // Neighbouring stages differ by one ply, so the rate that
                // suits one should nearly suit the next -- and the seeded
                // rates are smooth by construction. Independent halving
                // breaks that: one epoch's worth of val noise decides
                // whether a stage halves, and a single such decision leaves
                // it at twice or half its neighbour for good. Averaging in
                // log space keeps the ladder continuous while still letting
                // a genuinely different stage drift away over several
                // epochs.
                let src = stage_lr.clone();
                for st in 0..STAGE_COUNT {
                    if !stage_best[st].is_finite() {
                        continue;
                    }
                    let mut acc = 0.0f64;
                    let mut n = 0.0f64;
                    for d in [-1i64, 0, 1] {
                        let j = st as i64 + d;
                        if j < 0 || j as usize >= STAGE_COUNT {
                            continue;
                        }
                        let j = j as usize;
                        if !stage_best[j].is_finite() || src[j] <= 0.0 {
                            continue;
                        }
                        // Weight the stage itself double so smoothing pulls
                        // towards the neighbours without erasing the stage's
                        // own decisions.
                        let w = if d == 0 { 2.0 } else { 1.0 };
                        acc += w * (src[j] as f64).ln();
                        n += w;
                    }
                    if n > 0.0 {
                        stage_lr[st] = (acc / n).exp() as f32;
                    }
                }
            }
            println!(
                "  per-stage: {improved}/{stages_seen} stages beat their incumbent this epoch"
            );
            if args.plateau > 0 {
                println!("  per-stage lr");
                for (st, best) in stage_best.iter().enumerate() {
                    if best.is_finite() && st >= args.stages_lo && st <= args.stages_hi {
                        println!(
                            "  lr {:>5} {:>7} {:.9} stale {}",
                            st,
                            60 - st,
                            stage_lr[st],
                            stage_stale[st]
                        );
                    }
                }
            }
            // Every epoch, improvement or not. Gating this on `improved > 0`
            // means a run that never beats its start point leaves no
            // `.stagebest` at all, and `stage_merge` then silently takes
            // fewer inputs than it was given: measured here, four of the
            // fourteen files a sweep should have produced were simply
            // absent. Nothing is lost by writing it -- the incumbent is
            // seeded as each stage's best before the first epoch -- but the
            // file's absence is indistinguishable from a crash.
            {
                // Write the assembly now rather than only at the end: a run
                // this long is normally stopped by hand, and a file that only
                // appears on a clean exit is a file that never appears. The
                // live weights are put back afterwards, so training carries
                // on from where it was and not from an assembly no epoch
                // produced.
                let live: Vec<StageSnapshot> = (0..STAGE_COUNT)
                    .map(|st| trainer.linear.stage_weights(st))
                    .collect();
                for (st, snap) in stage_snap.iter().enumerate() {
                    if let Some((w, num)) = snap {
                        trainer.linear.set_stage_weights(st, w, num);
                    }
                }
                if let Err(e) = trainer.linear.save_weights(&stagebest_path) {
                    eprintln!("failed to save {}: {e}", stagebest_path.display());
                }
                for (st, (w, num)) in live.iter().enumerate() {
                    trainer.linear.set_stage_weights(st, w, num);
                }
            }
        }
        if args.patience > 0 {
            let live = (args.stages_lo..=args.stages_hi)
                .filter(|&st| stage_best[st].is_finite() && !stage_done[st])
                .count();
            if live == 0 {
                println!("  全ステージ収束、終了");
                break;
            }
        }
        if args.plateau > 0 {
            // "Did this epoch help" is a per-stage question when the stages
            // are kept separately: an epoch that moved even one of them left
            // the assembled model better than it found it. Only when nothing
            // moved at all has this rate finished its work.
            let progressed = if args.per_stage_best && !val.is_empty() {
                // A handful of stages still creeping is not the model
                // learning -- measured here, the count fell 50, 45, 14, 10,
                // 9, 4 while the training MSE never left 33.37..33.51 and
                // the val bounced between 47.8 and 50.8. That is a rate too
                // large for the surface, carried along by whichever few
                // stages happened to land well. Require a real share of the
                // model to move before calling the epoch progress.
                // Both conditions, because either alone misreads this run.
                // Stage count alone: a band that collapsed one epoch and
                // recovered the next counts as "improved" and resets the
                // stall, so the rate never falls (11 epochs, 0 reductions).
                // Val alone: it is one pooled number over stages that move
                // independently, which is what per-stage selection exists to
                // avoid trusting.
                (improved as f64) >= args.plateau_frac * (stages_seen as f64) && is_best
            } else {
                is_best
            };
            if progressed {
                stale = 0;
            } else {
                stale += 1;
                if stale >= args.plateau {
                    cur_lr *= args.plateau_factor;
                    stale = 0;
                    println!(
                        "  {} epochs without progress -> lr {cur_lr:.8}",
                        args.plateau
                    );
                    if cur_lr < args.plateau_min {
                        println!("  rate floor reached; stopping");
                        break;
                    }
                }
            }
        }
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
        if let Err(e) = trainer.linear.save_weights(&args.weights_path) {
            eprintln!("failed to save {}: {e}", args.weights_path.display());
            return ExitCode::FAILURE;
        }
        // Keep the best-by-val weights separately: the last epoch is not
        // necessarily the best, and this run overwrites `weights_path`.
        if is_best {
            best_val = vm;
            if let Err(e) = trainer.linear.save_weights(&best_path) {
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

    // Assemble the per-stage best into one model. Done last, and only into
    // the extra file, so the run's own weights are left as training ended
    // them -- restoring 61 stages in place would make a resumed run continue
    // from a model no epoch ever produced.
    if args.per_stage_best && stage_snap.iter().any(|x| x.is_some()) {
        let mut kept = 0usize;
        for (st, snap) in stage_snap.iter().enumerate() {
            if let Some((w, num)) = snap {
                trainer.linear.set_stage_weights(st, w, num);
                kept += 1;
            }
        }
        let p = stagebest_path.clone();
        match trainer.linear.save_weights(&p) {
            Ok(()) => {
                let pooled: f64 = stage_best
                    .iter()
                    .zip(0..STAGE_COUNT)
                    .filter(|(m, _)| m.is_finite())
                    .map(|(m, _)| *m)
                    .sum::<f64>()
                    / stage_best.iter().filter(|m| m.is_finite()).count().max(1) as f64;
                println!(
                    "per-stage best: {kept} stages assembled (mean of per-stage best MSE {pooled:.4}) saved to {}",
                    p.display()
                );
            }
            Err(e) => {
                eprintln!("failed to save {}: {e}", p.display());
                return ExitCode::FAILURE;
            }
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
    let prev = unsafe { libc::signal(libc::SIGINT, trampoline as *const () as libc::sighandler_t) };
    if prev == libc::SIG_ERR {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
