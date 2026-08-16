//! **NNUE で選択読みの σ を実測する。**
//!
//! `mpc_sigma` は線形評価で測った値を、NNUE でもそのまま使っている
//! (「NNUE のほうが正確だから安全側」という理屈)。**終盤 σ で同じことを
//! して 2 倍過大だった**前例があるので、中盤も実測して確かめる。
//!
//! σ が実際より大きいと枝を刈れず、同じ時間で読める深さが落ちる。
//!
//! 局面ごとに、置換表を消しながら独立に深さを変えて探索し、CSV で
//! 1 行ずつ出す。σ の当てはめは後段 (この出力を読む) で行う。
//!
//! **局面は並列に処理する。** 探索そのものを並列にすると木が変わって
//! しまう (Lazy SMP は同じ深さでも別のノードを踏む) ので、**1 局面 1
//! スレッドで、局面を並べて回す**。測るのは値であって速度ではない。
//!
//! 使い方:
//!   mpccalib_nnue [--threads N] [--stride N] [--max N] [--depths a,b,c]
//!                 <nnue.bin> <linear.bin> <data-file>...

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use kuroobi::evaluator::Evaluator;
use kuroobi::midgame::{NnueSearch, SharedTt};
use kuroobi::nnue::Nnue;
use kuroobi::pattern::EGAROUCID_PATTERNS;
use kuroobi::trainer::load_examples_binary;

fn main() -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut stride = 997usize; // 素数の刻みでファイル順との相関を切る
    let mut max_positions = 2000usize;
    let mut threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let mut depths: Vec<u32> = vec![0, 2, 4, 6, 8, 10];

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--threads" => threads = it.next().and_then(|v| v.parse().ok()).unwrap_or(threads),
            "--stride" => stride = it.next().and_then(|v| v.parse().ok()).unwrap_or(stride),
            "--max" => {
                max_positions = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(max_positions)
            }
            "--depths" => {
                if let Some(v) = it.next() {
                    depths = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                }
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if paths.len() < 3 {
        eprintln!(
            "usage: mpccalib_nnue [--threads N] [--stride N] [--max N] [--depths a,b,c] \
             <nnue.bin> <linear.bin> <data-file>..."
        );
        return ExitCode::FAILURE;
    }
    let nnue_path = paths.remove(0);
    let linear_path = paths.remove(0);

    let mut nn = Nnue::new(EGAROUCID_PATTERNS);
    if let Err(err) = nn.load(&nnue_path) {
        eprintln!("failed to load {}: {err}", nnue_path.display());
        return ExitCode::FAILURE;
    }
    // **忘れると SIMD 経路が未初期化領域を読む。**
    nn.quantize();
    let nn: &'static Nnue = Box::leak(Box::new(nn));

    // 深さ 0 の値だけは線形評価で出す (NNUE の生の出力と同じ石差の単位)
    let mut evaluator = Evaluator::new(EGAROUCID_PATTERNS);
    if let Err(err) = evaluator.load_weights(&linear_path) {
        eprintln!("failed to load {}: {err}", linear_path.display());
        return ExitCode::FAILURE;
    }

    // 局面を先に集める (並列に配るため)
    let mut boards = Vec::new();
    'outer: for path in &paths {
        let examples = match load_examples_binary(path) {
            Ok(ex) => ex,
            Err(err) => {
                eprintln!("failed to load {}: {err}", path.display());
                return ExitCode::FAILURE;
            }
        };
        for ex in examples.iter().step_by(stride) {
            let board = ex.board();
            let empties = 64 - (board.black | board.white).count_ones();
            if !(8..=45).contains(&empties) || board.movable() == 0 {
                continue;
            }
            boards.push(board);
            if boards.len() >= max_positions {
                break 'outer;
            }
        }
    }
    eprintln!("{} 局面を {threads} スレッドで測ります", boards.len());

    print!("empties");
    for d in &depths {
        print!(",d{d}");
    }
    println!();

    let next = AtomicUsize::new(0);
    let rows: Vec<Vec<String>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (next, boards, depths, evaluator) = (&next, &boards, &depths, &evaluator);
                s.spawn(move || {
                    /* **スレッドごとに置換表を持つ。** 共有すると、ある局面の
                    結果が別の局面の探索を助けてしまい、独立に測れない。
                    18 bit は 1 スレッドあたり 4 MB 程度。 */
                    let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(18)));
                    let mut search = NnueSearch::new(nn, tt);
                    search.threads = 1; // 木を変えないため探索は逐次
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(board) = boards.get(i) else { break };
                        let empties = 64 - (board.black | board.white).count_ones();
                        let mut row = format!("{empties}");
                        for &d in depths.iter() {
                            let v = if d == 0 {
                                evaluator.eval(board)
                            } else {
                                tt.clear();
                                let (_, v, _) = search.best_move_deadline(board, d, None);
                                v
                            };
                            row.push_str(&format!(",{v:.3}"));
                        }
                        out.push(row);
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let mut n = 0usize;
    for chunk in rows {
        for r in chunk {
            println!("{r}");
            n += 1;
        }
    }
    eprintln!("{n} positions calibrated");
    ExitCode::SUCCESS
}
