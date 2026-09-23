import type { GameView, GgsSnapshot, LogLine as RawLog } from './types';
import type { MoveInfo } from './state';
import type { Cell, EvalInfo } from './components/board';
import type { GraphPoint, Move, MoveSrc, StoneColor } from './components/data';
import type { LogLine } from './components/ggs';


export const sqName = (sq: number): string => 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);

const colorOf = (i: number): StoneColor => (i % 2 === 0 ? 'b' : 'w');

export const cellsOf = (v: GameView): Cell[] => v.cells as Cell[];

function lossOf(i: number, value: number | undefined, prev: number | undefined): number | undefined {
  if (value === undefined || prev === undefined) return undefined;
  const loss = i % 2 === 0 ? prev - value : value - prev;
  return loss >= 2 ? +loss.toFixed(1) : undefined;   // mark only losses of 2+ discs
}

export function movesOf(
  v: GameView,
  info: Record<number, MoveInfo>,
  values?: (GraphPoint | undefined)[] | null,
  sign: 1 | -1 = 1,
): Move[] {
  const shown = v.moves.map((_, i) => (info[i + 1] ? info[i + 1].value : values?.[i + 1]?.value));
  return v.moves.map((m, i) => {
    const n = i + 1;
    const rec = info[n];
    const gp = values?.[n];
    const value = shown[i];
    const exact = rec ? rec.exact : gp?.exact ?? false;
    const book = rec ? rec.source === 'book' : gp?.book ?? false;
    const prev = i === 0 ? values?.[0]?.value ?? 0 : shown[i - 1];
    return {
      n,
      move: m == null ? '' : sqName(m),
      pass: m == null,
      color: colorOf(i),
      score: value === undefined ? undefined : value * sign,
      loss: lossOf(i, value, prev),
      secs: rec?.secs,
      src: value === undefined ? undefined
        : book ? ((rec?.learned ? 'book_learned' : 'book') satisfies MoveSrc)
        : ((exact ? 'solve' : 'search') satisfies MoveSrc),
    };
  });
}

export function evalsOf(
  hints: Record<number, { value: number; exact: boolean; book: boolean; depth: number }> | null,
  blackToMove = true,
  sign: 1 | -1 = 1,
): Record<number, EvalInfo> | undefined {
  if (!hints) return undefined;
  const out: Record<number, EvalInfo> = {};
  let best = -Infinity;
  for (const h of Object.values(hints)) best = Math.max(best, h.value);
  const flip = (blackToMove ? 1 : -1) * sign;
  for (const [sq, h] of Object.entries(hints)) {
    out[+sq] = {
      score: h.value * flip,
      src: h.book ? { book: true }
        : h.exact ? { exact: true }
        : h.depth === 0 ? { select: true }
        : { depth: h.depth },
      best: h.value === best,
    };
  }
  return out;
}


export const connOf = (c: GgsSnapshot['conn'] | undefined): 'offline' | 'connecting' | 'logging-in' | 'online' =>
  c === 'online' ? 'online' : c === 'connecting' ? 'connecting'
    : c === 'logging_in' ? 'logging-in' : 'offline';

export function navBadges(snap: GgsSnapshot | null, chatUnread: number) {
  return {
    'ggs-lobby': { count: snap?.offers.filter((o) => o.incoming).length || undefined, alert: true },
    'ggs-play': {
      count: new Set(snap?.matches.map((m) => m.base || m.id) ?? []).size || undefined,
      dot: snap?.matches.some((m) => m.my_color && !m.over && m.turn === m.my_color)
        ? ('bad' as const) : undefined,
    },
    'ggs-chat': { count: chatUnread || undefined, alert: true },
    'ggs-standby': { dot: snap?.standby.enabled ? ('ok' as const) : undefined },
  };
}

export const ggsPlaying = (snap: GgsSnapshot | null): boolean =>
  snap?.matches.some((m) => m.my_color && !m.over) ?? false;

export const logLinesOf = (log: RawLog[]): LogLine[] =>
  log.map((l) => ({ dir: l.dir === 'info' ? 'app' : l.dir, text: l.text }));
