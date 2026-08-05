//! # Kuroobi
//! Ultra-fast Reversi board processing using 2x uint64 bitboards.
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

pub mod bitboard;
pub mod board;
pub mod book;
pub mod color;
pub mod engine;
pub mod evaluator;
pub mod game;
pub mod learn;
pub mod midgame;
pub mod nnue;
pub mod pattern;
pub mod pattern_index;
pub mod position;
pub mod resources;
pub mod search;
pub mod solver;
pub mod stability;
pub mod trainer;
pub mod zobrist;

pub use board::Board;
pub use board::GameError;
pub use board::MoveError;
pub use board::ParseError;
pub use board::BOARD_INIT_STRING;
pub use color::Color;
pub use evaluator::{AdamOptimizer, Evaluator, Optimizer, SgdOptimizer, STAGE_COUNT};
pub use game::MoveRecord;
pub use game::Reversi;
pub use pattern::{
    Pattern, PatternSet, PatternWeights, EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS,
};
pub use pattern_index::{PatternIndexer, PatternIndices};
pub use position::Position;
pub use search::{SearchResult, Searcher};
pub use solver::{EndSolverMode, EndSolverResult, Solver};
pub use trainer::{Example, Trainer};
pub use zobrist::ZobristTable;
