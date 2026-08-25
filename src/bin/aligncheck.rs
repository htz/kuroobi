//! Report the cache-line alignment of the inference tables.
//!
//! Rows sit at multiples of `H * 2` bytes from the buffer start, so if the
//! buffer itself is not 64-byte aligned every row straddles two cache lines
//! and an evaluation touches twice the lines it needs to.
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;

fn main() {
    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    nn.init_weights();
    nn.quantize();
    for (name, addr) in nn.table_addrs() {
        println!("{name}: addr {addr:#x}  mod 64 = {}", addr % 64);
    }
}
