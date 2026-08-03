//! 定石 book。
//!
//! 局面 → (最善手, 評価値) の表。**8 対称で正規化したキー**で引くので、
//! 回転・鏡像で同じ局面は 1 エントリに畳まれる (人間の棋譜は f5 系に偏る
//! ため、正規化しないと同型局面の大半を取りこぼす)。
//!
//! 値は**実戦より深い探索**で付ける前提。実戦深度で作った book には意味が
//! ないので、生成側 (`bookgen`) は深さと読切開始を実戦設定より上げて回す。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::{Board, Position};

/// book の候補手 1 つ。
#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    /// 正規化した盤面での手。
    pub mv: Position,
    /// 手番視点の評価値 (石差)。深い探索で付ける。
    pub value: f32,
    /// この手が棋譜で選ばれた回数 (人間の採用頻度)。
    pub games: u32,
}

/// book の 1 エントリ。**候補手を全部持つ**ので、同一棋譜の反復を避けるため
/// に「最善から許容幅以内の手」から選べる。
#[derive(Clone, Debug, Default)]
pub struct Entry {
    /// 評価値の高い順。空でない限り [0] が最善。
    pub moves: Vec<Candidate>,
    /// 評価に使った探索深さ (0 = 棋譜頻度のみ)。
    pub depth: u8,
    /// この局面が棋譜に現れた回数。
    pub games: u32,
}

impl Entry {
    pub fn best(&self) -> Option<&Candidate> {
        self.moves.first()
    }

    /// 候補 `mv` (正規化空間の手) の値を付け替える。無ければ追加する。
    /// 値の降順を保つので、付け替えで最善が入れ替わることがある
    /// (実戦の帰結を書き戻す学習がこれを使う)。
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

/// 盤面の 8 対称のうち辞書順で最小のものを正規化形とし、その変換 index を
/// 返す。手を戻すときに逆変換で使う。
fn normalize(board: &Board) -> (u64, u64, u8) {
    let mut best = (board.player_bb(), board.opponent_bb());
    let mut best_i = 0u8;
    let mut p = board.player_bb();
    let mut o = board.opponent_bb();
    // 8 対称: 転置・水平鏡像の組み合わせで全て作る
    for i in 0..8u8 {
        if i > 0 {
            // 4 回で 90 度回転一周、4 回目に鏡像へ切り替える
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

/// 盤面の向きに戻した候補手 (マス, 手番視点の値, 実戦での採用回数)。
pub type BookMove = (Position, f32, u32);

/// 変換 `i` をビットボード全体に適用する (`map_square` の 64 マス版)。
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

/// 盤面を変えない変換の一覧 (自己同型)。初期局面のように対称な盤面では
/// 複数あり、そこでは 1 つの手が同値な手すべてを代表する。
pub fn stabilizers(board: &Board) -> Vec<u8> {
    let (p, o) = (board.player_bb(), board.opponent_bb());
    (0..8u8)
        .filter(|&i| (transform_bb(p, i), transform_bb(o, i)) == (p, o))
        .collect()
}

/// 正規化変換 `i` を 1 マスに適用する。
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

/// 正規化変換 `i` の逆変換を 1 マスに適用する。
fn unmap_square(sq: u8, i: u8) -> u8 {
    // 逆変換は「同じ変換を適用して元に戻る回数」を総当たりで探すのが確実
    for cand in 0..64u8 {
        if map_square(cand, i) == sq {
            return cand;
        }
    }
    sq
}

/// 正規化キー (player, opponent) から盤面を復元する。book のキーは常に
/// 「手番側 = player」なので、黒番として組み立てれば手番視点が一致する。
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

    /// 正規化キーで直接引く (生成ツール用)。
    pub fn get_raw(&self, key: (u64, u64)) -> Option<&Entry> {
        self.map.get(&key)
    }

    /// 正規化キーで直接引く (学習の書き戻し用)。
    pub fn get_raw_mut(&mut self, key: (u64, u64)) -> Option<&mut Entry> {
        self.map.get_mut(&key)
    }

    pub fn insert_raw(&mut self, key: (u64, u64), e: Entry) {
        self.map.insert(key, e);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&(u64, u64), &Entry)> {
        self.map.iter()
    }

    /// 局面を引いて最善手を返す (研究・検証用の決定的な選択)。
    pub fn probe(&self, board: &Board) -> Option<(Position, f32, u8)> {
        let (cands, depth) = self.expand(board)?;
        // 値の高い順に並んでいる
        cands.first().map(|(p, v, _)| (*p, *v, depth))
    }

    /// 局面を引き、盤面の向きに戻した候補 (手, 値, 採用回数) と深さを返す。
    ///
    /// 対称な盤面 (初期局面など) では、記録された 1 手が同値な手すべてを
    /// 代表している。自己同型で写した先も同じ候補として広げる。これを
    /// しないと、互いに移り合うだけの手が抜け落ちる — 表示では手が欠け、
    /// 乱択では同じ向きの手ばかり選ばれる。
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

    /// 局面を引き、**最善から `tolerance` 石以内**の候補から 1 手選ぶ。
    ///
    /// 同じ相手と同じ条件で戦ったときに毎回同じ棋譜になるのを避けるための
    /// 選択。許容幅内は「実質互角」なので棋力はほぼ落ちない。候補の重みは
    /// 人間の採用頻度 (+1) にしてあり、定石らしい手ほど出やすい。
    pub fn probe_varied(
        &self,
        board: &Board,
        tolerance: f32,
        rand: u64,
    ) -> Option<(Position, f32, u8)> {
        let (all, depth) = self.expand(board)?;
        let best = all.first()?.1;
        // 許容幅内の候補だけ集める
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

    /// 局面の候補一覧を盤面の向きに戻して返す (合法手のみ、値の降順)。
    /// 検討画面などの表示用。
    pub fn candidates(&self, board: &Board) -> Option<Vec<(Position, f32)>> {
        let (out, _) = self.expand(board)?;
        Some(out.into_iter().map(|(p, v, _)| (p, v)).collect())
    }

    /// 正規化空間の手を元の盤面の向きへ戻す。
    fn back(mv: Position, i: u8) -> Option<Position> {
        Position::from_index(unmap_square(mv.index(), i) as u32)
    }

    /// 盤面を正規化キーにする (生成側と共用)。
    pub fn key(board: &Board) -> ((u64, u64), u8) {
        let (p, o, i) = normalize(board);
        ((p, o), i)
    }

    /// 手を正規化空間へ写す。
    pub fn map_move(pos: Position, i: u8) -> Position {
        Position(map_square(pos.index(), i))
    }

    /// テキスト形式で保存する。1 行 =
    /// `player_hex opponent_hex depth games | mv:value:games mv:value:games ...`
    /// バイナリにしない理由: 差分を目視でき、途中経過をシェルで扱えるため。
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
                    "KUROOBI_BOOK_1" => v1 = true, // 旧形式 (最善手のみ)
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
                // 旧: best value depth games
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

    /// 対称な 4 手はすべて同じキーに畳まれる。
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
        assert_eq!(
            keys.len(),
            1,
            "初期局面の 4 手は対称なので 1 キーに畳まれる"
        );
    }

    /// 正規化して入れた手が、任意の向きの同型局面で正しい手として返る。
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
            assert!(got.is_some(), "対称局面 {p:?} で book が引けない");
            let (mv, v, depth) = got.unwrap();
            assert!(c.check(mv), "返った手が合法である");
            assert_eq!(v, 1.5);
            assert_eq!(depth, 24);
        }
    }

    /// 許容幅内の候補が散らばり、幅外の悪手は選ばれない。
    #[test]
    fn varied_choice_stays_within_tolerance() {
        let b = Board::new();
        let (key, i) = Book::key(&b);
        // 初期局面の 4 手は対称で 1 手に畳まれるので、2 手目の局面で試す
        let mut after = b;
        after.make_move_bits(Position::from_kifu("f5").unwrap());
        let (key2, i2) = Book::key(&after);
        let _ = (key, i);

        let legal: Vec<Position> = after.movable_iter().collect();
        assert!(legal.len() >= 3);
        // 先頭 2 手を互角 (0.0 / -0.5)、3 手目を明確な悪手 (-5.0) にする
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
            let (mv, v, _) = book.probe_varied(&after, 1.0, r).expect("引ける");
            assert!(after.check(mv));
            assert!(v >= -1.0, "許容幅外の悪手が選ばれた: {v}");
            seen.insert(mv.index());
        }
        assert!(seen.len() >= 2, "候補が散らばる (実際 {})", seen.len());
        assert!(
            seen.len() <= 2,
            "許容幅外は除外される (実際 {})",
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

    /// 初期局面は 4 通りの対称で不変 (90 度回すと黒白が入れ替わるので
    /// 8 通りではない)。その 4 通りが 4 つの合法手を互いに移し合うので、
    /// 定石は 1 手だけ持てば足り、引くときに残りへ広げられる。
    #[test]
    fn the_opening_moves_are_all_equivalent() {
        let b = Board::new();
        let stab = stabilizers(&b);
        assert_eq!(stab.len(), 4, "自己同型は 4 通り: {stab:?}");

        let mut moves: Vec<u8> = b.movable_iter().map(|p| p.index()).collect();
        moves.sort_unstable();
        assert_eq!(moves.len(), 4);

        // 1 手を自己同型で写すと 4 手すべてに届く
        let mut reached: Vec<u8> = stab.iter().map(|&i| map_square(moves[0], i)).collect();
        reached.sort_unstable();
        reached.dedup();
        assert_eq!(reached, moves, "1 手から 4 手すべてへ");
    }

    /// 対称な盤面では、記録されている 1 手から同値の手すべてを返す。
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
        let got = book.candidates(&b).expect("定石にある");
        assert_eq!(got.len(), 4, "4 手すべて返る: {got:?}");
        assert!(got.iter().all(|(_, v)| *v == -1.5), "値は同じ: {got:?}");
    }

    /// 対称でない局面では広がらない (余計な手を作らない)。
    #[test]
    fn asymmetric_positions_are_untouched() {
        let mut b = Board::new();
        b.make_move(Position::from_index(26).unwrap()).unwrap(); // d3
        assert_eq!(stabilizers(&b).len(), 1, "自己同型は恒等だけ");
    }
}
