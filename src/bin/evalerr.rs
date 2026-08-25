//! Score a model's static evaluation against ground-truth values.
//!
//! Takes a `.data` file written by `gen_exact` (17-byte records whose label
//! is the position's exact solved value, not a noisy game outcome) and
//! reports mean absolute error, RMS and the error distribution. This is the
//! honest measure of evaluation quality: the training corpus's own val set
//! scores agreement with final game scores, which carry ~11 disc² of
//! irreducible noise in early positions.
//!
//! Usage: evalerr [--nnue path] [--linear] <file.data>...
use kuroobi::evaluator::Evaluator;
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::{Board, Color};

fn main() {
    let mut nnue_path = String::from("weights/nnue-h16.bin");
    let mut linear = false;
    let mut no_mlp = false;
    let mut files: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--nnue" => nnue_path = it.next().unwrap_or(nnue_path),
            "--linear" => linear = true,
            // Zero the additive head, to separate what it contributes from
            // what the training recipe did.
            "--no-mlp" => no_mlp = true,
            other if other.starts_with("--") => panic!("unknown flag {other}"),
            other => files.push(other.to_string()),
        }
    }
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
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
    nn.quantize();
    let mut lin = Evaluator::new(EGAROUCID_PATTERNS);
    if linear {
        lin.load_weights(std::path::Path::new("weights/linear.bin"))
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
