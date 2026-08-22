//! Reproduce a live-game position and re-measure its value (throwaway diagnostic).
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::Board;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let start = &args[1]; // BO[] board (64 cells + mover)
    let moves = &args[2]; // "F5F6H3..." (2 chars each)
    let upto: usize = args[3].parse().unwrap();
    let depth: u32 = args.get(4).and_then(|x| x.parse().ok()).unwrap_or(22);

    // GGS BO[] is 64 cells + mover, same order as Board::from_string.
    let mut b = Board::from_string(start).expect("board");
    /* Warm the table: live play arrives with previous moves already
    searched, so a cold table is a different condition. `WARM=<secs>`
    re-searches our earlier moves along the way (playing the recorded
    moves regardless). */
    let warm: u64 = std::env::var("WARM")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(0);

    let mut cfg = EngineConfig {
        depth: 60,
        solve_empties: 30,
        band: 8,
        threads: 8,
        ..Default::default()
    };
    cfg.use_book = false;
    let mut eng = Engine::new(cfg).expect("engine");

    let mv: Vec<String> = moves
        .as_bytes()
        .chunks(2)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect();
    for (i, m) in mv.iter().enumerate() {
        if i >= upto {
            break;
        }
        if m.eq_ignore_ascii_case("PA") {
            b.pass();
            continue;
        }
        let f = m.as_bytes()[0].to_ascii_lowercase() - b'a';
        let r = m.as_bytes()[1] - b'1';
        let pos = kuroobi::Position::from_index((f as u32) * 8 + r as u32).unwrap();
        // Search only our turns before advancing (even i = first player).
        if warm > 0 && i % 2 == 0 {
            let dl = std::time::Instant::now() + std::time::Duration::from_secs(warm);
            let mv = eng.choose_within(&b, Some(dl));
            println!(
                "  warm move {}: {:?} {:+.2} depth {}",
                i + 1,
                mv.pos,
                mv.value,
                mv.depth
            );
        }
        b.make_move_bits(pos);
    }
    println!("empties {} (warm {}s)", b.empty_count(), warm);
    // Measure through the play path (deadline choose_within).
    let secs: u64 = args.get(5).and_then(|x| x.parse().ok()).unwrap_or(0);
    if secs > 0 {
        let dl = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        let mv = eng.choose_within(&b, Some(dl));
        println!(
            "  play path: {:?} {:+.2} depth {} {}{}",
            mv.pos,
            mv.value,
            mv.depth,
            if mv.exact { "solve" } else { "search" },
            if mv.cut { " (cut)" } else { "" }
        );
    } else {
        let out = eng.analyze(&b, depth);
        let mut v: Vec<_> = out.into_iter().collect();
        v.sort_by(|a, b| b.1.value.partial_cmp(&a.1.value).unwrap());
        for (p, e) in v.iter().take(3) {
            println!("  {:?} {:+.2}", p, e.value);
        }
    }
}
