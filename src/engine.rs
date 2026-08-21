//! GUI・CLI・プロトコル実装が共用するエンジンセッション層。
//!
//! 対局路 (ggs) と同じ選択則 — 中盤 NNUE 探索 / 選択帯 / 完全読み — を
//! 1 つの構造体にまとめ、局面を渡すと着手と評価値を返す。探索は同期実行
//! なので、UI から使う側はワーカースレッドに載せて呼ぶこと。

use std::path::PathBuf;

use crate::book::{Book, BookCandidate};
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
/// 探索の値を石差へ直す。
///
/// **非数をここで 0 にするのは最後の砦であって、通ってよい道ではない。**
/// 中断値 (`-inf`) が漏れてくると、画面には「+0.00」という**もっともらしい
/// 数字**が出る。実戦でそれが起き、値と手が食い違ったまま X 打ちを指した
/// (原因は反復深化が中断した段を採っていたこと)。**気付けるように数える。**
fn stone_scale(v: f32) -> f32 {
    let v = if v.abs() >= 999.0 { v / 1000.0 } else { v };
    if v.is_finite() {
        v.clamp(-64.0, 64.0)
    } else {
        NON_FINITE_VALUES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        0.0
    }
}

/// `stone_scale` が非数を丸めた回数。**0 でなければどこかが壊れている。**
pub static NON_FINITE_VALUES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 先読みの深さの上限。**期限が決めるので実質無制限。**
///
/// 本番の探索が上限なしで走るのに、先読みだけ設定の深さで止めると、
/// 置換表に浅い答えしか残らない。
const PONDER_DEPTH: u32 = 60;

/// 読切が期限で切れたときに指す「保険の手」を取る深さ。
///
/// **浅くてよい。** 使われるのは見積りが外れたときだけで、そこで求められる
/// のは「まともな手」であって最善手ではない。深くすると保険そのものが本命の
/// 時間を食う。
const BACKUP_DEPTH: u32 = 8;

/// 保険に使ってよい残り時間の割合。
const BACKUP_SHARE: f32 = 0.05;

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
    /// **読切 (または選択読み) が期限で打ち切られ、保険の手を指した。**
    ///
    /// 入り口の判断が甘かったことの印。頻発するなら、入り口を浅くするか
    /// 保険を厚くするかを決める材料になる。
    pub cut: bool,
}

/// 探索一式を束ねたセッション。生成コストが高い (重み読み込み +
/// 置換表確保) ので、プロセスにつき 1 個作って使い回す。
/// **探索の途中経過。** 反復深化が段を 1 つ終えるたびに書き換える。
///
/// 対局中に「いま何を読んでいるか」を外から見るための唯一の口。読む側
/// (GUI) は別スレッドなので、ロックを取らずに済むアトミックだけで持つ。
/// **探索の挙動は変えない** — 書き込みは段の切れ目 (1 手につき十数回) で、
/// 探索の内側には入らない。
#[derive(Debug, Default)]
pub struct Progress {
    /// いま何をしているか ([`Progress::IDLE`] / [`Progress::THINK`] /
    /// [`Progress::PONDER`] / [`Progress::SOLVE`] / [`Progress::SELECT`])。
    pub kind: std::sync::atomic::AtomicU8,
    /// 到達した深さ。読切・選択読みでは 0。
    pub depth: std::sync::atomic::AtomicU32,
    /// いま最善と思っている手 (0..64)。64 以上は「まだ無い」。
    pub best: std::sync::atomic::AtomicU32,
    /// その評価値の 1000 倍 (石差)。`i32::MIN` は「まだ無い」。
    pub milli: std::sync::atomic::AtomicI32,
    /// **符号を反転して記録するか。**
    ///
    /// 先読みは「自分が指した後の局面」を読む。その局面の手番は相手なので、
    /// 探索が返す石差は**相手から見た値**になる。画面には常に自分から見た
    /// 値を出したいので、先読みのあいだだけ反転して記録する。
    flip: std::sync::atomic::AtomicBool,
}

impl Progress {
    pub const IDLE: u8 = 0;
    pub const THINK: u8 = 1;
    pub const PONDER: u8 = 2;
    pub const SOLVE: u8 = 3;
    pub const SELECT: u8 = 4;

    /// 何をしているかだけを立てる (深さと手は据え置き)。
    pub fn set_kind(&self, kind: u8) {
        self.kind.store(kind, std::sync::atomic::Ordering::Relaxed);
    }

    /// 何もしていない状態へ戻す。
    ///
    /// **反転の指定も戻す。** ここを落としていたせいで、一度でも先読みを
    /// した後の思考が**ずっと符号を反転して出ていた** (実戦で盤の途中経過
    /// が −35、指した手の確定値が +35.3 と食い違って露見した)。先読みは
    /// `clear()` の直後に自分で立て直すので、既定は倒しておいてよい。
    pub fn clear(&self) {
        self.kind
            .store(Self::IDLE, std::sync::atomic::Ordering::Relaxed);
        self.depth.store(0, std::sync::atomic::Ordering::Relaxed);
        self.best.store(64, std::sync::atomic::Ordering::Relaxed);
        self.milli
            .store(i32::MIN, std::sync::atomic::Ordering::Relaxed);
        self.flip.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// 段を 1 つ終えた。**値は常に自分から見た石差にして置く** (先読みは
    /// 相手の手番の局面を読んでいるので反転する)。
    pub fn reached(&self, depth: u32, best: Option<Position>, value: f32) {
        use std::sync::atomic::Ordering::Relaxed;
        self.depth.store(depth, Relaxed);
        self.best
            .store(best.map(|p| p.index() as u32).unwrap_or(64), Relaxed);
        if value.is_finite() {
            let v = if self.flip.load(Relaxed) {
                -value
            } else {
                value
            };
            self.milli.store((v * 1000.0) as i32, Relaxed);
        }
    }

    /// 予測した相手の手を置く (先読み用)。
    pub fn predict(&self, pos: Position) {
        self.best
            .store(pos.index() as u32, std::sync::atomic::Ordering::Relaxed);
    }

    /// 読む側が使う写し。
    pub fn snapshot(&self) -> (u8, u32, Option<u32>, Option<f32>) {
        use std::sync::atomic::Ordering::Relaxed;
        let b = self.best.load(Relaxed);
        let m = self.milli.load(Relaxed);
        (
            self.kind.load(Relaxed),
            self.depth.load(Relaxed),
            (b < 64).then_some(b),
            (m != i32::MIN).then(|| m as f32 / 1000.0),
        )
    }
}

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
    /// **学習分を重ねる前の定石本体。** 分析のグラフが「定石の値」として
    /// 使ってよいのはこちらだけ — 本体は深い探索で付けた値だが、学習分は
    /// 終局の石差を根まで書き戻したもので、その局面の評価ではない。
    ///
    /// `learned` との差分では代用できない。**序盤の局面は実戦で必ず通る
    /// ので学習分にも載っており**、「学習分にあるか」で外すと本体に載って
    /// いる定石手まで落ちる (実際に落ちた)。
    book_base: Option<Book>,
    /// 学習分の保存先 (book と同じディレクトリの book_learn.txt)。
    learn_path: std::path::PathBuf,
    /// 読み切りが訪れたノードの累計。中盤探索は `search.nodes` が自分で
    /// 積んでいるが、Solver は 1 回ぶんしか持たないのでここで足す。
    solver_nodes: u64,
    /// 探索の途中経過 (外から覗くための口)。
    progress: std::sync::Arc<Progress>,
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
        let progress = std::sync::Arc::new(Progress::default());
        let mut search = NnueSearch::new(nn, tt);
        search.set_progress(Some(progress.clone()));
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
        // **重ねる前の本体を控える。** 重ねると上書きされて区別できなくなる
        let book_base = match Book::load(&config.book) {
            Ok(b) if !b.is_empty() => Some(b),
            _ => None,
        };
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
            book_base,
            book_rand,
            learned,
            learn_path,
            solver_nodes: 0,
            progress: progress.clone(),
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// **期限の見張りを立てる。** 期限が来たら停止ハンドルを立てて、
    /// 走っている読切を打ち切らせる。
    ///
    /// 中盤の反復深化は自前で期限を見られる (段の切れ目で戻れる) が、
    /// 読切には切れ目が無い。外から止めるしかない。
    fn watch_deadline(
        &self,
        deadline: Option<std::time::Instant>,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        let dl = deadline?;
        let stop = self.stop.clone();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d2 = done.clone();
        std::thread::spawn(move || {
            while !d2.load(std::sync::atomic::Ordering::Relaxed) {
                let now = std::time::Instant::now();
                if now >= dl {
                    stop.stop();
                    return;
                }
                std::thread::sleep((dl - now).min(std::time::Duration::from_millis(20)));
            }
        });
        Some(done)
    }

    /// 見張りを畳んで、**期限で切られたか**を返す。
    fn stop_watch_done(
        &mut self,
        watcher: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> bool {
        let Some(done) = watcher else { return false };
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let cut = self.stop.is_stopped();
        // 次の探索のために戻す (`choose_within` の頭でも reset している)
        self.stop.reset();
        cut
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

    /// **次の探索で「この手を先に見ろ」と伝える。**
    ///
    /// 同期対局は 2 面が鏡像なので、相手が片方で指した手はこちらのもう
    /// 片方の候補そのものになる。2650 の相手が長考して選んだ手を並べ替えの
    /// 先頭に置ければ、同じ時間でより深く読める。
    ///
    /// **手の選択を委ねるわけではない。** 置換表には上下限を入れないので
    /// 打ち切りには使われず、探索は今までどおり全部の手を評価する。効くのは
    /// 順序だけ。
    pub fn hint_move(&mut self, board: &Board, pos: Position) {
        let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
        self.search.tt.seed_move(h, pos.index());
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

    /// 定石を眺めるための出し口。候補手 (値・採用回数) と、この局面が
    /// 実戦学習で書き戻されたものかを返す。
    ///
    /// `book_hints` と違って `use_book` を見ない — 定石を切っていても
    /// 中身は見たいことがあるし、切ったから消えるのは嘘になる。
    pub fn book_node(&self, board: &Board) -> Option<(Vec<BookCandidate>, bool)> {
        let moves = self.book.as_ref()?.candidates_detailed(board)?;
        Some((moves, self.learned.has(board)))
    }

    /// 中盤の置換表を空にする。
    ///
    /// **測定用。** 1 局ごとに消さないと、温まった表が次の局へ持ち越されて
    /// 同一条件の対戦ですら偏った結果が出る (CLAUDE.md の 4 番)。
    /// 対局中に呼ぶものではない — ポンダリングは表を持ち越すためにある。
    pub fn clear_tables(&mut self) {
        self.search.clear();
    }

    /// **この機械の読切速度 (ノード毎秒) を測る。**
    ///
    /// 持ち時間から読切の入り口を逆算する ([`crate::timectl::solve_entry`])
    /// には、ノード数の見積りを秒へ直す係数が要る。**そこだけが機械依存**
    /// なので、ここで実測して `resources.conf` に控える。
    ///
    /// 測るのは**この Engine 自身の Solver** — スレッド数も置換表の大きさも
    /// NNUE の有無も対局と同じものが効く。別に組んだ計測器で測ると、対局で
    /// 出る速度とずれる。
    ///
    /// 局面は空き 22 の 3 問 (`bench/calib1030.obf` から、その群の中央付近の
    /// 所要時間のものを選んだ)。1 スレッドで 2 秒強、8 スレッドなら 1 秒を
    /// 切る — **起動のたびに測っても待たされない**長さにしてある。
    ///
    /// 置換表を空にする時間は差し引く (毎回の固定費で、ノード数に比例
    /// しない)。空きが深くなると表が溢れて nps は 1 割ほど落ちるが、その
    /// ぶんは `timectl` 側の `DEEP_NPS_RATIO` が持つ。
    ///
    /// **2 周測って速いほうを採る。** 他のプロセスが CPU を使っていると
    /// 値が落ちる — 実機で 5 スレッドが 4 スレッドより遅く出た。外乱は
    /// **遅くする方向にしか効かない**ので、速いほうが真値に近い
    /// (真値より速くは測れないので、過大評価にはならない)。
    pub fn measure_solve_nps(&mut self) -> f64 {
        let a = self.measure_solve_nps_once();
        let b = self.measure_solve_nps_once();
        a.max(b)
    }

    /// 1 周ぶんの実測。[`Engine::measure_solve_nps`] が 2 回呼ぶ。
    fn measure_solve_nps_once(&mut self) -> f64 {
        /// 空き 22 の較正局面 (OBF)。
        const POSITIONS: [&str; 3] = [
            "--XOOO----XOOOOO-XXOOOOX-XXXOXXX-XXXOXXX-XOOOOXX--OO-O------O--- X",
            "----X-----OX-O---XXXXO--XXXXXO--OOXXXOO-OOOXOXOO-OOOOOX--XXXXXX- X",
            "-OOOOX---OXXOX--XXXOOOOOXXXXO-O-XXXOOO--XXXXXXX-X--XXX------X--- X",
        ];
        use std::sync::atomic::Ordering;
        let clear0 = crate::solver::CLEAR_NS.load(Ordering::Relaxed);
        let t0 = std::time::Instant::now();
        let mut nodes = 0u64;
        for p in POSITIONS {
            let Ok(board) = Board::from_string(p) else {
                continue;
            };
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, &board, Some(&self.evaluator));
            nodes += r.nodes;
        }
        self.solver_nodes += nodes;
        let clear = (crate::solver::CLEAR_NS.load(Ordering::Relaxed) - clear0) as f64 / 1e9;
        let secs = t0.elapsed().as_secs_f64() - clear;
        if secs <= 0.0 || nodes == 0 {
            return 0.0;
        }
        nodes as f64 / secs
    }

    /// 相手の手番中に先行して読む。**盤は動かさない。**
    ///
    /// `after_my_move` は自分が指し終えた局面 (= 相手の手番)。置換表が指す
    /// **相手の最善手 1 本だけ**を反復深化し、期限か `stop` まで走って、
    /// 訪れたノード数を返す。
    ///
    /// **全合法手に配る形と混合は測って捨てた** (`notes/pondering.md`)。
    /// 11 手に時間を配ると 1 手あたり 4〜6 段までしか届かず、17 段前後まで
    /// 読む本番の探索では枝を刈れない。到達深さの差は
    /// **予測手 1 本で +1.6〜1.8 段、全合法手では 0** だった。
    /// **先に読んでおけば足しになる、は成り立たない** — 本番と同じ深さまで
    /// 行けたものだけが役に立つ。
    ///
    /// **解析の経路 (`analyze` / `analyze_deepening`) は使えない。** あちらは
    /// 手どうしを公平に比べるために各手の前後で置換表を消しており
    /// (「解析でばらまいた浅いエントリを対局用の探索に引き継がない」)、
    /// ポンダリングの目的と正反対になる。
    ///
    /// **深さを決め打ちしていても効く。** 効き方が変わるだけ —
    /// 持ち時間で刻むなら同じ時間で **+1.25 段**深く、深さ固定なら同じ
    /// 深さへ **1/3 の時間**で着く (実測 −62〜65%)。
    /// 「深さ固定では無駄」と一度書いたが誤りだった。走り切る先が
    /// 置換表に載っていれば、走り切ること自体が速くなる。
    pub fn ponder(&mut self, after_my_move: &Board, deadline: std::time::Instant) -> u64 {
        let base = self.nodes();
        if is_game_over(after_my_move) || after_my_move.movable_count() == 0 {
            return 0;
        }
        /* 予測が取れないことがある (終盤は完全読みが別の表を使うので中盤の
        表に最善手が残らない。実測で終盤の 47%)。**そこで適当な手を選んで
        読まない** — 当たる見込みが無いまま時間を使うだけになる。 */
        let Some(pred) = self.tt_best(after_my_move) else {
            return 0;
        };
        self.progress.clear();
        self.progress.set_kind(Progress::PONDER);
        self.progress
            .flip
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.progress.predict(pred);
        let mut child = *after_my_move;
        child.make_move_bits(pred);
        if is_game_over(&child) {
            return 0;
        }
        /* **完全読み域は読まない。入れてみて、測って捨てた。**
        予測手を先に解いておけば本番が表から即取れるはず、と考えて入れたが、
        `solve 24 / depth 12 固定 / 16 局` で **A の 1 手あたりの探索時間が
        121.8 → 122.6 ms** と変わらなかった。先読み自体は 188 回・計 23.6 秒
        走っていたので動いてはいる。`Solver` は表を消していないので当たれば
        効くはずだが、**効かない理由は突き止められていない**。
        理由の分からない得は残さない (CLAUDE.md の 1 番)。期限で刻めない
        ぶん、受信を数秒ふさぐ危険も抱えるので尚更。 */
        if child.empty_count() <= self.config.solve_empties {
            return 0;
        }
        self.stop.reset();
        /* **期限は探索の中に渡す。** 自前で段ごとに見ていると、深い 1 段が
        始まってしまったら止まらない — 実戦の待ち行列 (GGS の受信ループ)
        を数秒ふさぐことになる。`best_move_deadline` は反復深化を内側に
        持っていて、期限が来た段は捨てて直前の段まで返す。 */
        /* **深さの上限を置かない。** 本番の探索が上限なしで走るのに先読みだけ
        設定の深さで止めると、置換表に浅い答えしか残らず、本番はそこから
        読み直すことになる。**先読みは本番と同じ深さまで行けたものだけが
        役に立つ** (全合法手に配る形を測って捨てたのと同じ理由)。 */
        self.search
            .best_move_deadline(&child, PONDER_DEPTH, Some(deadline));
        self.nodes() - base
    }

    /// 置換表に載っているこの局面の最善手。**探索し直さない。**
    ///
    /// ポンダリングが「相手が指すと思う手」を取るための口。自分の手を
    /// 指したあとの局面を渡すと、直前の探索が読んだ範囲での相手の最善手が
    /// 返る (表から溢れていれば `None`)。
    pub fn tt_best(&self, board: &Board) -> Option<Position> {
        let h = crate::zobrist::board_hash(board.player_bb(), board.opponent_bb());
        self.search
            .tt
            .best_move(h)
            .and_then(|i| Position::from_index(i as u32))
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

    /// **定石本体**に載っているこの局面の最善の値 (手番視点)。
    ///
    /// 分析のグラフが使う。学習分は見ない — 値の性質が違う (本体は深い
    /// 探索、学習分は終局の石差の書き戻し)。`use_book` を見るので、定石を
    /// 使わない設定なら返さない (対局の選択と揃える)。
    pub fn book_base_value(&self, board: &Board) -> Option<f32> {
        let hints = self
            .book_base
            .as_ref()
            .filter(|_| self.config.use_book)?
            .candidates(board)?;
        hints
            .iter()
            .map(|(_, v)| *v)
            .fold(f32::NEG_INFINITY, f32::max)
            .into()
    }

    /// 定石に載っているこの局面の (最善の値, 実戦学習由来か, 付けたときの
    /// 読み深さ)。**深さは「その値がどれくらい確かか」を読む手がかり**で、
    /// 定石の画面が「石差 · 12 手読み」と出すのに使う。
    pub fn book_entry(&self, board: &Board) -> Option<(f32, bool, u8)> {
        let book = self.book.as_ref()?;
        let (_, value, depth) = book.probe(board)?;
        let learned = self.learned.get_raw(Book::key(board).0).is_some();
        Some((value, learned, depth))
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
    /// 探索の途中経過を覗く口 (別スレッドから読んでよい)。
    pub fn progress(&self) -> std::sync::Arc<Progress> {
        self.progress.clone()
    }

    pub fn choose_within(
        &mut self,
        board: &Board,
        deadline: Option<std::time::Instant>,
    ) -> MoveEval {
        self.stop.reset();
        self.progress.clear();
        self.progress.set_kind(Progress::THINK);
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
                    cut: false,
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
                cut: false,
            };
        }
        if board.empty_count() <= c.solve_empties {
            /* **読切も期限で打ち切る。**

            入る前に所要時間を見積もってはいるが (`timectl`)、見積りは必ず
            外れる — 同じ空きでも局面によって中央値の 5 倍以上散る。外れた
            ときに終わるまで返ってこないと、**時間切れで一発レートダウン**に
            なる。打ち切って浅い答えを指すほうが、棋力の損は小さい。

            打ち切ったときのために**保険の手を先に取る**。読切の途中で
            止めた値は不完全 (番兵が混じる) ので使えない。 */
            let backup = deadline.map(|_| {
                let (pos, value, _) = self.search.best_move_deadline(
                    board,
                    BACKUP_DEPTH,
                    // 保険に使ってよいのは予算のごく一部。ここで使いすぎると
                    // 本命の読切に入る時間が無くなる
                    deadline.map(|d| {
                        let now = std::time::Instant::now();
                        now + (d - now).mul_f32(BACKUP_SHARE)
                    }),
                );
                (pos, value)
            });
            self.progress.set_kind(Progress::SOLVE);
            let watcher = self.watch_deadline(deadline);
            let r =
                self.solver
                    .solve_with_eval(EndSolverMode::Perfect, board, Some(&self.evaluator));
            self.solver_nodes += r.nodes;
            let cut = self.stop_watch_done(watcher);
            if cut {
                if let Some((pos, value)) = backup.filter(|(p, _)| p.is_some()) {
                    return MoveEval {
                        pos,
                        value: stone_scale(value),
                        exact: false,
                        from_book: false,
                        learned: false,
                        depth: BACKUP_DEPTH,
                        cut: true,
                    };
                }
            }
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: !cut,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
            }
        } else if let Some(t) = selective_band(board.empty_count(), c.solve_empties, c.band) {
            // 選択読みも同じ。期限で切れたら保険の手を返す
            let backup = deadline.map(|_| {
                let (pos, value, _) = self.search.best_move_deadline(
                    board,
                    BACKUP_DEPTH,
                    deadline.map(|d| {
                        let now = std::time::Instant::now();
                        now + (d - now).mul_f32(BACKUP_SHARE)
                    }),
                );
                (pos, value)
            });
            self.progress.set_kind(Progress::SELECT);
            let watcher = self.watch_deadline(deadline);
            let r = self.solver.solve_selective(board, Some(&self.evaluator), t);
            self.solver_nodes += r.nodes;
            let cut = self.stop_watch_done(watcher);
            if cut {
                if let Some((pos, value)) = backup.filter(|(p, _)| p.is_some()) {
                    return MoveEval {
                        pos,
                        value: stone_scale(value),
                        exact: false,
                        from_book: false,
                        learned: false,
                        depth: BACKUP_DEPTH,
                        cut: true,
                    };
                }
            }
            MoveEval {
                pos: r.best_move,
                value: stone_scale(r.value as f32),
                exact: false,
                from_book: false,
                learned: false,
                depth: 0,
                cut: false,
            }
        } else {
            /* **中盤にも見張りを付ける。** 読切と選択読みには付けていたのに、
            ここだけ「反復深化は段の切れ目で期限を見るから要らない」と思って
            外していた。**足りなかった。**

            段の頭で見ても、その段自体が長ければ止まらない。実戦 (15 分の
            同期対局、空き 37) で**期限 43.5 秒の手が 132.6 秒**走り、
            外側の保険が殺すまで返ってこなかった。

            **単体では再現しない。** 同じ空き・同じ期限で 1 局面を読ませても
            段の切れ目で素直に止まる。違いは**同期対局が CPU を取り合うこと**。
            2 ワーカー × 4 スレッドに先読みが重なると、単独なら 30 秒の段が
            3 倍に伸びる。段の頭で見る方式は「次の段がどれだけかかるか」を
            知らないので、これを防ぎようがない。

            見張りが停止を立てれば段の途中でも抜ける (保険がそれで止められた
            のが証拠)。切られた段は捨てて直前の段を返す仕組みが
            `lazy_smp` 側にあるので、手が消えることはない。 */
            let watcher = self.watch_deadline(deadline);
            let (pos, value, reached) = self.search.best_move_deadline(board, c.depth, deadline);
            let cut = self.stop_watch_done(watcher);
            MoveEval {
                pos,
                value: stone_scale(value),
                exact: false,
                from_book: false,
                learned: false,
                depth: reached,
                cut,
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

    /// 取り込みを 1 局ぶん取り消す。戻せた手の数を返す。
    ///
    /// 学習分を書き戻して保存したあと、**定石本体をファイルから読み直して
    /// 重ね直す**。あちらは「ファイル + 学習分」なので、学習分を直したら
    /// 作り直すのが正しい (部分的に戻すと、元のファイルに何が入っていたかを
    /// 推測することになる)。
    pub fn undo_learn(
        &mut self,
        start: Option<&str>,
        kifu: &str,
        changes: &[crate::learn::BackupChange],
    ) -> Result<usize, String> {
        let n = crate::learn::undo_backup(&mut self.learned, start, kifu, changes)?;
        self.learned
            .save(&self.learn_path)
            .map_err(|e| format!("学習の保存 {}: {e}", self.learn_path.display()))?;
        let mut book = match Book::load(&self.config.book) {
            Ok(b) if !b.is_empty() => Some(b),
            _ => None,
        };
        if !self.learned.is_empty() {
            let base = book.get_or_insert_with(Book::new);
            crate::learn::merge_learned(base, &self.learned);
        }
        self.book = book;
        Ok(n)
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
                cut: false,
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
                cut: false,
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
                cut: false,
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
                    cut: false,
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
                    cut: false,
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
                    cut: false,
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
                        cut: false,
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
                        cut: false,
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
                        depth: reached + 1,
                        cut: false, // 子を d 読んだ = 親から見て d+1 手先まで
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

#[cfg(test)]
mod progress_tests {
    use super::Progress;
    use crate::Position;

    /// **`clear()` は反転の指定も戻す。**
    ///
    /// 先読みは「自分が指した後の局面」を読むので手番が相手になり、画面へ
    /// 出す石差だけ符号を反転している。その指定を `clear()` が戻していな
    /// かったため、**一度でも先読みをすると以降の思考が全部反転して**
    /// 出ていた。実戦では盤の途中経過が −35、指した手の確定値が +35.3 と
    /// 逆に並んで露見した (指す手そのものは正しく、表示だけの害)。
    #[test]
    fn clear_drops_the_flip() {
        let p = Progress::default();
        let mv = Position::from_index(19);

        // 先読み: 相手の手番の値なので反転して置く
        p.set_kind(Progress::PONDER);
        p.flip.store(true, std::sync::atomic::Ordering::Relaxed);
        p.reached(6, mv, 4.0);
        assert_eq!(p.snapshot().3, Some(-4.0), "先読みは反転して置く");

        // 思考へ移る: clear() の後は反転しない
        p.clear();
        p.set_kind(Progress::THINK);
        p.reached(6, mv, 4.0);
        assert_eq!(
            p.snapshot().3,
            Some(4.0),
            "clear() の後は反転を持ち越さない"
        );
    }
}
