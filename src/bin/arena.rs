//! Head-to-head arena: play two weight files against each other and report
//! the win rate with a 95% confidence interval.
//!
//! Both engines pick 1-ply greedy moves with their own evaluator; the
//! endgame is played out the same way (no solver) so only the evaluators
//! differ. Opening diversity comes from playing k random plies before the
//! engines take over; each opening is played twice with colors swapped to
//! cancel first-mover bias.
//!
//! Usage:
//!   arena --a <weights-A> --b <weights-B> [OPTIONS]
//!
//! Options:
//!   --games <n>        Total games (default 1000; rounded up to even)
//!   --random-plies <n> Random opening plies (default 6)
//!   --patterns <set>   egaroucid | edax (default egaroucid)
//!   --seed <n>         RNG seed (default 7)

use std::path::PathBuf;
use std::process::ExitCode;

use kuroobi::engine::{Engine, EngineConfig as EngCfg};
use kuroobi::evaluator::Evaluator;
use kuroobi::pattern::{EDAX_PATTERNS, EGAROUCID_PATTERNS, EGAROUCID_PLUS_PATTERNS};
use kuroobi::search::Searcher;
use kuroobi::solver::{EndSolverMode, Solver};
use kuroobi::timectl::{self, Levels, Pace, Situation, SolveRef};
use kuroobi::{Board, Color, Position};
use std::time::Instant;

struct Args {
    /// NNUE モード。両方指定すると Engine (実戦そのもの) で戦わせる。
    nnue_a: Option<PathBuf>,
    nnue_b: Option<PathBuf>,
    /// **対局全体の持ち時間** (秒)。時間配分の良し悪しはこれでしか測れない。
    time_secs: u64,
    /// 時間の配り方。片側だけ変えて配り方の差を測る。
    pace_a: String,
    pace_b: String,
    /// **較正した読切速度** (ノード毎秒、`auto` でその場で実測)。渡した側
    /// だけ、読切に入る空きを残り時間から逆算する。**片方だけに渡せば
    /// 「較正あり 対 なし」を測れる** — 較正の効果はこれで見る。
    nps_a: Option<String>,
    nps_b: Option<String>,
    /// 片側だけの持ち時間 (省略時は `--time`)。
    time_a: Option<u64>,
    time_b: Option<u64>,
    /// 探索のスレッド数 (両側同じ)。較正した nps はスレッド数ごとの値
    /// なので、測ったときと同じ数で戦わせないと意味がない。
    threads: usize,
    /// 定石を使う。**実戦は使うのが既定**なので、時間の使われ方を測るなら
    /// 揃えないと序盤の数手ぶんずれる。
    use_book: bool,
    weights_a: PathBuf,
    weights_b: PathBuf,
    games: usize,
    random_plies: usize,
    depth: u8,
    solve_empties: u8,
    /// 選択読みの帯: `None` は予算から自動、`Some(n)` は固定 (旧挙動との比較用)。
    band_a: Option<u8>,
    band_b: Option<u8>,
    /// 残り手数を数える読切の基準 (`timectl::Situation::solve_ref`)。
    solve_ref_a: Option<SolveRef>,
    solve_ref_b: Option<SolveRef>,
    /// 持ち時間をどれだけ攻めて使うか (`timectl::Situation::budget_use`)。
    budget_use_a: Option<f64>,
    budget_use_b: Option<f64>,
    /// Per-side overrides; fall back to the shared values above.
    depth_a: Option<u8>,
    depth_b: Option<u8>,
    solve_a: Option<u8>,
    solve_b: Option<u8>,
    patterns: &'static str,
    patterns_a: Option<&'static str>,
    patterns_b: Option<&'static str>,
    seed: u64,
    mpc_a: bool,
    mpc_b: bool,
    mpc_t: f32,
}

fn parse_patterns(v: &str) -> Result<&'static str, String> {
    match v {
        "egaroucid" => Ok("egaroucid"),
        "egaroucid-plus" => Ok("egaroucid-plus"),
        "edax" => Ok("edax"),
        other => Err(format!("unknown pattern set: {other}")),
    }
}

const USAGE: &str = "\
Usage: arena --a <weights-A> --b <weights-B> [OPTIONS]

Play two weight files head-to-head (1-ply greedy both sides) and report
A's win rate with a 95% confidence interval. Each random opening is played
twice with colors swapped.

Options:
  --games <n>          Total games (default 1000, rounded up to even)
  --random-plies <n>   Random opening plies for diversity (default 6)
  --depth <n>          Search depth for both sides; 1 = greedy (default 1)
  --solve-empties <n>  Both sides play the endgame perfectly from n empties
                       (default 0 = off; matches real play conditions)
  --depth-a <n>        Search depth for A only (default: --depth)
  --depth-b <n>        Search depth for B only (default: --depth)
  --solve-a <n>        A's perfect-endgame threshold (default: --solve-empties)
  --solve-b <n>        B's perfect-endgame threshold (default: --solve-empties)
  --patterns <set>     egaroucid | edax | egaroucid-plus (default egaroucid)
  --patterns-a <set>   A's pattern library (default: --patterns)
  --patterns-b <set>   B's pattern library (default: --patterns)
  --seed <n>           RNG seed (default 7)
  --threads <n>        Search threads for both sides (default 1)
  -h, --help           Show this help

Timed play (the only way to measure time management):
  --book               Use the opening book (matches real play; off by default
                       so evaluator comparisons are not masked by book moves)
  --time <secs>        Whole-game clock per side; running out loses the game
  --pace-a <mode>      fast | depth | tail:<a> (default fast). slow/even were
                       dropped: slow lost 0.0% at 3s and 8s clocks, and fast
                       was never worse than even. tail:<a> still spans them
  --pace-b <mode>      same, for B
  --nps-a <n|auto>     Calibrated solve speed: the side that gets it derives
                       its exact-solve entry from the clock left instead of a
                       fixed staircase. `auto` measures it here. Give it to
                       one side only to measure what calibration is worth.
  --nps-b <n|auto>     same, for B
  --time-a <secs>      A's clock only (default: --time). Pit a short clock
                       against a long one to measure what the time limit
                       itself costs — comparing pacing modes cannot show it.
  --time-b <secs>      same, for B
  --budget-use-a <x>   How hard A spends its clock (default 2.5)
  --budget-use-b <x>   same, for B
  --solve-ref-a <auto|n>  Denominator for the moves-left estimate: (empties-n)/2.
                       auto derives it from the calibrated solve speed
  --solve-ref-b <n>    same, for B
  --band-a <auto|n>    Selective-read width: auto derives it from the move
                       budget (default), a number pins it the old way
  --band-b <auto|n>    same, for B";

fn parse_args() -> Result<Args, String> {
    let mut weights_a = None;
    let mut weights_b = None;
    let mut args = Args {
        weights_a: PathBuf::new(),
        weights_b: PathBuf::new(),
        games: 1000,
        random_plies: 6,
        depth: 1,
        solve_empties: 0,
        band_a: None,
        solve_ref_a: None,
        solve_ref_b: None,
        budget_use_a: None,
        budget_use_b: None,
        band_b: None,
        nnue_a: None,
        nnue_b: None,
        time_secs: 0,
        pace_a: "fast".into(),
        pace_b: "fast".into(),
        nps_a: None,
        nps_b: None,
        time_a: None,
        time_b: None,
        threads: 1,
        use_book: false,
        depth_a: None,
        depth_b: None,
        solve_a: None,
        solve_b: None,
        patterns: "egaroucid",
        patterns_a: None,
        patterns_b: None,
        seed: 7,
        mpc_a: false,
        mpc_b: false,
        mpc_t: 1.1,
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} requires a value"));
        match arg.as_str() {
            "--a" => weights_a = Some(PathBuf::from(value("--a")?)),
            "--b" => weights_b = Some(PathBuf::from(value("--b")?)),
            "--games" => {
                args.games = value("--games")?
                    .parse()
                    .map_err(|e| format!("--games: {e}"))?
            }
            "--random-plies" => {
                args.random_plies = value("--random-plies")?
                    .parse()
                    .map_err(|e| format!("--random-plies: {e}"))?
            }
            "--depth" => {
                args.depth = value("--depth")?
                    .parse()
                    .map_err(|e| format!("--depth: {e}"))?
            }
            "--time" => {
                args.time_secs = value("--time")?
                    .parse()
                    .map_err(|e| format!("--time: {e}"))?
            }
            "--pace-a" => args.pace_a = value("--pace-a")?.to_string(),
            "--pace-b" => args.pace_b = value("--pace-b")?.to_string(),
            // `auto` は実測 (エンジンを作ってから測るので後で埋める)
            "--time-a" => {
                args.time_a = Some(
                    value("--time-a")?
                        .parse()
                        .map_err(|e| format!("--time-a: {e}"))?,
                )
            }
            "--time-b" => {
                args.time_b = Some(
                    value("--time-b")?
                        .parse()
                        .map_err(|e| format!("--time-b: {e}"))?,
                )
            }
            "--nps-a" => args.nps_a = Some(value("--nps-a")?.to_string()),
            "--nps-b" => args.nps_b = Some(value("--nps-b")?.to_string()),
            "--book" => args.use_book = true,
            "--threads" => {
                args.threads = value("--threads")?
                    .parse()
                    .map_err(|e| format!("--threads: {e}"))?
            }
            "--nnue-a" => {
                args.nnue_a = Some(PathBuf::from(value("--nnue-a")?));
            }
            "--nnue-b" => {
                args.nnue_b = Some(PathBuf::from(value("--nnue-b")?));
            }
            "--budget-use-a" => {
                args.budget_use_a = Some(
                    value("--budget-use-a")?
                        .parse()
                        .map_err(|_| "--budget-use-a wants a number")?,
                );
            }
            "--budget-use-b" => {
                args.budget_use_b = Some(
                    value("--budget-use-b")?
                        .parse()
                        .map_err(|_| "--budget-use-b wants a number")?,
                );
            }
            "--solve-ref-a" => {
                args.solve_ref_a = Some(SolveRef::parse(&value("--solve-ref-a")?));
            }
            "--solve-ref-b" => {
                args.solve_ref_b = Some(SolveRef::parse(&value("--solve-ref-b")?));
            }
            "--band-a" => {
                let v = value("--band-a")?;
                args.band_a = if v == "auto" {
                    None
                } else {
                    Some(v.parse().map_err(|_| "--band-a wants auto or a number")?)
                };
            }
            "--band-b" => {
                let v = value("--band-b")?;
                args.band_b = if v == "auto" {
                    None
                } else {
                    Some(v.parse().map_err(|_| "--band-b wants auto or a number")?)
                };
            }
            "--solve-empties" => {
                args.solve_empties = value("--solve-empties")?
                    .parse()
                    .map_err(|e| format!("--solve-empties: {e}"))?
            }
            "--depth-a" => {
                args.depth_a = Some(
                    value("--depth-a")?
                        .parse()
                        .map_err(|e| format!("--depth-a: {e}"))?,
                )
            }
            "--depth-b" => {
                args.depth_b = Some(
                    value("--depth-b")?
                        .parse()
                        .map_err(|e| format!("--depth-b: {e}"))?,
                )
            }
            "--solve-a" => {
                args.solve_a = Some(
                    value("--solve-a")?
                        .parse()
                        .map_err(|e| format!("--solve-a: {e}"))?,
                )
            }
            "--solve-b" => {
                args.solve_b = Some(
                    value("--solve-b")?
                        .parse()
                        .map_err(|e| format!("--solve-b: {e}"))?,
                )
            }
            "--patterns" => args.patterns = parse_patterns(&value("--patterns")?)?,
            "--patterns-a" => args.patterns_a = Some(parse_patterns(&value("--patterns-a")?)?),
            "--patterns-b" => args.patterns_b = Some(parse_patterns(&value("--patterns-b")?)?),
            "--seed" => {
                args.seed = value("--seed")?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            "--mpc-a" => args.mpc_a = true,
            "--mpc-b" => args.mpc_b = true,
            "--mpc-t" => {
                args.mpc_t = value("--mpc-t")?
                    .parse()
                    .map_err(|e| format!("--mpc-t: {e}"))?
            }
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown option: {other}\n\n{USAGE}")),
        }
    }

    // NNUE モードでは線形の重みは終盤の並べ替えにしか使わないので、
    // 1 本を既定で共有する (両者で同じものを使うため差の原因にならない)
    if args.nnue_a.is_some() && args.nnue_b.is_some() {
        args.weights_a = weights_a.unwrap_or_else(|| PathBuf::from("weights/linear.bin"));
        args.weights_b = weights_b.unwrap_or_else(|| args.weights_a.clone());
    } else {
        args.weights_a = weights_a.ok_or(format!("--a is required\n\n{USAGE}"))?;
        args.weights_b = weights_b.ok_or(format!("--b is required\n\n{USAGE}"))?;
    }
    args.games = args.games.div_ceil(2) * 2;
    Ok(args)
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }
}

fn nth_move(mut mask: u64, mut n: u32) -> Position {
    while n > 0 {
        mask &= mask - 1;
        n -= 1;
    }
    Position::from_index(mask.trailing_zeros()).unwrap()
}

fn greedy_move(board: &Board, evaluator: &Evaluator) -> Position {
    let moves = board.movable();
    let mut best_pos = None;
    let mut best_val = f32::NEG_INFINITY;
    let mut m = moves;
    while m != 0 {
        let pos = Position::from_index(m.trailing_zeros()).unwrap();
        m &= m - 1;
        let mut child = *board;
        child.make_move_bits(pos);
        let val = -evaluator.eval(&child);
        if val > best_val {
            best_val = val;
            best_pos = Some(pos);
        }
    }
    best_pos.expect("at least one legal move")
}

/// Generate a random opening position with `plies` random moves.
fn random_opening(rng: &mut Rng, plies: usize) -> Board {
    let mut board = Board::new();
    for _ in 0..plies {
        let moves = board.movable();
        if moves == 0 {
            break;
        }
        board.make_move_bits(nth_move(moves, rng.below(moves.count_ones())));
    }
    board
}

/// One side's playing strength: evaluator, search depth (1 = greedy) and
/// the empties threshold from which it plays perfect endgame moves (0 = off).
struct EngineConfig<'a> {
    evaluator: &'a Evaluator,
    depth: u8,
    solve_empties: u8,
}

/// Play out one game from `start`; each side picks moves per its own
/// [`EngineConfig`]. Returns final score from Black's view.
///
/// Each engine uses its OWN searcher: transposition-table entries embed
/// evaluator-specific values, so sharing one table across engines would
/// let one engine consume the other's evaluations and silently blend them.
/// The solver IS shared: exact endgame values are evaluator-independent.
fn play(
    start: &Board,
    black: &EngineConfig,
    white: &EngineConfig,
    black_searcher: &mut Searcher,
    white_searcher: &mut Searcher,
    solver: &mut Solver,
) -> i32 {
    // Once BOTH sides are inside their perfect-play windows the outcome
    // is fixed; resolve it with one solver call instead of playing on.
    let both_perfect_from = black.solve_empties.min(white.solve_empties);

    let mut board = *start;
    loop {
        if board.movable() == 0 {
            let mut passed = board;
            passed.pass();
            if passed.movable() == 0 {
                let s = board.score();
                return if board.player() == Color::Black {
                    s
                } else {
                    -s
                };
            }
            board = passed;
            continue;
        }

        if both_perfect_from > 0 && board.empty_count() <= both_perfect_from {
            let result =
                solver.solve_with_eval(EndSolverMode::Perfect, &board, Some(black.evaluator));
            let s = result.value;
            return if board.player() == Color::Black {
                s
            } else {
                -s
            };
        }

        let is_black = board.player() == Color::Black;
        let cfg = if is_black { black } else { white };
        let pos = if cfg.solve_empties > 0 && board.empty_count() <= cfg.solve_empties {
            solver
                .solve_with_eval(EndSolverMode::Perfect, &board, Some(cfg.evaluator))
                .best_move
                .expect("legal move exists")
        } else if cfg.depth <= 1 {
            greedy_move(&board, cfg.evaluator)
        } else {
            let searcher = if is_black {
                &mut *black_searcher
            } else {
                &mut *white_searcher
            };
            searcher
                .search(&board, cfg.evaluator, cfg.depth)
                .best_move
                .expect("legal move exists")
        };
        board.make_move_bits(pos);
    }
}

/// NNUE 同士を **実戦そのもの** (`Engine::choose`) で戦わせる。
///
/// `Searcher` を使う線形の経路とは別に持つ。線形の `Evaluator` と NNUE は
/// 探索の入口から違い (`NnueSearch` / 帯 / MPC)、`Engine` を通さないと
/// 「実戦条件で測る」(CLAUDE.md 3) を満たせないため。
///
/// **定石は切る。** 両者が同じ定石を引くと序盤が完全に同一になり、
/// 評価関数の差が出るのは定石を外れた後だけになる。
fn play_nnue(start: &Board, black: &mut Engine, white: &mut Engine) -> i32 {
    let mut board = *start;
    loop {
        if board.movable() == 0 {
            let mut passed = board;
            passed.pass();
            if passed.movable() == 0 {
                let s = board.score();
                return if board.player() == Color::Black {
                    s
                } else {
                    -s
                };
            }
            board = passed;
            continue;
        }
        let is_black = board.player() == Color::Black;
        let eng = if is_black { &mut *black } else { &mut *white };
        let pos = eng.choose(&board).pos.expect("legal move exists");
        board.make_move_bits(pos);
    }
}

/// **持ち時間制**で戦わせる。時間配分の良し悪しはこれでしか測れない。
///
/// 各手で `timectl::plan` に残り時間と空きを渡して 1 手の期限を決め、
/// `choose_within` に渡す。**使った時間はその場の実測**なので、配り方が
/// 下手なら本当に時間切れになる。
///
/// 時間切れは**負け**として扱う (GGS と同じ)。石差ではなく勝敗で数える
/// ので、戻り値は「黒から見た石差」ではなく `Option` — `None` が時間切れ。
struct Clocks {
    black: f64,
    white: f64,
}

/// 1 局の結果。**残り時間まで返す。**
///
/// 時間切れの件数だけでは配り方を直せない。「余らせる前提」で組むなら
/// **どれだけ余ったか**が要る — 余りすぎなら読む時間を増やせるし、
/// 余りが薄いなら遅い機械で破綻する。
struct Timed {
    /// 黒から見た石差 (時間切れなら 0)。
    score: i32,
    /// 時間切れした側。
    timeout: Option<Color>,
    /// 終局時の残り時間 (秒)。
    left_black: f64,
    left_white: f64,
}

/// 空きごとに「何段読めたか・どこから読み切ったか」を出す。
///
/// **持ち時間ごとの実力はこれで見る。** 勝率は相手との相対でしか出ないが、
/// こちらは「この持ち時間なら空き 40 で 26 段、空き 26 から読切」と
/// 絶対値で言える。レベル設定 (深さ・読切) が実際に届いているかも分かる。
fn report_moves(moves: &[MoveInfo]) {
    if moves.is_empty() {
        return;
    }
    println!("空きごとの到達 (両者ぶん、定石の手は除く)");
    /* **中央値だけでは足りない。** 同じ空きでも局面によって使える時間が
    違うので、「最大どこまで読めたか」と「時間が無いとき何段まで落ちたか」の
    両端が要る。落ちた側がその持ち時間の実力の下限になる。 */
    println!("   空き | 手数 | 中盤の深さ 最浅/中央/最深 | 読切 | 打切 | 1 手の秒 中央/最大");
    println!("  ------+------+---------------------------+------+------+-------------------");
    for lo in (0..=60usize).rev().filter(|n| n % 4 == 0) {
        let hi = lo + 3;
        let grp: Vec<&MoveInfo> = moves
            .iter()
            .filter(|m| !m.from_book && (m.empties as usize) >= lo && (m.empties as usize) <= hi)
            .collect();
        if grp.is_empty() {
            continue;
        }
        let mut ds: Vec<u32> = grp.iter().filter(|m| !m.exact).map(|m| m.depth).collect();
        ds.sort_unstable();
        let mut ts: Vec<f64> = grp.iter().map(|m| m.secs).collect();
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let solved = grp.iter().filter(|m| m.exact).count();
        let dtxt = if ds.is_empty() {
            "—".to_string()
        } else {
            format!(
                "{:>2} / {:>2} / {:>2}",
                ds[0],
                ds[ds.len() / 2],
                ds[ds.len() - 1]
            )
        };
        let cut = grp.iter().filter(|m| m.cut).count();
        println!(
            "  {lo:>2}-{hi:<2} | {:>4} | {dtxt:>25} | {:>3}% | {:>4} | {:>8.1} / {:>6.1}",
            grp.len(),
            solved * 100 / grp.len(),
            cut,
            ts[ts.len() / 2],
            ts[ts.len() - 1],
        );
    }
    let book = moves.iter().filter(|m| m.from_book).count();
    let cut = moves.iter().filter(|m| m.cut).count();
    println!("  (定石で指した手 {book} / 読切を打ち切った手 {cut})");
}

/// 1 手ぶんの記録。**持ち時間ごとの実力を見るための計器。**
#[derive(Clone, Copy)]
struct MoveInfo {
    empties: u8,
    /// 中盤探索が到達した深さ (読切と定石では 0)。
    depth: u32,
    /// 読切で決めたか。
    exact: bool,
    /// 読切が期限で打ち切られ、保険の手を指したか。
    cut: bool,
    from_book: bool,
    secs: f64,
}

/// 片側の時間の使い方。**配り方と較正値は組で入れ替える** — 色を交換した
/// 再戦で片方だけ付け替えると、測っているものが変わってしまう。
#[derive(Clone, Copy)]
struct SideTime {
    pace: Pace,
    /// 選択読みの帯を予算から決めるか。偽なら `band` をそのまま使う。
    auto_band: bool,
    /// 較正した読切速度。`None` なら固定の階段で読切に入る。
    nps: Option<f64>,
    /// 残り手数を数える読切の基準。`None` は `timectl` の既定。
    solve_ref: Option<SolveRef>,
    /// 持ち時間をどれだけ攻めて使うか。`None` は `timectl` の既定。
    budget_use: Option<f64>,
    /// **この側の持ち時間** (秒)。片側だけ潤沢にできる。
    ///
    /// **「時間制限で棋力を削っていないか」はこれでしか測れない。** 配り方
    /// どうしを比べても「どの配り方でも同じ」しか出ず、時間制限そのものの
    /// 損は見えない。潤沢な側に勝てるなら、その持ち時間では飽和している。
    total: f64,
}

fn play_timed(
    start: &Board,
    black: &mut Engine,
    white: &mut Engine,
    black_time: SideTime,
    white_time: SideTime,
    threads: usize,
    moves: &mut Vec<MoveInfo>,
) -> Timed {
    let mut board = *start;
    let mut clocks = Clocks {
        black: black_time.total,
        white: white_time.total,
    };
    loop {
        if board.movable() == 0 {
            let mut passed = board;
            passed.pass();
            if passed.movable() == 0 {
                let s = board.score();
                let v = if board.player() == Color::Black {
                    s
                } else {
                    -s
                };
                return Timed {
                    score: v,
                    timeout: None,
                    left_black: clocks.black,
                    left_white: clocks.white,
                };
            }
            board = passed;
            continue;
        }
        let is_black = board.player() == Color::Black;
        let left = if is_black { clocks.black } else { clocks.white };
        if left <= 0.0 {
            // 時間切れ。指した側の負け
            return Timed {
                score: 0,
                timeout: Some(board.player()),
                left_black: clocks.black.max(0.0),
                left_white: clocks.white.max(0.0),
            };
        }
        let t = if is_black { black_time } else { white_time };
        let eng = if is_black { &mut *black } else { &mut *white };
        let base = Levels {
            depth: eng.config().depth,
            solve: eng.config().solve_empties,
            band: eng.config().band,
            auto_band: t.auto_band,
        };
        let plan = timectl::plan(
            Situation {
                clock_secs: Some(left as u64),
                budget_use: t.budget_use.unwrap_or(Situation::default().budget_use),
                in_overtime: false,
                grace_secs: 0,
                empties: board.empty_count(),
                max_move_secs: 0,
                reserve_secs: 20,
                nps: t.nps,
                threads,
                solve_ref: t.solve_ref.unwrap_or(Situation::default().solve_ref),
            },
            base,
            t.pace,
        );
        /* 計画した深さ・読切・帯を**その手だけ**効かせる。

        **戻すのを忘れると設定が毎手削られる。** 次の手の `base` は
        `eng.config()` から読むので、書きっぱなしにすると計画の結果が
        次の計画の入力になる。読切の入り口を残り時間から決める側は
        `solve_entry` の上限が `base.solve` なので、**ラチェットのように
        下がり続けて 0 に落ちた** (10 秒の対局で勝率 7%)。階段側は
        `min(14)` で止まるため無傷で、片側だけ壊れて差に見えていた。 */
        eng.set_levels(plan.depth, plan.solve, plan.band);
        let t0 = Instant::now();
        let mv = match plan.cap {
            Some(cap) => eng.choose_within(&board, Some(t0 + cap)),
            None => eng.choose(&board),
        };
        let pos = mv.pos.expect("legal move exists");
        let used = t0.elapsed().as_secs_f64();
        /* **その手が何をしたかを控える。** 「この持ち時間はどれくらいの
        棋力で打てているか」は、勝率ではなく**空きごとに何段読めたか・
        どこから読み切れたか**で見るのが直接的。 */
        moves.push(MoveInfo {
            empties: board.empty_count(),
            depth: mv.depth,
            exact: mv.exact,
            cut: mv.cut,
            from_book: mv.from_book,
            secs: used,
        });
        eng.set_levels(base.depth, base.solve, base.band);
        if is_black {
            clocks.black -= used;
        } else {
            clocks.white -= used;
        }
        board.make_move_bits(pos);
    }
}

/// NNUE モードの本体。開局・先後入れ替え・1 局ごとの表クリア・統計は
/// 線形側と同じ形にしてある (結果を並べて読めるように)。
fn run_nnue(args: &Args, a: &std::path::Path, b: &std::path::Path) -> ExitCode {
    let mk = |nnue: &std::path::Path, depth: u32, solve: u8, band: u8| -> Result<Engine, String> {
        Engine::new(EngCfg {
            depth,
            solve_empties: solve,
            band,
            threads: args.threads,
            mpc: true,
            midgame_hash_bits: 22,
            solver_hash_bits: 20,
            weights: args.weights_a.clone(),
            nnue: nnue.to_path_buf(),
            book: args.weights_a.with_file_name("book.txt"),
            use_book: args.use_book,
            // 同じ棋譜の繰り返しを避ける (実戦と同じ)
            book_tolerance: if args.use_book { 1.0 } else { 0.0 },
        })
    };
    let depth_a = u32::from(args.depth_a.unwrap_or(args.depth));
    let depth_b = u32::from(args.depth_b.unwrap_or(args.depth));
    let solve_a = args.solve_a.unwrap_or(args.solve_empties);
    let solve_b = args.solve_b.unwrap_or(args.solve_empties);
    let (mut eng_a, mut eng_b) = match (
        mk(a, depth_a, solve_a, args.band_a.unwrap_or(0)),
        mk(b, depth_b, solve_b, args.band_b.unwrap_or(0)),
    ) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    eng_a.set_use_book(args.use_book);
    eng_b.set_use_book(args.use_book);

    // 片側だけ指定したときも持ち時間制に入る (`--time` の指定は要らない)
    let timed = args.time_secs > 0 || args.time_a.is_some() || args.time_b.is_some();
    let (pace_a, pace_b) = (Pace::parse(&args.pace_a), Pace::parse(&args.pace_b));
    /* `--nps-* auto` はここで実測する。**A と B で別々に測らない** — 同じ
    機械の同じスレッド数なので値は同じで、片側だけ 2 回測ると熱の分だけ
    ずれ、較正の効果と区別がつかなくなる。 */
    let mut measured: Option<f64> = None;
    let mut resolve = |v: &Option<String>, eng: &mut Engine| -> Option<f64> {
        match v.as_deref() {
            None => None,
            Some("auto") => Some(*measured.get_or_insert_with(|| {
                let n = eng.measure_solve_nps();
                println!(
                    "較正: 読切 {:.1}M ノード/秒 ({} スレッド)",
                    n / 1e6,
                    args.threads
                );
                n
            })),
            Some(s) => s.parse().ok(),
        }
    };
    let side_a = SideTime {
        pace: pace_a,
        auto_band: args.band_a.is_none(),
        solve_ref: args.solve_ref_a,
        budget_use: args.budget_use_a,
        nps: resolve(&args.nps_a, &mut eng_a),
        total: args.time_a.unwrap_or(args.time_secs) as f64,
    };
    let side_b = SideTime {
        pace: pace_b,
        auto_band: args.band_b.is_none(),
        solve_ref: args.solve_ref_b,
        budget_use: args.budget_use_b,
        nps: resolve(&args.nps_b, &mut eng_b),
        total: args.time_b.unwrap_or(args.time_secs) as f64,
    };
    if timed {
        // 較正の有無は結果の解釈を変えるので、見出しに出す
        let tag = |s: &SideTime| {
            format!(
                "{}s, {}{}",
                s.total,
                s.pace.as_str(),
                if s.nps.is_some() {
                    ", 較正あり"
                } else {
                    ""
                }
            )
        };
        println!(
            "arena (持ち時間制, {} スレッド): A={} ({}) vs B={} ({})  ({} 局, 開局 {} 手ランダム, 定石なし)",
            args.threads,
            a.display(),
            tag(&side_a),
            b.display(),
            tag(&side_b),
            args.games,
            args.random_plies,
        );
    }
    if !timed {
        println!(
        "arena (NNUE): A={} (depth {depth_a}, solve {solve_a}) vs B={} (depth {depth_b}, solve {solve_b})  ({} games, {} random plies, book off)",
        a.display(),
        b.display(),
        args.games,
        args.random_plies,
    );
    }

    let mut rng = Rng::new(args.seed);
    let (mut a_wins, mut b_wins, mut draws) = (0usize, 0usize, 0usize);
    let mut a_disc_sum = 0i64;
    // 時間切れは配り方の失敗そのものなので別に数える
    let (mut a_timeouts, mut b_timeouts) = (0usize, 0usize);
    /* **終局時に余った時間。** 配り方は「使い切らず余らせる」前提で組むので、
    余りの平均と最小を出す。平均は「読む時間をまだ増やせるか」を、最小は
    「遅い機械で破綻しないか」を見るためのもの。 */
    // 空きごとに何段読めたか (両者ぶんまとめて。同じ設定なので分ける意味が薄い)
    let mut moves: Vec<MoveInfo> = Vec::new();
    let (mut a_left_sum, mut b_left_sum) = (0f64, 0f64);
    let (mut a_left_min, mut b_left_min) = (f64::MAX, f64::MAX);
    let mut left_n = 0usize;
    for pair in 0..args.games / 2 {
        let opening = random_opening(&mut rng, args.random_plies);
        // 1 局ごとに表を捨てる (CLAUDE.md 4)。温まった表を持ち越すと
        // 色を入れ替えた再戦が鏡にならない
        eng_a.clear_tables();
        eng_b.clear_tables();
        // 1 局目: A が黒
        let r1 = if timed {
            play_timed(
                &opening,
                &mut eng_a,
                &mut eng_b,
                side_a,
                side_b,
                args.threads,
                &mut moves,
            )
        } else {
            Timed {
                score: play_nnue(&opening, &mut eng_a, &mut eng_b),
                timeout: None,
                left_black: 0.0,
                left_white: 0.0,
            }
        };
        // 1 局目は A が黒
        if timed {
            a_left_sum += r1.left_black;
            b_left_sum += r1.left_white;
            a_left_min = a_left_min.min(r1.left_black);
            b_left_min = b_left_min.min(r1.left_white);
            left_n += 1;
        }
        let (s1, to1) = (r1.score, r1.timeout);
        match to1 {
            // 時間切れは負け。石差は数えない (勝敗だけが意味を持つ)
            Some(Color::Black) => {
                b_wins += 1;
                a_timeouts += 1;
            }
            Some(Color::White) => {
                a_wins += 1;
                b_timeouts += 1;
            }
            None => {
                a_disc_sum += s1 as i64;
                match s1.cmp(&0) {
                    std::cmp::Ordering::Greater => a_wins += 1,
                    std::cmp::Ordering::Less => b_wins += 1,
                    std::cmp::Ordering::Equal => draws += 1,
                }
            }
        }
        eng_a.clear_tables();
        eng_b.clear_tables();
        // 2 局目: 色を入れ替える
        let r2 = if timed {
            play_timed(
                &opening,
                &mut eng_b,
                &mut eng_a,
                side_b,
                side_a,
                args.threads,
                &mut moves,
            )
        } else {
            Timed {
                score: play_nnue(&opening, &mut eng_b, &mut eng_a),
                timeout: None,
                left_black: 0.0,
                left_white: 0.0,
            }
        };
        // 2 局目は B が黒
        if timed {
            b_left_sum += r2.left_black;
            a_left_sum += r2.left_white;
            b_left_min = b_left_min.min(r2.left_black);
            a_left_min = a_left_min.min(r2.left_white);
            left_n += 1;
        }
        let (s2, to2) = (r2.score, r2.timeout);
        match to2 {
            Some(Color::Black) => {
                a_wins += 1;
                b_timeouts += 1;
            }
            Some(Color::White) => {
                b_wins += 1;
                a_timeouts += 1;
            }
            None => {
                a_disc_sum -= s2 as i64;
                match s2.cmp(&0) {
                    std::cmp::Ordering::Greater => b_wins += 1,
                    std::cmp::Ordering::Less => a_wins += 1,
                    std::cmp::Ordering::Equal => draws += 1,
                }
            }
        }
        // 長く回るので途中経過を出す (400 局で数時間規模)
        if (pair + 1) % 10 == 0 {
            let n = (a_wins + b_wins + draws) as f64;
            let p = (a_wins as f64 + draws as f64 / 2.0) / n;
            println!("  [{}/{}] A {:.1}%", n as usize, args.games, p * 100.0);
        }
    }
    if timed {
        println!("時間切れ: A {a_timeouts} / B {b_timeouts}");
        report_moves(&moves);
        if left_n > 0 {
            println!(
                "終局時の残り時間 (持ち時間 A {}s / B {}s): A 平均 {:.1}s 最小 {:.1}s / B 平均 {:.1}s 最小 {:.1}s",
                side_a.total,
                side_b.total,
                a_left_sum / left_n as f64,
                a_left_min,
                b_left_sum / left_n as f64,
                b_left_min,
            );
        }
    }
    report(a_wins, b_wins, draws, a_disc_sum);
    ExitCode::SUCCESS
}

/// 勝率と 95% 信頼区間。引き分けは 0.5 勝で数える。
fn report(a_wins: usize, b_wins: usize, draws: usize, a_disc_sum: i64) {
    let n = (a_wins + b_wins + draws) as f64;
    let p = (a_wins as f64 + draws as f64 / 2.0) / n;
    let se = (p * (1.0 - p) / n).sqrt();
    let (lo, hi) = (p - 1.96 * se, p + 1.96 * se);
    println!("A wins {a_wins}, B wins {b_wins}, draws {draws}");
    println!(
        "A score {:.1}%  (95% CI {:.1}%..{:.1}%)  mean disc diff {:+.2}",
        p * 100.0,
        lo * 100.0,
        hi * 100.0,
        a_disc_sum as f64 / n
    );
    if lo > 0.5 {
        println!("=> A is significantly stronger");
    } else if hi < 0.5 {
        println!("=> B is significantly stronger");
    } else {
        println!("=> no significant difference");
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    // 両方そろったときだけ NNUE モード (片方だけの指定は取り違えのもと)
    match (&args.nnue_a, &args.nnue_b) {
        (Some(a), Some(b)) => return run_nnue(&args, &a.clone(), &b.clone()),
        (Some(_), None) | (None, Some(_)) => {
            eprintln!("--nnue-a と --nnue-b は両方まとめて指定してください");
            return ExitCode::FAILURE;
        }
        (None, None) => {}
    }

    let lib = |name: &str| match name {
        "edax" => EDAX_PATTERNS,
        "egaroucid-plus" => EGAROUCID_PLUS_PATTERNS,
        _ => EGAROUCID_PATTERNS,
    };
    let patterns_a = lib(args.patterns_a.unwrap_or(args.patterns));
    let patterns_b = lib(args.patterns_b.unwrap_or(args.patterns));

    let mut eval_a = Evaluator::new(patterns_a);
    if let Err(e) = eval_a.load_weights(&args.weights_a) {
        eprintln!("failed to load {}: {e}", args.weights_a.display());
        return ExitCode::FAILURE;
    }
    let mut eval_b = Evaluator::new(patterns_b);
    if let Err(e) = eval_b.load_weights(&args.weights_b) {
        eprintln!("failed to load {}: {e}", args.weights_b.display());
        return ExitCode::FAILURE;
    }

    let cfg_a = EngineConfig {
        evaluator: &eval_a,
        depth: args.depth_a.unwrap_or(args.depth),
        solve_empties: args.solve_a.unwrap_or(args.solve_empties),
    };
    let cfg_b = EngineConfig {
        evaluator: &eval_b,
        depth: args.depth_b.unwrap_or(args.depth),
        solve_empties: args.solve_b.unwrap_or(args.solve_empties),
    };

    println!(
        "arena: A={} (depth {}, solve {}) vs B={} (depth {}, solve {})  ({} games, {} random plies)",
        args.weights_a.display(),
        cfg_a.depth,
        cfg_a.solve_empties,
        args.weights_b.display(),
        cfg_b.depth,
        cfg_b.solve_empties,
        args.games,
        args.random_plies,
    );

    let mut rng = Rng::new(args.seed);
    // One searcher per evaluator: TT values are evaluator-specific
    let mut searcher_a = Searcher::new(17);
    let mut searcher_b = Searcher::new(17);
    searcher_a.mpc = args.mpc_a;
    searcher_b.mpc = args.mpc_b;
    searcher_a.mpc_t = args.mpc_t;
    searcher_b.mpc_t = args.mpc_t;
    let mut solver = Solver::new(18);
    let mut a_wins = 0usize;
    let mut b_wins = 0usize;
    let mut draws = 0usize;
    let mut a_disc_sum = 0i64;

    for _pair in 0..args.games / 2 {
        let opening = random_opening(&mut rng, args.random_plies);

        // Fresh TTs per game: with warm tables, cached deeper entries from
        // earlier games change move choices, so the color-swapped replay of
        // the same opening is no longer a mirror — measured as A scoring
        // ~42% in a same-weights self-match. Cold TTs make play a pure
        // deterministic function of (opening, weights, depth): a
        // same-weights match is exactly 50% by construction.
        searcher_a.clear();
        searcher_b.clear();

        // Game 1: A plays Black
        let s1 = play(
            &opening,
            &cfg_a,
            &cfg_b,
            &mut searcher_a,
            &mut searcher_b,
            &mut solver,
        );
        a_disc_sum += s1 as i64;
        match s1.cmp(&0) {
            std::cmp::Ordering::Greater => a_wins += 1,
            std::cmp::Ordering::Less => b_wins += 1,
            std::cmp::Ordering::Equal => draws += 1,
        }

        // Game 2: colors swapped (searcher stays with its evaluator)
        searcher_a.clear();
        searcher_b.clear();
        let s2 = play(
            &opening,
            &cfg_b,
            &cfg_a,
            &mut searcher_b,
            &mut searcher_a,
            &mut solver,
        );
        a_disc_sum -= s2 as i64;
        match s2.cmp(&0) {
            std::cmp::Ordering::Greater => b_wins += 1,
            std::cmp::Ordering::Less => a_wins += 1,
            std::cmp::Ordering::Equal => draws += 1,
        }
    }

    report(a_wins, b_wins, draws, a_disc_sum);
    ExitCode::SUCCESS
}
