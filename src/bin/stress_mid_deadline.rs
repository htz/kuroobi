//! 期限つきの**中盤**探索が、中断値をそのまま返していないかを見る。
//!
//! 実戦の署名: 評価が `+0.00` (= 数でない値を丸めたもの)、深さが空きマス数を
//! 超える、そして「打切」。この 3 つが揃った手が X 打ちだった。
//!
//! Usage: stress_mid_deadline [局面数] [スレッド]
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::{Board, Position};

fn rnd(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let threads: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let mut cfg = EngineConfig {
        depth: 60,
        solve_empties: 29,
        band: 8,
        threads,
        use_book: false,
        ..Default::default()
    };
    cfg.nnue = std::path::PathBuf::from("weights/nnue-h16.bin");
    cfg.weights = std::path::PathBuf::from("weights/linear.bin");
    let mut eng = Engine::new(cfg).expect("engine");

    let mut s: u64 = 0xC0FF_EE12_3456_789A;
    let (mut bad, mut cut, mut done) = (0usize, 0usize, 0usize);
    for i in 0..n {
        // 抽選開局の少し後 (実戦で出た空き 38-46) を作る
        let target = 38 + (rnd(&mut s) % 9) as u32;
        let mut b = Board::new();
        while b.empty_count() as u32 > target {
            let m = b.movable();
            if m == 0 {
                b.pass();
                if b.movable() == 0 {
                    break;
                }
                continue;
            }
            let k = (rnd(&mut s) % m.count_ones() as u64) as u32;
            let mut mm = m;
            for _ in 0..k {
                mm &= mm - 1;
            }
            b.make_move_bits(Position::from_index(mm.trailing_zeros()).unwrap());
        }
        if b.empty_count() as u32 != target || b.movable() == 0 {
            continue;
        }
        done += 1;
        // 実戦は 1 手 80〜150 秒。短くしても経路は同じ (段の途中で切られる)
        let ms = 200 + (rnd(&mut s) % 2800);
        let mv = eng.choose_within(
            &b,
            Some(std::time::Instant::now() + std::time::Duration::from_millis(ms)),
        );
        if mv.cut {
            cut += 1;
        }
        let empties = b.empty_count() as u32;
        if !mv.value.is_finite() || mv.depth > empties {
            bad += 1;
            println!(
                "局面 {i}: 空き {empties} 期限 {ms}ms → 手 {:?} 値 {} 深さ {}{}",
                mv.pos.map(|p| p.index()),
                mv.value,
                mv.depth,
                if mv.cut { " 打切" } else { "" }
            );
        }
    }
    let nf = kuroobi::engine::NON_FINITE_VALUES.load(std::sync::atomic::Ordering::Relaxed);
    println!("{done} 局面 (うち打切 {cut}) 中 {bad} 件が壊れた値/深さ、非数の丸め {nf} 件");
}
