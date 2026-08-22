//! Opening-book generator.
//!
//! Two stages:
//!   1. `--scan`   read WTHOR (tournament records), collect frequent
//!      opening positions as candidates (depth 0 = unevaluated)
//!   2. `--deepen` solve unevaluated/shallow entries with
//!      deeper-than-game search; interruptible, saved work persists.
//!
//! Book values must come from beyond game depth, so defaults are depth
//! 26 / solve 30 / band 8 (live GGS runs 22 / 26 / 6).
//!
//! Usage:
//!   bookgen --scan train_data/wthor --max-ply 24 --min-games 3 --out book.txt
//!   bookgen --deepen book.txt --depth 26 --solve 30 --band 8 [--limit 500]

use std::path::{Path, PathBuf};

use kuroobi::book::{Book, Candidate, Entry};
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Position};

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
    /// Max human candidate moves scored per position.
    max_cands: usize,
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
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    Ok(a)
}

/// Read one WTHOR file, returning each game's move list.
/// Format: 16-byte header + 68 bytes/game (8 meta + 60 moves); moves
/// are decimal `row*10 + col` (1-based), 0 terminates.
fn read_wtb(path: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    let data = std::fs::read(path)?;
    if data.len() < 16 {
        return Ok(Vec::new());
    }
    let mut games = Vec::new();
    let mut off = 16;
    while off + 68 <= data.len() {
        let rec = &data[off..off + 68];
        let mut moves = Vec::new();
        for &v in &rec[8..68] {
            if v == 0 {
                break;
            }
            let row = v / 10;
            let col = v % 10;
            if !(1..=8).contains(&row) || !(1..=8).contains(&col) {
                moves.clear();
                break;
            }
            // WTHOR is row-major; we are file-major (bit = file*8 + rank).
            moves.push((col - 1) * 8 + (row - 1));
        }
        if moves.len() >= 10 {
            games.push(moves);
        }
        off += 68;
    }
    Ok(games)
}

/// Replay records, counting positions and played moves up to `max_ply`.
fn scan(dir: &Path, max_ply: usize, min_games: u32, book: &mut Book) -> std::io::Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wtb")))
        .collect();
    files.sort();
    // (normalized key, normalized move) -> occurrence count.
    let mut counts: std::collections::HashMap<((u64, u64), u8), u32> = Default::default();
    let mut total_games = 0usize;
    for f in &files {
        let games = read_wtb(f)?;
        total_games += games.len();
        for moves in games {
            let mut b = Board::new();
            for (ply, &sq) in moves.iter().enumerate() {
                if ply >= max_ply {
                    break;
                }
                let Some(pos) = Position::from_index(sq as u32) else {
                    break;
                };
                // The format has no explicit pass; insert one and retry.
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

    // Take the most frequent move per position.
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
        // Keep all candidates by frequency (value 0 before deepening).
        cands.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        // Never clobber an existing (deep-evaluated) entry.
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
            },
        );
        kept += 1;
    }
    eprintln!("registered {kept} positions as candidates (from {total_games} games)");
    Ok(())
}

/// Re-solve unevaluated/shallow entries with deep search.
///
/// Parallelism is per position: many workers each solving one position
/// with few threads out-throughputs one many-threaded solve, because
/// parallel efficiency scales with tree size and opening trees are small.
fn deepen(book: &mut Book, out: &Path, a: &Args) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    // Solve by frequency (likely-hit positions gain value first).
    let mut todo: Vec<((u64, u64), Entry)> = book
        .iter()
        .filter(|(_, e)| e.depth < a.depth as u8)
        .map(|(k, e)| (*k, e.clone()))
        .collect();
    todo.sort_by_key(|(_, e)| std::cmp::Reverse(e.games));
    todo.truncate(a.limit);
    let total = todo.len();
    if total == 0 {
        eprintln!("nothing to deepen");
        return Ok(());
    }

    // workers x threads-per-worker ~= physical cores.
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
                    // Small trees, small tables: the solver clears per
                    // position, and a big table pays its clear cost every time.
                    midgame_hash_bits: a.hash_bits,
                    solver_hash_bits: a.hash_bits,
                    // Book off: the position being evaluated would hit
                    // the book and write its stale value straight back.
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
                    // Score only human-played moves plus the engine best:
                    // all legal moves is 3x slower and nobody uses exact
                    // values of never-played moves.
                    let best = engine.choose(&board);
                    let mut cands: Vec<(kuroobi::Position, u32)> = old
                        .moves
                        .iter()
                        .filter(|c| board.check(c.mv))
                        .map(|c| (c.mv, c.games))
                        .take(a.max_cands)
                        .collect();
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
