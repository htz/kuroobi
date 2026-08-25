//! Raw leaf-evaluation throughput, isolated from search.
//!
//! What an evaluation costs is set by the number of random row loads into
//! the transformer table, not by the arithmetic on those rows. Search nps
//! cannot answer that question on its own, because changing the weights
//! changes the tree. This times the evaluation call itself over a fixed set
//! of positions, so mask count and accumulator width can be compared
//! directly.
//!
//! Usage: evalbench [--patterns egaroucid|wide8] [--positions n] [--reps n]
use kuroobi::nnue::{Nnue, H};
use kuroobi::pattern::{EGAROUCID_PATTERNS, WIDE8_PATTERNS};
use kuroobi::{Board, Position};

fn main() {
    let mut which = String::from("egaroucid");
    let mut positions = 4096usize;
    let mut reps = 200usize;
    let mut replicas = 1usize;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--patterns" => which = it.next().unwrap_or(which),
            "--positions" => positions = it.next().unwrap().parse().unwrap(),
            "--reps" => reps = it.next().unwrap().parse().unwrap(),
            "--replicas" => replicas = it.next().unwrap().parse::<usize>().unwrap().max(1),
            other => panic!("unknown flag {other}"),
        }
    }
    let patterns = match which.as_str() {
        "wide8" => WIDE8_PATTERNS,
        _ => EGAROUCID_PATTERNS,
    };
    /* Independent copies of the table, cycled one per position.

    Each evaluation still reads the same 64 rows of the same width, so the
    work and the bytes per evaluation are identical -- only the footprint
    the copies share between them grows.

    Read the result as an upper bound, not as the cost. Cycling copies per
    position is the worst case for locality, and a real search is the best:
    one search spans a narrow band of stages, so it stays inside a single
    copy for the whole tree. This harness said four copies cost 1.56x per
    evaluation; the same four copies in `nnue_obf` cost nothing at all
    (band29 depth 13, the same nodes at the same nodes/s). Quote the in-place
    number when deciding, and quote this one only as "what it costs with no
    locality at all". */
    let mut nets: Vec<Nnue> = Vec::with_capacity(replicas);
    for _ in 0..replicas {
        let mut n = Nnue::new(patterns);
        n.init_weights();
        n.quantize();
        nets.push(n);
    }
    let nn = &nets[0];

    // A fixed spread of midgame positions, reached by random play.
    let mut s: u64 = 0x243F_6A88_85A3_08D3;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut boards = Vec::with_capacity(positions);
    while boards.len() < positions {
        let mut b = Board::new();
        let plies = 8 + (rnd() % 40) as u32;
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
        if b.movable() != 0 {
            boards.push(b);
        }
    }
    let idx: Vec<_> = boards
        .iter()
        .map(|b| nn.indices(b.black, b.white))
        .collect();

    // Warm the tables, then time.
    let mut sink = 0.0f32;
    for (k, (b, ix)) in boards.iter().zip(&idx).enumerate() {
        sink += nets[k % replicas].eval_from_indices(ix, b);
    }
    let t0 = std::time::Instant::now();
    for _ in 0..reps {
        for (k, (b, ix)) in boards.iter().zip(&idx).enumerate() {
            sink += nets[k % replicas].eval_from_indices(ix, b);
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    let evals = (positions * reps) as f64;
    let table_mb = nn.n_features() as f64 * H as f64 * 2.0 * 2.0 * replicas as f64 / 1.048_576e6;
    println!(
        "{which}: masks={} H={} features={} replicas={replicas} tables={table_mb:.0}MB  \
         {:.0} evals/s  ({:.1} ns/eval)  [checksum {:.1}]",
        patterns.iter().map(|p| p.masks.len()).sum::<usize>(),
        H,
        nn.n_features(),
        evals / secs,
        secs / evals * 1e9,
        sink,
    );
}
