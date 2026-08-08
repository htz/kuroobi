//! ポンダリングの**効果**を測る。
//!
//! 1 手あたりの持ち時間を揃えて A と B を戦わせ、**A だけが相手の手番中に
//! 先読みする**。同じ局・同じ局面で 2 人を比べるので、別々に走らせるより
//! ばらつきが小さい。
//!
//! **方式は「予測手 1 本」だけ。** 全合法手に配る形と混合は測って捨てた
//! (`notes/pondering.md`)。
//!
//! 見るのは 2 つ:
//!
//! * **到達深さ** … 同じ持ち時間で何段まで読めたか。ポンダリングが効いて
//!   いれば A のほうが深い。**仕組みが効いているかを直接見る数字**
//! * **勝率** … 棋力の判定はこちらでしか行わない (CLAUDE.md の 2 番)。
//!   ただし局数が要るので、深さで効きを確かめてから回す
//!
//! **深さ固定では差が出ない。** 探索がどのみち最後まで走るので、先に
//! 読んでも得るものが無い。必ず `--ms` の持ち時間で測る。
//!
//! 1 局ごとに置換表を消す。温まった表の持ち越しは同一設定の自己対戦
//! ですら偏った結果を出す (CLAUDE.md の 4 番)。
//!
//! Usage:
//!   ponderarena [OPTIONS]
//!
//! Options:
//!   --games <n>          対局数 (default 10)
//!   --ms <n>             1 手の持ち時間 (ミリ秒, default 200)
//!   --ponder <on|off>    先読みするか (default on)。off は対照実験
//!   --fixed-depth        深さ固定で測る (見るのは時間。深さは両者同じ)
//!   --ponder-ms <n>      深さ固定のときの先読み時間 (default 300)
//!   --depth <n>          中盤深さの上限 (default 20)
//!   --solve-empties <n>  完全読み開始 (default 14)
//!   --threads <n>        スレッド数 (default 1)
//!   --random-plies <n>   開幕の乱数手数 (default 8)
//!   --seed <n>           乱数の種 (default 7)
//!   --nnue <path>        NNUE の重み (default weights/nnue-h16.bin)
//!   --weights <path>     線形評価の重み (default weights/linear.bin)

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use kuroobi::board::Board;
use kuroobi::engine::{Engine, EngineConfig};
use kuroobi::position::Position;

struct Args {
    games: usize,
    ms: u64,
    ponder: bool,
    /// 深さ固定で測る。**効き方が変わる** — 同じ深さへより速く着くので、
    /// 見るのは到達深さではなく 1 手にかかった時間。
    fixed: bool,
    /// 深さ固定のときに先読みへ与える時間 (相手の考慮時間の見立て)。
    ponder_ms: u64,
    depth: u32,
    solve_empties: u8,
    threads: usize,
    random_plies: usize,
    seed: u64,
    nnue: PathBuf,
    weights: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            games: 10,
            ms: 200,
            ponder: true,
            fixed: false,
            ponder_ms: 300,
            depth: 20,
            solve_empties: 14,
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
            "--ms" => a.ms = val()?.parse().map_err(|_| "bad --ms")?,
            "--ponder" => {
                a.ponder = match val()?.as_str() {
                    "on" => true,
                    "off" => false,
                    m => return Err(format!("--ponder は on か off ({m})")),
                }
            }
            "--fixed-depth" => a.fixed = true,
            "--ponder-ms" => a.ponder_ms = val()?.parse().map_err(|_| "bad --ponder-ms")?,
            "--depth" => a.depth = val()?.parse().map_err(|_| "bad --depth")?,
            "--solve-empties" => {
                a.solve_empties = val()?.parse().map_err(|_| "bad --solve-empties")?
            }
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

/// 到達深さの集計。完全読みに入った手 (depth 0) は数えない — そこは
/// 深さの概念が無く、混ぜると平均が下がるだけで比較にならない。
#[derive(Default)]
struct Depths {
    sum: u64,
    n: u64,
}
impl Depths {
    fn add(&mut self, d: u32) {
        if d > 0 {
            self.sum += d as u64;
            self.n += 1;
        }
    }
    fn avg(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum as f64 / self.n as f64
        }
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let cfg = || EngineConfig {
        depth: args.depth,
        solve_empties: args.solve_empties,
        threads: args.threads,
        nnue: args.nnue.clone(),
        weights: args.weights.clone(),
        // 定石は切る。定石から返る手は探索を通らないので、ポンダリングの
        // 効きも持ち時間の使われ方も測れなくなる
        use_book: false,
        ..EngineConfig::default()
    };

    let mut a = match Engine::new(cfg()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine A: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut b = match Engine::new(cfg()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("engine B: {e}");
            return ExitCode::FAILURE;
        }
    };

    let budget = Duration::from_millis(args.ms);
    let mut rng = Rng(args.seed | 1);
    let (mut da, mut db) = (Depths::default(), Depths::default());
    // 深さ固定のときに見る数字。**同じ深さへ何ミリ秒で着いたか**
    let (mut ta, mut tb) = (0u128, 0u128);
    let (mut na, mut nb) = (0u64, 0u64);
    let (mut wins, mut losses, mut draws) = (0u32, 0u32, 0u32);
    let mut ponder_nodes = 0u64;

    for g in 0..args.games {
        // **1 局ごとに消す。**温まった表の持ち越しは同一設定でも結果を歪める
        a.clear_tables();
        b.clear_tables();

        let mut board = Board::new();
        for _ in 0..args.random_plies {
            if board.is_game_over() {
                break;
            }
            let ms: Vec<Position> = board.movable_iter().collect();
            if ms.is_empty() {
                board.pass();
                continue;
            }
            let p = ms[rng.below(ms.len())];
            board.make_move_unchecked(p);
        }
        // 開幕は 2 局で 1 組にして色を入れ替える (先後の差を消す)
        let mut a_turn = g % 2 == 0;

        while !board.is_game_over() {
            if board.movable_count() == 0 {
                board.pass();
                a_turn = !a_turn;
                continue;
            }
            let deadline = Instant::now() + budget;
            if a_turn {
                let t0 = Instant::now();
                let ev = if args.fixed {
                    a.choose(&board)
                } else {
                    a.choose_within(&board, Some(deadline))
                };
                ta += t0.elapsed().as_micros();
                na += 1;
                da.add(ev.depth);
                let Some(p) = ev.pos else { break };
                board.make_move_unchecked(p);
                /* **ここがポンダリング。** 実戦では相手が考えている間に
                走るので、A の持ち時間は減らない。測定でも B の持ち時間と
                同じ長さだけ回す */
                if args.ponder && !board.is_game_over() && board.movable_count() > 0 {
                    let slice = if args.fixed {
                        Duration::from_millis(args.ponder_ms)
                    } else {
                        budget
                    };
                    ponder_nodes += a.ponder(&board, Instant::now() + slice);
                }
            } else {
                let t0 = Instant::now();
                let ev = if args.fixed {
                    b.choose(&board)
                } else {
                    b.choose_within(&board, Some(deadline))
                };
                tb += t0.elapsed().as_micros();
                nb += 1;
                db.add(ev.depth);
                let Some(p) = ev.pos else { break };
                board.make_move_unchecked(p);
            }
            a_turn = !a_turn;
        }

        // 石差。手番視点にならないよう、A の色から数え直す
        let (bl, wh) = (
            board.player_bb().count_ones() as i32,
            board.opponent_bb().count_ones() as i32,
        );
        // `player_bb` はそのときの手番の石。終局後の手番は数え上げに関係
        // しないので、A が最後に手番だったかで向きを決める
        let diff = if a_turn { bl - wh } else { wh - bl };
        match diff.cmp(&0) {
            std::cmp::Ordering::Greater => wins += 1,
            std::cmp::Ordering::Less => losses += 1,
            std::cmp::Ordering::Equal => draws += 1,
        }
        eprint!("\r{}/{} 局", g + 1, args.games);
    }
    eprintln!();

    let m = if args.ponder {
        "する"
    } else {
        "しない (対照)"
    };
    println!(
        "ponderarena: 1 手 {} ms / depth 上限 {} / solve {} / {} スレッド / {} 局 (定石なし)",
        args.ms, args.depth, args.solve_empties, args.threads, args.games,
    );
    println!("  A = ポンダリング {m} / B = しない");
    if args.fixed {
        let (aa, bb) = (
            ta as f64 / na.max(1) as f64 / 1000.0,
            tb as f64 / nb.max(1) as f64 / 1000.0,
        );
        println!(
            "  1 手の時間  A {:.1} ms  B {:.1} ms   {:+.1}% ({} ms の先読み)",
            aa,
            bb,
            100.0 * (aa - bb) / bb,
            args.ponder_ms,
        );
    }
    println!(
        "  到達深さ  A {:.2}  B {:.2}   差 {:+.2} 段",
        da.avg(),
        db.avg(),
        da.avg() - db.avg()
    );
    println!(
        "  先読みで訪れたノード {} ({:.0} / 手)",
        ponder_nodes,
        if da.n == 0 {
            0.0
        } else {
            ponder_nodes as f64 / da.n as f64
        }
    );
    let total = (wins + losses + draws) as f64;
    println!(
        "  A の成績  {}勝 {}敗 {}分 ({:.1}%)",
        wins,
        losses,
        draws,
        100.0 * (wins as f64 + 0.5 * draws as f64) / total.max(1.0)
    );
    println!("  ※ 勝率はこの局数では判定に足りない。深さの差で効きを見る");
    ExitCode::SUCCESS
}
