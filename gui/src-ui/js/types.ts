export interface GameView {
  cells: number[];
  player: 'black' | 'white';
  legal: number[];
  black: number;
  white: number;
  over: boolean;
  last: number | null;
  move_count: number;
  moves: (number | null)[];
  cursor: number;
}

export interface ThinkView {
  pos: number | null;
  value: number;
  exact: boolean;
  from_book: boolean;
  learned: boolean;
  secs: number;
  nodes: number;
}

export interface SearchStat {
  nodes: number;
  secs: number;
}

export interface HintView {
  pos: number;
  value: number;
  exact: boolean;
  from_book: boolean;
  depth: number;
}

export interface EvalPoint {
  value: number;
  exact: boolean;
  from_book: boolean;
}

export interface LogLine {
  seq: number;
  dir: 'in' | 'out' | 'info';
  text: string;
}

export interface UserRow {
  name: string;
  rating: number | null;
  dev: number | null;
  rating_r: number | null;
  dev_r: number | null;
  open: string | null;
  raw: string;
}

interface RankRow {
  gtype: string;
  name: string;
  rating: number;
  dev: number;
  rank: number;
  wins: number;
  draws: number;
  losses: number;
}

interface FingerInfo {
  fields: [string, string][];
}

interface Offer {
  id: string;
  raw: string;
  incoming: boolean;
  names: string[];
  gtype: string;
  time: string;
  rated: boolean;
  /** When the request was first heard of (epoch ms). */
  since_ms: number;
}

interface OngoingView {
  id: string;
  watching: boolean;
  names: string[];
  ratings: string[];
  gtype: string;
  mine: boolean;
}

interface StoredView {
  id: string;
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
  over: boolean;
  ended: '' | 'finished' | 'adjourned' | 'aborted';
  left_by: string;
  rated: boolean | null;
  archive: string;
  busy: '' | 'think' | 'solve';
  busy_depth: number;
  busy_best: number | null;
  busy_eval: number | null;
  result: string;
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
  in_extension: boolean;
  players: PlayerView[];
  gtype: string;
  moves: string[];
  ggf: string;
  last_eval: number | null;
  last_eval_exact: boolean;
  opp_eval: number | null;
  opp_secs_used: number | null;
  eval_series: { n: number; mine: boolean; eval: number | null; secs: number | null }[];
  order: number;
  last_from_book: boolean;
  watch_eval: number | null;
  watch_best: string | null;
  seen: number;
  /** When the server's last update for this board arrived, in ms since the epoch. */
  updated_ms: number;
  think_since_ms: number;
  think_queued: boolean;
  legal: number[];
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
  boards?: BoardRecord[];
  /** "resign" | "adjourn": the whole match was decided that way. */
  ending?: string;
  ending_by?: string;
}

export interface BoardRecord {
  id: string;
  kifu: string;
  ggf: string;
  my_color?: string;
  disc_diff?: number | null;
  /** "over_time" | "out_of_time" */
  ending?: string;
  ending_by?: string;
  clocks?: { name: string; color: string; main: number | null; ext: number | null }[];
}

interface HistoryRow {
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
  chan: string;
  from: string;
  text: string;
  at: number;
  thread: string;
}

export interface StandbyCfg {
  enabled: boolean;
  auto_accept: boolean;
  rated: boolean;
  opponent: string;
  gtype: string;
  time: string;
  max_games: number;
  interval_secs: number;
}

interface StandbyStats {
  games: number;
  wins: number;
  losses: number;
  draws: number;
  diff_sum: number;
}

interface EngineCfgView {
  threads: number;
  use_book: boolean;
  book_loaded: boolean;
  learn: boolean;
}

export interface FetchedGgf {
  id: string;
  ggf: string;
  parts: string[];
  error: string;
}

export interface GgsSnapshot {
  conn: 'disconnected' | 'connecting' | 'logging_in' | 'online';
  login: string;
  server: string;
  my_ranks: RankRow[];
  log: LogLine[];
  users: UserRow[];
  ranking: UserRow[];
  fingers: Record<string, FingerInfo>;
  offers: Offer[];
  matches: MatchView[];
  /** Who is watching each match we play or watch, by match id (`.7`), in name order. */
  watchers: Record<string, string[]>;
  ongoing: OngoingView[];
  online: string[] | null;
  notice: string;
  /** The server refused our last aform/dform: "dform|O2.e{REAL}!={INT}", else empty. */
  formula_refused: string;
  stored: StoredView[];
  history: Record<string, HistoryRow[]>;
  chat: ChatMsg[];
  chat_seen: number;
  /** Moves whenever the recorded games change; the games themselves come from `ggsApi.results()`. */
  results_rev: number;
  standby: StandbyCfg;
  standby_stats: StandbyStats;
  engine: EngineCfgView;
  auto_play: boolean;
  open: boolean;
  watch_analysis: boolean;
  log_channel: string;
  thinking: string | null;
  last_search: { nodes: number; secs: number } | null;
  fetched_ggf: FetchedGgf | null;
  td: TdView;
}

export interface TourneyView {
  id: number;
  state: '' | 'announced' | 'running' | 'break' | 'over' | 'cancelled';
  desc: string;
  director: string;
  clock: string;
  gtype: string;
  players: string[];
  n_players: number;
  rounds_played: number;
  rounds: number;
  next_at: number | null;
  joined: boolean;
  winner: string;
  info: string;
  rankings: string;
  schedule: string;
}

interface TdView {
  tourneys: TourneyView[];
  reply: { cmd: string; text: string; err: boolean; at: number } | null;
  games: { id: string; tid: number; opp: string; diff: number | null }[];
}

declare global {
  interface Window {
    __TAURI__?: {
      core: { invoke: <T = unknown>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
      event: {
        listen: <T>(name: string, fn: (e: { payload: T }) => void) => Promise<() => void>;
        emit: (name: string, payload?: unknown) => Promise<void>;
      };
    };
  }
}

interface BookMove { pos: number; value: number; games: number }

export interface BookNode {
  cells: number[];
  player: 'black' | 'white';
  black: number;
  white: number;
  moves: BookMove[];
  learned: boolean;
  value: number | null;
  depth: number | null;
  size: number;
  learned_size: number;
}

export interface LearnEntry {
  at: number;
  kifu: string;
  black: number;
  white: number;
  positions: number;
  start: string;
  opponent: string;
  my_color?: string;
  changes: LearnChange[];
}

export interface LearnChange {
  ply: number;
  mv: string;
  before: number | null;
  after: number;
  best: number;
  new_entry: boolean;
}

export interface ClockView {
  black: number;
  white: number;
  lost: 'black' | 'white' | null;
}
