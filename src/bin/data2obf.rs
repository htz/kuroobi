//! Convert a record file into plain obf lines (64 board chars, a space, the
//! side to move), so other engines can be measured on exactly the same
//! positions.
//!
//! Board strings are **rank-major** while `Position` is file-major; writing
//! raw bit order transposes the board, which is silent and wrong — every
//! engine still parses it, they just all evaluate a different position.
//!
//! Usage: data2obf <file.data> > <file.obf>
use kuroobi::record;
use kuroobi::Position;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: data2obf <file.data>");
    let records = record::read_all(std::path::Path::new(&path)).expect("read data");
    let mut out = String::with_capacity(records.len() * 68);
    for r in &records {
        // The trainer reads the mover as Black, so the obf says so too.
        let (black, white) = (r.mover, r.opponent);
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
        out.push_str(" X\n");
    }
    print!("{out}");
}
