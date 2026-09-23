//! Learning: importing played games into the opening book.

use crate::book::{Book, Candidate, Entry};
use crate::solver::final_score;
use crate::{Board, Position};

#[derive(Debug, Clone, Copy)]
pub struct BackupChange {
    pub ply: usize,
    pub mv: Position,
    pub before: Option<f32>,
    pub after: f32,
    pub best: f32,
    pub new_entry: bool,
}

#[derive(Debug, Default, Clone)]
pub struct BackupOutcome {
    pub updated: usize,
    pub added: usize,
    pub changes: Vec<BackupChange>,
}

pub type Line = Vec<(Board, Option<Position>)>;

pub fn replay(start: Option<&str>, kifu: &str) -> Result<(Line, Board), String> {
    let mut b = match start {
        Some(s) => Board::from_string(s).map_err(|e| format!("start position: {e:?}"))?,
        None => Board::new(),
    };
    let mut seq = Vec::new();
    let chars: Vec<char> = kifu.chars().collect();
    let mut i = 0;
    while i + 2 <= chars.len() {
        let mv = Position::from_kifu(&chars[i..i + 2].iter().collect::<String>())
            .map_err(|e| format!("kifu move {}{}: {e}", chars[i], chars[i + 1]))?;
        if !b.check(mv) && b.movable() == 0 {
            seq.push((b, None));
            b.pass();
        }
        if !b.check(mv) {
            return Err(format!("illegal move {mv:?}"));
        }
        seq.push((b, Some(mv)));
        b.make_move(mv).map_err(|e| format!("{e:?}"))?;
        i += 2;
    }
    Ok((seq, b))
}

pub enum JobStep {
    Search(Board),
    Done(BackupOutcome),
}

pub struct BackupJob {
    line: Line,
    idx: usize,
    v_next: f32,
    terminal: Board,
    awaiting_terminal: bool,
    alt_queue: Vec<Position>,
    pending_alt: Option<Position>,
    alt_best: Option<(Position, f32)>,
    evaluating: bool,
    new_depth: u8,
    out: BackupOutcome,
    done: bool,
}

impl BackupJob {
    pub fn new(start: Option<&str>, kifu: &str, new_depth: u8) -> Result<BackupJob, String> {
        let (line, terminal) = replay(start, kifu)?;
        if line.is_empty() {
            return Err("empty game record".into());
        }
        let over = terminal.is_game_over();
        let v_next = if over {
            final_score(&terminal) as f32
        } else {
            f32::NAN // filled by feed
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

    pub fn remaining(&self) -> usize {
        self.idx
    }

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
                self.v_next = -self.v_next;
                self.idx -= 1;
                continue;
            };
            let (key, i) = Book::key(&board);
            let fresh = learned.get_raw(key).is_none();
            if fresh {
                if let Some(b) = base.get_raw(key) {
                    learned.insert_raw(key, b.clone());
                } else {
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
                        complete: true,
                    };
                    learned.insert_raw(key, e);
                    self.out.added += 1;
                }
            }
            let e = learned.get_raw_mut(key).expect("inserted just above");
            let mapped = Book::map_move(mv, i);
            let before = e.moves.iter().find(|c| c.mv == mapped).map(|c| c.value);
            let after = -self.v_next;
            e.update_move(mapped, after);
            self.out.updated += 1;
            self.out.changes.push(BackupChange {
                ply: self.line[..self.idx - 1]
                    .iter()
                    .filter(|(_, m)| m.is_some())
                    .count()
                    + 1,
                mv,
                before,
                after,
                best: e.best().map(|c| c.value).unwrap_or(after),
                new_entry: fresh,
            });
            self.v_next = e.best().map(|c| c.value).unwrap_or(-self.v_next);
            base.insert_raw(key, e.clone());
            self.idx -= 1;
        }
    }

    pub fn feed(&mut self, value: f32) {
        if self.awaiting_terminal {
            self.v_next = value;
            self.awaiting_terminal = false;
            return;
        }
        if let Some(p) = self.pending_alt.take() {
            let v = -value; // child's view -> this position's view
            if self.alt_best.is_none_or(|(_, bv)| v > bv) {
                self.alt_best = Some((p, v));
            }
        }
    }
}

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
                complete: true,
            },
        );
        let mut learned = Book::new();

        let mut job = BackupJob::new(None, "f5d6", 14).expect("replays");
        let mut first = true;
        let out = run_job(&mut job, &mut learned, &mut base, |_b| {
            if std::mem::take(&mut first) {
                -10.0 // terminal (after f5d6, Black to move)
            } else {
                -0.1 // alternative child (opponent view) -> all alts +0.1
            }
        });
        assert_eq!(out.added, 1, "the position after f5 gets added");
        assert_eq!(out.updated, 2, "both d6 and f5 get re-valued");

        let e0 = base.get_raw(key0).unwrap();
        assert_eq!(
            e0.best().unwrap().mv.index(),
            Book::map_move(pos("d3"), i0).index(),
            "f5 must drop and d3 become best"
        );
        assert_eq!(learned.len(), 2);
        assert_eq!(base.len(), 2, "learned positions reach the play-side book");
    }

    #[test]
    fn loss_stops_at_the_losing_move() {
        let mut base = Book::new();
        let mut learned = Book::new();
        let mut job = BackupJob::new(None, "f5d6c3", 14).unwrap();
        let mut first = true;
        let out = run_job(&mut job, &mut learned, &mut base, |b| {
            if std::mem::take(&mut first) {
                return 20.0; // terminal (after c3, +20 White view = Black loss)
            }
            match 64 - b.empty_count() {
                7 => -1.5, // c3-alternative child (White view) -> alt = +1.5
                _ => -0.1, // other alternative children -> alt = +0.1
            }
        });
        assert_eq!(out.updated, 3);

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
            "f5, rootward of the losing move, must stay neutral (got {f5v})"
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
        assert!(
            (c3v + 20.0).abs() < 1e-6,
            "losing move c3 = -20 (got {c3v})"
        );
    }

    #[test]
    fn loss_propagates_when_alternatives_are_also_bad() {
        let mut base = Book::new();
        let mut learned = Book::new();
        let mut job = BackupJob::new(None, "f5d6c3", 14).unwrap();
        let mut first = true;
        run_job(&mut job, &mut learned, &mut base, |_b| {
            if std::mem::take(&mut first) {
                20.0 // terminal
            } else {
                15.0 // every alt child gives the opponent +15 -> alts lose big
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
        assert!(
            f5v < -10.0,
            "with no good alternative the loss propagates ({f5v})"
        );
    }

    #[test]
    fn winning_games_are_absorbed_too() {
        let mut base = Book::new();
        let mut learned = Book::new();
        let mut job = BackupJob::new(None, "f5d6", 14).unwrap();
        let mut first = true;
        let out = run_job(&mut job, &mut learned, &mut base, |_b| {
            if std::mem::take(&mut first) {
                8.0 // terminal: Black is winning
            } else {
                -0.1 // alternative child -> alt = +0.1
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
        assert!(
            (f5v + 0.1).abs() < 1e-6,
            "even a win propagates through the alternative ({f5v})"
        );
    }

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
                complete: true,
            },
        );
        let mut learned = Book::new();
        let mut job = BackupJob::new(None, "f5", 14).unwrap();
        let mut searches = 0;
        run_job(&mut job, &mut learned, &mut base, |_b| {
            searches += 1;
            0.0
        });
        assert_eq!(
            searches, 1,
            "known positions must not re-measure alternatives"
        );
    }

    #[test]
    fn replay_inserts_passes() {
        let kifu = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";
        let (line, fin) = replay(None, kifu).expect("replays with passes");
        assert!(fin.is_game_over(), "replays to game over");
        assert!(line.iter().any(|(_, m)| m.is_none()), "passes are inserted");
    }

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
            complete: true,
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

pub fn undo_backup(
    learned: &mut Book,
    start: Option<&str>,
    kifu: &str,
    changes: &[BackupChange],
) -> Result<usize, String> {
    let (line, _) = replay(start, kifu)?;
    let boards: Vec<Board> = line
        .iter()
        .filter(|(_, m)| m.is_some())
        .map(|(b, _)| *b)
        .collect();
    let mut n = 0;
    for c in changes.iter().rev() {
        let Some(board) = boards.get(c.ply.wrapping_sub(1)) else {
            continue;
        };
        let (key, i) = Book::key(board);
        if c.new_entry {
            if learned.remove_raw(key).is_some() {
                n += 1;
            }
            continue;
        }
        let Some(e) = learned.get_raw_mut(key) else {
            continue;
        };
        let mapped = Book::map_move(c.mv, i);
        match c.before {
            Some(v) => e.update_move(mapped, v),
            None => e.remove_move(mapped),
        }
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod undo_tests {
    use super::*;

    #[test]
    fn undo_restores_learned() {
        let kifu = "f5d6c3d3c4f4c5b3c2e6";
        let mut learned = Book::new();
        let mut base = Book::new();
        let mut job = BackupJob::new(None, kifu, 8).expect("job builds");
        let out = loop {
            match job.next(&mut learned, &mut base) {
                JobStep::Search(_) => job.feed(0.0),
                JobStep::Done(o) => break o,
            }
        };
        assert!(out.updated > 0, "nothing was rewritten");
        assert!(!learned.is_empty());
        let n = undo_backup(&mut learned, None, kifu, &out.changes).expect("undo succeeds");
        assert!(n > 0);
        assert!(
            learned.is_empty(),
            "imported into an empty book, so undo must empty it"
        );
    }
}
