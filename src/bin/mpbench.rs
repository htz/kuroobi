//! Midgame parallel search check: same move as sequential, and how much faster.
use kuroobi::evaluator::Evaluator;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::search::Searcher;
use kuroobi::{Board, Position};
use std::time::Instant;

fn main() {
    let mut ev = Evaluator::new(EGAROUCID_PATTERNS);
    ev.load_weights(std::path::Path::new("weights/linear.bin"))
        .expect("weights");
    let depth: u8 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);

    let mut boards = Vec::new();
    let mut s: u64 = 0x1234_5678_9ABC_DEF0;
    for _ in 0..6 {
        let mut b = Board::new();
        for _ in 0..14 {
            let m = b.movable();
            if m == 0 {
                b.pass();
                if b.movable() == 0 {
                    break;
                }
                continue;
            }
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let mut k = (s % m.count_ones() as u64) as u32;
            let mut mm = m;
            loop {
                if k == 0 {
                    b.make_move_bits(Position(mm.trailing_zeros() as u8));
                    break;
                }
                mm &= mm - 1;
                k -= 1;
            }
        }
        boards.push(b);
    }

    let mut base = 0.0;
    for th in [1usize, 2, 4, 8] {
        let mut se = Searcher::new(20);
        se.threads = th;
        let t0 = Instant::now();
        let (mut moves, mut nodes) = (Vec::new(), 0u64);
        for b in &boards {
            se.clear();
            let r = se.search(b, &ev, depth);
            moves.push(r.best_move.map(|p| p.index()));
            nodes += r.nodes;
        }
        let el = t0.elapsed().as_secs_f64();
        if th == 1 {
            base = el;
        }
        println!(
            "threads {th}: {el:7.3}s ({:.2}x)  nodes {nodes:>12}  moves {moves:?}",
            base / el
        );
    }
}
