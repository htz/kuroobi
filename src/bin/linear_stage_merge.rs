//! Assemble one linear model out of several, taking each stage from whichever input scores best on a held-out set.
use kuroobi::linear::{Linear, STAGE_COUNT};
use kuroobi::pattern::LINEAR_PATTERNS;
use kuroobi::record::{Filter, TeacherPolicy};
use kuroobi::trainer::{load_examples_filtered_into, SelectBy, ValError};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: linear_stage_merge --val <file.data> [--val ...] --out <path>
  [--select-by mae|mse|spread] [--search-value-to-ply <n>]
  [--drop-random] [--max-score-diff <d>] [--keep-above-ply <n>] [--min-ply <n>]
  <weights.bin>...";

struct Args {
    val_paths: Vec<PathBuf>,
    filter: Filter,
    policy: TeacherPolicy,
    out: PathBuf,
    inputs: Vec<PathBuf>,
    select_by: SelectBy,
}

fn parse_args() -> Result<Args, String> {
    let mut val_paths = Vec::new();
    let mut filter = Filter::NONE;
    let mut policy = TeacherPolicy::DEFAULT;
    let mut out = None;
    let mut inputs = Vec::new();
    let mut select_by = SelectBy::Mae;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if filter.take_flag(&a, &mut it)? {
            continue;
        }
        match a.as_str() {
            "--val" => val_paths.extend(it.next().map(PathBuf::from)),
            "--search-value-to-ply" => {
                policy.search_value_to_ply = it.next().and_then(|v| v.parse().ok())
            }
            "--out" => out = it.next().map(PathBuf::from),
            "--select-by" => select_by = it.next().unwrap_or_default().parse()?,
            other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
            file => inputs.push(PathBuf::from(file)),
        }
    }
    let out = out.ok_or(USAGE)?;
    if val_paths.is_empty() {
        return Err("no --val given".into());
    }
    if inputs.is_empty() {
        return Err("no input weights given".into());
    }
    Ok(Args {
        val_paths,
        filter,
        policy,
        out,
        inputs,
        select_by,
    })
}

fn main() -> ExitCode {
    let Args {
        val_paths,
        filter,
        policy,
        out,
        inputs,
        select_by,
    } = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut val = Vec::new();
    for path in &val_paths {
        if let Err(e) = load_examples_filtered_into(path, &mut val, None, &filter, &policy) {
            eprintln!("failed to read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    println!(
        "val: {} positions from {} file(s), teacher: {}",
        val.len(),
        val_paths.len(),
        policy.describe()
    );

    let mut scores: Vec<Vec<ValError>> = Vec::with_capacity(inputs.len());
    let mut evs: Vec<Linear> = Vec::with_capacity(inputs.len());
    for p in &inputs {
        let mut ev = Linear::new(LINEAR_PATTERNS);
        if let Err(e) = ev.load_weights(p) {
            eprintln!("failed to load {}: {e}", p.display());
            return ExitCode::FAILURE;
        }
        let mut acc = vec![ValError::default(); STAGE_COUNT];
        for ex in &val {
            let board = ex.board();
            acc[Linear::stage(&board)].add(ev.eval(&board) as f64 - ex.score as f64);
        }
        scores.push(acc);
        evs.push(ev);
    }

    let mut merged = Linear::new(LINEAR_PATTERNS);
    if let Err(e) = merged.load_weights(&inputs[0]) {
        eprintln!("failed to load {}: {e}", inputs[0].display());
        return ExitCode::FAILURE;
    }

    let mut taken = vec![0usize; inputs.len()];
    println!(
        "  stage  empties       n      MSE     MAE     bias   spread   from  (selecting on {select_by})"
    );
    for (st, first) in scores[0].iter().enumerate() {
        let n = first.n;
        if n == 0.0 {
            continue;
        }
        let (best, _) = (0..inputs.len())
            .map(|i| (i, scores[i][st].score(select_by)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap();
        let (mse, mae, bias, sd) = scores[best][st].stats();
        let (w, num) = evs[best].stage_weights(st);
        merged.set_stage_weights(st, &w, &num);
        taken[best] += 1;
        println!(
            "  {:>5}  {:>7}  {:>6}  {:>7.3}  {:>6.3}  {:>+7.3}  {:>7.3}   {}",
            st,
            60 - st,
            n as u64,
            mse,
            mae,
            bias,
            sd,
            inputs[best].display()
        );
    }
    for (i, p) in inputs.iter().enumerate() {
        println!("{:>3} stages from {}", taken[i], p.display());
    }
    if let Err(e) = merged.save_weights(&out) {
        eprintln!("failed to save {}: {e}", out.display());
        return ExitCode::FAILURE;
    }
    println!("merged model saved to {}", out.display());
    ExitCode::SUCCESS
}
