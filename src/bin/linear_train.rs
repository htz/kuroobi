//! Trainer for the linear pattern evaluator ([`kuroobi::linear`]).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use kuroobi::linear::{AdamOptimizer, Linear, Optimizer, SgdOptimizer, STAGE_COUNT};
use kuroobi::pattern::LINEAR_PATTERNS;
use kuroobi::record::{Filter, TeacherPolicy};
use kuroobi::trainer::{
    count_examples_binary, load_examples_filtered_into, EpochStats, Example, Trainer,
};

const DEFAULT_MAX_EXAMPLES: usize = 64_000_000;

const EXAMPLE_BYTES: usize = std::mem::size_of::<Example>();

struct Args {
    epochs: usize,
    learning_rate: f32,
    decay: f32,
    optimizer: OptimizerKind,
    weights_path: PathBuf,
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
            "--cosine" => args.cosine = true,
            "--gpu" => args.gpu = true,
            "--minibatch" => {
                args.minibatch = value("--minibatch")?
                    .parse()
                    .map_err(|e| format!("--minibatch: {e}"))?
            }
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
    if args.optimizer == OptimizerKind::Sgd && args.stages_lo != args.stages_hi {
        return Err(format!(
            "one run trains one stage: pass --stage N (got {}-{}). \
             To cover several, run one process per stage.",
            args.stages_lo, args.stages_hi
        ));
    }
    Ok(args)
}

fn filter_of(args: &Args) -> Filter {
    Filter {
        min_ply: args.min_ply,
        max_score_diff: None,
        drop_random: args.drop_random,
        keep_above_ply: args.keep_above_ply,
    }
}

fn policy_of(args: &Args) -> TeacherPolicy {
    TeacherPolicy {
        search_value_to_ply: args.search_value_to_ply,
    }
}

fn load_file_into(
    path: &Path,
    limit: Option<usize>,
    out: &mut Vec<Example>,
    filter: &Filter,
    policy: &TeacherPolicy,
) -> std::io::Result<usize> {
    load_examples_filtered_into(path, out, limit, filter, policy)
}

struct DataPlan {
    files: Vec<PathBuf>,
    counts: Vec<usize>,
}

struct Shard {
    files: Vec<usize>,
    examples: usize,
}

impl DataPlan {
    fn new(files: Vec<PathBuf>, limit: Option<usize>) -> std::io::Result<DataPlan> {
        let mut counts = Vec::with_capacity(files.len());
        for f in &files {
            let est = count_examples_binary(f)?;
            counts.push(limit.map_or(est, |l| est.min(l)));
        }
        Ok(DataPlan { files, counts })
    }

    fn total(&self) -> usize {
        self.counts.iter().sum()
    }

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

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

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

const REFERENCE_EXAMPLES: f64 = 12_500_000.0;

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    let patterns = LINEAR_PATTERNS;

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

fn draw_progress(
    epoch: usize,
    epochs: usize,
    shard: (usize, usize),
    done: usize,
    total: usize,
    started: &Instant,
) {
    const BAR_WIDTH: usize = 24;

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

type StageSnapshot = (Vec<Vec<f32>>, Vec<f32>);

fn val_by_stage(linear: &Linear, val: &[Example]) -> Vec<[f64; 4]> {
    let mut acc = vec![[0.0f64; 4]; STAGE_COUNT];
    for ex in val {
        let board = ex.board();
        let e = linear.eval(&board) as f64 - ex.score as f64;
        let a = &mut acc[Linear::stage(&board)];
        a[0] += 1.0;
        a[1] += e * e;
        a[2] += e.abs();
        a[3] += e;
    }
    acc
}

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

struct SwaAccumulator {
    header: Vec<u8>,
    sum: Vec<f64>,
    count: u64,
    out: PathBuf,
}

impl SwaAccumulator {
    fn fold(&mut self, weights_path: &Path) -> std::io::Result<()> {
        let bytes = std::fs::read(weights_path)?;
        if self.header.is_empty() {
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

fn cosine_lr(args: &Args, epoch: usize, si: usize, n_shards: usize) -> f32 {
    if !args.cosine {
        return args.learning_rate;
    }
    const ETA_MIN: f32 = 1e-8;
    let t = (epoch as f32 - 1.0 + si as f32 / n_shards as f32) / args.epochs as f32;
    ETA_MIN
        + (args.learning_rate - ETA_MIN) * 0.5 * (1.0 + (std::f32::consts::PI * t.min(1.0)).cos())
}
struct StageState {
    lr: Vec<f32>,
    stale: Vec<usize>,
    done: [bool; STAGE_COUNT],
    since_best: Vec<usize>,
    age: Vec<usize>,
    age_at_halve: Vec<usize>,
    prev: Vec<f64>,
    counts: Vec<usize>,
    best: Vec<f64>,
    snap: Vec<Option<StageSnapshot>>,
}

impl StageState {
    fn new(lr: f32) -> Self {
        StageState {
            lr: vec![lr; STAGE_COUNT],
            stale: vec![0; STAGE_COUNT],
            done: [false; STAGE_COUNT],
            since_best: vec![0; STAGE_COUNT],
            age: vec![0; STAGE_COUNT],
            age_at_halve: vec![0; STAGE_COUNT],
            prev: vec![f64::INFINITY; STAGE_COUNT],
            counts: vec![0; STAGE_COUNT],
            best: vec![f64::INFINITY; STAGE_COUNT],
            snap: vec![None; STAGE_COUNT],
        }
    }
}
fn load_shard(
    shard: &Shard,
    plan: &mut DataPlan,
    args: &Args,
    filter: &Filter,
    policy: &TeacherPolicy,
    examples: &mut Vec<Example>,
) -> Result<(), PathBuf> {
    examples.clear();
    for &fi in &shard.files {
        let before = examples.len();
        let path = &plan.files[fi];
        match load_file_into(path, args.limit, examples, filter, policy) {
            Ok(_) => plan.counts[fi] = examples.len() - before,
            Err(e) => {
                eprintln!("{e}");
                return Err(path.clone());
            }
        }
    }
    Ok(())
}

fn run_epochs<O: Optimizer>(
    mut trainer: Trainer<O>,
    args: &Args,
    plan: &mut DataPlan,
    val: &[Example],
    interrupted: &AtomicBool,
) -> ExitCode {
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
    let mut examples: Vec<Example> = Vec::new();
    let mut best_val = f64::INFINITY;
    let mut cur_lr = args.learning_rate;
    let mut stale = 0usize;
    let mut stage = StageState::new(args.learning_rate);
    let mut counted_shards: Vec<usize> = Vec::new();
    let mut stage_lr_seeded = false;
    if args.per_stage_best && !val.is_empty() {
        seed_stage_best(&trainer.linear, args, val, &mut stage);
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

        let mut order: Vec<usize> = (0..plan.files.len()).collect();
        if epoch > 1 {
            Rng::new(epoch as u64).shuffle(&mut order);
        }
        let shards = plan.shards(&order, args.max_examples);
        let epoch_total: usize = shards.iter().map(|s| s.examples).sum();

        let mut stats = EpochStats::default();
        let mut done = 0usize;
        for (si, shard) in shards.iter().enumerate() {
            if let Err(path) = load_shard(shard, plan, args, &filter, &policy, &mut examples) {
                eprintln!("\nfailed to load {}", path.display());
                return ExitCode::FAILURE;
            }
            Rng::new((epoch as u64) << 32 | si as u64).shuffle(&mut examples);

            if args.cell_lr && !counted_shards.contains(&si) {
                counted_shards.push(si);
                count_cells(&mut trainer, args, &examples);
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
                let st = args.stages_lo;
                if !stage_lr_seeded {
                    stage.counts[st] += examples
                        .iter()
                        .filter(|e| Linear::stage(&e.board()) == st)
                        .count();
                }
                let lr = stage_lr(args, &stage, epoch, st);
                trainer.train_stage_epoch(&examples, st, lr, progress)
            } else if gpu.is_some() {
                #[cfg(feature = "gpu")]
                {
                    let g = gpu.as_mut().unwrap();
                    let mut lr_fn = || {
                        if args.plateau > 0 {
                            stage.lr[args.stages_lo]
                        } else {
                            cosine_lr(args, epoch, si, shards.len())
                        }
                    };
                    let sq = g.train_shard(&examples, &mut lr_fn);
                    g.download(&mut trainer.linear);
                    let mut st = EpochStats::default();
                    st.loss_sum[0] = sq;
                    st.samples[0] = (examples.len() * kuroobi::linear::gpu::FORMS) as u64;
                    st
                }
                #[cfg(not(feature = "gpu"))]
                unreachable!("the flag is refused without the feature")
            } else {
                if args.cosine {
                    trainer
                        .optimizer
                        .set_lr(cosine_lr(args, epoch, si, shards.len()));
                }
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

        eprint!("\r{:width$}\r", "", width = 90);
        let elapsed = t.elapsed().as_secs_f32();
        let per_sec = done as f32 / elapsed;
        let vm = if val.is_empty() {
            f64::NAN
        } else {
            val_mse(&trainer.linear, val, args.threads)
        };
        let is_best = vm < best_val; // false when vm is NaN (no val set)
        if !stage_lr_seeded && args.plateau > 0 {
            let st = args.stages_lo;
            let c = stage.counts[st];
            if c > 0 {
                let ratio = (REFERENCE_EXAMPLES / c as f64).clamp(0.05, 20.0);
                stage.lr[st] = args.learning_rate * ratio as f32;
                println!(
                    "stage {st} lr seeded from {c} examples: {} x{:.2} -> {}",
                    args.learning_rate, ratio, stage.lr[st]
                );
            }
            stage_lr_seeded = true;
        }
        let (improved, stages_seen) = if args.per_stage_best && !val.is_empty() {
            per_stage_epoch(&mut trainer, args, val, &mut stage, &stagebest_path)
        } else {
            (0, 0)
        };
        if args.patience > 0 {
            let live = (args.stages_lo..=args.stages_hi)
                .filter(|&st| stage.best[st].is_finite() && !stage.done[st])
                .count();
            if live == 0 {
                println!("  全ステージ収束、終了");
                break;
            }
        }
        if args.plateau > 0 {
            let progressed = if args.per_stage_best && !val.is_empty() {
                (improved as f64) >= args.plateau_frac * (stages_seen as f64) && is_best
            } else {
                is_best
            };
            if global_plateau(args, &mut cur_lr, &mut stale, progressed) {
                break;
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

        if persist_epoch(
            &trainer,
            args,
            epoch,
            &stats,
            is_best.then_some(best_path.as_path()),
            &mut swa,
        )
        .is_err()
        {
            return ExitCode::FAILURE;
        }
        if is_best {
            best_val = vm;
        }
        if interrupted.load(Ordering::SeqCst) {
            report_swa(&swa);
            println!(
                "stopped after epoch {epoch}; weights saved to {}",
                args.weights_path.display()
            );
            return ExitCode::SUCCESS;
        }
    }

    if args.per_stage_best
        && stage.snap.iter().any(|x| x.is_some())
        && !write_stage_best(&mut trainer.linear, &stage, &stagebest_path)
    {
        return ExitCode::FAILURE;
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

fn ctrlc_handler<F: FnMut() + Send + 'static>(handler: F) -> std::io::Result<()> {
    use std::sync::Mutex;
    static HANDLER: Mutex<Option<Box<dyn FnMut() + Send>>> = Mutex::new(None);

    extern "C" fn trampoline(_: libc::c_int) {
        if let Ok(mut guard) = HANDLER.try_lock() {
            if let Some(h) = guard.as_mut() {
                h();
            }
        }
    }

    *HANDLER.lock().unwrap() = Some(Box::new(handler));
    let prev = unsafe { libc::signal(libc::SIGINT, trampoline as *const () as libc::sighandler_t) };
    if prev == libc::SIG_ERR {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn seed_stage_best(lin: &Linear, args: &Args, val: &[Example], stage: &mut StageState) {
    let acc = val_by_stage(lin, val);
    let mut seeded = 0usize;
    for (st, a) in acc.iter().enumerate() {
        if a[0] == 0.0 {
            continue;
        }
        stage.best[st] = select_score(&args.select_by, a);
        stage.snap[st] = Some(lin.stage_weights(st));
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

fn count_cells<O: Optimizer>(trainer: &mut Trainer<O>, args: &Args, examples: &[Example]) {
    trainer
        .linear
        .count_appearances(examples.iter().map(|e| e.board()));
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

fn stage_lr(args: &Args, stage: &StageState, epoch: usize, st: usize) -> f32 {
    if stage.done[st] {
        return 0.0;
    }
    let mut lr = if args.plateau > 0 {
        stage.lr[st]
    } else {
        args.learning_rate * args.decay.powi(epoch as i32 - 1)
    };
    if args.warmup > 0 && epoch <= args.warmup {
        lr *= 0.2 + 0.8 * (epoch as f32 / args.warmup as f32);
    }
    lr
}

/// (stages that beat their incumbent, stages in the reported range)
fn per_stage_epoch<O: Optimizer>(
    trainer: &mut Trainer<O>,
    args: &Args,
    val: &[Example],
    stage: &mut StageState,
    stagebest_path: &Path,
) -> (usize, usize) {
    let mut improved = 0usize;
    let mut stages_seen = 0usize;
    let acc = val_by_stage(&trainer.linear, val);
    println!("  stage  empties       n      MSE      MAE     bias   spread");
    for (st, a) in acc.iter().enumerate() {
        if a[0] == 0.0 {
            continue;
        }
        let reported = st >= args.stages_lo && st <= args.stages_hi;
        if reported {
            stages_seen += 1;
        }
        let (mse, mae, bias, sd) = stage_stats(a);
        let score = select_score(&args.select_by, a);
        let mut restored = false;
        let better = score < stage.best[st];
        if better {
            stage.best[st] = score;
            stage.snap[st] = Some(trainer.linear.stage_weights(st));
            if reported {
                improved += 1;
            }
            stage.stale[st] = 0;
            stage.since_best[st] = 0;
            stage.age[st] = 0;
            stage.age_at_halve[st] = 0;
        } else {
            stage.since_best[st] += 1;
            stage.age[st] += 1;
            if score < stage.prev[st] && score > stage.best[st] {
                stage.since_best[st] = 0;
            }
            let stalled_out = stage.age[st] >= args.patience * 3;
            let flat = score == stage.prev[st];
            let descending = score < stage.prev[st];
            if reported
                && args.patience > 0
                && (stage.since_best[st] >= args.patience || stalled_out)
                && !stage.done[st]
            {
                if let Some((w, num)) = &stage.snap[st] {
                    trainer.linear.set_stage_weights(st, w, num);
                }
                stage.done[st] = true;
                println!("  stage {st} (空き {}) 収束、学習終了", 60 - st);
            }
            if args.plateau > 0 {
                if descending {
                    stage.stale[st] = 0;
                } else if !flat {
                    stage.stale[st] += 1;
                    let ceiling = args.plateau * 3;
                    let overdue = stage.age[st] >= stage.age_at_halve[st] + ceiling;
                    if stage.stale[st] >= args.plateau || overdue {
                        stage.lr[st] *= args.plateau_factor;
                        stage.stale[st] = 0;
                        stage.age_at_halve[st] = stage.age[st];
                        if args.restore_on_halve {
                            if let Some((w, num)) = &stage.snap[st] {
                                trainer.linear.set_stage_weights(st, w, num);
                                restored = true;
                            }
                        }
                    }
                }
            }
        }
        stage.prev[st] = if restored { stage.best[st] } else { score };
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
        let src = stage.lr.clone();
        for st in 0..STAGE_COUNT {
            if !stage.best[st].is_finite() {
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
                if !stage.best[j].is_finite() || src[j] <= 0.0 {
                    continue;
                }
                let w = if d == 0 { 2.0 } else { 1.0 };
                acc += w * (src[j] as f64).ln();
                n += w;
            }
            if n > 0.0 {
                stage.lr[st] = (acc / n).exp() as f32;
            }
        }
    }
    println!("  per-stage: {improved}/{stages_seen} stages beat their incumbent this epoch");
    if args.plateau > 0 {
        println!("  per-stage lr");
        for (st, best) in stage.best.iter().enumerate() {
            if best.is_finite() && st >= args.stages_lo && st <= args.stages_hi {
                println!(
                    "  lr {:>5} {:>7} {:.9} stale {}",
                    st,
                    60 - st,
                    stage.lr[st],
                    stage.stale[st]
                );
            }
        }
    }
    {
        let live: Vec<StageSnapshot> = (0..STAGE_COUNT)
            .map(|st| trainer.linear.stage_weights(st))
            .collect();
        for (st, snap) in stage.snap.iter().enumerate() {
            if let Some((w, num)) = snap {
                trainer.linear.set_stage_weights(st, w, num);
            }
        }
        if let Err(e) = trainer.linear.save_weights(stagebest_path) {
            eprintln!("failed to save {}: {e}", stagebest_path.display());
        }
        for (st, (w, num)) in live.iter().enumerate() {
            trainer.linear.set_stage_weights(st, w, num);
        }
    }
    (improved, stages_seen)
}

/// True once the rate has reached its floor and training should stop.
fn global_plateau(args: &Args, cur_lr: &mut f32, stale: &mut usize, progressed: bool) -> bool {
    if progressed {
        *stale = 0;
        return false;
    }
    *stale += 1;
    if *stale < args.plateau {
        return false;
    }
    *cur_lr *= args.plateau_factor;
    *stale = 0;
    println!(
        "  {} epochs without progress -> lr {:.8}",
        args.plateau, *cur_lr
    );
    if *cur_lr < args.plateau_min {
        println!("  rate floor reached; stopping");
        return true;
    }
    false
}

fn persist_epoch<O: Optimizer>(
    trainer: &Trainer<O>,
    args: &Args,
    epoch: usize,
    stats: &EpochStats,
    best: Option<&Path>,
    swa: &mut Option<SwaAccumulator>,
) -> Result<(), ()> {
    if let Some(log) = &args.log_path {
        if let Err(e) = append_log(log, epoch, stats) {
            eprintln!("failed to write log {}: {e}", log.display());
            return Err(());
        }
    }
    if let Err(e) = trainer.linear.save_weights(&args.weights_path) {
        eprintln!("failed to save {}: {e}", args.weights_path.display());
        return Err(());
    }
    if let Some(p) = best {
        if let Err(e) = trainer.linear.save_weights(p) {
            eprintln!("failed to save {}: {e}", p.display());
            return Err(());
        }
    }
    if let Some(acc) = swa {
        if epoch >= args.swa_start {
            if let Err(e) = acc
                .fold(&args.weights_path)
                .and_then(|_| acc.write().map(|_| ()))
            {
                eprintln!("failed to update SWA: {e}");
                return Err(());
            }
        }
    }
    Ok(())
}

fn report_swa(swa: &Option<SwaAccumulator>) {
    if let Some(acc) = swa {
        if acc.count > 0 {
            println!(
                "SWA mean of {} epochs saved to {}",
                acc.count,
                acc.out.display()
            );
        }
    }
}

fn write_stage_best(lin: &mut Linear, stage: &StageState, path: &Path) -> bool {
    let mut kept = 0usize;
    for (st, snap) in stage.snap.iter().enumerate() {
        if let Some((w, num)) = snap {
            lin.set_stage_weights(st, w, num);
            kept += 1;
        }
    }
    if let Err(e) = lin.save_weights(path) {
        eprintln!("failed to save {}: {e}", path.display());
        return false;
    }
    let live = stage.best.iter().filter(|m| m.is_finite());
    let n = live.clone().count().max(1);
    let pooled: f64 = live.sum::<f64>() / n as f64;
    println!(
        "per-stage best: {kept} stages assembled \
         (mean of per-stage best MSE {pooled:.4}) saved to {}",
        path.display()
    );
    true
}
