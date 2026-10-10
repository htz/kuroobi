//! A record and its board agree under the eight symmetries, and a position scores the same whichever colour is to move.

use kuroobi::bitboard;
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::NNUE_PATTERNS;
use kuroobi::{Board, Position};

const KIFU: &str = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2\
                    a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";

const SYM_NAMES: [&str; 8] = [
    "identity",
    "rotate 90",
    "rotate 180",
    "rotate 270",
    "mirror",
    "mirror + 90",
    "mirror + 180",
    "mirror + 270",
];

fn sym_pos(p: Position, i: usize) -> Position {
    let bit = bitboard::symmetries(p.to_bit())[i];
    Position::from_index(bit.trailing_zeros()).expect("a single bit is on the board")
}

fn sym_kifu(kifu: &str, i: usize) -> String {
    kifu.as_bytes()
        .chunks(2)
        .map(|mv| {
            let s = std::str::from_utf8(mv).expect("ascii");
            sym_pos(Position::from_kifu(s).expect("kifu"), i).to_kifu()
        })
        .collect()
}

fn replay(kifu: &str, plies: usize) -> Board {
    let mut board = Board::new();
    for (n, mv) in kifu.as_bytes().chunks(2).enumerate() {
        if n == plies {
            break;
        }
        let s = std::str::from_utf8(mv).expect("ascii");
        let p = Position::from_kifu(s).expect("kifu");
        if board.movable() & p.to_bit() == 0 {
            board.pass();
        }
        board.make_move_bits(p);
    }
    board
}

fn flip_colors(b: &Board) -> Board {
    let mut out = *b;
    std::mem::swap(&mut out.black, &mut out.white);
    out.player = b.player.opponent();
    out
}

fn rotation_swaps_colors(i: usize) -> bool {
    let init = Board::new();
    bitboard::symmetries(init.black)[i] != init.black
}

#[test]
fn kifu_and_board_symmetries_agree() {
    for plies in [4, 12, 30, 44] {
        let base = replay(KIFU, plies);
        for (i, name) in SYM_NAMES.iter().enumerate() {
            let from_kifu = replay(&sym_kifu(KIFU, i), plies);
            let black = bitboard::symmetries(base.black)[i];
            let white = bitboard::symmetries(base.white)[i];
            let want = if rotation_swaps_colors(i) {
                (white, black)
            } else {
                (black, white)
            };
            assert_eq!(
                (from_kifu.black, from_kifu.white),
                want,
                "ply {plies}, {name}: transformed record differs from transformed board"
            );
        }
    }
}

#[test]
#[ignore = "requires weights/ (not in git)"]
fn eval_is_the_same_for_either_colour() {
    let mut nn = Nnue::new(NNUE_PATTERNS);
    nn.load(std::path::Path::new("weights/nnue.bin"))
        .expect("nnue");
    nn.quantize();

    for plies in [0, 4, 12, 30, 44] {
        for (i, name) in SYM_NAMES.iter().enumerate() {
            let b = replay(&sym_kifu(KIFU, i), plies);
            assert_eq!(
                nn.eval(&b),
                nn.eval(&flip_colors(&b)),
                "ply {plies}, {name}"
            );
        }
    }
}

#[test]
#[ignore = "requires weights/ (not in git)"]
fn search_value_is_the_same_for_either_colour() {
    let mut engine = Engine::new(EngineConfig {
        depth: 8,
        solve_empties: 0,
        threads: 1,
        ..Default::default()
    })
    .expect("engine init");
    engine.set_use_book(false);

    for plies in [0, 4, 12, 30] {
        for (i, name) in SYM_NAMES.iter().enumerate() {
            let b = replay(&sym_kifu(KIFU, i), plies);
            engine.clear_tables();
            let own = engine.eval_position(&b, 8).value;
            engine.clear_tables();
            let flipped = engine.eval_position(&flip_colors(&b), 8).value;
            assert_eq!(own, flipped, "ply {plies}, {name}");
        }
    }
}
