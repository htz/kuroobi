//! Trainer for the NNUE-style evaluator ([`kuroobi::nnue`]).
//!
//! Loads training records (`kuroobi::record`; the teacher value is the
//! game's final disc difference by the record's rule), runs SGD through
//! the network, and reports both the training MSE and a held-out val MSE
//! each epoch. The held-out MSE is the honest signal to compare against the
//! linear evaluator's ~39 floor.
//!
//! Usage:
//!   nnue_train [--epochs n] [--lr f] [--limit n] [--val f]... [--out path]
//!              [--select-by mse|mae|spread] [--val-by-stage] [--grid]
//!              [--min-ply n] [--max-score-diff d] [--drop-random]
//!              [--keep-above-ply n] [--search-value-to-ply n]
//!              [--checkpoint path] [--resume path] [--keep-every n]
//!              <data-file>...
//!
//! The four filter flags are `kuroobi::record::Filter` and apply to
//! training and held-out data alike; none of them is on by default, and the
//! filter in force is printed at startup. `Filter::TRAINING` is
//! `--min-ply 8 --max-score-diff 12 --drop-random --keep-above-ply 50`.
//!
//! `--search-value-to-ply n` reads the search value rather than the game's
//! final disc difference up to ply n. A record carries both, and neither is
//! right everywhere: against a perfect solve the game's result is 2.67 discs
//! off at 28 empties and 0.04 at 24, while a depth-4 search is 2.69 and 2.17.
//! The teacher in force is printed at startup alongside the filter.
//!
//! `--checkpoint <path>` writes weights, Adam moments and the loop's own
//! state after every epoch, and `--resume <path>` picks one up. `--init`
//! restores weights only, which is not the same thing: the moments start
//! from zero and the model is thrown off its converged point for an epoch
//! (+0.8 val at H=64, whatever the rate). A resumed run is
//! indistinguishable from an uninterrupted one -- 1+1 epochs against 2
//! straight through landed at val 173645.7401 and 173645.7407, inside the
//! 0.0047 two uninterrupted runs differ by. `--resume` keeps writing to
//! the file it came from unless `--checkpoint` says otherwise, and
//! `--keep-every n` also parks a copy every n epochs. One checkpoint of
//! the deployed shape is about 3.4 GB.
//!
//! `--select-by` chooses which held-out number keeps a snapshot in `--out`;
//! `<out>.last.bin` holds the weights after every epoch regardless. The default
//! stays `mse`; `spread` is the error with each stage's constant offset
//! removed, which is what move ordering sees. `--val-by-stage` prints the
//! breakdown behind those numbers, one row per stage. The `grid` column is
//! how far the engine's integer evaluation sits from the f32 model on the
//! same positions; `--grid` trains with the rounding the engine will apply
//! in the forward pass (see `Nnue::set_so_grid`).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use kuroobi::linear::{Linear, STAGE_COUNT};
use kuroobi::nnue::{AdamState, Nnue};
use kuroobi::pattern::{self, LINEAR_PATTERNS, NNUE_PATTERNS};
use kuroobi::record::{Filter, TeacherPolicy};
use kuroobi::trainer::{
    count_examples_binary, load_examples_filtered_into, load_examples_range_into, Example, SymPlan,
};

/// Per-stage `[count, sum_sq, sum_abs, sum_err]` over a held-out set.
///
/// The signed sum is what the pooled MSE has always thrown away, and it is
/// the half that decides how a model should be judged. A stage's mean error
/// is a constant offset on every position in that stage. Move ordering never
/// sees it -- the moves compared at a node are all one ply deeper, hence all
/// in the same stage, so a per-stage constant cancels in the argmax -- while
/// the spread around it is the part that can reorder moves.
///
/// Measured on the same positions, the H=64 network scores MAE 5.493 against
/// the linear model's 4.621 and loses on that number, yet it carries a +3.75
/// offset: take the offset out and its spread is 5.356 against 6.131, and it
/// wins 63-28 over the board. Ranked by spread, five evaluators came out in
/// exactly their head-to-head order; ranked by MAE, the strongest placed
/// fourth.
///
/// The offset is still worth removing, which is why it is reported rather
/// than discarded: a midgame score meets an exact endgame value at the
/// solver boundary, aspiration windows carry a bound across depths, and the
/// selective-search margins are calibrated in discs. All three compare
/// numbers a per-stage offset does move.
/// Scored through the quantized read-out, which is the one that plays.
///
/// `eval_indices` reads the f32 weights the optimizer is updating;
/// `eval_from_indices` reads the int8 tables `quantize` derives from them,
/// and that is what the search calls. The two are not close enough to
/// substitute: on the same positions H=16 scores MAE 3.937 in f32 and 8.798
/// after conversion, and it was chosen as the best snapshot on the first
/// number while playing on the second -- it lost to the linear evaluator
/// 41-54 at one ply. H=64 converts almost losslessly (5.556 -> 5.493), so
/// which path a run is judged on decides which of the two looks better.
///
/// `quantize` fills the integer tables from the current weights and leaves
/// the f32 side alone, so calling it here costs one pass over the weights
/// per epoch and does not disturb training.
fn val_by_stage(nn: &mut Nnue, base: Option<&Linear>, val: &[Example]) -> Vec<[f64; 5]> {
    nn.quantize();
    let mut acc = vec![[0.0f64; 5]; STAGE_COUNT];
    for ex in val {
        let board = ex.board();
        let ix = nn.indices(ex.black, ex.white);
        /* With a frozen base the model is the sum, so that is what is
        scored -- the net alone is a residual and its error against the
        label means nothing. */
        let b = base.map_or(0.0, |e| e.eval_indices(&board, &ix));
        // Prediction minus truth, so a positive mean reads as "this model
        // scores positions high".
        let q = nn.eval_from_indices(&ix, &board);
        let e = (q + b) as f64 - ex.score as f64;
        let a = &mut acc[Linear::stage(&board)];
        a[0] += 1.0;
        a[1] += e * e;
        a[2] += e.abs();
        a[3] += e;
        /* How far the integer path is from the f32 weights it was derived
        from. With the stacked read-out this is the grid the hidden layers
        were rounded onto: a stage whose weights all sit below one step
        rounds to nothing, and the engine then plays a different model
        from the one the loss was measured on. */
        a[4] += (q - nn.eval_indices(&board, &ix)).abs() as f64;
    }
    acc
}

/// `(mse, mae, bias, spread)` from one stage's sums, or from a pooled row.
fn stats_of(a: &[f64; 5]) -> (f64, f64, f64, f64) {
    let n = a[0];
    if n == 0.0 {
        return (f64::NAN, f64::NAN, f64::NAN, f64::NAN);
    }
    let (mse, mae, bias) = (a[1] / n, a[2] / n, a[3] / n);
    (mse, mae, bias, (mse - bias * bias).max(0.0).sqrt())
}

/// Sums over every stage, so the pooled numbers weigh a position once each.
fn pooled(acc: &[[f64; 5]]) -> [f64; 5] {
    let mut t = [0.0f64; 5];
    for a in acc {
        for (d, s) in t.iter_mut().zip(a) {
            *d += s;
        }
    }
    t
}

/// One row per stage that has validation positions: the error of the
/// f32 model at that stage and how far the integer path sits from it.
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

fn val_mse(nn: &mut Nnue, base: Option<&Linear>, val: &[Example]) -> f64 {
    if val.is_empty() {
        return f64::NAN;
    }
    stats_of(&pooled(&val_by_stage(nn, base, val))).0
}

/// How many examples may be resident at once. The full corpus is far larger
/// than RAM, so an epoch is run as a sequence of shards: whole files grouped
/// up to this budget, loaded, trained on, and dropped. Reading is a rounding
/// error next to the gradient work, so nothing is lost by not caching.
const DEFAULT_MAX_EXAMPLES: usize = 48_000_000;

/// A group of record ranges that fit in the budget together; each part is
/// `(file, first record, records)`.
struct Shard {
    parts: Vec<(usize, usize, usize)>,
    examples: usize,
}

/// Group whole files into shards no larger than `max` examples. `order`
/// varies which files share a shard between epochs, so a fixed grouping does
/// not turn into a fixed correlation in the data.
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

/// Cut every file into the same number of slices and build each shard from
/// one slice of every file, so every shard carries the corpus's mix rather
/// than that of the few files that happened to land in it. The files are
/// split by how many random opening plies their games have, which makes a
/// file's stage mix anything from full-range to endgame-only. Shuffling the
/// whole corpus onto disk first would avoid it too, at the cost of a second
/// copy of the data. `rot`
/// rotates each file's slice boundaries per epoch so shard membership keeps
/// changing; a slice that wraps past the end becomes two parts.
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

/// Synchronous minibatch AdamW: threads accumulate gradients into private
/// sinks, one optimizer step per batch. No Hogwild races, so Adam's moments
/// stay coherent (the async variant diverged).
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
        /* Hand the batch out in small pieces through a shared counter
        rather than splitting it into one equal part per thread.

        This machine is eight performance cores plus two efficiency cores,
        and an equal split gives the same count of examples to a core that
        runs them at roughly a third of the speed. Every batch then ends when
        the slowest share ends: measured, an equal ten-way split of two
        million examples took 2.1s where a seven-way split -- small enough to
        stay off the slow cores -- took 1.7s, against 11.1s on one core. Small
        pieces let each core take as many as it can finish. */
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
    // Hogwild: workers share the model (and the moments) through raw pointers.
    // Single-threaded runs use the same path; two update rules would
    // inevitably drift apart.
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
                /* Training-time symmetrization: draw one of the 8 forms
                per example (labels unchanged — rotation preserves value).
                Unlike post-hoc averaging this improves the fit while
                staying symmetric. Per-thread seeds vary the draw. */
                let mut rs = sym.stream(ti as u64 + 1);
                for ex in part {
                    let ex = sym.apply(ex, &mut rs);
                    let ex = &ex;
                    let board = ex.board();
                    let stage = Linear::stage(&board);
                    // Examples are normalized to Black to move, so the
                    // mover's disc count = Black's.
                    let discs = ex.black.count_ones() as usize;
                    let mob = kuroobi::nnue::Nnue::mob_index(&board);
                    let ix = nn_ref.indices(ex.black, ex.white);
                    // SAFETY: `view` / `av` are from `nn` and its moments,
                    // both borrowed immutably here.
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

/// Everything a resumed run needs that is neither a weight nor a moment.
///
/// Kept as plain `key value` lines at the head of the file so that
/// `head -c 512 run.ckpt` says what a 5 GB blob is. Unknown keys are
/// ignored and missing ones keep their default, so a checkpoint written by
/// an older build still resumes -- the alternative, refusing it, throws
/// away the run this whole feature exists to protect.
#[derive(Default)]
struct CkptHeader {
    epoch: usize,
    mb_step: u64,
    mb_total_steps: u64,
    plateau_lr: f32,
    stale: usize,
    best: f64,
    rng: u64,
    swa_n: usize,
}

/// Magic and version. Bumped only when the *layout* changes; a new header
/// key does not need it, because unknown keys are skipped.
const CKPT_MAGIC: &[u8; 8] = b"BBRVCK01";
/// The header is a fixed-size block so the payload starts at a known
/// offset and the file can be inspected without parsing anything.
const CKPT_HEADER_LEN: usize = 512;

/// Write weights, moments and loop state as one file.
///
/// One file, not three: a resume has to pair the moments with the exact
/// weights they were computed against, and three paths can be updated
/// out of step. Written to `.part` and renamed, because the machine dying
/// mid-write is the event this guards against, and a half-written
/// checkpoint that looks complete is worse than none.
fn write_checkpoint(
    path: &std::path::Path,
    nn: &Nnue,
    adam: &AdamState,
    h: &CkptHeader,
    swa: Option<&(Vec<f32>, usize)>,
) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("part");
    {
        let f = std::fs::File::create(&tmp)?;
        let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
        let text = format!(
            "epoch {}\nmb_step {}\nmb_total_steps {}\nplateau_lr {}\nstale {}\nbest {}\n\
             rng {}\nswa_n {}\n",
            h.epoch, h.mb_step, h.mb_total_steps, h.plateau_lr, h.stale, h.best, h.rng, h.swa_n
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
        // The weights inline, in `.bin` format, so the checkpoint is
        // self-contained and can be read by anything that reads weights.
        let mut bin: Vec<u8> = Vec::new();
        nn.write_to(&mut bin)?;
        w.write_all(&(bin.len() as u64).to_le_bytes())?;
        w.write_all(&bin)?;
        adam.write_state(&mut w)?;
        let (acc, _) = match swa {
            Some((a, n)) => (a.as_slice(), *n),
            None => (&[][..], 0),
        };
        w.write_all(&(acc.len() as u64).to_le_bytes())?;
        for x in acc {
            w.write_all(&x.to_le_bytes())?;
        }
        w.flush()?;
    }
    std::fs::rename(&tmp, path)
}

/// Read back what [`write_checkpoint`] wrote.
fn read_checkpoint(
    path: &std::path::Path,
    nn: &mut Nnue,
    adam: &mut AdamState,
) -> std::io::Result<(CkptHeader, Vec<f32>)> {
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
            "swa_n" => h.swa_n = v.parse().unwrap_or(0),
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
    r.read_exact(&mut u8b)?;
    let n = u64::from_le_bytes(u8b) as usize;
    let mut swa = vec![0.0f32; n];
    let mut b4 = [0u8; 4];
    for x in swa.iter_mut() {
        r.read_exact(&mut b4)?;
        *x = f32::from_le_bytes(b4);
    }
    Ok((h, swa))
}

fn main() -> ExitCode {
    let mut epochs = 10usize;
    let mut lr = 0.02f32;
    let mut decay = 1.0f32;
    let mut cosine = false;
    let mut plateau = 0usize;
    // Which held-out number decides the best snapshot, and whether to print
    // the per-stage breakdown. See `val_by_stage`.
    let mut select_by = String::from("mse");
    let mut val_by_stage_report = false;
    let mut plateau_factor = 0.5f32;
    let mut plateau_min = 1e-6f32;
    let mut wd = 0.0f32;
    let mut minibatch = 0usize;
    let mut adam = false;
    let mut sym_train = false;
    let mut sym_all = false;
    // Training on the engine's integer grid (see `Nnue::set_so_grid`). Off
    // by default: measured against the plain f32 run under the same
    // conditions (4 epochs, 25.5M positions) it changed nothing -- val MSE
    // 43.23 against 43.30, grid 0.096 against 0.102 discs -- because the
    // rounding error the grid adds to a layer that sits inside its clamps
    // is already small; the disc-sized disagreements came from stages whose
    // hidden layers had collapsed, and those were an initialisation fault.
    let mut so_grid = false;
    let mut lookahead = 0u32;
    let mut legacy_optimizer = false;
    // Run the optimizer step on the GPU (`gpu` feature); see `nnue::gpu`.
    let mut gpu = false;
    let mut fit_num = false;
    let mut fit_lambda = 1000.0f64;
    let mut swa_from = 0usize;
    let mut threads = 1usize;
    let mut limit: Option<usize> = None;
    let mut filter = Filter::NONE;
    // The corpus carries both a search value and the game's result; which one
    // teaches which stage is a decision for the run, not for the data.
    let mut policy = TeacherPolicy::DEFAULT;
    let mut out = PathBuf::from("weights/nnue.bin");
    let mut val_files: Vec<PathBuf> = Vec::new();
    let mut data_files: Vec<PathBuf> = Vec::new();
    let mut max_examples = DEFAULT_MAX_EXAMPLES;
    let mut interleave = false;
    let mut val_cap: Option<usize> = None;
    let mut init: Option<PathBuf> = None;
    /* Where to leave a resumable state, and where to pick one up.
    Separate from `--init`, which restores weights and nothing else: a run
    restarted that way begins with zeroed Adam moments, and the first epoch
    after such a restart cost +0.8 val at H=64 whatever the rate. */
    let mut checkpoint: Option<PathBuf> = None;
    let mut resume: Option<PathBuf> = None;
    /* Keep every Nth epoch's checkpoint under its own name. Off by
    default: one of these is the weights plus two moments plus a slow copy,
    which for the deployed shape is about 5 GB. */
    let mut keep_every = 0usize;
    let mut which_patterns = String::from("nnue");
    let mut patterns_file: Option<PathBuf> = None;
    let mut patterns_share = false;
    /* A frozen linear linear under the net.

    The net has to spend capacity learning the level of the score before it
    can learn its shape, and it does that badly: over eleven epochs the bias
    on val61 went +1.34, -0.61, -0.02, -0.70, -2.04 while the spread fell
    steadily, so the run kept discarding weights that were better shaped
    because the level had drifted. Stockfish's answer is a PSQT column read
    straight out of the feature transformer to the output, added because
    "nets have a hard time learning high material imbalance, or even
    representing high evaluations at all".

    Here that column already exists as a trained model: the deployed pattern
    linear, which reads the same rows from the same `PatternIndices` this
    net computes. Frozen under the net, it fixes the level, and the net is
    trained on what is left over. It also floors the result -- a stage the
    net cannot learn (stage 4 has 282 training positions) still gets the
    linear linear's answer rather than noise. */
    let mut base_path: Option<PathBuf> = None;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match filter.take_flag(&a, &mut it) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
        match a.as_str() {
            "--patterns" => which_patterns = it.next().unwrap(),
            // A spec file carries one base mask per shape; the eight
            // symmetries are generated on load. Racing a few dozen
            // candidate sets needs this -- one `const` each would be
            // a recompile per candidate.
            "--patterns-file" => patterns_file = Some(PathBuf::from(it.next().unwrap())),
            // Orientations share one table. Cuts the rows 4-8x, so it
            // is never the default.
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
            /* Every example in all eight forms each epoch, instead of one
            drawn at random. Eight times the work per epoch, so the epoch
            count has to come down to match. */
            "--sym-all" => sym_all = true,
            "--grid" => so_grid = true,
            // Wrap AdamW in Lookahead(k=6, alpha=0.5). It rewrites every
            // weight every k steps, which on a CPU is not free -- hence a
            // flag rather than always on.
            "--lookahead" => lookahead = 6,
            // Reproduce the optimizer as it was before this recipe; see
            // `AdamState::legacy_optimizer`.
            "--legacy-optimizer" => legacy_optimizer = true,
            "--gpu" => gpu = true,
            "--fit-num" => fit_num = true,
            "--fit-num-lambda" => fit_lambda = it.next().unwrap().parse().unwrap(),
            "--swa" => swa_from = it.next().unwrap().parse().unwrap(),
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
            "--keep-every" => keep_every = it.next().unwrap().parse().unwrap(),
            "--base" => base_path = Some(PathBuf::from(it.next().unwrap())),
            other if other.starts_with('-') => {
                eprintln!("unknown option {other}");
                return ExitCode::FAILURE;
            }
            file => data_files.push(PathBuf::from(file)),
        }
    }
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
        "train {} records in {} files (shard budget {}) / val {} / filter {} / teacher {}",
        total,
        data_files.len(),
        max_examples,
        val.len(),
        filter.describe(),
        policy.describe()
    );

    /* A weight file belongs to the pattern set it was trained on -- the
    feature space is a different size and a different shape -- so `--init`
    across sets cannot work and is not worth a fallback. */
    let patterns = match &patterns_file {
        Some(path) => {
            let text = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("patterns-file {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            match pattern::from_spec(&text, patterns_share) {
                Ok(p) => {
                    which_patterns = path.display().to_string();
                    p
                }
                Err(e) => {
                    eprintln!("patterns-file {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
            }
        }
        None => match which_patterns.as_str() {
            "nnue" => NNUE_PATTERNS,
            "linear" => LINEAR_PATTERNS,
            other => {
                eprintln!("unknown pattern set {other} (nnue | linear)");
                return ExitCode::FAILURE;
            }
        },
    };
    let base = match &base_path {
        Some(p) => {
            let mut e = Linear::new(patterns);
            if let Err(err) = e.load_weights(p) {
                eprintln!("base {}: {err}", p.display());
                return ExitCode::FAILURE;
            }
            println!("base: frozen linear linear from {}", p.display());
            Some(e)
        }
        None => None,
    };

    /* A run resumed without `--checkpoint` would stop being resumable at
    the moment it was resumed, which is the opposite of what was asked
    for. Keep writing to the file it came from. */
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
    if resume.is_some() && gpu {
        eprintln!("--resume does not cover the GPU trainer's own moments");
        return ExitCode::FAILURE;
    }
    let mut nn = Nnue::new(patterns);
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
    /* Training never carries a ProbCut sigma forward. Even one gradient
    step makes the model evaluate differently, so the margins measured for
    the model `--init` came from no longer describe this one; inheriting
    them would leave a file that claims to be calibrated and is not.
    `nnue_mpccalib` is the only writer. */
    nn.set_mpc_sigma(None);
    nn.set_so_grid(so_grid);
    println!(
        "nnue: patterns={which_patterns} masks={} H={} features={}",
        patterns.iter().map(|p| p.masks.len()).sum::<usize>(),
        kuroobi::nnue::H,
        nn.n_features()
    );

    /* Fit the disc-count table in closed form: with the network frozen
    the optimum per bucket is its mean residual — exact, fast, and it
    bounds the achievable gain. Joint training barely moved (-0.007). */
    if fit_num {
        let n_buckets = nn.num_w_len();
        let mut sum = vec![0.0f64; n_buckets];
        let mut cnt = vec![0u64; n_buckets];
        // Zero the table before counting residuals (no double-adding).
        nn.set_num_w(&vec![0.0f32; n_buckets]);
        for (fi, f) in data_files.iter().enumerate() {
            let mut ex = Vec::new();
            if let Err(e) = load_examples_filtered_into(f, &mut ex, limit, &filter, &policy) {
                eprintln!("load failed: {e}");
                return ExitCode::FAILURE;
            }
            for e in &ex {
                let board = e.board();
                let stage = Linear::stage(&board);
                let discs = e.black.count_ones() as usize;
                let ix = nn.indices(e.black, e.white);
                let r = e.score as f64 - nn.eval_indices(&board, &ix) as f64;
                let b = stage * 65 + discs;
                sum[b] += r;
                cnt[b] += 1;
            }
            eprintln!("  fit-num: {}/{} files", fi + 1, data_files.len());
        }
        /* Shrink sparse buckets toward zero (`--fit-num-lambda`): a raw
        mean lets a 1-example bucket adopt its full residual (110 discs!)
        and worsen val. `sum / (cnt + LAMBDA)` is ridge shrinkage. */
        let table: Vec<f32> = sum
            .iter()
            .zip(&cnt)
            .map(|(s, c)| (*s / (*c as f64 + fit_lambda)) as f32)
            .collect();
        let filled = cnt.iter().filter(|&&c| c > 0).count();
        let mx = table.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        println!("fit-num: {filled}/{n_buckets} buckets, max |correction| {mx:.3} discs");
        nn.set_num_w(&table);
        let vm = val_mse(&mut nn, base.as_ref(), &val);
        println!("fit-num: val {vm:.4}");
        if let Err(e) = nn.save(&out) {
            eprintln!("save failed: {e}");
            return ExitCode::FAILURE;
        }
        println!("  saved {}", out.display());
        return ExitCode::SUCCESS;
    }

    /* Adam moments cost two weight copies (78 MB at H=16); allocate
    only on request. */
    let mut sinks: Vec<kuroobi::nnue::GradSink> = Vec::new();
    let mut mb_step: u64 = 0;
    let base_lr = lr;
    // Total optimizer steps for the cosine sweep (estimated from the first
    // epoch's example count; refined after epoch 1).
    let mut mb_total_steps: u64 = 0;
    let mut adam_state = adam.then(|| {
        println!(
            "adam: moments {} MB",
            (nn.ft_len() + STAGE_COUNT * kuroobi::nnue::H) * 2 * 4 / 1_000_000
        );
        let mut st = AdamState::new(&nn);
        st.wd = wd;
        st.legacy_optimizer = legacy_optimizer;
        if legacy_optimizer {
            println!("adam: legacy (no bias correction, zero-gradient cells skipped)");
        }
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
    #[cfg(feature = "gpu")]
    let mut gpu_trainer = gpu.then(|| {
        if base.is_some() {
            eprintln!("--gpu does not take --base");
            std::process::exit(2);
        }
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

    let mut swa_sum: Option<(Vec<f32>, usize)> = None;

    /* The shuffle's generator, in a cell rather than captured by value, so
    a checkpoint can record where it had got to. Resuming with a fresh
    generator would re-walk the shard order the run already used, which is
    not the same run continued. */
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
        select_score(
            &select_by,
            &pooled(&val_by_stage(&mut nn, base.as_ref(), &val)),
        )
    };
    if best.is_finite() {
        let acc = val_by_stage(&mut nn, base.as_ref(), &val);
        let (m, a, b, sd) = stats_of(&pooled(&acc));
        println!(
            "starting val mse {m:.4} mae {a:.4} bias {b:+.4} spread {sd:.4}  \
             (selecting on {select_by})"
        );
        // The same breakdown as after each epoch, so the first epoch's
        // table has something to be read against.
        if val_by_stage_report {
            print_stage_table(&acc);
        }
    }
    /* `--plateau n`: hold the rate while the model improves and halve it
    after n epochs without a new best, all inside one process.

    The same ladder run as a shell loop -- train, stop, restart from the
    best weights at half the rate -- measurably does not work, because
    `--init` restores weights but not Adam's moments. Rebuilding them
    from zero throws the model off its converged point at the start of
    every restart: at H=64 the first epoch after a restart cost +0.8
    regardless of whether the rate was 0.003 or 0.0015, so halving the
    rate bought nothing. Lowering it in place keeps the moments and the
    step size falls without the model being moved first. */
    let mut plateau_lr = lr;
    let mut stale = 0usize;
    let mut first_epoch = 1usize;
    if let Some(path) = &resume {
        let Some(ad) = adam_state.as_mut() else {
            eprintln!("--resume needs --adam");
            return ExitCode::FAILURE;
        };
        match read_checkpoint(path, &mut nn, ad) {
            Ok((h, swa)) => {
                first_epoch = h.epoch + 1;
                mb_step = h.mb_step;
                mb_total_steps = h.mb_total_steps;
                plateau_lr = h.plateau_lr;
                stale = h.stale;
                best = h.best;
                rng_state.set(h.rng);
                if h.swa_n > 0 && !swa.is_empty() {
                    swa_sum = Some((swa, h.swa_n));
                }
                nn.set_so_grid(so_grid);
                println!(
                    "resumed {} at epoch {} (next {first_epoch}), best {best:.4}, lr {plateau_lr:.8}",
                    path.display(),
                    h.epoch
                );
                if first_epoch > epochs {
                    println!("nothing left to do: --epochs {epochs} already reached");
                    return ExitCode::SUCCESS;
                }
            }
            Err(e) => {
                eprintln!("resume {} failed: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }
    for epoch in first_epoch..=epochs {
        let t = Instant::now();
        // Cosine annealing over the run (`--cosine`): lr0 -> ~0 in one sweep,
        // replacing hand-tuned lr ladders. Otherwise geometric `--decay`.
        if minibatch > 0 && epoch == 1 && mb_total_steps == 0 {
            // Not yet known; provisional value so the first epoch's lr stays
            // near base_lr (cos(0)=1) until seen==total is measured.
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

        // Fresh file order (or slice rotation) each epoch, so shard
        // membership keeps changing.
        let plan = if interleave {
            let rot: Vec<usize> = counts
                .iter()
                .map(|&n| (rand() % n.max(1) as u64) as usize)
                .collect();
            interleaved_shards(&counts, &rot, max_examples)
        } else {
            let mut order: Vec<usize> = (0..data_files.len()).collect();
            for i in (1..order.len()).rev() {
                let j = (rand() % (i as u64 + 1)) as usize;
                order.swap(i, j);
            }
            shards(&counts, &order, max_examples)
        };

        let mut sq_total = 0.0f64;
        let mut seen = 0usize;
        for (si, shard) in plan.iter().enumerate() {
            let mut examples = match load_parts(&shard.parts) {
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
            let sym_seed = if sym_train { rand() | 1 } else { 0 };
            /* `--sym-all` trains every example in all eight forms. They are
            separate passes, not eight copies inside one minibatch: a batch
            holding one position eight times over correlates its own
            gradient, and the pass structure leaves batch size, step count
            and memory exactly as they were. */
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
                    // A fresh order per form, or the eight passes present
                    // the same sequence of positions eight times.
                    for i in (1..examples.len()).rev() {
                        let j = (rand() % (i as u64 + 1)) as usize;
                        examples.swap(i, j);
                    }
                }
                rows += examples.len();
                sq += if minibatch > 0 {
                    let ad = adam_state
                        .as_mut()
                        .expect("--minibatch requires --adam (moments)");
                    // Per-step cosine inside the shard as well, indexed by the
                    // global optimizer step so the sweep stays smooth.
                    let mut lr_fn = || {
                        let lr = if cosine {
                            let t = mb_step as f32 / mb_total_steps.max(1) as f32;
                            // The schedule floors at 1e-8 rather than at zero.
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
                        // The next shard only needs the indexer, but val and the
                        // save need the tables.
                        if si + 1 == plan.len() {
                            g.download(&mut nn);
                        }
                        sq
                    } else {
                        train_pass_minibatch(
                            &mut nn,
                            base.as_ref(),
                            ad,
                            &mut sinks,
                            &examples,
                            threads,
                            minibatch,
                            &mut lr_fn,
                            wd,
                            sym,
                        )
                    }
                    #[cfg(not(feature = "gpu"))]
                    train_pass_minibatch(
                        &mut nn,
                        base.as_ref(),
                        ad,
                        &mut sinks,
                        &examples,
                        threads,
                        minibatch,
                        &mut lr_fn,
                        wd,
                        sym,
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
        }
        /* SWA: average points orbiting at a constant lr. Averaging a
        chain of decaying-lr points just drags toward stale weights
        (31.0437 worsened to 31.0505); only same-lr epoch weights
        average toward the center. `--swa N` includes epochs from N. */
        if swa_from > 0 && epoch >= swa_from {
            let w = nn.weights_flat();
            match &mut swa_sum {
                None => swa_sum = Some((w, 1usize)),
                Some((acc, n)) => {
                    for (a, x) in acc.iter_mut().zip(&w) {
                        *a += x;
                    }
                    *n += 1;
                }
            }
        }

        if minibatch > 0 && epoch == 1 {
            // First epoch measured the real step count; pin the cosine sweep
            // to the remaining schedule.
            mb_total_steps = mb_step * epochs as u64;
        }
        let train_mse = sq_total / seen.max(1) as f64;
        let acc = val_by_stage(&mut nn, base.as_ref(), &val);
        let tot = pooled(&acc);
        let (vmse, vmae, vbias, vsd) = stats_of(&tot);
        let vgrid = tot[4] / tot[0].max(1.0);
        let vm = select_score(&select_by, &tot);
        let is_best = vm < best;
        let marker = if is_best { " *best" } else { "" };
        println!(
            "epoch {epoch:>2}/{epochs}: train {train_mse:.4}  val mse {vmse:.4} mae {vmae:.4} \
             bias {vbias:+.4} spread {vsd:.4} grid {vgrid:.3}{marker}  ({:.1}s, {:.0} pos/s)",
            t.elapsed().as_secs_f32(),
            seen as f32 / t.elapsed().as_secs_f32(),
        );
        if val_by_stage_report {
            print_stage_table(&acc);
        }
        /* Settle the deferred decay before any save. The sparse update
        leaves a row's weight decay owing until the row is next touched,
        which costs nothing during training and is wrong in a file: rows
        that went quiet early would be saved holding a value the schedule
        had long since shrunk.

        Applies to every shape. It was held back from the shipped one at
        first, on the theory that its tuning depended on the sparse update
        as it stood; that theory was tested and wrong -- removing the
        settling changed the shipped model's held-out error by 0.06 discs,
        well inside the run-to-run spread. */
        // The GPU steps every row every batch; nothing is owed.
        if let Some(ad) = adam_state.as_mut().filter(|_| !gpu) {
            nn.settle_adam(ad, cur_lr, wd);
        }
        /* The weights as they stand go to `<out>.last.bin` every epoch,
        whether or not val improved. A run that never beats its starting
        val used to leave nothing on disk, so stopping it -- to change the
        data, the rate, anything -- threw away every epoch it had run
        (three epochs, a night, once). `--out` itself still holds only the
        best-by-val model, since val overfits after a few epochs. */
        let last = out.with_extension("last.bin");
        if let Err(e) = nn.save(&last) {
            eprintln!("save failed: {e}");
            return ExitCode::FAILURE;
        }
        /* The checkpoint goes out after the weights, so a crash between
        the two leaves a checkpoint one epoch behind rather than one that
        claims an epoch it does not hold. */
        if let (Some(path), Some(ad)) = (&checkpoint, adam_state.as_ref()) {
            let h = CkptHeader {
                epoch,
                mb_step,
                mb_total_steps,
                plateau_lr,
                stale,
                best: if is_best { vm } else { best },
                rng: rng_state.get(),
                swa_n: swa_sum.as_ref().map_or(0, |(_, n)| *n),
            };
            if let Err(e) = write_checkpoint(path, &nn, ad, &h, swa_sum.as_ref()) {
                eprintln!("checkpoint failed: {e}");
                return ExitCode::FAILURE;
            }
            if keep_every > 0 && epoch.is_multiple_of(keep_every) {
                let kept = path.with_extension(format!("e{epoch}.ckpt"));
                if let Err(e) = std::fs::copy(path, &kept) {
                    eprintln!("keeping {}: {e}", kept.display());
                }
            }
        }
        if is_best {
            best = vm;
            if let Err(e) = nn.save(&out) {
                eprintln!("save failed: {e}");
                return ExitCode::FAILURE;
            }
            println!("  saved {}", out.display());
        }
        if plateau > 0 {
            if is_best {
                stale = 0;
            } else {
                stale += 1;
                if stale >= plateau {
                    plateau_lr *= plateau_factor;
                    stale = 0;
                    println!("  {plateau} epochs without a best -> lr {plateau_lr:.8}");
                    if plateau_lr < plateau_min {
                        println!("  rate floor reached; stopping");
                        break;
                    }
                }
            }
        }
    }
    // Evaluate the average itself; if it beats the points, it wins.
    if let Some((acc, n)) = swa_sum {
        let mean: Vec<f32> = acc.iter().map(|x| x / n as f32).collect();
        let mut avg = Nnue::new(patterns);
        avg.set_weights_flat(&mean);
        let vm = val_mse(&mut avg, base.as_ref(), &val);
        println!("swa over {n} epochs: val {vm:.4}");
        /* Always save the average: against a symmetrized baseline the
        raw SWA average looks worse by its asymmetry (0.006-0.008), and
        save-on-improve would silently discard averages that win after
        `nnue_symmetrize`. Decide after symmetrizing. */
        let p = out.with_extension("swa.bin");
        if let Err(e) = avg.save(&p) {
            eprintln!("save failed: {e}");
            return ExitCode::FAILURE;
        }
        println!("  saved {} (symmetrize before judging)", p.display());
        if vm < best {
            best = vm;
        }
    }
    if best.is_finite() && !val.is_empty() {
        println!("best val {best:.4}");
    }
    ExitCode::SUCCESS
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
            // The big files land in every shard, in corpus proportion.
            let big: usize = shard.parts.iter().filter(|p| p.0 == 0).map(|p| p.2).sum();
            assert!((333..=334).contains(&big));
        }
        // Every record exactly once per epoch.
        assert!(seen.iter().flatten().all(|&c| c == 1));
    }
}
