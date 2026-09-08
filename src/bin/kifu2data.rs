//! Kifu-to-training-data converter: replay game records and emit every
//! position as a training record (`kuroobi::record`).
//!
//! Inputs are text files of `f5d6...` transcripts, one game per line, or
//! WTHOR archives (`.wtb`). A game gives the board, the move played, the
//! side to move and the final result (disc difference, empties to the
//! winner); the record keeps all of it. What a game record does not have is
//! a search value, so the result stands in for it: the score-disagreement
//! filter never fires on this data, and a random-opening position the
//! filter keeps is taught the result.
//!
//! A game is used only if it ran to the end: an illegal move, or a final
//! position where someone can still move, leaves the result unknown, and
//! such games are skipped and counted. A WTHOR game must also agree with
//! the archive's own score for it.
//!
//! Usage:
//!   kifu2data [OPTIONS] <transcript.txt | archive.wtb>...
//!
//! Options:
//!   --limit-games <n>   Convert at most n games per input file (default all)
//!   --skip-games <n>    Skip the first n games of each input file (for
//!                       carving out a validation set disjoint from training)
//!   --random-plies <k>  Flag the first k positions of each game as reached
//!                       by random moves (datasets whose first k moves are
//!                       random; the trainer's `--drop-random` drops them)
//!   --out <file>        Concatenate everything into one output file
//!   --out-dir <dir>     One output per input: <dir>/<input stem>.data
//!                       (default: alongside inputs)
//!   --min-ply, --max-score-diff, --drop-random, --keep-above-ply
//!                       The training filter (see
//!                       `kuroobi::record::Filter`), applied while writing
//!                       so that positions the trainer would drop anyway
//!                       never reach the disk. Dropped records are counted.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::record::{Filter, Record, Writer};
use kuroobi::{wthor, Board, Color, Position};

struct RecordedGame {
    /// Positions before each move, in play order, with the move made.
    positions: Vec<(Board, Position)>,
    /// Final score from Black's perspective, empties to the winner.
    score_black: i32,
}

/// Replay one transcript. Returns None if the game is corrupt (illegal
/// move, bad coordinates) or did not run to the end.
fn replay(line: &str) -> Option<RecordedGame> {
    let s = line.trim();
    if s.is_empty() || !s.len().is_multiple_of(2) {
        return None;
    }

    let mut board = Board::new();
    let mut positions = Vec::with_capacity(s.len() / 2 + 1);
    let bytes = s.as_bytes();

    for chunk in bytes.chunks(2) {
        let file = chunk[0].to_ascii_lowercase().wrapping_sub(b'a');
        let rank = chunk[1].wrapping_sub(b'1');
        if file >= 8 || rank >= 8 {
            return None;
        }
        let pos = Position::from_file_rank(file, rank)?;

        // Forced pass is implicit in the transcript
        if board.movable() == 0 {
            board.pass();
        }
        if !board.check(pos) {
            return None;
        }
        positions.push((board, pos));
        board.make_move_unchecked(pos);
    }
    if !board.is_game_over() {
        return None;
    }

    let diff = board.black_count() as i32 - board.white_count() as i32;
    let empties = board.empty_count() as i32;
    let score_black = match diff.cmp(&0) {
        std::cmp::Ordering::Greater => diff + empties,
        std::cmp::Ordering::Less => diff - empties,
        std::cmp::Ordering::Equal => 0,
    };

    Some(RecordedGame {
        positions,
        score_black,
    })
}

/// One position of a replayed game as a record. The mover's discs come
/// first and the result is turned to the mover's view.
fn record(board: &Board, mv: Position, score_black: i32, random: bool, game_id: u16) -> Record {
    let (mover, opponent, score) = if board.player() == Color::Black {
        (board.black, board.white, score_black)
    } else {
        (board.white, board.black, -score_black)
    };
    let score = score.clamp(-64, 64) as i8;
    Record {
        mover,
        opponent,
        score: f32::from(score),
        game_score: score,
        ply: 60 - board.empty_count(),
        random,
        sq: mv.index(),
        black_to_move: board.player() == Color::Black,
        game_id,
    }
}

/// The games of one input as transcripts, each with the score the input
/// itself states for it (WTHOR does, a transcript file does not).
fn read_games(input: &std::path::Path) -> std::io::Result<Vec<(String, Option<i32>)>> {
    if input
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wtb"))
    {
        return Ok(wthor::read(input)?
            .into_iter()
            .map(|g| (g.transcript(), Some(g.score_black())))
            .collect());
    }
    let mut games = Vec::new();
    for line in BufReader::new(File::open(input)?).lines() {
        games.push((line?, None));
    }
    Ok(games)
}

fn main() -> ExitCode {
    let mut limit_games: Option<usize> = None;
    let mut skip_games = 0usize;
    let mut random_plies = 0usize;
    let mut out_dir: Option<PathBuf> = None;
    let mut out_file: Option<PathBuf> = None;
    let mut filter = Filter::NONE;
    let mut inputs: Vec<PathBuf> = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match filter.take_flag(&arg, &mut it) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        }
        match arg.as_str() {
            "--limit-games" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) => limit_games = Some(n),
                None => {
                    eprintln!("--limit-games requires a number");
                    return ExitCode::FAILURE;
                }
            },
            "--skip-games" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) => skip_games = n,
                None => {
                    eprintln!("--skip-games requires a number");
                    return ExitCode::FAILURE;
                }
            },
            "--random-plies" => match it.next().and_then(|v| v.parse().ok()) {
                Some(k) => random_plies = k,
                None => {
                    eprintln!("--random-plies requires a number");
                    return ExitCode::FAILURE;
                }
            },
            "--out" => match it.next() {
                Some(f) => out_file = Some(PathBuf::from(f)),
                None => {
                    eprintln!("--out requires a value");
                    return ExitCode::FAILURE;
                }
            },
            "--out-dir" => match it.next() {
                Some(d) => out_dir = Some(PathBuf::from(d)),
                None => {
                    eprintln!("--out-dir requires a value");
                    return ExitCode::FAILURE;
                }
            },
            other => inputs.push(PathBuf::from(other)),
        }
    }
    if inputs.is_empty() {
        eprintln!("usage: kifu2data [OPTIONS] <transcript>...  (see --help in source)");
        return ExitCode::FAILURE;
    }
    if let Some(dir) = &out_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("failed to create {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
    }
    if let Some(parent) = out_file.as_ref().and_then(|f| f.parent()) {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("failed to create {}: {e}", parent.display());
            return ExitCode::FAILURE;
        }
    }

    // Single concatenated output, or per-input files.
    let mut single_out = match &out_file {
        Some(path) => match Writer::create(path) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("failed to create {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let mut total_games = 0u64;
    let mut total_positions = 0u64;
    let mut total_dropped = 0u64;
    let mut total_corrupt = 0u64;

    for input in &inputs {
        let games_in = match read_games(input) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("failed to read {}: {e}", input.display());
                return ExitCode::FAILURE;
            }
        };

        let mut per_input_out = if single_out.is_none() {
            let out_path = match &out_dir {
                Some(dir) => dir.join(format!(
                    "{}.data",
                    input.file_stem().and_then(|s| s.to_str()).unwrap_or("out")
                )),
                None => input.with_extension("data"),
            };
            match Writer::create(&out_path) {
                Ok(w) => Some((w, out_path)),
                Err(e) => {
                    eprintln!("failed to create {}: {e}", out_path.display());
                    return ExitCode::FAILURE;
                }
            }
        } else {
            None
        };

        let mut games = 0u64;
        let mut positions = 0u64;
        let mut corrupt = 0u64;
        let mut seen = 0usize;

        for (line, archive_score) in games_in {
            seen += 1;
            if seen <= skip_games {
                continue;
            }
            if let Some(max) = limit_games {
                if games >= max as u64 {
                    break;
                }
            }
            let game = replay(&line).filter(|g| archive_score.is_none_or(|s| s == g.score_black));
            match game {
                Some(game) => {
                    let out: &mut Writer = match (&mut single_out, &mut per_input_out) {
                        (Some(w), _) => w,
                        (None, Some((w, _))) => w,
                        _ => unreachable!(),
                    };
                    // Games are numbered within their input; the id only
                    // has to tell neighbours apart.
                    let game_id = (seen % 65536) as u16;
                    for (i, (b, mv)) in game.positions.iter().enumerate() {
                        let r = record(b, *mv, game.score_black, i < random_plies, game_id);
                        if !filter.keeps(&r) {
                            total_dropped += 1;
                            continue;
                        }
                        if out.write(&r).is_err() {
                            eprintln!("write error");
                            return ExitCode::FAILURE;
                        }
                        positions += 1;
                    }
                    games += 1;
                }
                None => corrupt += 1,
            }
        }
        if let Some((w, path)) = per_input_out {
            if w.finish().is_err() {
                eprintln!("flush error on {}", path.display());
                return ExitCode::FAILURE;
            }
        }

        total_games += games;
        total_positions += positions;
        total_corrupt += corrupt;
    }

    if let Some(w) = single_out {
        if w.finish().is_err() {
            eprintln!("flush error on --out file");
            return ExitCode::FAILURE;
        }
    }

    println!(
        "total: {total_games} games, {total_positions} positions written, {total_dropped} dropped by filter {}, {total_corrupt} skipped as corrupt or unfinished (random_plies {random_plies})",
        filter.describe()
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Play a deterministic pseudo-random game to the end; return the
    /// f5d6-style transcript and the final board.
    fn random_game(seed: u64) -> (String, Board) {
        let mut board = Board::new();
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let mut transcript = String::new();
        loop {
            let moves = board.movable();
            if moves == 0 {
                let mut p = board;
                p.pass();
                if p.movable() == 0 {
                    break;
                }
                board = p;
                continue;
            }
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut nth = (state >> 33) % moves.count_ones() as u64;
            let mut m = moves;
            while nth > 0 {
                m &= m - 1;
                nth -= 1;
            }
            let sq = m.trailing_zeros() as u8;
            let (file, rank) = (sq / 8, sq % 8);
            transcript.push((b'a' + file) as char);
            transcript.push((b'1' + rank) as char);
            board.make_move_unchecked(Position::from_index(sq as u32).unwrap());
        }
        (transcript, board)
    }

    #[test]
    fn test_replay_roundtrip_random_games() {
        for seed in 1..=20u64 {
            let (transcript, final_board) = random_game(seed);
            let game = replay(&transcript).expect("self-generated game must replay");
            assert_eq!(
                game.positions.len() * 2,
                transcript.len(),
                "one recorded position per move"
            );

            let diff = final_board.black_count() as i32 - final_board.white_count() as i32;
            let empties = final_board.empty_count() as i32;
            let expected = match diff.cmp(&0) {
                std::cmp::Ordering::Greater => diff + empties,
                std::cmp::Ordering::Less => diff - empties,
                std::cmp::Ordering::Equal => 0,
            };
            assert_eq!(game.score_black, expected, "seed {seed}: final score");

            // First recorded position is the initial board
            assert_eq!(game.positions[0].0.black, Board::new().black);
            assert_eq!(game.positions[0].0.white, Board::new().white);
        }
    }

    #[test]
    fn test_replay_rejects_illegal() {
        assert!(replay("a1f5").is_none(), "a1 is not a legal first move");
        assert!(replay("f5f5").is_none(), "duplicate square");
        assert!(replay("f9").is_none(), "bad coordinates");
        assert!(replay("f5d").is_none(), "odd length");
    }

    #[test]
    fn test_replay_rejects_an_unfinished_game() {
        let (transcript, _) = random_game(3);
        assert!(replay(&transcript).is_some());
        assert!(
            replay(&transcript[..transcript.len() - 2]).is_none(),
            "a game cut before its end has no result"
        );
    }

    #[test]
    fn test_record_takes_the_movers_view() {
        let mut board = Board::new();
        let pos = Position::from_index(board.movable().trailing_zeros()).unwrap();
        board.make_move_unchecked(pos); // now White to move
        assert_eq!(board.player(), Color::White);
        let next = Position::from_index(board.movable().trailing_zeros()).unwrap();

        let r = record(&board, next, 10, true, 7);
        // White's discs are the mover's, and the result is negated to match.
        assert_eq!(r.mover, board.white);
        assert_eq!(r.opponent, board.black);
        assert_eq!(r.game_score, -10);
        assert_eq!(r.score, -10.0);
        assert!(!r.black_to_move);
        assert!(r.random);
        assert_eq!(r.ply, 1);
        assert_eq!(r.sq, next.index());
        assert_eq!(r.game_id, 7);
        assert_eq!(Record::from_bytes(&r.to_bytes()), r);
    }
}
