// GGS の状態と共通ヘルパ。バックエンド (gui/src/ggs.rs) が Tauri イベント
// "ggs" で流すスナップショットを購読し、画面はそこから導くだけにする。

import { useCallback, useEffect, useState } from 'react';
import { ggsApi, jsLog, onGgsSnapshot } from './api';
import type { GgsSnapshot, MatchView, PlayerView } from './types';

/* ---------------- スナップショットの購読 ---------------- */

export function useGgs() {
  const [snap, setSnap] = useState<GgsSnapshot | null>(null);

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await onGgsSnapshot((s) => {
          if (alive) setSnap(s);
        });
        const s = await ggsApi.snapshot();
        if (alive) setSnap((prev) => prev ?? s);
      } catch (e) {
        jsLog('GGS 購読の開始に失敗: ' + e);
      }
    })();
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  // 画面確認用 (KUROOBI_GGS_AUTOVIEW のデモ): 次の実イベントで上書きされる
  const patch = useCallback((p: Partial<GgsSnapshot>) => {
    setSnap((prev) => (prev ? { ...prev, ...p } : prev));
  }, []);

  return { snap, patch };
}

/* ---------------- 時計 ---------------- */
// サーバー更新のたびに基準を取り直し、手番側だけ減らして表示する。
// 基準と現在時刻を state に置き、表示の計算は純粋関数にしてある
// (render 中に Date.now() を呼ばない)。

interface ClockBase {
  match: MatchView;
  at: number;
}

interface ClockState {
  bases: Record<string, ClockBase>;
  now: number;
}

export type ClockSide = 'my' | 'opp' | 'p0' | 'p1';

export interface ClockView {
  text: string;
  cls: '' | 'turn' | 'ext' | 'dead';
}

function clockView(c: ClockBase | undefined, side: ClockSide, now: number): ClockView {
  if (!c) return { text: '', cls: '' };
  const m = c.match;
  let base: number | null;
  let ext: number | null;
  let raw: string;
  let color: string;
  if (side === 'p0' || side === 'p1') {
    const p: PlayerView | undefined = m.players[side === 'p0' ? 0 : 1];
    if (!p) return { text: '', cls: '' };
    base = p.secs; ext = p.ext; raw = p.clock; color = p.color;
  } else if (side === 'my') {
    base = m.my_secs; ext = m.my_ext; raw = m.my_clock; color = m.my_color;
  } else {
    base = m.opp_secs; ext = m.opp_ext; raw = m.opp_clock;
    color = m.my_color === 'black' ? 'white' : m.my_color === 'white' ? 'black' : '';
  }
  const active = !!m.turn && !!color && m.turn === color;
  if (base === null) return { text: raw || '', cls: active ? 'turn' : '' };
  const rem = base - (active ? (now - c.at) / 1000 : 0);
  if (rem >= 0) return { text: fmtSecs(rem), cls: active ? 'turn' : '' };
  if (ext && ext + rem >= 0) return { text: `ロス ${fmtSecs(ext + rem)}`, cls: 'ext' };
  return { text: '時間切れ', cls: 'dead' };
}

export function useClocks(matches: MatchView[]): (id: string, side: ClockSide) => ClockView {
  const [state, setState] = useState<ClockState>({ bases: {}, now: 0 });

  useEffect(() => {
    const sync = () => setState((prev) => {
      const bases: Record<string, ClockBase> = {};
      for (const m of matches) {
        const old = prev.bases[m.id];
        // seen が進んだ更新だけ基準を取り直す (同じ盤面の再送で時計を戻さない)
        bases[m.id] = old && old.match.seen === m.seen ? old : { match: m, at: Date.now() };
      }
      return { bases, now: Date.now() };
    });
    // 初回は次のティックを待たずに映す
    const t0 = window.setTimeout(sync, 0);
    const t = window.setInterval(sync, 500);
    return () => { clearTimeout(t0); clearInterval(t); };
  }, [matches]);

  return useCallback(
    (id: string, side: ClockSide) => clockView(state.bases[id], side, state.now),
    [state],
  );
}

/* ---------------- 書式 ---------------- */

export function fmtSecs(s: number): string {
  s = Math.max(0, Math.floor(s));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

const GTYPE: Record<string, string> = {
  s8r16: '同期・ランダム16手', s8r18: '同期・ランダム18手', s8r20: '同期・ランダム20手',
  s8r14: '同期・ランダム14手', s8: '同期・通常', '8': '通常', '8r16': 'ランダム16手',
};
export const gtypeLabel = (t: string): string => GTYPE[t] ?? (t || '?');

export function relTime(unixSecs: number): string {
  if (!unixSecs) return '';
  const d = Math.max(0, Date.now() / 1000 - unixSecs);
  if (d < 60) return '今';
  if (d < 3600) return `${Math.floor(d / 60)} 分前`;
  if (d < 86400) return `${Math.floor(d / 3600)} 時間前`;
  return `${Math.floor(d / 86400)} 日前`;
}

/** UNIX 秒を "14:03" にする。 */
export function clockOf(at: number): string {
  return new Date(at * 1000).toLocaleTimeString('ja-JP',
    { hour: '2-digit', minute: '2-digit' });
}

/* ---------------- 棋譜 ---------------- */

export const kifuOf = (moves: string[]): string =>
  moves.filter((x) => !/^pa/i.test(x)).map((x) => x.toLowerCase()).join('');

// 抽選オープニングの対局は着手列だけでは再生できない。GGS の標準形式
// (GGF) は開始局面を持つので、あればそのまま渡して往復させる。
export const kifuText = (ggf: string, moves: string[]): string => ggf || kifuOf(moves);

/** GGS の着手 ("F5" / "pa") をマス番号 (file-major) にする。パスは null。 */
export function ggsMoveToIndex(mv: string): number | null {
  if (/^pa/i.test(mv)) return null;
  const m = mv.trim().toLowerCase();
  const f = m.charCodeAt(0) - 97;
  const r = m.charCodeAt(1) - 49;
  if (f < 0 || f > 7 || r < 0 || r > 7) return null;
  return f * 8 + r;
}

export function countDiscs(cells: number[]): { black: number; white: number } {
  let black = 0;
  let white = 0;
  for (const v of cells) {
    if (v === 1) black++;
    else if (v === 2) white++;
  }
  return { black, white };
}

/* ---------------- 翻訳 ---------------- */

export async function translate(text: string, target: string): Promise<string> {
  const url = 'https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=' +
    target + '&dt=t&q=' + encodeURIComponent(text);
  const r = await fetch(url);
  if (!r.ok) throw new Error('translate ' + r.status);
  // gtx の応答は [[[訳文, 原文, ...], ...], ...] という入れ子配列
  const j = (await r.json()) as [[string, string][]];
  return (j[0] ?? []).map((x) => x[0]).join('');
}

export const hasJapanese = (t: string): boolean => /[぀-ヿ一-鿿]/.test(t);

/* ---------------- 条件式 (formula) の読み下し ---------------- */
// 自動受諾/拒否の条件式を日本語にする。記法は `tell /os help formula` 準拠。
// m* が自分、o* が相手。

const FORMULA_WORDS: [RegExp, string][] = [
  [/\bsaved\b/g, '中断対局'],
  [/\brated\b/g, 'レート戦'],
  [/\brand\b/g, 'ランダム開局'],
  [/\bsynchro\b/g, '同期対局'],
  [/\bkomi\b/g, 'コミあり'],
  [/\banti\b/g, 'アンチオセロ'],
  [/\bdiscs\b/g, '開局の石数'],
  [/\bsize\b/g, '盤の大きさ'],
  [/\bstored\b/g, 'この相手との中断対局数'],
  [/\bplaying\b/g, '自分の対局数'],
  [/\bmc\b/g, '自分の色'],
  [/\boc\b/g, '相手の色'],
  [/\bmt1\b/g, '自分の持ち時間(秒)'],
  [/\bot1\b/g, '相手の持ち時間(秒)'],
  [/\bmt2\b/g, '自分の加算(秒)'],
  [/\bot2\b/g, '相手の加算(秒)'],
  [/\bmt3\b/g, '自分の延長(秒)'],
  [/\bot3\b/g, '相手の延長(秒)'],
  [/\bmm1\b/g, '自分の初期手数'],
  [/\bom1\b/g, '相手の初期手数'],
  [/\bml1\b/g, '自分は時間切れ負け'],
  [/\bol1\b/g, '相手は時間切れ負け'],
  [/\bmr\b/g, '自分のレート'],
  [/\bor\b/g, '相手のレート'],
];

export function readFormula(src: string): string {
  let t = ` ${src} `;
  for (const [re, word] of FORMULA_WORDS) t = t.replace(re, word);
  return t
    .replace(/([^\s()!=<>]+)\s*==\s*F\b/g, '$1 ではない')   // ml1==F
    .replace(/([^\s()!=<>]+)\s*==\s*T\b/g, '$1 である')
    .replace(/!=\s*\?/g, ' がおまかせでない')                  // mc!=?
    .replace(/!=/g, ' ≠ ')                                    // その他の比較
    .replace(/!\s*([^\s()!]+)/g, '$1 ではない')                // !saved → 中断対局 ではない
    .replace(/\s*&\s*/g, ' かつ ')
    .replace(/\s*\|\s*/g, ' または ')
    .replace(/\s*(<=|>=|<|>)\s*/g, ' $1 ')
    .replace(/\s+/g, ' ')
    .trim();
}
