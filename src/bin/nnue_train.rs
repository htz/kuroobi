//! Trainer for the NNUE-style evaluator ([`kuroobi::nnue`]).
//!
//! Loads Black-to-move-normalized examples (same 17-byte format as `train`),
//! runs plain SGD through the single-hidden-layer network, and reports both
//! the training MSE and a held-out val MSE each epoch. The held-out MSE is the
//! honest signal to compare against the linear evaluator's ~39 floor.
//!
//! Usage:
//!   nnue_train [--epochs n] [--lr f] [--limit n] [--val f]... [--out path]
//!              <data-file>...

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use kuroobi::evaluator::Evaluator;
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::trainer::{count_examples_binary, load_examples_binary_into, Example};

fn val_mse(nn: &Nnue, val: &[Example]) -> f64 {
    if val.is_empty() {
        return f64::NAN;
    }
    let mut sum = 0.0f64;
    for ex in val {
        let board = ex.board();
        let ix = nn.indices(ex.black, ex.white);
        let e = ex.score as f64 - nn.eval_indices(&board, &ix) as f64;
        sum += e * e;
    }
    sum / val.len() as f64
}

/// How many examples may be resident at once. The full corpus is far larger
/// than RAM, so an epoch is run as a sequence of shards: whole files grouped
/// up to this budget, loaded, trained on, and dropped. Reading is a rounding
/// error next to the gradient work, so nothing is lost by not caching.
const DEFAULT_MAX_EXAMPLES: usize = 48_000_000;

/// A group of whole files that fit in the budget together.
struct Shard {
    files: Vec<usize>,
    examples: usize,
}

/// Group files into shards no larger than `max` examples. `order` varies which
/// files share a shard between epochs, so a fixed grouping does not turn into a
/// fixed correlation in the data.
fn shards(counts: &[usize], order: &[usize], max: usize) -> Vec<Shard> {
    let mut out: Vec<Shard> = Vec::new();
    let mut cur = Shard { files: Vec::new(), examples: 0 };
    for &i in order {
        let n = counts[i];
        if !cur.files.is_empty() && cur.examples + n > max {
            out.push(std::mem::replace(&mut cur, Shard { files: Vec::new(), examples: 0 }));
        }
        cur.files.push(i);
        cur.examples += n;
    }
    if !cur.files.is_empty() {
        out.push(cur);
    }
    out
}

fn train_pass(nn: &mut Nnue, examples: &[Example], threads: usize, lr: f32) -> f64 {
    if threads > 1 {
        // Hogwild: workers share the model through raw pointers.
        let view = nn.view();
        let nn_ref = &*nn;
        let chunk = examples.len().div_ceil(threads);
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for part in examples.chunks(chunk) {
                let view = &view;
                handles.push(scope.spawn(move || {
                    let mut s = 0.0f64;
                    for ex in part {
                        let board = ex.board();
                        let stage = Evaluator::stage(&board);
                        let ix = nn_ref.indices(ex.black, ex.white);
                        // SAFETY: `view` is from `nn`, borrowed immutably here.
                        s += unsafe {
                            nn_ref.train_black_shared(view, &ix, stage, ex.score as f32, lr)
                        } as f64;
                    }
                    s
                }));
            }
            handles.into_iter().map(|h| h.join().unwrap()).sum()
        })
    } else {
        let mut s = 0.0f64;
        for ex in examples {
            let board = ex.board();
            let stage = Evaluator::stage(&board);
            let ix = nn.indices(ex.black, ex.white);
            s += nn.train_black(&ix, stage, ex.score as f32, lr) as f64;
        }
        s
    }
}

fn main() -> ExitCode {
    let mut epochs = 10usize;
    let mut lr = 0.02f32;
    let mut decay = 1.0f32;
    let mut threads = 1usize;
    let mut limit: Option<usize> = None;
    let mut out = PathBuf::from("weights/nnue.bin");
    let mut val_files: Vec<PathBuf> = Vec::new();
    let mut data_files: Vec<PathBuf> = Vec::new();
    let mut max_examples = DEFAULT_MAX_EXAMPLES;
    let mut val_cap: Option<usize> = None;
    let mut init: Option<PathBuf> = None;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--epochs" => epochs = it.next().unwrap().parse().unwrap(),
            "--lr" => lr = it.next().unwrap().parse().unwrap(),
            "--decay" => decay = it.next().unwrap().parse().unwrap(),
            "--threads" => threads = it.next().unwrap().parse().unwrap(),
            "--limit" => limit = Some(it.next().unwrap().parse().unwrap()),
            "--out" => out = PathBuf::from(it.next().unwrap()),
            "--val" => val_files.push(PathBuf::from(it.next().unwrap())),
            "--max-examples" => max_examples = it.next().unwrap().parse().unwrap(),
            "--val-cap" => val_cap = Some(it.next().unwrap().parse().unwrap()),
            "--init" => init = Some(PathBuf::from(it.next().unwrap())),
            other if other.starts_with('-') => {
                eprintln!("unknown option {other}");
                return ExitCode::FAILURE;
            }
            file => data_files.push(PathBuf::from(file)),
        }
    }
    if data_files.is_empty() {
        eprintln!("usage: nnue_train [--epochs n] [--lr f] [--limit n] [--val f]... [--out p] <data>...");
        return ExitCode::FAILURE;
    }

    let load = |files: &[PathBuf]| -> std::io::Result<Vec<Example>> {
        let mut v = Vec::new();
        for f in files {
            load_examples_binary_into(f, &mut v, limit)?;
        }
        Ok(v)
    };
    let counts: Vec<usize> = {
        let mut c = Vec::with_capacity(data_files.len());
        for f in &data_files {
            // Binary records are fixed width, so size/record is exact.
            match count_examples_binary(f) {
                Ok(n) => c.push(limit.map_or(n, |l| n.min(l))),
                Err(e) => {
                    eprintln!("cannot size {}: {e}", f.display());
                    return ExitCode::FAILURE;
                }
            }
        }
        c
    };
    let total: usize = counts.iter().sum();
    let mut val = load(&val_files).unwrap_or_default();
    // Cap the held-out set: a full-pass val each epoch dominates wall time,
    // and a few hundred k positions estimate the MSE tightly enough. Raise it
    // with `--val-cap` when the point is to compare models rather than to
    // train — a sampled val is fine for picking an epoch but is not the same
    // number as a full-record MSE, so the two must not be compared to each
    // other.
    let val_cap = val_cap.unwrap_or(400_000);
    if val.len() > val_cap {
        // Stride-sample so all game phases stay represented.
        let step = val.len() / val_cap;
        val = val.iter().step_by(step).copied().collect();
    }
    println!(
        "train {} examples in {} files (shard budget {}) / val {}",
        total,
        data_files.len(),
        max_examples,
        val.len()
    );

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    match &init {
        // Warm start: keep training a model instead of starting over.
        Some(p) => match nn.load(p) {
            Ok(()) => println!("resumed from {}", p.display()),
            Err(e) => {
                eprintln!("load {} failed: {e}", p.display());
                return ExitCode::FAILURE;
            }
        },
        None => nn.init_weights(),
    }
    println!("nnue: H={} features={}", kuroobi::nnue::H, nn.n_features());

    let mut state = 0x9E3779B97F4A7C15u64;
    let mut rand = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545F4914F6CDD1D)
    };
    let mut best = if val.is_empty() { f64::INFINITY } else { val_mse(&nn, &val) };
    if best.is_finite() {
        println!("starting val {best:.4}");
    }
    for epoch in 1..=epochs {
        let t = Instant::now();
        let cur_lr = lr * decay.powi(epoch as i32 - 1);

        // Fresh file order each epoch, so shard membership keeps changing.
        let mut order: Vec<usize> = (0..data_files.len()).collect();
        for i in (1..order.len()).rev() {
            let j = (rand() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let plan = shards(&counts, &order, max_examples);

        let mut sq_total = 0.0f64;
        let mut seen = 0usize;
        for (si, shard) in plan.iter().enumerate() {
            let files: Vec<PathBuf> = shard.files.iter().map(|&i| data_files[i].clone()).collect();
            let mut examples = match load(&files) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("load failed: {e}");
                    return ExitCode::FAILURE;
                }
            };
            // Shuffle within the shard: file order alone leaves each file's
            // games adjacent, which correlates consecutive updates.
            for i in (1..examples.len()).rev() {
                let j = (rand() % (i as u64 + 1)) as usize;
                examples.swap(i, j);
            }
            let ts = Instant::now();
            let sq: f64 = train_pass(&mut nn, &examples, threads, cur_lr);
            sq_total += sq;
            seen += examples.len();
            println!(
                "  epoch {epoch} shard {}/{}: {} examples, train {:.4}  ({:.1}s, {:.0} pos/s)",
                si + 1,
                plan.len(),
                examples.len(),
                sq / examples.len() as f64,
                ts.elapsed().as_secs_f32(),
                examples.len() as f32 / ts.elapsed().as_secs_f32(),
            );
        }
        let train_mse = sq_total / seen.max(1) as f64;
        let vm = val_mse(&nn, &val);
        let is_best = vm < best;
        let marker = if is_best { " *best" } else { "" };
        println!(
            "epoch {epoch:>2}/{epochs}: train {train_mse:.4}  val {vm:.4}{marker}  ({:.1}s, {:.0} pos/s)",
            t.elapsed().as_secs_f32(),
            seen as f32 / t.elapsed().as_secs_f32(),
        );
        // Save only the best-by-val model (val overfits after a few epochs).
        if is_best {
            best = vm;
            if let Err(e) = nn.save(&out) {
                eprintln!("save failed: {e}");
                return ExitCode::FAILURE;
            }
            println!("  saved {}", out.display());
        }
    }
    if best.is_finite() && !val.is_empty() {
        println!("best val {best:.4}");
    }
    ExitCode::SUCCESS
}
