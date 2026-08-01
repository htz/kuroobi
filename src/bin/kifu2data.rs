//! Kifu-to-training-data converter: replay `f5d6...` transcripts and emit
//! labeled positions in the 17-byte binary format `train` consumes
//! (black u64 LE, white u64 LE, score i8; rank-major bits; positions
//! normalized to Black-to-move with the score from Black's perspective).
//!
//! Every position of every game is labeled with the game's final score
//! (disc difference, empties awarded to the winner). Games with illegal
//! moves are skipped and counted.
//!
//! Usage:
//!   kifu2data [OPTIONS] <transcript>...
//!
//! Options:
//!   --limit-games <n>  Convert at most n games per input file (default all)
//!   --skip-games <n>   Skip the first n games of each input file (for
//!                      carving out a validation set disjoint from training)
//!   --skip-plies <k>   Don't record the first k positions of each game
//!                      (use for datasets whose first k moves are random:
//!                      their outcome labels are noise)
//!   --out <file>       Concatenate everything into one output file
//!   --out-dir <dir>    One output per input: <dir>/<input stem>.data
//!                      (default: alongside inputs)

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::{bitboard, Board, Color, Position};

struct RecordedGame {
    /// Positions before each move, in play order.
    positions: Vec<Board>,
    /// Final score from Black's perspective, empties to the winner.
    score_black: i32,
}

/// Replay one transcript line. Returns None if the game is corrupt
/// (illegal move / bad coordinates).
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
        positions.push(board);
        board.make_move_unchecked(pos);
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

/// Write one position as a normalized 17-byte record.
fn write_record(w: &mut impl Write, board: &Board, score_black: i32) -> std::io::Result<()> {
    // Normalize to Black to move: White-to-move positions are stored with
    // the color planes swapped and the score negated.
    let (black, white, score) = if board.player() == Color::Black {
        (board.black, board.white, score_black)
    } else {
        (board.white, board.black, -score_black)
    };
    // Memory layout is file-major; the on-disk convention is rank-major.
    w.write_all(&bitboard::transpose(black).to_le_bytes())?;
    w.write_all(&bitboard::transpose(white).to_le_bytes())?;
    w.write_all(&[(score.clamp(-64, 64) as i8) as u8])?;
    Ok(())
}

fn main() -> ExitCode {
    let mut limit_games: Option<usize> = None;
    let mut skip_games = 0usize;
    let mut skip_plies = 0usize;
    let mut out_dir: Option<PathBuf> = None;
    let mut out_file: Option<PathBuf> = None;
    let mut inputs: Vec<PathBuf> = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
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
            "--skip-plies" => match it.next().and_then(|v| v.parse().ok()) {
                Some(k) => skip_plies = k,
                None => {
                    eprintln!("--skip-plies requires a number");
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
        Some(path) => match File::create(path) {
            Ok(f) => Some(BufWriter::new(f)),
            Err(e) => {
                eprintln!("failed to create {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let mut total_games = 0u64;
    let mut total_positions = 0u64;
    let mut total_corrupt = 0u64;

    for input in &inputs {
        let file = match File::open(input) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("failed to open {}: {e}", input.display());
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
            match File::create(&out_path) {
                Ok(f) => Some((BufWriter::new(f), out_path)),
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

        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            seen += 1;
            if seen <= skip_games {
                continue;
            }
            if let Some(max) = limit_games {
                if games >= max as u64 {
                    break;
                }
            }
            match replay(&line) {
                Some(game) => {
                    let mut out: &mut dyn Write = match (&mut single_out, &mut per_input_out) {
                        (Some(w), _) => w,
                        (None, Some((w, _))) => w,
                        _ => unreachable!(),
                    };
                    for b in game.positions.iter().skip(skip_plies) {
                        if write_record(&mut out, b, game.score_black).is_err() {
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
        if let Some((w, path)) = &mut per_input_out {
            if w.flush().is_err() {
                eprintln!("flush error on {}", path.display());
                return ExitCode::FAILURE;
            }
        }

        total_games += games;
        total_positions += positions;
        total_corrupt += corrupt;
    }

    if let Some(w) = &mut single_out {
        if w.flush().is_err() {
            eprintln!("flush error on --out file");
            return ExitCode::FAILURE;
        }
    }

    println!(
        "total: {total_games} games, {total_positions} positions, {total_corrupt} corrupt (skip_plies {skip_plies})"
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
            assert_eq!(game.positions[0].black, Board::new().black);
            assert_eq!(game.positions[0].white, Board::new().white);
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
    fn test_write_record_normalizes_white_to_move() {
        let mut board = Board::new();
        let pos = Position::from_index(board.movable().trailing_zeros()).unwrap();
        board.make_move_unchecked(pos); // now White to move
        assert_eq!(board.player(), Color::White);

        let mut buf = Vec::new();
        write_record(&mut buf, &board, 10).unwrap();
        assert_eq!(buf.len(), 17);
        let stored_black = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let stored_white = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        // Planes are swapped (White's discs stored as "black" = mover)
        assert_eq!(bitboard::transpose(stored_black), board.white);
        assert_eq!(bitboard::transpose(stored_white), board.black);
        // Score negated to the mover's perspective
        assert_eq!(buf[16] as i8, -10);
    }
}
