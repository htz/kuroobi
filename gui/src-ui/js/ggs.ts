
import { useCallback, useEffect, useState } from 'react';
import { ggsApi, jsLog, onGgsSnapshot } from './api';
import { t, dateLocale } from './i18n';
import type { GgsSnapshot, MatchView, PlayerView, UserRow } from './types';


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
        jsLog('failed to subscribe to GGS: ' + e);
      }
    })();
    return () => {
      alive = false;
      unlisten?.();
    };
  }, []);

  return { snap };
}


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
  active: boolean;
}

const theirTurn = (m: MatchView) =>
  !m.over && !!m.my_color && (m.turn === 'black' || m.turn === 'white') && m.turn !== m.my_color;

// null while the opponent's clock waits for the other board
function oppSince(m: MatchView, sib: MatchView | undefined): number | null {
  if (!sib) return m.updated_ms;
  // An opponent that searches one board at a time takes the board whose turn came first.
  if (theirTurn(sib)) {
    const first = m.updated_ms < sib.updated_ms || (m.updated_ms === sib.updated_ms && m.id < sib.id);
    return first ? m.updated_ms : null;
  }
  return Math.max(m.updated_ms, sib.updated_ms);
}

function clockView(c: ClockBase | undefined, side: ClockSide, now: number,
                   sib?: MatchView): ClockView {
  if (!c) return { text: '', active: false };
  const m = c.match;
  let base: number | null;
  let ext: number | null;
  let raw: string;
  let color: string;
  if (side === 'p0' || side === 'p1') {
    const p: PlayerView | undefined = m.players[side === 'p0' ? 0 : 1];
    if (!p) return { text: '', active: false };
    base = p.secs; ext = p.ext; raw = p.clock; color = p.color;
  } else if (side === 'my') {
    base = m.my_secs; ext = m.my_ext; raw = m.my_clock; color = m.my_color;
  } else {
    base = m.opp_secs; ext = m.opp_ext; raw = m.opp_clock;
    color = m.my_color === 'black' ? 'white' : m.my_color === 'white' ? 'black' : '';
  }
  if (m.ended === 'adjourned' || m.ended === 'aborted') return { text: '', active: false };
  const active = !m.over && !!m.turn && !!color && m.turn === color;
  if (base === null) return { text: raw || '', active };
  // A finished board keeps the time each side had left at its last move, without overtime marks.
  if (m.over) return { text: fmtSecs(base), active: false };
  // From the server update, not first draw, so a remount does not reset the clock.
  const since = m.updated_ms || c.at;
  const mine = side === 'my' || (!!color && color === m.my_color);
  const elapsed = active ? Math.max(0, now - since) / 1000 : 0;
  // The server charges others the time they report with the move, not the wall time.
  if (!mine && base > 0) {
    const from = side === 'opp' && m.updated_ms ? oppSince(m, sib) : since;
    const spent = active && from != null ? Math.max(0, now - from) / 1000 : 0;
    return { text: fmtSecs(Math.max(0, base - spent)), active };
  }
  const ours = !active || m.think_queued ? 0
    : m.think_since_ms ? Math.max(0, now - m.think_since_ms) / 1000 : elapsed;
  const rem = base - (mine ? ours : 0);
  if (mine && m.in_overtime && rem >= 0) {
    return { text: t('ggs.clock.overtime', { t: fmtSecs(rem) }), active: false };
  }
  if (rem >= 0) return { text: fmtSecs(rem), active };
  if (ext && ext + rem >= 0) {
    return { text: t('ggs.clock.overtime', { t: fmtSecs(ext + rem) }), active: false };
  }
  return { text: t('ggs.clock.timeout'), active: false };
}

type Presence = 'playing' | 'online' | 'offline';

// `/os who` lists only free players taking requests; match players and the server's own `who` fill in the rest.
export function onlineRows(snap: GgsSnapshot): UserRow[] {
  const rows = [...snap.users];
  const have = new Set(rows.map((u) => u.name));
  for (const o of snap.ongoing) {
    const random = o.gtype.startsWith('s8r') || o.gtype.startsWith('8r');
    o.names.forEach((name, i) => {
      if (have.has(name)) return;
      have.add(name);
      const r = Number(o.ratings[i]);
      const rating = Number.isFinite(r) ? r : null;
      rows.push({
        name, rating: random ? null : rating, dev: null,
        rating_r: random ? rating : null, dev_r: null, open: null, raw: '',
      });
    });
  }
  for (const name of snap.online ?? []) {
    if (have.has(name)) continue;
    have.add(name);
    const mine = name === snap.login ? snap.my_ranks : [];
    const n8 = mine.find((r) => r.gtype === '8');
    const n8r = mine.find((r) => r.gtype === '8r');
    rows.push({
      name, rating: n8?.rating ?? null, dev: n8?.dev ?? null,
      rating_r: n8r?.rating ?? null, dev_r: n8r?.dev ?? null, open: '-', raw: '',
    });
  }
  return rows.sort((a, b) => (b.rating ?? b.rating_r ?? 0) - (a.rating ?? a.rating_r ?? 0));
}

export function presence(snap: GgsSnapshot, name: string): Presence {
  if (snap.ongoing.some((o) => o.names.includes(name))) return 'playing';
  const on = snap.users.some((u) => u.name === name) || (snap.online ?? []).includes(name);
  return on ? 'online' : 'offline';
}

export function useClocks(matches: MatchView[]): (id: string, side: ClockSide) => ClockView {
  const [state, setState] = useState<ClockState>({ bases: {}, now: 0 });

  useEffect(() => {
    const sync = () => setState((prev) => {
      const bases: Record<string, ClockBase> = {};
      for (const m of matches) {
        const old = prev.bases[m.id];
        bases[m.id] = old && old.match.seen === m.seen ? old : { match: m, at: Date.now() };
      }
      return { bases, now: Date.now() };
    });
    const t0 = window.setTimeout(sync, 0);
    const iv = window.setInterval(sync, 500);
    return () => { clearTimeout(t0); clearInterval(iv); };
  }, [matches]);

  return useCallback(
    (id: string, side: ClockSide) => {
      const c = state.bases[id];
      const sib = c && Object.values(state.bases)
        .find((b) => b.match.base === c.match.base && b.match.id !== id)?.match;
      return clockView(c, side, state.now, sib);
    },
    [state],
  );
}


export function fmtSecs(s: number): string {
  s = Math.max(0, Math.floor(s));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

const gtypeNames = (): Record<string, string> => ({
  s8r16: t('ggs.gtype.synchro_rand16'),
  s8r18: t('ggs.gtype.synchro_rand18'),
  s8r20: t('ggs.gtype.synchro_rand20'),
  s8r14: t('ggs.gtype.synchro_rand14'),
  s8: t('ggs.gtype.synchro_std'),
  '8': t('ggs.gtype.std'),
  '8r16': t('ggs.gtype.rand16'),
  '8r': t('ggs.gtype.rand_opening'),
});
export function gtypeLabel(raw: string): string {
  // The stored-games list adds a mark after a colon ("s8r14:l"); the type is what comes before it.
  const code = raw.split(':')[0];
  const hit = gtypeNames()[code];
  if (hit) return hit;
  // Other random lengths, as GGS and GGF write them ("8r12", "s8r22").
  const m = /^(s)?8r(\d+)$/.exec(code);
  if (m) return t(m[1] ? 'ggs.gtype.synchro_rand' : 'ggs.gtype.rand', { n: Number(m[2]) });
  return code || '?';
}

export const gtypeChoices = (): [string, string][] => [
  ['s8r14', t('ggs.gtype.synchro_rand14')],
  ['s8r16', t('ggs.gtype.synchro_rand16')],
  ['s8r18', t('ggs.gtype.synchro_rand18')],
  ['s8r20', t('ggs.gtype.synchro_rand20')],
  ['s8', t('ggs.gtype_choice.synchro_std')],
  ['8', t('ggs.gtype_choice.std')],
  ['8r16', t('ggs.gtype_choice.rand16')],
];
/** The shortest main time for a local game; GGS itself takes any, down to 0. */
export const MIN_CLOCK = 30;

/** GGS's extension when a request names none. */
export const DEFAULT_EXT = 120;

/** "4 分 30 秒", "5 分", "30 秒". */
export function secsText(secs: number): string {
  const m = Math.floor(secs / 60), s = secs % 60;
  if (!m) return t('ggs.clock_choice.seconds', { n: s });
  return s ? t('ggs.clock_choice.min_sec', { m, s }) : t('ggs.clock_choice.minutes', { n: m });
}

const hms = (secs: number): string =>
  [Math.floor(secs / 3600), Math.floor(secs / 60) % 60, secs % 60].map((n) => String(n).padStart(2, '0')).join(':');

const toSecs = (v: string): number | null => {
  const parts = v.split(',')[0].trim().split(':');
  if (!parts.every((p) => /^\d+$/.test(p))) return null;
  // A bare number is minutes, as GGS reads it.
  return parts.length === 1 ? Number(parts[0]) * 60 : parts.reduce((a, p) => a * 60 + Number(p), 0);
};

/** The request's clock as GGS takes it: the main time, and the extension only when it is not GGS's own. */
export function clockSpec(main: number, ext: number): string {
  return ext === DEFAULT_EXT ? hms(main) : `${hms(main)}//${hms(ext)}`;
}

/** The main time and extension of a clock ("05:00//02:00", "14:59,0:0//02:00,0:0"); null with an increment. */
export function parseClockSpec(spec: string): { main: number; ext: number } | null {
  const [base, inc = '', ext] = spec.split('/');
  const step = inc.trim() ? toSecs(inc) : 0;
  const main = toSecs(base);
  const e = ext == null || ext.trim() === '' ? DEFAULT_EXT : toSecs(ext);
  return main == null || e == null || step !== 0 ? null : { main, ext: e };
}

export const clockChoices = (): [number, string][] =>
  [1, 3, 5, 10, 15, 20, 30].map((n) => [n * 60, t('ggs.clock_choice.minutes', { n })]);

/** "3 分 · 延長 30 秒" */
export const pairLabel = (main: number, ext: number): string => ext
  ? t('ggs.clock_choice.pair', { main: secsText(main), ext: secsText(ext) })
  : t('ggs.clock_choice.pair_none', { main: secsText(main) });

// The last tournaments' two clocks, then the usual ones with GGS's own extension.
export const askClockChoices = (): [string, string][] =>
  ([[60, 30], [180, 30], [300, DEFAULT_EXT], [600, DEFAULT_EXT], [900, DEFAULT_EXT]] as const)
    .map(([m, e]) => [clockSpec(m, e), pairLabel(m, e)]);



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


export async function translate(text: string, target: string): Promise<string> {
  const url = 'https://translate.googleapis.com/translate_a/single?client=gtx&sl=auto&tl=' +
    target + '&dt=t&q=' + encodeURIComponent(text);
  const r = await fetch(url);
  if (!r.ok) throw new Error('translate ' + r.status);
  const j = (await r.json()) as [[string, string][]];
  return (j[0] ?? []).map((x) => x[0]).join('');
}

export const hasJapanese = (s: string): boolean =>
  /[\u3040-\u30ff\u4e00-\u9fff]/.test(s);


const normKey = (k: string): string => k.replace(/\s+/g, '');

export const fingerKey = (k: string): string => normKey(k).replace(/\(.*\)/, '');

const fingerLabels = (): Record<string, string> => ({
  open: t('ggs.finger.open'),
  accept: t('ggs.finger.accept'),
  decline: t('ggs.finger.decline'),
  'request(+)': t('ggs.finger.request'), 'request(-)': t('ggs.finger.request'),
  rated: t('ggs.rated'), play: t('ggs.finger.play'),
  'stored(+)': t('ggs.finger.stored'), 'stored(-)': t('ggs.finger.stored'),
  name: t('ggs.finger.name'), info: t('ggs.finger.info'), email: t('ggs.finger.email'),
  since: t('ggs.finger.since'), idle: t('ggs.finger.idle'), host: t('ggs.finger.host'),
  dblen: t('ggs.finger.dblen'),
  level: t('ggs.finger.level'), trust: t('ggs.finger.trust'), client: t('ggs.finger.client'),
  bell: t('ggs.finger.bell'), hear: t('ggs.finger.hear'),
  vt100: t('ggs.finger.vt100'),
  'watch(+)': t('ggs.finger.watch'), 'watch(-)': t('ggs.finger.watch'),
  'track(+)': t('ggs.finger.track'), 'track(-)': t('ggs.finger.track'),
  'groups(+)': t('ggs.finger.groups'), 'groups(-)': t('ggs.finger.groups'),
  'channs(+)': t('ggs.finger.channs'), 'channs(-)': t('ggs.finger.channs'),
  'notify(+)': t('ggs.finger.notify'), 'notify(-)': t('ggs.finger.notify'),
  'ignore(+)': t('ggs.finger.ignore'), 'ignore(-)': t('ggs.finger.ignore'),
});

const FINGER_HIDDEN = ['passw', 'password', 'login', '/os', 'sock'];

const FINGER_ALWAYS = ['open', 'accept', 'decline', 'request(+)', 'request(-)'];

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

interface FingerRow { key: string; label: string; value: string }

export function fingerGroups(
  fields: [string, string][],
): { id: string; title: string; rows: FingerRow[] }[] {
  const labels = fingerLabels();
  const got = new Map(fields.map(([k, v]) => [normKey(k), v]));
  const used = new Set<string>();
  const row = (k: string): FingerRow | null => {
    if (FINGER_HIDDEN.includes(fingerKey(k))) return null;
    const v = got.get(k);
    if (v === undefined && !FINGER_ALWAYS.includes(k)) return null;
    used.add(k);
    return { key: k, label: labels[k] ?? k, value: v ?? '' };
  };
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
  const rest = [...got.entries()]
    .filter(([k]) => !used.has(k) && !FINGER_HIDDEN.includes(fingerKey(k)))
    .map(([k, v]) => ({ key: k, label: labels[k] ?? k, value: v }));
  out[out.length - 1].rows.push(...dedupe(rest));
  return out.filter((g) => g.rows.length > 0);
}

export function fingerValue(k: string, v: string): string {
  const key = fingerKey(k);
  if (key === 'open') {
    return v === '0' || v === '-'
      ? t('ggs.finger.value.not_accepting') : t('ggs.finger.value.accepting');
  }
  if (key === 'rated' || key === 'trust') {
    return v === '+' ? t('ggs.finger.value.yes') : t('ggs.finger.value.no');
  }
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
    const m = /([\d.]+)\s*=\s*([\d,]+)\s*\/\s*([\d,]+)/.exec(v);
    return m ? t('ggs.finger.value.dblen', { pct: m[1], n: m[2], total: m[3] }) : v;
  }
  if (key === 'groups') return v === '_client' ? t('ggs.finger.value.group_client') : v;
  if (key === 'bell') return readBell(v);
  if (key === 'since' || key === 'idle') return readTime(k, v);
  if (['track', 'watch', 'channs', 'ignore', 'stored'].includes(key)) {
    return v.trim() || t('ggs.finger.value.none');
  }
  return v;
}

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

function readTime(k: string, v: string): string {
  if (k === 'since') {
    const at = Date.parse(v.replace(/\s*[A-Z]{3}$/, ' GMT-0600'));
    if (!Number.isNaN(at)) {
      return new Date(at).toLocaleString(dateLocale(), {
        year: 'numeric', month: 'long', day: 'numeric',
        hour: '2-digit', minute: '2-digit', weekday: 'short',
      });
    }
    return v;
  }
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


export interface Who {
  me: string;
  them: string;
}

const defaultWho = (): Who => ({
  me: t('ggs.formula.who.me'),
  them: t('ggs.formula.who.them'),
});

const SIDED: Record<string, string> = {
  mc: 'c', oc: 'c',
  ml1: 'l1', ol1: 'l1',
  mm1: 'm1', om1: 'm1',
  mm2: 'm2', om2: 'm2',
  ma2: 'a2', oa2: 'a2',
  mm3: 'm3', om3: 'm3',
  ma3: 'a3', oa3: 'a3',
  mr: 'r', or: 'r',
  md: 'd', od: 'd',
  mt1: 't1', ot1: 't1',
  mt2: 't2', ot2: 't2',
  mt3: 't3', ot3: 't3',
};

function varLabel(name: string, who: Who = defaultWho()): string {
  const side = SIDED[name];
  if (side) {
    return t(`ggs.formula.side.${side}`, { who: name.startsWith('m') ? who.me : who.them });
  }
  if (name === 'playing') return t('ggs.formula.side.playing', { who: who.me });
  return t(`ggs.formula.${name}`);
}

interface FormulaVar {
  name: string;
  label: string;
  type: 'bool' | 'num' | 'color';
  unit?: string;
  def?: number;
  /** Called with an argument: a number (md, od, rnd) or the kind of games to count (playing). */
  arg?: 'num' | 'pool';
  argDef?: string;
  /** GGS types mr, or, md() and od() as REAL: their numbers need a "." (or>=1600.0), the rest must not have one. */
  real?: boolean;
}

export const formulaVars = (who: Who = defaultWho()): FormulaVar[] => {
  const sec = t('ggs.unit.seconds');
  const v = (name: string, type: FormulaVar['type'], rest: Partial<FormulaVar> = {}): FormulaVar =>
    ({ name, label: varLabel(name, who), type, ...rest });
  return [
    v('rated', 'bool'), v('rand', 'bool'), v('synchro', 'bool'), v('saved', 'bool'),
    v('komi', 'bool'), v('anti', 'bool'), v('ml1', 'bool'), v('ol1', 'bool'),
    v('ma2', 'bool'), v('oa2', 'bool'), v('ma3', 'bool'), v('oa3', 'bool'),
    v('size', 'num', { def: 8 }), v('discs', 'num', { def: 16 }),
    v('mr', 'num', { def: 2000, real: true }), v('or', 'num', { def: 2000, real: true }),
    v('md', 'num', { def: 0, arg: 'num', argDef: '+1.0', real: true }),
    v('od', 'num', { def: 0, arg: 'num', argDef: '+1.0', real: true }),
    v('mt1', 'num', { unit: sec, def: 600 }), v('ot1', 'num', { unit: sec, def: 600 }),
    v('mt2', 'num', { unit: sec, def: 0 }), v('ot2', 'num', { unit: sec, def: 0 }),
    v('mt3', 'num', { unit: sec, def: 0 }), v('ot3', 'num', { unit: sec, def: 0 }),
    v('mm1', 'num', { def: 0 }), v('om1', 'num', { def: 0 }),
    v('mm2', 'num', { def: 1 }), v('om2', 'num', { def: 1 }),
    v('mm3', 'num', { def: 0 }), v('om3', 'num', { def: 0 }),
    v('stored', 'num', { def: 0 }), v('playing', 'num', { def: 0, arg: 'pool', argDef: 'both' }),
    v('rnd', 'num', { def: 50, arg: 'num', argDef: '100' }),
    v('mc', 'color'), v('oc', 'color'),
  ];
};

// The tournament director's guidance (discs<14, mt1<60), plus what KUROOBI cannot play or plays at a
// disadvantage: other boards, komi, anti, move-count clocks, increments, and a main time with no loss.
export const DEFAULT_DECLINE = 'size!=8|anti|komi|mm1!=0|mt2!=0|mm3!=0|!ml1|discs<14|mt1<60';

export const FORMULA_OPS = ['=', '≠', '<', '>', '≤', '≥'] as const;
export type FormulaOp = (typeof FORMULA_OPS)[number];

// GGS knows only "==": a single "=" is a type error on the server.
const OP_SRC: Record<FormulaOp, string> = {
  '=': '==', '≠': '!=', '<': '<', '>': '>', '≤': '<=', '≥': '>=',
};
const SRC_OP: Record<string, FormulaOp> = {
  '=': '=', '==': '=', '!=': '≠', '<': '<', '>': '>', '<=': '≤', '>=': '≥',
};

export type Atom = { kind: 'atom'; name: string; op: FormulaOp; val: string; neg: boolean; arg?: string };
export type Cond =
  | { kind: 'all' | 'any'; kids: Cond[]; neg?: boolean }
  | Atom
  /** Anything the rows cannot hold (arithmetic such as `or+100`), kept and edited as text. */
  | { kind: 'expr'; src: string };

export const varOf = (name: string, who: Who = defaultWho()): FormulaVar | undefined =>
  formulaVars(who).find((v) => v.name === name);

export const colorChoices = (): [string, string][] => [
  ['?', t('ggs.color.any')], ['B', t('ggs.color.black')], ['W', t('ggs.color.white')],
];

export const boolOps = (): [boolean, string][] =>
  [[false, t('ggs.formula.bool_is')], [true, t('ggs.formula.bool_is_not')]];

/** What `playing(…)` counts: T random games, F the others, or both added up. */
export const poolChoices = (): [string, string][] => [
  ['both', t('ggs.formula.pool.both')], ['T', t('ggs.formula.pool.rand')], ['F', t('ggs.formula.pool.std')],
];

/** A group's kind with its `!( … )`: "all", "any", "!all", "!any". */
export const groupChoices = (): [string, string][] => [
  ['all', t('ggs.formula.all_of')], ['any', t('ggs.formula.any_of')],
  ['!all', t('ggs.formula.not_all_of')], ['!any', t('ggs.formula.none_of')],
];

export const isGroup = (c: Cond): c is { kind: 'all' | 'any'; kids: Cond[]; neg?: boolean } =>
  c.kind === 'all' || c.kind === 'any';

export function groupLabel(c: { kind: 'all' | 'any'; neg?: boolean }): string {
  const key = (c.neg ? '!' : '') + c.kind;
  return groupChoices().find(([k]) => k === key)?.[1] ?? '';
}

/** A number as GGS reads it for a REAL or an INT slot. */
export const numOk = (val: string, real = false): boolean =>
  real ? /^[+-]?\d+(\.\d+)?$/.test(val) : /^[+-]?\d+$/.test(val);
const numSrc = (val: string, real = false): string => (real && /^[+-]?\d+$/.test(val) ? `${val}.0` : val);

const callSrc = (name: string, arg = ''): string =>
  name === 'playing' ? (arg === 'both' ? 'playing(T)+playing(F)' : `playing(${arg})`)
    : `${name}(${numSrc(arg, name !== 'rnd')})`;

export function condLabel(c: Cond, who: Who = defaultWho()): string {
  if (isGroup(c)) return groupLabel(c);
  if (c.kind === 'expr') return t('ggs.formula.expr', { x: c.src });
  const v = varOf(c.name, who);
  if (!v) return t('ggs.formula.unknown', { x: condToSrc(c) });
  let label = v.label;
  if (c.name === 'rnd') label = t('ggs.formula.rnd_below', { n: c.arg ?? '' });
  else if (v.arg === 'num') label = t('ggs.formula.after_result', { x: label, d: c.arg ?? '' });
  else if (v.arg === 'pool') label = `${label} (${poolChoices().find(([k]) => k === c.arg)?.[1] ?? c.arg})`;
  if (v.type === 'bool') {
    return c.neg ? t('ggs.formula.is_false', { x: label }) : t('ggs.formula.is_true', { x: label });
  }
  if (v.type === 'color') {
    const name = colorChoices().find(([x]) => x === c.val)?.[1] ?? c.val;
    return c.op === '≠'
      ? t('ggs.formula.color_is_not', { x: label, c: name })
      : t('ggs.formula.color_is', { x: label, c: name });
  }
  const other = varOf(c.val, who);
  const rhs = other ? other.label : `${c.val}${v.unit ?? ''}`;
  return `${label} ${c.op} ${rhs}`;
}

const NUM = /^[+-]?\d+(\.\d+)?$/;

/** The left side of a comparison as a row: a plain number variable or a call with a literal argument. */
function parseLhs(s: string): { name: string; arg?: string } | null {
  let m = /^([A-Za-z]\w*)$/.exec(s);
  if (m) return varOf(m[1]) && !varOf(m[1])!.arg ? { name: m[1] } : null;
  m = /^(md|od|rnd)\(\s*([+-]?\d+(?:\.\d+)?)\s*\)$/.exec(s);
  if (m) return { name: m[1], arg: m[2] };
  m = /^playing\(\s*([TF])\s*\)$/.exec(s);
  if (m) return { name: 'playing', arg: m[1] };
  if (/^playing\(\s*([TF])\s*\)\s*\+\s*playing\(\s*(?!\1)[TF]\s*\)$/.test(s)) return { name: 'playing', arg: 'both' };
  return null;
}

function parseAtomSrc(src: string): Cond {
  const s = src.trim();
  const expr: Cond = { kind: 'expr', src: s };
  const b = /^(!?)\s*([A-Za-z]\w*)\s*(?:==\s*([TF]))?$/.exec(s);
  if (b) {
    if (varOf(b[2])?.type !== 'bool') return expr;
    return { kind: 'atom', name: b[2], op: '=', val: '', neg: (b[1] === '!') !== (b[3] === 'F') };
  }
  const m = /^(.+?)\s*(==|!=|<=|>=|<|>|=)\s*(.+)$/.exec(s);
  const lhs = m && parseLhs(m[1].trim());
  if (!m || !lhs) return expr;
  const v = varOf(lhs.name)!;
  const op = SRC_OP[m[2]];
  const val = m[3].trim();
  if (v.type === 'color') {
    return /^[?WB]$/.test(val) && (op === '=' || op === '≠') ? { kind: 'atom', name: lhs.name, op, val, neg: false } : expr;
  }
  const other = varOf(val);
  const same = other && other.type === 'num' && !other.arg && !other.real === !v.real;
  if (!(NUM.test(val) && numOk(val, v.real)) && !same) return expr;
  return { kind: 'atom', name: lhs.name, op, val, neg: false, ...(lhs.arg !== undefined ? { arg: lhs.arg } : {}) };
}

/** Every number in the tree is one GGS takes for its slot; otherwise the server refuses the formula. */
export function condValid(c: Cond): boolean {
  if (isGroup(c)) return c.kids.every(condValid);
  if (c.kind === 'expr') return true;
  const v = varOf(c.name);
  if (!v || v.type !== 'num') return true;
  if (v.arg === 'num' && !numOk(c.arg ?? '', c.name !== 'rnd')) return false;
  return !!varOf(c.val) || numOk(c.val, v.real);
}

export function parseCond(src: string): Cond | null {
  const body = src.replace(/^\s*:\s*/, '').trim();
  if (!body) return null;
  return parseFormula(body);
}

export function condToSrc(c: Cond): string {
  if (c.kind === 'expr') return c.src.trim();
  if (c.kind === 'atom') {
    const v = varOf(c.name);
    if (!v) return c.name;
    if (v.type === 'bool') return (c.neg ? '!' : '') + c.name;
    const rhs = varOf(c.val) ? c.val : numSrc(c.val, v.real);
    return `${v.arg ? callSrc(c.name, c.arg) : c.name}${OP_SRC[c.op]}${rhs}`;
  }
  const parts = c.kids.map((k) => [k, condToSrc(k)] as const).filter(([, p]) => p);
  if (parts.length === 1 && !c.neg) return parts[0][1];
  const sep = c.kind === 'all' ? '&' : '|';
  // Groups always keep their brackets: GGS gives & and | the same level.
  const inner = parts.map(([k, p]) => (isGroup(k) && !k.neg && /[&|]/.test(p) ? `(${p})` : p)).join(sep);
  if (!inner) return '';
  return c.neg ? `!(${inner})` : inner;
}

function parseFormula(src: string): Cond {
  // A "(" opening an atom groups (so does one after a lone "!"); after a name it is a call: md(+1.0).
  const toks: string[] = [];
  let cur = '';
  let depth = 0;
  const flush = () => { if (cur.trim()) toks.push(cur.trim()); cur = ''; };
  for (const ch of src) {
    if (depth === 0 && (ch === '&' || ch === '|')) { flush(); toks.push(ch); }
    else if (ch === '(' && depth === 0 && (!cur.trim() || cur.trim() === '!')) { flush(); toks.push(ch); }
    else if (ch === ')' && depth === 0) { flush(); toks.push(ch); }
    else {
      if (ch === '(') depth++;
      if (ch === ')') depth--;
      cur += ch;
    }
  }
  flush();
  let i = 0;
  const atom = (): Cond => {
    const neg = toks[i] === '!' && toks[i + 1] === '(';
    if (neg) i++;
    if (toks[i] === '(') {
      i++;
      const inner = expr();
      if (toks[i] === ')') i++;
      if (!neg) return inner;
      return isGroup(inner) ? { ...inner, neg: !inner.neg } : { kind: 'all', kids: [inner], neg: true };
    }
    return parseAtomSrc(toks[i++] ?? '');
  };
  // GGS gives & and | one level and binds to the right: `a & b | c` is `a & (b | c)`.
  const expr = (): Cond => {
    const left = atom();
    const op = toks[i];
    if (op !== '&' && op !== '|') return left;
    i++;
    const right = expr();
    const kind = op === '&' ? 'all' : 'any';
    return { kind, kids: [left, ...(isGroup(right) && right.kind === kind && !right.neg ? right.kids : [right])] };
  };
  return expr();
}
