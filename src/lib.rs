//! # Kuroobi
//! Ultra-fast Reversi (Othello) board processing using 2x uint64 bitboards.
//!
//! ## Quick Start
//! ```rust
//! use kuroobi::{Board, Reversi, Color, Position};
//!
//! // Create initial board
//! let mut board = Board::new();
//! assert_eq!(board.piece_count(), (2, 2));
//! assert_eq!(board.movable_count(), 4);
//!
//! // Check and make a move (Black places 1 stone and flips 1 White stone)
//! let movable = board.movable();
//! let first = movable.trailing_zeros();
//! let pos = Position::from_index(first).unwrap();
//! board.make_move(pos).unwrap();
//! assert_eq!(board.piece_count(), (4, 1));
//!
//! // Full game with history
//! let mut game = Reversi::new();
//! game.make_move(pos).unwrap();
//! assert_eq!(game.to_kifu().len(), 2); // "c4" etc.
//! ```

pub mod color;
pub mod position;
pub mod bitboard;
pub mod board;
pub mod zobrist;
pub mod pattern;
pub mod game;
pub mod evaluator;
pub mod solver;

pub use color::Color;
pub use position::Position;
pub use board::Board;
pub use board::MoveError;
pub use board::GameError;
pub use board::ParseError;
pub use board::BOARD_INIT_STRING;
pub use game::Reversi;
pub use game::MoveRecord;
pub use zobrist::ZobristTable;
pub use pattern::{Pattern, PatternSet, PatternWeights, EDAX_PATTERNS, EGAROUCID_PATTERNS};
pub use evaluator::{Evaluator, STAGE_COUNT};
pub use solver::{EndSolverMode, EndSolverResult, Solver};
