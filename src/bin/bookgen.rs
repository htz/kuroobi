//! 定石 book の生成ツール。
//!
//! 2 段階で作る:
//!   1. `--scan`  WTHOR (公式大会棋譜) を読み、序盤の頻出局面を候補として
//!      book に積む (深さ 0 = 未評価、出現回数のみ)
//!   2. `--deepen` 未評価・浅い評価のエントリを**実戦より深い探索**で解く。
//!      途中で止めても保存済みの分は残るので、何度でも継ぎ足せる。
//!
//! book の値は「実戦では届かない深さ」でなければ意味がないので、既定は
//! 深さ 26 / 読切 30 / 帯 8 (実戦の GGS 設定は 22 / 26 / 6)。
//!
//! 使い方:
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
    /// 1 局面あたり採点する人間の候補手の上限。
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

/// WTHOR の 1 ファイルを読み、各局の着手列を返す。
/// 形式: 16 バイトのヘッダ + 68 バイト/局 (先頭 8 バイトがメタ、以降 60 手)。
/// 着手は 10 進で `行*10 + 列` (1 始まり)、0 は終端。
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
            // WTHOR は行優先、当方は file-major (bit = file*8 + rank)
            moves.push((col - 1) * 8 + (row - 1));
        }
        if moves.len() >= 10 {
            games.push(moves);
        }
        off += 68;
    }
    Ok(games)
}

/// 棋譜を並べて、序盤 `max_ply` 手までの局面と「実際に指された手」を数える。
fn scan(dir: &Path, max_ply: usize, min_games: u32, book: &mut Book) -> std::io::Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wtb")))
        .collect();
    files.sort();
    // (正規化キー, 正規化した手) → 出現回数
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
                // パスの明示が無い形式なので、指せなければパスを入れて再挑戦
                if !b.check(pos) {
                    b.pass();
                    if !b.check(pos) {
                        break; // 棋譜が壊れている
                    }
                }
                let (key, i) = Book::key(&b);
                let mapped = Book::map_move(pos, i);
                *counts.entry((key, mapped.index())).or_insert(0) += 1;
                b.make_move_bits(pos);
            }
        }
        eprint!("\r{} を読み込み中… 累計 {total_games} 局", f.display());
    }
    eprintln!();

    // 局面ごとに最頻の手を採る
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
        // 出現頻度の高い順に候補手を全部持つ (深化前は評価値 0)
        cands.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        // 既存エントリ (深い評価済み) は壊さない
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
    eprintln!("局面 {kept} 件を候補として登録 (棋譜 {total_games} 局)");
    Ok(())
}

/// 未評価・浅い評価のエントリを深い探索で解き直す。
///
/// **局面ごとの並列化**: 1 局面を多スレッドで解くより、局面を並べて
/// 各ワーカーが 1 局面ずつ (少スレッドで) 解く方が総スループットが高い。
/// 探索の並列効率は木の大きさで決まるので、序盤の小さい木を 10 並列しても
/// 遊びが出るためである。
fn deepen(book: &mut Book, out: &Path, a: &Args) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    // 出現回数の多い順に解く (実戦で当たりやすい局面から価値が乗る)
    let mut todo: Vec<((u64, u64), Entry)> = book
        .iter()
        .filter(|(_, e)| e.depth < a.depth as u8)
        .map(|(k, e)| (*k, e.clone()))
        .collect();
    todo.sort_by_key(|(_, e)| std::cmp::Reverse(e.games));
    todo.truncate(a.limit);
    let total = todo.len();
    if total == 0 {
        eprintln!("深化対象なし");
        return Ok(());
    }

    // ワーカー数 × 1 ワーカーあたりのスレッド数 ≒ 物理コア数
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);
    let per = a.threads.max(1);
    let workers = (cores / per).max(1).min(total);
    eprintln!(
        "深化対象 {total} 局面 (深さ {} / 読切 {} / 帯 {}) — {workers} ワーカー × {per} スレッド",
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
                    // 序盤の木は小さいので表も小さくする。ソルバは 1 局面ごとに
                    // 表を全消去するため、大きな表は消去コストがそのまま損になる。
                    midgame_hash_bits: a.hash_bits,
                    solver_hash_bits: a.hash_bits,
                    // 定石は引かない。引くと「これから評価する局面」が
                    // 定石に当たり、探索せずに古い値 (未評価なら 0) を
                    // そのまま書き戻してしまう
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
                    // 採点するのは「人間が実際に打った手」+「エンジンの最善手」
                    // だけ。全合法手を回すと 3 倍以上遅くなるうえ、誰も打たない
                    // 手の正確な値は乱択にも棋力にも使わない。
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
                            "{:.1}% ({n}/{}) 経過 {:.1} 分 / 残り 約 {:.1} 分 ({:.0} 局面/分)",
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
    eprintln!("完了: {:.1} 分", t0.elapsed().as_secs_f64() / 60.0);
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
    eprintln!("book: {} 局面 ({})", book.len(), a.out.display());

    if let Some(dir) = &a.scan {
        if let Err(e) = scan(dir, a.max_ply, a.min_games, &mut book) {
            eprintln!("scan failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
        if let Err(e) = book.save(&a.out) {
            eprintln!("save failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
        eprintln!("保存: {} 局面 → {}", book.len(), a.out.display());
    }

    if a.deepen {
        if let Err(e) = deepen(&mut book, &a.out, &a) {
            eprintln!("deepen failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
        eprintln!("保存: {} 局面 → {}", book.len(), a.out.display());
    }
    std::process::ExitCode::SUCCESS
}
