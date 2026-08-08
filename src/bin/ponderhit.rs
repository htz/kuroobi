//! ポンダリングの「予測手」がどれくらい当たるかを測る。
//!
//! 相手の手番中に先読みする方式のうち、**予測手 1 本を追う形 (方式 1) の
//! 値打ちはこの的中率でほぼ決まる。** 当たれば探索木がまるごと使えるが、
//! 外すと置換表に残るぶんしか得をしない。全合法手に配る方式 (2) と
//! どちらを採るかの判断材料にするために測る。
//!
//! **予測手は探索し直して出さない。** 自分の手を指したあとの局面を
//! 置換表に問い合わせる (`Engine::tt_best`) — 実際のポンダリングも
//! そうするしかないので、そこより良い予測を測っても意味がない。
//!
//! 自己対戦で測るので、**相手も同じエンジン**という点は割り引いて読む。
//! 相手の強さを変えられるようにしてあるのは、そのため:
//!
//! * 同じ設定どうし … 上限に近い値 (相手が自分と同じ読み筋を辿る)
//! * 相手だけ浅い   … 人間や別のエンジンに近い、下振れした値
//!
//! Usage:
//!   ponderhit [OPTIONS]
//!
//! Options:
//!   --games <n>          対局数 (default 20)
//!   --depth <n>          自分の中盤深さ (default 8)
//!   --solve-empties <n>  自分の完全読み開始 (default 14)
//!   --opp-depth <n>      相手の中盤深さ (default = --depth)
//!   --opp-solve <n>      相手の完全読み開始 (default = --solve-empties)
//!   --threads <n>        スレッド数 (default 1)
//!   --random-plies <n>   開幕の乱数手数 (default 8)
//!   --seed <n>           乱数の種 (default 7)
//!   --nnue <path>        NNUE の重み (default weights/nnue-h16.bin)
//!   --weights <path>     線形評価の重み (default weights/linear.bin)

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::board::Board;
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::position::Position;

struct Args {
    games: usize,
    depth: u32,
    solve_empties: u8,
    opp_depth: Option<u32>,
    opp_solve: Option<u8>,
    threads: usize,
    random_plies: usize,
    seed: u64,
    nnue: PathBuf,
    weights: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            games: 20,
            depth: 8,
            solve_empties: 14,
            opp_depth: None,
            opp_solve: None,
            threads: 1,
            random_plies: 8,
            seed: 7,
            nnue: PathBuf::from("weights/nnue-h16.bin"),
            weights: PathBuf::from("weights/linear.bin"),
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{k} needs a value"));
        match k.as_str() {
            "--games" => a.games = val()?.parse().map_err(|_| "bad --games")?,
            "--depth" => a.depth = val()?.parse().map_err(|_| "bad --depth")?,
            "--solve-empties" => {
                a.solve_empties = val()?.parse().map_err(|_| "bad --solve-empties")?
            }
            "--opp-depth" => a.opp_depth = Some(val()?.parse().map_err(|_| "bad --opp-depth")?),
            "--opp-solve" => a.opp_solve = Some(val()?.parse().map_err(|_| "bad --opp-solve")?),
            "--threads" => a.threads = val()?.parse().map_err(|_| "bad --threads")?,
            "--random-plies" => {
                a.random_plies = val()?.parse().map_err(|_| "bad --random-plies")?
            }
            "--seed" => a.seed = val()?.parse().map_err(|_| "bad --seed")?,
            "--nnue" => a.nnue = PathBuf::from(val()?),
            "--weights" => a.weights = PathBuf::from(val()?),
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(a)
}

/// 乱数。開幕のばらしにしか使わないので xorshift で足りる。
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// 空きマス数を局面の進み具合として使う (60 → 0)。
fn empties(b: &Board) -> u32 {
    64 - (b.player_bb() | b.opponent_bb()).count_ones()
}

/// 3 つの区間に分けて出す。**序盤だけ当たっても値打ちが薄い** —
/// 定石を抜けたあとの中盤で当たるかが知りたい。
fn phase(e: u32) -> usize {
    match e {
        45..=64 => 0, // 序盤
        21..=44 => 1, // 中盤
        _ => 2,       // 終盤
    }
}
const PHASE_NAME: [&str; 3] = ["序盤 (空き 45+)", "中盤 (空き 21-44)", "終盤 (空き 20-)"];

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let cfg = |depth: u32, solve: u8| EngineConfig {
        depth,
        solve_empties: solve,
        threads: args.threads,
        nnue: args.nnue.clone(),
        weights: args.weights.clone(),
        // **定石は切る。** 定石から返る手は探索を通らないので置換表に何も
        // 残らず、予測できないのが当たり前になる。測りたいのは探索の予測力
        use_book: false,
        ..EngineConfig::default()
    };

    let mut me = match Engine::new(cfg(args.depth, args.solve_empties)) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut opp = match Engine::new(cfg(
        args.opp_depth.unwrap_or(args.depth),
        args.opp_solve.unwrap_or(args.solve_empties),
    )) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut rng = Rng(args.seed | 1);
    // [予測できた, そのうち当たった] を区間ごとに
    let mut got = [0u64; 3];
    let mut hit = [0u64; 3];
    // 予測そのものが取れなかった回数 (置換表から溢れた)
    let mut miss_pred = [0u64; 3];
    /* 相手の合法手の数。**方式 2 (全合法手に配る) の 1/N がこれ。**
    オセロは狭いはずだが、見積もらずに測る */
    let mut br_sum = [0u64; 3];
    let mut br_n = [0u64; 3];

    for g in 0..args.games {
        let mut b = Board::new();
        // 開幕をばらす。**同じ手順を何十局も測っても 1 局ぶんの情報しかない**
        for _ in 0..args.random_plies {
            if b.is_game_over() {
                break;
            }
            let ms: Vec<Position> = b.movable_iter().collect();
            if ms.is_empty() {
                b.pass();
                continue;
            }
            let p = ms[rng.below(ms.len())];
            b.make_move_unchecked(p);
        }

        // 偶数局は自分が先に指す。色を入れ替えないと片方の手番だけを測る
        let mut my_turn = g % 2 == 0;
        while !b.is_game_over() {
            if b.movable_count() == 0 {
                b.pass();
                my_turn = !my_turn;
                continue;
            }
            if my_turn {
                let ev = me.choose(&b);
                let Some(p) = ev.pos else { break };
                let e = empties(&b);
                b.make_move_unchecked(p);
                // **指したあとの局面**の最善手が、相手が指すと思う手
                let pred = me.tt_best(&b);
                my_turn = false;

                // 相手の手番。パスを挟むと予測の相手が変わるので、その局は数えない
                if b.is_game_over() || b.movable_count() == 0 {
                    continue;
                }
                let actual = opp.choose(&b).pos;
                let Some(actual) = actual else { break };
                let ph = phase(e);
                br_sum[ph] += b.movable_count() as u64;
                br_n[ph] += 1;
                match pred {
                    Some(pred) if pred == actual => {
                        got[ph] += 1;
                        hit[ph] += 1;
                    }
                    Some(_) => got[ph] += 1,
                    None => miss_pred[ph] += 1,
                }
                b.make_move_unchecked(actual);
                my_turn = true;
            } else {
                let Some(p) = opp.choose(&b).pos else { break };
                b.make_move_unchecked(p);
                my_turn = true;
            }
        }
        eprint!("\r{}/{} 局", g + 1, args.games);
    }
    eprintln!();

    println!(
        "ponderhit: 自分 depth {} solve {} / 相手 depth {} solve {} ({} 局, 乱数開幕 {} 手, 定石なし)",
        args.depth,
        args.solve_empties,
        args.opp_depth.unwrap_or(args.depth),
        args.opp_solve.unwrap_or(args.solve_empties),
        args.games,
        args.random_plies,
    );
    let mut t_got = 0u64;
    let mut t_hit = 0u64;
    let mut t_none = 0u64;
    for i in 0..3 {
        let n = got[i] + miss_pred[i];
        if n == 0 {
            continue;
        }
        println!(
            "  {:<18} 予測できた {:>5}/{:<5} ({:>4.1}%)   当たった {:>5}/{:<5} ({:>4.1}%)",
            PHASE_NAME[i],
            got[i],
            n,
            100.0 * got[i] as f64 / n as f64,
            hit[i],
            got[i],
            if got[i] == 0 {
                0.0
            } else {
                100.0 * hit[i] as f64 / got[i] as f64
            },
        );
        if br_n[i] > 0 {
            println!(
                "  {:<18} 相手の合法手 平均 {:.1} 手 (方式 2 の 1/N)",
                "",
                br_sum[i] as f64 / br_n[i] as f64
            );
        }
        t_got += got[i];
        t_hit += hit[i];
        t_none += miss_pred[i];
    }
    let n = t_got + t_none;
    if n > 0 {
        println!(
            "  {:<18} 予測できた {:>5}/{:<5} ({:>4.1}%)   当たった {:>5}/{:<5} ({:>4.1}%)",
            "合計",
            t_got,
            n,
            100.0 * t_got as f64 / n as f64,
            t_hit,
            t_got,
            if t_got == 0 {
                0.0
            } else {
                100.0 * t_hit as f64 / t_got as f64
            },
        );
        // **予測が取れなかった回も外れとして数えた率**。方式 1 の期待値はこちら
        println!(
            "  実効の的中率 (予測できなかった回も外れに数える): {:.1}%",
            100.0 * t_hit as f64 / n as f64
        );
    }
    ExitCode::SUCCESS
}
