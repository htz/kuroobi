//! Loss-attribution tool: for each position, compare a model's shallow
//! evaluation against the position's true value, so "how wrong is the eval"
//! can be separated from "how deep did the search get".
//!
//! Reads an obf file, and for every position reports:
//!   depth-1 eval, fixed-depth search value, and (with --exact) the solved
//!   value. The mean absolute error against the exact column is the
//!   number that matters for eval quality.
//!
//! Usage: evalcmp [--depth n] [--nnue path] [--threads n] <file.obf>
use kuroobi::midgame::{NnueSearch, SharedTt};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::Board;

fn main() {
    let mut depth: u32 = 12;
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

    println!("# empties  eval1  search{depth}");
    for f in files {
        let content = std::fs::read_to_string(&f).expect("read obf");
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let part = line.split(';').next().unwrap_or(line);
            let board = match Board::from_string(part) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let acc = nn.indices(board.black, board.white);
            let e1 = nn.eval_from_indices(&acc, &board);
            tt.clear();
            let mut se = NnueSearch::new(nn, tt);
            se.mpc = true;
            se.threads = threads;
            let (_pos, v, _d) = se.best_move_deadline(&board, depth, None);
            println!("{:>3} {:>8.2} {:>8.2}", board.empty_count(), e1, v);
        }
    }
}
