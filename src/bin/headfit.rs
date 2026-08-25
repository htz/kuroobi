//! How much accuracy is still reachable from the accumulator we already have?
//!
//! Every candidate shape for the read-out normally costs a full retrain to
//! evaluate, which is hours per answer. This asks a cheaper question that
//! ranks the candidates first: freeze the trained feature transformer, take
//! its H pre-activation lanes as fixed inputs, and fit each candidate head on
//! the *residual* the current read-out leaves against exact solved values.
//!
//! The numbers are a lower bound on what a real retrain would get, in two
//! ways: the transformer cannot co-adapt to the new head, and the fit sees
//! ~16k positions where training sees a billion. A shape that cannot help
//! here is very unlikely to help there; a shape that helps a lot here is
//! worth the retrain. Treat the ordering as the result, not the magnitudes.
//!
//! Usage: headfit --nnue <model.bin> --train <f.data>... --val <f.data>
// The layer loops index several flat parameter slices at once by the same
// counter, which iterators only obscure -- the same exception the model itself
// takes.
#![allow(clippy::needless_range_loop)]
use kuroobi::nnue::{Nnue, H};
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::{Board, Color};

/// The clamp the product gate already uses, reused so a squared lane and a
/// gated lane are on the same footing.
const CLAMP: f32 = 16.0;

struct Example {
    /// Pre-activation accumulator lanes (feature rows + the stage's bias).
    a: [f32; H],
    /// What the current model leaves on the table: exact value - prediction.
    resid: f32,
}

fn load(nn: &Nnue, path: &str) -> Vec<Example> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let n = bytes.len() / 17;
    let mut out = Vec::with_capacity(n);
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
        let ix = nn.indices(black, white);
        let pred = nn.eval_from_indices(&ix, &board);
        out.push(Example {
            a: nn.acc_pre_relu(&board),
            resid: truth - pred,
        });
    }
    out
}

/// Which nonlinearity of the accumulator a candidate head is allowed to see.
#[derive(Clone, Copy, PartialEq)]
enum Feat {
    /// `relu(a)` -- what the read-out and the current head already consume.
    Relu,
    /// `clamp(a,0,C)^2 / C` -- curvature per lane, the shape the stacked
    /// read-out uses throughout.
    Square,
    /// Both, concatenated.
    Both,
}

impl Feat {
    fn dim(self) -> usize {
        match self {
            Feat::Both => 2 * H,
            _ => H,
        }
    }

    fn build(self, a: &[f32; H], out: &mut [f32]) {
        match self {
            Feat::Relu => {
                for h in 0..H {
                    out[h] = a[h].max(0.0);
                }
            }
            Feat::Square => {
                for h in 0..H {
                    let c = a[h].clamp(0.0, CLAMP);
                    out[h] = c * c / CLAMP;
                }
            }
            Feat::Both => {
                for h in 0..H {
                    out[h] = a[h].max(0.0);
                    let c = a[h].clamp(0.0, CLAMP);
                    out[H + h] = c * c / CLAMP;
                }
            }
        }
    }
}

/// A head candidate: zero, one or two ReLU hidden layers over `feat`.
struct Head {
    feat: Feat,
    h1: usize,
    h2: usize,
    p: Vec<f32>,
    m: Vec<f32>,
    v: Vec<f32>,
    g: Vec<f32>,
    step: u32,
}

impl Head {
    fn new(feat: Feat, h1: usize, h2: usize) -> Head {
        let n_in = feat.dim();
        let n = if h1 == 0 {
            n_in + 1
        } else if h2 == 0 {
            h1 * n_in + h1 + h1 + 1
        } else {
            h1 * n_in + h1 + h2 * h1 + h2 + h2 + 1
        };
        // Small deterministic spread; a zero hidden layer never receives a
        // gradient, which is how the engine's own head sat dead for a run.
        let mut s: u64 = 0x51ED_2701_ABCD_1234;
        let mut p = vec![0.0f32; n];
        let scale = 1.0 / (n_in as f32).sqrt();
        for (i, w) in p.iter_mut().enumerate() {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            // Read-out weights start at zero so the head begins inert.
            if h1 != 0 && i >= n - (if h2 == 0 { h1 } else { h2 }) - 1 {
                continue;
            }
            *w = (((s >> 40) as f32 - 8.388_608e6) / 8.388_608e6) * scale;
        }
        Head {
            feat,
            h1,
            h2,
            m: vec![0.0; n],
            v: vec![0.0; n],
            g: vec![0.0; n],
            p,
            step: 0,
        }
    }

    /// Forward, writing the hidden activations the backward pass needs.
    fn forward(&self, x: &[f32], z1: &mut [f32], z2: &mut [f32]) -> f32 {
        let n_in = self.feat.dim();
        if self.h1 == 0 {
            let mut o = self.p[n_in];
            for i in 0..n_in {
                o += self.p[i] * x[i];
            }
            return o;
        }
        let (w1, rest) = self.p.split_at(self.h1 * n_in);
        let (b1, rest) = rest.split_at(self.h1);
        for i in 0..self.h1 {
            let mut s = b1[i];
            for j in 0..n_in {
                s += w1[i * n_in + j] * x[j];
            }
            z1[i] = s.max(0.0);
        }
        if self.h2 == 0 {
            let (w3, b3) = rest.split_at(self.h1);
            let mut o = b3[0];
            for i in 0..self.h1 {
                o += w3[i] * z1[i];
            }
            return o;
        }
        let (w2, rest) = rest.split_at(self.h2 * self.h1);
        let (b2, rest) = rest.split_at(self.h2);
        for j in 0..self.h2 {
            let mut s = b2[j];
            for i in 0..self.h1 {
                s += w2[j * self.h1 + i] * z1[i];
            }
            z2[j] = s.max(0.0);
        }
        let (w3, b3) = rest.split_at(self.h2);
        let mut o = b3[0];
        for j in 0..self.h2 {
            o += w3[j] * z2[j];
        }
        o
    }

    /// Accumulate d(err^2)/dp into `g` for one example (the 2 is folded into
    /// the learning rate, as the engine's own trainer does).
    fn backward(&mut self, x: &[f32], z1: &[f32], z2: &[f32], err: f32) {
        let n_in = self.feat.dim();
        if self.h1 == 0 {
            for i in 0..n_in {
                self.g[i] += err * x[i];
            }
            self.g[n_in] += err;
            return;
        }
        let off_b1 = self.h1 * n_in;
        let off_w2 = off_b1 + self.h1;
        if self.h2 == 0 {
            let off_b3 = off_w2 + self.h1;
            let mut d1 = vec![0.0f32; self.h1];
            for i in 0..self.h1 {
                self.g[off_w2 + i] += err * z1[i];
                if z1[i] > 0.0 {
                    d1[i] = self.p[off_w2 + i];
                }
            }
            self.g[off_b3] += err;
            for i in 0..self.h1 {
                if d1[i] == 0.0 {
                    continue;
                }
                self.g[off_b1 + i] += err * d1[i];
                for j in 0..n_in {
                    self.g[i * n_in + j] += err * d1[i] * x[j];
                }
            }
            return;
        }
        let off_b2 = off_w2 + self.h2 * self.h1;
        let off_w3 = off_b2 + self.h2;
        let off_b3 = off_w3 + self.h2;
        let mut d2 = vec![0.0f32; self.h2];
        for j in 0..self.h2 {
            self.g[off_w3 + j] += err * z2[j];
            if z2[j] > 0.0 {
                d2[j] = self.p[off_w3 + j];
            }
        }
        self.g[off_b3] += err;
        let mut d1 = vec![0.0f32; self.h1];
        for j in 0..self.h2 {
            if d2[j] == 0.0 {
                continue;
            }
            self.g[off_b2 + j] += err * d2[j];
            for i in 0..self.h1 {
                self.g[off_w2 + j * self.h1 + i] += err * d2[j] * z1[i];
                d1[i] += d2[j] * self.p[off_w2 + j * self.h1 + i];
            }
        }
        for i in 0..self.h1 {
            if z1[i] <= 0.0 || d1[i] == 0.0 {
                continue;
            }
            self.g[off_b1 + i] += err * d1[i];
            for j in 0..n_in {
                self.g[i * n_in + j] += err * d1[i] * x[j];
            }
        }
    }

    fn adam(&mut self, lr: f32, wd: f32, scale: f32) {
        self.step += 1;
        let b1 = 0.9f32;
        let b2 = 0.999f32;
        let c1 = 1.0 - b1.powi(self.step as i32);
        let c2 = 1.0 - b2.powi(self.step as i32);
        for i in 0..self.p.len() {
            let g = self.g[i] * scale;
            self.m[i] = b1 * self.m[i] + (1.0 - b1) * g;
            self.v[i] = b2 * self.v[i] + (1.0 - b2) * g * g;
            let mh = self.m[i] / c1;
            let vh = self.v[i] / c2;
            self.p[i] -= lr * (mh / (vh.sqrt() + 1e-8) + wd * self.p[i]);
            self.g[i] = 0.0;
        }
    }

    fn n_params(&self) -> usize {
        self.p.len()
    }
}

fn mae(head: &Head, set: &[Example], xs: &[Vec<f32>]) -> f32 {
    let mut z1 = vec![0.0f32; head.h1.max(1)];
    let mut z2 = vec![0.0f32; head.h2.max(1)];
    let mut s = 0.0f64;
    for (e, x) in set.iter().zip(xs) {
        let o = head.forward(x, &mut z1, &mut z2);
        s += (e.resid - o).abs() as f64;
    }
    (s / set.len() as f64) as f32
}

fn main() {
    let mut nnue_path = String::new();
    let mut train_files: Vec<String> = Vec::new();
    let mut val_file = String::new();
    let mut epochs = 300usize;
    let mut lr = 0.002f32;
    let mut wd = 0.001f32;
    let mut it = std::env::args().skip(1);
    let mut mode = "";
    while let Some(a) = it.next() {
        match a.as_str() {
            "--nnue" => nnue_path = it.next().expect("--nnue needs a path"),
            "--epochs" => epochs = it.next().unwrap().parse().unwrap(),
            "--lr" => lr = it.next().unwrap().parse().unwrap(),
            "--wd" => wd = it.next().unwrap().parse().unwrap(),
            "--train" => mode = "train",
            "--val" => mode = "val",
            other if other.starts_with("--") => panic!("unknown flag {other}"),
            other if mode == "val" => val_file = other.to_string(),
            other => train_files.push(other.to_string()),
        }
    }
    assert!(!nnue_path.is_empty(), "--nnue required");
    assert!(!val_file.is_empty(), "--val required");

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(std::path::Path::new(&nnue_path)).expect("nnue");
    nn.quantize();

    let mut train: Vec<Example> = Vec::new();
    for f in &train_files {
        train.extend(load(&nn, f));
    }
    let val = load(&nn, &val_file);
    assert!(!train.is_empty(), "--train required");

    let base_train: f64 =
        train.iter().map(|e| e.resid.abs() as f64).sum::<f64>() / train.len() as f64;
    let base_val: f64 = val.iter().map(|e| e.resid.abs() as f64).sum::<f64>() / val.len() as f64;
    println!(
        "model {nnue_path}\ntrain {} positions, val {} positions",
        train.len(),
        val.len()
    );
    println!("current model            train MAE {base_train:.3}  val MAE {base_val:.3}");

    let candidates: [(&str, Feat, usize, usize); 5] = [
        ("per-lane square, linear", Feat::Square, 0, 0),
        ("head 32->16->16 (current)", Feat::Relu, 16, 16),
        ("head 32->32->32", Feat::Relu, 32, 32),
        ("head [relu,sq] 64->32->32", Feat::Both, 32, 32),
        ("head [relu,sq] 64->64->64", Feat::Both, 64, 64),
    ];

    for (name, feat, h1, h2) in candidates {
        let dim = feat.dim();
        let build = |set: &[Example]| -> Vec<Vec<f32>> {
            set.iter()
                .map(|e| {
                    let mut x = vec![0.0f32; dim];
                    feat.build(&e.a, &mut x);
                    x
                })
                .collect()
        };
        let xt = build(&train);
        let xv = build(&val);

        let mut head = Head::new(feat, h1, h2);
        let batch = 256usize;
        let mut z1 = vec![0.0f32; h1.max(1)];
        let mut z2 = vec![0.0f32; h2.max(1)];
        let mut order: Vec<usize> = (0..train.len()).collect();
        let mut s: u64 = 0x1234_5678_9ABC_DEF0;
        let mut best = f32::MAX;
        for ep in 0..epochs {
            // Shuffle so consecutive positions from one file do not form a
            // batch together.
            for i in (1..order.len()).rev() {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                order.swap(i, (s % (i as u64 + 1)) as usize);
            }
            // Cosine decay: the same schedule the engine's trainer uses.
            let t = ep as f32 / epochs as f32;
            let lr_ep = lr * 0.5 * (1.0 + (std::f32::consts::PI * t).cos());
            let mut k = 0;
            while k < order.len() {
                let end = (k + batch).min(order.len());
                for &i in &order[k..end] {
                    let o = head.forward(&xt[i], &mut z1, &mut z2);
                    head.backward(&xt[i], &z1, &z2, o - train[i].resid);
                }
                head.adam(lr_ep, wd, 1.0 / (end - k) as f32);
                k = end;
            }
            let v = mae(&head, &val, &xv);
            if v < best {
                best = v;
            }
        }
        let t = mae(&head, &train, &xt);
        let v = mae(&head, &val, &xv);
        println!(
            "{name:26} train MAE {t:.3}  val MAE {v:.3} (best {best:.3})  \
             gain {:+.3}  params {}",
            base_val as f32 - v,
            head.n_params()
        );
    }
}
