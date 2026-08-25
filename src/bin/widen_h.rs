//! Net2Net-style widening: read an NNUE weight file of a *smaller* H and
//! write one for the H this binary was compiled with (an exact multiple).
//!
//! Each transformer lane is duplicated `K = H_new / H_old` times with a
//! small symmetry-breaking perturbation, and the read-out weights are
//! divided by K — so the widened model evaluates (almost) identically to
//! the source and fine-tuning starts from the source's optimum instead of
//! from scratch. The perturbation is what lets the copies diverge under
//! training; without it the duplicated lanes receive identical gradients
//! forever and the extra capacity is dead.
//!
//! Usage: widen_h --in <nnue-hSMALL.bin> --out <nnue-hBIG.bin> [--noise f]
use kuroobi::nnue::{Nnue, H};
use kuroobi::pattern::EGAROUCID_PATTERNS;
use std::path::PathBuf;

fn read_f32s(r: &mut impl std::io::Read, n: usize) -> std::io::Result<Vec<f32>> {
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
    let mut noise = 1e-3f32;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--in" => inp = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            "--noise" => noise = it.next().unwrap().parse().unwrap(),
            other => panic!("unknown arg {other}"),
        }
    }
    let inp = inp.expect("--in required");
    let out = out.expect("--out required");

    use std::io::Read;
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
    let h_old = u32::from_le_bytes(u) as usize;
    assert!(
        H.is_multiple_of(h_old) && H > h_old,
        "compiled H={H} must be a larger multiple of the file's H={h_old}"
    );
    let k = H / h_old;
    r.read_exact(&mut u).unwrap();
    let n_features = u32::from_le_bytes(u) as usize;
    r.read_exact(&mut u).unwrap();
    let stages = u32::from_le_bytes(u) as usize;

    let ft = read_f32s(&mut r, n_features * h_old).unwrap();
    let ft_bias = if staged_bias {
        read_f32s(&mut r, stages * h_old).unwrap()
    } else {
        // Shared bias: replicate per stage, as `Nnue::load` does.
        let one = read_f32s(&mut r, h_old).unwrap();
        let mut v = vec![0f32; stages * h_old];
        for st in 0..stages {
            v[st * h_old..(st + 1) * h_old].copy_from_slice(&one);
        }
        v
    };
    let out_w = read_f32s(&mut r, stages * h_old).unwrap();
    let out_b = read_f32s(&mut r, stages).unwrap();
    let num_w = if has_num {
        read_f32s(&mut r, stages * 65).unwrap()
    } else {
        vec![0f32; stages * 65]
    };
    let pw_old = if has_pw {
        read_f32s(&mut r, stages * (h_old / 2)).unwrap()
    } else {
        vec![0f32; stages * (h_old / 2)]
    };
    let mob_old = if has_mob {
        read_f32s(&mut r, stages * 24).unwrap()
    } else {
        vec![0f32; stages * 24]
    };
    // Head: only the first layer indexes accumulator lanes.
    let (mlp_l1_w_old, mlp_l1_b, mlp_l2_w, mlp_l2_b, mlp_out_w) = if has_mlp {
        let a = read_f32s(&mut r, 16 * h_old).unwrap();
        let b = read_f32s(&mut r, 16).unwrap();
        let c = read_f32s(&mut r, 16 * 16).unwrap();
        let d = read_f32s(&mut r, 16).unwrap();
        let e = read_f32s(&mut r, stages * 16).unwrap();
        (a, b, c, d, e)
    } else {
        (
            vec![0f32; 16 * h_old],
            vec![0f32; 16],
            vec![0f32; 16 * 16],
            vec![0f32; 16],
            vec![0f32; stages * 16],
        )
    };

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    assert_eq!(nn.n_features(), n_features, "pattern set mismatch");

    // Deterministic tiny perturbation; scaled to the lane's own magnitude so
    // small weights stay small. Centred on zero: `s >> 40` is 24 bits and so
    // never negative, and the obvious cast left every copy biased upwards
    // instead of scattered around the source lane.
    let mut s: u64 = 0x0DDB_A11C_0FFE_E000;
    let mut jitter = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        ((s >> 40) as f32 - 8.388_608e6) / 8.388_608e6 // in [-1, 1)
    };

    /* Lane layout must keep the product-gate pairing intact.

    The readout pairs lane `i` with lane `i + H/2`, so a naive
    "duplicate the lane block k times" layout re-pairs lane j with itself
    and the source `pw` becomes meaningless — dropping it cost 2.6 MSE
    when widening a jointly-trained H=32 model (31.42 -> 34.06).

    Instead, keep the two halves separate: copy `c` of an old lane from
    the *first* half lands in the first half, and copy `c` of an old lane
    from the *second* half lands in the second half, at matching offsets.
    Then new pair (c*half_old + j, H/2 + c*half_old + j) is exactly old
    pair (j, half_old + j), and `pw` carries over on copy 0 (the other
    copies start at zero so the total product term is unchanged). */
    let half_old = h_old / 2;
    let half_new = H / 2;
    let lane = |c: usize, h: usize| -> usize {
        if h < half_old {
            c * half_old + h
        } else {
            half_new + c * half_old + (h - half_old)
        }
    };

    let mut ft_new = vec![0f32; n_features * H];
    for f in 0..n_features {
        for h in 0..h_old {
            let v = ft[f * h_old + h];
            for c in 0..k {
                ft_new[f * H + lane(c, h)] = v * (1.0 + noise * jitter());
            }
        }
    }
    let mut bias_new = vec![0f32; stages * H];
    let mut ow_new = vec![0f32; stages * H];
    let mut pw_new = vec![0f32; stages * half_new];
    for st in 0..stages {
        for h in 0..h_old {
            for c in 0..k {
                bias_new[st * H + lane(c, h)] = ft_bias[st * h_old + h];
                ow_new[st * H + lane(c, h)] = out_w[st * h_old + h] / k as f32;
            }
        }
        for j in 0..half_old {
            pw_new[st * half_new + j] = pw_old[st * half_old + j];
        }
    }

    let mut mlp_l1_w_new = vec![0f32; 16 * H];
    for i in 0..16 {
        for h in 0..h_old {
            let v = mlp_l1_w_old[i * h_old + h] / k as f32;
            for c in 0..k {
                mlp_l1_w_new[i * H + lane(c, h)] = v;
            }
        }
    }

    nn.set_all_weights(&ft_new, &bias_new, &ow_new, &out_b, &num_w);
    nn.set_pw(&pw_new);
    nn.set_mob_w(&mob_old);
    nn.set_mlp(&mlp_l1_w_new, &mlp_l1_b, &mlp_l2_w, &mlp_l2_b, &mlp_out_w);
    nn.save(&out).expect("save");
    eprintln!(
        "widened H={h_old} -> H={H} (x{k}, noise {noise}), saved {}",
        out.display()
    );
}
