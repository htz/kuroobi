//! Self-play training data: one record per searched position (`kuroobi::record`,
//! 24 bytes, `.rec`) plus one transcript line per game. Random opening moves are
//! not searched and not recorded; the transcript keeps them. Just above the solve,
//! a selective endgame solve replaces the midgame search (`SELECTIVE_T`).
//! With `--starts`, each game starts from one position of the given records instead:
//! no random moves, every start played once, in an order shuffled by the seed.
//!
//! Usage: gendata [--out-dir DIR] [--jobs N] [--threads 1] [--games N] [--positions N]
//!                [--random-plies 12,14,16] [--depth 16] [--no-mpc] [--solve-empties 20]
//!                [--selective 21:1.1,22:0.8,23:0.6,24:0.4|none]
//!                [--hash-mid 22] [--hash-end 22] [--weights PATH] [--nnue PATH]
//!                [--shard-size N] [--seed N] [--starts FILE.rec]... [--starts-ply N]

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use kuroobi::engine::{Engine, EngineAssets, EngineConfig};
use kuroobi::record::{Record, ScoreKind, SIZE as RECORD};
use kuroobi::{Board, Color, Position};

struct Args {
    out_dir: PathBuf,
    jobs: usize,
    threads: usize,
    positions: u64,
    games: u64,
    random_plies: Vec<u32>,
    depth: u32,
    mpc: bool,
    solve_empties: u8,
    /// (empties, t) pairs; empties at or below `solve_empties` stay exact.
    selective: Vec<(u32, f32)>,
    hash_mid: u32,
    hash_end: u32,
    weights: PathBuf,
    nnue: PathBuf,
    shard_size: usize,
    seed: u64,
    starts: Vec<PathBuf>,
    starts_ply: Option<u8>,
}

fn parse_args() -> Args {
    parse_args_from(std::env::args().skip(1).collect())
}

fn parse_args_from(args: Vec<String>) -> Args {
    let mut a = Args {
        out_dir: PathBuf::from("out"),
        jobs: std::thread::available_parallelism().map_or(4, |n| n.get()),
        threads: 1,
        positions: 0,
        games: 0,
        // Below 12 the openings repeat: 4 plies reach 236 positions, 12 reach 994k.
        random_plies: vec![12, 14, 16],
        depth: 16,
        mpc: true,
        solve_empties: 20,
        selective: SELECTIVE_T.to_vec(),
        hash_mid: 22,
        hash_end: 22,
        weights: EngineConfig::default().weights,
        nnue: EngineConfig::default().nnue,
        shard_size: 100_000,
        seed: 0,
        starts: Vec::new(),
        starts_ply: None,
    };
    let mut it = args.into_iter();
    while let Some(f) = it.next() {
        let mut val = || it.next().expect("missing value");
        match f.as_str() {
            "--out-dir" => a.out_dir = PathBuf::from(val()),
            "--jobs" => a.jobs = val().parse().expect("--jobs"),
            "--threads" => a.threads = val().parse().expect("--threads"),
            "--positions" => a.positions = val().parse().expect("--positions"),
            "--games" => a.games = val().parse().expect("--games"),
            "--random-plies" => {
                a.random_plies = val()
                    .split(',')
                    .map(|s| s.trim().parse().expect("--random-plies"))
                    .collect()
            }
            "--depth" => a.depth = val().parse().expect("--depth"),
            "--no-mpc" => a.mpc = false,
            "--solve-empties" => a.solve_empties = val().parse().expect("--solve-empties"),
            "--selective" => a.selective = parse_selective(&val()),
            "--hash-mid" => a.hash_mid = val().parse().expect("--hash-mid"),
            "--hash-end" => a.hash_end = val().parse().expect("--hash-end"),
            "--weights" => a.weights = PathBuf::from(val()),
            "--nnue" => a.nnue = PathBuf::from(val()),
            "--shard-size" => a.shard_size = val().parse().expect("--shard-size"),
            "--seed" => a.seed = val().parse().expect("--seed"),
            "--starts" => a.starts.push(PathBuf::from(val())),
            "--starts-ply" => a.starts_ply = Some(val().parse().expect("--starts-ply")),
            other => panic!("unknown flag {other}"),
        }
    }
    if a.seed == 0 {
        use std::hash::{BuildHasher, Hasher};
        a.seed = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
            | 1;
    }
    assert!(!a.random_plies.is_empty(), "--random-plies is empty");
    assert!(
        a.jobs > 0 && a.threads > 0,
        "--jobs and --threads must be at least 1"
    );
    a
}

/// `21:1.1,22:0.8`, or `none` for the midgame search everywhere above the solve.
fn parse_selective(v: &str) -> Vec<(u32, f32)> {
    if v == "none" {
        return Vec::new();
    }
    v.split(',')
        .map(|p| {
            let (e, t) = p.trim().split_once(':').expect("--selective: empties:t");
            let t: f32 = t.parse().expect("--selective: t");
            assert!(t > 0.0 && t < 6.3, "--selective: t outside 0..6.3");
            (e.parse().expect("--selective: empties"), t)
        })
        .collect()
}

/// Per empties, the most accurate t no slower than the depth-16 midgame search: on 40 self-play
/// positions each, mean error against exact solves 1.31 -> 0.69 discs at 238 against 259 ms
/// over 21-24 empties; at 25 the t no slower erred as much (1.40 vs 1.45), at 26 every t was slower.
const SELECTIVE_T: [(u32, f32); 4] = [(21, 1.1), (22, 0.8), (23, 0.6), (24, 0.4)];

fn selective_t(a: &Args, empties: u32) -> Option<f32> {
    if empties <= u32::from(a.solve_empties) {
        return None;
    }
    a.selective
        .iter()
        .find(|&&(e, _)| e == empties)
        .map(|&(_, t)| t)
}

/// `ScoreKind::Selective`'s depth field.
fn selective_depth(t: f32) -> u8 {
    (t * 10.0).round() as u8
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn pick(&mut self, moves: u64) -> Position {
        let k = (self.next() % moves.count_ones() as u64) as u32;
        let mut m = moves;
        for _ in 0..k {
            m &= m - 1;
        }
        Position(m.trailing_zeros() as u8)
    }
}

type StartKey = (u64, u64, bool);

fn start_key(b: &Board) -> StartKey {
    (b.black, b.white, b.player() == Color::Black)
}

/// The distinct positions of the `--starts` records (those at `--starts-ply`, when given) that no
/// transcript in `out_dir` has played yet, shuffled by the seed.
fn load_starts(a: &Args) -> std::io::Result<Vec<Board>> {
    let mut seen = played_starts(&a.out_dir)?;
    let mut starts = Vec::new();
    for path in &a.starts {
        kuroobi::record::for_each(path, |r| {
            if a.starts_ply.is_none_or(|p| p == r.ply) {
                let (black, white) = if r.black_to_move {
                    (r.mover, r.opponent)
                } else {
                    (r.opponent, r.mover)
                };
                let b = Board {
                    black,
                    white,
                    player: if r.black_to_move {
                        Color::Black
                    } else {
                        Color::White
                    },
                    empty_count: 64 - (black | white).count_ones() as u8,
                };
                if seen.insert(start_key(&b)) {
                    starts.push(b);
                }
            }
            true
        })?;
    }
    let mut rng = Rng(a.seed);
    for i in (1..starts.len()).rev() {
        starts.swap(i, (rng.next() % (i as u64 + 1)) as usize);
    }
    Ok(starts)
}

/// A game from a start writes its board first: `<64 squares> <X|O> <moves> <final> 0`.
fn played_starts(dir: &Path) -> std::io::Result<HashSet<StartKey>> {
    let mut played = HashSet::new();
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        if !name.starts_with("games_") || !name.ends_with(".txt") {
            continue;
        }
        for line in std::fs::read_to_string(&p)?.lines() {
            let f: Vec<&str> = line.split(' ').collect();
            if f.len() == 5 {
                if let Ok(b) = Board::from_string(&format!("{} {}", f[0], f[1])) {
                    played.insert(start_key(&b));
                }
            }
        }
    }
    Ok(played)
}

/// Written as `.part`, renamed to `.rec` when full. Numbering continues after
/// whatever the directory already holds, so a restart never overwrites.
struct Shard {
    dir: PathBuf,
    worker: usize,
    index: usize,
    file: Option<std::fs::File>,
    written: usize,
    cap: usize,
}

impl Shard {
    fn new(dir: PathBuf, worker: usize, cap: usize) -> Shard {
        let prefix = format!("shard_{worker:02}_");
        let index = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.strip_prefix(&prefix)?.get(..4)?.parse::<usize>().ok()
            })
            .max()
            .map_or(0, |i| i + 1);
        Shard {
            dir,
            worker,
            index,
            file: None,
            written: 0,
            cap,
        }
    }

    fn path(&self, ext: &str) -> PathBuf {
        self.dir
            .join(format!("shard_{:02}_{:04}.{ext}", self.worker, self.index))
    }

    fn push(&mut self, rec: &[u8; RECORD]) -> std::io::Result<()> {
        if self.file.is_none() {
            self.file = Some(std::fs::File::create(self.path("part"))?);
            self.written = 0;
        }
        self.file.as_mut().unwrap().write_all(rec)?;
        self.written += 1;
        if self.written >= self.cap {
            self.close()?;
        }
        Ok(())
    }

    fn close(&mut self) -> std::io::Result<()> {
        if let Some(f) = self.file.take() {
            drop(f);
            std::fs::rename(self.path("part"), self.path("rec"))?;
            self.index += 1;
        }
        Ok(())
    }
}

/// A `.part` holds whole records only, so one left by a kill is kept as data.
fn adopt_leftovers(dir: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.extension().and_then(|s| s.to_str()) != Some("part") {
            continue;
        }
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let mut target = dir.join(format!("{stem}.rec"));
        let mut bump = 0;
        while target.exists() {
            bump += 1;
            target = dir.join(format!("{stem}_prev{bump}.rec"));
        }
        std::fs::rename(&p, &target)?;
        n += 1;
    }
    Ok(n)
}

fn main() {
    let a = parse_args();
    std::fs::create_dir_all(&a.out_dir).expect("create out dir");
    match adopt_leftovers(&a.out_dir) {
        Ok(0) => {}
        Ok(n) => eprintln!("gendata: kept {n} unfinished shard(s) from a previous run"),
        Err(e) => {
            eprintln!("gendata: cannot tidy {}: {e}", a.out_dir.display());
            return;
        }
    }
    eprintln!(
        "gendata: {} jobs x {} threads, depth {}{} solve {}, selective {:?}, random plies {:?}\n         \
         hash mid {} end {}, weights {} nnue {} -> {}\n         seed {}",
        a.jobs,
        a.threads,
        a.depth,
        if a.mpc { "" } else { " (no MPC)" },
        a.solve_empties,
        a.selective,
        a.random_plies,
        a.hash_mid,
        a.hash_end,
        a.weights.display(),
        a.nnue.display(),
        a.out_dir.display(),
        a.seed
    );
    let starts = if a.starts.is_empty() {
        None
    } else {
        match load_starts(&a) {
            Ok(s) => {
                eprintln!("gendata: {} starts left to play", s.len());
                Some(s)
            }
            Err(e) => {
                eprintln!("gendata: cannot read the starts: {e}");
                return;
            }
        }
    };
    let next_start = AtomicUsize::new(0);
    // One copy of the weights for every job; each is over a gigabyte.
    let assets = match EngineAssets::load(&config(&a)) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("gendata: {e}");
            return;
        }
    };
    let stop = AtomicBool::new(false);
    let positions = AtomicU64::new(0);
    let games = AtomicU64::new(0);
    let running = AtomicU64::new(a.jobs as u64);
    let t0 = std::time::Instant::now();
    std::thread::scope(|scope| {
        for w in 0..a.jobs {
            let (a, assets, stop, positions, games, running) =
                (&a, &assets, &stop, &positions, &games, &running);
            let starts = starts.as_deref().map(|s| (s, &next_start));
            scope.spawn(move || {
                worker(w, a, assets, stop, positions, games, starts);
                if running.fetch_sub(1, Ordering::Relaxed) == 1 {
                    stop.store(true, Ordering::Relaxed);
                }
            });
        }
        let mut ticks = 0u32;
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(200));
            ticks += 1;
            if !ticks.is_multiple_of(50) {
                continue;
            }
            let p = positions.load(Ordering::Relaxed);
            let el = t0.elapsed().as_secs_f64();
            eprintln!(
                "  {p} positions, {} games, {:.0}/s, {:.1} min elapsed",
                games.load(Ordering::Relaxed),
                p as f64 / el,
                el / 60.0
            );
        }
    });
    eprintln!(
        "done: {} positions from {} games in {:.1} min",
        positions.load(Ordering::Relaxed),
        games.load(Ordering::Relaxed),
        t0.elapsed().as_secs_f64() / 60.0
    );
}

fn config(a: &Args) -> EngineConfig {
    EngineConfig {
        depth: a.depth,
        solve_empties: a.solve_empties,
        threads: a.threads,
        mpc: a.mpc,
        midgame_hash_bits: a.hash_mid,
        solver_hash_bits: a.hash_end,
        weights: a.weights.clone(),
        nnue: a.nnue.clone(),
        use_book: false,
        ..Default::default()
    }
}

fn worker(
    w: usize,
    a: &Args,
    assets: &EngineAssets,
    stop: &AtomicBool,
    positions: &AtomicU64,
    games: &AtomicU64,
    starts: Option<(&[Board], &AtomicUsize)>,
) {
    let mut engine = match Engine::with_assets(assets.clone(), config(a)) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("worker {w}: {e}");
            stop.store(true, Ordering::Relaxed);
            return;
        }
    };
    let mut rng = Rng(a.seed ^ (w as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut shard = Shard::new(a.out_dir.clone(), w, a.shard_size);
    let mut rot = w % a.random_plies.len();
    let games_path = a.out_dir.join(format!("games_{w:02}.txt"));
    let mut games_out = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&games_path)
    {
        Ok(f) => std::io::BufWriter::new(f),
        Err(e) => {
            eprintln!("worker {w}: cannot open {}: {e}", games_path.display());
            stop.store(true, Ordering::Relaxed);
            return;
        }
    };
    let mut searched: Vec<(Board, f32, ScoreKind, u8, Position)> = Vec::with_capacity(64);
    while !stop.load(Ordering::Relaxed)
        && (a.positions == 0 || positions.load(Ordering::Relaxed) < a.positions)
        && (a.games == 0 || games.load(Ordering::Relaxed) < a.games)
    {
        let start = match starts {
            Some((list, next)) => match list.get(next.fetch_add(1, Ordering::Relaxed)) {
                Some(&b) => Some(b),
                None => break,
            },
            None => None,
        };
        let n_random = if start.is_some() {
            0
        } else {
            let n = a.random_plies[rot];
            rot = (rot + 1) % a.random_plies.len();
            n
        };
        searched.clear();
        let mut moves = String::new();
        let mut b = start.unwrap_or_default();
        let mut ply = 0u32;
        engine.clear_tables();
        loop {
            if b.movable() == 0 {
                b.pass();
                if b.movable() == 0 {
                    break;
                }
            }
            let mv = if ply < n_random {
                rng.pick(b.movable())
            } else if let Some(t) = selective_t(a, b.empty_count() as u32) {
                let ev = engine.solve_selective(&b, t);
                let Some(best) = ev.pos else { break };
                searched.push((b, ev.value, ScoreKind::Selective, selective_depth(t), best));
                best
            } else {
                let ev = engine.choose(&b);
                let Some(best) = ev.pos else { break };
                let (kind, depth) = if ev.exact {
                    (ScoreKind::Exact, 0)
                } else {
                    (ScoreKind::Midgame, ev.depth.min(63) as u8)
                };
                searched.push((b, ev.value, kind, depth, best));
                best
            };
            moves += &mv.to_kifu();
            let _ = b.make_move(mv);
            ply += 1;
        }
        let final_black = kuroobi::solver::final_score(&b);
        let final_black = if b.player() == Color::Black {
            final_black
        } else {
            -final_black
        }
        .clamp(-64, 64) as i8;
        let from = start.map_or(String::new(), |s| format!("{s} "));
        if let Err(e) = writeln!(games_out, "{from}{moves} {final_black:+} {n_random}")
            .and_then(|()| games_out.flush())
        {
            eprintln!("worker {w}: write failed: {e}");
            stop.store(true, Ordering::Relaxed);
            break;
        }
        for (i, &(board, score, score_kind, score_depth, mv)) in searched.iter().enumerate() {
            let black = board.player() == Color::Black;
            let (mover, opponent) = if black {
                (board.black, board.white)
            } else {
                (board.white, board.black)
            };
            let rec = Record {
                mover,
                opponent,
                score,
                game_score: if black { final_black } else { -final_black },
                ply: 60 - board.empty_count(),
                random: false,
                sq: mv.0,
                black_to_move: black,
                score_kind,
                score_depth,
                game_score_from: 0,
                game_start: i == 0,
            };
            if let Err(e) = shard.push(&rec.to_bytes()) {
                eprintln!("worker {w}: write failed: {e}");
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
        positions.fetch_add(searched.len() as u64, Ordering::Relaxed);
        games.fetch_add(1, Ordering::Relaxed);
    }
    let _ = shard.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selective_flag_reads_pairs_or_none() {
        assert_eq!(
            parse_selective("21:1.1, 22:0.8"),
            vec![(21, 1.1), (22, 0.8)]
        );
        assert!(parse_selective("none").is_empty());
    }

    #[test]
    #[should_panic(expected = "outside")]
    fn selective_t_must_fit_the_depth_field() {
        parse_selective("21:6.4");
    }

    #[test]
    fn starts_are_the_distinct_unplayed_records_at_the_ply() {
        let dir = std::env::temp_dir().join(format!("gendata-starts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rec = |b: &Board, ply: u8| Record {
            mover: b.player_bb(),
            opponent: b.opponent_bb(),
            score: 0.0,
            game_score: 0,
            ply,
            random: false,
            sq: 0,
            black_to_move: b.player() == Color::Black,
            score_kind: ScoreKind::Unknown,
            score_depth: 0,
            game_score_from: 0,
            game_start: false,
        };
        let root = Board::new();
        let mut after = Vec::new();
        for m in root.movable_iter() {
            let mut b = root;
            b.make_move(m).unwrap();
            after.push(b);
        }
        let mut records = vec![rec(&root, 0)];
        records.extend(after.iter().map(|b| rec(b, 1)));
        records.push(rec(&after[0], 1));
        let path = dir.join("in.rec");
        kuroobi::record::write_all(&path, &records).unwrap();
        std::fs::write(dir.join("games_00.txt"), format!("{} c4 +0 0\n", after[1])).unwrap();

        let mut a = parse_args_from(Vec::new());
        a.out_dir = dir.clone();
        a.starts = vec![path];
        a.starts_ply = Some(1);
        let got: HashSet<StartKey> = load_starts(&a).unwrap().iter().map(start_key).collect();
        let want: HashSet<StartKey> = after
            .iter()
            .filter(|b| start_key(b) != start_key(&after[1]))
            .map(start_key)
            .collect();
        assert_eq!(got, want);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_solve_wins_over_a_selective_entry() {
        let mut a = parse_args_from(Vec::new());
        a.solve_empties = 22;
        assert_eq!(selective_t(&a, 22), None);
        assert_eq!(selective_t(&a, 23), Some(0.6));
        assert_eq!(selective_t(&a, 25), None);
    }
}
