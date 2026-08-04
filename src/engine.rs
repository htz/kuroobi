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

/// 中盤探索は「探索の途中で終局まで読み切った」ことを石差 × 1000 で
/// 表す (ヒューリスティック評価より必ず優先されるように)。エンジンの
/// 外へ返す値は石差のスケールへ戻す。学習 (learn.rs) が定石に書く値も
/// この出口を通るので、スケールが混ざらない。
///
/// あわせて石差の範囲 (±64) に収める。中断された探索は番兵 (i32 の極値)
/// を返し、それが素通りすると「+2147483600 石」のような値が画面や定石に
/// 出る。石差はどうやっても ±64 なので、外へ出る前にここで潰す。
fn stone_scale(v: f32) -> f32 {
    let v = if v.abs() >= 999.0 { v / 1000.0 } else { v };
    if v.is_finite() {
        v.clamp(-64.0, 64.0)
    } else {
        0.0
    }
}

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
            weights: PathBuf::from("weights/linear.bin"),
            nnue: PathBuf::from("weights/nnue-h16.bin"),
            book: PathBuf::from("weights/book.txt"),
            use_book: true,
            book_tolerance: 1.0,
        }
    }
}

/// 1 手の判断結果。`value` は手番視点の石差 (exact なら厳密値)。
#[derive(Clone, Copy, Debug, Default)]
pub struct MoveEval {
    pub pos: Option<Position>,
    pub value: f32,
    /// 完全読みによる厳密値かどうか。
    pub exact: bool,
    /// 定石 book から返した手かどうか。
    pub from_book: bool,
    /// 実戦から学習した局面の定石から返した手か (表示用)。
    pub learned: bool,
    /// 中盤探索が到達した深さ。読み切りと定石では 0。
    pub depth: u32,
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
    /// 実戦から取り込んだ学習分 (保存対象)。`book` には起動時と
    /// 取り込み時に重ねてあり、選択は `book` 側で行う。
    learned: Book,
    /// 学習分の保存先 (book と同じディレクトリの book_learn.txt)。
    learn_path: std::path::PathBuf,
    /// 読み切りが訪れたノードの累計。中盤探索は `search.nodes` が自分で
    /// 積んでいるが、Solver は 1 回ぶんしか持たないのでここで足す。
    solver_nodes: u64,
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
        // 読み込みは常に試みる。`use_book` は参照するかどうかの切り替えで、
        // 対局中に切っても読み直しが要らないようにしてある。
        let mut book = match Book::load(&config.book) {
            Ok(b) if !b.is_empty() => Some(b),
            _ => None,
        };
        // 実戦から取り込んだ学習分 (learn.rs) を定石へ重ねる。学習分だけ
        // でも定石として機能する (定石ファイルが無い環境でも実戦で通った
        // 局面から「経験の定石」が育つ)。別ファイルなので bookgen が
        // 定石本体を回している間も衝突しない。
        let learn_path = config.book.with_file_name("book_learn.txt");
        let learned = Book::load(&learn_path).unwrap_or_default();
        if !learned.is_empty() {
            let base = book.get_or_insert_with(Book::new);
            crate::learn::merge_learned(base, &learned);
        }
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
            learned,
            learn_path,
            solver_nodes: 0,
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// この Engine が生涯に訪れたノードの累計 (中盤探索 + 読み切り)。
    /// 差分を取れば 1 回の探索ぶんが出る。表示専用。
    pub fn nodes(&self) -> u64 {
        self.search.nodes + self.solver_nodes
    }

    /// 読み込んだ book の局面数 (0 = book なし)。
    pub fn book_size(&self) -> usize {
        self.book.as_ref().map_or(0, |b| b.len())
    }

    /// 実戦から取り込んだ学習分の局面数。
    pub fn learned_size(&self) -> usize {
        self.learned.len()
    }

    /// 進行中の探索を中断させるためのハンドル (別スレッドから使う)。
    /// 立てた後の結果は不完全なので捨てること。次の探索前に `reset` される。
    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    /// 探索条件 (深さ・完全読み開始・帯) だけを差し替える。置換表と重みは
    /// そのまま。
    /// 定石 book を参照するかを切り替える。研究中に自力の手を見たいときに切る。
    pub fn set_use_book(&mut self, on: bool) {
        self.config.use_book = on;
    }

    /// book を読み込めているか (画面に「book なし」と出すため)。
    pub fn has_book(&self) -> bool {
        self.book.is_some()
    }

    /// 表示用: 局面が定石にあれば候補手の値の一覧 (盤面の向き・手番視点)
    /// を返す。定石を使わない設定では返さない (対局の選択と揃える)。
    pub fn book_hints(&self, board: &Board) -> Option<Vec<(Position, f32)>> {
        self.book
            .as_ref()
            .filter(|_| self.config.use_book)?
            .candidates(board)
    }

    /// 表示用: 局面が定石にあれば (最善の値, 実戦学習由来か) を返す。
    /// 評価値グラフが探索の代わりに使う。
    pub fn book_value(&self, board: &Board) -> Option<(f32, bool)> {
        let hints = self.book_hints(board)?;
        let best = hints
            .iter()
            .map(|(_, v)| *v)
            .fold(f32::NEG_INFINITY, f32::max);
        let learned = self.learned.get_raw(Book::key(board).0).is_some();
        Some((best, learned))
    }

    pub fn set_levels(&mut self, depth: u32, solve_empties: u8, band: u8) {
        self.config.depth = depth;
        self.config.solve_empties = solve_empties;
        self.config.band = band;
    }

    /// 探索の並列スレッド数 (中盤・終盤とも) を差し替える。実行中の探索には
    /// 効かず、次の探索から効く。
    pub fn set_threads(&mut self, n: usize) {
        let n = n.max(1);
        self.config.threads = n;
        self.search.threads = n;
        self.solver.set_threads(n);
    }

    /// 対局と同じ選択則で着手を決める。合法手がなければ `pos: None`
    /// (パス)。値は手番視点の石差。
    ///
    /// 定石には実戦から学習した局面 (learn.rs) も重ねてあり、選択は
    /// どちらも同じ乱択 (`probe_varied`) に乗る。負けた帰結で値が下がった
    /// 手は自然に選ばれなくなる — 回避のための特別な判定は持たない。
    pub fn choose(&mut self, board: &Board) -> MoveEval {
        self.choose_within(board, None)
    }

    /// 期限つきの着手選択。中盤探索は反復深化なので、期限が来ても直前に
    /// 完了した深さの答えを返せる。読み切りと定石は途中で刻めないので
    /// 期限を見ない (読み切りに入る手前で残り時間を見て決めるのは
    /// 呼び出し側の仕事)。
    pub fn choose_within(
        &mut self,
        board: &Board,
        deadline: Option<std::time::Instant>,
    ) -> MoveEval {
        self.stop.reset();
        // 定石 book: 実戦より深い探索で付けた答えなので、あれば即返す
        if let Some(book) = self.book.as_ref().filter(|_| self.config.use_book) {
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
                // 実戦から学習した局面か (画面の表示用)
                let learned = self.learned.get_raw(Book::key(board).0).is_some();
                return MoveEval {
                    pos: Some(pos),
                    value,
                    exact: false,
                    from_book: true,
                    learned,
                    depth: 0,
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
                learned: false,
                depth: 0,
            };
        }
        if board.empty_count() <= c.solve_empties {
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            self.solver_nodes += r.nodes;
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: true,
                from_book: false,
                learned: false,
                depth: 0,
            }
        } else if let Some(t) = selective_band(board.empty_count(), c.solve_empties, c.band) {
            let r = self.solver.solve_selective(board, Some(&self.evaluator), t);
            self.solver_nodes += r.nodes;
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: false,
                from_book: false,
                learned: false,
                depth: 0,
            }
        } else {
            let (pos, value, reached) = self.search.best_move_deadline(board, c.depth, deadline);
            MoveEval {
                pos,
                value: stone_scale(value),
                exact: false,
                from_book: false,
                learned: false,
                depth: reached,
            }
        }
    }

    /// 対局の取り込み (learn.rs) を用意する。実行は `learn_step` で
    /// 1 探索ずつ進める。
    pub fn learn_start(
        &self,
        start: Option<&str>,
        kifu: &str,
        learn_depth: u32,
    ) -> Result<crate::learn::BackupJob, String> {
        crate::learn::BackupJob::new(start, kifu, learn_depth.min(u8::MAX as u32) as u8)
    }

    /// 対局の取り込みを 1 探索ぶんだけ進める。完了したら結果を返し、
    /// 学習分をファイルへ保存する。
    ///
    /// 1 回の呼び出しは高々 1 回の評価 (代替手の子局面 1 つ、または
    /// 終局していない棋譜の終端) しかしないので、対局の合間に呼んでも
    /// 応答性を壊さない。`learn_depth` は評価の深さ (実戦より浅い速報値。
    /// 完全読みの開始も一時的に絞る)。
    pub fn learn_step(
        &mut self,
        job: &mut crate::learn::BackupJob,
        learn_depth: u32,
    ) -> Result<Option<crate::learn::BackupOutcome>, String> {
        use crate::learn::JobStep;
        self.stop.reset();
        // learned/book を外してジョブに渡す (self は探索にだけ使う)
        let mut base = self.book.take().unwrap_or_default();
        let mut learned = std::mem::take(&mut self.learned);
        let done = match job.next(&mut learned, &mut base) {
            JobStep::Search(b) => {
                // 学習は局面ごとに合法手の数だけ評価するので、完全読みの
                // 開始を一時的に絞ってコストを抑える (速報値でよい)
                let saved_solve = self.config.solve_empties;
                self.config.solve_empties = saved_solve.min(20);
                let v = self.eval_position_inner(&b, learn_depth);
                self.config.solve_empties = saved_solve;
                job.feed(v.value);
                None
            }
            JobStep::Done(out) => Some(out),
        };
        let save = if done.is_some() {
            learned.save(&self.learn_path)
        } else {
            Ok(())
        };
        self.book = (!base.is_empty()).then_some(base);
        self.learned = learned;
        save.map_err(|e| format!("学習の保存 {}: {e}", self.learn_path.display()))?;
        Ok(done)
    }

    /// 局面を指定深さで評価する (評価値グラフ・検討用)。完全読み域は厳密値。
    /// 値は手番視点。
    pub fn eval_position(&mut self, board: &Board, depth: u32) -> MoveEval {
        self.stop.reset();
        self.eval_position_inner(board, depth)
    }

    /// `eval_position` の本体。停止ハンドルをリセットしないので、外から
    /// 止められる長い処理 (学習の書き戻し) が繰り返し呼べる。
    fn eval_position_inner(&mut self, board: &Board, depth: u32) -> MoveEval {
        if is_game_over(board) {
            return MoveEval {
                pos: None,
                value: final_score(board) as f32,
                exact: true,
                from_book: false,
                learned: false,
                depth: 0,
            };
        }
        if board.empty_count() <= self.config.solve_empties {
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: true,
                from_book: false,
                learned: false,
                depth: 0,
            }
        } else {
            let (pos, value) = self.search.best_move_valued(board, depth);
            MoveEval {
                pos,
                value: stone_scale(value),
                exact: false,
                from_book: false,
                learned: false,
                depth: 0,
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
                    learned: false,
                    depth: 0,
                }
            } else if child.empty_count() <= self.config.solve_empties {
                let r = self.solver.solve_with_eval(
                    EndSolverMode::Perfect,
                    &child,
                    Some(&self.evaluator),
                );
                self.solver_nodes += r.nodes;
                MoveEval {
                    pos: Some(pos),
                    value: -(r.value as f32),
                    exact: true,
                    from_book: false,
                    learned: false,
                    depth: 0,
                }
            } else {
                self.search.clear();
                let d = depth.saturating_sub(1).max(1);
                let (_, v) = self.search.best_move_valued(&child, d);
                MoveEval {
                    pos: Some(pos),
                    value: stone_scale(-v),
                    exact: false,
                    from_book: false,
                    learned: false,
                    depth: 0,
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

    /// 深さを 1 ずつ上げながら全合法手を採点し、段ごとに結果を渡す。
    ///
    /// 止めるまで深くし続ける (`on_pass` が false を返すか、停止ハンドルが
    /// 立つまで)。深い答えが出るたびに前の答えを置き換えるので、途中で
    /// 止めてもその時点の最良が手元に残る。
    ///
    /// 読み切りに届いた手はそこで確定するので、以後の段では測り直さない。
    /// `on_pass` の第 3 引数は、この呼び出しが始まってから訪れたノードの数。
    /// 画面に働きぶりを出すために渡す (探索の判断には使わない)。
    ///
    /// **強さの設定 (`solve_empties`) は見ない。** ここは「反復深化がいま
    /// どこまで読めたか」をそのまま出す場所で、深さは止めるまで上がり続ける。
    /// 読み切りに入るかは深さが残り空きマス数に届いたかだけで決まる。
    /// 設定の読切は対局と評価値グラフ (固定の強さで測る側) に効く。
    pub fn analyze_deepening(
        &mut self,
        board: &Board,
        from_depth: u32,
        mut on_pass: impl FnMut(u32, &[(Position, MoveEval)], u64) -> bool,
    ) {
        let base_nodes = self.nodes();
        self.stop.reset();
        let saved_threads = self.search.threads;
        self.search.threads = 1;
        let mut depth = from_depth.max(1);
        loop {
            let mut out: Vec<(Position, MoveEval)> = Vec::new();
            let mut all_exact = true;
            for pos in board.movable_iter() {
                if self.stop.is_stopped() {
                    self.search.threads = saved_threads;
                    self.search.clear();
                    return;
                }
                let mut child = *board;
                child.make_move_bits(pos);
                let ev = if is_game_over(&child) {
                    MoveEval {
                        pos: Some(pos),
                        value: -(final_score(&child) as f32),
                        exact: true,
                        from_book: false,
                        learned: false,
                        depth: 0,
                    }
                } else if u32::from(child.empty_count()) <= depth {
                    // 深化がこの手の終局まで届いた。中盤探索は MPC で枝を
                    // 刈るので深さが足りていても厳密ではない — ここだけは
                    // ソルバに渡して厳密値にし、読み切りとして確定させる
                    let r = self.solver.solve_with_eval(
                        EndSolverMode::Perfect,
                        &child,
                        Some(&self.evaluator),
                    );
                    self.solver_nodes += r.nodes;
                    MoveEval {
                        pos: Some(pos),
                        value: stone_scale(-(r.value as f32)),
                        exact: true,
                        from_book: false,
                        learned: false,
                        depth: 0,
                    }
                } else {
                    all_exact = false;
                    self.search.clear();
                    let (_, v, reached) = self.search.best_move_deadline(&child, depth, None);
                    MoveEval {
                        pos: Some(pos),
                        value: stone_scale(-v),
                        exact: false,
                        from_book: false,
                        learned: false,
                        depth: reached + 1, // 子を d 読んだ = 親から見て d+1 手先まで
                    }
                };
                out.push((pos, ev));
            }
            out.sort_by(|a, b| b.1.value.total_cmp(&a.1.value));
            let go_on = on_pass(depth, &out, self.nodes() - base_nodes);
            // 全部が読み切りなら、深くしても答えは変わらない
            if !go_on || all_exact || depth >= 60 {
                break;
            }
            depth += 1;
        }
        self.search.threads = saved_threads;
        // 解析でばらまいた浅いエントリを対局用の探索に引き継がない
        self.search.clear();
    }
}
