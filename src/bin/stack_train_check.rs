//! Check that this model *trains* the way the numbers say, not just that it
//! evaluates that way.
//!
//! Forward agreement leaves the optimizer untested, and the optimizer is
//! where a clone quietly stops being one: bias correction, where the decay
//! is applied, the gradient clip, Lookahead. Same initial weights, same
//! batch, same number of steps, then compare the loss at every step and the
//! weights at the end.
//!
//! Weights are compared per table, since a disagreement confined to one of
//! them says where to look.

use kuroobi::board::Board;
use kuroobi::nnue::{AdamState, GradSink, Nnue};
use kuroobi::pattern::NNUE_PATTERNS;

fn read_f32s(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).expect("weights");
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn main() {
    let mut a = std::env::args().skip(1);
    let init = a
        .next()
        .expect("usage: nr_train_check <init> <cases> <losses> <final> [--lookahead]");
    let cases = a.next().expect("cases");
    let losses = a.next().expect("losses");
    let final_w = a.next().expect("final");
    let lookahead = a.any(|x| x == "--lookahead");

    const LR: f32 = 1e-3;
    // See the note in `tmp/table_dump_train.py`: zero here isolates everything
    // except the sparse/dense difference in how decay reaches untouched rows.
    let wd: f32 = std::env::var("WD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1e-2);

    let mut nn = Nnue::new(NNUE_PATTERNS);
    nn.load_reference_tables(std::path::Path::new(&init))
        .expect("init weights");

    let text = std::fs::read_to_string(&cases).expect("cases");
    let batch: Vec<(Board, f32)> = text
        .lines()
        .map(|l| {
            let mut it = l.split_whitespace();
            let p = u64::from_str_radix(it.next().unwrap(), 16).unwrap();
            let o = u64::from_str_radix(it.next().unwrap(), 16).unwrap();
            let t: f32 = it.next().unwrap().parse().unwrap();
            (
                Board {
                    black: p,
                    white: o,
                    player: kuroobi::color::Color::Black,
                    empty_count: 64 - (p | o).count_ones() as u8,
                },
                t,
            )
        })
        .collect();

    let want_loss: Vec<f32> = std::fs::read_to_string(&losses)
        .expect("losses")
        .lines()
        .map(|l| l.split_whitespace().nth(1).unwrap().parse().unwrap())
        .collect();

    let mut adam = AdamState::new(&nn);
    adam.wd = wd;
    if let Some(e) = std::env::var("EPS").ok().and_then(|v| v.parse().ok()) {
        adam.eps = e;
    }
    if lookahead {
        adam.set_lookahead(6, 0.5);
    }
    let mut sink = GradSink::new(1);

    let mut worst_loss = 0.0f32;
    let mut worst_step = 0;
    for (i, want) in want_loss.iter().enumerate() {
        sink.clear();
        let mut sq = 0.0f64;
        for (b, t) in &batch {
            let ix = nn.indices(b.black, b.white);
            let stage = kuroobi::evaluator::Evaluator::stage(b);
            let discs = b.black.count_ones() as usize;
            let mob = Nnue::mob_index(b);
            sq += nn.grad_black_into(&ix, stage, discs, mob, *t, &mut sink) as f64;
        }
        // `grad_black_into` returns the squared error in discs; the
        // reference's loss is the mean of the same thing on its /64 scale.
        let got = (sq / batch.len() as f64) as f32 / (64.0 * 64.0);
        let d = (got - want).abs() / want.abs().max(1e-6);
        if d > worst_loss {
            worst_loss = d;
            worst_step = i + 1;
        }
        let mut sinks = [std::mem::replace(&mut sink, GradSink::new(1))];
        nn.apply_adamw_batch(&mut sinks, &mut adam, LR, wd, 1.0 / batch.len() as f32);
        sink = std::mem::replace(&mut sinks[0], GradSink::new(1));
    }

    // Rows that went quiet still owe their decay; settle before comparing.
    nn.settle_adam(&mut adam, LR, wd);

    let want_final = read_f32s(&final_w);
    let names = [
        "ft", "ft_bias", "pa", "pa_bias", "so_l1_w", "so_l1_b", "so_l2_w", "so_l2_b", "so_out_w",
        "so_out_b",
    ];
    let got = nn.reference_tables();
    let mut off = 0;
    println!(
        "loss: worst relative difference {worst_loss:.6} at step {worst_step} of {}",
        want_loss.len()
    );
    for (name, t) in names.iter().zip(got.iter()) {
        let w = &want_final[off..off + t.len()];
        off += t.len();
        // Only cells the batch actually touched can agree: a dense
        // optimizer decays every row every step, a sparse one leaves
        // untouched rows alone. The tables that are dense on both sides are
        // the ones with a real comparison here.
        /* Two numbers, because one of them lies. A relative difference on a
        cell whose value is 1e-9 reads as 2.0 when the two sides land on
        opposite sides of zero, which is noise dressed up as a disagreement.
        The absolute difference says whether anything actually moved; the
        relative one is taken only over cells large enough for it to mean
        something. */
        let mut worst_abs = 0.0f32;
        let mut worst_rel = 0.0f32;
        let mut rel_at = 0.0f32;
        let mut moved = 0usize;
        for (g, wv) in t.iter().zip(w.iter()) {
            if *g == *wv {
                continue;
            }
            moved += 1;
            let a = (g - wv).abs();
            if a > worst_abs {
                worst_abs = a;
            }
            if wv.abs() > 1e-3 {
                let d = a / wv.abs();
                if d > worst_rel {
                    worst_rel = d;
                    rel_at = *wv;
                }
            }
        }
        println!(
            "{name:>9}: {moved:>9} cells differ, worst |diff| {worst_abs:.3e}, \
             worst relative {worst_rel:.3e} (on a cell of {rel_at:.4})"
        );
    }
}
