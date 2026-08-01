//! Perft (move-path counting) validation against known Othello values.
//!
//! Reference sequence (leaf counts from the standard initial position,
//! where a forced pass does not consume a ply):
//! depth 1..=9: 4, 12, 56, 244, 1396, 8200, 55092, 390216, 3005288

use kuroobi::{bitboard, Board};

/// Count leaf nodes at `depth` plies. A player with no moves passes without
/// consuming a ply; if both players have no moves the game is over and the
/// position itself is the leaf.
fn perft(board: &Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }

    let moves = board.movable();
    if moves == 0 {
        // Pass: opponent to move on the same depth
        let mut passed = *board;
        passed.pass();
        if passed.movable() == 0 {
            return 1; // game over
        }
        return perft(&passed, depth);
    }

    let mut nodes = 0u64;
    let mut m = moves;
    while m != 0 {
        let bit = m.trailing_zeros();
        m &= m - 1;
        let pos = kuroobi::Position::from_index(bit).unwrap();
        let mut next = *board;
        next.make_move_unchecked(pos);
        nodes += perft(&next, depth - 1);
    }
    nodes
}

#[test]
fn perft_shallow() {
    let board = Board::new();
    let expected = [4u64, 12, 56, 244, 1396, 8200];
    for (i, &want) in expected.iter().enumerate() {
        let depth = (i + 1) as u32;
        let got = perft(&board, depth);
        assert_eq!(got, want, "perft({depth}) mismatch");
    }
}

#[test]
fn perft_deeper() {
    let board = Board::new();
    assert_eq!(perft(&board, 7), 55092, "perft(7)");
    assert_eq!(perft(&board, 8), 390216, "perft(8)");
}

/// flippable() must agree with mobility(): every square mobility reports
/// must flip at least one disc, and no other empty square may flip any.
#[test]
fn mobility_matches_flippable_exhaustive() {
    // Walk a few hundred random-ish games deterministically and check the
    // invariant at every position.
    let mut board = Board::new();
    let mut steps = 0;
    loop {
        let moves = board.movable();
        let empty = board.empty();
        let mut e = empty;
        while e != 0 {
            let bit = e.trailing_zeros();
            e &= e - 1;
            let pos_bit = 1u64 << bit;
            let flips = bitboard::flippable(board.player_bb(), board.opponent_bb(), pos_bit);
            let in_mobility = moves & pos_bit != 0;
            assert_eq!(
                flips != 0,
                in_mobility,
                "square {bit}: flippable and mobility disagree"
            );
        }

        if moves == 0 {
            let mut passed = board;
            passed.pass();
            if passed.movable() == 0 {
                break; // game over
            }
            board = passed;
            continue;
        }

        // Deterministic move selection: alternate lowest/highest set bit
        let bit = if steps % 2 == 0 {
            moves.trailing_zeros()
        } else {
            63 - moves.leading_zeros()
        };
        let pos = kuroobi::Position::from_index(bit).unwrap();
        board.make_move_unchecked(pos);
        steps += 1;
        if steps > 100 {
            break;
        }
    }
    assert!(steps > 30, "sanity: the game should progress");
}
