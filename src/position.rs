//! Board position: A1=0, B1=1, ..., H8=63.
//! Supports KIFU notation conversion (e.g., "f5").

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Position(pub u8);

impl Position {
    /// Minimum position index (A1).
    pub const MIN: u8 = 0;
    /// Maximum position index (H8).
    pub const MAX: u8 = 63;

    /// Creates a Position from a raw index (0..63).
    /// Returns None if out of range.
    #[inline]
    pub const fn from_index(i: u32) -> Option<Position> {
        if i <= Self::MAX as u32 {
            Some(Position(i as u8))
        } else {
            None
        }
    }

    /// Creates a Position from file (0=A, 7=H) and rank (0=1, 7=8).
    /// Returns None if either is out of range.
    #[inline]
    pub const fn from_file_rank(file: u8, rank: u8) -> Option<Position> {
        if file < 8 && rank < 8 {
            Some(Position(file * 8 + rank))
        } else {
            None
        }
    }

    /// Returns the raw index (0..63).
    #[inline]
    pub const fn index(self) -> u8 {
        self.0
    }

    /// Returns the file (column) as 0..7 (A..H).
    /// Position stores file-major (file*8+rank).
    #[inline]
    pub const fn file(self) -> u8 {
        self.0 / 8
    }

    /// Returns the rank (row) as 0..7 (1..8).
    /// Position stores file-major (file*8+rank).
    #[inline]
    pub const fn rank(self) -> u8 {
        self.0 % 8
    }

    /// Returns the bitboard bit for this position.
    #[inline]
    pub const fn to_bit(self) -> u64 {
        1u64 << self.0
    }

    /// Converts to KIFU notation (e.g., "f5").
    pub fn to_kifu(self) -> String {
        let col = (b'a' + self.file()) as char;
        let row = (b'1' + self.rank()) as char;
        format!("{}{}", col, row)
    }

    /// Parses KIFU notation (e.g., "f5") to Position.
    pub fn from_kifu(s: &str) -> Result<Position, String> {
        let bytes = s.as_bytes();
        if bytes.len() != 2 {
            return Err(format!("棋譜として読み取れません: {s}"));
        }
        let file = bytes[0].wrapping_sub(b'a');
        let rank = bytes[1].wrapping_sub(b'1');
        match Position::from_file_rank(file, rank) {
            Some(pos) => Ok(pos),
            None => Err(format!("invalid KIFU: {}", s)),
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_kifu())
    }
}

/// Parse a pair of KIFU strings (e.g., "f5g5" -> (Position, Position)).
pub fn kifu_pair(s: &str) -> Result<(Position, Position), String> {
    if s.len() != 4 {
        return Err(format!("invalid KIFU pair: {}", s));
    }
    Ok((
        Position::from_kifu(&s[..2])?,
        Position::from_kifu(&s[2..4])?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_index() {
        assert_eq!(Position::from_index(0), Some(Position(0))); // A1
        assert_eq!(Position::from_index(63), Some(Position(63))); // H8
        assert_eq!(Position::from_index(64), None);
    }

    #[test]
    fn test_from_file_rank() {
        assert_eq!(Position::from_file_rank(0, 0), Some(Position(0))); // A1
        assert_eq!(Position::from_file_rank(5, 4), Some(Position(44))); // F5 (file*8+rank = 5*8+4)
        assert_eq!(Position::from_file_rank(6, 4), Some(Position(52))); // G5 (6*8+4)
        assert_eq!(Position::from_file_rank(7, 7), Some(Position(63))); // H8
        assert_eq!(Position::from_file_rank(8, 0), None);
    }

    #[test]
    fn test_file_rank() {
        let p = Position(44); // F5
        assert_eq!(p.file(), 5); // F
        assert_eq!(p.rank(), 4); // 5 (0-indexed)
    }

    #[test]
    fn test_to_kifu() {
        assert_eq!(Position(0).to_kifu(), "a1");
        assert_eq!(Position(44).to_kifu(), "f5");
        assert_eq!(Position(52).to_kifu(), "g5");
        assert_eq!(Position(63).to_kifu(), "h8");
    }

    #[test]
    fn test_from_kifu() {
        assert_eq!(Position::from_kifu("a1"), Ok(Position(0)));
        assert_eq!(Position::from_kifu("f5"), Ok(Position(44)));
        assert_eq!(Position::from_kifu("g5"), Ok(Position(52)));
        assert_eq!(Position::from_kifu("h8"), Ok(Position(63)));
        assert!(Position::from_kifu("z9").is_err());
        assert!(Position::from_kifu("a").is_err());
    }

    #[test]
    fn test_to_bit() {
        assert_eq!(Position(0).to_bit(), 1u64);
        assert_eq!(Position(44).to_bit(), 1u64 << 44);
        assert_eq!(Position(63).to_bit(), 1u64 << 63);
    }

    #[test]
    fn test_kifu_pair() {
        assert_eq!(kifu_pair("f5g5"), Ok((Position(44), Position(52))));
        assert!(kifu_pair("fg").is_err());
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Position(44)), "f5");
    }
}
