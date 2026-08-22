// GGS state and shared helpers. Subscribes to the snapshots the
// backend (gui/src/ggs.rs) streams over the "ggs" Tauri event; screens
// derive everything from them.

import { useCallback, useEffect, useRef, useState } from 'react';
import { ggsApi, jsLog, onGgsSnapshot } from './api';
import type { GgsSnapshot, MatchView, PlayerView } from './types';

/* ---------------- Snapshot subscription ---------------- */

export function useGgs() {
  const [snap, setSnap] = useState<GgsSnapshot | null>(null);
  // Whether demo fixtures were applied; backend pushes are ignored
  // afterwards (the disconnected stream would fight the fixtures and
  // flicker).
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

  /// Screenshot fixtures (KUROOBI_GGS_AUTOVIEW); sticky once applied —
  /// being washed away by real events would defeat the purpose.
  const patch = useCallback((p: Partial<GgsSnapshot>) => {
    demo.current = true;
    setSnap((prev) => (prev ? { ...prev, ...p } : prev));
  }, []);

  return { snap, patch };
}

/* ---------------- Clocks ---------------- */
// Re-baseline on every server update and tick down only the mover's
// clock. Baseline and now live in state; display math is pure (no
// Date.now() during render).

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
  /* No clocks on adjourned/aborted games — "overtime"/"flagged" labels
     would be lies; GGS freezes their clocks too. */
  if (m.ended === 'adjourned' || m.ended === 'aborted') return { text: '', cls: '' };
  /* Freeze finished games' clocks: the last update can hand the turn
     back and the local tick once ran on, displaying a bogus overtime
     that was genuinely mistaken for the real thing. */
  const active = !m.over && !!m.turn && !!color && m.turn === color;
  if (base === null) return { text: raw || '', cls: active ? 'turn' : '' };
  const rem = base - (active ? (now - c.at) / 1000 : 0);
  /* Overtime is invisible in the remaining time: the server adds the
     grace onto the single clock, so a healthy-looking `01:30` can be
     overtime. Only the engine-side `in_overtime` flag knows. */
  const mine = side === 'my' || (!!color && color === m.my_color);
  if (mine && m.in_overtime && rem >= 0) return { text: `ロス ${fmtSecs(rem)}`, cls: 'ext' };
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
        // Re-baseline only when seen advanced (a re-sent board must
        // not rewind the clock).
        bases[m.id] = old && old.match.seen === m.seen ? old : { match: m, at: Date.now() };
      }
      return { bases, now: Date.now() };
    });
    // Show immediately on first render, without waiting for a tick.
    const t0 = window.setTimeout(sync, 0);
    const t = window.setInterval(sync, 500);
    return () => { clearTimeout(t0); clearInterval(t); };
  }, [matches]);

  return useCallback(
    (id: string, side: ClockSide) => clockView(state.bases[id], side, state.now),
    [state],
  );
}

/* ---------------- Formatting ---------------- */

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
  // Rating pools are coarser than game types (two of them), and a raw
  // "8r" means nothing on screen.
  '8r': 'ランダム開局',
};
export const gtypeLabel = (t: string): string => GTYPE[t] ?? (t || '?');

/// Game types and clocks offerable from the lobby and standby. They
/// used to be two diverging arrays (some types offerable in one place
/// only); unified to the wider set in one place.
export const GTYPE_CHOICES: [string, string][] = [
  ['s8r16', '同期・ランダム16手 (推奨)'],
  ['s8r18', '同期・ランダム18手'],
  ['s8r20', '同期・ランダム20手'],
  ['s8', '同期・通常開局'],
  ['8', '通常 (1局)'],
  ['8r16', '通常・ランダム16手'],
];
export const CLOCK_CHOICES: [string, string][] = [
  ['00:05:00', '5 分'],
  ['00:10:00', '10 分'],
  ['00:15:00', '15 分'],
  ['00:20:00', '20 分'],
  ['00:30:00', '30 分'],
];

/** Unix seconds -> "14:03". */
export function clockOf(at: number): string {
  return new Date(at * 1000).toLocaleTimeString('ja-JP',
    { hour: '2-digit', minute: '2-digit' });
}

/* ---------------- Records ---------------- */

/** GGS move ("F5"/"pa") to a file-major square index; pass = null. */
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

/* ---------------- Translation ---------------- */

export async function translate(text: string, target: string): Promise<string> {
  const url = 'https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=' +
    target + '&dt=t&q=' + encodeURIComponent(text);
  const r = await fetch(url);
  if (!r.ok) throw new Error('translate ' + r.status);
  // gtx replies are nested arrays: [[[translated, original, ...], ...], ...]
  const j = (await r.json()) as [[string, string][]];
  return (j[0] ?? []).map((x) => x[0]).join('');
}

export const hasJapanese = (t: string): boolean => /[぀-ヿ一-鿿]/.test(t);

/* ---------------- Finger field rendering ---------------- */

/// Finger keys can contain spaces, like "stored (-)".
export const normKey = (k: string): string => k.replace(/\s+/g, '');

/* Finger field labels. Raw server keys (dblen, vt100...) are
 * unreadable; screen text stays Japanese per the UI convention. */
const FINGER_LABEL: Record<string, string> = {
  // --- What you want to see before offering a game ---
  open: '申し込み受付', accept: '自動で受ける条件', decline: '自動で断る条件',
  'request(+)': '募集中の条件', 'request(-)': '募集中の条件',
  rated: 'レート戦', play: '対局の状況',
  'stored(+)': '中断中の対局', 'stored(-)': '中断中の対局',
  // --- Identity ---
  name: '登録名', info: '備考', email: 'メール', since: '接続開始',
  idle: '無操作の時間', host: 'ホスト', dblen: '定石どおりの手',
  // --- Settings and state ---
  level: 'アクセス権限', trust: '信用', client: 'クライアント',
  sock: '接続方式', bell: '通知を受け取るもの', hear: '発言の受信', vt100: 'VT100 表示',
  'watch(+)': '観戦中の対局', 'watch(-)': '観戦中の対局',
  'track(+)': '入退室を知らせる相手', 'track(-)': '入退室を知らせる相手',
  'groups(+)': '所属グループ', 'groups(-)': '所属グループ',
  'channs(+)': '参加チャンネル', 'channs(-)': '参加チャンネル',
  'notify(+)': '通知を受け取る相手', 'notify(-)': '通知を受け取る相手',
  'ignore(+)': '無視している相手', 'ignore(-)': '無視している相手',
};

/** Fields meaningless on screen (credentials, command echoes). */
const FINGER_HIDDEN = ['passw', 'password', 'login', '/os', 'sock'];

/** Pre-offer fields; empty values still render as "unset". */
const FINGER_ALWAYS = ['open', 'accept', 'decline', 'request(+)', 'request(-)'];

/* Field groups; ordering is decided here too. Unlisted fields go to
 * the end of Settings — a uniform 24-row list buries what matters
 * before an offer. */
const FINGER_GROUPS: { title: string; keys: string[] }[] = [
  {
    title: '対局の申し込み',
    keys: ['open', 'accept', 'decline', 'request(+)', 'request(-)', 'rated',
           'play', 'stored(+)', 'stored(-)'],
  },
  { title: '素性', keys: ['name', 'info', 'email', 'since', 'idle', 'host', 'dblen'] },
  {
    title: '設定',
    keys: ['level', 'trust', 'client', 'vt100', 'hear', 'bell', 'groups(+)',
           'groups(-)', 'channs(+)', 'channs(-)', 'notify(+)', 'notify(-)',
           'watch(+)', 'watch(-)', 'track(+)', 'track(-)', 'ignore(+)', 'ignore(-)'],
  },
];

/** One profile row; `label` is display text, `raw` the original key
 * (used by formula logic). */
export interface FingerRow { key: string; label: string; value: string }

/** Regroup raw finger fields and attach display labels. */
export function fingerGroups(fields: [string, string][]): { title: string; rows: FingerRow[] }[] {
  const got = new Map(fields.map(([k, v]) => [normKey(k), v]));
  const used = new Set<string>();
  const row = (k: string): FingerRow | null => {
    if (FINGER_HIDDEN.includes(k.replace(/\(.*\)/, ''))) return null;
    const v = got.get(k);
    if (v === undefined && !FINGER_ALWAYS.includes(k)) return null;
    used.add(k);
    return { key: k, label: FINGER_LABEL[k] ?? k, value: v ?? '' };
  };
  /* `foo(+)` and `foo(-)` share a label (the server returns both
   * spellings); keep only the one with a value. */
  const dedupe = (rows: FingerRow[]): FingerRow[] => {
    const seen = new Map<string, FingerRow>();
    for (const r of rows) {
      const cur = seen.get(r.label);
      if (!cur || (!cur.value.trim() && r.value.trim())) seen.set(r.label, r);
    }
    return [...seen.values()];
  };
  const out = FINGER_GROUPS.map((g) => ({
    title: g.title,
    rows: dedupe(g.keys.map(row).filter((r): r is FingerRow => r !== null)),
  }));
  // Unlisted fields are kept at the end of Settings — silently
  // vanishing when the server adds fields would be worse.
  const rest = [...got.entries()]
    .filter(([k]) => !used.has(k) && !FINGER_HIDDEN.includes(k.replace(/\(.*\)/, '')))
    .map(([k, v]) => ({ key: k, label: FINGER_LABEL[k] ?? k, value: v }));
  out[out.length - 1].rows.push(...dedupe(rest));
  return out.filter((g) => g.rows.length > 0);
}

// GGS symbols are unreadable raw; rephrase them.
export function fingerValue(k: string, v: string): string {
  const key = normKey(k).replace(/\(.*\)/, '');
  if (key === 'open') return v === '0' || v === '-' ? '受け付けていない' : '受け付ける';
  if (key === 'rated' || key === 'trust') return v === '+' ? 'あり' : 'なし';
  // accept/decline/request never reach here: they are formulas and
  // FingerValue renders them as trees.
  if (key === 'notify') return v === '/os' ? 'リバーシサービス全体' : (v.trim() || 'なし');
  if (key === 'play') return v === '-' || !v ? '対局していない' : `対局中 (${v})`;
  if (key === 'client') return v === '+' ? '専用クライアント' : 'telnet など';
  if (key === 'hear') return v === '+' ? '受け取る' : '受け取らない';
  if (key === 'vt100') return v === '+' ? '対応' : '非対応';
  if (key === 'level') return v === '1' ? '一般' : v;
  if (key === 'dblen') {
    // "100.0 = 2,862 / 2,862" = share of moves matching the public
    // game database.
    const m = /([\d.]+)\s*=\s*([\d,]+)\s*\/\s*([\d,]+)/.exec(v);
    return m ? `${m[1]}% (${m[2]} / ${m[3]} 手が一致)` : v;
  }
  if (key === 'groups') return v === '_client' ? 'クライアント (対局プログラム)' : v;
  if (key === 'bell') return readBell(v);
  if (key === 'since' || key === 'idle') return readTime(k, v);
  if (['track', 'watch', 'groups', 'channs', 'notify', 'ignore', 'stored'].includes(key)) {
    return v.trim() || 'なし';
  }
  return v;
}

/// Notification settings (`-r -p -w ...`) are symbol soup; list only
/// the enabled ones.
const BELL_LABEL: Record<string, string> = {
  r: '対局の申し込み', p: '個人あての発言', w: '観戦中の対局', n: 'お知らせ',
  ns: '対局開始', nn: '新しい対局', nt: '手番', ni: '中断', nr: '再開', nw: '観戦',
  ta: '全体の発言', to: '対局中の発言', tp: '個人あて',
};
function readBell(v: string): string {
  const on = v.split(/\s+/).filter((t) => t.startsWith('+')).map((t) => t.slice(1));
  const names = on.map((k) => BELL_LABEL[k] || k).filter(Boolean);
  return names.length ? names.join('、') : 'すべて切っている';
}

/// Convert GGS time strings for display.
/// `since` is "Thu 30 Jul 2026 17:39:06 MDT"; `idle` is
/// "00:14:02, on line : 1.09:59:08".
function readTime(k: string, v: string): string {
  if (k === 'since') {
    const t = Date.parse(v.replace(/\s*[A-Z]{3}$/, ' GMT-0600'));
    if (!Number.isNaN(t)) {
      return new Date(t).toLocaleString('ja-JP', {
        year: 'numeric', month: 'long', day: 'numeric',
        hour: '2-digit', minute: '2-digit', weekday: 'short',
      });
    }
    return v;
  }
  // idle: leading part is inactivity, "on line" is connection age.
  const m = /^([\d:.]+)(?:,\s*on line\s*:\s*([\d:.]+))?/.exec(v.trim());
  if (!m) return v;
  const span = (x: string): string => {
    const [d, hms] = x.includes('.') ? x.split('.') : ['0', x];
    const [h, mi] = hms.split(':').map(Number);
    const parts: string[] = [];
    if (+d) parts.push(`${+d} 日`);
    if (h) parts.push(`${h} 時間`);
    parts.push(`${mi ?? 0} 分`);
    return parts.join(' ');
  };
  const idle = span(m[1]);
  return m[2] ? `${idle} (接続してから ${span(m[2])})` : idle;
}

/* ---------------- Formula rendering ---------------- */
// Render accept/decline formulas for display. Notation per
// `tell /os help formula`; m* = us, o* = the opponent.

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

/// Render one leaf (`size!=8`, `!saved`); the tree stays intact, so
/// `&`/`|` are not handled here.
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

/** Formula tree; `all` = every condition (&), `any` = any (|). */
export type Formula =
  | { kind: 'all' | 'any'; kids: Formula[] }
  | { kind: 'atom'; text: string; src: string };

/* ---- Vocabulary for building formulas ---- */

/** Variables usable in conditions; the editor picks input widgets by `type`. */
export interface FormulaVar {
  name: string;
  label: string;
  /** bool = as-is / num = comparison + value / color = black/white/any. */
  type: 'bool' | 'num' | 'color';
  /** Numeric unit (display only). */
  unit?: string;
  /** Numeric default. */
  def?: number;
}

/// Available variables: the subset of `tell /os help formula` that
/// matters for judging offers, in display order.
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

/** Screen comparison symbols -> GGS notation. */
const OP_SRC: Record<FormulaOp, string> = {
  '=': '=', '≠': '!=', '<': '<', '>': '>', '≤': '<=', '≥': '>=',
};
const SRC_OP: Record<string, FormulaOp> = {
  '=': '=', '==': '=', '!=': '≠', '<': '<', '>': '>', '<=': '≤', '>=': '≥',
};

/** One condition being edited; kept as a tree, stringified only on save. */
export type Cond =
  | { kind: 'all' | 'any'; kids: Cond[] }
  | { kind: 'atom'; name: string; op: FormulaOp; val: string; neg: boolean };

export const varOf = (name: string): FormulaVar | undefined =>
  FORMULA_VARS.find((v) => v.name === name);

/// Color options. GGS notation is `*` = black / `O` = white; `b`/`w`
/// are rejected. Distinct from the screen's stone colors — never mix.
export const COLOR_CHOICES: [string, string][] = [
  ['?', 'おまかせ'], ['*', '黒'], ['O', '白'],
];

/// Boolean options as [negated?, label], phrased to match the color
/// options.
export const BOOL_OPS: [boolean, string][] = [[false, 'である'], [true, 'ではない']];

/** Bundles (`&`/`|`) vs leaves, split by type. */
export const isGroup = (c: Cond): c is { kind: 'all' | 'any'; kids: Cond[] } =>
  c.kind !== 'atom';

/// Render one leaf (for the read-only tree); same vocabulary as
/// `readAtom`, different entry point.
export function condLabel(c: Cond): string {
  if (isGroup(c)) return c.kind === 'all' ? 'すべて満たす' : '次のどれか';
  const v = varOf(c.name);
  const label = v?.label ?? c.name;
  if (!v || v.type === 'bool') return label + (c.neg ? ' ではない' : '');
  if (v.type === 'color') {
    const name = COLOR_CHOICES.find(([x]) => x === c.val)?.[1] ?? c.val;
    return `${label} ${c.op === '≠' ? 'ではない' : 'である'} ${name}`;
  }
  return `${label} ${c.op} ${c.val}${v.unit ?? ''}`;
}

/// Parse one leaf into editable form. Unknown spellings keep their
/// name rather than degrading — re-saving must not clobber someone
/// else's setting.
function parseAtomSrc(src: string): Cond {
  const m = /^\s*(!?)\s*([A-Za-z][A-Za-z0-9]*)\s*(==|!=|<=|>=|<|>|=)?\s*(.*?)\s*$/.exec(src);
  if (!m) return { kind: 'atom', name: src.trim(), op: '=', val: '', neg: false };
  const [, bang, name, op, rawVal] = m;
  // `ml1==F`/`ml1==T` are boolean spellings; treating them as
  // comparisons renders awkwardly.
  if (op === '==' && /^[TF]$/.test(rawVal)) {
    return { kind: 'atom', name, op: '=', val: '', neg: rawVal === 'F' };
  }
  if (!op) return { kind: 'atom', name, op: '=', val: '', neg: bang === '!' };
  return { kind: 'atom', name, op: SRC_OP[op] ?? '=', val: rawVal, neg: bang === '!' };
}

/** Parse a formula into an editable tree. */
export function parseCond(src: string): Cond | null {
  const body = src.replace(/^\s*:\s*/, '').trim();
  if (!body) return null;
  const walk = (n: Formula): Cond =>
    n.kind === 'atom' ? parseAtomSrc(n.src) : { kind: n.kind, kids: n.kids.map(walk) };
  return walk(parseFormula(body));
}

/** Serialize the edited tree back to GGS notation; empty bundles drop. */
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
  // `&` binds tighter; parentheses only when an `|` bundle nests in `&`.
  return parts.map((p) => (c.kind === 'all' && p.includes('|') ? `(${p})` : p)).join(sep);
}

/// Extract the formula as a tree. The source is a boolean expression
/// like `!saved & (size!=8 | anti | ...) | !rated` whose structure IS
/// its meaning; flattened to a sentence it becomes a five-line
/// incantation. Nesting replaces parentheses. `&` binds tighter than
/// `|` (per `tell /os help formula`).
function parseFormula(src: string): Formula {
  // Drop whitespace-only fragments — otherwise the space in `& (`
  // becomes a leaf and the following group renders as an empty card.
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
    // A single item is not bundled (no "all of: 1 item").
    return kids.length === 1 ? kids[0] : { kind, kids };
  };
  const and = () => join('all', '&', atom);
  const or = () => join('any', '|', and);
  return or();
}

