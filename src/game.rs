//! Full Reversi game with history, KIFU, and game management.

use crate::board::{Board, GameError, MoveError};
use crate::color::Color;
use crate::position::Position;

#[derive(Debug, Clone)]
pub struct MoveRecord {
    pub pos: Option<Position>, // None for pass
    pub flipped: u64,
}

pub struct Reversi {
    pub board: Board,
    pub history: Vec<MoveRecord>,
    redo_stack: Vec<MoveRecord>,
}

impl Reversi {
    pub fn new() -> Reversi {
        Self::from_board(Board::new())
    }

    pub fn from_string(s: &str) -> Result<Reversi, String> {
        let board = Board::from_string(s).map_err(|e| format!("parse error: {:?}", e))?;
        Ok(Self::from_board(board))
    }

    fn from_board(board: Board) -> Reversi {
        Reversi {
            board,
            history: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn make_move(&mut self, pos: Position) -> Result<(), MoveError> {
        let pos_bit = pos.to_bit();
        if (self.board.black | self.board.white) & pos_bit != 0 {
            return Err(MoveError::Occupied);
        }
        let flipped =
            crate::bitboard::flippable(self.board.player_bb(), self.board.opponent_bb(), pos_bit);
        if flipped == 0 {
            return Err(MoveError::NotPlayable);
        }

        self.history.push(MoveRecord {
            pos: Some(pos),
            flipped,
        });
        self.redo_stack.clear();
        self.board.apply_flips(pos, flipped);
        Ok(())
    }

    pub fn pass(&mut self) {
        self.history.push(MoveRecord {
            pos: None,
            flipped: 0,
        });
        self.redo_stack.clear();
        self.board.pass();
    }

    pub fn undo(&mut self) -> Result<(), GameError> {
        let record = self.history.pop().ok_or(GameError::NoMoves)?;

        if let Some(pos) = record.pos {
            self.board.undo_move(pos, record.flipped);
        } else {
            self.board.pass();
        }

        self.redo_stack.push(record);

        Ok(())
    }

    pub fn redo(&mut self) -> Result<(), GameError> {
        let record = self.redo_stack.pop().ok_or(GameError::NoMoves)?;

        if let Some(pos) = record.pos {
            self.board.apply_flips(pos, record.flipped);
        } else {
            self.board.pass();
        }

        self.history.push(record);

        Ok(())
    }

    pub fn line(&self) -> Vec<Option<Position>> {
        self.history
            .iter()
            .map(|r| r.pos)
            .chain(self.redo_stack.iter().rev().map(|r| r.pos))
            .collect()
    }

    pub fn movable(&self) -> u64 {
        self.board.movable()
    }

    pub fn is_game_over(&self) -> bool {
        self.board.is_game_over()
    }

    pub fn piece_count(&self) -> (u8, u8) {
        self.board.piece_count()
    }

    pub fn player(&self) -> Color {
        self.board.player()
    }

    pub fn to_kifu(&self) -> String {
        self.history
            .iter()
            .filter_map(|r| r.pos.map(|p| p.to_kifu()))
            .collect()
    }

    pub fn from_kifu(kifu: &str) -> Result<Reversi, String> {
        let mut game = Reversi::new();
        game.replay_kifu(kifu)?;
        Ok(game)
    }

    pub fn from_kifu_with_start(start: &str, kifu: &str) -> Result<Reversi, String> {
        let mut game = Reversi::from_string(start)?;
        game.replay_kifu(kifu)?;
        Ok(game)
    }

    fn replay_kifu(&mut self, kifu: &str) -> Result<(), String> {
        let chars: Vec<char> = kifu.chars().collect();

        if !chars.len().is_multiple_of(2) {
            return Err(format!("KIFU length is not valid: {kifu}"));
        }

        for chunk in chars.chunks(2) {
            let s: String = chunk.iter().collect();
            let pos = Position::from_kifu(&s).map_err(|e| format!("KIFU parse error: {}", e))?;

            if !self.board.check(pos) && !self.board.check_all() {
                self.pass();
            }
            self.make_move(pos)
                .map_err(|e| format!("cannot replay KIFU move ({s}): {e}"))?;
        }

        Ok(())
    }

    pub fn move_count(&self) -> usize {
        self.history.len()
    }

    pub fn movable_list(&self) -> Vec<Position> {
        self.board.movable_iter().collect()
    }
}

impl Default for Reversi {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BOARD_INIT_STRING;

    #[test]
    fn test_new_game() {
        let g = Reversi::new();
        assert_eq!(g.player(), Color::Black);
        assert_eq!(g.piece_count(), (2, 2));
        assert_eq!(g.board.empty_count(), 60);
        assert_eq!(g.move_count(), 0);
        assert_eq!(g.to_kifu(), "");
    }

    #[test]
    fn test_make_move() {
        let mut g = Reversi::new();

        let pos = Position(19);
        let result = g.make_move(pos);
        assert!(result.is_ok(), "First move should be valid: {result:?}");

        assert_eq!(g.move_count(), 1);
        assert_eq!(g.piece_count(), (4, 1));
        assert_eq!(g.player(), Color::White);
    }

    #[test]
    fn test_invalid_move() {
        let mut g = Reversi::new();

        let result = g.make_move(Position(28));
        assert_eq!(result, Err(MoveError::Occupied));

        let result = g.make_move(Position(0)); // A1 is far from pieces
        assert_eq!(result, Err(MoveError::NotPlayable));
    }

    #[test]
    fn test_pass() {
        let mut g = Reversi::new();

        g.make_move(Position(19)).unwrap();

        g.pass();
        assert_eq!(g.move_count(), 2);
        assert_eq!(g.player(), Color::Black);
    }

    #[test]
    fn test_undo_redo() {
        let mut g = Reversi::new();

        g.make_move(Position(19)).unwrap();

        g.undo().unwrap();
        assert_eq!(g.move_count(), 0);
        assert_eq!(g.piece_count(), (2, 2));
        assert_eq!(g.player(), Color::Black);

        g.redo().unwrap();
        assert_eq!(g.move_count(), 1);
        assert_eq!(g.piece_count(), (4, 1));
    }

    fn state(g: &Reversi) -> (u64, u64, Color) {
        (g.board.black, g.board.white, g.player())
    }

    #[test]
    fn test_undo_redo_edge_cases() {
        let mut g = Reversi::new();

        assert!(g.undo().is_err(), "undo on fresh game must fail");
        assert!(g.redo().is_err(), "redo on fresh game must fail");

        g.make_move(Position(19)).unwrap();
        let after_move = state(&g);
        g.undo().unwrap();
        g.redo().unwrap();
        assert_eq!(state(&g), after_move, "redo restores the exact board");

        g.undo().unwrap();
        g.make_move(Position(26)).unwrap(); // d3, another legal opening
        assert!(g.redo().is_err(), "new move must clear the redo stack");

        let mut g = Reversi::new();
        g.make_move(Position(19)).unwrap();
        let m2 = Position::from_index(g.movable().trailing_zeros()).unwrap();
        g.make_move(m2).unwrap();
        let last = state(&g);
        g.undo().unwrap();
        g.undo().unwrap();
        assert_eq!(g.piece_count(), (2, 2));
        g.redo().unwrap();
        g.redo().unwrap();
        assert_eq!(state(&g), last, "two undos + two redos round-trip");
    }

    #[test]
    fn test_undo_after_pass() {
        let mut g = Reversi::new();
        g.make_move(Position(19)).unwrap();
        let player_before_pass = g.player();

        g.pass();
        g.undo().unwrap();

        assert_eq!(
            g.player(),
            player_before_pass,
            "undoing a pass restores player"
        );
        assert_eq!(g.move_count(), 1);
    }

    #[test]
    fn test_kifu_roundtrip() {
        let mut g = Reversi::new();

        g.make_move(Position(19)).unwrap(); // c4

        let movable = g.movable();
        let first = movable.trailing_zeros();
        let pos2 = Position::from_index(first).unwrap();
        g.make_move(pos2).unwrap();

        let kifu = g.to_kifu();
        assert!(!kifu.is_empty(), "KIFU should not be empty");

        let g2 = Reversi::from_kifu(&kifu).unwrap();
        assert_eq!(g2.piece_count(), g.piece_count());
        assert_eq!(g2.player(), g.player());
    }

    #[test]
    fn test_movable_list() {
        let g = Reversi::new();
        let list = g.movable_list();
        assert_eq!(list.len(), 4, "Initial position: 4 moves for Black");

        for pos in &list {
            assert!(g.board.check(*pos));
        }
    }

    #[test]
    fn test_game_over_detection() {
        let g = Reversi::new();
        assert!(!g.is_game_over(), "Initial position: game not over");
    }

    #[test]
    fn test_from_string() {
        let g = Reversi::from_string(BOARD_INIT_STRING).unwrap();
        assert_eq!(g.piece_count(), (2, 2));
        assert_eq!(g.player(), Color::Black);
    }

    #[test]
    fn test_from_string_invalid() {
        assert!(Reversi::from_string("invalid").is_err());
    }

    #[test]
    fn test_full_game_and_kifu_replay() {
        let mut g = Reversi::new();
        let mut plies = 0;
        loop {
            let moves = g.movable();
            if moves == 0 {
                if g.is_game_over() {
                    break;
                }
                g.pass();
                continue;
            }
            let bit = if plies % 2 == 0 {
                moves.trailing_zeros()
            } else {
                63 - moves.leading_zeros()
            };
            g.make_move(Position::from_index(bit).unwrap()).unwrap();
            plies += 1;

            let (b, w) = g.piece_count();
            assert_eq!(
                b as u32 + w as u32 + g.board.empty_count() as u32,
                64,
                "piece conservation"
            );
        }
        assert!(plies >= 20, "a full game plays plenty of moves");
        assert!(g.is_game_over());

        let replay = Reversi::from_kifu(&g.to_kifu()).unwrap();
        assert_eq!(replay.board.black, g.board.black);
        assert_eq!(replay.board.white, g.board.white);
    }

    #[test]
    fn kifu_from_a_drawn_opening() {
        let mut drawn = Reversi::new();
        for _ in 0..5 {
            let p = drawn.board.movable_iter().next().unwrap();
            drawn.make_move(p).unwrap();
        }
        let start = drawn.board.to_string();
        let stones_at_start = (drawn.board.black | drawn.board.white).count_ones();

        let mut played = String::new();
        for _ in 0..3 {
            let p = drawn.board.movable_iter().next().unwrap();
            drawn.make_move(p).unwrap();
            played.push_str(&p.to_kifu());
        }

        let replayed = Reversi::from_kifu_with_start(&start, &played).unwrap();
        assert_eq!(replayed.board.black, drawn.board.black);
        assert_eq!(replayed.board.white, drawn.board.white);
        assert_eq!(replayed.board.player(), drawn.board.player());

        match Reversi::from_kifu(&played) {
            Err(_) => {}
            Ok(naive) => assert_ne!(naive.board.black, drawn.board.black),
        }
        assert_eq!(stones_at_start, 4 + 5);
    }

    #[test]
    fn kifu_with_start_defaults_to_standard_opening() {
        let start = Board::new().to_string();
        let a = Reversi::from_kifu_with_start(&start, "f5d6c3").unwrap();
        let b = Reversi::from_kifu("f5d6c3").unwrap();
        assert_eq!(a.board.black, b.board.black);
        assert_eq!(a.board.white, b.board.white);
    }
}
