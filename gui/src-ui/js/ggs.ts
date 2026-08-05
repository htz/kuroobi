// GGS の状態と共通ヘルパ。バックエンド (gui/src/ggs.rs) が Tauri イベント
// "ggs" で流すスナップショットを購読し、画面はそこから導くだけにする。

import { useCallback, useEffect, useRef, useState } from 'react';
import { ggsApi, jsLog, onGgsSnapshot } from './api';
import type { GgsSnapshot, MatchView, PlayerView } from './types';

/* ---------------- スナップショットの購読 ---------------- */

export function useGgs() {
  const [snap, setSnap] = useState<GgsSnapshot | null>(null);
  // 画面確認用のデモを貼ったか。貼った後はバックエンドの押し戻しを無視する。
  // 未接続だとサーバー側は「未接続」を繰り返し流してくるので、これが無いと
  // デモと実イベントが取り合って画面が点滅する。
  const demo = useRef(false);

  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await onGgsSnapshot((s) => {
          if (alive && !demo.current) setSnap(s);
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

  /// 画面確認用 (KUROOBI_GGS_AUTOVIEW のデモ)。**貼ったらそのまま残る** —
  /// 見た目を確かめるための経路なので、実イベントに流されると用をなさない。
  const patch = useCallback((p: Partial<GgsSnapshot>) => {
    demo.current = true;
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
  [/\banti\b/g, 'アンチ (石が少ない方が勝ち)'],
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

/// 葉 1 つ (`size!=8`、`!saved`) を日本語にする。
/// 木を保ったまま葉だけ訳すので、`&` `|` はここでは扱わない。
function readAtom(src: string): string {
  let t = ` ${src} `;
  for (const [re, word] of FORMULA_WORDS) t = t.replace(re, word);
  return t
    .replace(/([^\s()!=<>]+)\s*==\s*F\b/g, '$1 ではない')   // ml1==F
    .replace(/([^\s()!=<>]+)\s*==\s*T\b/g, '$1 である')
    .replace(/!=\s*\?/g, ' がおまかせでない')                  // mc!=?
    .replace(/!=/g, ' ≠ ')                                    // その他の比較
    .replace(/!\s*([^\s()!]+)/g, '$1 ではない')                // !saved → 中断対局 ではない
    .replace(/\s*(<=|>=|<|>)\s*/g, ' $1 ')
    .replace(/\s+/g, ' ')
    .trim();
}

/** 条件式の木。`all` = すべて満たす (&)、`any` = どれか (|)。 */
export type Formula =
  | { kind: 'all' | 'any'; kids: Formula[] }
  | { kind: 'atom'; text: string; src: string };

/* ---- 条件式を組み立てるための語彙 ---- */

/** 条件に使える変数。編集画面は入力の形をここの `type` で決める。 */
export interface FormulaVar {
  name: string;
  label: string;
  /** bool = そのもの / num = 比較と値 / color = 黒白おまかせ。 */
  type: 'bool' | 'num' | 'color';
  /** 数値の単位 (画面に添えるだけ)。 */
  unit?: string;
  /** 数値の既定値。 */
  def?: number;
}

/// 使える変数の一覧。GGS の `tell /os help formula` に載っているもののうち、
/// 対局の申し込みを判断するのに意味があるものだけ。並び順が画面の並び順。
export const FORMULA_VARS: FormulaVar[] = [
  { name: 'rated', label: 'レート戦', type: 'bool' },
  { name: 'rand', label: 'ランダム開局', type: 'bool' },
  { name: 'synchro', label: '同期対局', type: 'bool' },
  { name: 'saved', label: '中断対局', type: 'bool' },
  { name: 'komi', label: 'コミあり', type: 'bool' },
  { name: 'anti', label: 'アンチ (石が少ない方が勝ち)', type: 'bool' },
  { name: 'ml1', label: '自分は時間切れ負け', type: 'bool' },
  { name: 'ol1', label: '相手は時間切れ負け', type: 'bool' },
  { name: 'size', label: '盤の大きさ', type: 'num', def: 8 },
  { name: 'discs', label: '開局の石数', type: 'num', def: 16 },
  { name: 'mr', label: '自分のレート', type: 'num', def: 2000 },
  { name: 'or', label: '相手のレート', type: 'num', def: 2000 },
  { name: 'mt1', label: '自分の持ち時間', type: 'num', unit: '秒', def: 600 },
  { name: 'ot1', label: '相手の持ち時間', type: 'num', unit: '秒', def: 600 },
  { name: 'mt2', label: '自分の加算', type: 'num', unit: '秒', def: 0 },
  { name: 'ot2', label: '相手の加算', type: 'num', unit: '秒', def: 0 },
  { name: 'mt3', label: '自分の延長', type: 'num', unit: '秒', def: 0 },
  { name: 'ot3', label: '相手の延長', type: 'num', unit: '秒', def: 0 },
  { name: 'mm1', label: '自分の初期手数', type: 'num', def: 0 },
  { name: 'om1', label: '相手の初期手数', type: 'num', def: 0 },
  { name: 'stored', label: 'この相手との中断対局数', type: 'num', def: 0 },
  { name: 'playing', label: '自分の対局数', type: 'num', def: 0 },
  { name: 'mc', label: '自分の色', type: 'color' },
  { name: 'oc', label: '相手の色', type: 'color' },
];

export const FORMULA_OPS = ['=', '≠', '<', '>', '≤', '≥'] as const;
export type FormulaOp = (typeof FORMULA_OPS)[number];

/** 画面の比較記号 → GGS の記法。 */
const OP_SRC: Record<FormulaOp, string> = {
  '=': '=', '≠': '!=', '<': '<', '>': '>', '≤': '<=', '≥': '>=',
};
const SRC_OP: Record<string, FormulaOp> = {
  '=': '=', '==': '=', '!=': '≠', '<': '<', '>': '>', '<=': '≤', '>=': '≥',
};

/** 編集中の 1 条件。木のまま持ち、保存するときだけ文字列にする。 */
export type Cond =
  | { kind: 'all' | 'any'; kids: Cond[] }
  | { kind: 'atom'; name: string; op: FormulaOp; val: string; neg: boolean };

export const varOf = (name: string): FormulaVar | undefined =>
  FORMULA_VARS.find((v) => v.name === name);

/// 葉 1 つを編集できる形に読み解く。読めない綴りは `rated` 扱いに倒さず、
/// 名前をそのまま残す (保存し直したときに他人の設定を壊さないため)。
function parseAtomSrc(src: string): Cond {
  const m = /^\s*(!?)\s*([A-Za-z][A-Za-z0-9]*)\s*(==|!=|<=|>=|<|>|=)?\s*(.*?)\s*$/.exec(src);
  if (!m) return { kind: 'atom', name: src.trim(), op: '=', val: '', neg: false };
  const [, bang, name, op, rawVal] = m;
  // `ml1==F` / `ml1==T` は真偽の書き方。比較として持つと画面が不自然になる
  if (op === '==' && /^[TF]$/.test(rawVal)) {
    return { kind: 'atom', name, op: '=', val: '', neg: rawVal === 'F' };
  }
  if (!op) return { kind: 'atom', name, op: '=', val: '', neg: bang === '!' };
  return { kind: 'atom', name, op: SRC_OP[op] ?? '=', val: rawVal, neg: bang === '!' };
}

/** 条件式を編集できる木にする。 */
export function parseCond(src: string): Cond | null {
  const body = src.replace(/^\s*:\s*/, '').trim();
  if (!body) return null;
  const walk = (n: Formula): Cond =>
    n.kind === 'atom' ? parseAtomSrc(n.src) : { kind: n.kind, kids: n.kids.map(walk) };
  return walk(parseFormula(body));
}

/** 編集した木を GGS の記法に戻す。空の束は落とす。 */
export function condToSrc(c: Cond): string {
  if (c.kind === 'atom') {
    const v = varOf(c.name);
    if (!v || v.type === 'bool') return (c.neg ? '!' : '') + c.name;
    return `${c.name}${OP_SRC[c.op]}${c.val}`;
  }
  const parts = c.kids.map(condToSrc).filter(Boolean);
  if (!parts.length) return '';
  if (parts.length === 1) return parts[0];
  const sep = c.kind === 'all' ? '&' : '|';
  // `&` のほうが強いので、`|` の束を `&` の中へ入れるときだけ括弧が要る
  return parts.map((p) => (c.kind === 'all' && p.includes('|') ? `(${p})` : p)).join(sep);
}

/// 条件式を木のまま取り出す。
///
/// もとは `!saved & (size!=8 | anti | …) | !rated` のような論理式で、**構造が
/// そのまま意味**になっている。1 本の文に潰すと「かつ」「または」が入り混じった
/// 5 行の呪文になり、括弧の対応を目で追う羽目になる。木で返して、括弧の代わりに
/// 入れ子で見せる。
///
/// `&` が `|` より強い (GGS の `tell /os help formula` に従う)。
export function parseFormula(src: string): Formula {
  // 空白だけの断片を落とす。落とさないと `& (` の間の " " を葉として食い、
  // 続く括弧が束ねられずに空の札が出る
  const toks = (src.match(/\(|\)|&|\||[^()&|]+/g) ?? [])
    .map((t) => t.trim()).filter(Boolean);
  let i = 0;
  const atom = (): Formula => {
    if (toks[i] === '(') {
      i++;
      const inner = or();
      if (toks[i] === ')') i++;
      return inner;
    }
    const raw = toks[i++] ?? '';
    return { kind: 'atom', text: readAtom(raw), src: raw };
  };
  const join = (kind: 'all' | 'any', sep: string, next: () => Formula): Formula => {
    const kids = [next()];
    while (toks[i] === sep) { i++; kids.push(next()); }
    // 1 つしかないなら束ねない (「すべて満たす: 1 件」を出さない)
    return kids.length === 1 ? kids[0] : { kind, kids };
  };
  const and = () => join('all', '&', atom);
  const or = () => join('any', '|', and);
  return or();
}

