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
use kuroobi::trainer::{load_examples_binary_into, Example};

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

fn main() -> ExitCode {
    let mut epochs = 10usize;
    let mut lr = 0.02f32;
    let mut decay = 1.0f32;
    let mut threads = 1usize;
    let mut limit: Option<usize> = None;
    let mut out = PathBuf::from("nnue.bin");
    let mut val_files: Vec<PathBuf> = Vec::new();
    let mut data_files: Vec<PathBuf> = Vec::new();

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
    let mut examples = match load(&data_files) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("load failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut val = load(&val_files).unwrap_or_default();
    // Cap the held-out set: a full-pass val each epoch dominates wall time,
    // and a few hundred k positions estimate the MSE tightly enough.
    const VAL_CAP: usize = 400_000;
    if val.len() > VAL_CAP {
        // Stride-sample so all game phases stay represented.
        let step = val.len() / VAL_CAP;
        val = val.iter().step_by(step).copied().collect();
    }
    println!("train {} / val {} examples", examples.len(), val.len());

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.init_weights();
    println!("nnue: H={} features={}", kuroobi::nnue::H, nn.n_features());

    let mut state = 0x9E3779B97F4A7C15u64;
    let mut best = f64::INFINITY;
    for epoch in 1..=epochs {
        // deterministic Fisher-Yates shuffle
        for i in (1..examples.len()).rev() {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let j = (state.wrapping_mul(0x2545F4914F6CDD1D) % (i as u64 + 1)) as usize;
            examples.swap(i, j);
        }

        let t = Instant::now();
        let cur_lr = lr * decay.powi(epoch as i32 - 1);
        let sq: f64 = if threads > 1 {
            // Hogwild: workers share the model through raw pointers.
            let view = nn.view();
            let nn_ref = &nn;
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
                                nn_ref.train_black_shared(view, &ix, stage, ex.score as f32, cur_lr)
                            } as f64;
                        }
                        s
                    }));
                }
                handles.into_iter().map(|h| h.join().unwrap()).sum()
            })
        } else {
            let mut s = 0.0f64;
            for ex in &examples {
                let board = ex.board();
                let stage = Evaluator::stage(&board);
                let ix = nn.indices(ex.black, ex.white);
                s += nn.train_black(&ix, stage, ex.score as f32, cur_lr) as f64;
            }
            s
        };
        let train_mse = sq / examples.len() as f64;
        let vm = val_mse(&nn, &val);
        let is_best = vm < best;
        let marker = if is_best { " *best" } else { "" };
        println!(
            "epoch {epoch:>2}/{epochs}: train {train_mse:.4}  val {vm:.4}{marker}  ({:.1}s, {:.0} pos/s)",
            t.elapsed().as_secs_f32(),
            examples.len() as f32 / t.elapsed().as_secs_f32(),
        );
        // Save only the best-by-val model (val overfits after a few epochs).
        if is_best {
            best = vm;
            if let Err(e) = nn.save(&out) {
                eprintln!("save failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    println!("saved {}", out.display());
    ExitCode::SUCCESS
}
