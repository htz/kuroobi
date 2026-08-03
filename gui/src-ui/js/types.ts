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
}

export interface HintView {
  pos: number;
  value: number;
  exact: boolean;
  /** 定石 book の値か (探索でなく)。 */
  from_book: boolean;
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
}

export interface FetchedGgf {
  id: string;
  ggf: string;
  error: string;
}

export interface GgsSnapshot {
  conn: 'disconnected' | 'connecting' | 'logging_in' | 'online';
  login: string;
  my_rating: number | null;
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
      };
    };
  }
}
