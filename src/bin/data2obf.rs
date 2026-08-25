//! Convert a 17-byte label file into plain obf lines (64 board chars, a
//! space, the side to move), so other engines can be measured on exactly
//! the same positions.
//!
//! Board strings are **rank-major** while `Position` is file-major; writing
//! raw bit order transposes the board, which is silent and wrong — every
//! engine still parses it, they just all evaluate a different position.
//!
//! Usage: data2obf <file.data> > <file.obf>
use kuroobi::Position;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: data2obf <file.data>");
    let bytes = std::fs::read(&path).expect("read data");
    let n = bytes.len() / 17;
    let mut out = String::with_capacity(n * 68);
    for i in 0..n {
        let r = &bytes[i * 17..i * 17 + 17];
        let black = u64::from_le_bytes(r[0..8].try_into().unwrap());
        let white = u64::from_le_bytes(r[8..16].try_into().unwrap());
        for idx in 0..64u8 {
            let file = idx % 8;
            let rank = idx / 8;
            let bit = Position::from_file_rank(file, rank).unwrap().to_bit();
            out.push(if black & bit != 0 {
                'X'
            } else if white & bit != 0 {
                'O'
            } else {
                '-'
            });
        }
        // The data format normalizes every record to Black to move.
        out.push_str(" X\n");
    }
    print!("{out}");
}
