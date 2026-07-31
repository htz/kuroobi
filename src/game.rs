//! Full Reversi game with history, KIFU, and game management.

use crate::board::{Board, GameError, MoveError};
use crate::color::Color;
use crate::position::Position;
use crate::zobrist;

/// Record of a single move for history tracking.
#[derive(Debug, Clone)]
pub struct MoveRecord {
    pub pos: Option<Position>, // None for pass
    pub flipped: u64,
    pub hash: u64,
}

/// Full game state with history support.
pub struct Reversi {
    pub board: Board,
    pub history: Vec<MoveRecord>,
    /// Moves undone and eligible for redo (most recent undo last).
    redo_stack: Vec<MoveRecord>,
    pub hash: u64,
}

impl Reversi {
    /// Create a new game from the standard initial position.
    pub fn new() -> Reversi {
        let board = Board::new();
        let hash = zobrist::compute_hash(board.black, board.white, board.player());

        Reversi {
            board,
            history: Vec::new(),
            redo_stack: Vec::new(),
            hash,
        }
    }

    /// Create a game from a position string.
    pub fn from_string(s: &str) -> Result<Reversi, String> {
        let board = Board::from_string(s).map_err(|e| format!("parse error: {:?}", e))?;
        let hash = zobrist::compute_hash(board.black, board.white, board.player());

        Ok(Reversi {
            board,
            history: Vec::new(),
            redo_stack: Vec::new(),
            hash,
        })
    }

    /// Make a move (checked). Returns the list of flipped positions.
    pub fn make_move(&mut self, pos: Position) -> Result<Vec<Position>, MoveError> {
        if (self.board.black | self.board.white) & pos.to_bit() != 0 {
            return Err(MoveError::Occupied);
        }
        if !self.board.check(pos) {
            return Err(MoveError::NotPlayable);
        }

        // Save history before modifying
        let prev_hash = self.hash;

        // Compute flips
        let pos_bit = pos.to_bit();
        let player_bb = self.board.player_bb();
        let opponent_bb = self.board.opponent_bb();
        let flipped = crate::bitboard::flippable(player_bb, opponent_bb, pos_bit);

        // Record history; a fresh move invalidates the redo stack
        self.history.push(MoveRecord {
            pos: Some(pos),
            flipped,
            hash: prev_hash,
        });
        self.redo_stack.clear();

        // Hash update needs the mover's color, so capture it before the move
        let mover = self.board.player();

        // Apply the move
        self.board.make_move_unchecked(pos);

        // Update hash incrementally
        self.hash = zobrist::update_hash_on_move(prev_hash, pos, flipped, mover);

        let positions: Vec<Position> = crate::bitboard::iter_bits(flipped)
            .filter_map(|i| Position::from_index(i as u32))
            .collect();
        Ok(positions)
    }

    /// Make a move (unchecked). Panics on invalid move.
    pub fn make_move_unchecked(&mut self, pos: Position) {
        self.make_move(pos).expect("invalid move");
    }

    /// Pass the turn (checked).
    pub fn pass(&mut self) -> Result<(), GameError> {
        // Save history before modifying
        let prev_hash = self.hash;

        self.history.push(MoveRecord {
            pos: None, // Pass
            flipped: 0,
            hash: prev_hash,
        });
        self.redo_stack.clear();

        self.board.pass();
        self.hash = zobrist::update_hash_on_pass(prev_hash);

        Ok(())
    }

    /// Undo the last move (or pass).
    pub fn undo(&mut self) -> Result<(), GameError> {
        let record = self.history.pop().ok_or(GameError::NoMoves)?;

        // Restore previous state
        self.hash = record.hash;

        // Restore board state
        if let Some(pos) = record.pos {
            self.board.undo_move(pos, record.flipped);
        } else {
            // Undo pass: just swap player back
            self.board.pass();
        }

        self.redo_stack.push(record);

        Ok(())
    }

    /// Redo the last undone move.
    pub fn redo(&mut self) -> Result<(), GameError> {
        let record = self.redo_stack.pop().ok_or(GameError::NoMoves)?;

        let prev_hash = self.hash;
        let mover = self.board.player();

        if let Some(pos) = record.pos {
            self.board.make_move_unchecked(pos);
            self.hash = zobrist::update_hash_on_move(prev_hash, pos, record.flipped, mover);
        } else {
            self.board.pass();
            self.hash = zobrist::update_hash_on_pass(prev_hash);
        }

        self.history.push(record);

        Ok(())
    }

    /// The whole line of the game, including moves undone (and thus
    /// redoable): confirmed history first, then the redo tail in play order.
    /// `None` entries are passes. `history.len()` is the current cursor.
    pub fn line(&self) -> Vec<Option<Position>> {
        self.history
            .iter()
            .map(|r| r.pos)
            .chain(self.redo_stack.iter().rev().map(|r| r.pos))
            .collect()
    }

    /// Returns a bitboard of all playable positions for the current player.
    pub fn movable(&self) -> u64 {
        self.board.movable()
    }

    /// Count of playable positions.
    pub fn movable_count(&self) -> u8 {
        self.board.movable_count()
    }

    /// Check if the game is over (both players have no moves).
    pub fn is_game_over(&self) -> bool {
        self.board.is_game_over()
    }

    /// Check if current player can move.
    pub fn check_all(&self) -> bool {
        self.board.check_all()
    }

    /// Get piece count per color.
    pub fn piece_count(&self) -> (u8, u8) {
        self.board.piece_count()
    }

    /// Current player.
    pub fn player(&self) -> Color {
        self.board.player()
    }

    /// Number of moves made (turn count).
    pub fn turn(&self) -> usize {
        self.board.turn()
    }

    /// Get Zobrist hash.
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// Convert to KIFU notation (e.g., "f5g5h5...").
    pub fn to_kifu(&self) -> String {
        self.history.iter().filter_map(|r| {
            r.pos.map(|p| p.to_kifu())
        }).collect()
    }

    /// Parse KIFU string back to a game (for replay).
    /// KIFU records placed moves only; passes are implicit. If the side to
    /// move has no legal move but the game is not over, insert a pass and
    /// let the opponent play the recorded move.
    pub fn from_kifu(kifu: &str) -> Result<Reversi, String> {
        let mut game = Reversi::new();
        let chars: Vec<char> = kifu.chars().collect();

        if !chars.len().is_multiple_of(2) {
            return Err(format!("invalid KIFU length: {}", kifu));
        }

        for chunk in chars.chunks(2) {
            let s: String = chunk.iter().collect();
            let pos = Position::from_kifu(&s).map_err(|e| format!("KIFU parse error: {}", e))?;

            if !game.board.check(pos) && !game.board.check_all() {
                // Implicit pass: current player has no move at all
                game.pass().map_err(|e| format!("pass error: {}", e))?;
            }
            game.make_move(pos)
                .map_err(|e| format!("invalid KIFU move {}: {}", s, e))?;
        }

        Ok(game)
    }

    /// Move count (number of moves + passes).
    pub fn move_count(&self) -> usize {
        self.history.len()
    }

    /// Get board state after n moves.
    pub fn board_at(&self, n: usize) -> Result<Board, String> {
        if n > self.history.len() {
            return Err(format!("move index {} out of range", n));
        }
        let mut b = Board::new();
        for i in 0..n {
            let r = &self.history[i];
            if let Some(pos) = r.pos {
                b.make_move_unchecked(pos);
            } else {
                b.pass();
            }
        }
        Ok(b)
    }

    /// Get empty cell count.
    pub fn empty_count(&self) -> u8 {
        self.board.empty_count()
    }

    /// Get all playable positions as a vector.
    pub fn movable_list(&self) -> Vec<Position> {
        let mob = self.movable();
        let mut result = Vec::new();
        let mut b = mob;
        while b != 0 {
            let bit = b.trailing_zeros() as u8;
            if let Some(pos) = Position::from_index(bit as u32) {
                result.push(pos);
            }
            b &= b - 1;
        }
        result
    }

    /// Play a uniformly random legal move (for self-play / Monte Carlo).
    /// Returns None if the current player has no legal move.
    #[cfg(feature = "random")]
    pub fn random_move(&mut self, rng: &mut impl rand::Rng) -> Option<Position> {
        let movable = self.movable();
        if movable == 0 {
            return None;
        }
        let choice = rng.gen_range(0..movable.count_ones());
        let mut b = movable;
        for _ in 0..choice {
            b &= b - 1; // skip to next set bit
        }
        let pos = Position::from_index(b.trailing_zeros())?;
        self.make_move(pos)
            .expect("moves from movable() are always legal");
        Some(pos)
    }

    /// Serialize board state for transposition table.
    pub fn serialize(&self) -> [u64; 3] {
        [self.board.black, self.board.white, self.hash]
    }

    /// Deserialize board state from transposition table.
    /// The player is recovered from the hash: computing the hash for Black
    /// and comparing tells us which side the stored hash was made for
    /// (they differ exactly by swap_player_zobrist).
    pub fn deserialize(black: u64, white: u64, hash: u64) -> Result<Reversi, String> {
        let empty_count = (64 - black.count_ones() - white.count_ones()) as u8;

        let player = if zobrist::compute_hash(black, white, Color::Black) == hash {
            Color::Black
        } else if zobrist::compute_hash(black, white, Color::White) == hash {
            Color::White
        } else {
            return Err("hash does not match board state".to_string());
        };

        Ok(Reversi {
            board: Board {
                black,
                white,
                player,
                empty_count,
            },
            history: Vec::new(),
            redo_stack: Vec::new(),
            hash,
        })
    }
}

impl Default for Reversi {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BOARD_INIT_STRING;

    #[test]
    fn test_new_game() {
        let g = Reversi::new();
        assert_eq!(g.player(), Color::Black);
        assert_eq!(g.piece_count(), (2, 2));
        assert_eq!(g.empty_count(), 60);
        assert_eq!(g.move_count(), 0);
        assert_eq!(g.to_kifu(), "");
    }

    #[test]
    fn test_make_move() {
        let mut g = Reversi::new();

        // First move: Black places at c4 (Position 19)
        let pos = Position(19);
        let result = g.make_move(pos);
        assert!(result.is_ok(), "First move should be valid: {result:?}");

        assert_eq!(g.move_count(), 1);
        assert_eq!(g.piece_count(), (4, 1)); // Black placed 1, White flipped 1
        assert_eq!(g.player(), Color::White);
    }

    #[test]
    fn test_invalid_move() {
        let mut g = Reversi::new();

        // Try placing on an occupied square (D4 = Position 28)
        let result = g.make_move(Position(28));
        assert_eq!(result, Err(MoveError::Occupied));

        // Try placing where nothing flips
        let result = g.make_move(Position(0)); // A1 is far from pieces
        assert_eq!(result, Err(MoveError::NotPlayable));
    }

    #[test]
    fn test_pass() {
        let mut g = Reversi::new();

        // Make a move at c4 (Position 19)
        g.make_move(Position(19)).unwrap();

        // Pass
        g.pass().unwrap();
        assert_eq!(g.move_count(), 2);
        assert_eq!(g.player(), Color::Black); // Player swapped back
    }

    #[test]
    fn test_undo_redo() {
        let mut g = Reversi::new();

        g.make_move(Position(19)).unwrap();

        // Undo
        g.undo().unwrap();
        assert_eq!(g.move_count(), 0);
        assert_eq!(g.piece_count(), (2, 2));
        assert_eq!(g.player(), Color::Black);

        // Redo
        g.redo().unwrap();
        assert_eq!(g.move_count(), 1);
        assert_eq!(g.piece_count(), (4, 1));
    }

    #[test]
    fn test_undo_redo_edge_cases() {
        let mut g = Reversi::new();

        // Nothing to undo/redo initially
        assert!(g.undo().is_err(), "undo on fresh game must fail");
        assert!(g.redo().is_err(), "redo on fresh game must fail");

        // Redo hash must match the hash recomputed from the board
        g.make_move(Position(19)).unwrap();
        let hash_after_move = g.hash();
        g.undo().unwrap();
        g.redo().unwrap();
        assert_eq!(g.hash(), hash_after_move, "redo restores the exact hash");
        assert_eq!(
            g.hash(),
            zobrist::compute_hash(g.board.black, g.board.white, g.player()),
            "hash stays consistent with the board after redo"
        );

        // A fresh move invalidates the redo stack
        g.undo().unwrap();
        g.make_move(Position(26)).unwrap(); // d3, another legal opening
        assert!(g.redo().is_err(), "new move must clear the redo stack");

        // Multi-level undo/redo preserves order (LIFO)
        let mut g = Reversi::new();
        g.make_move(Position(19)).unwrap();
        let m2 = Position::from_index(g.movable().trailing_zeros()).unwrap();
        g.make_move(m2).unwrap();
        let final_hash = g.hash();
        g.undo().unwrap();
        g.undo().unwrap();
        assert_eq!(g.piece_count(), (2, 2));
        g.redo().unwrap();
        g.redo().unwrap();
        assert_eq!(g.hash(), final_hash, "two undos + two redos round-trip");
    }

    #[test]
    fn test_undo_after_pass() {
        let mut g = Reversi::new();
        g.make_move(Position(19)).unwrap();
        let hash_before_pass = g.hash();
        let player_before_pass = g.player();

        g.pass().unwrap();
        g.undo().unwrap();

        assert_eq!(g.hash(), hash_before_pass, "undoing a pass restores hash");
        assert_eq!(g.player(), player_before_pass, "undoing a pass restores player");
        assert_eq!(g.move_count(), 1);
    }

    #[test]
    fn test_kifu_roundtrip() {
        let mut g = Reversi::new();

        // Make a few moves
        g.make_move(Position(19)).unwrap(); // c4

        let movable = g.movable();
        let first = movable.trailing_zeros();
        let pos2 = Position::from_index(first).unwrap();
        g.make_move(pos2).unwrap();

        let kifu = g.to_kifu();
        assert!(!kifu.is_empty(), "KIFU should not be empty");

        // Replay from KIFU
        let g2 = Reversi::from_kifu(&kifu).unwrap();
        assert_eq!(g2.piece_count(), g.piece_count());
        assert_eq!(g2.player(), g.player());
    }

    #[test]
    fn test_hash_consistency() {
        let mut g = Reversi::new();
        let h1 = g.hash();

        g.make_move(Position(19)).unwrap();

        let h2 = g.hash();
        assert_ne!(h1, h2, "Hash should change after move");

        // Undo should restore the hash
        g.undo().unwrap();
        assert_eq!(g.hash(), h1, "Hash should be restored after undo");
    }

    #[test]
    fn test_serialize_deserialize() {
        let mut g = Reversi::new();

        g.make_move(Position(19)).unwrap();

        let serialized = g.serialize();
        let g2 = Reversi::deserialize(serialized[0], serialized[1], serialized[2]).unwrap();

        assert_eq!(g2.board.black, g.board.black);
        assert_eq!(g2.board.white, g.board.white);
        assert_eq!(g2.player(), g.player(), "player must be recovered from hash");
        assert_eq!(g2.hash(), g.hash());

        // Tampered hash must be rejected
        assert!(Reversi::deserialize(serialized[0], serialized[1], serialized[2] ^ 1).is_err());
    }

    #[test]
    fn test_board_at() {
        let mut g = Reversi::new();

        // Board at 0 should be initial position
        let b0 = g.board_at(0).unwrap();
        assert_eq!(b0.piece_count(), (2, 2));

        // Make a move
        g.make_move(Position(19)).unwrap();

        // Board at 1 should have 3 black, 1 white
        let b1 = g.board_at(1).unwrap();
        assert_eq!(b1.piece_count(), (4, 1));
    }

    #[test]
    fn test_movable_list() {
        let g = Reversi::new();
        let list = g.movable_list();
        assert_eq!(list.len(), 4, "Initial position: 4 moves for Black");

        // All positions should be valid
        for pos in &list {
            assert!(g.board.check(*pos));
        }
    }

    #[test]
    fn test_game_over_detection() {
        let g = Reversi::new();
        assert!(!g.is_game_over(), "Initial position: game not over");
    }

    #[test]
    fn test_from_string() {
        let g = Reversi::from_string(BOARD_INIT_STRING).unwrap();
        assert_eq!(g.piece_count(), (2, 2));
        assert_eq!(g.player(), Color::Black);
    }

    #[test]
    fn test_from_string_invalid() {
        assert!(Reversi::from_string("invalid").is_err());
    }

    /// Play a deterministic full game to the end, verifying invariants
    /// (hash consistency, piece conservation) at every ply, then replay
    /// it from the KIFU and compare final states.
    #[test]
    fn test_full_game_and_kifu_replay() {
        let mut g = Reversi::new();
        let mut plies = 0;
        loop {
            let moves = g.movable();
            if moves == 0 {
                if g.is_game_over() {
                    break;
                }
                g.pass().unwrap();
                continue;
            }
            // Deterministic choice: lowest bit on even plies, highest on odd
            let bit = if plies % 2 == 0 {
                moves.trailing_zeros()
            } else {
                63 - moves.leading_zeros()
            };
            g.make_move(Position::from_index(bit).unwrap()).unwrap();
            plies += 1;

            // Invariants
            let (b, w) = g.piece_count();
            assert_eq!(
                b as u32 + w as u32 + g.empty_count() as u32,
                64,
                "piece conservation"
            );
            let recomputed =
                zobrist::compute_hash(g.board.black, g.board.white, g.player());
            assert_eq!(g.hash(), recomputed, "incremental hash must stay in sync");
        }
        assert!(plies >= 20, "a full game plays plenty of moves");
        assert!(g.is_game_over());

        // KIFU replay reproduces the same final position
        let replay = Reversi::from_kifu(&g.to_kifu()).unwrap();
        assert_eq!(replay.board.black, g.board.black);
        assert_eq!(replay.board.white, g.board.white);
    }

    #[cfg(feature = "random")]
    #[test]
    fn test_random_move_plays_legal_moves() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let mut g = Reversi::new();
        for _ in 0..10 {
            if g.movable() == 0 {
                break;
            }
            let before = g.movable();
            let pos = g.random_move(&mut rng).unwrap();
            assert!(before & pos.to_bit() != 0, "returned move was legal");
        }
        assert!(g.move_count() > 0);
    }
}
