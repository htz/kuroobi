//! Check this model against independently computed numbers, position by position.
//!
//! Reads the weight dump and the expected outputs written by
//! `tmp/table_dump_check.py`, evaluates the same positions here, and reports the
//! largest disagreement. A table of "what the source appears to say" is not
//! evidence that two models compute the same function; this is.

use kuroobi::board::Board;
use kuroobi::nnue::Nnue;
use kuroobi::pattern::NNUE_PATTERNS;

fn main() {
    let mut args = std::env::args().skip(1);
    let weights = args
        .next()
        .expect("usage: stack_check <dump.bin> <cases.txt>");
    let cases = args
        .next()
        .expect("usage: stack_check <dump.bin> <cases.txt>");

    let mut nn = Nnue::new(NNUE_PATTERNS);
    nn.load_reference_tables(std::path::Path::new(&weights))
        .expect("reference dump");
    nn.quantize();

    let text = std::fs::read_to_string(&cases).expect("cases");
    let (mut worst, mut worst_line) = (0.0f32, String::new());
    let mut n = 0;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let p = u64::from_str_radix(it.next().unwrap(), 16).unwrap();
        let o = u64::from_str_radix(it.next().unwrap(), 16).unwrap();
        let want: f32 = it.next().unwrap().parse().unwrap();
        // The dump is from the mover's point of view, so the mover is Black.
        let board = Board {
            black: p,
            white: o,
            player: kuroobi::color::Color::Black,
            empty_count: 64 - (p | o).count_ones() as u8,
        };
        let got = nn.eval(&board);
        let d = (got - want).abs();
        if d > worst {
            worst = d;
            worst_line = format!("want {want:.6} got {got:.6}");
        }
        n += 1;
    }
    println!("{n} positions, worst |difference| {worst:.6}  ({worst_line})");
}
