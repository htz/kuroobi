// GGS state and shared helpers. Subscribes to the snapshots the
// backend (gui/src/ggs.rs) streams over the "ggs" Tauri event; screens
// derive everything from them.

import { useCallback, useEffect, useRef, useState } from 'react';
import { ggsApi, jsLog, onGgsSnapshot } from './api';
import { t } from './i18n';
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
        jsLog('failed to subscribe to GGS: ' + e);
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
  if (mine && m.in_overtime && rem >= 0) {
    return { text: t('ggs.clock.overtime', { t: fmtSecs(rem) }), cls: 'ext' };
  }
  if (rem >= 0) return { text: fmtSecs(rem), cls: active ? 'turn' : '' };
  if (ext && ext + rem >= 0) {
    return { text: t('ggs.clock.overtime', { t: fmtSecs(ext + rem) }), cls: 'ext' };
  }
  return { text: t('ggs.clock.timeout'), cls: 'dead' };
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
    const iv = window.setInterval(sync, 500);
    return () => { clearTimeout(t0); clearInterval(iv); };
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

/* Evaluated per call so a language switch re-renders (rule: no
 * module-level tables of display text). */
const gtypeNames = (): Record<string, string> => ({
  s8r16: t('ggs.gtype.synchro_rand16'),
  s8r18: t('ggs.gtype.synchro_rand18'),
  s8r20: t('ggs.gtype.synchro_rand20'),
  s8r14: t('ggs.gtype.synchro_rand14'),
  s8: t('ggs.gtype.synchro_std'),
  '8': t('ggs.gtype.std'),
  '8r16': t('ggs.gtype.rand16'),
  // Rating pools are coarser than game types (two of them), and a raw
  // "8r" means nothing on screen.
  '8r': t('ggs.gtype.rand_opening'),
});
export const gtypeLabel = (code: string): string => gtypeNames()[code] ?? (code || '?');

/// Game types and clocks offerable from the lobby and standby. They
/// used to be two diverging arrays (some types offerable in one place
/// only); unified to the wider set in one place.
export const gtypeChoices = (): [string, string][] => [
  ['s8r16', t('ggs.gtype_choice.synchro_rand16')],
  ['s8r18', t('ggs.gtype.synchro_rand18')],
  ['s8r20', t('ggs.gtype.synchro_rand20')],
  ['s8', t('ggs.gtype_choice.synchro_std')],
  ['8', t('ggs.gtype_choice.std')],
  ['8r16', t('ggs.gtype_choice.rand16')],
];
export const clockChoices = (): [string, string][] => [
  ['00:05:00', t('ggs.clock_choice.minutes', { n: 5 })],
  ['00:10:00', t('ggs.clock_choice.minutes', { n: 10 })],
  ['00:15:00', t('ggs.clock_choice.minutes', { n: 15 })],
  ['00:20:00', t('ggs.clock_choice.minutes', { n: 20 })],
  ['00:30:00', t('ggs.clock_choice.minutes', { n: 30 })],
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

/* Language detector, not display text: kana and CJK ranges spelled as
 * escapes so the file itself carries no Japanese. */
export const hasJapanese = (s: string): boolean =>
  /[\u3040-\u30ff\u4e00-\u9fff]/.test(s);

/* ---------------- Finger field rendering ---------------- */

/// Finger keys can contain spaces, like "stored (-)".
export const normKey = (k: string): string => k.replace(/\s+/g, '');

/* Finger field labels. Raw server keys (dblen, vt100...) are
 * unreadable; built per call so a language switch re-renders. */
const fingerLabels = (): Record<string, string> => ({
  // --- What you want to see before offering a game ---
  open: t('ggs.finger.open'),
  accept: t('ggs.finger.accept'),
  decline: t('ggs.finger.decline'),
  'request(+)': t('ggs.finger.request'), 'request(-)': t('ggs.finger.request'),
  rated: t('ggs.rated'), play: t('ggs.finger.play'),
  'stored(+)': t('ggs.finger.stored'), 'stored(-)': t('ggs.finger.stored'),
  // --- Identity ---
  name: t('ggs.finger.name'), info: t('ggs.finger.info'), email: t('ggs.finger.email'),
  since: t('ggs.finger.since'), idle: t('ggs.finger.idle'), host: t('ggs.finger.host'),
  dblen: t('ggs.finger.dblen'),
  // --- Settings and state ---
  level: t('ggs.finger.level'), trust: t('ggs.finger.trust'), client: t('ggs.finger.client'),
  sock: t('ggs.finger.sock'), bell: t('ggs.finger.bell'), hear: t('ggs.finger.hear'),
  vt100: t('ggs.finger.vt100'),
  'watch(+)': t('ggs.finger.watch'), 'watch(-)': t('ggs.finger.watch'),
  'track(+)': t('ggs.finger.track'), 'track(-)': t('ggs.finger.track'),
  'groups(+)': t('ggs.finger.groups'), 'groups(-)': t('ggs.finger.groups'),
  'channs(+)': t('ggs.finger.channs'), 'channs(-)': t('ggs.finger.channs'),
  'notify(+)': t('ggs.finger.notify'), 'notify(-)': t('ggs.finger.notify'),
  'ignore(+)': t('ggs.finger.ignore'), 'ignore(-)': t('ggs.finger.ignore'),
});

/** Fields meaningless on screen (credentials, command echoes). */
const FINGER_HIDDEN = ['passw', 'password', 'login', '/os', 'sock'];

/** Pre-offer fields; empty values still render as "unset". */
const FINGER_ALWAYS = ['open', 'accept', 'decline', 'request(+)', 'request(-)'];

/* Field groups; ordering is decided here too. Unlisted fields go to
 * the end of Settings — a uniform 24-row list buries what matters
 * before an offer. `id` is the stable handle (screens single out the
 * request group); `title` is display text. */
const fingerGroupDefs = (): { id: string; title: string; keys: string[] }[] => [
  {
    id: 'request',
    title: t('ggs.match_requests'),
    keys: ['open', 'accept', 'decline', 'request(+)', 'request(-)', 'rated',
           'play', 'stored(+)', 'stored(-)'],
  },
  {
    id: 'identity',
    title: t('ggs.finger.group.identity'),
    keys: ['name', 'info', 'email', 'since', 'idle', 'host', 'dblen'],
  },
  {
    id: 'settings',
    title: t('ggs.finger.group.settings'),
    keys: ['level', 'trust', 'client', 'vt100', 'hear', 'bell', 'groups(+)',
           'groups(-)', 'channs(+)', 'channs(-)', 'notify(+)', 'notify(-)',
           'watch(+)', 'watch(-)', 'track(+)', 'track(-)', 'ignore(+)', 'ignore(-)'],
  },
];

/** One profile row; `label` is display text, `raw` the original key
 * (used by formula logic). */
export interface FingerRow { key: string; label: string; value: string }

/** Regroup raw finger fields and attach display labels. */
export function fingerGroups(
  fields: [string, string][],
): { id: string; title: string; rows: FingerRow[] }[] {
  const labels = fingerLabels();
  const got = new Map(fields.map(([k, v]) => [normKey(k), v]));
  const used = new Set<string>();
  const row = (k: string): FingerRow | null => {
    if (FINGER_HIDDEN.includes(k.replace(/\(.*\)/, ''))) return null;
    const v = got.get(k);
    if (v === undefined && !FINGER_ALWAYS.includes(k)) return null;
    used.add(k);
    return { key: k, label: labels[k] ?? k, value: v ?? '' };
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
  const out = fingerGroupDefs().map((g) => ({
    id: g.id,
    title: g.title,
    rows: dedupe(g.keys.map(row).filter((r): r is FingerRow => r !== null)),
  }));
  // Unlisted fields are kept at the end of Settings — silently
  // vanishing when the server adds fields would be worse.
  const rest = [...got.entries()]
    .filter(([k]) => !used.has(k) && !FINGER_HIDDEN.includes(k.replace(/\(.*\)/, '')))
    .map(([k, v]) => ({ key: k, label: labels[k] ?? k, value: v }));
  out[out.length - 1].rows.push(...dedupe(rest));
  return out.filter((g) => g.rows.length > 0);
}

// GGS symbols are unreadable raw; rephrase them.
export function fingerValue(k: string, v: string): string {
  const key = normKey(k).replace(/\(.*\)/, '');
  if (key === 'open') {
    return v === '0' || v === '-'
      ? t('ggs.finger.value.not_accepting') : t('ggs.finger.value.accepting');
  }
  if (key === 'rated' || key === 'trust') {
    return v === '+' ? t('ggs.finger.value.yes') : t('ggs.finger.value.no');
  }
  // accept/decline/request never reach here: they are formulas and
  // FingerValue renders them as trees.
  if (key === 'notify') {
    return v === '/os' ? t('ggs.finger.value.all_os') : (v.trim() || t('ggs.finger.value.none'));
  }
  if (key === 'play') {
    return v === '-' || !v
      ? t('ggs.finger.value.not_playing') : t('ggs.finger.value.playing_n', { v });
  }
  if (key === 'client') {
    return v === '+'
      ? t('ggs.finger.value.client_dedicated') : t('ggs.finger.value.client_telnet');
  }
  if (key === 'hear') {
    return v === '+' ? t('ggs.finger.value.receiving') : t('ggs.finger.value.not_receiving');
  }
  if (key === 'vt100') {
    return v === '+' ? t('ggs.finger.value.supported') : t('ggs.finger.value.unsupported');
  }
  if (key === 'level') return v === '1' ? t('ggs.finger.value.level_normal') : v;
  if (key === 'dblen') {
    // "100.0 = 2,862 / 2,862" = share of moves matching the public
    // game database.
    const m = /([\d.]+)\s*=\s*([\d,]+)\s*\/\s*([\d,]+)/.exec(v);
    return m ? t('ggs.finger.value.dblen', { pct: m[1], n: m[2], total: m[3] }) : v;
  }
  if (key === 'groups') return v === '_client' ? t('ggs.finger.value.group_client') : v;
  if (key === 'bell') return readBell(v);
  if (key === 'since' || key === 'idle') return readTime(k, v);
  if (['track', 'watch', 'groups', 'channs', 'notify', 'ignore', 'stored'].includes(key)) {
    return v.trim() || t('ggs.finger.value.none');
  }
  return v;
}

/// Notification settings (`-r -p -w ...`) are symbol soup; list only
/// the enabled ones.
const bellLabels = (): Record<string, string> => ({
  r: t('ggs.match_requests'), p: t('ggs.bell.private'), w: t('ggs.finger.watch'),
  n: t('ggs.bell.notices'), ns: t('ggs.bell.game_start'), nn: t('ggs.bell.new_game'),
  nt: t('ggs.bell.turn'), ni: t('ggs.state.adjourned'), nr: t('ggs.bell.resumed'),
  nw: t('ggs.observe'), ta: t('ggs.bell.public_chat'), to: t('ggs.bell.game_chat'),
  tp: t('ggs.bell.private_short'),
});
function readBell(v: string): string {
  const labels = bellLabels();
  const on = v.split(/\s+/).filter((x) => x.startsWith('+')).map((x) => x.slice(1));
  const names = on.map((k) => labels[k] || k).filter(Boolean);
  return names.length ? names.join(t('ggs.list_separator')) : t('ggs.bell.all_off');
}

/// Convert GGS time strings for display.
/// `since` is "Thu 30 Jul 2026 17:39:06 MDT"; `idle` is
/// "00:14:02, on line : 1.09:59:08".
function readTime(k: string, v: string): string {
  if (k === 'since') {
    const at = Date.parse(v.replace(/\s*[A-Z]{3}$/, ' GMT-0600'));
    if (!Number.isNaN(at)) {
      return new Date(at).toLocaleString('ja-JP', {
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
    if (+d) parts.push(t('ggs.duration.days', { n: +d }));
    if (h) parts.push(t('ggs.duration.hours', { n: h }));
    parts.push(t('ggs.duration.minutes', { n: mi ?? 0 }));
    return parts.join(' ');
  };
  const idle = span(m[1]);
  return m[2] ? t('ggs.finger.idle_online', { idle, total: span(m[2]) }) : idle;
}

/* ---------------- Formula rendering ---------------- */
// Render accept/decline formulas for display. Notation per
// `tell /os help formula`; m* = us, o* = the opponent.

/* Substituted in order, so a replacement must never contain another
 * entry's identifier. */
const formulaWords = (): [RegExp, string][] => [
  [/\bsaved\b/g, t('ggs.formula.saved')],
  [/\brated\b/g, t('ggs.formula.rated')],
  [/\brand\b/g, t('ggs.formula.rand')],
  [/\bsynchro\b/g, t('ggs.formula.synchro')],
  [/\bkomi\b/g, t('ggs.formula.komi')],
  [/\banti\b/g, t('ggs.formula.anti')],
  [/\bdiscs\b/g, t('ggs.formula.discs')],
  [/\bsize\b/g, t('ggs.formula.size')],
  [/\bstored\b/g, t('ggs.formula.stored')],
  [/\bplaying\b/g, t('ggs.formula.playing')],
  [/\bmc\b/g, t('ggs.formula.mc')],
  [/\boc\b/g, t('ggs.formula.oc')],
  [/\bmt1\b/g, t('ggs.formula.mt1')],
  [/\bot1\b/g, t('ggs.formula.ot1')],
  [/\bmt2\b/g, t('ggs.formula.mt2')],
  [/\bot2\b/g, t('ggs.formula.ot2')],
  [/\bmt3\b/g, t('ggs.formula.mt3')],
  [/\bot3\b/g, t('ggs.formula.ot3')],
  [/\bmm1\b/g, t('ggs.formula.mm1')],
  [/\bom1\b/g, t('ggs.formula.om1')],
  [/\bml1\b/g, t('ggs.formula.ml1')],
  [/\bol1\b/g, t('ggs.formula.ol1')],
  [/\bmr\b/g, t('ggs.formula.mr')],
  [/\bor\b/g, t('ggs.formula.or')],
];

/// Render one leaf (`size!=8`, `!saved`); the tree stays intact, so
/// `&`/`|` are not handled here.
///
/// Structure first, words last: an English label is several words, and
/// substituting it before the `!x` rule left the negation attached to
/// the first word only.
function readAtom(src: string): string {
  let s = ` ${src} `
    .replace(/([^\s()!=<>]+)\s*==\s*F\b/g,                   // ml1==F
      (_m, x: string) => t('ggs.formula.is_false', { x }))
    .replace(/([^\s()!=<>]+)\s*==\s*T\b/g,
      (_m, x: string) => t('ggs.formula.is_true', { x }))
    .replace(/!=\s*\?/g, ' ' + t('ggs.formula.not_any'))     // mc!=?
    .replace(/!=/g, ' ≠ ')                              // other comparisons
    .replace(/!\s*([^\s()!]+)/g,                             // !saved
      (_m, x: string) => t('ggs.formula.is_false', { x }))
    .replace(/\s*(<=|>=|<|>)\s*/g, ' $1 ');
  for (const [re, word] of formulaWords()) s = s.replace(re, word);
  return s.replace(/\s+/g, ' ').trim();
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
export const formulaVars = (): FormulaVar[] => [
  { name: 'rated', label: t('ggs.formula.rated'), type: 'bool' },
  { name: 'rand', label: t('ggs.formula.rand'), type: 'bool' },
  { name: 'synchro', label: t('ggs.formula.synchro'), type: 'bool' },
  { name: 'saved', label: t('ggs.formula.saved'), type: 'bool' },
  { name: 'komi', label: t('ggs.formula.komi'), type: 'bool' },
  { name: 'anti', label: t('ggs.formula.anti'), type: 'bool' },
  { name: 'ml1', label: t('ggs.formula.ml1'), type: 'bool' },
  { name: 'ol1', label: t('ggs.formula.ol1'), type: 'bool' },
  { name: 'size', label: t('ggs.formula.size'), type: 'num', def: 8 },
  { name: 'discs', label: t('ggs.formula.discs'), type: 'num', def: 16 },
  { name: 'mr', label: t('ggs.formula.mr'), type: 'num', def: 2000 },
  { name: 'or', label: t('ggs.formula.or'), type: 'num', def: 2000 },
  { name: 'mt1', label: t('ggs.formula.var.mt1'), type: 'num', unit: t('ggs.unit.seconds'), def: 600 },
  { name: 'ot1', label: t('ggs.formula.var.ot1'), type: 'num', unit: t('ggs.unit.seconds'), def: 600 },
  { name: 'mt2', label: t('ggs.formula.var.mt2'), type: 'num', unit: t('ggs.unit.seconds'), def: 0 },
  { name: 'ot2', label: t('ggs.formula.var.ot2'), type: 'num', unit: t('ggs.unit.seconds'), def: 0 },
  { name: 'mt3', label: t('ggs.formula.var.mt3'), type: 'num', unit: t('ggs.unit.seconds'), def: 0 },
  { name: 'ot3', label: t('ggs.formula.var.ot3'), type: 'num', unit: t('ggs.unit.seconds'), def: 0 },
  { name: 'mm1', label: t('ggs.formula.mm1'), type: 'num', def: 0 },
  { name: 'om1', label: t('ggs.formula.om1'), type: 'num', def: 0 },
  { name: 'stored', label: t('ggs.formula.stored'), type: 'num', def: 0 },
  { name: 'playing', label: t('ggs.formula.playing'), type: 'num', def: 0 },
  { name: 'mc', label: t('ggs.formula.mc'), type: 'color' },
  { name: 'oc', label: t('ggs.formula.oc'), type: 'color' },
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
  formulaVars().find((v) => v.name === name);

/// Color options. GGS notation is `*` = black / `O` = white; `b`/`w`
/// are rejected. Distinct from the screen's stone colors — never mix.
export const colorChoices = (): [string, string][] => [
  ['?', t('ggs.color.any')], ['*', t('ggs.color.black')], ['O', t('ggs.color.white')],
];

/// Boolean options as [negated?, label], phrased to match the color
/// options.
export const boolOps = (): [boolean, string][] =>
  [[false, t('ggs.formula.bool_is')], [true, t('ggs.formula.bool_is_not')]];

/** Bundles (`&`/`|`) vs leaves, split by type. */
export const isGroup = (c: Cond): c is { kind: 'all' | 'any'; kids: Cond[] } =>
  c.kind !== 'atom';

/// Render one leaf (for the read-only tree); same vocabulary as
/// `readAtom`, different entry point.
export function condLabel(c: Cond): string {
  if (isGroup(c)) return c.kind === 'all' ? t('ggs.formula.all_of') : t('ggs.formula.any_of');
  const v = varOf(c.name);
  const label = v?.label ?? c.name;
  if (!v || v.type === 'bool') return c.neg ? t('ggs.formula.is_false', { x: label }) : label;
  if (v.type === 'color') {
    const name = colorChoices().find(([x]) => x === c.val)?.[1] ?? c.val;
    return c.op === '≠'
      ? t('ggs.formula.color_is_not', { x: label, c: name })
      : t('ggs.formula.color_is', { x: label, c: name });
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
    .map((x) => x.trim()).filter(Boolean);
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
