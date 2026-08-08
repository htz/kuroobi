// バックエンド (gui/src/main.rs) が返す型。Rust 側と 1 対 1 で対応させる。

export interface GameView {
  /** 64 マス: 0 空 / 1 黒 / 2 白 (A1=0 の file-major)。 */
  cells: number[];
  player: 'black' | 'white';
  legal: number[];
  black: number;
  white: number;
  over: boolean;
  last: number | null;
  kifu: string;
  move_count: number;
  /** 全手順 (undo で戻った先も含む)。null はパス。 */
  moves: (number | null)[];
  cursor: number;
}

export interface ThinkView {
  pos: number | null;
  value: number;
  exact: boolean;
  from_book: boolean;
  /** 実戦から学習した局面の定石か。 */
  learned: boolean;
  /** この手に使った時間 (秒)。 */
  secs: number;
  /** この手を選ぶまでに訪れたノード数 (定石なら 0)。 */
  nodes: number;
}

/** 探索の働きぶり (盤の下に出す)。動いていないときは null にする。 */
export interface SearchStat {
  /** 訪れたノード数。 */
  nodes: number;
  /** そこまでの経過秒。速さ (nps) はこの 2 つから割って出す。 */
  secs: number;
}

export interface HintView {
  pos: number;
  value: number;
  exact: boolean;
  /** 定石 book の値か (探索でなく)。 */
  from_book: boolean;
  /** この値を出した探索の深さ (読み切り・定石は 0)。 */
  depth: number;
}

export interface EvalPoint {
  n: number;
  /** 黒視点の石差。 */
  value: number;
  exact: boolean;
  /** 定石 book の値か (探索でなく)。 */
  from_book: boolean;
}

/* ============================ GGS ============================ */
// バックエンド (gui/src/ggs.rs) が送ってくる状態の型。

export interface LogLine {
  dir: 'in' | 'out' | 'info';
  text: string;
}

export interface UserRow {
  name: string;
  rating: number | null;
  /** レートの偏差。`/os top` は返すが `/os who` は返さない。 */
  dev: number | null;
  raw: string;
}

export interface RankRow {
  gtype: string;
  name: string;
  rating: number;
  dev: number;
  rank: number;
  wins: number;
  draws: number;
  losses: number;
}

export interface FingerInfo {
  name: string;
  /** [見出し, 値] の並び。 */
  fields: [string, string][];
  raw: string[];
}

export interface Offer {
  id: string;
  raw: string;
  incoming: boolean;
  names: string[];
  gtype: string;
  time: string;
  rated: boolean;
}

export interface OngoingView {
  id: string;
  raw: string;
  watching: boolean;
  names: string[];
  ratings: string[];
  gtype: string;
  mine: boolean;
}

export interface StoredView {
  id: string;
  raw: string;
  opp: string;
  gtype: string;
}

export interface PlayerView {
  name: string;
  rating: string;
  clock: string;
  color: 'black' | 'white';
  secs: number | null;
  ext: number | null;
}

export interface MatchView {
  id: string;
  base: string;
  /** 終局したか (終わっても一覧には残る)。 */
  over: boolean;
  /** 終局の結果 (石差)。 */
  result: string;
  /** 64 マス: 0 空 / 1 黒 / 2 白 (file-major)。 */
  cells: number[];
  turn: '' | 'black' | 'white';
  my_color: '' | 'black' | 'white';
  opp_name: string;
  opp_rating: string;
  opp_clock: string;
  my_clock: string;
  my_secs: number | null;
  opp_secs: number | null;
  my_ext: number | null;
  opp_ext: number | null;
  players: PlayerView[];
  gtype: string;
  moves: string[];
  ggf: string;
  last_eval: number | null;
  last_eval_exact: boolean;
  last_from_book: boolean;
  watch_eval: number | null;
  watch_best: string | null;
  watch_exact: boolean;
  seen: number;
}

export interface GameResult {
  id: string;
  base: string;
  raw: string;
  my_diff: number | null;
  opp: string;
  kifu: string;
  ggf: string;
  archive: string;
  seq: number;
  my_rating: number | null;
  at: number;
}

export interface HistoryRow {
  id: string;
  at: string;
  black: string;
  black_rating: string;
  white: string;
  white_rating: string;
  score: string;
  gtype: string;
}

export interface ChatMsg {
  /** チャンネル名 (".chat" 等)。ダイレクトは空文字。 */
  chan: string;
  from: string;
  text: string;
  /** 受信時刻 (UNIX 秒)。 */
  at: number;
  /** 会話のまとまり (".chat" か相手の名前)。 */
  thread: string;
}

export interface StandbyCfg {
  enabled: boolean;
  auto_accept: boolean;
  /** 待機モードから申し込むときレート戦にするか。 */
  rated: boolean;
  opponent: string;
  gtype: string;
  time: string;
  max_games: number;
  interval_secs: number;
}

export interface StandbyStats {
  games: number;
  wins: number;
  losses: number;
  draws: number;
  diff_sum: number;
}

export interface EngineCfgView {
  depth: number;
  solve: number;
  band: number;
  threads: number;
  ready: boolean;
  use_book: boolean;
  book_loaded: boolean;
  /** 終わった対局を定石の学習に取り込むか。 */
  learn: boolean;
  /** 相手の手番中に先読みするか。**「深さ固定」では効かない。** */
  ponder: boolean;
  /** 持ち時間の配り方 ("depth" 深さ固定 / "slow" / "even" / "fast")。 */
  pace: string;
  /** 1 手に使う上限 (秒)。0 で上限なし。 */
  max_move_secs: number;
  /** 読み切り用に残す秒数。 */
  reserve_secs: number;
}

export interface FetchedGgf {
  id: string;
  ggf: string;
  error: string;
}

export interface GgsSnapshot {
  conn: 'disconnected' | 'connecting' | 'logging_in' | 'online';
  login: string;
  my_ranks: RankRow[];
  log: LogLine[];
  users: UserRow[];
  ranking: UserRow[];
  fingers: Record<string, FingerInfo>;
  offers: Offer[];
  matches: MatchView[];
  ongoing: OngoingView[];
  /** 画面に出す一言 (観戦に失敗した等)。 */
  notice: string;
  stored: StoredView[];
  history: Record<string, HistoryRow[]>;
  chat: ChatMsg[];
  results: GameResult[];
  standby: StandbyCfg;
  standby_stats: StandbyStats;
  engine: EngineCfgView;
  auto_play: boolean;
  watch_analysis: boolean;
  thinking: string | null;
  fetched_ggf: FetchedGgf | null;
}

declare global {
  interface Window {
    __TAURI__?: {
      core: { invoke: <T = unknown>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
      event: {
        listen: <T>(name: string, fn: (e: { payload: T }) => void) => Promise<() => void>;
        /** 窓をまたいで報せる。付属ウィンドウ (設定) の変更を主画面へ伝える。 */
        emit: (name: string, payload?: unknown) => Promise<void>;
      };
    };
  }
}

/** 定石の 1 手。value は手番から見た石差、games は棋譜での採用回数。 */
export interface BookMove { pos: number; value: number; games: number }

/** 定石のある 1 局面 (book_node の返り)。 */
export interface BookNode {
  cells: number[];
  player: 'black' | 'white';
  black: number;
  white: number;
  /** 値の高い順。空なら「この局面は定石に無い」。 */
  moves: BookMove[];
  /** 実戦から学習して書き戻された局面か。 */
  learned: boolean;
  /** 定石に載っている局面の総数。 */
  size: number;
  /** そのうち実戦から書き戻したぶん。 */
  learned_size: number;
}

/** 定石に取り込んだ対局 1 件 (learn_log の返り)。 */
export interface LearnEntry {
  /** unix 秒。書式は見る側で決める。 */
  at: number;
  kifu: string;
  black: number;
  white: number;
  /** 書き戻した局面数。 */
  positions: number;
  /** 抽選開局の開始局面 (盤面文字列)。標準の初期局面なら空。 */
  start: string;
  /** GGS の対局なら相手の名前。ローカル対局は空。 */
  opponent: string;
  /** 定石を書き換えた明細。古い控えには無い。 */
  changes: LearnChange[];
}

/** 定石を 1 手ぶん書き換えた記録。 */
export interface LearnChange {
  /** 棋譜の何手目か (パスを除いた 1 始まり)。 */
  ply: number;
  mv: string;
  /** 上書き前の値。定石に無かった手なら null。 */
  before: number | null;
  after: number;
  /** 書き換えたあとのその局面の最善値。best - after が損した石差。 */
  best: number;
  /** この取り込みで学習分に新しく作った局面か。 */
  new_entry: boolean;
}
