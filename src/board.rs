//! Board state management using 2x uint64 bitboards.

use crate::bitboard;
use crate::color::Color;
use crate::position::Position;

/// Board representation. 24 bytes with #[derive(Clone, Copy)].
#[derive(Debug, Clone, Copy)]
pub struct Board {
    pub black: u64,
    pub white: u64,
    pub player: Color,
    pub empty_count: u8,
}

/// Error when making an invalid move.
#[derive(Debug, Clone, PartialEq)]
pub enum MoveError {
    InvalidPosition,
    NotPlayable,
    Occupied,
}

impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoveError::InvalidPosition => write!(f, "invalid position"),
            MoveError::NotPlayable => write!(f, "not playable (no flip)"),
            MoveError::Occupied => write!(f, "square already occupied"),
        }
    }
}

impl std::error::Error for MoveError {}

/// Error for game-level operations.
#[derive(Debug, Clone, PartialEq)]
pub enum GameError {
    NoMoves,
    GameOver,
}

impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GameError::NoMoves => write!(f, "no moves available"),
            GameError::GameOver => write!(f, "game is over"),
        }
    }
}

impl std::error::Error for GameError {}

/// Error for parsing position strings.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    InvalidString,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid board string")
    }
}

impl std::error::Error for ParseError {}

/// Standard initial position string (rank-major, 64 squares + " X" for the
/// player to move): D4=O(White), E4=X(Black), D5=X(Black), E5=O(White),
/// Black to move.
pub const BOARD_INIT_STRING: &str =
    "---------------------------OX------XO--------------------------- X";

impl Board {
    /// Create a new board from the standard initial position.
    pub fn new() -> Board {
        let mut b = Board {
            black: 0,
            white: 0,
            player: Color::Black,
            empty_count: 60,
        };
        b.set_initial();
        b
    }

    /// Set the board from a position string.
    pub fn from_string(s: &str) -> Result<Board, ParseError> {
        let mut b = Board {
            black: 0,
            white: 0,
            player: Color::Black,
            empty_count: 0,
        };
        b.set_from_string(s)?;
        Ok(b)
    }

    fn set_initial(&mut self) {
        // Standard Reversi initial position (file-major indices):
        // D4 (file=3, rank=3) → 27: White
        // E4 (file=4, rank=3) → 35: Black
        // D5 (file=3, rank=4) → 28: Black
        // E5 (file=4, rank=4) → 36: White
        self.black = (1u64 << 35) | (1u64 << 28); // E4 + D5
        self.white = (1u64 << 27) | (1u64 << 36); // D4 + E5
        self.player = Color::Black;
        self.empty_count = 60;
    }

    fn set_from_string(&mut self, s: &str) -> Result<(), ParseError> {
        self.black = 0;
        self.white = 0;
        self.empty_count = 0;

        let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
        if chars.len() != 65 {
            return Err(ParseError::InvalidString);
        }

        for (i, &ch) in chars.iter().take(64).enumerate() {
            // Board string uses rank-major (rank*8+file), Position uses file-major (file*8+rank)
            let file = (i % 8) as u8;
            let rank = (i / 8) as u8;
            let pos = Position::from_file_rank(file, rank).ok_or(ParseError::InvalidString)?;
            match ch.to_ascii_lowercase() {
                'x' | '*' => {
                    self.black |= pos.to_bit();
                }
                'o' => {
                    self.white |= pos.to_bit();
                }
                '-' | '.' => {
                    self.empty_count += 1;
                }
                _ => return Err(ParseError::InvalidString),
            }
        }

        // Last character is the player
        let player_char = s
            .chars()
            .filter(|c| !c.is_whitespace())
            .nth(64)
            .ok_or(ParseError::InvalidString)?;
        match player_char.to_ascii_lowercase() {
            'x' | '*' => self.player = Color::Black,
            'o' => self.player = Color::White,
            _ => return Err(ParseError::InvalidString),
        }

        Ok(())
    }

    /// Returns the bitboard of empty squares.
    #[inline]
    pub fn empty(&self) -> u64 {
        bitboard::empty_bb(self.black, self.white)
    }

    /// Returns the opponent's bitboard.
    #[inline]
    pub fn opponent_bb(&self) -> u64 {
        // A select, not a two-element array the compiler may spill.
        if matches!(self.player, Color::Black) {
            self.white
        } else {
            self.black
        }
    }

    /// Returns the current player's bitboard.
    #[inline]
    pub fn player_bb(&self) -> u64 {
        if matches!(self.player, Color::Black) {
            self.black
        } else {
            self.white
        }
    }

    /// Check if a move at `pos` is valid for the current player.
    pub fn check(&self, pos: Position) -> bool {
        let pos_bit = pos.to_bit();
        bitboard::check(self.player_bb(), self.opponent_bb(), pos_bit)
    }

    /// Check if either player can move (game end).
    pub fn check_all(&self) -> bool {
        bitboard::check_all(self.player_bb(), self.opponent_bb())
    }

    /// Make a move. Returns the list of flipped positions.
    /// Errors (Occupied / NotPlayable) leave the board unchanged.
    pub fn make_move(&mut self, pos: Position) -> Result<Vec<Position>, MoveError> {
        let pos_bit = pos.to_bit();

        if (self.black | self.white) & pos_bit != 0 {
            return Err(MoveError::Occupied);
        }

        // Single flippable computation serves both validation and application
        let flipped = bitboard::flippable(self.player_bb(), self.opponent_bb(), pos_bit);
        if flipped == 0 {
            return Err(MoveError::NotPlayable);
        }

        self.apply_move(pos_bit, flipped);
        let positions: Vec<Position> = bitboard::iter_bits(flipped)
            .filter_map(|i| Position::from_index(i as u32))
            .collect();
        Ok(positions)
    }

    /// Make a move (unchecked). Panics on invalid move.
    pub fn make_move_unchecked(&mut self, pos: Position) {
        self.make_move_bits(pos);
    }

    /// Make a move without legality checks and return the flipped bitboard.
    /// Returns 0 (and still mutates state incorrectly) if the move is not
    /// legal — callers must only pass moves from `movable()`.
    #[inline]
    pub fn make_move_bits(&mut self, pos: Position) -> u64 {
        let pos_bit = pos.to_bit();
        let flipped = bitboard::flippable(self.player_bb(), self.opponent_bb(), pos_bit);
        self.apply_move(pos_bit, flipped);
        flipped
    }

    /// Apply a move whose flips are already known. Callers that had to
    /// compute the flip mask anyway (to test legality, say) must use this
    /// instead of `make_move_bits`, which would recompute it.
    #[inline]
    pub fn apply_flips(&mut self, pos: Position, flipped: u64) {
        self.apply_move(pos.to_bit(), flipped);
    }

    /// Apply a precomputed move: place on `pos_bit`, recolor `flipped`.
    #[inline]
    fn apply_move(&mut self, pos_bit: u64, flipped: u64) {
        // Mover gains the placed piece and the flipped pieces; opponent loses the flipped pieces.
        // Branchless: the mover's board takes the placed disc plus the flips,
        // the other only the flips. Selecting the two masks costs a pair of
        // conditional moves instead of a branch that alternates every ply.
        let black_is_mover = matches!(self.player, Color::Black);
        let mover_mask = pos_bit | flipped;
        let (black_mask, white_mask) = if black_is_mover {
            (mover_mask, flipped)
        } else {
            (flipped, mover_mask)
        };
        self.black ^= black_mask;
        self.white ^= white_mask;

        self.player = self.player.opponent();

        // Flips only recolor occupied squares; the placement fills exactly one empty square.
        self.empty_count -= 1;
    }

    /// Undo a move: restore bitboards, player, empty_count.
    /// `pos` is the placed piece position, `flipped` is the bitboard of flipped pieces.
    pub fn undo_move(&mut self, pos: Position, flipped: u64) {
        // Restore player first so the XOR pattern mirrors make_move_bits.
        self.player = self.player.opponent();

        let pos_bit = pos.to_bit();
        match self.player {
            Color::Black => {
                self.black ^= pos_bit | flipped;
                self.white ^= flipped;
            }
            Color::White => {
                self.white ^= pos_bit | flipped;
                self.black ^= flipped;
            }
        }

        self.empty_count += 1;
    }

    /// Pass: swap player without changing the board.
    pub fn pass(&mut self) {
        self.player = self.player.opponent();
    }

    /// All 8 symmetric variants of this position (rotations and mirrors).
    /// Player and empty count are invariant under board symmetry.
    pub fn symmetries(&self) -> [Board; 8] {
        let blacks = bitboard::symmetries(self.black);
        let whites = bitboard::symmetries(self.white);
        std::array::from_fn(|i| Board {
            black: blacks[i],
            white: whites[i],
            player: self.player,
            empty_count: self.empty_count,
        })
    }

    /// Returns a bitboard of all playable positions for the current player.
    pub fn movable(&self) -> u64 {
        bitboard::mobility(self.player_bb(), self.opponent_bb(), self.empty())
    }

    /// Count of playable positions.
    pub fn movable_count(&self) -> u8 {
        bitboard::mobility_count(self.player_bb(), self.opponent_bb())
    }

    /// Iterate over playable positions.
    pub fn movable_iter(&self) -> impl Iterator<Item = Position> {
        let mut mob = self.movable();
        std::iter::from_fn(move || {
            if mob == 0 {
                None
            } else {
                let bit = mob.trailing_zeros();
                mob &= mob - 1;
                Position::from_index(bit)
            }
        })
    }

    /// Check if the game is over (neither player has a move).
    pub fn is_game_over(&self) -> bool {
        if self.empty_count == 0 {
            return true; // Board full
        }
        // Current player can move -> not over
        if self.check_all() {
            return false;
        }
        // Current player must pass; game continues only if the opponent can move
        !bitboard::check_all(self.opponent_bb(), self.player_bb())
    }

    /// Get piece count per color.
    pub fn piece_count(&self) -> (u8, u8) {
        (self.black.count_ones() as u8, self.white.count_ones() as u8)
    }

    /// Black disc count.
    pub fn black_count(&self) -> u8 {
        self.black.count_ones() as u8
    }

    /// White disc count.
    pub fn white_count(&self) -> u8 {
        self.white.count_ones() as u8
    }

    /// Number of empty squares.
    pub fn empty_count(&self) -> u8 {
        self.empty_count
    }

    /// Current player.
    pub fn player(&self) -> Color {
        self.player
    }

    /// Score from current player's perspective (black - white, signed).
    pub fn score(&self) -> i32 {
        let score = self.black.count_ones() as i32 - self.white.count_ones() as i32;
        if self.player == Color::White {
            -score
        } else {
            score
        }
    }

    /// Get the piece at a position.
    pub fn color_at(&self, pos: Position) -> Option<Color> {
        let bit = pos.to_bit();
        if self.black & bit != 0 {
            Some(Color::Black)
        } else if self.white & bit != 0 {
            Some(Color::White)
        } else {
            None
        }
    }

    /// Number of moves made (turn count).
    pub fn turn(&self) -> usize {
        60 - self.empty_count as usize
    }
}

impl std::fmt::Display for Board {
    /// Board string representation: rank-major (rank*8+file) 64 chars,
    /// then a space and the current player ('X' or 'O').
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for i in 0..64u32 {
            // Board string index i → file-major bit position
            let file = i % 8;
            let rank = i / 8;
            let bit = 1u64 << (file * 8 + rank);
            let c = if self.black & bit != 0 {
                'X'
            } else if self.white & bit != 0 {
                'O'
            } else {
                '-'
            };
            f.write_str(c.encode_utf8(&mut [0u8; 4]))?;
        }
        f.write_str(match self.player {
            Color::Black => " X",
            Color::White => " O",
        })
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_board() {
        let b = Board::new();
        assert_eq!(b.player(), Color::Black);
        assert_eq!(b.empty_count(), 60);
        assert_eq!(b.piece_count(), (2, 2));
        assert_eq!(b.to_string(), BOARD_INIT_STRING);
    }

    #[test]
    fn test_board_from_string() {
        let b = Board::from_string(BOARD_INIT_STRING).unwrap();
        assert_eq!(b.player(), Color::Black);
        assert_eq!(b.empty_count(), 60);
    }

    #[test]
    fn test_board_from_string_invalid() {
        assert!(Board::from_string("invalid").is_err());
    }

    #[test]
    fn test_initial_pieces() {
        let b = Board::new();
        assert_eq!(b.color_at(Position(27)), Some(Color::White)); // D4 (file=3, rank=3)
        assert_eq!(b.color_at(Position(35)), Some(Color::Black)); // E4 (file=4, rank=3)
        assert_eq!(b.color_at(Position(28)), Some(Color::Black)); // D5 (file=3, rank=4)
        assert_eq!(b.color_at(Position(36)), Some(Color::White)); // E5 (file=4, rank=4)
        assert_eq!(b.color_at(Position(0)), None); // A1 is empty
    }

    #[test]
    fn test_first_moves() {
        let b = Board::new();

        // Standard Reversi: Black's opening moves are D3, C4, F5, E6
        let movable = b.movable();
        assert_eq!(movable.count_ones(), 4, "Black should have 4 valid moves");

        // f5 (Position 44): f5 -> E5 (White) -> D5 (Black) flips E5
        assert!(b.check(Position(44))); // f5
    }

    #[test]
    fn test_make_move() {
        let mut b = Board::new();

        let movable = b.movable();
        assert!(movable != 0, "Should have valid moves");

        // Find first movable position
        let first = movable.trailing_zeros();
        let pos = Position::from_index(first).unwrap();

        // Make the move
        let result = b.make_move(pos);
        assert!(result.is_ok(), "First move should be valid: {:?}", pos);

        // Black places 1 and flips exactly 1 White in the opening
        assert_eq!(
            b.black.count_ones(),
            4,
            "Black should have 4 pieces after move"
        );
        assert_eq!(b.white.count_ones(), 1, "White should have 1 piece left");
        assert_eq!(b.player(), Color::White, "Player should switch to White");
        assert_eq!(b.empty_count(), 59, "One less empty square");
    }

    #[test]
    fn test_invalid_move_occupied() {
        let mut b = Board::new();
        // Try placing on E5 (already Black at Position 36)
        let result = b.make_move(Position(36));
        assert_eq!(result, Err(MoveError::Occupied));
    }

    #[test]
    fn test_invalid_move_not_playable() {
        let mut b = Board::new();
        // A1 (index 0) is far from any piece
        let result = b.make_move(Position(0));
        assert_eq!(result, Err(MoveError::NotPlayable));
    }

    #[test]
    fn test_movable() {
        let b = Board::new();
        let movable = b.movable();
        assert_eq!(
            movable.count_ones(),
            4,
            "Initial position: 4 moves for Black"
        );

        // All movable positions should be empty
        let empty = b.empty();
        assert_eq!(
            movable & empty,
            movable,
            "All movable positions must be empty"
        );
    }

    #[test]
    fn test_piece_count() {
        let b = Board::new();
        assert_eq!(b.piece_count(), (2, 2));
        assert_eq!(b.black_count(), 2);
        assert_eq!(b.white_count(), 2);
    }

    #[test]
    fn test_to_from_string_roundtrip() {
        let b = Board::new();
        let s = b.to_string();
        let b2 = Board::from_string(&s).unwrap();
        assert_eq!(b.black, b2.black);
        assert_eq!(b.white, b2.white);
        assert_eq!(b.player, b2.player);
    }

    #[test]
    fn test_score() {
        let b = Board::new();
        assert_eq!(b.score(), 0, "Initial score should be 0");

        let mut b = Board::new();
        let movable = b.movable();
        let first = movable.trailing_zeros();
        b.make_move_unchecked(Position::from_index(first).unwrap());
        // Black 4, White 1; current player is now White, so score = -(4-1)
        assert_eq!(
            b.score(),
            -3,
            "Score is from White's perspective after Black's move"
        );
    }

    #[test]
    fn test_turn() {
        let b = Board::new();
        assert_eq!(b.turn(), 0, "Initial turn count");

        let mut b = Board::new();
        let movable = b.movable();
        let first = movable.trailing_zeros();
        b.make_move_unchecked(Position::from_index(first).unwrap());
        assert_eq!(b.turn(), 1, "After 1 move, turn count is 1");
    }

    #[test]
    fn test_check_all() {
        let b = Board::new();
        assert!(
            b.check_all(),
            "Initial position: current player (Black) can move"
        );
    }

    #[test]
    fn test_is_game_over() {
        let b = Board::new();
        assert!(!b.is_game_over(), "Initial position: game not over");
    }

    #[test]
    fn test_movable_iter() {
        let b = Board::new();
        let from_iter: Vec<Position> = b.movable_iter().collect();
        assert_eq!(
            from_iter.len(),
            4,
            "Iterator must terminate and yield 4 moves"
        );
        for pos in &from_iter {
            assert!(b.check(*pos), "Every yielded move must be legal");
        }
    }

    #[test]
    fn test_is_game_over_pass_situations() {
        // Board full -> over regardless of anything else
        let mut b = Board::new();
        b.black = !0u64 ^ 0xFF;
        b.white = 0xFF;
        b.empty_count = 0;
        assert!(b.is_game_over(), "Full board is over");

        // Current player cannot move but opponent can -> NOT over (pass situation)
        // Black: A1. White: B1. Black to move: placing anywhere flips nothing
        // for Black... construct instead: White to move with no moves, Black has one.
        // Simple known position: Black A1(0), White B1(8), empty elsewhere.
        // Black to move: no move flips B1 without a Black anchor beyond it -> C1 works!
        // So use White to move: White has no legal move (nothing to flip).
        let mut b = Board::new();
        b.black = 1u64 << 0; // A1
        b.white = 1u64 << 8; // B1
        b.empty_count = 62;
        b.player = Color::White;
        // White cannot flip the lone black stone (no white anchor beyond it
        // in any direction), but Black CAN play C1 to flip B1.
        assert!(!b.check_all(), "White has no moves here");
        assert!(
            !b.is_game_over(),
            "White must pass, but Black can still move -> not over"
        );

        // Neither side can move -> over. Two isolated same-color-adjacent stones:
        // Black A1, Black B1: nothing to flip for either side.
        let mut b = Board::new();
        b.black = (1u64 << 0) | (1u64 << 8);
        b.white = 0;
        b.empty_count = 62;
        b.player = Color::Black;
        assert!(
            b.is_game_over(),
            "No flips possible for either side -> over"
        );
    }

    #[test]
    fn test_from_string_error_cases() {
        // Too short / too long
        assert!(Board::from_string("").is_err());
        assert!(
            Board::from_string(&"-".repeat(64)).is_err(),
            "missing player"
        );
        assert!(
            Board::from_string(&format!("{} XX", "-".repeat(64))).is_err(),
            "too long"
        );

        // Invalid piece character
        let mut s: String = "-".repeat(64);
        s.replace_range(0..1, "Z");
        assert!(Board::from_string(&format!("{s} X")).is_err());

        // Invalid player character
        assert!(Board::from_string(&format!("{} -", "-".repeat(64))).is_err());

        // White to move is accepted
        let b = Board::from_string(&format!("{} O", "-".repeat(64))).unwrap();
        assert_eq!(b.player(), Color::White);
        assert_eq!(b.empty_count(), 64);

        // Lowercase and alternate glyphs ('*' = black, '.' = empty)
        let mut s: String = ".".repeat(64);
        s.replace_range(0..1, "*");
        s.replace_range(1..2, "o");
        let b = Board::from_string(&format!("{s} x")).unwrap();
        assert_eq!(b.piece_count(), (1, 1));
        assert_eq!(b.player(), Color::Black);
    }

    #[test]
    fn test_string_roundtrip_midgame() {
        // Round-trip through the string form must be lossless mid-game too
        let mut b = Board::new();
        for _ in 0..10 {
            let moves = b.movable();
            if moves == 0 {
                break;
            }
            b.make_move_unchecked(Position::from_index(moves.trailing_zeros()).unwrap());
        }
        let b2 = Board::from_string(&b.to_string()).unwrap();
        assert_eq!(b.black, b2.black);
        assert_eq!(b.white, b2.white);
        assert_eq!(b.player, b2.player);
        assert_eq!(b.empty_count(), b2.empty_count());
    }

    #[test]
    fn test_undo_move_roundtrip() {
        let mut b = Board::new();
        let orig = b;
        let pos = Position::from_index(b.movable().trailing_zeros()).unwrap();
        let flipped = b.make_move_bits(pos);
        b.undo_move(pos, flipped);
        assert_eq!(b.black, orig.black);
        assert_eq!(b.white, orig.white);
        assert_eq!(b.player, orig.player);
        assert_eq!(b.empty_count, orig.empty_count);
    }
}
