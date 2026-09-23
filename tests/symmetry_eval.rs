//! Does the same position evaluate identically from every orientation?  Checks evaluation and search values across all 16 variants (8 board symmetries x color flip).

use kuroobi::bitboard;
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::LINEAR_PATTERNS;
use kuroobi::{Board, Color, Position};

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
    out.player = match b.player {
        Color::Black => Color::White,
        Color::White => Color::Black,
    };
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

fn all_views(kifu: &str, plies: usize) -> Vec<(String, Board)> {
    let mut out = Vec::new();
    for (i, name) in SYM_NAMES.iter().enumerate() {
        let b = replay(&sym_kifu(kifu, i), plies);
        out.push(((*name).to_string(), b));
        out.push((format!("{name} + color flip"), flip_colors(&b)));
    }
    out
}

#[test]
#[ignore = "requires weights/ (not in git)"]
fn eval_is_the_same_from_every_view() {
    let mut nn = Nnue::new(LINEAR_PATTERNS);
    nn.load(std::path::Path::new("weights/nnue.bin"))
        .expect("nnue");
    nn.quantize();

    for plies in [0, 4, 12, 30, 44] {
        let views = all_views(KIFU, plies);
        let vals: Vec<f32> = views.iter().map(|(_, b)| nn.eval(b)).collect();
        let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
            - vals.iter().cloned().fold(f32::MAX, f32::min);
        println!(
            "ply {plies:>2}: 16-variant eval spread {spread:.6}  (e.g. {:.4})",
            vals[0]
        );
        if spread > 1e-3 {
            for ((name, _), v) in views.iter().zip(&vals) {
                println!("    {name:<24} {v:.6}");
            }
        }
        assert!(
            spread <= 1e-3,
            "eval spread at ply {plies} (spread {spread})"
        );
    }
}

#[test]
#[ignore = "requires weights/ (not in git)"]
fn search_value_is_the_same_from_every_view() {
    let mut engine = Engine::new(EngineConfig {
        depth: 8,
        solve_empties: 0, // solves agree trivially; test the midgame only
        threads: 1,       // parallel scan order jitters; isolate orientation
        ..Default::default()
    })
    .expect("engine init");
    engine.set_use_book(false);

    for plies in [0, 4, 12, 30] {
        let views = all_views(KIFU, plies);
        let mut vals = Vec::new();
        for (_, b) in &views {
            engine.clear_tables();
            vals.push(engine.eval_position(b, 8).value);
        }
        let spread = vals.iter().cloned().fold(f32::MIN, f32::max)
            - vals.iter().cloned().fold(f32::MAX, f32::min);
        println!(
            "ply {plies:>2}: 16-variant search spread {spread:.6}  (e.g. {:.4})",
            vals[0]
        );
        if spread > 1e-3 {
            for ((name, _), v) in views.iter().zip(&vals) {
                println!("    {name:<24} {v:.6}");
            }
        }
        assert!(
            spread <= 1e-3,
            "search spread at ply {plies} (spread {spread})"
        );
    }
}
