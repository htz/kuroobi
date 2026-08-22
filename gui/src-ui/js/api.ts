// Backend I/O.

import type { BookNode, ClockView, EvalPoint, GameView, GgsSnapshot, HintView, LearnEntry, StandbyCfg, ThinkView } from './types';

const core = () => window.__TAURI__?.core;

/** Send JS exceptions to the backend log (the WebView console is invisible). */
export function jsLog(msg: unknown): void {
  try {
    void core()?.invoke('js_log', { msg: String(msg) });
  } catch {
    /* ignore log-send failures */
  }
}

async function call<T = void>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const c = core();
  if (!c) throw new Error('Tauri IPC unavailable');
  return c.invoke<T>(cmd, args);
}

/* In-app notifications: tells the board's book indicator about file
 * swaps in settings (kept — it avoids lifting state to the parent). */
export function emitApp(name: string): void {
  void window.__TAURI__?.event.emit(name).catch(() => { /* no listener is fine */ });
}

export function onApp(name: string, fn: () => void): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) return Promise.resolve(() => { /* outside Tauri */ });
  return ev.listen(name, () => fn());
}

export const api = {
  state: () => call<GameView>('state'),
  newGame: () => call<GameView>('new_game'),
  /** Read the clock; the running turn's elapsed time is subtracted in Rust. */
  clocks: () => call<ClockView>('clocks'),
  /** Initialize the clock; 0 = none. */
  setClock: (secs: number) => call<ClockView>('set_clock', { secs }),
  play: (sq: number) => call<GameView>('play', { sq }),
  undo: () => call<GameView>('undo'),
  goto: (n: number) => call<GameView>('goto', { n }),
  setUseBook: (on: boolean) => call<void>('set_use_book', { on }),
  setLearn: (on: boolean) => call<void>('set_learn', { on }),
  /** `myColor` = the human's color ('b'/'w'); empty for both/none.
   *  Without it the log cannot decide results from disc counts. */
  /** Active override env vars (name, value); empty on a plain launch. */
  envOverrides: () => call<[string, string][]>('env_overrides'),
  learnGame: (myColor: string) => call<void>('learn_game', { myColor }),
  /** Imported-game log, newest first. */
  learnLog: () => call<LearnEntry[]>('learn_log', {}),
  /** Undo one import; returns how many moves reverted. */
  learnUndo: (at: number, kifu: string) => call<number>('learn_undo', { at, kifu }),
  hasBook: () => call<boolean>('has_book', {}),
  /** Browse the book; kifu is moves from the start ("f5d6", empty = start). */
  bookNode: (kifu: string) => call<BookNode>('book_node', { kifu }),
  autoplay: () => call<string>('autoplay', {}),
  /** Screenshot theme pin (KUROOBI_THEME); empty follows the preference. */
  themeOverride: () => call<string>('theme_override', {}),
  /** Screenshot hook: pinned UI language, or '' when unset. */
  langOverride: () => call<string>('lang_override', {}),
  /** The machine's language (e.g. "ja-JP"), for the `auto` setting. */
  systemLang: () => call<string>('system_lang', {}),
  /** Name, path, existence, size (bytes), format tag. */
  resourceStatus: () => call<[string, string, boolean, number, string][]>('resource_status', {}),
  pickResource: (kind: string) => call<string | null>('pick_resource', { kind }),
  setResource: (kind: string, path: string | null) =>
    call<void>('set_resource', { kind, path }),
  setLevels: (depth: number, solveEmpties: number, band: number) =>
    call('set_levels', { depth, solveEmpties, band }),
  stopSearch: () => call('stop_search'),
  think: () => call<ThinkView>('think'),
  applyMove: (sq: number | null) => call<GameView>('apply_move', { sq }),
  analyzeLive: () => call<void>('analyze_live'),
  /** Ponder during the human's turn (fixed depth: the gain is speed). */
  ponderLive: () => call<void>('ponder_live'),
  evalAt: (n: number, depth: number) => call<EvalPoint>('eval_at', { n, depth }),
  /** Save; names go into the GGF (written only for .ggf). */
  /** Save the record; names only appear in GGF output. */
  saveKifu: (black: string, white: string) =>
    call<string | null>('save_kifu', { black, white }),
  loadKifu: () => call<GameView | null>('load_kifu'),
  loadKifuText: (text: string) => call<GameView>('load_kifu_text', { text }),
  previewKifu: (text: string) => call<KifuFrame[]>('preview_kifu', { text }),
  localThreads: () => call<ThreadsView>('local_threads', {}),
  setLocalThreads: (n: number | null) => call('set_local_threads', { n }),
  calibrateNps: () => call<ThreadsView>('calibrate_nps', {}),
  hashSizes: () => call<HashView>('hash_sizes', {}),
  setHashSizes: (mid: number, end: number) => call<HashView>('set_hash_sizes', { mid, end }),
  activity: () => call<ActivityView>('activity_status', {}),
  /** Hand the backend the strings it renders itself (OS notifications,
   *  native file dialogs). Re-sent whenever the language changes. */
  setBackendStrings: (strings: Record<string, string>) =>
    call('set_backend_strings', { strings }),
};

/** The record expanded into per-move boards (viewing only). */
export interface KifuFrame {
  cells: number[];
  last: number | null;
  black: number;
  white: number;
  player: string;
}

/** Thread setting (null set = auto). */
/** Table sizes (2^bits) and their combined memory. */
export interface HashView {
  mid: number; end: number; min: number; max: number; bytes: number;
}

export interface ThreadsView {
  set: number | null;
  auto: number;
  /** Calibrated solve speed (nodes/sec); null if unmeasured. */
  nps: number | null;
  /** Thread count changed since measurement (stale value unused). */
  nps_stale: boolean;
}

/** What currently uses the CPU (nav display). */
export interface ActivityView {
  /** Local search kind; null if none. */
  local: string | null;
  local_threads: number;
  /** Learning import [done, total]. */
  learn: [number, number] | null;
  learn_paused: boolean;
  ggs_match: boolean;
  ggs_thinking: boolean;
  ggs_threads: number;
  /** Process CPU usage (%); 100% = one core. */
  cpu: number;
  /** Core count (usage ceiling = cores x 100%). */
  cores: number;
  /** Resident and total physical memory (bytes). */
  mem: number;
  mem_total: number;
}

/* ============================ GGS ============================ */

export const ggsApi = {
  connect: (login: string, pw: string) => call<string>('ggs_connect', { login, pw }),
  disconnect: () => call('ggs_disconnect'),
  snapshot: () => call<GgsSnapshot>('ggs_snapshot'),
  raw: (cmd: string) => call('ggs_raw', { cmd }),
  ask: (gtype: string, time: string, opponent: string, rated: boolean) =>
    call('ggs_ask', { gtype, time, opponent, rated }),
  accept: (id: string) => call('ggs_accept', { id }),
  decline: (id: string) => call('ggs_decline', { id }),
  finger: (name: string) => call('ggs_finger', { name }),
  who: () => call('ggs_who', {}),
  top: (gtype: string, n: number) => call('ggs_top', { gtype, n }),
  rank: (gtype: string, name: string) => call('ggs_rank', { gtype, name }),
  watch: (id: string, on: boolean) => call('ggs_watch', { id, on }),
  closeMatch: (id: string) => call('ggs_close_match', { id }),
  look: (id: string) => call('ggs_look', { id }),
  /** Clear the notice and fetched record (never leave them up). */
  ack: () => call('ggs_ack', {}),
  autoview: () => call<string>('ggs_autoview', {}),
  /** Whether rated play is forbidden (KUROOBI_NO_RATED=1). */
  noRated: () => call<boolean>('ggs_no_rated', {}),
  chat: (target: string, text: string) => call('ggs_chat', { target, text }),
  matchCmd: (id: string, verb: 'undo' | 'abort' | 'resign' | 'tell', arg = '') =>
    call('ggs_match_cmd', { id, verb, arg }),
  setFormula: (kind: 'aform' | 'dform', expr: string) =>
    call('ggs_set_formula', { kind, expr }),
  listStored: () => call('ggs_list_stored'),
  listMatches: () => call('ggs_list_matches'),
  resumeStored: (id: string) => call('ggs_resume_stored', { id }),
  history: (name: string) => call('ggs_history', { name }),
  /** Advance the chat read marker (unix secs); survives restarts. */
  chatSeen: (at: number) => call('ggs_chat_seen', { at }),
  setEngine: (depth: number, solve: number, band: number, ponder: boolean) =>
    call('ggs_set_engine', { depth, solve, band, ponder }),
  setPacing: (pace: string, maxMoveSecs: number, reserveSecs: number, budgetUse: number) =>
    call('ggs_set_pacing', { pace, maxMoveSecs, reserveSecs, budgetUse }),
  setAutoPlay: (on: boolean) => call('ggs_set_auto_play', { on }),
  setWatchAnalysis: (on: boolean) => call('ggs_set_watch_analysis', { on }),
  setUseBook: (on: boolean) => call('ggs_set_use_book', { on }),
  setLearn: (on: boolean) => call('ggs_set_learn', { on }),
  setStandby: (cfg: StandbyCfg) => call('ggs_set_standby', { cfg }),
  /** Save; names go into the GGF (written only for .ggf). */
  saveKifu: (kifu: string, name: string) =>
    call<string | null>('ggs_save_kifu', { kifu, name }),
  /** Save the wire log (separate from records: different filters/names). */
  saveLog: (text: string) => call<string | null>('ggs_save_log', { text }),
};

/** Subscribe to analysis progress (depth, all-move evals, nodes, seconds). */
export async function onHints(
  fn: (depth: number, hints: HintView[], nodes: number, secs: number) => void,
): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) throw new Error('Tauri events unavailable');
  return ev.listen<[number, HintView[], number, number]>(
    'hints',
    (e) => fn(e.payload[0], e.payload[1], e.payload[2], e.payload[3]),
  );
}

/** Subscribe to backend state updates; the return value unsubscribes. */
export async function onGgsSnapshot(fn: (s: GgsSnapshot) => void): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) throw new Error('Tauri events unavailable');
  return ev.listen<GgsSnapshot>('ggs', (e) => fn(e.payload));
}
