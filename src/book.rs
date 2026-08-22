//! Opening book.
//!
//! A position -> (best move, value) table keyed by the 8-symmetry
//! normal form, so rotated/mirrored positions collapse into one entry
//! (human game records skew toward f5 lines; without normalization most
//! transpositions are missed).
//!
//! Values are assumed to come from deeper-than-game search; `bookgen`
//! therefore runs with depth and solve entry above game settings.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::{Board, Position};

/// One book candidate.
#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    /// Move in the normalized orientation.
    pub mv: Position,
    /// Value in discs from the mover's view, from deep search.
    pub value: f32,
    /// How often game records chose this move.
    pub games: u32,
}

/// One book entry. It keeps every candidate so play can pick among moves
/// within a tolerance of best, avoiding repeated identical games.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    /// Sorted by value, descending; [0] is best when non-empty.
    pub moves: Vec<Candidate>,
    /// Search depth behind the values (0 = frequency only).
    pub depth: u8,
    /// How often the position appeared in game records.
    pub games: u32,
}

impl Entry {
    pub fn best(&self) -> Option<&Candidate> {
        self.moves.first()
    }

    /// Remove candidate `mv` (used when un-importing a move that did not exist before).
    pub fn remove_move(&mut self, mv: Position) {
        self.moves.retain(|c| c.mv != mv);
    }

    /// Re-value candidate `mv` (normalized space), inserting if absent.
    /// Descending order is preserved, so a re-value may change the best
    /// move (game-outcome learning uses this).
    pub fn update_move(&mut self, mv: Position, value: f32) {
        match self.moves.iter_mut().find(|c| c.mv == mv) {
            Some(c) => c.value = value,
            None => self.moves.push(Candidate {
                mv,
                value,
                games: 0,
            }),
        }
        self.moves.sort_by(|a, b| b.value.total_cmp(&a.value));
    }
}

/// Normal form = lexicographically smallest of the 8 symmetries; returns
/// the transform index so moves can be mapped back.
fn normalize(board: &Board) -> (u64, u64, u8) {
    let mut best = (board.player_bb(), board.opponent_bb());
    let mut best_i = 0u8;
    let mut p = board.player_bb();
    let mut o = board.opponent_bb();
    // All 8 symmetries from transpose + horizontal mirror.
    for i in 0..8u8 {
        if i > 0 {
            // Four rotations, then switch to the mirrored family.
            if i == 4 {
                p = crate::bitboard::mirror_horizontal(board.player_bb());
                o = crate::bitboard::mirror_horizontal(board.opponent_bb());
            } else {
                p = crate::bitboard::rotate_90(p);
                o = crate::bitboard::rotate_90(o);
            }
        }
        if (p, o) < best {
            best = (p, o);
            best_i = i;
        }
    }
    (best.0, best.1, best_i)
}

/// Candidate mapped back to board orientation (square, mover-view value, adoption count).
pub type BookMove = (Position, f32, u32);

/// Apply transform `i` to a whole bitboard (`map_square` for all 64 squares).
fn transform_bb(bb: u64, i: u8) -> u64 {
    let mut b = bb;
    if i >= 4 {
        b = crate::bitboard::mirror_horizontal(b);
        for _ in 0..(i - 4) {
            b = crate::bitboard::rotate_90(b);
        }
    } else {
        for _ in 0..i {
            b = crate::bitboard::rotate_90(b);
        }
    }
    b
}

/// Transforms that leave the board unchanged (stabilizers). Symmetric
/// boards (e.g. the opening) have several; there one stored move stands
/// for every equivalent move.
pub fn stabilizers(board: &Board) -> Vec<u8> {
    let (p, o) = (board.player_bb(), board.opponent_bb());
    (0..8u8)
        .filter(|&i| (transform_bb(p, i), transform_bb(o, i)) == (p, o))
        .collect()
}

/// Apply normalization transform `i` to one square.
fn map_square(sq: u8, i: u8) -> u8 {
    let mut bit = 1u64 << sq;
    if i >= 4 {
        bit = crate::bitboard::mirror_horizontal(bit);
        for _ in 0..(i - 4) {
            bit = crate::bitboard::rotate_90(bit);
        }
    } else {
        for _ in 0..i {
            bit = crate::bitboard::rotate_90(bit);
        }
    }
    bit.trailing_zeros() as u8
}

/// Apply the inverse of transform `i` to one square.
fn unmap_square(sq: u8, i: u8) -> u8 {
    // Brute-force the inverse: apply until the square returns.
    for cand in 0..64u8 {
        if map_square(cand, i) == sq {
            return cand;
        }
    }
    sq
}

/// Rebuild a board from a normalized (player, opponent) key. Keys always
/// have the mover as `player`, so building it as Black keeps the view.
pub fn board_from_key(key: (u64, u64)) -> Board {
    let mut b = Board::new();
    b.black = key.0;
    b.white = key.1;
    b.player = crate::Color::Black;
    b.empty_count = (!(key.0 | key.1)).count_ones() as u8;
    b
}

#[derive(Default)]
pub struct Book {
    map: HashMap<(u64, u64), Entry>,
}

/// One candidate: (square, mover-view value, adoption count).
pub type BookCandidate = (Position, f32, u32);

impl Book {
    pub fn new() -> Book {
        Book {
            map: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Direct lookup by normalized key (for the generator).
    pub fn get_raw(&self, key: (u64, u64)) -> Option<&Entry> {
        self.map.get(&key)
    }

    /// Direct lookup by normalized key (for learning write-back).
    pub fn get_raw_mut(&mut self, key: (u64, u64)) -> Option<&mut Entry> {
        self.map.get_mut(&key)
    }

    /// Remove by normalized key (used when un-importing).
    pub fn remove_raw(&mut self, key: (u64, u64)) -> Option<Entry> {
        self.map.remove(&key)
    }

    pub fn insert_raw(&mut self, key: (u64, u64), e: Entry) {
        self.map.insert(key, e);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&(u64, u64), &Entry)> {
        self.map.iter()
    }

    /// Look up the position and return the best move (deterministic; for study/verification).
    pub fn probe(&self, board: &Board) -> Option<(Position, f32, u8)> {
        let (cands, depth) = self.expand(board)?;
        // Sorted by value, descending.
        cands.first().map(|(p, v, _)| (*p, *v, depth))
    }

    /// Look up and return candidates mapped back to board orientation
    /// (move, value, count) plus depth.
    ///
    /// On symmetric boards one stored move represents every equivalent
    /// move, so candidates are expanded through the stabilizers;
    /// otherwise equivalent moves would be missing from display and the
    /// randomized pick would favor one orientation.
    fn expand(&self, board: &Board) -> Option<(Vec<BookMove>, u8)> {
        let (key, i) = Book::key(board);
        let e = self.map.get(&key)?;
        let stab = stabilizers(board);
        let mut out: Vec<BookMove> = Vec::new();
        for c in &e.moves {
            let Some(p0) = Self::back(c.mv, i) else {
                continue;
            };
            for &g in &stab {
                let Some(p) = Position::from_index(map_square(p0.index(), g) as u32) else {
                    continue;
                };
                if !board.check(p) || out.iter().any(|(q, _, _)| *q == p) {
                    continue;
                }
                out.push((p, c.value, c.games));
            }
        }
        if out.is_empty() {
            return None;
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        Some((out, e.depth))
    }

    /// Look up and pick one candidate within `tolerance` discs of best.
    ///
    /// Avoids replaying identical games against the same opponent; moves
    /// within tolerance are effectively equal so strength is preserved.
    /// Weighted by human adoption count (+1) so book-like moves appear
    /// more often.
    pub fn probe_varied(
        &self,
        board: &Board,
        tolerance: f32,
        rand: u64,
    ) -> Option<(Position, f32, u8)> {
        let (all, depth) = self.expand(board)?;
        let best = all.first()?.1;
        // Collect candidates within tolerance.
        let cands: Vec<(Position, f32, u64)> = all
            .into_iter()
            .filter(|(_, v, _)| *v >= best - tolerance)
            .map(|(p, v, g)| (p, v, g as u64 + 1))
            .collect();
        if cands.is_empty() {
            return None;
        }
        let total: u64 = cands.iter().map(|(_, _, w)| *w).sum();
        let mut pick = rand % total.max(1);
        for (p, v, w) in &cands {
            if pick < *w {
                return Some((*p, *v, depth));
            }
            pick -= *w;
        }
        let (p, v, _) = cands[0];
        Some((p, v, depth))
    }

    /// Candidates in board orientation (legal only, by value desc); for display.
    pub fn candidates(&self, board: &Board) -> Option<Vec<(Position, f32)>> {
        let (out, _) = self.expand(board)?;
        Some(out.into_iter().map(|(p, v, _)| (p, v)).collect())
    }

    /// Candidates including adoption counts (for browsing the book);
    /// `candidates` drops counts because play does not need them.
    pub fn candidates_detailed(&self, board: &Board) -> Option<Vec<BookCandidate>> {
        let (out, _) = self.expand(board)?;
        Some(out)
    }

    /// Whether the position exists in the book (even with no playable candidate).
    pub fn has(&self, board: &Board) -> bool {
        self.map.contains_key(&Book::key(board).0)
    }

    /// Map a normalized-space move back to board orientation.
    fn back(mv: Position, i: u8) -> Option<Position> {
        Position::from_index(unmap_square(mv.index(), i) as u32)
    }

    /// Normalize a board into a key (shared with the generator).
    pub fn key(board: &Board) -> ((u64, u64), u8) {
        let (p, o, i) = normalize(board);
        ((p, o), i)
    }

    /// Map a move into normalized space.
    pub fn map_move(pos: Position, i: u8) -> Position {
        Position(map_square(pos.index(), i))
    }

    /// Save as text. One line =
    /// `player_hex opponent_hex depth games | mv:value:games mv:value:games ...`
    /// Text rather than binary: diffs stay readable and shell tools work.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            writeln!(f, "KUROOBI_BOOK_2")?;
            for ((p, o), e) in &self.map {
                write!(f, "{p:016x} {o:016x} {} {}", e.depth, e.games)?;
                for c in &e.moves {
                    write!(f, " {}:{:.3}:{}", c.mv.index(), c.value, c.games)?;
                }
                writeln!(f)?;
            }
            f.flush()?;
        }
        std::fs::rename(tmp, path)
    }

    pub fn load(path: &Path) -> std::io::Result<Book> {
        let f = BufReader::new(std::fs::File::open(path)?);
        let mut book = Book::new();
        let mut v1 = false;
        for (i, line) in f.lines().enumerate() {
            let line = line?;
            if i == 0 {
                match line.trim() {
                    "KUROOBI_BOOK_2" => {}
                    "KUROOBI_BOOK_1" => v1 = true, // legacy format (best move only)
                    _ => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "bad book magic",
                        ))
                    }
                }
                continue;
            }
            let t: Vec<&str> = line.split_whitespace().collect();
            if t.len() < 4 {
                continue;
            }
            let (Ok(p), Ok(o)) = (u64::from_str_radix(t[0], 16), u64::from_str_radix(t[1], 16))
            else {
                continue;
            };
            if v1 {
                // Legacy: best value depth games
                let (Ok(best), Ok(value), Ok(depth), Ok(games)) = (
                    t[2].parse::<u8>(),
                    t[3].parse::<f32>(),
                    t.get(4).unwrap_or(&"0").parse::<u8>(),
                    t.get(5).unwrap_or(&"0").parse::<u32>(),
                ) else {
                    continue;
                };
                let Some(mv) = Position::from_index(best as u32) else {
                    continue;
                };
                book.map.insert(
                    (p, o),
                    Entry {
                        moves: vec![Candidate { mv, value, games }],
                        depth,
                        games,
                    },
                );
                continue;
            }
            let (Ok(depth), Ok(games)) = (t[2].parse::<u8>(), t[3].parse::<u32>()) else {
                continue;
            };
            let mut moves = Vec::new();
            for tok in &t[4..] {
                let mut it = tok.split(':');
                let (Some(m), Some(v), Some(g)) = (it.next(), it.next(), it.next()) else {
                    continue;
                };
                let (Ok(m), Ok(v), Ok(g)) = (m.parse::<u8>(), v.parse::<f32>(), g.parse::<u32>())
                else {
                    continue;
                };
                let Some(mv) = Position::from_index(m as u32) else {
                    continue;
                };
                moves.push(Candidate {
                    mv,
                    value: v,
                    games: g,
                });
            }
            moves.sort_by(|a, b| b.value.total_cmp(&a.value));
            book.map.insert(
                (p, o),
                Entry {
                    moves,
                    depth,
                    games,
                },
            );
        }
        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Board;

    fn entry(mv: Position, value: f32, games: u32) -> Entry {
        Entry {
            moves: vec![Candidate { mv, value, games }],
            depth: 24,
            games,
        }
    }

    /// The four symmetric openings collapse into one key.
    #[test]
    fn opening_symmetries_collapse() {
        let b = Board::new();
        let mut keys = std::collections::HashSet::new();
        for p in b.movable_iter() {
            let mut c = b;
            c.make_move_bits(p);
            let (k, _) = Book::key(&c);
            keys.insert(k);
        }
        assert_eq!(keys.len(), 1, "the 4 symmetric opening moves share one key");
    }

    /// A move stored normalized comes back correct for any orientation.
    #[test]
    fn probe_maps_move_back() {
        let b = Board::new();
        let mut book = Book::new();
        let f5 = Position::from_kifu("f5").unwrap();
        let mut after = b;
        after.make_move_bits(f5);
        let (key, i) = Book::key(&after);
        let d6 = Position::from_kifu("d6").unwrap();
        book.insert_raw(key, entry(Book::map_move(d6, i), 1.5, 100));

        for p in b.movable_iter() {
            let mut c = b;
            c.make_move_bits(p);
            let got = book.probe(&c);
            assert!(got.is_some(), "book lookup failed for symmetric {p:?}");
            let (mv, v, depth) = got.unwrap();
            assert!(c.check(mv), "returned move must be legal");
            assert_eq!(v, 1.5);
            assert_eq!(depth, 24);
        }
    }

    /// Picks spread within tolerance; blunders outside it are never picked.
    #[test]
    fn varied_choice_stays_within_tolerance() {
        let b = Board::new();
        let (key, i) = Book::key(&b);
        // The opening collapses to one move, so test from move two.
        let mut after = b;
        after.make_move_bits(Position::from_kifu("f5").unwrap());
        let (key2, i2) = Book::key(&after);
        let _ = (key, i);

        let legal: Vec<Position> = after.movable_iter().collect();
        assert!(legal.len() >= 3);
        // Two near-equal moves (0.0 / -0.5) and one clear blunder (-5.0).
        let moves: Vec<Candidate> = vec![
            Candidate {
                mv: Book::map_move(legal[0], i2),
                value: 0.0,
                games: 50,
            },
            Candidate {
                mv: Book::map_move(legal[1], i2),
                value: -0.5,
                games: 30,
            },
            Candidate {
                mv: Book::map_move(legal[2], i2),
                value: -5.0,
                games: 5,
            },
        ];
        let mut book = Book::new();
        book.insert_raw(
            key2,
            Entry {
                moves,
                depth: 26,
                games: 85,
            },
        );

        let mut seen = std::collections::HashSet::new();
        for r in 0..200u64 {
            let (mv, v, _) = book.probe_varied(&after, 1.0, r).expect("lookup succeeds");
            assert!(after.check(mv));
            assert!(v >= -1.0, "picked a blunder outside tolerance: {v}");
            seen.insert(mv.index());
        }
        assert!(seen.len() >= 2, "picks should spread (got {})", seen.len());
        assert!(
            seen.len() <= 2,
            "outside-tolerance moves must be excluded (got {})",
            seen.len()
        );
    }

    #[test]
    fn save_load_roundtrip() {
        let mut book = Book::new();
        let b = Board::new();
        let (key, _) = Book::key(&b);
        book.insert_raw(
            key,
            Entry {
                moves: vec![
                    Candidate {
                        mv: Position::from_kifu("f5").unwrap(),
                        value: -2.0,
                        games: 7,
                    },
                    Candidate {
                        mv: Position::from_kifu("d3").unwrap(),
                        value: -2.4,
                        games: 3,
                    },
                ],
                depth: 26,
                games: 10,
            },
        );
        let path = std::env::temp_dir().join("kuroobi_book_test.txt");
        book.save(&path).unwrap();
        let back = Book::load(&path).unwrap();
        assert_eq!(back.len(), 1);
        let e = back.get_raw(key).unwrap();
        assert_eq!(e.depth, 26);
        assert_eq!(e.moves.len(), 2);
        assert!((e.moves[0].value + 2.0).abs() < 1e-6);
        assert_eq!(e.moves[1].games, 3);
        std::fs::remove_file(&path).ok();
    }
}

#[cfg(test)]
mod symmetry_tests {
    use super::*;

    /// The opening is invariant under 4 symmetries (not 8: a 90-degree
    /// turn swaps the colors). Those 4 map the 4 legal moves onto each
    /// other, so the book stores one and expands on lookup.
    #[test]
    fn the_opening_moves_are_all_equivalent() {
        let b = Board::new();
        let stab = stabilizers(&b);
        assert_eq!(stab.len(), 4, "expected 4 stabilizers: {stab:?}");

        let mut moves: Vec<u8> = b.movable_iter().map(|p| p.index()).collect();
        moves.sort_unstable();
        assert_eq!(moves.len(), 4);

        // One move mapped through the stabilizers reaches all four.
        let mut reached: Vec<u8> = stab.iter().map(|&i| map_square(moves[0], i)).collect();
        reached.sort_unstable();
        reached.dedup();
        assert_eq!(reached, moves, "one move must reach all four");
    }

    /// On a symmetric board one stored move yields every equivalent move.
    #[test]
    fn candidates_expand_over_the_symmetry() {
        let b = Board::new();
        let (key, i) = Book::key(&b);
        let mv = b.movable_iter().next().unwrap();
        let mut book = Book::new();
        book.insert_raw(
            key,
            Entry {
                moves: vec![Candidate {
                    mv: Position::from_index(map_square(mv.index(), i) as u32).unwrap(),
                    value: -1.5,
                    games: 10,
                }],
                depth: 26,
                games: 10,
            },
        );
        let got = book.candidates(&b).expect("position is in the book");
        assert_eq!(got.len(), 4, "all four moves returned: {got:?}");
        assert!(
            got.iter().all(|(_, v)| *v == -1.5),
            "values must match: {got:?}"
        );
    }

    /// Asymmetric positions must not expand (no fabricated moves).
    #[test]
    fn asymmetric_positions_are_untouched() {
        let mut b = Board::new();
        b.make_move(Position::from_index(26).unwrap()).unwrap(); // d3
        assert_eq!(stabilizers(&b).len(), 1, "identity is the only stabilizer");
    }
}
