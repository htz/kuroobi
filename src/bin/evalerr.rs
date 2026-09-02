//! Score a model's static evaluation against ground-truth values.
//!
//! Takes a `.data` file written by `gen_exact` (17-byte records whose label
//! is the position's exact solved value, not a noisy game outcome) and
//! reports mean absolute error, RMS and the error distribution. This is the
//! honest measure of evaluation quality: the training corpus's own val set
//! scores agreement with final game scores, which carry ~11 disc² of
//! irreducible noise in early positions.
//!
//! Usage: evalerr [--nnue path] [--linear] [--linear-path p] [--by-stage] <file.data>...
use kuroobi::evaluator::Evaluator;
use kuroobi::nnue::Nnue;
use kuroobi::pattern::{COMPACT_PATTERNS, EGAROUCID_PATTERNS, NNUE_PATTERNS};
use kuroobi::{Board, Color};

fn main() {
    let mut nnue_path = String::from("weights/nnue-h16.bin");
    let mut linear = false;
    let mut by_stage = false;
    let mut linear_path = String::from("weights/linear.bin");
    let mut no_mlp = false;
    let mut files: Vec<String> = Vec::new();
    let mut which = String::from("egaroucid");
    // Which of the three read-out precisions to score. The model is one
    // model; `quantize` is the only thing between them, so scoring both
    // says how much of the held-out error is the model and how much is the
    // conversion the search actually reads.
    let mut path = String::from("int");
    let mut head_f32 = false;
    let mut no_pw = false;
    let mut reps = 0usize;
    let mut act_units = kuroobi::nnue::ACT_UNITS;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--nnue" => nnue_path = it.next().unwrap_or(nnue_path),
            "--patterns" => which = it.next().unwrap_or(which),
            "--linear" => linear = true,
            // Break the error down by game stage. Both evaluators keep
            // per-stage parameters -- the linear one entirely so, the NNUE
            // in its read-out -- so a single pooled number can hide one
            // stage improving while another rots. A set drawn from one
            // empty count only ever reports one stage; use a set that
            // spans the game to see all 61.
            "--by-stage" => by_stage = true,
            // Score a linear weight file other than the deployed one, so a
            // training run's output can be measured without moving it into
            // place first.
            "--linear-path" => {
                linear = true;
                linear_path = it.next().unwrap();
            }
            // int = int8 transformer + int16 read-out (what the search uses)
            // f32 = the trained weights, unconverted
            "--precision" => path = it.next().unwrap_or(path),
            // Run the head's first layer in f32; see `Nnue::head_f32`.
            "--head-f32" => head_f32 = true,
            "--no-pw" => no_pw = true,
            // Time the leaf evaluation instead of scoring it: the same
            // positions, evaluated repeatedly, so the cost per eval is
            // comparable across models with different shapes.
            "--time" => reps = it.next().unwrap().parse().unwrap(),
            "--act-units" => act_units = it.next().unwrap().parse().unwrap(),
            // Zero the additive head, to separate what it contributes from
            // what the training recipe did.
            "--no-mlp" => no_mlp = true,
            other if other.starts_with("--") => panic!("unknown flag {other}"),
            other => files.push(other.to_string()),
        }
    }
    // A weight file belongs to the set it was trained on; loading it under
    // the wrong one is a size mismatch, not a silent misread.
    let patterns = match which.as_str() {
        "compact" => COMPACT_PATTERNS,
        "nnue" => NNUE_PATTERNS,
        "egaroucid" => EGAROUCID_PATTERNS,
        other => panic!("unknown pattern set {other}"),
    };
    let mut nn = Nnue::new(patterns);
    nn.load(std::path::Path::new(&nnue_path)).expect("nnue");
    if no_mlp {
        nn.set_mlp(
            &vec![0.0; 16 * kuroobi::nnue::H],
            &[0.0; 16],
            &[0.0; 16 * 16],
            &[0.0; 16],
            &vec![0.0; 61 * 16],
        );
    }
    if no_pw {
        nn.zero_pw();
    }
    nn.act_units = act_units;
    nn.quantize();
    if no_pw {
        nn.zero_pw();
    }
    nn.head_f32 = head_f32;
    {
        let (ft_s, w_s, ft_max) = nn.quant_scales();
        let (clipped, total) = nn.ft_clipped();
        eprintln!(
            "quant: ft_scale={ft_s} (max|ft|={ft_max:.3}, so {:.0} levels of {}), \
             out_scale={w_s:.0}, clipped {clipped}/{total} ({:.4}%)",
            ft_max * ft_s,
            127,
            clipped as f64 / total.max(1) as f64 * 100.0,
        );
    }
    let mut lin = Evaluator::new(EGAROUCID_PATTERNS);
    if linear {
        lin.load_weights(std::path::Path::new(&linear_path))
            .expect("linear weights");
    }

    for f in &files {
        let bytes = std::fs::read(f).expect("read data");
        // Records are fixed-width, so a file that is not a whole number of
        // them is truncated or misaligned, and dividing would silently score
        // whatever prefix happened to fit. Refuse instead: a scorer that
        // quietly measures a subset reads exactly like one that measured
        // everything.
        assert_eq!(
            bytes.len() % 17,
            0,
            "{f}: {} bytes is not a whole number of 17-byte records",
            bytes.len()
        );
        // A disc difference cannot exceed the board, so a label outside ±64
        // is the file being wrong, not the model. Cheap enough to check on
        // every run, and it is the check that would have caught the
        // sign-flipped labels years earlier than the mean did: the average
        // moved 0.37 while the worst case sat at 87.5, which is impossible.
        // `checkdata` is the thorough version (it re-solves every position).
        let impossible = (0..bytes.len() / 17)
            .filter(|i| (bytes[i * 17 + 16] as i8).unsigned_abs() > 64)
            .count();
        if impossible > 0 {
            eprintln!("{f}: WARNING {impossible} labels are outside +-64 discs; run checkdata");
        }
        let n = bytes.len() / 17;
        let mut sum = 0.0f64;
        let mut abs_sum = 0.0f64;
        let mut sq_sum = 0.0f64;
        let mut within1 = 0usize;
        let mut within3 = 0usize;
        let mut worst = 0.0f32;
        // [n, abs_sum, sq_sum, signed_sum] per stage.
        let mut per_stage = vec![[0.0f64; 4]; kuroobi::evaluator::STAGE_COUNT];
        for i in 0..n {
            let r = &bytes[i * 17..i * 17 + 17];
            let black = u64::from_le_bytes(r[0..8].try_into().unwrap());
            let white = u64::from_le_bytes(r[8..16].try_into().unwrap());
            let truth = r[16] as i8 as f32;
            let board = Board {
                black,
                white,
                player: Color::Black,
                empty_count: 64 - (black | white).count_ones() as u8,
            };
            let pred = if linear {
                let ix = lin.indexer().init(black, white);
                lin.eval_order_bb(board.player_bb(), board.opponent_bb(), Color::Black, &ix)
            } else if path == "f32" {
                nn.eval(&board)
            } else {
                let ix = nn.indices(black, white);
                nn.eval_from_indices(&ix, &board)
            };
            let e = pred - truth;
            sum += e as f64;
            abs_sum += e.abs() as f64;
            sq_sum += (e * e) as f64;
            if e.abs() <= 1.0 {
                within1 += 1;
            }
            if e.abs() <= 3.0 {
                within3 += 1;
            }
            if e.abs() > worst {
                worst = e.abs();
            }
            let st = kuroobi::evaluator::Evaluator::stage(&board);
            let acc = &mut per_stage[st];
            acc[0] += 1.0;
            acc[1] += e.abs() as f64;
            acc[2] += (e * e) as f64;
            acc[3] += e as f64;
        }
        if by_stage {
            println!("{f}: per-stage");
            println!("  stage  empties       n      MAE      RMS     bias");
            for (st, acc) in per_stage.iter().enumerate() {
                if acc[0] == 0.0 {
                    continue;
                }
                let k = acc[0];
                println!(
                    "  {:>5}  {:>7}  {:>6}  {:>7.3}  {:>7.3}  {:>+7.3}",
                    st,
                    60 - st,
                    k as u64,
                    acc[1] / k,
                    (acc[2] / k).sqrt(),
                    acc[3] / k
                );
            }
        }
        if reps > 0 {
            let boards: Vec<Board> = (0..n)
                .map(|i| {
                    let r = &bytes[i * 17..i * 17 + 17];
                    let black = u64::from_le_bytes(r[0..8].try_into().unwrap());
                    let white = u64::from_le_bytes(r[8..16].try_into().unwrap());
                    Board {
                        black,
                        white,
                        player: Color::Black,
                        empty_count: 64 - (black | white).count_ones() as u8,
                    }
                })
                .collect();
            // Indices built once, as a search holds them; what is timed is
            // the evaluation itself.
            let ixs: Vec<_> = boards
                .iter()
                .map(|b| nn.indices(b.black, b.white))
                .collect();
            let t = std::time::Instant::now();
            let mut sink = 0.0f32;
            for _ in 0..reps {
                for (b, ix) in boards.iter().zip(ixs.iter()) {
                    sink += nn.eval_from_indices(ix, b);
                }
            }
            let el = t.elapsed().as_secs_f64();
            let evals = (reps * n) as f64;
            println!(
                "{f}: {evals:.0} evals in {el:.3}s = {:.0} ns/eval ({:.2} M/s)  [{sink:.0}]",
                el / evals * 1e9,
                evals / el / 1e6,
            );
            continue;
        }
        let nf = n as f64;
        println!(
            "{f}: n={n}  MAE={:.3}  RMS={:.3}  bias={:+.3}  |e|<=1: {:.1}%  |e|<=3: {:.1}%  worst={worst:.1}",
            abs_sum / nf,
            (sq_sum / nf).sqrt(),
            sum / nf,
            within1 as f64 / nf * 100.0,
            within3 as f64 / nf * 100.0,
        );
    }
}
