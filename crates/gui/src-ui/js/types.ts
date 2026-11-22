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

export interface Notice {
  seq: number;
  at: number;
  name: string;
  kind: 'login' | 'logout' | 'start' | 'end';
  opp: string;
  id: string;
  diff: number | null;
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
  passw: string;
  notify: string[];
  track: string[];
  ignore: string[];
  notices: Notice[];
  notices_seen: number;
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
  open_requests: boolean;
  watch_analysis: boolean;
  /** While the opponent is to move on every board of ours, the threads search its position. */
  ponder: boolean;
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

/** FlipSight: one leg of a match (the site calls it 局). `cells` run a1 = 0, b1 = 1, ...; 1 black, 2 white. */
export interface FsLeg {
  cells: number[];
  turn: number;
  my_color: number;
  clocks_ms: number[];
  discs: number[];
  last: number | null;
  ply: number;
  active: boolean;
  thinking: boolean;
  /** Our moves: [move number, value, seconds]; no seconds for a move read back from the site. */
  evals: [number, number, (number | null)?][];
  /** The opponent's moves: [move number, seconds], from its clock. */
  opp_secs: [number, number][];
  /** The opponent's moves: [move number, the value it sent, from its side]. */
  opp_evals?: [number, number][];
  moves: string;
}

export interface FsMatch {
  id: number;
  /** bot: two legs from an opening; human: one game from the start position. */
  kind: 'bot' | 'human';
  opponent: number;
  opponent_name: string;
  opponent_rating: number | null;
  legs: FsLeg[];
}

export interface FsOtherBot {
  id: number;
  name: string;
  rating: number | null;
  /** null: the site does not say (a bot known only from the games played against it). */
  online: boolean | null;
  playing: boolean;
  /** Its owner paused its matches on the site. */
  paused: boolean;
  builtin: boolean;
  /** The bot's picture on the site (a full address). */
  photo: string | null;
}

export interface FsBotView {
  id: number;
  name: string;
  photo: string | null;
  rating: number | null;
  max_games: number | null;
  conn: string;
  matches: FsMatch[];
  outstanding: { target: number; name: string; since_ms: number } | null;
  cooling: number[];
  bots: FsOtherBot[];
  finished: number;
}

export type FsConn = 'off' | 'connecting' | 'online' | 'draining' | 'retrying' | 'refused';

export interface FsBotRow {
  id: number;
  name: string;
  photo: string | null;
  threads: number;
  cap: number | null;
  max_games: number | null;
  book: boolean;
  accept: boolean;
  /** Depth and solve every move is played at; null plays by the clock. */
  level: [number, number] | null;
  /** Midgame and endgame table bits; null follows the app's setting. */
  hash: [number | null, number | null];
  /** Searches the opponent's position while it thinks; on the clock only. */
  ponder: boolean;
  conn: FsConn;
  view: FsBotView | null;
}

export interface FsPending { busy: boolean; error: string; done: number | null }

export interface FsSnapshot {
  cores: number;
  bots: FsBotRow[];
  adding: FsPending | null;
  token_change: FsPending | null;
  connected: boolean;
  playing: boolean;
  tables: FsTables;
}

export interface FsTables {
  /** The app's midgame and endgame bits. */
  app: [number, number];
  min: number;
  max: number;
  /** Bytes an entry of the clock search's, the midgame search's and the solver's table takes. */
  entry: [number, number, number];
  /** This machine's memory, in bytes. */
  memory: number;
}

/** One finished match as `results.jsonl` keeps it. */
export interface FsResult {
  t: number;
  match: number;
  /** Absent in results kept before games against people were told apart. */
  kind?: 'bot' | 'human' | null;
  opponent: number;
  opponent_name?: string | null;
  opponent_rating?: number | null;
  opponent_photo?: string | null;
  status: string;
  result: 'win' | 'loss' | 'draw' | 'aborted';
  mine: number;
  theirs: number;
  diff: number;
  finish_reason?: string | null;
  legs: { black: boolean; discs: number[]; result_discs?: number[] | null; status: string; moves?: string | null; evals?: [number, number, (number | null)?][]; opp_secs?: [number, number][];
          opp_evals?: [number, number][];
          /** The final board (site order) and last square, replayed from the moves. */
          cells?: number[]; last?: number | null }[];
  my_rating?: number | null;
  my_rating_before?: number | null;
}
