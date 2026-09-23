//! Ultra-fast Reversi board processing using 2x uint64 bitboards.
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

pub mod bitboard;
pub mod board;
pub mod book;
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
pub mod timectl;
pub mod trainer;
pub mod wthor;
pub mod zobrist;

pub use board::Board;
pub use board::GameError;
pub use board::MoveError;
pub use board::ParseError;
pub use board::BOARD_INIT_STRING;
pub use color::Color;
pub use game::MoveRecord;
pub use game::Reversi;
pub use linear::{AdamOptimizer, Linear, Optimizer, SgdOptimizer, STAGE_COUNT};
pub use pattern::{Pattern, LINEAR_PATTERNS, NNUE_PATTERNS};
pub use pattern_index::{PatternIndexer, PatternIndices};
pub use position::Position;
pub use solver::{EndSolverMode, EndSolverResult, Solver};
pub use trainer::{Example, Trainer};
pub use zobrist::ZobristTable;
