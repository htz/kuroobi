
import type { BookNode, ClockView, EvalPoint, GameView, GgsSnapshot, HintView, LearnEntry, StandbyCfg, ThinkView } from './types';

const core = () => window.__TAURI__?.core;

export function jsLog(msg: unknown): void {
  try {
    void core()?.invoke('js_log', { msg: String(msg) });
  } catch {
    // logging must never throw
  }
}

async function call<T = void>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const c = core();
  if (!c) throw new Error('Tauri IPC unavailable');
  return c.invoke<T>(cmd, args);
}

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
  clocks: () => call<ClockView>('clocks'),
  setClock: (secs: number) => call<ClockView>('set_clock', { secs }),
  runClock: (on: boolean) => call<ClockView>('run_clock', { on }),
  play: (sq: number) => call<GameView>('play', { sq }),
  undo: () => call<GameView>('undo'),
  goto: (n: number) => call<GameView>('goto', { n }),
  setUseBook: (on: boolean) => call<void>('set_use_book', { on }),
  setLearn: (on: boolean) => call<void>('set_learn', { on }),
  envOverrides: () => call<[string, string][]>('env_overrides'),
  learnGame: (myColor: string) => call<void>('learn_game', { myColor }),
  learnLog: () => call<LearnEntry[]>('learn_log', {}),
  learnUndo: (at: number, kifu: string) => call<number>('learn_undo', { at, kifu }),
  hasBook: () => call<boolean>('has_book', {}),
  bookNode: (kifu: string) => call<BookNode>('book_node', { kifu }),
  autoplay: () => call<string>('autoplay', {}),
  themeOverride: () => call<string>('theme_override', {}),
  langOverride: () => call<string>('lang_override', {}),
  systemLang: () => call<string>('system_lang', {}),
  resourceStatus: () => call<[string, string, boolean, number, string][]>('resource_status', {}),
  pickResource: (kind: string) => call<string | null>('pick_resource', { kind }),
  setResource: (kind: string, path: string | null) =>
    call<void>('set_resource', { kind, path }),
  setLevels: (depth: number, solveEmpties: number) =>
    call('set_levels', { depth, solveEmpties }),
  stopSearch: () => call('stop_search'),
  think: () => call<ThinkView>('think'),
  applyMove: (sq: number | null) => call<GameView>('apply_move', { sq }),
  analyzeLive: () => call<void>('analyze_live'),
  evalAt: (n: number, depth: number) =>
    call<EvalPoint>('eval_at', { n, depth }),
  saveKifu: (black: string, white: string) =>
    call<string | null>('save_kifu', { black, white }),
  loadKifu: () => call<GameView | null>('load_kifu'),
  loadKifuText: (text: string) => call<GameView>('load_kifu_text', { text }),
  loadBoard: (board: string) => call<GameView>('load_board', { board }),
  previewKifu: (text: string) => call<KifuFrame[]>('preview_kifu', { text }),
  localThreads: () => call<ThreadsView>('local_threads', {}),
  setLocalThreads: (n: number | null) => call('set_local_threads', { n }),
  hashSizes: () => call<HashView>('hash_sizes', {}),
  setHashSizes: (mid: number, end: number) => call<HashView>('set_hash_sizes', { mid, end }),
  activity: () => call<ActivityView>('activity_status', {}),
  setBackendStrings: (strings: Record<string, string>) =>
    call('set_backend_strings', { strings }),
};

export interface KifuFrame {
  cells: number[];
  last: number | null;
  black: number;
  white: number;
  turn: 'black' | 'white';
}

export interface HashView {
  mid: number; end: number; min: number; max: number;
}

export interface ThreadsView {
  set: number | null;
  auto: number;
}

export interface ActivityView {
  local: string | null;
  local_threads: number;
  learn: [number, number] | null;
  learn_paused: boolean;
  ggs_match: boolean;
  ggs_thinking: boolean;
  ggs_threads: number;
  cpu: number;
  cores: number;
  mem: number;
  mem_total: number;
}


export interface SavedServer { addr: string; name: string; public: boolean }

/** The opponent's decline formula as an outline: `group` is set (any / all) on a group line. */
export type UnfitView = {
  top: 'any' | 'all' | 'one';
  rows: { depth: number; group: boolean | null; text: string; hit: boolean }[];
};

export const ggsApi = {
  connect: (login: string, pw: string, server: string) => call<void>('ggs_connect', { login, pw, server }),
  disconnect: () => call('ggs_disconnect'),
  accounts: () => call<{ server: string; login: string; last?: number }[]>('ggs_accounts'),
  connectSaved: (server: string, login: string) => call<void>('ggs_connect_saved', { server, login }),
  forgetAccount: (server: string, login: string) => call<void>('ggs_forget_account', { server, login }),
  servers: () => call<SavedServer[]>('ggs_servers'),
  saveServer: (old: string, name: string, addr: string, isPublic: boolean) =>
    call<void>('ggs_save_server', { old, name, addr, public: isPublic }),
  forgetServer: (addr: string) => call<void>('ggs_forget_server', { addr }),
  autoAccount: () => call<[string, string] | null>('ggs_auto_account'),
  setAutoAccount: (server: string, login: string) => call<void>('ggs_set_auto_account', { server, login }),
  switchAccount: (server: string, login: string) => call<void>('ggs_switch_account', { server, login }),
  snapshot: () => call<GgsSnapshot>('ggs_snapshot'),
  raw: (cmd: string) => call('ggs_raw', { cmd }),
  ask: (gtype: string, time: string, opponent: string, rated: boolean) =>
    call('ggs_ask', { gtype, time, opponent, rated }),
  accept: (id: string) => call('ggs_accept', { id }),
  decline: (id: string) => call('ggs_decline', { id }),
  finger: (name: string) => call('ggs_finger', { name }),
  /** Why the opponent's decline formula turns this request away, laid out; null when it does not (or not known yet). */
  unfit: (gtype: string, time: string, opponent: string, rated: boolean) =>
    call<UnfitView | null>('ggs_unfit', { gtype, time, opponent, rated }),
  who: () => call('ggs_who', {}),
  top: (gtype: string, n: number) => call('ggs_top', { gtype, n }),
  rank: (gtype: string, name: string) => call('ggs_rank', { gtype, name }),
  watch: (id: string, on: boolean) => call('ggs_watch', { id, on }),
  closeMatch: (id: string) => call('ggs_close_match', { id }),
  look: (id: string) => call('ggs_look', { id }),
  ack: () => call('ggs_ack', {}),
  autoview: () => call<string>('ggs_autoview', {}),
  noRated: () => call<boolean>('ggs_no_rated', {}),
  chat: (target: string, text: string) => call('ggs_chat', { target, text }),
  matchCmd: (id: string, verb: 'undo' | 'abort' | 'break' | 'resign' | 'tell', arg = '') =>
    call('ggs_match_cmd', { id, verb, arg }),
  setFormula: (kind: 'aform' | 'dform', expr: string) =>
    call('ggs_set_formula', { kind, expr }),
  listStored: () => call('ggs_list_stored'),
  listMatches: () => call('ggs_list_matches'),
  resumeStored: (id: string) => call('ggs_resume_stored', { id }),
  history: (name: string) => call('ggs_history', { name }),
  chatSeen: (at: number) => call('ggs_chat_seen', { at }),
  setAutoPlay: (on: boolean) => call('ggs_set_auto_play', { on }),
  setOpen: (on: boolean) => call('ggs_set_open', { on }),
  setWatchAnalysis: (on: boolean) => call('ggs_set_watch_analysis', { on }),
  setLogChannel: (channel: string) => call('ggs_set_log_channel', { channel }),
  setUseBook: (on: boolean) => call('ggs_set_use_book', { on }),
  setLearn: (on: boolean) => call('ggs_set_learn', { on }),
  setStandby: (cfg: StandbyCfg) => call('ggs_set_standby', { cfg }),
  saveKifu: (kifu: string, name: string) =>
    call<string | null>('ggs_save_kifu', { kifu, name }),
  saveLog: (text: string) => call<string | null>('ggs_save_log', { text }),
};

export async function onHints(
  fn: (hints: HintView[], nodes: number, secs: number) => void,
): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) throw new Error('Tauri events unavailable');
  return ev.listen<[HintView[], number, number]>(
    'hints',
    (e) => fn(e.payload[0], e.payload[1], e.payload[2]),
  );
}

export async function onGgsSnapshot(fn: (s: GgsSnapshot) => void): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) throw new Error('Tauri events unavailable');
  return ev.listen<GgsSnapshot>('ggs', (e) => fn(e.payload));
}
