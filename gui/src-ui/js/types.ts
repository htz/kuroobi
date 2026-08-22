// Types returned by the backend (gui/src/main.rs); 1:1 with the Rust side.

export interface GameView {
  /** 64 cells: 0 empty / 1 black / 2 white (file-major, A1 = 0). */
  cells: number[];
  player: 'black' | 'white';
  legal: number[];
  black: number;
  white: number;
  over: boolean;
  last: number | null;
  kifu: string;
  move_count: number;
  /** Full move line (including undone moves); null = pass. */
  moves: (number | null)[];
  cursor: number;
}

export interface ThinkView {
  pos: number | null;
  value: number;
  exact: boolean;
  from_book: boolean;
  /** Whether it is a game-learned book entry. */
  learned: boolean;
  /** Seconds spent on this move. */
  secs: number;
  /** Nodes visited for this move (0 for book moves). */
  nodes: number;
}

/** Search workload (shown under the board); null while idle. */
export interface SearchStat {
  /** Nodes visited. */
  nodes: number;
  /** Elapsed seconds; nps derives from these two. */
  secs: number;
}

export interface HintView {
  pos: number;
  value: number;
  exact: boolean;
  /** Whether the value came from the book, not search. */
  from_book: boolean;
  /** Search depth behind the value (0 for solves and book). */
  depth: number;
}

export interface EvalPoint {
  n: number;
  /** Disc difference from Black's view. */
  value: number;
  exact: boolean;
  /** Whether the value came from the book, not search. */
  from_book: boolean;
}

/* ============================ GGS ============================ */
// State types streamed by the backend (gui/src/ggs.rs).

export interface LogLine {
  dir: 'in' | 'out' | 'info';
  text: string;
}

export interface UserRow {
  name: string;
  rating: number | null;
  /** Rating deviation; from `/os top` only. */
  dev: number | null;
  /** Random-opening (8r) rating and deviation; who list only. */
  rating_r: number | null;
  dev_r: number | null;
  /** Accepting: '+' open / '-' closed / 'x' ghost / null unknown. */
  open: string | null;
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
  /** [heading, value] pairs. */
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
  /** Whether it ended (finished/adjourned/aborted; stays listed). */
  over: boolean;
  /** How it ended: '' ongoing / 'finished' / 'adjourned' / 'aborted'. */
  ended: '' | 'finished' | 'adjourned' | 'aborted';
  /** Who left, for adjournments. */
  left_by: string;
  /** Archive id (post-game); used to re-fetch the record. */
  archive: string;
  /** Current activity: '' / 'think' / 'ponder' / 'solve' / 'select'. */
  busy: '' | 'think' | 'ponder' | 'solve' | 'select';
  /** Progress depth (0 during solves). */
  busy_depth: number;
  /** Current best move (the predicted reply while pondering). */
  busy_best: number | null;
  /** Its value in discs, mover view. */
  busy_eval: number | null;
  /** Final result (disc difference). */
  result: string;
  /** 64 cells: 0 empty / 1 black / 2 white (file-major). */
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
  /** Whether we entered overtime (the game is then lost on time). */
  in_overtime: boolean;
  players: PlayerView[];
  gtype: string;
  moves: string[];
  ggf: string;
  last_eval: number | null;
  last_eval_exact: boolean;
  /** Opponent's reported eval for their last move (their view); null if unreported. */
  opp_eval: number | null;
  /** Seconds the opponent reported spending. */
  opp_secs_used: number | null;
  /** Both players' reported evals in move order (mover-view discs). */
  eval_series: { n: number; mine: boolean; eval: number }[];
  /** Listing order (larger = newer); sorting only. */
  order: number;
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
  /** Channel name (".chat" etc.); empty for directs. */
  chan: string;
  from: string;
  text: string;
  /** Receive time (unix seconds). */
  at: number;
  /** Conversation key (".chat" or the peer). */
  thread: string;
}

export interface StandbyCfg {
  enabled: boolean;
  auto_accept: boolean;
  /** Whether standby offers are rated. */
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
  /** Whether finished games feed book learning. */
  learn: boolean;
  /** Whether to ponder. Works under fixed levels too — the gain just
      shifts from depth to speed. */
  ponder: boolean;
  /** Pacing ("fast" saves for the endgame / "depth" fixed). */
  pace: string;
  /** Per-move cap (seconds); 0 = none. */
  max_move_secs: number;
  /** Seconds reserved for the solve. */
  reserve_secs: number;
  /** Clock aggressiveness (1.0 = as allocated). */
  budget_use: number;
}

export interface FetchedGgf {
  id: string;
  ggf: string;
  /** All returned games; synchro archives hold two. */
  parts: string[];
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
  /** One-line notice (watch failed etc.). */
  notice: string;
  stored: StoredView[];
  history: Record<string, HistoryRow[]>;
  chat: ChatMsg[];
  /** Read-up-to marker (unix seconds); newer counts as unread. */
  chat_seen: number;
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
        /** Cross-window notify (settings changes to the main screen). */
        emit: (name: string, payload?: unknown) => Promise<void>;
      };
    };
  }
}

/** One book move; mover-view value, games = adoption count. */
export interface BookMove { pos: number; value: number; games: number }

/** One book position (book_node's return). */
export interface BookNode {
  cells: number[];
  player: 'black' | 'white';
  black: number;
  white: number;
  /** By value descending; empty = not in the book. */
  moves: BookMove[];
  /** Whether the position was game-learned. */
  learned: boolean;
  /** Book value in discs; null if absent. */
  value: number | null;
  /** Search depth behind the value — its trustworthiness hint. */
  depth: number | null;
  /** Total book positions. */
  size: number;
  /** Of which game-learned. */
  learned_size: number;
}

/** One imported game (learn_log's return). */
export interface LearnEntry {
  /** Unix seconds; formatting is the viewer's business. */
  at: number;
  kifu: string;
  black: number;
  white: number;
  /** Positions written back. */
  positions: number;
  /** Drawn-opening start (board string); empty for the standard start. */
  start: string;
  /** Opponent name for GGS games; empty for local. */
  opponent: string;
  /** Which color we played ('b'/'w'); recorded because disc counts
   *  alone cannot decide results. Empty in old logs and both/none games. */
  my_color?: string;
  /** Book-rewrite details; absent in old logs. */
  changes: LearnChange[];
}

/** One book-move rewrite record. */
export interface LearnChange {
  /** Move number (1-based, passes excluded). */
  ply: number;
  mv: string;
  /** Value before the overwrite; null if absent. */
  before: number | null;
  after: number;
  /** Best value after the rewrite; best - after = discs lost. */
  best: number;
  /** Whether this import created the entry. */
  new_entry: boolean;
}

/** Local game clock; total 0 = no clock. */
export interface ClockView {
  total: number;
  black: number;
  white: number;
  /** Which side flagged; null if none. */
  lost: 'black' | 'white' | null;
}
