//! GUI・CLI・プロトコル実装が共用するエンジンセッション層。
//!
//! 対局路 (ggs) と同じ選択則 — 中盤 NNUE 探索 / 選択帯 / 完全読み — を
//! 1 つの構造体にまとめ、局面を渡すと着手と評価値を返す。探索は同期実行
//! なので、UI から使う側はワーカースレッドに載せて呼ぶこと。

use std::path::PathBuf;

use crate::book::Book;
use crate::evaluator::Evaluator;
use crate::midgame::{selective_band, NnueSearch, SharedTt, StopHandle};
use crate::nnue::Nnue;
use crate::pattern::EGAROUCID_PATTERNS;
use crate::solver::{final_score, EndSolverMode, Solver};
use crate::{Board, Position};

/// 双方に合法手がない = 終局。
fn is_game_over(board: &Board) -> bool {
    if board.movable() != 0 {
        return false;
    }
    let mut b = *board;
    b.pass();
    b.movable() == 0
}

/// エンジンの探索条件と資源。既定値は GUI・ローカル解析向けの軽めの設定
/// (対局向けの本気設定は呼び出し側で depth / solve_empties を上げる)。
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// 中盤探索の深さ (手数)。
    pub depth: u32,
    /// この空きマス数以下は完全読み。
    pub solve_empties: u8,
    /// 選択帯の幅 (空きマス数)。0 で帯なし。
    pub band: u8,
    /// 探索の並列スレッド数 (中盤・終盤とも)。
    pub threads: usize,
    /// 中盤探索の ProbCut (MPC)。対局・解析とも通常はオン。
    pub mpc: bool,
    /// 中盤共有置換表のサイズ (2^bits エントリ)。
    pub midgame_hash_bits: u32,
    /// 終盤置換表のサイズ (2^bits エントリ)。
    pub solver_hash_bits: u32,
    /// 線形評価の重み (終盤の並べ替えに使用)。
    pub weights: PathBuf,
    /// NNUE の重み (中盤探索・帯 probe に使用)。
    pub nnue: PathBuf,
    /// 定石 book (無ければ book なしで動く)。
    pub book: PathBuf,
    /// book を使うか。研究時に切りたいことがあるのでノブにしてある。
    pub use_book: bool,
    /// book の乱択の許容幅 (石)。最善からこの差以内の手を候補にする。
    /// 0 なら常に最善手 (決定的)。同じ相手と同じ棋譜を繰り返さないための設定。
    pub book_tolerance: f32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            depth: 12,
            solve_empties: 18,
            band: 0,
            threads: 4,
            mpc: true,
            midgame_hash_bits: 22,
            solver_hash_bits: 22,
            weights: PathBuf::from("weights/weights_full.bin"),
            nnue: PathBuf::from("weights/nnue_champion.bin"),
            book: PathBuf::from("weights/book.txt"),
            use_book: true,
            book_tolerance: 1.0,
        }
    }
}

/// 1 手の判断結果。`value` は手番視点の石差 (exact なら厳密値)。
#[derive(Clone, Copy, Debug)]
pub struct MoveEval {
    pub pos: Option<Position>,
    pub value: f32,
    /// 完全読みによる厳密値かどうか。
    pub exact: bool,
    /// 定石 book から返した手かどうか。
    pub from_book: bool,
}

/// 探索一式を束ねたセッション。生成コストが高い (重み読み込み +
/// 置換表確保) ので、プロセスにつき 1 個作って使い回す。
pub struct Engine {
    evaluator: Evaluator,
    search: NnueSearch,
    solver: Solver,
    config: EngineConfig,
    stop: StopHandle,
    book: Option<Book>,
    /// book の乱択用の乱数状態 (起動ごとに変わる)。
    book_rand: u64,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Engine, String> {
        let mut evaluator = Evaluator::new(EGAROUCID_PATTERNS);
        evaluator
            .load_weights(&config.weights)
            .map_err(|e| format!("weights {}: {e}", config.weights.display()))?;
        let mut nn = Nnue::new(EGAROUCID_PATTERNS);
        nn.load(&config.nnue)
            .map_err(|e| format!("nnue {}: {e}", config.nnue.display()))?;
        // int16 テーブルの構築。忘れると eval が未初期化領域を読む。
        nn.quantize();
        // NnueSearch / Solver はプロセス寿命の参照を要求する。Engine 自体も
        // プロセスに 1 個・使い回し前提なのでリークで満たす。
        let nn: &'static Nnue = Box::leak(Box::new(nn));
        let tt: &'static SharedTt = Box::leak(Box::new(SharedTt::new(config.midgame_hash_bits)));
        let mut search = NnueSearch::new(nn, tt);
        search.threads = config.threads;
        search.mpc = config.mpc;
        let mut solver = Solver::new(config.solver_hash_bits);
        solver.set_nnue(nn, tt);
        solver.set_threads(config.threads);
        let stop = StopHandle::new();
        search.set_stop(Some(stop.clone()));
        solver.set_stop(Some(stop.clone()));
        // book は任意 (無くても動く)。深い探索で作った手をそのまま返すので、
        // 実戦の思考時間を序盤で使わずに済む。
        let book = if config.use_book {
            match Book::load(&config.book) {
                Ok(b) if !b.is_empty() => Some(b),
                _ => None,
            }
        } else {
            None
        };
        let book_rand = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e3779b97f4a7c15)
            | 1;
        Ok(Engine {
            evaluator,
            search,
            solver,
            config,
            stop,
            book,
            book_rand,
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// 読み込んだ book の局面数 (0 = book なし)。
    pub fn book_size(&self) -> usize {
        self.book.as_ref().map_or(0, |b| b.len())
    }

    /// 進行中の探索を中断させるためのハンドル (別スレッドから使う)。
    /// 立てた後の結果は不完全なので捨てること。次の探索前に `reset` される。
    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    /// 探索条件 (深さ・完全読み開始・帯) だけを差し替える。置換表と重みは
    /// そのまま。
    pub fn set_levels(&mut self, depth: u32, solve_empties: u8, band: u8) {
        self.config.depth = depth;
        self.config.solve_empties = solve_empties;
        self.config.band = band;
    }

    /// 対局と同じ選択則で着手を決める。合法手がなければ `pos: None`
    /// (パス)。値は手番視点の石差。
    pub fn choose(&mut self, board: &Board) -> MoveEval {
        self.stop.reset();
        // 定石 book: 実戦より深い探索で付けた答えなので、あれば即返す
        if let Some(book) = &self.book {
            let hit = if self.config.book_tolerance > 0.0 {
                // 同一棋譜の反復を避けるため、互角の候補から選ぶ
                self.book_rand = self
                    .book_rand
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                book.probe_varied(board, self.config.book_tolerance, self.book_rand >> 11)
            } else {
                book.probe(board)
            };
            if let Some((pos, value, _depth)) = hit {
                return MoveEval {
                    pos: Some(pos),
                    value,
                    exact: false,
                    from_book: true,
                };
            }
        }
        let c = &self.config;
        if is_game_over(board) {
            // 終局: 探索に聞くと 0 が返る。盤面の石差 (空きマスは勝者へ加算、
            // FFO 規約) をそのまま厳密値として返す
            return MoveEval {
                pos: None,
                value: final_score(board) as f32,
                exact: true,
                from_book: false,
            };
        }
        if board.empty_count() <= c.solve_empties {
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            MoveEval {
                pos: r.best_move,
                value: r.value as f32,
                exact: true,
                from_book: false,
            }
        } else if let Some(t) = selective_band(board.empty_count(), c.solve_empties, c.band) {
            let r = self.solver.solve_selective(board, Some(&self.evaluator), t);
            MoveEval {
                pos: r.best_move,
                value: r.value as f32,
                exact: false,
                from_book: false,
            }
        } else {
            let (pos, value) = self.search.best_move_valued(board, c.depth);
            MoveEval {
                pos,
                value,
                exact: false,
                from_book: false,
            }
        }
    }

    /// 局面を指定深さで評価する (評価値グラフ・検討用)。完全読み域は厳密値。
    /// 値は手番視点。
    pub fn eval_position(&mut self, board: &Board, depth: u32) -> MoveEval {
        self.stop.reset();
        if is_game_over(board) {
            return MoveEval {
                pos: None,
                value: final_score(board) as f32,
                exact: true,
                from_book: false,
            };
        }
        if board.empty_count() <= self.config.solve_empties {
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            MoveEval {
                pos: r.best_move,
                value: r.value as f32,
                exact: true,
                from_book: false,
            }
        } else {
            let (pos, value) = self.search.best_move_valued(board, depth);
            MoveEval {
                pos,
                value,
                exact: false,
                from_book: false,
            }
        }
    }

    /// 全合法手を採点する (WZebra 的なヒント表示用)。各手を打った子局面を
    /// `depth - 1` (完全読み域は厳密) で評価し、親の手番視点の値で返す。
    /// 値の大きい順にソート済み。
    ///
    /// 手の間の比較可能性を優先し、各子局面を**同一条件** (クリアした
    /// 置換表・単一スレッド) で測る。共有の温まった表のままだと、先に測った
    /// 手のエントリが後の手の探索に混ざり、対称局面ですら数石ずれる。
    pub fn analyze(&mut self, board: &Board, depth: u32) -> Vec<(Position, MoveEval)> {
        self.stop.reset();
        let saved_threads = self.search.threads;
        self.search.threads = 1;
        let mut out = Vec::new();
        for pos in board.movable_iter() {
            if self.stop.is_stopped() {
                break; // 中断: ここまでの採点だけ返す
            }
            let mut child = *board;
            child.make_move_bits(pos);
            let ev = if is_game_over(&child) {
                MoveEval {
                    pos: Some(pos),
                    value: -(final_score(&child) as f32),
                    exact: true,
                    from_book: false,
                }
            } else if child.empty_count() <= self.config.solve_empties {
                let r = self.solver.solve_with_eval(
                    EndSolverMode::Perfect,
                    &child,
                    Some(&self.evaluator),
                );
                MoveEval {
                    pos: Some(pos),
                    value: -(r.value as f32),
                    exact: true,
                    from_book: false,
                }
            } else {
                self.search.clear();
                let d = depth.saturating_sub(1).max(1);
                let (_, v) = self.search.best_move_valued(&child, d);
                MoveEval {
                    pos: Some(pos),
                    value: -v,
                    exact: false,
                    from_book: false,
                }
            };
            out.push((pos, ev));
        }
        self.search.threads = saved_threads;
        // 解析でばらまいた浅いエントリを対局用の探索に引き継がない
        self.search.clear();
        out.sort_by(|a, b| b.1.value.total_cmp(&a.1.value));
        out
    }
}
