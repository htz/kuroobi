//! 実戦の対局を定石へ取り込む学習。
//!
//! 自分の対局を**勝敗にかかわらず**取り込む。対局で通った各局面に
//! 「実戦で指した手」と「それ以外の合法手の中の最善 (代替)」を評価して
//! 持たせ、終局の石差 (終局していない棋譜は終端の探索値) を negamax で
//! 根まで書き戻す。
//!
//! - 負けにつながった手は値が下がり、対局時の選択 (`probe_varied`) が
//!   代替へ自然に分岐する。同じ負け方を繰り返さない
//! - 各局面の値は「候補の最善」なので、負けの値は「代替でもまだ悪かった」
//!   区間だけを遡り、良い代替が残っている地点 — 敗着 — で止まる。
//!   序盤の正常な手に負けの値は付かない
//! - 勝った対局も取り込む。代替の方が良かったのに相手のミスで勝てた
//!   ラインを、良いと思い込み続けないため
//!
//! 回避のための特別なロジックは持たない。「同じ負けを繰り返さない」は
//! negamax の値がそうさせる性質であって、手を禁止する仕組みではない。
//!
//! 学習は 1 探索ずつ進む ([`BackupJob`])。対局の合間に少しずつ回すので、
//! 思考やサーバーへの応答を分単位で待たせない。学習分は定石本体とは
//! 別のファイルに保存し、起動時に重ねて選択に使う (bookgen が本体を
//! 更新中でも衝突しない)。

use crate::book::{Book, Candidate, Entry};
use crate::solver::final_score;
use crate::{Board, Position};

/// 書き換えた手 1 つ。
///
/// **旧→新を残すのは後から確かめるため。** 書き戻しは値を上書きするので、
/// 変な対局が 1 局混ざると以後の手が変わる。何がどう変わったかが残って
/// いないと、見つけることも戻すこともできない。
#[derive(Debug, Clone, Copy)]
pub struct BackupChange {
    /// 棋譜の何手目か (パスを除いた 1 始まり)。
    pub ply: usize,
    /// 実戦で指した手 (盤の向きのまま。正規化空間の手ではない)。
    pub mv: Position,
    /// 上書き前の値。定石に無かった手なら None。
    pub before: Option<f32>,
    /// 上書き後の値。
    pub after: f32,
    /// 書き換えたあとの、その局面の最善手の値。
    /// `best - after` がその手で損した石差になる。
    pub best: f32,
}

/// 取り込みの結果。
#[derive(Debug, Default, Clone)]
pub struct BackupOutcome {
    /// 値を付け替えた手の数。
    pub updated: usize,
    /// 新たに学習分へ足した局面の数。
    pub added: usize,
    /// 書き換えの明細 (終局側から前へ向かう順)。
    pub changes: Vec<BackupChange>,
}

/// 再生した手順。要素は (指す前の盤面, 指した手)。パスは `None`。
pub type Line = Vec<(Board, Option<Position>)>;

/// 棋譜 (f5d6… 形式) を、パスを補いながら (盤面, 手) の列にする。
/// 戻り値の最後は終局 (または棋譜が尽きた) 時点の盤面。
pub fn replay(start: Option<&str>, kifu: &str) -> Result<(Line, Board), String> {
    let mut b = match start {
        Some(s) => Board::from_string(s).map_err(|e| format!("開始局面: {e:?}"))?,
        None => Board::new(),
    };
    let mut seq = Vec::new();
    let chars: Vec<char> = kifu.chars().collect();
    let mut i = 0;
    while i + 2 <= chars.len() {
        let mv = Position::from_kifu(&chars[i..i + 2].iter().collect::<String>())
            .map_err(|e| format!("棋譜の手 {}{}: {e}", chars[i], chars[i + 1]))?;
        // 打てない手番はパスを 1 手として挟む
        if !b.check(mv) && b.movable() == 0 {
            seq.push((b, None));
            b.pass();
        }
        if !b.check(mv) {
            return Err(format!("打てない手 {mv:?}"));
        }
        seq.push((b, Some(mv)));
        b.make_move(mv).map_err(|e| format!("{e:?}"))?;
        i += 2;
    }
    Ok((seq, b))
}

/// [`BackupJob::next`] が次にしてほしいこと。
pub enum JobStep {
    /// この盤面を評価してほしい。手番視点の値を [`BackupJob::feed`] で返す。
    Search(Board),
    /// 取り込みが終わった。
    Done(BackupOutcome),
}

/// 1 局ぶんの取り込みを 1 探索ずつ進める状態機械。
///
/// 探索そのものは持たない。`next` が `Search(盤面)` を返したら呼び出し側が
/// 評価して `feed` で値を渡す。この分割で、学習を対局の合間に少しずつ
/// 進められるし、探索なしでロジックをテストできる。
pub struct BackupJob {
    line: Line,
    /// 次に処理する局面 = `line[idx - 1]`。後ろ (終局側) から前へ。0 で完了。
    idx: usize,
    /// 子側 (1 手先) の局面の手番視点の値。
    v_next: f32,
    /// 終端局面 (終局していない棋譜では探索値を待つ)。
    terminal: Board,
    awaiting_terminal: bool,
    /// 評価待ちの代替手 (現在の局面の、実戦手以外の合法手)。
    alt_queue: Vec<Position>,
    /// 直前に `Search` へ出した代替手。`feed` が受けて `alt_best` を更新する。
    pending_alt: Option<Position>,
    alt_best: Option<(Position, f32)>,
    /// 代替の評価中か (空の `alt_queue` と「未着手」を区別する)。
    evaluating: bool,
    /// 新規エントリに記録する探索深さ。
    new_depth: u8,
    out: BackupOutcome,
    done: bool,
}

impl BackupJob {
    /// 対局 1 局ぶんの取り込みを用意する。終局している棋譜なら終端値は
    /// 石差 (厳密)。終局していなければ最初の `next` が終端の評価を求める。
    pub fn new(start: Option<&str>, kifu: &str, new_depth: u8) -> Result<BackupJob, String> {
        let (line, terminal) = replay(start, kifu)?;
        if line.is_empty() {
            return Err("棋譜が空です".into());
        }
        let over = terminal.is_game_over();
        let v_next = if over {
            final_score(&terminal) as f32
        } else {
            f32::NAN // feed で入る
        };
        Ok(BackupJob {
            idx: line.len(),
            line,
            v_next,
            terminal,
            awaiting_terminal: !over,
            alt_queue: Vec::new(),
            pending_alt: None,
            alt_best: None,
            evaluating: false,
            new_depth,
            out: BackupOutcome::default(),
            done: false,
        })
    }

    /// 残りのおおよその仕事量 (未処理の局面数)。表示用。
    pub fn remaining(&self) -> usize {
        self.idx
    }

    /// 状態を進め、次にすべきことを返す。`Search` を返したら評価値を
    /// `feed` してから再度呼ぶこと。
    pub fn next(&mut self, learned: &mut Book, base: &mut Book) -> JobStep {
        loop {
            if self.done {
                return JobStep::Done(std::mem::take(&mut self.out));
            }
            if self.awaiting_terminal {
                return JobStep::Search(self.terminal);
            }
            if self.idx == 0 {
                self.done = true;
                continue;
            }
            let (board, mv) = self.line[self.idx - 1];
            let Some(mv) = mv else {
                // パス: 盤は同じまま手番だけ替わる
                self.v_next = -self.v_next;
                self.idx -= 1;
                continue;
            };
            let (key, i) = Book::key(&board);
            if learned.get_raw(key).is_none() {
                if let Some(b) = base.get_raw(key) {
                    // 定石本体の候補一式 (深い値) を引き継いで学習を始める
                    learned.insert_raw(key, b.clone());
                } else {
                    // どこにも無い局面: 実戦手**以外**の合法手の最善を測って
                    // 一緒に入れる。ここが負の伝播を止める壁になる。実戦手
                    // (探索最善と一致しがち) だけでは候補が 1 本になり、
                    // 負けの値が敗着を越えて序盤まで素通りしてしまう。
                    if !self.evaluating {
                        self.alt_queue = board.movable_iter().filter(|p| *p != mv).collect();
                        self.alt_best = None;
                        self.evaluating = true;
                    }
                    if let Some(p) = self.alt_queue.pop() {
                        let mut child = board;
                        child.make_move_bits(p);
                        self.pending_alt = Some(p);
                        return JobStep::Search(child);
                    }
                    // 全代替を評価し終えた → エントリを作る
                    self.evaluating = false;
                    let e = Entry {
                        moves: self
                            .alt_best
                            .take()
                            .map(|(amv, av)| Candidate {
                                mv: Book::map_move(amv, i),
                                value: av,
                                games: 0,
                            })
                            .into_iter()
                            .collect(),
                        depth: self.new_depth,
                        games: 0,
                    };
                    learned.insert_raw(key, e);
                    self.out.added += 1;
                }
            }
            let e = learned.get_raw_mut(key).expect("直前に挿入済み");
            let mapped = Book::map_move(mv, i);
            let before = e.moves.iter().find(|c| c.mv == mapped).map(|c| c.value);
            let after = -self.v_next;
            e.update_move(mapped, after);
            self.out.updated += 1;
            self.out.changes.push(BackupChange {
                // パスは手数に数えない (棋譜の表と番号を合わせる)
                ply: self.line[..self.idx - 1]
                    .iter()
                    .filter(|(_, m)| m.is_some())
                    .count()
                    + 1,
                mv,
                before,
                after,
                best: e.best().map(|c| c.value).unwrap_or(after),
            });
            // この局面の値は「更新後の最善」。実戦手より良い代替が残って
            // いればそちらが親へ伝わる (敗着より根側へ負を遡らせない)
            self.v_next = e.best().map(|c| c.value).unwrap_or(-self.v_next);
            // 対局の選択に使うマージ済みの側にも常に反映する
            base.insert_raw(key, e.clone());
            self.idx -= 1;
        }
    }

    /// 直前の `Search` の結果 (渡した盤面の手番視点の値) を返す。
    pub fn feed(&mut self, value: f32) {
        if self.awaiting_terminal {
            self.v_next = value;
            self.awaiting_terminal = false;
            return;
        }
        if let Some(p) = self.pending_alt.take() {
            let v = -value; // 子の手番視点 → この局面の手番視点
            if self.alt_best.is_none_or(|(_, bv)| v > bv) {
                self.alt_best = Some((p, v));
            }
        }
    }
}

/// 学習分を定石本体へ重ねる (学習側を優先して丸ごと上書き)。
/// 学習エントリは定石本体の候補一式を引き継いだ上で実戦の帰結を
/// 上書きしたものなので、これで候補を失わずに値だけ新しくなる。
pub fn merge_learned(base: &mut Book, learned: &Book) {
    for (key, e) in learned.iter() {
        base.insert_raw(*key, e.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(s: &str) -> Position {
        Position::from_kifu(s).unwrap()
    }

    /// ジョブを最後まで回す。Search 要求には `eval` が答える。
    fn run_job(
        job: &mut BackupJob,
        learned: &mut Book,
        base: &mut Book,
        mut eval: impl FnMut(&Board) -> f32,
    ) -> BackupOutcome {
        loop {
            match job.next(learned, base) {
                JobStep::Search(b) => {
                    let v = eval(&b);
                    job.feed(v);
                }
                JobStep::Done(out) => return out,
            }
        }
    }

    /// f5d6 で負けた (終端で黒視点 -10) とき、初期局面の f5 の値が
    /// 下がって次善だった d3 が最善に入れ替わる。
    #[test]
    fn losing_move_gets_demoted() {
        let b0 = Board::new();
        let (key0, i0) = Book::key(&b0);
        let mut base = Book::new();
        base.insert_raw(
            key0,
            Entry {
                moves: vec![
                    Candidate {
                        mv: Book::map_move(pos("f5"), i0),
                        value: 1.0,
                        games: 10,
                    },
                    Candidate {
                        mv: Book::map_move(pos("d3"), i0),
                        value: 0.8,
                        games: 5,
                    },
                ],
                depth: 26,
                games: 15,
            },
        );
        let mut learned = Book::new();

        // 棋譜は途中まで (終局していない) → 終端は探索値 -10 (黒視点)。
        // 最初の Search は必ず終端で、以降は代替の子局面。
        let mut job = BackupJob::new(None, "f5d6", 14).expect("再生できる");
        let mut first = true;
        let out = run_job(&mut job, &mut learned, &mut base, |_b| {
            if std::mem::take(&mut first) {
                -10.0 // 終端 (f5d6 後、黒番)
            } else {
                -0.1 // 代替の子 (相手視点) → 代替はどれも +0.1
            }
        });
        assert_eq!(out.added, 1, "f5 後の局面が学習分に足される");
        assert_eq!(out.updated, 2, "d6 と f5 の値が付け替わる");

        // d6 の値 = +10 (相手にとっての -10)。f5 = -(d6 局面の最善)。
        // 初期局面では f5 が下がり d3 が最善になる
        let e0 = base.get_raw(key0).unwrap();
        assert_eq!(
            e0.best().unwrap().mv.index(),
            Book::map_move(pos("d3"), i0).index(),
            "f5 が下がって d3 が最善になる"
        );
        // 学習側にも同じエントリがあり、選択側 (base) にも常に反映される
        assert_eq!(learned.len(), 2);
        assert_eq!(base.len(), 2, "学習した局面は選択側にも入る");
    }

    /// 敗着で伝播が止まる: 途中の局面に「まだ良かった代替」があれば、
    /// それより根側 (序盤側) の手に負けの値が付かない。2 手目の正常な
    /// 応手が「初手から負け」に巻き込まれないための要の性質。
    #[test]
    fn loss_stops_at_the_losing_move() {
        let mut base = Book::new();
        let mut learned = Book::new();
        // f5(黒) d6(白) c3(黒) で黒が負けた (c3 後の白番視点 +20)。
        // 敗着は c3 とし、その局面 (6 石) の代替だけ +1.5 (黒視点) ある。
        let mut job = BackupJob::new(None, "f5d6c3", 14).unwrap();
        let mut first = true;
        let out = run_job(&mut job, &mut learned, &mut base, |b| {
            if std::mem::take(&mut first) {
                return 20.0; // 終端 (c3 の後、白番視点で +20 = 黒の負け)
            }
            match 64 - b.empty_count() {
                7 => -1.5, // c3 の代替の子 (白視点) → 代替 = +1.5
                _ => -0.1, // それ以外の代替の子 → 代替 = +0.1
            }
        });
        assert_eq!(out.updated, 3);

        // 敗着 c3 には -20 が付き、その局面の値は代替の +1.5 に留まる。
        // 根側: d6 = -1.5、f5 = -(d6 局面の最善 +0.1) = -0.1。
        // 序盤の手はほぼ中立で、「初手から負け」にはならない。
        let b0 = Board::new();
        let (key0, i0) = Book::key(&b0);
        let f5v = learned
            .get_raw(key0)
            .unwrap()
            .moves
            .iter()
            .find(|c| c.mv.index() == Book::map_move(pos("f5"), i0).index())
            .unwrap()
            .value;
        assert!(
            f5v > -1.0,
            "敗着 (c3) より根側の f5 に負けの値が付かない (実際 {f5v})"
        );

        let mut b2 = Board::new();
        b2.make_move(pos("f5")).unwrap();
        b2.make_move(pos("d6")).unwrap();
        let (key2, i2) = Book::key(&b2);
        let c3v = learned
            .get_raw(key2)
            .unwrap()
            .moves
            .iter()
            .find(|c| c.mv.index() == Book::map_move(pos("c3"), i2).index())
            .unwrap()
            .value;
        assert!((c3v + 20.0).abs() < 1e-6, "敗着 c3 = -20 (実際 {c3v})");
    }

    /// 代替も悪い区間 (負けが確定した後) は、負の値がそのまま遡上する。
    #[test]
    fn loss_propagates_when_alternatives_are_also_bad() {
        let mut base = Book::new();
        let mut learned = Book::new();
        let mut job = BackupJob::new(None, "f5d6c3", 14).unwrap();
        let mut first = true;
        run_job(&mut job, &mut learned, &mut base, |_b| {
            if std::mem::take(&mut first) {
                20.0 // 終端
            } else {
                15.0 // どの代替の子も相手が +15 = 代替は -15 で大差の負け
            }
        });
        let b0 = Board::new();
        let (key0, i0) = Book::key(&b0);
        let f5v = learned
            .get_raw(key0)
            .unwrap()
            .moves
            .iter()
            .find(|c| c.mv.index() == Book::map_move(pos("f5"), i0).index())
            .unwrap()
            .value;
        assert!(f5v < -10.0, "どこにも良い代替が無ければ負けが遡る ({f5v})");
    }

    /// 勝った対局も取り込まれ、勝ちの値がそのまま入る。
    #[test]
    fn winning_games_are_absorbed_too() {
        let mut base = Book::new();
        let mut learned = Book::new();
        let mut job = BackupJob::new(None, "f5d6", 14).unwrap();
        let mut first = true;
        let out = run_job(&mut job, &mut learned, &mut base, |_b| {
            if std::mem::take(&mut first) {
                8.0 // 終端: 黒が勝っている
            } else {
                -0.1 // 代替の子 → 代替は +0.1
            }
        });
        assert_eq!(out.updated, 2);
        let b0 = Board::new();
        let (key0, i0) = Book::key(&b0);
        let f5v = learned
            .get_raw(key0)
            .unwrap()
            .moves
            .iter()
            .find(|c| c.mv.index() == Book::map_move(pos("f5"), i0).index())
            .unwrap()
            .value;
        // 終端 +8 (黒視点) → d6 = -8 → d6 局面の最善は代替 (+0.1)
        // → f5 = -0.1。相手の悪手 (d6) のおかげの勝ちは f5 の値を
        // 押し上げない (代替の方が良ければそちらが伝わる)
        assert!(
            (f5v + 0.1).abs() < 1e-6,
            "勝ちでも代替経由の値になる ({f5v})"
        );
    }

    /// 定石本体に元からある局面は候補一式を引き継ぎ、代替の評価をしない。
    #[test]
    fn known_positions_reuse_book_candidates() {
        let b0 = Board::new();
        let (key0, _) = Book::key(&b0);
        let mut base = Book::new();
        base.insert_raw(
            key0,
            Entry {
                moves: vec![Candidate {
                    mv: Book::map_move(pos("f5"), Book::key(&b0).1),
                    value: 1.0,
                    games: 1,
                }],
                depth: 26,
                games: 1,
            },
        );
        let mut learned = Book::new();
        let mut job = BackupJob::new(None, "f5", 14).unwrap();
        let mut searches = 0;
        run_job(&mut job, &mut learned, &mut base, |_b| {
            searches += 1;
            0.0
        });
        // 終局していない棋譜なので終端評価の 1 回だけ。初期局面は
        // 本体にあるので代替評価は走らない
        assert_eq!(searches, 1, "既知の局面では代替を測り直さない");
    }

    /// パスを含む実対局の棋譜が終局まで再生できる。
    #[test]
    fn replay_inserts_passes() {
        let kifu = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";
        let (line, fin) = replay(None, kifu).expect("パス込みで再生できる");
        assert!(fin.is_game_over(), "終局まで再生できる");
        assert!(line.iter().any(|(_, m)| m.is_none()), "パスが補われている");
    }

    /// merge_learned は学習側を優先して丸ごと上書きする。
    #[test]
    fn merge_prefers_learned_entries() {
        let b0 = Board::new();
        let (key0, i0) = Book::key(&b0);
        let mk = |v: f32| Entry {
            moves: vec![Candidate {
                mv: Book::map_move(pos("f5"), i0),
                value: v,
                games: 0,
            }],
            depth: 20,
            games: 0,
        };
        let mut base = Book::new();
        base.insert_raw(key0, mk(1.0));
        let mut learned = Book::new();
        learned.insert_raw(key0, mk(-5.0));
        merge_learned(&mut base, &learned);
        let v = base.get_raw(key0).unwrap().best().unwrap().value;
        assert!((v + 5.0).abs() < 1e-6);
    }
}
