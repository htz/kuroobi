//! Learning: importing played games into the opening book.
//!
//! Own games are imported win or lose. Each position along the game gets
//! the played move plus the best alternative among the other legal moves,
//! and the final disc difference (or a search value for unfinished games)
//! is backed up to the root with negamax.
//!
//! - Losing moves lose value, so `probe_varied` naturally branches to an
//!   alternative next time — the same loss is not repeated.
//! - Each position's value is the best of its candidates, so a losing
//!   value only propagates through the span where even the alternatives
//!   were bad, and stops at the losing move; normal opening moves keep
//!   neutral values.
//! - Wins are imported too, so a line won only through the opponent's
//!   mistake does not stay overrated.
//!
//! There is no special avoidance logic: "don't repeat the loss" falls
//! out of the negamax values; nothing forbids moves.
//!
//! Import advances one search at a time ([`BackupJob`]) so it can run
//! between games without stalling thinking or server replies. Learned
//! entries are saved separately from the main book and overlaid at load
//! (no conflict while bookgen rebuilds the base).

use crate::book::{Book, Candidate, Entry};
use crate::solver::final_score;
use crate::{Board, Position};

/// One rewritten move.
///
/// The old and new values are kept for auditability: back-up overwrites
/// values, so one bad game changes later play, and without a record it
/// could be neither found nor reverted.
#[derive(Debug, Clone, Copy)]
pub struct BackupChange {
    /// Move number in the game record (1-based, passes excluded).
    pub ply: usize,
    /// The move actually played (board orientation, not normalized space).
    pub mv: Position,
    /// Value before the overwrite; None if the move was not in the book.
    pub before: Option<f32>,
    /// Value after the overwrite.
    pub after: f32,
    /// Best value of the position after the rewrite;
    /// `best - after` is what the move cost in discs.
    pub best: f32,
    /// Whether this import created the entry; undo either restores the
    /// value or deletes the whole entry accordingly.
    pub new_entry: bool,
}

/// Import result.
#[derive(Debug, Default, Clone)]
pub struct BackupOutcome {
    /// Moves whose values changed.
    pub updated: usize,
    /// Positions newly added to the learned overlay.
    pub added: usize,
    /// Rewrite details, ordered from the end of the game backwards.
    pub changes: Vec<BackupChange>,
}

/// Replayed line; each element is (board before the move, move), pass = `None`.
pub type Line = Vec<(Board, Option<Position>)>;

/// Turn a game record (f5d6...) into a (board, move) list, inserting
/// passes; also returns the final board.
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
        // Insert a pass when the mover has no legal move.
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

/// What [`BackupJob::next`] wants next.
pub enum JobStep {
    /// Evaluate this board; return the mover-view value via [`BackupJob::feed`].
    Search(Board),
    /// Import finished.
    Done(BackupOutcome),
}

/// State machine that advances one game's import one search at a time.
///
/// It owns no search: when `next` returns `Search(board)` the caller
/// evaluates and passes the value back via `feed`. The split lets
/// learning run in slices between games and lets the logic be tested
/// without a search.
pub struct BackupJob {
    line: Line,
    /// Next position = `line[idx - 1]`, walking from the end; 0 = done.
    idx: usize,
    /// Mover-view value of the child (one ply later) position.
    v_next: f32,
    /// Terminal board (unfinished games wait for a search value).
    terminal: Board,
    awaiting_terminal: bool,
    /// Alternatives pending evaluation (legal moves other than the played one).
    alt_queue: Vec<Position>,
    /// Alternative last sent to `Search`; `feed` uses it to update `alt_best`.
    pending_alt: Option<Position>,
    alt_best: Option<(Position, f32)>,
    /// Whether alternatives are being evaluated (distinguishes an empty
    /// `alt_queue` from "not started").
    evaluating: bool,
    /// Search depth recorded on new entries.
    new_depth: u8,
    out: BackupOutcome,
    done: bool,
}

impl BackupJob {
    /// Prepare one game's import. Finished games use the exact disc
    /// difference as the terminal value; otherwise the first `next` asks
    /// for a terminal evaluation.
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

    /// Approximate remaining work (positions left); for display.
    pub fn remaining(&self) -> usize {
        self.idx
    }

    /// Advance and return the next step. After a `Search`, call `feed`
    /// with the value before calling again.
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
                // Pass: same board, only the mover flips.
                self.v_next = -self.v_next;
                self.idx -= 1;
                continue;
            };
            let (key, i) = Book::key(&board);
            let fresh = learned.get_raw(key).is_none();
            if fresh {
                if let Some(b) = base.get_raw(key) {
                    // Start from the base book's candidates (deep values).
                    learned.insert_raw(key, b.clone());
                } else {
                    // Unknown position: measure the best of the other
                    // legal moves and store it too. This is the wall that
                    // stops loss propagation — with only the played move
                    // as candidate, a losing value would flow straight
                    // past the losing move into the opening.
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
                    // All alternatives evaluated: create the entry.
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
            let e = learned.get_raw_mut(key).expect("inserted just above");
            let mapped = Book::map_move(mv, i);
            let before = e.moves.iter().find(|c| c.mv == mapped).map(|c| c.value);
            let after = -self.v_next;
            e.update_move(mapped, after);
            self.out.updated += 1;
            self.out.changes.push(BackupChange {
                // Passes don't count as moves (keeps numbers aligned with the record).
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
            // The position's value is the post-update best; a better
            // alternative propagates instead (no loss past the losing move).
            self.v_next = e.best().map(|c| c.value).unwrap_or(-self.v_next);
            // Mirror into the merged book used for play.
            base.insert_raw(key, e.clone());
            self.idx -= 1;
        }
    }

    /// Return the result of the last `Search` (mover-view value).
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

/// Overlay learned entries onto the base book (learned side wins).
/// Learned entries started from the base candidates before overwriting
/// with game outcomes, so candidates survive and only values refresh.
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

    /// Drive a job to completion; `eval` answers the Search requests.
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

    /// Losing after f5d6 (-10 for Black at the end) lowers f5 in the
    /// opening and promotes the former runner-up d3.
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

        // Unfinished record: terminal value is a search value (-10,
        // Black view). The first Search is the terminal, the rest are
        // alternative children.
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

        // d6 = +10 (the opponent's -10); f5 = -(best after d6).
        // In the opening f5 drops and d3 becomes best.
        let e0 = base.get_raw(key0).unwrap();
        assert_eq!(
            e0.best().unwrap().mv.index(),
            Book::map_move(pos("d3"), i0).index(),
            "f5 must drop and d3 become best"
        );
        // The learned overlay has the entry and the play-side book sees it.
        assert_eq!(learned.len(), 2);
        assert_eq!(base.len(), 2, "learned positions reach the play-side book");
    }

    /// Propagation stops at the losing move: with a good alternative en
    /// route, rootward moves keep neutral values. This is what keeps a
    /// normal second move from being marked "lost from move one".
    #[test]
    fn loss_stops_at_the_losing_move() {
        let mut base = Book::new();
        let mut learned = Book::new();
        // Black loses after f5 d6 c3 (+20 for White after c3). c3 is
        // the losing move; only its position has a +1.5 alternative.
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

        // c3 gets -20 but its position keeps the +1.5 alternative.
        // Rootward: d6 = -1.5, f5 = -0.1 — the opening stays neutral.
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

    /// Where alternatives are also bad the losing value does propagate.
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

    /// Wins are imported too, with their values.
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
        // Terminal +8 -> d6 = -8 -> best after d6 is the +0.1 alt ->
        // f5 = -0.1. A win owed to the opponent's blunder does not
        // inflate f5.
        assert!(
            (f5v + 0.1).abs() < 1e-6,
            "even a win propagates through the alternative ({f5v})"
        );
    }

    /// Positions already in the base book inherit candidates and skip
    /// alternative evaluation.
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
        // Unfinished record: one terminal evaluation only; the opening
        // is in the base book so no alternative searches run.
        assert_eq!(
            searches, 1,
            "known positions must not re-measure alternatives"
        );
    }

    /// A real game record containing passes replays to the end.
    #[test]
    fn replay_inserts_passes() {
        let kifu = "e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2a6c1d1e1f2f1f7h3a5a7a8b7g2g8h8g1b3a4a2b2a1b1g7g6h6h7h5h4h2h1";
        let (line, fin) = replay(None, kifu).expect("replays with passes");
        assert!(fin.is_game_over(), "replays to game over");
        assert!(line.iter().any(|(_, m)| m.is_none()), "passes are inserted");
    }

    /// merge_learned overwrites wholesale, learned side first.
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

/// Undo one game's import: restore recorded `before` values and delete
/// entries this import created. Returns how many moves were reverted.
///
/// The base book is left alone — it is file + overlay, so after fixing
/// the overlay the right move is to reload, not to guess what the file
/// originally contained.
pub fn undo_backup(
    learned: &mut Book,
    start: Option<&str>,
    kifu: &str,
    changes: &[BackupChange],
) -> Result<usize, String> {
    let (line, _) = replay(start, kifu)?;
    // Records address positions by move number (passes excluded); walk
    // the boards with the same numbering.
    let boards: Vec<Board> = line
        .iter()
        .filter(|(_, m)| m.is_some())
        .map(|(b, _)| *b)
        .collect();
    let mut n = 0;
    // Revert in reverse write order; correct even when a position
    // repeats within the game.
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

    /// Import followed by undo restores the overlay.
    #[test]
    fn undo_restores_learned() {
        let kifu = "f5d6c3d3c4f4c5b3c2e6";
        let mut learned = Book::new();
        let mut base = Book::new();
        let mut job = BackupJob::new(None, kifu, 8).expect("job builds");
        // Return 0 instead of searching; undo does not care about values.
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
