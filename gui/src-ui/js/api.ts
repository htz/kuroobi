// バックエンドとの入出力。

import type { EvalPoint, GameView, GgsSnapshot, HintView, StandbyCfg, ThinkView } from './types';

const core = () => window.__TAURI__?.core;

/** JS の例外をバックエンドのログへ送る (WebView のコンソールが見えない環境用)。 */
export function jsLog(msg: unknown): void {
  try {
    void core()?.invoke('js_log', { msg: String(msg) });
  } catch {
    /* ログ送信の失敗は無視する */
  }
}

async function call<T = void>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const c = core();
  if (!c) throw new Error('Tauri IPC が使えません');
  return c.invoke<T>(cmd, args);
}

export const api = {
  state: () => call<GameView>('state'),
  newGame: () => call<GameView>('new_game'),
  play: (sq: number) => call<GameView>('play', { sq }),
  undo: () => call<GameView>('undo'),
  goto: (n: number) => call<GameView>('goto', { n }),
  setUseBook: (on: boolean) => call<void>('set_use_book', { on }),
  setLearn: (on: boolean) => call<void>('set_learn', { on }),
  learnGame: () => call<void>('learn_game'),
  hasBook: () => call<boolean>('has_book', {}),
  autoplay: () => call<string>('autoplay', {}),
  resourceStatus: () => call<[string, string, boolean][]>('resource_status', {}),
  pickResource: (kind: string) => call<string | null>('pick_resource', { kind }),
  setResource: (kind: string, path: string | null) =>
    call<void>('set_resource', { kind, path }),
  setLevels: (depth: number, solveEmpties: number, band: number) =>
    call('set_levels', { depth, solveEmpties, band }),
  stopSearch: () => call('stop_search'),
  think: () => call<ThinkView>('think'),
  applyMove: (sq: number | null) => call<GameView>('apply_move', { sq }),
  analyze: (depth: number) => call<HintView[]>('analyze', { depth }),
  evalAt: (n: number, depth: number) => call<EvalPoint>('eval_at', { n, depth }),
  saveKifu: () => call<string | null>('save_kifu'),
  loadKifu: () => call<GameView | null>('load_kifu'),
  loadKifuText: (text: string) => call<GameView>('load_kifu_text', { text }),
  localThreads: () => call<ThreadsView>('local_threads', {}),
  setLocalThreads: (n: number | null) => call('set_local_threads', { n }),
  activity: () => call<ActivityView>('activity_status', {}),
};

/** スレッド数の設定 (set が null なら自動 = auto の値)。 */
export interface ThreadsView { set: number | null; auto: number }

/** いま何が CPU を使っているか (ナビの常時表示)。 */
export interface ActivityView {
  /** ローカル探索の種別 (思考 / 解析 / 採点)。無ければ null。 */
  local: string | null;
  local_threads: number;
  /** 学習の取り込み [済み, 総数]。 */
  learn: [number, number] | null;
  learn_paused: boolean;
  ggs_match: boolean;
  ggs_thinking: boolean;
  ggs_threads: number;
  /** プロセス全体の CPU 使用率 (%)。100% = 1 コア。 */
  cpu: number;
  /** マシンのコア数 (使用率の上限は cores × 100%)。 */
  cores: number;
  /** 使用中の物理メモリと、積んでいる総量 (バイト)。 */
  mem: number;
  mem_total: number;
}

/* ============================ GGS ============================ */

export const ggsApi = {
  connect: (login: string, pw: string) => call<string>('ggs_connect', { login, pw }),
  disconnect: () => call('ggs_disconnect'),
  snapshot: () => call<GgsSnapshot>('ggs_snapshot'),
  raw: (cmd: string) => call('ggs_raw', { cmd }),
  ask: (gtype: string, time: string, opponent: string) =>
    call('ggs_ask', { gtype, time, opponent }),
  accept: (id: string) => call('ggs_accept', { id }),
  decline: (id: string) => call('ggs_decline', { id }),
  finger: (name: string) => call('ggs_finger', { name }),
  who: (gtype: string) => call('ggs_who', { gtype }),
  top: (gtype: string, n: number) => call('ggs_top', { gtype, n }),
  rank: (gtype: string, name: string) => call('ggs_rank', { gtype, name }),
  watch: (id: string, on: boolean) => call('ggs_watch', { id, on }),
  look: (id: string) => call('ggs_look', { id }),
  autoview: () => call<string>('ggs_autoview', {}),
  chat: (target: string, text: string) => call('ggs_chat', { target, text }),
  matchCmd: (id: string, verb: 'undo' | 'abort' | 'resign' | 'tell', arg = '') =>
    call('ggs_match_cmd', { id, verb, arg }),
  setFormula: (kind: 'aform' | 'dform', expr: string) =>
    call('ggs_set_formula', { kind, expr }),
  listStored: () => call('ggs_list_stored'),
  listMatches: () => call('ggs_list_matches'),
  resumeStored: (id: string) => call('ggs_resume_stored', { id }),
  history: (name: string) => call('ggs_history', { name }),
  setEngine: (depth: number, solve: number, band: number, threads: number) =>
    call('ggs_set_engine', { depth, solve, band, threads }),
  setAutoPlay: (on: boolean) => call('ggs_set_auto_play', { on }),
  setWatchAnalysis: (on: boolean) => call('ggs_set_watch_analysis', { on }),
  setUseBook: (on: boolean) => call('ggs_set_use_book', { on }),
  setLearn: (on: boolean) => call('ggs_set_learn', { on }),
  setStandby: (cfg: StandbyCfg) => call('ggs_set_standby', { cfg }),
  saveKifu: (kifu: string, name: string) =>
    call<string | null>('ggs_save_kifu', { kifu, name }),
};

/** バックエンドからの状態更新を購読する。戻り値で購読を解除できる。 */
export async function onGgsSnapshot(fn: (s: GgsSnapshot) => void): Promise<() => void> {
  const ev = window.__TAURI__?.event;
  if (!ev) throw new Error('Tauri event が使えません');
  return ev.listen<GgsSnapshot>('ggs', (e) => fn(e.payload));
}
