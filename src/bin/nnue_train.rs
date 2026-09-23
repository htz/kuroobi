//! Trainer for the NNUE-style evaluator ([`kuroobi::nnue`]).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use kuroobi::linear::{Linear, STAGE_COUNT};
use kuroobi::nnue::{AdamState, Nnue};
use kuroobi::pattern;
use kuroobi::record::{Filter, TeacherPolicy};
use kuroobi::trainer::{
    count_examples_binary, load_examples_filtered_into, load_examples_range_into, Example, SymPlan,
};

fn val_by_stage(nn: &mut Nnue, base: Option<&Linear>, val: &[Example]) -> Vec<[f64; 5]> {
    nn.quantize();
    let mut acc = vec![[0.0f64; 5]; STAGE_COUNT];
    for ex in val {
        let board = ex.board();
        let ix = nn.indices(ex.black, ex.white);
        let b = base.map_or(0.0, |e| e.eval_indices(&board, &ix));
        let q = nn.eval_from_indices(&ix, &board);
        let e = (q + b) as f64 - ex.score as f64;
        let a = &mut acc[Linear::stage(&board)];
        a[0] += 1.0;
        a[1] += e * e;
        a[2] += e.abs();
        a[3] += e;
        a[4] += (q - nn.eval_indices(&board, &ix)).abs() as f64;
    }
    acc
}

fn stats_of(a: &[f64; 5]) -> (f64, f64, f64, f64) {
    let n = a[0];
    if n == 0.0 {
        return (f64::NAN, f64::NAN, f64::NAN, f64::NAN);
    }
    let (mse, mae, bias) = (a[1] / n, a[2] / n, a[3] / n);
    (mse, mae, bias, (mse - bias * bias).max(0.0).sqrt())
}

fn pooled(acc: &[[f64; 5]]) -> [f64; 5] {
    let mut t = [0.0f64; 5];
    for a in acc {
        for (d, s) in t.iter_mut().zip(a) {
            *d += s;
        }
    }
    t
}

fn print_stage_table(acc: &[[f64; 5]]) {
    println!("  stage  empties       n      MSE      MAE     bias   spread     grid");
    for (st, a) in acc.iter().enumerate() {
        if a[0] == 0.0 {
            continue;
        }
        let (mse, mae, bias, sd) = stats_of(a);
        println!(
            "  {:>5}  {:>7}  {:>6}  {:>7.3}  {:>7.3}  {:>+7.3}  {:>7.3}  {:>7.3}",
            st,
            60 - st,
            a[0] as u64,
            mse,
            mae,
            bias,
            sd,
            a[4] / a[0]
        );
    }
}

fn select_score(which: &str, a: &[f64; 5]) -> f64 {
    let (mse, mae, _, sd) = stats_of(a);
    match which {
        "mae" => mae,
        "spread" => sd,
        _ => mse,
    }
}

const DEFAULT_MAX_EXAMPLES: usize = 48_000_000;

struct Shard {
    parts: Vec<(usize, usize, usize)>,
    examples: usize,
}

fn shards(counts: &[usize], order: &[usize], max: usize) -> Vec<Shard> {
    let mut out: Vec<Shard> = Vec::new();
    let mut cur = Shard {
        parts: Vec::new(),
        examples: 0,
    };
    for &i in order {
        let n = counts[i];
        if !cur.parts.is_empty() && cur.examples + n > max {
            out.push(std::mem::replace(
                &mut cur,
                Shard {
                    parts: Vec::new(),
                    examples: 0,
                },
            ));
        }
        cur.parts.push((i, 0, n));
        cur.examples += n;
    }
    if !cur.parts.is_empty() {
        out.push(cur);
    }
    out
}

fn interleaved_shards(counts: &[usize], rot: &[usize], max: usize) -> Vec<Shard> {
    let total: usize = counts.iter().sum();
    let s = total.div_ceil(max).max(1);
    (0..s)
        .map(|k| {
            let mut shard = Shard {
                parts: Vec::new(),
                examples: 0,
            };
            for (i, &n) in counts.iter().enumerate() {
                let (lo, hi) = (k * n / s, (k + 1) * n / s);
                let len = hi - lo;
                if len == 0 {
                    continue;
                }
                let start = (lo + rot[i]) % n;
                if start + len <= n {
                    shard.parts.push((i, start, len));
                } else {
                    shard.parts.push((i, start, n - start));
                    shard.parts.push((i, 0, len - (n - start)));
                }
                shard.examples += len;
            }
            shard
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn train_pass_minibatch(
    nn: &mut Nnue,
    base: Option<&Linear>,
    adam: &mut AdamState,
    sinks: &mut Vec<kuroobi::nnue::GradSink>,
    examples: &[Example],
    threads: usize,
    batch: usize,
    lr_for_step: &mut impl FnMut() -> f32,
    wd: f32,
    sym: SymPlan,
) -> f64 {
    let threads = threads.max(1);
    while sinks.len() < threads {
        sinks.push(kuroobi::nnue::GradSink::new(threads));
    }
    let mut sq_total = 0.0f64;
    for (bno, chunk) in examples.chunks(batch.max(1)).enumerate() {
        let bno = bno as u64;
        const PIECE: usize = 256;
        let pieces = chunk.len().div_ceil(PIECE);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let next = &next;
        let nn_ref = &*nn;
        let used = threads;
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for sink in sinks[..threads].iter_mut() {
                handles.push(scope.spawn(move || {
                    sink.clear();
                    let mut s = 0.0f64;
                    loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if k >= pieces {
                            break;
                        }
                        let lo = k * PIECE;
                        let hi = ((k + 1) * PIECE).min(chunk.len());
                        let mut rs = sym.stream((k as u64 + 1) * 0x1000 + bno + 1);
                        for ex in &chunk[lo..hi] {
                            let ex = sym.apply(ex, &mut rs);
                            let board = ex.board();
                            let stage = Linear::stage(&board);
                            let discs = ex.black.count_ones() as usize;
                            let mob = kuroobi::nnue::Nnue::mob_index(&board);
                            let ix = nn_ref.indices(ex.black, ex.white);
                            s += nn_ref.grad_black_into(
                                &ix,
                                stage,
                                discs,
                                mob,
                                ex.score - base.map_or(0.0, |e| e.eval_indices(&board, &ix)),
                                sink,
                            ) as f64;
                        }
                    }
                    s
                }));
            }
            for h in handles {
                sq_total += h.join().unwrap();
            }
        });
        let lr = lr_for_step();
        nn.apply_adamw_batch(&mut sinks[..used], adam, lr, wd, 1.0 / chunk.len() as f32);
    }
    sq_total
}

fn train_pass(
    nn: &mut Nnue,
    adam: Option<&mut AdamState>,
    examples: &[Example],
    threads: usize,
    lr: f32,
    sym: SymPlan,
) -> f64 {
    let av = adam.map(|a| a.view());
    let view = nn.view();
    let nn_ref = &*nn;
    let threads = threads.max(1);
    let chunk = examples.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (ti, part) in examples.chunks(chunk).enumerate() {
            let view = &view;
            let av = av.as_ref();
            handles.push(scope.spawn(move || {
                let mut s = 0.0f64;
                let mut rs = sym.stream(ti as u64 + 1);
                for ex in part {
                    let ex = sym.apply(ex, &mut rs);
                    let ex = &ex;
                    let board = ex.board();
                    let stage = Linear::stage(&board);
                    let discs = ex.black.count_ones() as usize;
                    let mob = kuroobi::nnue::Nnue::mob_index(&board);
                    let ix = nn_ref.indices(ex.black, ex.white);
                    s += unsafe {
                        match av {
                            Some(a) => nn_ref.train_black_adam_shared(
                                view, a, &ix, stage, discs, mob, ex.score, lr,
                            ),
                            None => nn_ref
                                .train_black_shared(view, &ix, stage, discs, mob, ex.score, lr),
                        }
                    } as f64;
                }
                s
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap()).sum()
    })
}

#[derive(Default)]
struct CkptHeader {
    epoch: usize,
    mb_step: u64,
    mb_total_steps: u64,
    plateau_lr: f32,
    stale: usize,
    best: f64,
    rng: u64,
}

const CKPT_MAGIC: &[u8; 8] = b"BBRVCK01";
const CKPT_HEADER_LEN: usize = 512;

fn write_checkpoint(
    path: &std::path::Path,
    nn: &Nnue,
    adam: &AdamState,
    h: &CkptHeader,
) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("part");
    {
        let f = std::fs::File::create(&tmp)?;
        let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
        let text = format!(
            "epoch {}\nmb_step {}\nmb_total_steps {}\nplateau_lr {}\nstale {}\nbest {}\n\
             rng {}\n",
            h.epoch, h.mb_step, h.mb_total_steps, h.plateau_lr, h.stale, h.best, h.rng
        );
        let mut head = [b' '; CKPT_HEADER_LEN];
        let bytes = text.as_bytes();
        if bytes.len() > CKPT_HEADER_LEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "checkpoint header does not fit",
            ));
        }
        head[..bytes.len()].copy_from_slice(bytes);
        w.write_all(CKPT_MAGIC)?;
        w.write_all(&head)?;
        let mut bin: Vec<u8> = Vec::new();
        nn.write_to(&mut bin)?;
        w.write_all(&(bin.len() as u64).to_le_bytes())?;
        w.write_all(&bin)?;
        adam.write_state(&mut w)?;
        w.flush()?;
    }
    std::fs::rename(&tmp, path)
}

fn read_checkpoint(
    path: &std::path::Path,
    nn: &mut Nnue,
    adam: &mut AdamState,
) -> std::io::Result<CkptHeader> {
    use std::io::Read;
    let f = std::fs::File::open(path)?;
    let mut r = std::io::BufReader::with_capacity(1 << 20, f);
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != CKPT_MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a checkpoint",
        ));
    }
    let mut head = [0u8; CKPT_HEADER_LEN];
    r.read_exact(&mut head)?;
    let mut h = CkptHeader::default();
    for line in String::from_utf8_lossy(&head).lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else {
            continue;
        };
        match k {
            "epoch" => h.epoch = v.parse().unwrap_or(0),
            "mb_step" => h.mb_step = v.parse().unwrap_or(0),
            "mb_total_steps" => h.mb_total_steps = v.parse().unwrap_or(0),
            "plateau_lr" => h.plateau_lr = v.parse().unwrap_or(0.0),
            "stale" => h.stale = v.parse().unwrap_or(0),
            "best" => h.best = v.parse().unwrap_or(f64::INFINITY),
            "rng" => h.rng = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    let mut u8b = [0u8; 8];
    r.read_exact(&mut u8b)?;
    let bin_len = u64::from_le_bytes(u8b) as usize;
    let mut bin = vec![0u8; bin_len];
    r.read_exact(&mut bin)?;
    nn.read_from(&mut &bin[..])?;
    adam.read_state(&mut r)?;
    Ok(h)
}

struct Args {
    epochs: usize,
    lr: f32,
    decay: f32,
    cosine: bool,
    plateau: usize,
    select_by: String,
    val_by_stage_report: bool,
    plateau_factor: f32,
    plateau_min: f32,
    wd: f32,
    minibatch: usize,
    adam: bool,
    sym_train: bool,
    sym_all: bool,
    lookahead: u32,
    gpu: bool,
    threads: usize,
    limit: Option<usize>,
    filter: Filter,
    policy: TeacherPolicy,
    out: PathBuf,
    val_files: Vec<PathBuf>,
    data_files: Vec<PathBuf>,
    max_examples: usize,
    interleave: bool,
    val_cap: Option<usize>,
    init: Option<PathBuf>,
    checkpoint: Option<PathBuf>,
    resume: Option<PathBuf>,
    start_epoch: usize,
    which_patterns: String,
    patterns_file: Option<PathBuf>,
    patterns_share: bool,
}

fn parse_args() -> Result<Args, ExitCode> {
    let mut epochs = 10usize;
    let mut lr = 0.02f32;
    let mut decay = 1.0f32;
    let mut cosine = false;
    let mut plateau = 0usize;
    let mut select_by = String::from("mse");
    let mut val_by_stage_report = false;
    let mut plateau_factor = 0.5f32;
    let mut plateau_min = 1e-6f32;
    let mut wd = 0.0f32;
    let mut minibatch = 0usize;
    let mut adam = false;
    let mut sym_train = false;
    let mut sym_all = false;
    let mut lookahead = 0u32;
    let mut gpu = false;
    let mut threads = 1usize;
    let mut limit: Option<usize> = None;
    let mut filter = Filter::NONE;
    let mut policy = TeacherPolicy::DEFAULT;
    let mut out = PathBuf::from("weights/nnue.bin");
    let mut val_files: Vec<PathBuf> = Vec::new();
    let mut data_files: Vec<PathBuf> = Vec::new();
    let mut max_examples = DEFAULT_MAX_EXAMPLES;
    let mut interleave = false;
    let mut val_cap: Option<usize> = None;
    let mut init: Option<PathBuf> = None;
    let mut checkpoint: Option<PathBuf> = None;
    let mut resume: Option<PathBuf> = None;
    let mut start_epoch = 1usize;
    let mut which_patterns = String::from("nnue");
    let mut patterns_file: Option<PathBuf> = None;
    let mut patterns_share = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match filter.take_flag(&a, &mut it) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                eprintln!("{e}");
                return Err(ExitCode::FAILURE);
            }
        }
        match a.as_str() {
            "--patterns" => which_patterns = it.next().unwrap(),
            "--patterns-file" => patterns_file = Some(PathBuf::from(it.next().unwrap())),
            "--patterns-share" => patterns_share = true,
            "--epochs" => epochs = it.next().unwrap().parse().unwrap(),
            "--lr" => lr = it.next().unwrap().parse().unwrap(),
            "--decay" => decay = it.next().unwrap().parse().unwrap(),
            "--cosine" => cosine = true,
            "--plateau" => plateau = it.next().unwrap().parse().unwrap(),
            "--select-by" => select_by = it.next().unwrap(),
            "--val-by-stage" => val_by_stage_report = true,
            "--plateau-factor" => plateau_factor = it.next().unwrap().parse().unwrap(),
            "--plateau-min" => plateau_min = it.next().unwrap().parse().unwrap(),
            "--wd" => wd = it.next().unwrap().parse().unwrap(),
            "--minibatch" => minibatch = it.next().unwrap().parse().unwrap(),
            "--adam" => adam = true,
            "--search-value-to-ply" => {
                policy.search_value_to_ply = it.next().and_then(|v| v.parse().ok());
            }
            "--sym-train" => sym_train = true,
            "--sym-all" => sym_all = true,
            "--lookahead" => lookahead = 6,
            "--gpu" => gpu = true,
            "--threads" => threads = it.next().unwrap().parse().unwrap(),
            "--limit" => limit = Some(it.next().unwrap().parse().unwrap()),
            "--out" => out = PathBuf::from(it.next().unwrap()),
            "--val" => val_files.push(PathBuf::from(it.next().unwrap())),
            "--max-examples" => max_examples = it.next().unwrap().parse().unwrap(),
            "--interleave" => interleave = true,
            "--val-cap" => val_cap = Some(it.next().unwrap().parse().unwrap()),
            "--init" => init = Some(PathBuf::from(it.next().unwrap())),
            "--checkpoint" => checkpoint = Some(PathBuf::from(it.next().unwrap())),
            "--resume" => resume = Some(PathBuf::from(it.next().unwrap())),
            "--start-epoch" => start_epoch = it.next().unwrap().parse().unwrap(),
            other if other.starts_with('-') => {
                eprintln!("unknown option {other}");
                return Err(ExitCode::FAILURE);
            }
            file => data_files.push(PathBuf::from(file)),
        }
    }
    Ok(Args {
        epochs,
        lr,
        decay,
        cosine,
        plateau,
        select_by,
        val_by_stage_report,
        plateau_factor,
        plateau_min,
        wd,
        minibatch,
        adam,
        sym_train,
        sym_all,
        lookahead,
        gpu,
        threads,
        limit,
        filter,
        policy,
        out,
        val_files,
        data_files,
        max_examples,
        interleave,
        val_cap,
        init,
        checkpoint,
        resume,
        start_epoch,
        which_patterns,
        patterns_file,
        patterns_share,
    })
}

fn main() -> ExitCode {
    let Args {
        epochs,
        lr,
        decay,
        cosine,
        plateau,
        select_by,
        val_by_stage_report,
        plateau_factor,
        plateau_min,
        wd,
        minibatch,
        adam,
        sym_train,
        sym_all,
        lookahead,
        gpu,
        threads,
        limit,
        filter,
        policy,
        out,
        val_files,
        data_files,
        max_examples,
        interleave,
        val_cap,
        init,
        mut checkpoint,
        resume,
        start_epoch,
        mut which_patterns,
        patterns_file,
        patterns_share,
    } = match parse_args() {
        Ok(a) => a,
        Err(code) => return code,
    };
    if data_files.is_empty() {
        eprintln!(
            "usage: nnue_train [--epochs n] [--lr f] [--plateau n] [--limit n] [--val f]... [--out p] <data>..."
        );
        return ExitCode::FAILURE;
    }

    let load = |files: &[PathBuf]| -> std::io::Result<Vec<Example>> {
        let mut v = Vec::new();
        for f in files {
            load_examples_filtered_into(f, &mut v, limit, &filter, &policy)?;
        }
        Ok(v)
    };
    let load_parts = |parts: &[(usize, usize, usize)]| -> std::io::Result<Vec<Example>> {
        let mut v = Vec::new();
        for &(i, start, len) in parts {
            load_examples_range_into(&data_files[i], &mut v, start, len, &filter, &policy)?;
        }
        Ok(v)
    };
    let counts = match size_files(&data_files, limit) {
        Ok(c) => c,
        Err(()) => return ExitCode::FAILURE,
    };
    let total: usize = counts.iter().sum();
    let mut val = load(&val_files).unwrap_or_default();
    let val_cap = val_cap.unwrap_or(400_000);
    if val.len() > val_cap {
        let step = val.len() / val_cap;
        val = val.iter().step_by(step).copied().collect();
    }
    println!(
        "train {} records in {} files (shard budget {}) / val {} / filter {} / teacher {}",
        total,
        data_files.len(),
        max_examples,
        val.len(),
        filter.describe(),
        policy.describe()
    );

    let patterns = match resolve_patterns(
        patterns_file.as_deref(),
        &mut which_patterns,
        patterns_share,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if checkpoint.is_none() {
        checkpoint.clone_from(&resume);
    }
    if resume.is_some() && init.is_some() {
        eprintln!("--resume and --init both restore weights; pass one");
        return ExitCode::FAILURE;
    }
    if resume.is_some() && !adam {
        eprintln!("--resume restores Adam moments, so it needs --adam");
        return ExitCode::FAILURE;
    }
    let mut nn = Nnue::new(patterns);
    match &init {
        Some(p) => match nn.load(p) {
            Ok(()) => println!("resumed from {}", p.display()),
            Err(e) => {
                eprintln!("load {} failed: {e}", p.display());
                return ExitCode::FAILURE;
            }
        },
        None => nn.init_weights(),
    }
    nn.set_mpc_sigma(None);
    println!(
        "nnue: patterns={which_patterns} masks={} H={} features={}",
        patterns.iter().map(|p| p.masks.len()).sum::<usize>(),
        kuroobi::nnue::H,
        nn.n_features()
    );

    let mut sinks: Vec<kuroobi::nnue::GradSink> = Vec::new();
    let mut mb_step: u64 = 0;
    let mut mb_step_done: u64 = 0;
    let base_lr = lr;
    let mut mb_total_steps: u64 = 0;
    let mut adam_state = adam.then(|| {
        println!(
            "adam: moments {} MB",
            (nn.ft_len() + STAGE_COUNT * kuroobi::nnue::H) * 2 * 4 / 1_000_000
        );
        let mut st = AdamState::new(&nn);
        st.wd = wd;
        if lookahead > 0 {
            st.set_lookahead(lookahead, 0.5);
            println!("adam: lookahead k={lookahead} alpha=0.5");
        }
        st
    });

    #[cfg(not(feature = "gpu"))]
    if gpu {
        eprintln!("--gpu needs a build with `--features gpu`");
        return ExitCode::FAILURE;
    }

    let rng_state = std::cell::Cell::new(0x9E3779B97F4A7C15u64);
    let rand = || {
        let mut state = rng_state.get();
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        rng_state.set(state);
        state.wrapping_mul(0x2545F4914F6CDD1D)
    };
    let mut best = if val.is_empty() {
        f64::INFINITY
    } else {
        select_score(&select_by, &pooled(&val_by_stage(&mut nn, None, &val)))
    };
    if best.is_finite() {
        report_start_val(&mut nn, &val, &select_by, val_by_stage_report);
    }
    let mut sched = Sched {
        plateau_lr: lr,
        stale: 0,
        first_epoch: 1,
        virtual_done: 0,
    };
    match enter_schedule(start_epoch, epochs, resume.is_some(), lr) {
        Ok(Some((first, done))) => {
            sched.first_epoch = first;
            sched.virtual_done = done;
        }
        Ok(None) => {}
        Err(()) => return ExitCode::FAILURE,
    }
    if let Some(path) = &resume {
        let Some(ad) = adam_state.as_mut() else {
            eprintln!("--resume needs --adam");
            return ExitCode::FAILURE;
        };
        let Ok(h) = apply_resume(path, &mut nn, ad, &mut sched, &rng_state) else {
            return ExitCode::FAILURE;
        };
        mb_step = h.mb_step;
        mb_step_done = h.mb_step;
        mb_total_steps = h.mb_total_steps;
        best = h.best;
        if sched.first_epoch > epochs {
            println!("nothing left to do: --epochs {epochs} already reached");
            return ExitCode::SUCCESS;
        }
    }
    let Sched {
        mut plateau_lr,
        mut stale,
        first_epoch,
        virtual_done,
    } = sched;
    #[cfg(feature = "gpu")]
    let mut gpu_trainer = gpu.then(|| {
        let ad = adam_state
            .as_ref()
            .expect("--gpu requires --adam --minibatch N");
        assert!(minibatch > 0, "--gpu requires --minibatch N");
        let la = if lookahead > 0 {
            (lookahead, 0.5)
        } else {
            (0, 0.0)
        };
        kuroobi::nnue::gpu::GpuTrainer::new(&nn, minibatch, ad, la)
    });

    for epoch in first_epoch..=epochs {
        let t = Instant::now();
        if minibatch > 0 && epoch == first_epoch && mb_total_steps <= mb_step_done {
            mb_total_steps = u64::MAX;
        }
        let cur_lr = if plateau > 0 {
            plateau_lr
        } else if cosine {
            let t = (epoch as f32 - 1.0) / epochs as f32;
            lr * 0.5 * (1.0 + (std::f32::consts::PI * t).cos())
        } else {
            lr * decay.powi(epoch as i32 - 1)
        };

        let plan = epoch_plan(interleave, &counts, max_examples, &rand);
        let mut sq_total = 0.0f64;
        let mut seen = 0usize;
        let tprof = std::env::var_os("KUROOBI_TRAIN_PROF").is_some();
        #[cfg(feature = "gpu")]
        let want_ckpt = checkpoint.is_some();
        let (mut t_load, mut t_shuf, mut t_pass) = (0.0f64, 0.0f64, 0.0f64);
        for (si, shard) in plan.iter().enumerate() {
            let t_l = Instant::now();
            let mut examples = match load_parts(&shard.parts) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("load failed: {e}");
                    return ExitCode::FAILURE;
                }
            };
            t_load += t_l.elapsed().as_secs_f64();
            let t_s = Instant::now();
            shuffle(&mut examples, &rand);
            t_shuf += t_s.elapsed().as_secs_f64();
            let ts = Instant::now();
            let t_p = Instant::now();
            let sym_seed = if sym_train { rand() | 1 } else { 0 };
            let forms: Vec<Option<u8>> = if sym_all {
                (0..8u8).map(Some).collect()
            } else {
                vec![None]
            };
            let mut sq = 0.0f64;
            let mut rows = 0usize;
            for (fi, &fixed) in forms.iter().enumerate() {
                let sym = SymPlan {
                    seed: sym_seed,
                    fixed,
                };
                if fi > 0 {
                    let t_s = Instant::now();
                    shuffle(&mut examples, &rand);
                    t_shuf += t_s.elapsed().as_secs_f64();
                }
                rows += examples.len();
                sq += if minibatch > 0 {
                    let ad = adam_state
                        .as_mut()
                        .expect("--minibatch requires --adam (moments)");
                    let entry_t = virtual_done as f32 / epochs as f32;
                    let mut lr_fn = || {
                        let lr = if cosine {
                            let t = if mb_total_steps == u64::MAX {
                                entry_t
                            } else {
                                mb_step as f32 / mb_total_steps.max(1) as f32
                            };
                            const ETA_MIN: f32 = 1e-8;
                            ETA_MIN
                                + (base_lr - ETA_MIN)
                                    * 0.5
                                    * (1.0 + (std::f32::consts::PI * t.min(1.0)).cos())
                        } else {
                            cur_lr
                        };
                        mb_step += 1;
                        lr
                    };
                    #[cfg(feature = "gpu")]
                    if let Some(g) = gpu_trainer.as_mut() {
                        let sq = g.train_shard(&nn, &examples, threads, &mut lr_fn, wd, sym);
                        if si + 1 == plan.len() {
                            match adam_state.as_mut().filter(|_| want_ckpt) {
                                Some(ad) => g.download_state(&mut nn, ad),
                                None => g.download(&mut nn),
                            }
                        }
                        sq
                    } else {
                        train_pass_minibatch(
                            &mut nn, None, ad, &mut sinks, &examples, threads, minibatch,
                            &mut lr_fn, wd, sym,
                        )
                    }
                    #[cfg(not(feature = "gpu"))]
                    train_pass_minibatch(
                        &mut nn, None, ad, &mut sinks, &examples, threads, minibatch, &mut lr_fn,
                        wd, sym,
                    )
                } else {
                    train_pass(
                        &mut nn,
                        adam_state.as_mut(),
                        &examples,
                        threads,
                        cur_lr,
                        sym,
                    )
                };
            }
            sq_total += sq;
            seen += rows;
            println!(
                "  epoch {epoch} shard {}/{}: {} examples, train {:.4}  ({:.1}s, {:.0} pos/s)",
                si + 1,
                plan.len(),
                examples.len(),
                sq / rows as f64,
                ts.elapsed().as_secs_f32(),
                rows as f32 / ts.elapsed().as_secs_f32(),
            );
            t_pass += t_p.elapsed().as_secs_f64();
        }
        if minibatch > 0 && epoch == first_epoch {
            let per_epoch = mb_step - mb_step_done;
            if virtual_done > 0 {
                let skipped = per_epoch * virtual_done as u64;
                mb_step_done += skipped;
                mb_step += skipped;
            }
            mb_total_steps = mb_step_done + per_epoch * (epochs - first_epoch + 1) as u64;
        }
        let vm = report_epoch(
            &mut nn,
            &val,
            &select_by,
            val_by_stage_report,
            EpochLine {
                epoch,
                epochs,
                train_mse: sq_total / seen.max(1) as f64,
                seen,
                secs: t.elapsed().as_secs_f32(),
                best,
            },
            tprof.then_some((t_load, t_shuf, t_pass)),
        );
        let is_best = vm < best;
        if let Some(ad) = adam_state.as_mut().filter(|_| !gpu) {
            nn.settle_adam(ad, cur_lr, wd);
        }
        let h = CkptHeader {
            epoch,
            mb_step,
            mb_total_steps,
            plateau_lr,
            stale,
            best: if is_best { vm } else { best },
            rng: rng_state.get(),
        };
        if save_epoch(
            &nn,
            &out,
            checkpoint.as_deref(),
            adam_state.as_ref(),
            &h,
            is_best,
        )
        .is_err()
        {
            return ExitCode::FAILURE;
        }
        if is_best {
            best = vm;
        }
        if plateau > 0
            && plateau_step(
                &mut plateau_lr,
                &mut stale,
                is_best,
                (plateau, plateau_factor, plateau_min),
            )
        {
            break;
        }
    }
    if best.is_finite() && !val.is_empty() {
        println!("best val {best:.4}");
    }
    ExitCode::SUCCESS
}

fn shuffle(v: &mut [Example], rand: &dyn Fn() -> u64) {
    for i in (1..v.len()).rev() {
        let j = (rand() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

struct EpochLine {
    epoch: usize,
    epochs: usize,
    train_mse: f64,
    seen: usize,
    secs: f32,
    best: f64,
}

/// Validates, prints the epoch's line, and returns the score selection runs on.
fn report_epoch(
    nn: &mut Nnue,
    val: &[Example],
    select_by: &str,
    table: bool,
    line: EpochLine,
    prof: Option<(f64, f64, f64)>,
) -> f64 {
    let t_v = Instant::now();
    let acc = val_by_stage(nn, None, val);
    if let Some((load, shuf, pass)) = prof {
        eprintln!(
            "train prof: load {load:.2}s shuffle {shuf:.2}s pass {pass:.2}s val {:.2}s",
            t_v.elapsed().as_secs_f64()
        );
    }
    let tot = pooled(&acc);
    let (vmse, vmae, vbias, vsd) = stats_of(&tot);
    let vgrid = tot[4] / tot[0].max(1.0);
    let vm = select_score(select_by, &tot);
    let marker = if vm < line.best { " *best" } else { "" };
    let EpochLine {
        epoch,
        epochs,
        train_mse,
        seen,
        secs,
        ..
    } = line;
    println!(
        "epoch {epoch:>2}/{epochs}: train {train_mse:.4}  val mse {vmse:.4} mae {vmae:.4} \
         bias {vbias:+.4} spread {vsd:.4} grid {vgrid:.3}{marker}  ({secs:.1}s, {:.0} pos/s)",
        seen as f32 / secs,
    );
    if table {
        print_stage_table(&acc);
    }
    vm
}

struct Sched {
    plateau_lr: f32,
    stale: usize,
    first_epoch: usize,
    virtual_done: usize,
}

fn size_files(files: &[PathBuf], limit: Option<usize>) -> Result<Vec<usize>, ()> {
    let mut c = Vec::with_capacity(files.len());
    for f in files {
        match count_examples_binary(f) {
            Ok(n) => c.push(limit.map_or(n, |l| n.min(l))),
            Err(e) => {
                eprintln!("cannot size {}: {e}", f.display());
                return Err(());
            }
        }
    }
    Ok(c)
}

fn resolve_patterns(
    file: Option<&Path>,
    which: &mut String,
    share: bool,
) -> Result<&'static [pattern::Pattern], String> {
    let Some(path) = file else {
        return pattern::resolve(which, None).map_err(|e| format!("{e} (nnue | linear)"));
    };
    let at = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| format!("patterns-file {at}: {e}"))?;
    let p = pattern::from_spec(&text, share).map_err(|e| format!("patterns-file {at}: {e}"))?;
    *which = at.to_string();
    Ok(p)
}

fn report_start_val(nn: &mut Nnue, val: &[Example], select_by: &str, table: bool) {
    let acc = val_by_stage(nn, None, val);
    let (m, a, b, sd) = stats_of(&pooled(&acc));
    println!(
        "starting val mse {m:.4} mae {a:.4} bias {b:+.4} spread {sd:.4}  \
         (selecting on {select_by})"
    );
    if table {
        print_stage_table(&acc);
    }
}

/// (first epoch, epochs treated as already run), or None when starting at 1.
fn enter_schedule(
    start_epoch: usize,
    epochs: usize,
    resuming: bool,
    lr: f32,
) -> Result<Option<(usize, usize)>, ()> {
    if start_epoch <= 1 {
        return Ok(None);
    }
    if resuming {
        eprintln!("--start-epoch and --resume both set the schedule's position; pick one");
        return Err(());
    }
    if start_epoch > epochs {
        eprintln!("--start-epoch {start_epoch} is past --epochs {epochs}");
        return Err(());
    }
    let done = start_epoch - 1;
    let f = 0.5 * (1.0 + (std::f32::consts::PI * done as f32 / epochs as f32).cos());
    println!(
        "entering the schedule at epoch {start_epoch}/{epochs}: lr {:.8} ({f:.3} of {lr:.8})",
        lr * f
    );
    Ok(Some((start_epoch, done)))
}

fn apply_resume(
    path: &Path,
    nn: &mut Nnue,
    ad: &mut AdamState,
    sched: &mut Sched,
    rng: &std::cell::Cell<u64>,
) -> Result<CkptHeader, ()> {
    let h = match read_checkpoint(path, nn, ad) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("resume {} failed: {e}", path.display());
            return Err(());
        }
    };
    sched.first_epoch = h.epoch + 1;
    sched.plateau_lr = h.plateau_lr;
    sched.stale = h.stale;
    rng.set(h.rng);
    println!(
        "resumed {} at epoch {} (next {}), best {:.4}, lr {:.8}",
        path.display(),
        h.epoch,
        sched.first_epoch,
        h.best,
        sched.plateau_lr
    );
    Ok(h)
}

fn epoch_plan(
    interleave: bool,
    counts: &[usize],
    max_examples: usize,
    rand: &dyn Fn() -> u64,
) -> Vec<Shard> {
    if interleave {
        let rot: Vec<usize> = counts
            .iter()
            .map(|&n| (rand() % n.max(1) as u64) as usize)
            .collect();
        return interleaved_shards(counts, &rot, max_examples);
    }
    let mut order: Vec<usize> = (0..counts.len()).collect();
    for i in (1..order.len()).rev() {
        let j = (rand() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    shards(counts, &order, max_examples)
}

fn save_epoch(
    nn: &Nnue,
    out: &Path,
    checkpoint: Option<&Path>,
    ad: Option<&AdamState>,
    h: &CkptHeader,
    is_best: bool,
) -> Result<(), ()> {
    if let Err(e) = nn.save(&out.with_extension("last.bin")) {
        eprintln!("save failed: {e}");
        return Err(());
    }
    if let (Some(path), Some(ad)) = (checkpoint, ad) {
        if let Err(e) = write_checkpoint(path, nn, ad, h) {
            eprintln!("checkpoint failed: {e}");
            return Err(());
        }
        if is_best {
            let b = path.with_extension("best.ckpt");
            if let Err(e) = std::fs::copy(path, &b) {
                eprintln!("keeping {}: {e}", b.display());
            }
        }
    }
    if is_best {
        if let Err(e) = nn.save(out) {
            eprintln!("save failed: {e}");
            return Err(());
        }
        println!("  saved {}", out.display());
    }
    Ok(())
}

/// True once the rate has reached its floor and training should stop.
fn plateau_step(
    plateau_lr: &mut f32,
    stale: &mut usize,
    is_best: bool,
    (plateau, factor, min): (usize, f32, f32),
) -> bool {
    if is_best {
        *stale = 0;
        return false;
    }
    *stale += 1;
    if *stale < plateau {
        return false;
    }
    *plateau_lr *= factor;
    *stale = 0;
    println!("  {plateau} epochs without a best -> lr {:.8}", *plateau_lr);
    if *plateau_lr < min {
        println!("  rate floor reached; stopping");
        return true;
    }
    false
}

#[cfg(test)]
mod shard_tests {
    use super::*;

    #[test]
    fn interleaving_gives_every_shard_a_slice_of_every_file() {
        let counts = [1_000usize, 250, 7, 400];
        let rot = [999usize, 0, 5, 123];
        let plan = interleaved_shards(&counts, &rot, 600);
        assert_eq!(plan.len(), 3);
        let mut seen = [vec![0u8; 1_000], vec![0; 250], vec![0; 7], vec![0; 400]];
        for shard in &plan {
            assert!(shard.examples <= 600);
            let mut n = 0;
            for &(i, start, len) in &shard.parts {
                assert!(start + len <= counts[i]);
                for c in &mut seen[i][start..start + len] {
                    *c += 1;
                }
                n += len;
            }
            assert_eq!(n, shard.examples);
            let big: usize = shard.parts.iter().filter(|p| p.0 == 0).map(|p| p.2).sum();
            assert!((333..=334).contains(&big));
        }
        assert!(seen.iter().flatten().all(|&c| c == 1));
    }
}
