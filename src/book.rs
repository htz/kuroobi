//! Opening book.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::bitboard::{symmetries, symmetry};
use crate::{Board, Position};

#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    pub mv: Position,
    pub value: f32,
    pub games: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub moves: Vec<Candidate>,
    pub depth: u8,
    pub games: u32,
    pub complete: bool,
}

impl Entry {
    pub fn best(&self) -> Option<&Candidate> {
        self.moves.first()
    }

    pub fn remove_move(&mut self, mv: Position) {
        self.moves.retain(|c| c.mv != mv);
    }

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

fn stabilizers(board: &Board) -> Vec<u8> {
    let (p, o) = (board.player_bb(), board.opponent_bb());
    (0..8u8)
        .filter(|&i| (symmetry(p, i), symmetry(o, i)) == (p, o))
        .collect()
}

fn map_square(sq: u8, i: u8) -> u8 {
    symmetry(1u64 << sq, i).trailing_zeros() as u8
}

fn unmap_square(sq: u8, i: u8) -> u8 {
    (0..64u8).find(|&c| map_square(c, i) == sq).unwrap_or(sq)
}

pub fn board_from_key(key: (u64, u64)) -> Board {
    let mut b = Board::new();
    b.black = key.0;
    b.white = key.1;
    b.player = crate::Color::Black;
    b.empty_count = (!(key.0 | key.1)).count_ones() as u8;
    b
}

#[derive(Clone, Default)]
pub struct Book {
    map: HashMap<(u64, u64), Entry>,
}

pub type BookCandidate = (Position, f32, u32);

impl Book {
    pub fn new() -> Book {
        Book::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn get_raw(&self, key: (u64, u64)) -> Option<&Entry> {
        self.map.get(&key)
    }

    pub fn get_raw_mut(&mut self, key: (u64, u64)) -> Option<&mut Entry> {
        self.map.get_mut(&key)
    }

    pub fn remove_raw(&mut self, key: (u64, u64)) -> Option<Entry> {
        self.map.remove(&key)
    }

    pub fn insert_raw(&mut self, key: (u64, u64), e: Entry) {
        self.map.insert(key, e);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&(u64, u64), &Entry)> {
        self.map.iter()
    }

    pub fn probe(&self, board: &Board) -> Option<(Position, f32, u8)> {
        let (cands, depth, complete) = self.expand(board)?;
        if !complete {
            return None;
        }
        cands.first().map(|(p, v, _)| (*p, *v, depth))
    }

    fn expand(&self, board: &Board) -> Option<(Vec<BookCandidate>, u8, bool)> {
        let (key, i) = Book::key(board);
        let e = self.map.get(&key)?;
        let stab = stabilizers(board);
        let mut out: Vec<BookCandidate> = Vec::new();
        for c in &e.moves {
            let p0 = Position(unmap_square(c.mv.index(), i));
            for &g in &stab {
                let p = Self::map_move(p0, g);
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
        Some((out, e.depth, e.complete))
    }

    pub fn probe_varied(
        &self,
        board: &Board,
        tolerance: f32,
        rand: u64,
    ) -> Option<(Position, f32, u8)> {
        let (all, depth, complete) = self.expand(board)?;
        if !complete {
            return None;
        }
        let best = all.first()?.1;
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

    pub fn candidates(&self, board: &Board) -> Option<Vec<(Position, f32)>> {
        let (out, _, _) = self.expand(board)?;
        Some(out.into_iter().map(|(p, v, _)| (p, v)).collect())
    }

    pub fn candidates_detailed(&self, board: &Board) -> Option<Vec<BookCandidate>> {
        let (out, _, _) = self.expand(board)?;
        Some(out)
    }

    pub fn has(&self, board: &Board) -> bool {
        self.map.contains_key(&Book::key(board).0)
    }

    /// The smallest of the position's eight forms, and which form that is.
    pub fn key(board: &Board) -> ((u64, u64), u8) {
        let ps = symmetries(board.player_bb());
        let os = symmetries(board.opponent_bb());
        (0..8u8)
            .map(|i| ((ps[i as usize], os[i as usize]), i))
            .min_by_key(|&(k, _)| k)
            .expect("eight forms")
    }

    pub fn map_move(pos: Position, i: u8) -> Position {
        Position(map_square(pos.index(), i))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            writeln!(f, "KUROOBI_BOOK_3")?;
            for ((p, o), e) in &self.map {
                write!(
                    f,
                    "{p:016x} {o:016x} {} {} {}",
                    e.depth,
                    e.games,
                    u8::from(e.complete)
                )?;
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
        for (i, line) in f.lines().enumerate() {
            let line = line?;
            if i == 0 {
                if line.trim() != "KUROOBI_BOOK_3" {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "{}: expected KUROOBI_BOOK_3, found {:?} \
                             (rebuild it with `bookgen --scan` and \
                             `--deepen --all-moves`)",
                            path.display(),
                            line.trim()
                        ),
                    ));
                }
                continue;
            }
            let t: Vec<&str> = line.split_whitespace().collect();
            if t.len() < 5 {
                continue;
            }
            let (Ok(p), Ok(o)) = (u64::from_str_radix(t[0], 16), u64::from_str_radix(t[1], 16))
            else {
                continue;
            };
            let (Ok(depth), Ok(games), Ok(complete)) =
                (t[2].parse::<u8>(), t[3].parse::<u32>(), t[4].parse::<u8>())
            else {
                continue;
            };
            let mut moves = Vec::new();
            for tok in &t[5..] {
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
                    complete: complete != 0,
                },
            );
        }
        Ok(book)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mv: Position, value: f32, games: u32) -> Entry {
        Entry {
            moves: vec![Candidate { mv, value, games }],
            depth: 24,
            games,
            complete: true,
        }
    }

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

    #[test]
    fn varied_choice_stays_within_tolerance() {
        let b = Board::new();
        let mut after = b;
        after.make_move_bits(Position::from_kifu("f5").unwrap());
        let (key2, i2) = Book::key(&after);

        let legal: Vec<Position> = after.movable_iter().collect();
        assert!(legal.len() >= 3);
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
                complete: true,
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
                complete: true,
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

    #[test]
    fn the_opening_moves_are_all_equivalent() {
        let b = Board::new();
        let stab = stabilizers(&b);
        assert_eq!(stab.len(), 4, "expected 4 stabilizers: {stab:?}");

        let mut moves: Vec<u8> = b.movable_iter().map(|p| p.index()).collect();
        moves.sort_unstable();
        assert_eq!(moves.len(), 4);

        let mut reached: Vec<u8> = stab.iter().map(|&i| map_square(moves[0], i)).collect();
        reached.sort_unstable();
        reached.dedup();
        assert_eq!(reached, moves, "one move must reach all four");
    }

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
                complete: true,
            },
        );
        let got = book.candidates(&b).expect("position is in the book");
        assert_eq!(got.len(), 4, "all four moves returned: {got:?}");
        assert!(
            got.iter().all(|(_, v)| *v == -1.5),
            "values must match: {got:?}"
        );
    }

    #[test]
    fn asymmetric_positions_are_untouched() {
        let mut b = Board::new();
        b.make_move(Position::from_index(26).unwrap()).unwrap(); // d3
        assert_eq!(stabilizers(&b).len(), 1, "identity is the only stabilizer");
    }
}
