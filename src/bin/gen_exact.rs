//! Build a ground-truth validation set: random positions at a fixed empty
//! count, each labeled with its **exact** value from the solver.
//!
//! The training corpus labels every position with the final disc difference
//! of the game it came from, so the same position gets different labels
//! depending on how the game continued (measured irreducible MSE ~11 in the
//! early band). A model can only be scored honestly against values that are
//! actually true of the position, which is what this produces.
//!
//! Writes two files: `<out>.data` in the training record format (so the
//! existing tooling can score against it; the solved value is stored as
//! the game result, with no game behind it) and `<out>.obf` so other
//! engines can be measured on the same positions.
//!
//! How the positions are reached matters as much as how they are labelled.
//! By default every move is uniformly random, which is diverse but is not
//! what the engine meets over a board: random play wanders into positions no
//! search would allow. `--play-depth` plays the opening at random for
//! `--random-plies` and then lets the engine choose, which costs generation
//! time and buys a set whose error actually stands for error in a game. The
//! trade is that positions then reflect the model doing the playing, so a
//! set built this way is tied to the model that built it -- record which one.
//!
//! Usage: gen_exact [--empties n] [--count n] [--seed n] [--threads n]
//!                  [--random-plies n] [--play-depth n] [--nnue f] --out <prefix>
use kuroobi::evaluator::Evaluator;
use kuroobi::midgame::{NnueSearch, SharedTt};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::record::{Record, Writer, NO_SQUARE};
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::{Board, Color, Position};
use std::io::Write;
use std::path::Path;

fn main() {
    let mut empties: u8 = 22;
    let mut count: usize = 500;
    let mut seed: u64 = 0x1234_5678_9ABC_DEF0;
    let mut threads: usize = 10;
    let mut out = String::from("exact22");
    let mut random_plies: u32 = 8;
    let mut play_depth: u32 = 0;
    let mut nnue_path = String::from("weights/nnue.bin");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--empties" => empties = it.next().and_then(|v| v.parse().ok()).unwrap_or(empties),
            "--count" => count = it.next().and_then(|v| v.parse().ok()).unwrap_or(count),
            "--seed" => seed = it.next().and_then(|v| v.parse().ok()).unwrap_or(seed),
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(threads),
            "--out" => out = it.next().unwrap_or(out),
            "--random-plies" => {
                random_plies = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(random_plies)
            }
            "--play-depth" => {
                play_depth = it.next().and_then(|v| v.parse().ok()).unwrap_or(play_depth)
            }
            "--nnue" => nnue_path = it.next().unwrap_or(nnue_path),
            other => panic!("unknown flag {other}"),
        }
    }

    // The engine that plays the non-random part, if any.
    let searcher: Option<(std::sync::Arc<Nnue>, std::sync::Arc<SharedTt>)> = if play_depth > 0 {
        let mut nn = Nnue::new(EGAROUCID_PATTERNS);
        nn.load(std::path::Path::new(&nnue_path)).expect("nnue");
        nn.quantize();
        eprintln!("playing plies past {random_plies} with {nnue_path} at depth {play_depth}");
        Some((
            std::sync::Arc::new(nn),
            std::sync::Arc::new(SharedTt::new(20)),
        ))
    } else {
        None
    };

    // Positions: play from the start until the target empty count.
    let mut s = seed;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let mut boards: Vec<Board> = Vec::with_capacity(count);
    while boards.len() < count {
        let mut b = Board::new();
        let mut ok = true;
        let mut ply = 0u32;
        while b.empty_count() > empties {
            let m = b.movable();
            if m == 0 {
                b.pass();
                if b.movable() == 0 {
                    ok = false;
                    break;
                }
                continue;
            }
            // Random for the opening (diversity), then the engine (realism).
            // The table is cleared per move: a warm table carried between
            // playouts makes the games correlate with each other.
            let chosen = match &searcher {
                Some((nn, tt)) if ply >= random_plies => {
                    tt.clear();
                    let mut se = NnueSearch::new(nn.clone(), tt.clone());
                    se.threads = 1;
                    se.best_move_deadline(&b, play_depth, None).0
                }
                _ => None,
            };
            let pos = chosen.unwrap_or_else(|| {
                let n = m.count_ones();
                let k = (rnd() % n as u64) as u32;
                let mut mm = m;
                for _ in 0..k {
                    mm &= mm - 1;
                }
                Position(mm.trailing_zeros() as u8)
            });
            let _ = b.make_move(pos);
            ply += 1;
        }
        if ok && b.empty_count() == empties && b.movable() != 0 {
            boards.push(b);
        }
    }
    eprintln!("generated {} positions at {empties} empties", boards.len());

    // Solve exactly, in parallel over positions (each solver single-threaded:
    // many independent solves beat one parallel solve at this size).
    let t0 = std::time::Instant::now();
    let chunk = boards.len().div_ceil(threads.max(1));
    let mut results: Vec<(Board, i32)> = Vec::with_capacity(boards.len());
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for part in boards.chunks(chunk) {
            handles.push(scope.spawn(move || {
                let mut ev = Evaluator::new(EGAROUCID_PATTERNS);
                ev.load_weights(std::path::Path::new("weights/linear.bin"))
                    .expect("weights");
                let mut solver = Solver::new(22);
                let mut out = Vec::with_capacity(part.len());
                for b in part {
                    let r = solver.solve_with_eval(EndSolverMode::Perfect, b, Some(&ev));
                    out.push((*b, r.value));
                }
                out
            }));
        }
        for h in handles {
            results.extend(h.join().unwrap());
        }
    });
    eprintln!(
        "solved {} positions in {:.1}s",
        results.len(),
        t0.elapsed().as_secs_f64()
    );

    // Records with the mover as Black (the training convention).
    let mut data = Writer::create(Path::new(&format!("{out}.data"))).expect("create data");
    let mut obf = std::fs::File::create(format!("{out}.obf")).expect("create obf");
    for (b, v) in &results {
        let (mover, opponent) = if b.player() == Color::Black {
            (b.black, b.white)
        } else {
            /* Relabel the mover's discs as the record's "Black".

            The solver reports from the side to move, and the swap does not
            change who is to move -- it only renames them -- so the value
            carries over untouched. Negating it here (as this did) made the
            label the opposite of the truth for every White-to-move position:
            all of them at an odd empty count, and the ones a pass produced at
            an even count, which is how 0.5% of the sets that every accuracy
            number in this project is measured against came to be inverted.
            `checkdata` re-solves a set and catches exactly this. */
            (b.white, b.black)
        };
        let score = (*v).clamp(-64, 64) as i8;
        data.write(&Record {
            mover,
            opponent,
            score: f32::from(score),
            game_score: score,
            ply: 60 - b.empty_count(),
            random: false,
            sq: NO_SQUARE,
            black_to_move: true,
            game_id: 0,
        })
        .unwrap();

        // obf: 64 board chars in rank-major order, then the side to move, as
        // the bench files use.
        let mut line = String::with_capacity(70);
        for idx in 0..64u8 {
            let bit = Position::from_file_rank(idx % 8, idx / 8).unwrap().to_bit();
            line.push(if b.black & bit != 0 {
                'X'
            } else if b.white & bit != 0 {
                'O'
            } else {
                '-'
            });
        }
        line.push(' ');
        line.push(if b.player() == Color::Black { 'X' } else { 'O' });
        writeln!(obf, "{line}; exact {v}").unwrap();
    }
    data.finish().expect("flush data");
    eprintln!("wrote {out}.data and {out}.obf");
}
