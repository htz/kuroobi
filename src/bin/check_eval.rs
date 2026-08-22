//! 実戦の局面を再現して評価値を測り直す (調べもの用・使い捨て)。
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::Board;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let start = &args[1]; // BO[] の盤 (64 文字 + 手番)
    let moves = &args[2]; // "F5F6H3..." (2 文字ずつ)
    let upto: usize = args[3].parse().unwrap();
    let depth: u32 = args.get(4).and_then(|x| x.parse().ok()).unwrap_or(22);

    // GGS の BO[] は盤 64 文字 + 手番。Board::from_string と同じ並び
    let mut b = Board::from_string(start).expect("盤面");
    /* **置換表を温める。** 実戦は前の手を同じエンジンで読んでから来るので、
    冷えた表で測ると条件が違う。`WARM=<秒>` を与えると、ここまでの自分の手を
    同じ経路で読み直してから進める (指す手は棋譜のものに揃える)。 */
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
        // 自分の手番のところだけ読ませてから進める (i が偶数 = 先手側)
        if warm > 0 && i % 2 == 0 {
            let dl = std::time::Instant::now() + std::time::Duration::from_secs(warm);
            let mv = eng.choose_within(&b, Some(dl));
            println!(
                "  温め {} 手目: {:?} {:+.2} 深さ {}",
                i + 1,
                mv.pos,
                mv.value,
                mv.depth
            );
        }
        b.make_move_bits(pos);
    }
    println!("空き {} (温め {}s)", b.empty_count(), warm);
    // 対局と同じ経路で測る (期限つき choose_within)
    let secs: u64 = args.get(5).and_then(|x| x.parse().ok()).unwrap_or(0);
    if secs > 0 {
        let dl = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        let mv = eng.choose_within(&b, Some(dl));
        println!(
            "  対局経路: {:?} {:+.2} 深さ {} {}{}",
            mv.pos,
            mv.value,
            mv.depth,
            if mv.exact { "読切" } else { "探索" },
            if mv.cut { " (打切)" } else { "" }
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
