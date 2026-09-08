//! WTHOR game files (`.wtb`), the tournament-record archive.
//!
//! Layout: a 16-byte file header, then 68 bytes per game -- tournament
//! (u16), Black (u16), White (u16), Black's real disc count (u8), Black's
//! theoretical disc count (u8), then 60 move bytes. A move is decimal
//! `row * 10 + col`, both 1-based (`f5` is 56); 0 ends the list. The real
//! disc count already gives the empties to the winner.

use std::path::Path;

use crate::Position;

/// One game: the moves as played, and Black's final disc count as the
/// archive records it (empties to the winner).
pub struct Game {
    pub moves: Vec<Position>,
    pub black_discs: u8,
}

impl Game {
    /// The moves in `f5d6` notation, the form every other tool reads.
    pub fn transcript(&self) -> String {
        self.moves.iter().map(|p| p.to_kifu()).collect()
    }

    /// Final disc difference for Black, empties to the winner.
    pub fn score_black(&self) -> i32 {
        2 * i32::from(self.black_discs) - 64
    }
}

/// Every game in one file. A game with a move outside the board is
/// dropped; the caller decides what to do with games that did not run to
/// the end.
pub fn read(path: &Path) -> std::io::Result<Vec<Game>> {
    let data = std::fs::read(path)?;
    let mut games = Vec::new();
    let mut off = 16;
    while off + 68 <= data.len() {
        let rec = &data[off..off + 68];
        off += 68;
        let mut moves = Vec::new();
        let mut ok = true;
        for &v in &rec[8..68] {
            if v == 0 {
                break;
            }
            let (row, col) = (v / 10, v % 10);
            match Position::from_file_rank(col.wrapping_sub(1), row.wrapping_sub(1)) {
                Some(p) => moves.push(p),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            games.push(Game {
                moves,
                black_discs: rec[6],
            });
        }
    }
    Ok(games)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decodes_a_game_record() {
        let mut data = vec![0u8; 16];
        let mut rec = vec![0u8; 68];
        rec[6] = 40;
        rec[8] = 56; // f5
        rec[9] = 66; // f6
        data.extend(rec);
        let games = read_from_bytes("game", &data);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].transcript(), "f5f6");
        assert_eq!(games[0].score_black(), 16);
    }

    #[test]
    fn test_drops_a_game_with_a_bad_square() {
        let mut data = vec![0u8; 16];
        let mut rec = vec![0u8; 68];
        rec[8] = 99;
        data.extend(rec);
        assert!(read_from_bytes("bad", &data).is_empty());
    }

    fn read_from_bytes(name: &str, data: &[u8]) -> Vec<Game> {
        let p =
            std::env::temp_dir().join(format!("kuroobi-wthor-{}-{name}.wtb", std::process::id()));
        std::fs::write(&p, data).unwrap();
        let g = read(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        g
    }
}
