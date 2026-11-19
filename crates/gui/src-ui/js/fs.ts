import { useEffect, useState } from 'react';
import { fsApi, jsLog, onFsSnapshot } from './api';
import type { Cell } from './components/board';
import type { FsBotRow, FsResult, FsSnapshot } from './types';
import { sumBoards, type TrendPoint } from './ggsui';

export function useFs() {
  const [snap, setSnap] = useState<FsSnapshot | null>(null);
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await onFsSnapshot((s) => { if (alive) setSnap(s); });
        const s = await fsApi.snapshot();
        if (alive) setSnap((prev) => prev ?? s);
      } catch (e) {
        jsLog('failed to subscribe to FlipSight: ' + e);
      }
    })();
    return () => { alive = false; unlisten?.(); };
  }, []);
  return snap;
}

/** A choice of how a FlipSight screen shows its list, kept across launches. */
export function useKept<T>(name: string, initial: T): [T, (v: T) => void] {
  const key = 'kuroobi.fs.' + name;
  const [v, setV] = useState<T>(() => {
    try { const raw = localStorage.getItem(key); return raw == null ? initial : JSON.parse(raw) as T; } catch { return initial; }
  });
  return [v, (next) => { setV(next); try { localStorage.setItem(key, JSON.stringify(next)); } catch { /* storage off */ } }];
}

/** The board component's order (file * 8 + rank) from the site's (a1 = 0, b1 = 1, ...). */
export const boardIndex = (i: number): number => (i % 8) * 8 + Math.floor(i / 8);

export function boardCells(cells: number[]): Cell[] {
  const out: Cell[] = new Array(64).fill(0);
  cells.forEach((c, i) => { out[boardIndex(i)] = (c === 1 || c === 2 ? c : 0) as Cell; });
  return out;
}

/** Matches at once a bot takes: its own cap here, else the site's setting (2 when not known). */
export const matchesOf = (b: FsBotRow): number => Math.max(1, b.cap ?? b.max_games ?? 2);

/** Threads a bot can take at once: its threads are per game (as FlipSight counts them), and a
 * match plays its two games at once. */
export const threadsOf = (b: FsBotRow): number => b.threads * 2 * matchesOf(b);

export const liveConn = (c: FsBotRow['conn']) => c === 'online' || c === 'connecting' || c === 'draining' || c === 'retrying';

export const clockText = (ms: number): string => {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

export const myRating = (r: FsResult): number | null => r.my_rating ?? null;

export const legKifu = (l: FsResult['legs'][number]): string => l.moves ?? '';

export const legDiff = (l: FsResult['legs'][number]): number | null => {
  const d = l.result_discs ?? l.discs;
  if (!d || d.length < 2) return null;
  return l.black ? d[0] - d[1] : d[1] - d[0];
};

type Leg = { evals?: [number, number, number?][]; opp_secs?: [number, number][]; moves?: string | null };

/** One game's line as GGS draws its own games: our values and both sides' thinking times by move number. */
function legTrend(l: Leg): TrendPoint[] {
  const at = new Map<number, TrendPoint>();
  const point = (x: number) => at.get(x) ?? at.set(x, { x, mine: null, opp: null, mySecs: null, oppSecs: null }).get(x)!;
  for (const [x, v, secs] of l.evals ?? []) Object.assign(point(x), { mine: v, mySecs: secs ?? null });
  for (const [x, secs] of l.opp_secs ?? []) point(x).oppSecs = secs;
  return [...at.values()].sort((a, b) => a.x - b.x);
}

/** A match's two games summed as a GGS synchro pair (same position, colours swapped). */
export const trendOf = (legs: Leg[]): TrendPoint[] => sumBoards(legs.map(legTrend));

const movesOf = (l: Leg): string[] => (l.moves ?? '').match(/[a-h][1-8]/g) ?? [];

/** Where the two games split: the first move that differs, counted as on GGS. */
export function splitOf(legs: Leg[]): number | undefined {
  if (legs.length !== 2) return undefined;
  const [a, b] = legs.map(movesOf);
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return i + 1;
  return undefined;
}

export const pliesOf = (legs: Leg[]): number => Math.max(60, ...legs.map((l) => movesOf(l).length));
