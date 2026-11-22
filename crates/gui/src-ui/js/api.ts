import type { Account } from './components/accounts';
import type { BookNode, ClockView, EvalPoint, FsResult, FsSnapshot, GameResult, GameView, GgsSnapshot, HintView, LearnEntry, StandbyCfg, ThinkView } from './types';

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

/** The control socket that `kuroobi-mcp` (shell or MCP server) drives the app through. */
export interface ControlInfo {
  enabled: boolean; active: boolean; forced_off: boolean; socket: string; ctl: string;
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
  engineReady: () => call<void>('engine_ready', {}),
  controlInfo: () => call<ControlInfo>('control_info', {}),
  setControl: (on: boolean) => call<void>('set_control', { on }),
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
  exportKifu: (format: ExportFormat, black: string, white: string) =>
    call<string>('export_kifu', { format, black, white }),
  saveKifuText: (text: string, ggf: boolean) => call<string | null>('save_kifu_text', { text, ggf }),
  readKifuFile: () => call<{ name: string; text: string } | null>('read_kifu_file'),
  loadKifuText: (text: string) => call<GameView>('load_kifu_text', { text }),
  loadBoard: (board: string) => call<GameView>('load_board', { board }),
  previewKifu: (text: string) => call<KifuPreview>('preview_kifu', { text }),
  localThreads: () => call<ThreadsView>('local_threads', {}),
  setLocalThreads: (n: number | null) => call('set_local_threads', { n }),
  hashSizes: () => call<HashView>('hash_sizes', {}),
  setHashSizes: (mid: number, end: number) => call<HashView>('set_hash_sizes', { mid, end }),
  activity: () => call<ActivityView>('activity_status', {}),
  setBackendStrings: (strings: Record<string, string>) =>
    call('set_backend_strings', { strings }),
};

export type ExportFormat = 'moves' | 'ggf' | 'board_moves' | 'board';

/** What a pasted or opened text was read as, and the board after every move of it. */
export interface KifuPreview {
  format: 'moves' | 'board' | 'board_moves' | 'ggf' | 'quest';
  frames: KifuFrame[];
}

export interface KifuFrame {
  cells: number[];
  last: number | null;
  black: number;
  white: number;
  turn: 'black' | 'white';
}

export interface HashView {
  mid: number; end: number; min: number; max: number;
  /** Bytes an entry of the clock search's and the solver's table takes. */
  entry: [number, number];
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

export const fsApi = {
  snapshot: () => call<FsSnapshot>('fs_snapshot'),
  add: (token: string, cap: number | null, threads: number | null, level: [number, number] | null,
        hashMid: number | null, hashEnd: number | null) =>
    call('fs_add', { token, cap, threads, level, hashMid, hashEnd }),
  connect: (id: number) => call('fs_connect', { id }),
  disconnect: (id: number) => call('fs_disconnect', { id }),
  remove: (id: number) => call('fs_remove', { id }),
  /** `cap` 0 is the bot's own setting on the site; `level` [0, 0] plays by the clock. */
  /** `hashMid`, `hashEnd`: table bits, 0 for the app's setting. */
  set: (id: number, s: { cap?: number; threads?: number; book?: boolean; accept?: boolean; level?: [number, number];
                         hashMid?: number; hashEnd?: number; ponder?: boolean; learn?: boolean }) =>
    call('fs_set', { id, ...s }),
  challenge: (id: number, target: number) => call('fs_challenge', { id, target }),
  changeToken: (id: number, token: string) => call('fs_change_token', { id, token }),
  clearPending: () => call('fs_clear_pending'),
  /** Reads a disconnected bot's settings on the site again. */
  refresh: (id: number) => call('fs_refresh', { id }),
  /** Reads the site's records of a disconnected bot's games since the last one kept. */
  sync: (id: number) => call('fs_sync', { id }),
  results: (id: number) => call<FsResult[]>('fs_results', { id }),
};

export async function onFsSnapshot(fn: (s: FsSnapshot) => void): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) throw new Error('Tauri events unavailable');
  return ev.listen<FsSnapshot>('fs', (e) => fn(e.payload));
}

export const ggsApi = {
  connect: (login: string, pw: string, server: string) => call<void>('ggs_connect', { login, pw, server }),
  disconnect: () => call('ggs_disconnect'),
  accounts: () => call<Account[]>('ggs_accounts'),
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
  results: () => call<GameResult[]>('ggs_results'),
  raw: (cmd: string) => call('ggs_raw', { cmd }),
  ask: (gtype: string, time: string, opponent: string, rated: boolean) =>
    call('ggs_ask', { gtype, time, opponent, rated }),
  accept: (id: string) => call('ggs_accept', { id }),
  decline: (id: string) => call('ggs_decline', { id }),
  finger: (name: string) => call('ggs_finger', { name }),
  setInfo: (text: string) => call('ggs_set_info', { text }),
  setList: (list: 'notify' | 'track' | 'ignore', name: string, on: boolean) => call('ggs_set_list', { list, name, on }),
  noticesSeen: (seq: number) => call('ggs_notices_seen', { seq }),
  changePassword: (old: string, next: string, id: number) => call('ggs_change_password', { old, new: next, id }),
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
  setOpenRequests: (on: boolean) => call('ggs_set_open_requests', { on }),
  setWatchAnalysis: (on: boolean) => call('ggs_set_watch_analysis', { on }),
  setPonder: (on: boolean) => call('ggs_set_ponder', { on }),
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

export type ControlRequest = { action?: string; menu?: string; category?: string };

/**
 * Serves the control socket (`crates/gui/src/control.rs`): a `call` runs a command through the same
 * `call` the screens use; a screen request goes to `screen`. Either way the answer goes back.
 */
export async function serveControl(screen: (r: ControlRequest) => unknown): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) return () => { /* outside Tauri */ };
  const answer = (id: number, run: () => unknown) => void Promise.resolve().then(run)
    .then((value) => call('control_reply', { id, ok: true, value: value ?? null }))
    .catch((e: unknown) => call('control_reply', { id, ok: false, value: String(e) }));
  // Commands take camelCase keys from the screens; the list gives them as in Rust.
  const camel = (o: Record<string, unknown>) => Object.fromEntries(
    Object.entries(o).map(([k, v]) => [k.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()), v]));
  const offs = await Promise.all([
    ev.listen<{ id: number; command: string; args: Record<string, unknown> }>('control-call', (e) =>
      answer(e.payload.id, () => call(e.payload.command, camel(e.payload.args ?? {})))),
    ev.listen<ControlRequest & { id: number }>('control-screen', (e) =>
      answer(e.payload.id, () => screen(e.payload))),
  ]);
  return () => offs.forEach((f) => f());
}

export async function onGgsSnapshot(fn: (s: GgsSnapshot) => void): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) throw new Error('Tauri events unavailable');
  return ev.listen<GgsSnapshot>('ggs', (e) => fn(e.payload));
}
