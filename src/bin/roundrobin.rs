//! Round-robin tournament between our engine and external ones.
//!
//! Every engine plays every other over the same set of random openings, with
//! colours swapped so each opening is played twice from both sides. The point
//! is to compare evaluation functions rather than search machinery, so all
//! engines are configured for a plain fixed-depth search: at N plies, a
//! position with N or fewer empties is solved to the end anyway, so the
//! endgame needs no separate setting.
//!
//! Usage:
//!   roundrobin --games <n> --depth <n> [--engine name=protocol=path]...
//!
//! `protocol` is one of `edax`, `zebra`, `egaroucid`, or `ours`.
//! Exactly one `ours` entry is expected; its path is ignored.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio};

use kuroobi::evaluator::Evaluator;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::search::Searcher;
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::{Board, Color, Position};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
}

fn nth_move(mut moves: u64, mut n: u32) -> Position {
    loop {
        let sq = moves.trailing_zeros();
        if n == 0 {
            return Position(sq as u8);
        }
        moves &= moves - 1;
        n -= 1;
    }
}

fn square_name(p: Position) -> String {
    let f = (b'a' + p.index() / 8) as char;
    let r = (b'1' + p.index() % 8) as char;
    format!("{f}{r}")
}

/// An opponent process and the dialect it speaks.
struct Engine {
    name: String,
    protocol: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    history: Vec<String>,
    /// Our own engine keeps its state here instead of in a child process.
    ours: Option<(Searcher, Solver)>,
    depth: u8,
}

impl Engine {
    fn spawn(name: &str, protocol: &str, path: &PathBuf, depth: u8) -> std::io::Result<Engine> {
        if protocol == "ours" {
            let mut searcher = Searcher::new(20);
            searcher.mpc = false; // a plain fixed-depth search, like the others
            return Ok(Engine {
                name: name.into(),
                protocol: protocol.into(),
                child: None,
                stdin: None,
                stdout: None,
                history: Vec::new(),
                ours: Some((searcher, Solver::new(20))),
                depth,
            });
        }

        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let args: Vec<String> = match protocol {
            // midgame depth, then the empty counts for exact and win/loss
            "zebra" => vec![
                depth.to_string(),
                depth.to_string(),
                depth.to_string(),
                "20".to_string(),
            ],
            "egaroucid" => vec![
                "-gtp".into(),
                "-l".into(),
                depth.to_string(),
                "-t".into(),
                "1".into(),
                "-nobook".into(),
                "-q".into(),
            ],
            _ => vec![
                "-book-usage".into(),
                "off".into(),
                "-l".into(),
                depth.to_string(),
                "-n".into(),
                "1".into(),
            ],
        };
        let mut child = Command::new(path)
            .args(args)
            // Edax's level table couples midgame depth to the endgame
            // threshold; this makes it a plain fixed-depth search instead.
            .env("EDAX_FIXED_DEPTH", "1")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Engine {
            name: name.into(),
            protocol: protocol.into(),
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(stdout),
            history: Vec::new(),
            ours: None,
            depth,
        })
    }

    fn send(&mut self, cmd: &str) -> std::io::Result<()> {
        let s = self.stdin.as_mut().unwrap();
        s.write_all(cmd.as_bytes())?;
        s.write_all(b"\n")?;
        s.flush()
    }

    fn read_gtp(&mut self) -> std::io::Result<String> {
        let mut out = String::new();
        let mut saw = false;
        loop {
            let mut line = String::new();
            if self.stdout.as_mut().unwrap().read_line(&mut line)? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "engine exited",
                ));
            }
            let t = line.trim_end();
            if let Some(rest) = t.strip_prefix('=') {
                out = rest.trim().to_string();
                saw = true;
            } else if t.starts_with('?') {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("gtp error: {t}"),
                ));
            } else if t.is_empty() && saw {
                return Ok(out);
            }
        }
    }

    fn parse_square(mv: &str) -> Option<Position> {
        let b = mv.as_bytes();
        if b.len() < 2 {
            return None;
        }
        let file = b[0].to_ascii_uppercase().wrapping_sub(b'A');
        let rank = b[1].wrapping_sub(b'1');
        (file < 8 && rank < 8).then(|| Position::from_file_rank(file, rank))?
    }

    fn best_move(
        &mut self,
        board: &Board,
        evaluator: &Evaluator,
    ) -> Result<Option<Position>, String> {
        let proto = self.protocol.clone();
        match proto.as_str() {
            "ours" => {
                let depth = self.depth;
                let (searcher, solver) = self.ours.as_mut().unwrap();
                // At `depth` plies a position with that many empties is
                // already solved to the end, so this is the same search.
                if board.empty_count() <= depth {
                    Ok(solver
                        .solve_with_eval(EndSolverMode::Perfect, board, Some(evaluator))
                        .best_move)
                } else {
                    Ok(searcher.search(board, evaluator, depth).best_move)
                }
            }
            "egaroucid" => {
                self.send("clear_board").map_err(|e| e.to_string())?;
                self.read_gtp().map_err(|e| e.to_string())?;
                let hist = std::mem::take(&mut self.history);
                for (i, mv) in hist.iter().enumerate() {
                    let c = if i % 2 == 0 { "B" } else { "W" };
                    self.send(&format!("play {c} {mv}"))
                        .map_err(|e| e.to_string())?;
                    self.read_gtp().map_err(|e| e.to_string())?;
                }
                self.history = hist;
                let c = if board.player() == Color::Black {
                    "B"
                } else {
                    "W"
                };
                self.send(&format!("genmove {c}"))
                    .map_err(|e| e.to_string())?;
                let reply = self.read_gtp().map_err(|e| e.to_string())?;
                if reply.trim().eq_ignore_ascii_case("pass") {
                    return Ok(None);
                }
                Self::parse_square(reply.trim())
                    .map(Some)
                    .ok_or_else(|| format!("bad gtp move {reply}"))
            }
            proto => {
                let proto = proto.to_string();
                self.send(&format!("setboard {board}"))
                    .map_err(|e| e.to_string())?;
                self.send("go").map_err(|e| e.to_string())?;
                loop {
                    let mut line = String::new();
                    if self
                        .stdout
                        .as_mut()
                        .unwrap()
                        .read_line(&mut line)
                        .map_err(|e| e.to_string())?
                        == 0
                    {
                        return Err(format!("{} exited", self.name));
                    }
                    let found = if proto == "zebra" {
                        line.strip_prefix("move ").map(|m| m.trim().to_string())
                    } else {
                        line.find("plays").map(|i| line[i + 5..].trim().to_string())
                    };
                    if let Some(mv) = found {
                        if mv.eq_ignore_ascii_case("PS") || mv.eq_ignore_ascii_case("pass") {
                            return Ok(None);
                        }
                        return Self::parse_square(&mv)
                            .map(Some)
                            .ok_or_else(|| format!("bad move from {}: {line}", self.name));
                    }
                }
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = self.send("quit");
            let _ = self.child.as_mut().unwrap().wait();
        }
    }
}

/// Disc difference from the first engine's point of view.
fn play(
    a: &mut Engine,
    b: &mut Engine,
    start: &Board,
    opening: &[String],
    a_is_black: bool,
    evaluator: &Evaluator,
) -> Result<i32, String> {
    let mut board = *start;
    for e in [&mut *a, &mut *b] {
        e.history = opening.to_vec();
        if let Some((s, _)) = e.ours.as_mut() {
            s.clear();
        }
    }

    loop {
        if board.movable() == 0 {
            let mut passed = board;
            passed.pass();
            if passed.movable() == 0 {
                break;
            }
            for e in [&mut *a, &mut *b] {
                e.history.push("pass".into());
            }
            board = passed;
            continue;
        }
        let a_turn = (board.player() == Color::Black) == a_is_black;
        let pos = {
            let e: &mut Engine = if a_turn { a } else { b };
            e.best_move(&board, evaluator)?
                .ok_or_else(|| format!("{} passed with moves available", e.name))?
        };
        if board.movable() & pos.to_bit() == 0 {
            return Err(format!("illegal move {pos:?}"));
        }
        let name = square_name(pos);
        for e in [&mut *a, &mut *b] {
            e.history.push(name.clone());
        }
        board.make_move_bits(pos);
    }

    let diff = board.black_count() as i32 - board.white_count() as i32;
    let empties = board.empty_count() as i32;
    let black_score = match diff.cmp(&0) {
        std::cmp::Ordering::Greater => diff + empties,
        std::cmp::Ordering::Less => diff - empties,
        std::cmp::Ordering::Equal => 0,
    };
    Ok(if a_is_black {
        black_score
    } else {
        -black_score
    })
}

fn main() -> ExitCode {
    let mut games = 20usize;
    let mut depth = 8u8;
    let mut seed = 1234u64;
    let mut plies = 6usize;
    let mut specs: Vec<(String, String, PathBuf)> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--games" => games = it.next().unwrap().parse().unwrap(),
            "--depth" => depth = it.next().unwrap().parse().unwrap(),
            "--seed" => seed = it.next().unwrap().parse().unwrap(),
            "--random-plies" => plies = it.next().unwrap().parse().unwrap(),
            "--engine" => {
                let v = it.next().unwrap();
                let p: Vec<&str> = v.splitn(3, '=').collect();
                if p.len() != 3 {
                    eprintln!("--engine wants name=protocol=path");
                    return ExitCode::FAILURE;
                }
                specs.push((p[0].into(), p[1].into(), PathBuf::from(p[2])));
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    if specs.len() < 2 {
        eprintln!("need at least two --engine entries");
        return ExitCode::FAILURE;
    }

    let mut evaluator = Evaluator::new(EGAROUCID_PATTERNS);
    if let Err(e) = evaluator.load_weights(std::path::Path::new("weights/linear.bin")) {
        eprintln!("failed to load weights: {e}");
        return ExitCode::FAILURE;
    }

    let n = specs.len();
    println!(
        "round robin: {} engines, {games} games per pairing, {depth}-ply fixed depth",
        n
    );
    for (name, proto, _) in &specs {
        println!("  {name} ({proto})");
    }

    // score[i][j] = points engine i took from engine j (win 1, draw 0.5)
    let mut score = vec![vec![0.0f64; n]; n];
    let mut wins = vec![vec![(0usize, 0usize, 0usize); n]; n];

    for i in 0..n {
        for j in (i + 1)..n {
            let mut ea = match Engine::spawn(&specs[i].0, &specs[i].1, &specs[i].2, depth) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("spawn {}: {e}", specs[i].0);
                    return ExitCode::FAILURE;
                }
            };
            let mut eb = match Engine::spawn(&specs[j].0, &specs[j].1, &specs[j].2, depth) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("spawn {}: {e}", specs[j].0);
                    return ExitCode::FAILURE;
                }
            };
            let mut rng = Rng(seed);
            let (mut w, mut l, mut d) = (0usize, 0usize, 0usize);
            for _ in 0..games / 2 {
                // One opening, played from both sides, so the pair is fair.
                let mut board = Board::new();
                let mut moves = Vec::new();
                for _ in 0..plies {
                    let m = board.movable();
                    if m == 0 {
                        break;
                    }
                    let p = nth_move(m, rng.below(m.count_ones()));
                    moves.push(square_name(p));
                    board.make_move_bits(p);
                }
                for a_black in [true, false] {
                    match play(&mut ea, &mut eb, &board, &moves, a_black, &evaluator) {
                        Ok(s) => match s.cmp(&0) {
                            std::cmp::Ordering::Greater => w += 1,
                            std::cmp::Ordering::Less => l += 1,
                            std::cmp::Ordering::Equal => d += 1,
                        },
                        Err(e) => {
                            eprintln!("game {} vs {}: {e}", specs[i].0, specs[j].0);
                            return ExitCode::FAILURE;
                        }
                    }
                }
            }
            score[i][j] = w as f64 + d as f64 / 2.0;
            score[j][i] = l as f64 + d as f64 / 2.0;
            wins[i][j] = (w, l, d);
            wins[j][i] = (l, w, d);
            println!(
                "{:>12} vs {:<12} {w}-{l}-{d}  ({:.1}%)",
                specs[i].0,
                specs[j].0,
                100.0 * (w as f64 + d as f64 / 2.0) / (w + l + d) as f64
            );
        }
    }

    println!("\n=== standings ===");
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        score[b]
            .iter()
            .sum::<f64>()
            .partial_cmp(&score[a].iter().sum::<f64>())
            .unwrap()
    });
    for &i in &order {
        let pts: f64 = score[i].iter().sum();
        let played = (n - 1) * games;
        println!(
            "{:>12}  {:.1} / {}  ({:.1}%)",
            specs[i].0,
            pts,
            played,
            100.0 * pts / played as f64
        );
    }
    ExitCode::SUCCESS
}
