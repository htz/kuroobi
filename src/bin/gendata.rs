//! Generate training positions by self-play, in `kuroobi::record`'s format:
//! every position carries both the value the search assigned it and the
//! final disc difference of the game it came from.
//!
//! The first version of this tool wrote only the search value, in the
//! 17-byte format the trainer then read, on the reasoning that a search
//! value is a property of the position while the final result depends on
//! how the game went on. That threw the final result away, and it is the
//! final result the trainer fits (searched positions take `game_score`;
//! the search value is kept for filtering games whose result disagrees
//! with it by more than a threshold). 33
//! million positions generated that way could not be repaired, because
//! nothing else was kept. So this writes every field the record has,
//! plus the games themselves as transcripts, and the trainer reads this
//! record directly; the choice of teacher value is made there.
//!
//! The record is `kuroobi::record`: mover's discs, opponent's discs, search value (mover's
//! view), final disc difference (mover's view, empties to the winner), ply,
//! random-move flag, move played, side to move, game id. Positions reached
//! by the opening's random moves are recorded too, flagged, with the search
//! value the engine gives them, so the filter can drop or keep them by
//! that flag.
//!
//! **Openings are randomised, and the amount of randomness rotates.** A
//! self-play game between two copies of the same engine is deterministic, so
//! without randomness every game is the same game -- and since the opening
//! decides the whole game, the number of distinct games is capped by the
//! number of distinct openings. Four random plies reach only 236 positions,
//! which is why the first corpus generated this way came out 56% duplicate.
//! Each game opens with a random number of random plies, drawn in turn from
//! `--random-plies`, so stopping the run at any moment leaves the counts
//! within one game of each other.
//!
//! **Shards are self-contained, and none of the work is thrown away.** A
//! worker writes to `.part` and renames to `.data` when the shard fills or
//! when the run ends, whichever comes first. Records are fixed width and
//! written whole, so a shard cut short is simply a smaller shard.
//!
//! A `.part` left by a killed process is in the same state, and the next run
//! adopts it: startup renames any leftover `.part` to `.data` before opening
//! new shards. Nothing needs doing by hand, which matters because the event
//! that leaves one -- the machine shutting down -- is not something anyone
//! schedules.
//!
//! Duplicates are not removed here. Workers use different seeds, so the
//! overlap is small (about 1.6% in a million records), and a sort over
//! fixed-width records afterwards
//! is both cheaper and lets the unit -- raw board, or the smallest of its
//! eight symmetries -- be decided later.
//!
//! Usage:
//!   gendata --out-dir DIR [--jobs N] [--positions N] [--games N]
//!           [--random-plies 4,6,8,10,12] [--depth N] [--solve-empties N]
//!           [--band N] [--shard-size N] [--seed N] [--nnue PATH]
//!           [--patterns nnue|egaroucid|compact] [--threads N]
//!
//! Output: `shard_WW_NNNN.data` (records) and `games_WW.txt`, one
//! game per line: the moves in `f5d6` notation (passes are not written; a
//! replay infers them), the final disc difference for Black with empties to
//! the winner, and the number of random opening plies.
//!
//! `--seed` is for reproducing a run. Left out, one is drawn from the OS, so
//! two machines started with the same command line still generate different
//! games.
//!
//! `src/bin/openings.rs` measures how many distinct positions a given
//! opening length reaches, which is what `--random-plies` has to be chosen
//! against.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use kuroobi::datagen::{adopt_leftovers, record, Pending, Shard};
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::pattern::{COMPACT_PATTERNS, EGAROUCID_PATTERNS, NNUE_PATTERNS};
use kuroobi::{Board, Color, Position};

struct Args {
    out_dir: PathBuf,
    jobs: usize,
    threads: usize,
    positions: u64,
    games: u64,
    random_plies: Vec<u32>,
    depth: u32,
    solve_empties: u8,
    band: u8,
    shard_size: usize,
    seed: u64,
    nnue: PathBuf,
    patterns: &'static [kuroobi::pattern::Pattern],
    /// Transposition table sizes, as powers of two. The engine's defaults
    /// are sized for a full-strength game; a generator plays one fixed
    /// depth over and over and needs far less, and every worker pays for
    /// its own table twice over -- once in memory, once in the per-game
    /// clear.
    hash_mid: u32,
    hash_end: u32,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            out_dir: PathBuf::from("data/records/gen"),
            // Independent games, one search thread each. Measured on a
            // 10-core machine: eight of these run 6.9x faster than one,
            // where giving a single game eight threads reaches only 1.55x.
            jobs: 8,
            threads: 1,
            positions: 0,
            games: 0,
            /* Twelve is where the openings stop repeating. Measured over a
            million samples: 4 plies reach 236 distinct positions, 8 reach
            235k (76% duplicate), 12 reach 994k (0.6%). Since the search
            after the opening is deterministic, two games from the same
            opening position are the same game -- so the opening's reach is
            the corpus's diversity, and a first run at 4..12 came out 56%
            duplicate.

            Not deeper than 16: 12 already suffices, and every extra random
            ply is one more position the engine would not have played.
            A corpus built this way opens for 11-13 plies (nothing above 50
            empties in a million records) and carries 1.6% duplicates. */
            random_plies: vec![12, 14, 16],
            // Level 17 settings:
            // 17 midgame plies, and the endgame solved exactly from 22
            // empties.
            depth: 17,
            solve_empties: 22,
            band: 6,
            /* Small on purpose. A shard becomes usable data only when it
            is renamed, and the rename happens when the shard fills -- so
            the size decides how much work a kill leaves needing manual
            recovery. Signals cannot be caught here (there is no handler),
            and a `.part` is exactly as good as a `.data` once renamed, but
            the fewer of those the better. At 70 positions a second this
            closes a shard every twenty minutes or so. */
            shard_size: 100_000,
            // Zero means "not given"; `main` draws one from the OS. A seed
            // is for reproducing a run, not for defining one -- defaulting
            // it to a constant made every machine that omitted the flag
            // generate byte-identical data, silently.
            seed: 0,
            nnue: PathBuf::from("weights/nnue.bin"),
            patterns: EGAROUCID_PATTERNS,
            hash_mid: 22,
            hash_end: 22,
        }
    }
}

/// A seed from the OS, for when the caller did not pick one.
///
/// `RandomState` is seeded from the operating system's entropy, which is
/// what "no seed given" should mean. Two machines started in the same second
/// still diverge.
fn os_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let s = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    // Zero is the "not given" marker, so never return it.
    if s == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        s
    }
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
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
                    .collect();
                assert!(!a.random_plies.is_empty(), "--random-plies is empty");
            }
            "--depth" => a.depth = val().parse().expect("--depth"),
            "--solve-empties" => a.solve_empties = val().parse().expect("--solve-empties"),
            "--band" => a.band = val().parse().expect("--band"),
            "--shard-size" => a.shard_size = val().parse().expect("--shard-size"),
            "--seed" => a.seed = val().parse().expect("--seed"),
            "--nnue" => a.nnue = PathBuf::from(val()),
            "--hash-mid" => a.hash_mid = val().parse().expect("--hash-mid"),
            "--hash-end" => a.hash_end = val().parse().expect("--hash-end"),
            "--patterns" => {
                a.patterns = match val().as_str() {
                    "egaroucid" => EGAROUCID_PATTERNS,
                    "compact" => COMPACT_PATTERNS,
                    "nnue" => NNUE_PATTERNS,
                    other => panic!("unknown pattern set {other}"),
                }
            }
            other => panic!("unknown flag {other}"),
        }
    }
    if a.seed == 0 {
        a.seed = os_seed();
    }
    a
}

/// xorshift64*, one per worker so the games never line up.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// One of the legal moves, uniformly.
    fn pick(&mut self, moves: u64) -> Position {
        let n = moves.count_ones();
        let k = (self.next() % n as u64) as u32;
        let mut m = moves;
        for _ in 0..k {
            m &= m - 1;
        }
        Position(m.trailing_zeros() as u8)
    }
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

    let stop = Arc::new(AtomicBool::new(false));

    let positions = Arc::new(AtomicU64::new(0));
    let games = Arc::new(AtomicU64::new(0));
    let t0 = std::time::Instant::now();

    eprintln!(
        "gendata: {} jobs, depth {} solve {} band {}, random plies {:?}, shard {} positions",
        a.jobs, a.depth, a.solve_empties, a.band, a.random_plies, a.shard_size
    );
    eprintln!(
        "         nnue {} -> {}",
        a.nnue.display(),
        a.out_dir.display()
    );
    // Printed whether it was given or drawn: without it, a run cannot be
    // reproduced, and two runs cannot be told apart after the fact.
    eprintln!("         seed {}", a.seed);

    let a = Arc::new(a);
    let running = Arc::new(AtomicU64::new(a.jobs as u64));
    std::thread::scope(|scope| {
        for w in 0..a.jobs {
            let (a, stop, positions, games, running) = (
                a.clone(),
                stop.clone(),
                positions.clone(),
                games.clone(),
                running.clone(),
            );
            scope.spawn(move || {
                worker(w, &a, &stop, &positions, &games);
                // The last worker out ends the run, so the progress thread
                // does not hold the scope open.
                if running.fetch_sub(1, Ordering::Relaxed) == 1 {
                    stop.store(true, Ordering::Relaxed);
                }
            });
        }
        // Progress, so a long run says what it is doing.
        let (a2, stop2, pos2, gam2) = (a.clone(), stop.clone(), positions.clone(), games.clone());
        scope.spawn(move || {
            let mut ticks = 0u32;
            while !stop2.load(Ordering::Relaxed) {
                // Short sleeps so a finished run exits promptly; report only
                // every ten seconds.
                std::thread::sleep(std::time::Duration::from_millis(200));
                ticks += 1;
                if !ticks.is_multiple_of(50) {
                    continue;
                }
                let p = pos2.load(Ordering::Relaxed);
                let g = gam2.load(Ordering::Relaxed);
                let el = t0.elapsed().as_secs_f64().max(1e-9);
                eprintln!(
                    "  {p} positions, {g} games, {:.0}/s, {:.1} min elapsed",
                    p as f64 / el,
                    el / 60.0
                );
                if (a2.positions > 0 && p >= a2.positions) || (a2.games > 0 && g >= a2.games) {
                    stop2.store(true, Ordering::Relaxed);
                }
            }
        });
    });

    let p = positions.load(Ordering::Relaxed);
    let g = games.load(Ordering::Relaxed);
    eprintln!(
        "done: {p} positions from {g} games in {:.1} min",
        t0.elapsed().as_secs_f64() / 60.0
    );
}

fn worker(w: usize, a: &Args, stop: &AtomicBool, positions: &AtomicU64, games: &AtomicU64) {
    let mut cfg = EngineConfig {
        depth: a.depth,
        solve_empties: a.solve_empties,
        band: a.band,
        threads: a.threads,
        nnue: a.nnue.clone(),
        nnue_patterns: a.patterns,
        midgame_hash_bits: a.hash_mid,
        solver_hash_bits: a.hash_end,
        // No book: it would replay the same opening every game, which is
        // exactly what the random plies exist to avoid.
        use_book: false,
        ..Default::default()
    };
    cfg.weights = PathBuf::from("weights/linear.bin");
    /* No stop handle. `Engine::new` installs one unconditionally, and the
    solver then spawns a watcher thread for every solve it runs -- some
    twenty per game here -- to poll a flag this generator never raises. The
    thread costs its own creation and, at the end of the solve, however much
    of its 5 ms sleep is left. Nothing here can ask a search to stop. */
    let mut engine = match Engine::new(cfg) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("worker {w}: {e}");
            stop.store(true, Ordering::Relaxed);
            return;
        }
    };
    if std::env::var_os("KUROOBI_KEEP_STOP").is_none() {
        engine.drop_solver_stop();
    }

    let mut rng = Rng(a.seed ^ (w as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).max(1));
    let mut shard = Shard::new(a.out_dir.clone(), w, a.shard_size);
    // Where this worker is in the rotation over `--random-plies`. Advancing
    // one step per game keeps the counts even at every moment, not only at
    // the end.
    let mut rot = w % a.random_plies.len();
    // Measurement only: what the per-game table clear costs.
    let skip_clear = std::env::var_os("KUROOBI_NO_CLEAR").is_some();
    let mut buf: Vec<Pending> = Vec::with_capacity(64);
    // The games themselves, one per line, appended so a restart continues
    // the same file. Everything in a `.data` can be rebuilt from this.
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
    let mut game_id: u16 = 0;

    while !stop.load(Ordering::Relaxed) {
        if a.positions > 0 && positions.load(Ordering::Relaxed) >= a.positions {
            break;
        }
        if a.games > 0 && games.load(Ordering::Relaxed) >= a.games {
            break;
        }

        let n_random = a.random_plies[rot];
        rot = (rot + 1) % a.random_plies.len();

        buf.clear();
        let mut b = Board::new();
        let mut ply = 0u32;
        // A fresh table per game: carried over, it correlates one game's
        // search with the next one's.
        if !skip_clear {
            engine.clear_tables();
        }

        loop {
            if b.movable() == 0 {
                b.pass();
                if b.movable() == 0 {
                    break;
                }
            }
            /* The opening's random moves are recorded as well, flagged, with
            the value the search gives the position (`is_random`), so what to
            do with them is decided at filter time; the earlier version of
            this tool skipped them, which left
            nothing at all for the stages the opening covers. */
            let random = ply < n_random;
            let ev = engine.choose(&b);
            let Some(best) = ev.pos else { break };
            let mv = if random { rng.pick(b.movable()) } else { best };
            // Record before the move: the value belongs to the position the
            // search was run on.
            buf.push(Pending {
                board: b,
                value: ev.value,
                ply: ply as u8,
                random,
                mv,
            });
            let _ = b.make_move(mv);
            ply += 1;
        }

        // The result every position is judged by: the final disc
        // difference, empties to the winner, from Black's side.
        let final_black = kuroobi::datagen::final_black(&b);

        let moves: String = buf.iter().map(|p| p.mv.to_kifu()).collect();
        if let Err(e) = writeln!(games_out, "{moves} {final_black:+} {n_random}")
            .and_then(|()| games_out.flush())
        {
            eprintln!("worker {w}: write failed: {e}");
            stop.store(true, Ordering::Relaxed);
            break;
        }

        // The game is written as a unit, so a stop between games never
        // leaves half of one in the file.
        for p in &buf {
            let final_mover = if p.board.player() == Color::Black {
                final_black
            } else {
                -final_black
            };
            if let Err(e) = shard.push(&record(p, final_mover, game_id)) {
                eprintln!("worker {w}: write failed: {e}");
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
        game_id = game_id.wrapping_add(1);
        positions.fetch_add(buf.len() as u64, Ordering::Relaxed);
        games.fetch_add(1, Ordering::Relaxed);
    }

    shard.finish();
}
