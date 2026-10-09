//! WTHOR game files (`.wtb`), the tournament-record archive.

use std::path::Path;

use crate::Position;

pub fn read(path: &Path) -> std::io::Result<Vec<Vec<Position>>> {
    Ok(parse(&std::fs::read(path)?))
}

fn parse(data: &[u8]) -> Vec<Vec<Position>> {
    data.get(16..)
        .unwrap_or_default()
        .as_chunks::<68>()
        .0
        .iter()
        .filter_map(|rec| {
            rec[8..68]
                .iter()
                .take_while(|&&v| v != 0)
                .map(|&v| {
                    Position::from_file_rank((v % 10).wrapping_sub(1), (v / 10).wrapping_sub(1))
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decodes_a_game_record() {
        let mut data = vec![0u8; 16];
        let mut rec = vec![0u8; 68];
        rec[8] = 56; // f5
        rec[9] = 66; // f6
        data.extend(rec);
        let games = parse(&data);
        assert_eq!(games.len(), 1);
        let transcript: String = games[0].iter().map(|p| p.to_kifu()).collect();
        assert_eq!(transcript, "f5f6");
    }

    #[test]
    fn test_drops_a_game_with_a_bad_square() {
        let mut data = vec![0u8; 16];
        let mut rec = vec![0u8; 68];
        rec[8] = 99;
        data.extend(rec);
        assert!(parse(&data).is_empty());
    }
}
