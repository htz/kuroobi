//! Board piece color. Black=0, White=1. Opponent is XOR with 1.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    White = 1,
}

impl Color {
    /// Returns the opponent color. Black <-> White.
    #[inline]
    pub const fn opponent(self) -> Color {
        match self {
            Color::Black => Color::White,
            Color::White => Color::Black,
        }
    }

    /// Returns the numeric index (0 or 1).
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Creates a Color from a numeric index.
    #[inline]
    pub const fn from_usize(u: usize) -> Option<Color> {
        match u {
            0 => Some(Color::Black),
            1 => Some(Color::White),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Color;

    #[test]
    fn test_opponent() {
        assert_eq!(Color::Black.opponent(), Color::White);
        assert_eq!(Color::White.opponent(), Color::Black);
    }

    #[test]
    fn test_index() {
        assert_eq!(Color::Black.index(), 0);
        assert_eq!(Color::White.index(), 1);
    }

    #[test]
    fn test_from_usize() {
        assert_eq!(Color::from_usize(0), Some(Color::Black));
        assert_eq!(Color::from_usize(1), Some(Color::White));
        assert_eq!(Color::from_usize(2), None);
    }
}
