//! Opening-book generator.

use std::path::{Path, PathBuf};

use kuroobi::book::{Book, Candidate, Entry};
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{wthor, Board, Position};

struct Args {
    scan: Option<PathBuf>,
    deepen: bool,
    out: PathBuf,
    max_ply: usize,
    min_games: u32,
    depth: u32,
    solve: u8,
    band: u8,
    threads: usize,
    limit: usize,
    hash_bits: u32,
    max_cands: usize,
    all_moves: bool,
    min_empties: u8,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        scan: None,
        deepen: false,
        out: PathBuf::from("weights/book.txt"),
        max_ply: 24,
        min_games: 3,
        depth: 26,
        solve: 30,
        band: 8,
        threads: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8),
        limit: usize::MAX,
        hash_bits: 19,
        max_cands: 4,
        all_moves: false,
        min_empties: 0,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let val = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("missing value for {}", argv[*i - 1]))
        };
        match argv[i].as_str() {
            "--scan" => a.scan = Some(PathBuf::from(val(&mut i)?)),
            "--deepen" => a.deepen = true,
            "--out" | "--book" => a.out = PathBuf::from(val(&mut i)?),
            "--max-ply" => a.max_ply = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--min-games" => a.min_games = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--depth" => a.depth = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--solve" => a.solve = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--band" => a.band = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--threads" => a.threads = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--limit" => a.limit = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--hash-bits" => a.hash_bits = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--max-cands" => a.max_cands = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            "--all-moves" => a.all_moves = true,
            "--min-empties" => a.min_empties = val(&mut i)?.parse().map_err(|e| format!("{e}"))?,
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    Ok(a)
}

fn scan(dir: &Path, max_ply: usize, min_games: u32, book: &mut Book) -> std::io::Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wtb")))
        .collect();
    files.sort();
    let mut counts: std::collections::HashMap<((u64, u64), u8), u32> = Default::default();
    let mut total_games = 0usize;
    for f in &files {
        let games = wthor::read(f)?.into_iter().filter(|g| g.moves.len() >= 10);
        for game in games {
            total_games += 1;
            let mut b = Board::new();
            for (ply, &pos) in game.moves.iter().enumerate() {
                if ply >= max_ply {
                    break;
                }
                if !b.check(pos) {
                    b.pass();
                    if !b.check(pos) {
                        break; // corrupt record
                    }
                }
                let (key, i) = Book::key(&b);
                let mapped = Book::map_move(pos, i);
                *counts.entry((key, mapped.index())).or_insert(0) += 1;
                b.make_move_bits(pos);
            }
        }
        eprint!("\rreading {}... {total_games} games so far", f.display());
    }
    eprintln!();

    let mut by_pos: std::collections::HashMap<(u64, u64), Vec<(u8, u32)>> = Default::default();
    for ((key, mv), n) in counts {
        by_pos.entry(key).or_default().push((mv, n));
    }
    let mut kept = 0usize;
    for (key, mut cands) in by_pos {
        let total: u32 = cands.iter().map(|(_, n)| *n).sum();
        if total < min_games {
            continue;
        }
        cands.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        if book.get_raw(key).is_some_and(|e| e.depth > 0) {
            continue;
        }
        let moves: Vec<Candidate> = cands
            .iter()
            .filter_map(|(mv, n)| {
                Position::from_index(*mv as u32).map(|p| Candidate {
                    mv: p,
                    value: 0.0,
                    games: *n,
                })
            })
            .collect();
        if moves.is_empty() {
            continue;
        }
        book.insert_raw(
            key,
            Entry {
                moves,
                depth: 0,
                games: total,
                complete: false,
            },
        );
        kept += 1;
    }
    eprintln!("registered {kept} positions as candidates (from {total_games} games)");
    Ok(())
}

fn deepen(book: &mut Book, out: &Path, a: &Args) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    if a.all_moves {
        let ready: Vec<(u64, u64)> = book
            .iter()
            .filter(|(k, e)| {
                if e.complete || e.depth == 0 {
                    return false;
                }
                let b = kuroobi::book::board_from_key(**k);
                let scored = e.moves.iter().filter(|c| b.check(c.mv)).count();
                scored >= b.movable_iter().count()
            })
            .map(|(k, _)| *k)
            .collect();
        let n = ready.len();
        for k in ready {
            if let Some(e) = book.get_raw_mut(k) {
                e.complete = true;
            }
        }
        if n > 0 {
            eprintln!("{n} entries already cover every legal move - complete without searching");
        }
    }
    let mut todo: Vec<((u64, u64), Entry)> = book
        .iter()
        .filter(|(k, e)| {
            if a.min_empties > 0 && kuroobi::book::board_from_key(**k).empty_count() < a.min_empties
            {
                return false;
            }
            e.depth < a.depth as u8 || (a.all_moves && !e.complete)
        })
        .map(|(k, e)| (*k, e.clone()))
        .collect();
    todo.sort_by_key(|(_, e)| std::cmp::Reverse(e.games));
    todo.truncate(a.limit);
    let total = todo.len();
    if total == 0 {
        eprintln!("nothing to deepen");
        return book.save(out).map_err(|e| format!("{e}"));
    }

    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);
    let per = a.threads.max(1);
    let workers = (cores / per).max(1).min(total);
    eprintln!(
        "deepening {total} positions (depth {} / solve {} / band {}) - {workers} workers x {per} threads",
        a.depth, a.solve, a.band
    );

    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<((u64, u64), Entry)>> = Mutex::new(Vec::new());
    let t0 = std::time::Instant::now();
    let todo_ref = &todo;

    std::thread::scope(|scope| -> Result<(), String> {
        let mut handles = Vec::new();
        for _ in 0..workers {
            let next = &next;
            let done = &done;
            let results = &results;
            handles.push(scope.spawn(move || -> Result<(), String> {
                let cfg = EngineConfig {
                    depth: a.depth,
                    solve_empties: a.solve,
                    band: a.band,
                    threads: per,
                    midgame_hash_bits: a.hash_bits,
                    solver_hash_bits: a.hash_bits,
                    use_book: false,
                    ..Default::default()
                };
                let mut engine = Engine::new(cfg)?;
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= todo_ref.len() {
                        return Ok(());
                    }
                    let (key, old) = &todo_ref[i];
                    let key = *key;
                    let board = kuroobi::book::board_from_key(key);
                    let best = engine.choose(&board);
                    let mut cands: Vec<(kuroobi::Position, u32)> = if a.all_moves {
                        board
                            .movable_iter()
                            .map(|p| {
                                let games = old
                                    .moves
                                    .iter()
                                    .find(|c| c.mv == p)
                                    .map(|c| c.games)
                                    .unwrap_or(0);
                                (p, games)
                            })
                            .collect()
                    } else {
                        old.moves
                            .iter()
                            .filter(|c| board.check(c.mv))
                            .map(|c| (c.mv, c.games))
                            .take(a.max_cands)
                            .collect()
                    };
                    if let Some(bp) = best.pos {
                        if !cands.iter().any(|(p, _)| *p == bp) {
                            cands.push((bp, 0));
                        }
                    }
                    let mut moves: Vec<Candidate> = Vec::new();
                    for (p, games) in cands {
                        let value = if Some(p) == best.pos {
                            best.value
                        } else {
                            let mut child = board;
                            child.make_move_bits(p);
                            -engine
                                .eval_position(&child, a.depth.saturating_sub(1))
                                .value
                        };
                        moves.push(Candidate {
                            mv: p,
                            value,
                            games,
                        });
                    }
                    if !moves.is_empty() {
                        moves.sort_by(|x, y| y.value.total_cmp(&x.value));
                        results.lock().unwrap().push((
                            key,
                            Entry {
                                moves,
                                complete: a.all_moves,
                                depth: a.depth as u8,
                                games: old.games,
                            },
                        ));
                    }
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(20) || n == todo_ref.len() {
                        let el = t0.elapsed().as_secs_f64();
                        let rate = n as f64 / el.max(0.001);
                        let remain = (todo_ref.len() - n) as f64 / rate.max(1e-9);
                        eprintln!(
                            "{:.1}% ({n}/{}) elapsed {:.1} min / about {:.1} min left ({:.0} positions/min)",
                            100.0 * n as f64 / todo_ref.len() as f64,
                            todo_ref.len(),
                            el / 60.0,
                            remain / 60.0,
                            rate * 60.0,
                        );
                    }
                }
            }));
        }
        for h in handles {
            h.join().map_err(|_| "worker panicked".to_string())??;
        }
        Ok(())
    })?;

    for (key, e) in results.into_inner().unwrap() {
        book.insert_raw(key, e);
    }
    book.save(out).map_err(|e| e.to_string())?;
    eprintln!("done: {:.1} min", t0.elapsed().as_secs_f64() / 60.0);
    Ok(())
}

fn main() -> std::process::ExitCode {
    let a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut book = Book::load(&a.out).unwrap_or_else(|_| Book::new());
    eprintln!("book: {} positions ({})", book.len(), a.out.display());

    if let Some(dir) = &a.scan {
        if let Err(e) = scan(dir, a.max_ply, a.min_games, &mut book) {
            eprintln!("scan failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
        if let Err(e) = book.save(&a.out) {
            eprintln!("save failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
        eprintln!("saved: {} positions -> {}", book.len(), a.out.display());
    }

    if a.deepen {
        if let Err(e) = deepen(&mut book, &a.out, &a) {
            eprintln!("deepen failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
        eprintln!("saved: {} positions -> {}", book.len(), a.out.display());
    }
    std::process::ExitCode::SUCCESS
}
