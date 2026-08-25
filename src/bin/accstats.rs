//! Temporary: measure the distribution of the pre-ReLU accumulator
//! (acc + per-stage bias) of the current model, to choose the clamp
//! bound for the product-gate readout terms.
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::{Board, Position};

fn main() {
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(std::path::Path::new("weights/nnue-h16.bin"))
        .expect("nnue");
    let mut s: u64 = 0xDEAD_BEEF_1234_5678;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut vals: Vec<f32> = Vec::new();
    for _ in 0..2000 {
        let mut b = Board::new();
        let plies = 4 + (rnd() % 50) as u32;
        for _ in 0..plies {
            let m = b.movable();
            if m == 0 {
                b.pass();
                if b.movable() == 0 {
                    break;
                }
                continue;
            }
            let n = m.count_ones();
            let k = (rnd() % n as u64) as u32;
            let mut mm = m;
            for _ in 0..k {
                mm &= mm - 1;
            }
            let _ = b.make_move(Position(mm.trailing_zeros() as u8));
        }
        vals.extend(nn.acc_pre_relu(&b));
    }
    vals.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| vals[((vals.len() - 1) as f64 * p) as usize];
    println!(
        "n={}  min {:.2}  p1 {:.2}  p10 {:.2}  p50 {:.2}  p90 {:.2}  p99 {:.2}  max {:.2}",
        vals.len(),
        pct(0.0),
        pct(0.01),
        pct(0.10),
        pct(0.50),
        pct(0.90),
        pct(0.99),
        pct(1.0)
    );
    let pos: Vec<f32> = vals.iter().copied().filter(|v| *v > 0.0).collect();
    println!(
        "positive share {:.1}%  pos p50 {:.2}  pos p90 {:.2}  pos p99 {:.2}",
        pos.len() as f64 / vals.len() as f64 * 100.0,
        pos[pos.len() / 2],
        pos[(pos.len() - 1) * 9 / 10],
        pos[(pos.len() - 1) * 99 / 100]
    );
}
