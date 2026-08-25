//! Searches each position of an obf file to a fixed depth with the midgame
//! NNUE search the engine actually plays with, and reports nodes and time.
//!
//! The fixed depth is what makes it useful: the tree is then a property of
//! the move ordering alone, so two builds that search the same positions to
//! the same depth can be compared on time directly. A change that leaves the
//! node count untouched changed no decision, only speed.
//!
//! Usage: nnue_obf --depth <n> [--threads <n>] [--nnue <file>] <file.obf>
use kuroobi::midgame::{NnueSearch, SharedTt};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::Board;
use std::time::Instant;

fn main() {
    let mut depth: u32 = 15;
    let mut threads: usize = 1;
    let mut nnue_path = String::from("weights/nnue-h16.bin");
    let mut files: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--depth" => depth = it.next().and_then(|v| v.parse().ok()).unwrap_or(depth),
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(threads),
            "--nnue" => nnue_path = it.next().unwrap_or(nnue_path),
            other => files.push(other.to_string()),
        }
    }
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.load(std::path::Path::new(&nnue_path)).expect("nnue");
    nn.quantize();
    let nn: &'static Nnue = Box::leak(Box::new(nn));
    let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(22)));

    for f in files {
        let content = std::fs::read_to_string(&f).expect("read obf");
        let mut total_nodes = 0u64;
        let mut total_time = 0f64;
        for (i, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let board_part = match line.find(';') {
                Some(semi) => &line[..semi],
                None => line,
            };
            let board = match Board::from_string(board_part) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("line {}: bad board: {e:?}", i + 1);
                    continue;
                }
            };
            tt.clear();
            let mut se = NnueSearch::new(nn, tt);
            se.mpc = true;
            se.threads = threads;
            let t0 = Instant::now();
            let (pos, value, _reached) = se.best_move_deadline(&board, depth, None);
            let secs = t0.elapsed().as_secs_f64();
            total_nodes += se.nodes;
            total_time += secs;
            println!(
                "{:>2} | {:>2} empty | {:>6.1} | {:?} | {:>7.3}s | {:>9} | {:>6.2}M",
                i + 1,
                board.empty_count(),
                value,
                pos.map(|p| p.index()),
                secs,
                se.nodes,
                se.nodes as f64 / secs / 1e6
            );
        }
        println!(
            "{f}: {total_nodes} nodes in {total_time:.3}s ({:.2}M nodes/s)",
            total_nodes as f64 / total_time / 1e6
        );
    }
}
