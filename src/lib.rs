//! A Reversi engine on 2×u64 bitboards.
//!
//! ```rust
//! use kuroobi::{Board, Reversi, Color, Position};
//!
//! let mut board = Board::new();
//! assert_eq!(board.piece_count(), (2, 2));
//! assert_eq!(board.movable_count(), 4);
//!
//! let pos = Position::from_index(board.movable().trailing_zeros()).unwrap();
//! board.make_move(pos).unwrap();
//! assert_eq!(board.piece_count(), (4, 1));
//!
//! let mut game = Reversi::new();
//! game.make_move(pos).unwrap();
//! assert_eq!(game.to_kifu().len(), 2);
//! ```

#[macro_use]
mod knob;

pub mod bitboard;
pub mod board;
pub mod book;
pub mod clock;
pub mod color;
pub mod engine;
pub mod game;
pub mod learn;
pub mod linear;
pub mod midgame;
pub mod nnue;
pub mod pattern;
pub mod pattern_index;
pub mod position;
pub mod record;
pub mod resources;
pub mod solver;
pub mod stability;
pub mod trainer;
pub mod wthor;
pub mod zobrist;

pub use board::Board;
pub use board::BOARD_INIT_STRING;
pub use color::Color;
pub use game::Reversi;
pub use linear::{Linear, STAGE_COUNT};
pub use pattern::LINEAR_PATTERNS;
pub use position::Position;
pub use solver::{EndSolverMode, Solver};
