//! Give a trained model one transformer copy per slice of the game, by
//! replicating the copy it already has.
//!
//! The replicated model evaluates *identically* to its source -- every stage
//! still reads the same numbers, they are just stored once per bucket now --
//! so fine-tuning starts at the source's accuracy rather than from scratch,
//! and any movement afterwards is the buckets specialising. This is the same
//! trick `widen_h` plays on the accumulator width, applied to depth of table
//! instead of width of row.
//!
//! Build it with the bucket count you want and point it at a model that has
//! fewer:
//!   cargo build --release --features ftb4 --target-dir target-ftb4 --bin bucketize
//!   ./target-ftb4/release/bucketize --in h32.bin --out h32-ftb4.bin
//!
//! Usage: bucketize --in <model.bin> --out <model.bin>
use kuroobi::nnue::{Nnue, FT_BUCKETS, H};
use kuroobi::pattern::EGAROUCID_PATTERNS;
use std::io::Read;
use std::path::PathBuf;

fn read_f32s(r: &mut impl Read, n: usize) -> std::io::Result<Vec<f32>> {
    let mut v = vec![0f32; n];
    let mut b = [0u8; 4];
    for x in v.iter_mut() {
        r.read_exact(&mut b)?;
        *x = f32::from_le_bytes(b);
    }
    Ok(v)
}

fn main() {
    let mut inp: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--in" => inp = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            other => panic!("unknown arg {other}"),
        }
    }
    let inp = inp.expect("--in required");
    let out = out.expect("--out required");

    let mut r = std::io::BufReader::new(std::fs::File::open(&inp).expect("open"));
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic).unwrap();
    let (staged_bias, has_num, has_pw, has_mob, has_mlp) = match &magic {
        b"BBRVNN06" => (true, true, true, true, true),
        b"BBRVNN05" => (true, true, true, true, false),
        b"BBRVNN04" => (true, true, true, false, false),
        b"BBRVNN03" => (true, true, false, false, false),
        b"BBRVNN02" => (false, true, false, false, false),
        b"BBRVNN01" => (false, false, false, false, false),
        m => panic!("unsupported magic {m:?}"),
    };
    let mut u = [0u8; 4];
    r.read_exact(&mut u).unwrap();
    let h_file = u32::from_le_bytes(u) as usize;
    assert_eq!(h_file, H, "this build is H={H}, the file is H={h_file}");
    r.read_exact(&mut u).unwrap();
    let n_features_file = u32::from_le_bytes(u) as usize;
    r.read_exact(&mut u).unwrap();
    let stages = u32::from_le_bytes(u) as usize;

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    let per_bucket = nn.n_features() / FT_BUCKETS;
    assert_eq!(
        n_features_file % per_bucket,
        0,
        "file has {n_features_file} feature rows, not a multiple of this \
         pattern set's {per_bucket} per copy"
    );
    let buckets_file = n_features_file / per_bucket;
    assert!(
        FT_BUCKETS > buckets_file,
        "this build has {FT_BUCKETS} copies and the file already has \
         {buckets_file}; nothing to replicate"
    );
    assert_eq!(
        FT_BUCKETS % buckets_file,
        0,
        "{FT_BUCKETS} copies is not a whole multiple of the file's {buckets_file}"
    );

    let ft = read_f32s(&mut r, n_features_file * H).unwrap();
    let ft_bias = if staged_bias {
        read_f32s(&mut r, stages * H).unwrap()
    } else {
        let one = read_f32s(&mut r, H).unwrap();
        let mut v = vec![0f32; stages * H];
        for st in 0..stages {
            v[st * H..(st + 1) * H].copy_from_slice(&one);
        }
        v
    };
    let out_w = read_f32s(&mut r, stages * H).unwrap();
    let out_b = read_f32s(&mut r, stages).unwrap();
    let num_w = if has_num {
        read_f32s(&mut r, stages * 65).unwrap()
    } else {
        vec![0f32; stages * 65]
    };
    let pw = if has_pw {
        read_f32s(&mut r, stages * (H / 2)).unwrap()
    } else {
        vec![0f32; stages * (H / 2)]
    };
    let mob_w = if has_mob {
        read_f32s(&mut r, stages * 24).unwrap()
    } else {
        vec![0f32; stages * 24]
    };
    let (l1_w, l1_b, l2_w, l2_b, mlp_out) = if has_mlp {
        let a = read_f32s(&mut r, 16 * H).unwrap();
        let b = read_f32s(&mut r, 16).unwrap();
        let c = read_f32s(&mut r, 16 * 16).unwrap();
        let d = read_f32s(&mut r, 16).unwrap();
        let e = read_f32s(&mut r, stages * 16).unwrap();
        (a, b, c, d, e)
    } else {
        (
            vec![0f32; 16 * H],
            vec![0f32; 16],
            vec![0f32; 16 * 16],
            vec![0f32; 16],
            vec![0f32; stages * 16],
        )
    };

    /* Every new copy is an exact duplicate -- no perturbation.

    Widening the accumulator needs jitter because duplicated *lanes* sit side
    by side in one sum and would otherwise receive identical gradients for
    ever. Copies here never meet: a position reads exactly one of them, so
    each already sees a different slice of the data and diverges on its own
    from the first step. Jitter would only cost accuracy at the start. */
    let mut ft_new = vec![0f32; nn.n_features() * H];
    let copies = FT_BUCKETS / buckets_file;
    for src_bucket in 0..buckets_file {
        let src = src_bucket * per_bucket * H;
        for c in 0..copies {
            let dst = (src_bucket * copies + c) * per_bucket * H;
            ft_new[dst..dst + per_bucket * H].copy_from_slice(&ft[src..src + per_bucket * H]);
        }
    }

    nn.set_all_weights(&ft_new, &ft_bias, &out_w, &out_b, &num_w);
    nn.set_pw(&pw);
    nn.set_mob_w(&mob_w);
    nn.set_mlp(&l1_w, &l1_b, &l2_w, &l2_b, &mlp_out);
    nn.save(&out).expect("save");
    eprintln!(
        "replicated {buckets_file} -> {FT_BUCKETS} transformer copies at H={H} \
         ({:.0} MB of f32 weights), saved {}",
        nn.n_features() as f64 * H as f64 * 4.0 / 1.048_576e6,
        out.display()
    );
}
