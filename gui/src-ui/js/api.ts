// バックエンドとの入出力。

import type { EvalPoint, GameView, HintView, ThinkView } from './types';

const core = () => window.__TAURI__?.core;

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
};
