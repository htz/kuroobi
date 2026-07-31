//! GUI・CLI・プロトコル実装が共用するエンジンセッション層。
//!
//! 対局路 (ggs) と同じ選択則 — 中盤 NNUE 探索 / 選択帯 / 完全読み — を
//! 1 つの構造体にまとめ、局面を渡すと着手と評価値を返す。探索は同期実行
//! なので、UI から使う側はワーカースレッドに載せて呼ぶこと。

use std::path::PathBuf;

use crate::evaluator::Evaluator;
use crate::midgame::{selective_band, NnueSearch, SharedTt};
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
}

/// 探索一式を束ねたセッション。生成コストが高い (重み読み込み +
/// 置換表確保) ので、プロセスにつき 1 個作って使い回す。
pub struct Engine {
    evaluator: Evaluator,
    search: NnueSearch,
    solver: Solver,
    config: EngineConfig,
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
        Ok(Engine { evaluator, search, solver, config })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
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
        let c = &self.config;
        if is_game_over(board) {
            // 終局: 探索に聞くと 0 が返る。盤面の石差 (空きマスは勝者へ加算、
            // FFO 規約) をそのまま厳密値として返す
            return MoveEval { pos: None, value: final_score(board) as f32, exact: true };
        }
        if board.empty_count() <= c.solve_empties {
            let r = self
                .solver
                .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            MoveEval { pos: r.best_move, value: r.value as f32, exact: true }
        } else if let Some(t) = selective_band(board.empty_count(), c.solve_empties, c.band) {
            let r = self.solver.solve_selective(board, Some(&self.evaluator), t);
            MoveEval { pos: r.best_move, value: r.value as f32, exact: false }
        } else {
            let (pos, value) = self.search.best_move_valued(board, c.depth);
            MoveEval { pos, value, exact: false }
        }
    }

    /// 局面を指定深さで評価する (評価値グラフ・検討用)。完全読み域は厳密値。
    /// 値は手番視点。
    pub fn eval_position(&mut self, board: &Board, depth: u32) -> MoveEval {
        if is_game_over(board) {
            return MoveEval { pos: None, value: final_score(board) as f32, exact: true };
        }
        if board.empty_count() <= self.config.solve_empties {
            let r = self
                .solver
                .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            MoveEval { pos: r.best_move, value: r.value as f32, exact: true }
        } else {
            let (pos, value) = self.search.best_move_valued(board, depth);
            MoveEval { pos, value, exact: false }
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
        let saved_threads = self.search.threads;
        self.search.threads = 1;
        let mut out = Vec::new();
        for pos in board.movable_iter() {
            let mut child = *board;
            child.make_move_bits(pos);
            let ev = if is_game_over(&child) {
                MoveEval { pos: Some(pos), value: -(final_score(&child) as f32), exact: true }
            } else if child.empty_count() <= self.config.solve_empties {
                let r = self
                    .solver
                    .solve_with_eval(EndSolverMode::Perfect, &child, Some(&self.evaluator));
                MoveEval { pos: Some(pos), value: -(r.value as f32), exact: true }
            } else {
                self.search.clear();
                let d = depth.saturating_sub(1).max(1);
                let (_, v) = self.search.best_move_valued(&child, d);
                MoveEval { pos: Some(pos), value: -v, exact: false }
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
