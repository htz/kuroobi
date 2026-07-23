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
//!   --log <path>      Append per-epoch stage losses as CSV

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use kuroobi::evaluator::{AdamOptimizer, Evaluator, Optimizer, SgdOptimizer, STAGE_COUNT};
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::trainer::{load_examples_binary, load_examples_text, Example, Trainer};

struct Args {
    epochs: usize,
    learning_rate: f32,
    decay: f32,
    optimizer: OptimizerKind,
    weights_path: PathBuf,
    patterns: &'static str,
    limit: Option<usize>,
    log_path: Option<PathBuf>,
    threads: usize,
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
  --log <path>      Append per-epoch stage losses as CSV
  -h, --help        Show this help";

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        epochs: 10,
        learning_rate: f32::NAN, // resolved after optimizer choice
        decay: 0.95,
        optimizer: OptimizerKind::Sgd,
        threads: 1,
        weights_path: PathBuf::from("weights.bin"),
        patterns: "egaroucid",
        limit: None,
        log_path: None,
        data_files: Vec::new(),
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match arg.as_str() {
            "--epochs" => args.epochs = value("--epochs")?.parse().map_err(|e| format!("--epochs: {e}"))?,
            "--lr" => args.learning_rate = value("--lr")?.parse().map_err(|e| format!("--lr: {e}"))?,
            "--decay" => args.decay = value("--decay")?.parse().map_err(|e| format!("--decay: {e}"))?,
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
            "--limit" => args.limit = Some(value("--limit")?.parse().map_err(|e| format!("--limit: {e}"))?),
            "--log" => args.log_path = Some(PathBuf::from(value("--log")?)),
            "-h" | "--help" => return Err(USAGE.to_string()),
            other if other.starts_with('-') => return Err(format!("unknown option: {other}\n\n{USAGE}")),
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

fn load_file(path: &Path, limit: Option<usize>) -> std::io::Result<Vec<Example>> {
    let mut examples = if path.extension().is_some_and(|e| e == "txt") {
        load_examples_text(path)?
    } else {
        load_examples_binary(path)?
    };
    if let Some(n) = limit {
        examples.truncate(n);
    }
    Ok(examples)
}

fn append_log(path: &Path, epoch: usize, stats: &kuroobi::trainer::EpochStats) -> std::io::Result<()> {
    use std::io::Write;
    let new_file = !path.exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
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

    // Load all examples up front
    let mut examples: Vec<Example> = Vec::new();
    for file in &args.data_files {
        let t = Instant::now();
        match load_file(file, args.limit) {
            Ok(mut ex) => {
                println!(
                    "loaded {:>9} examples from {} ({:.1}s)",
                    ex.len(),
                    file.display(),
                    t.elapsed().as_secs_f32()
                );
                examples.append(&mut ex);
            }
            Err(e) => {
                eprintln!("failed to load {}: {e}", file.display());
                return ExitCode::FAILURE;
            }
        }
    }
    println!("total: {} examples", examples.len());

    // Deterministic Fisher-Yates shuffle: SGD assumes IID sample order, but
    // concatenated multi-source datasets (e.g. per-opening-depth files) are
    // grossly ordered — training in file order makes the model drift toward
    // whichever source came last and the epoch loss creep upward.
    {
        let mut state = 0x9E3779B97F4A7C15u64;
        for i in (1..examples.len()).rev() {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let j = (state.wrapping_mul(0x2545F4914F6CDD1D) % (i as u64 + 1)) as usize;
            examples.swap(i, j);
        }
        println!("shuffled (deterministic seed)");
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
            println!("optimizer: sgd (lr {}, decay {})", args.learning_rate, args.decay);
            let trainer = Trainer::new(
                evaluator,
                SgdOptimizer::new(args.learning_rate, args.decay),
            );
            run_epochs(trainer, &args, &examples, &interrupted)
        }
        OptimizerKind::Adam => {
            println!("optimizer: adam (lr {})", args.learning_rate);
            let trainer = Trainer::new(evaluator, AdamOptimizer::new(args.learning_rate));
            run_epochs(trainer, &args, &examples, &interrupted)
        }
    }
}

/// Render an in-place progress bar on stderr:
/// `epoch 3/100 [=========>          ]  45.2%  12.3M/25.5M  33k pos/s  ETA 6m32s`
fn draw_progress(epoch: usize, epochs: usize, done: usize, total: usize, started: &Instant) {
    const BAR_WIDTH: usize = 24;

    let frac = if total == 0 { 1.0 } else { done as f64 / total as f64 };
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
    let per_sec = if elapsed > 0.0 { done as f64 / elapsed } else { 0.0 };
    let eta_secs = if per_sec > 0.0 {
        ((total - done) as f64 / per_sec) as u64
    } else {
        0
    };
    let eta = if eta_secs >= 60 {
        format!("{}m{:02}s", eta_secs / 60, eta_secs % 60)
    } else {
        format!("{eta_secs}s")
    };

    let fmt_count = |n: usize| -> String {
        if n >= 1_000_000 {
            format!("{:.1}M", n as f64 / 1e6)
        } else if n >= 1_000 {
            format!("{:.0}k", n as f64 / 1e3)
        } else {
            n.to_string()
        }
    };

    eprint!(
        "\repoch {epoch}/{epochs} [{bar}] {:5.1}%  {}/{}  {:.0}k pos/s  ETA {eta}   ",
        frac * 100.0,
        fmt_count(done),
        fmt_count(total),
        per_sec / 1e3,
    );
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

fn run_epochs<O: Optimizer>(
    mut trainer: Trainer<O>,
    args: &Args,
    examples: &[Example],
    interrupted: &AtomicBool,
) -> ExitCode {
    for epoch in 1..=args.epochs {
        let t = Instant::now();
        let stats = if args.threads > 1 && args.optimizer == OptimizerKind::Sgd {
            // The decay schedule lives in the optimizer; mirror it here so
            // the parallel path follows the same curve.
            let lr = args.learning_rate * args.decay.powi(epoch as i32 - 1);
            trainer.train_epoch_parallel(examples, lr, args.threads, |done, total| {
                draw_progress(epoch, args.epochs, done, total, &t);
            })
        } else {
            trainer.train_epoch_with_progress(examples, |done, total| {
                draw_progress(epoch, args.epochs, done, total, &t);
            })
        };
        // Clear the progress line before printing the epoch summary
        eprint!("\r{:width$}\r", "", width = 90);
        let elapsed = t.elapsed().as_secs_f32();
        let per_sec = examples.len() as f32 / elapsed;
        println!(
            "epoch {:>3}/{}: mse {:.4}  ({:.1}s, {:.0} pos/s)",
            epoch,
            args.epochs,
            stats.mse(),
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

        if interrupted.load(Ordering::SeqCst) {
            println!(
                "stopped after epoch {epoch}; weights saved to {}",
                args.weights_path.display()
            );
            return ExitCode::SUCCESS;
        }
    }

    println!("weights saved to {}", args.weights_path.display());
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
