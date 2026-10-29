//! Board state management using 2x uint64 bitboards.

use crate::bitboard;
use crate::color::Color;
use crate::position::Position;
use std::fmt::Write;

#[derive(Debug, Clone, Copy)]
pub struct Board {
    pub black: u64,
    pub white: u64,
    pub player: Color,
    pub empty_count: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MoveError {
    NotPlayable,
    Occupied,
}

impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoveError::NotPlayable => write!(f, "not playable (no flip)"),
            MoveError::Occupied => write!(f, "square already occupied"),
        }
    }
}

impl std::error::Error for MoveError {}

#[derive(Debug, Clone, PartialEq)]
pub enum GameError {
    NoMoves,
}

impl std::fmt::Display for GameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GameError::NoMoves => write!(f, "no moves available"),
        }
    }
}

impl std::error::Error for GameError {}

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

pub const BOARD_INIT_STRING: &str =
    "---------------------------OX------XO--------------------------- X";

impl Board {
    pub fn new() -> Board {
        Board {
            black: (1u64 << 35) | (1u64 << 28), // E4 + D5
            white: (1u64 << 27) | (1u64 << 36), // D4 + E5
            player: Color::Black,
            empty_count: 60,
        }
    }

    pub fn from_string(s: &str) -> Result<Board, ParseError> {
        let mut b = Board {
            black: 0,
            white: 0,
            player: Color::Black,
            empty_count: 0,
        };
        let mut n = 0u32;
        for ch in s.chars().filter(|c| !c.is_whitespace()) {
            let bit = 1u64.checked_shl((n % 8) * 8 + n / 8).unwrap_or(0);
            match (n, ch.to_ascii_lowercase()) {
                (0..=63, 'x' | '*') => b.black |= bit,
                (0..=63, 'o') => b.white |= bit,
                (0..=63, '-' | '.') => b.empty_count += 1,
                (64, 'x' | '*') => b.player = Color::Black,
                (64, 'o') => b.player = Color::White,
                _ => return Err(ParseError::InvalidString),
            }
            n += 1;
        }
        if n != 65 {
            return Err(ParseError::InvalidString);
        }
        Ok(b)
    }

    #[inline]
    pub fn empty(&self) -> u64 {
        bitboard::empty_bb(self.black, self.white)
    }

    #[inline]
    pub fn opponent_bb(&self) -> u64 {
        if matches!(self.player, Color::Black) {
            self.white
        } else {
            self.black
        }
    }

    #[inline]
    pub fn player_bb(&self) -> u64 {
        if matches!(self.player, Color::Black) {
            self.black
        } else {
            self.white
        }
    }

    pub fn check(&self, pos: Position) -> bool {
        let pos_bit = pos.to_bit();
        self.empty() & pos_bit != 0
            && bitboard::flippable(self.player_bb(), self.opponent_bb(), pos_bit) != 0
    }

    pub fn check_all(&self) -> bool {
        bitboard::check_all(self.player_bb(), self.opponent_bb())
    }

    pub fn make_move(&mut self, pos: Position) -> Result<u64, MoveError> {
        let pos_bit = pos.to_bit();

        if (self.black | self.white) & pos_bit != 0 {
            return Err(MoveError::Occupied);
        }

        let flipped = bitboard::flippable(self.player_bb(), self.opponent_bb(), pos_bit);
        if flipped == 0 {
            return Err(MoveError::NotPlayable);
        }

        self.apply_move(pos_bit, flipped);
        Ok(flipped)
    }

    #[inline]
    pub fn make_move_bits(&mut self, pos: Position) -> u64 {
        let pos_bit = pos.to_bit();
        let flipped = bitboard::flippable(self.player_bb(), self.opponent_bb(), pos_bit);
        self.apply_move(pos_bit, flipped);
        flipped
    }

    #[inline]
    pub fn apply_flips(&mut self, pos: Position, flipped: u64) {
        self.apply_move(pos.to_bit(), flipped);
    }

    #[inline]
    fn apply_move(&mut self, pos_bit: u64, flipped: u64) {
        self.xor_move(pos_bit, flipped);
        self.player = self.player.opponent();
        self.empty_count -= 1;
    }

    pub fn undo_move(&mut self, pos: Position, flipped: u64) {
        self.player = self.player.opponent();
        self.xor_move(pos.to_bit(), flipped);
        self.empty_count += 1;
    }

    /// Places or lifts the side to move's disc and toggles the flipped ones.
    #[inline]
    fn xor_move(&mut self, pos_bit: u64, flipped: u64) {
        let mover_mask = pos_bit | flipped;
        let (black_mask, white_mask) = if matches!(self.player, Color::Black) {
            (mover_mask, flipped)
        } else {
            (flipped, mover_mask)
        };
        self.black ^= black_mask;
        self.white ^= white_mask;
    }

    pub fn pass(&mut self) {
        self.player = self.player.opponent();
    }

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

    pub fn movable(&self) -> u64 {
        bitboard::mobility(self.player_bb(), self.opponent_bb(), self.empty())
    }

    pub fn movable_count(&self) -> u8 {
        self.movable().count_ones() as u8
    }

    pub fn movable_iter(&self) -> impl Iterator<Item = Position> {
        bitboard::iter_bits(self.movable()).map(Position)
    }

    pub fn is_game_over(&self) -> bool {
        if self.empty_count == 0 {
            return true;
        }
        if self.check_all() {
            return false;
        }
        !bitboard::check_all(self.opponent_bb(), self.player_bb())
    }

    pub fn piece_count(&self) -> (u8, u8) {
        (self.black.count_ones() as u8, self.white.count_ones() as u8)
    }

    pub fn empty_count(&self) -> u8 {
        self.empty_count
    }

    pub fn player(&self) -> Color {
        self.player
    }

    pub fn score(&self) -> i32 {
        let score = self.black.count_ones() as i32 - self.white.count_ones() as i32;
        if self.player == Color::White {
            -score
        } else {
            score
        }
    }
}

impl std::fmt::Display for Board {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for i in 0..64u32 {
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
            f.write_char(c)?;
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
    fn test_first_moves() {
        let b = Board::new();

        let movable = b.movable();
        assert_eq!(movable.count_ones(), 4, "Black should have 4 valid moves");

        assert!(b.check(Position(44))); // f5
    }

    #[test]
    fn test_make_move() {
        let mut b = Board::new();

        let movable = b.movable();
        assert!(movable != 0, "Should have valid moves");

        let first = movable.trailing_zeros();
        let pos = Position::from_index(first).unwrap();

        let result = b.make_move(pos);
        assert!(result.is_ok(), "First move should be valid: {:?}", pos);

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
        let result = b.make_move(Position(36));
        assert_eq!(result, Err(MoveError::Occupied));
    }

    #[test]
    fn test_invalid_move_not_playable() {
        let mut b = Board::new();
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
        b.make_move_bits(Position::from_index(first).unwrap());
        assert_eq!(
            b.score(),
            -3,
            "Score is from White's perspective after Black's move"
        );
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
        let mut b = Board::new();
        b.black = !0u64 ^ 0xFF;
        b.white = 0xFF;
        b.empty_count = 0;
        assert!(b.is_game_over(), "Full board is over");

        let mut b = Board::new();
        b.black = 1u64 << 0; // A1
        b.white = 1u64 << 8; // B1
        b.empty_count = 62;
        b.player = Color::White;
        assert!(!b.check_all(), "White has no moves here");
        assert!(
            !b.is_game_over(),
            "White must pass, but Black can still move -> not over"
        );

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
        assert!(Board::from_string("").is_err());
        assert!(
            Board::from_string(&"-".repeat(64)).is_err(),
            "missing player"
        );
        assert!(
            Board::from_string(&format!("{} XX", "-".repeat(64))).is_err(),
            "too long"
        );

        let mut s: String = "-".repeat(64);
        s.replace_range(0..1, "Z");
        assert!(Board::from_string(&format!("{s} X")).is_err());

        assert!(Board::from_string(&format!("{} -", "-".repeat(64))).is_err());

        let b = Board::from_string(&format!("{} O", "-".repeat(64))).unwrap();
        assert_eq!(b.player(), Color::White);
        assert_eq!(b.empty_count(), 64);

        let mut s: String = ".".repeat(64);
        s.replace_range(0..1, "*");
        s.replace_range(1..2, "o");
        let b = Board::from_string(&format!("{s} x")).unwrap();
        assert_eq!(b.piece_count(), (1, 1));
        assert_eq!(b.player(), Color::Black);
    }

    #[test]
    fn test_string_roundtrip_midgame() {
        let mut b = Board::new();
        for _ in 0..10 {
            let moves = b.movable();
            if moves == 0 {
                break;
            }
            b.make_move_bits(Position::from_index(moves.trailing_zeros()).unwrap());
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
