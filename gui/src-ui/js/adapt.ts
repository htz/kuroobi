import type { GameView, GgsSnapshot, LogLine as RawLog } from './types';
import type { MoveInfo } from './state';
import type { Cell, EvalInfo } from './components/board';
import type { GraphPoint, Move, MoveSrc, StoneColor } from './components/data';
import type { LogLine } from './components/ggs';

/* Engine state -> screen shapes.
 *
 * All conversions live here: per-screen copies of "which color is move
 * N" inevitably drift (the old UI had the record table and the graph
 * disagree). */

export const sqName = (sq: number): string => 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);

/** With passes counted, turns alternate every move: odd moves (even i) are Black. */
const colorOf = (i: number): StoneColor => (i % 2 === 0 ? 'b' : 'w');

/** The 64 cells; the engine sends number[] but values are only 0/1/2. */
export const cellsOf = (v: GameView): Cell[] => v.cells as Cell[];

/** Loss marker. Values are Black-view: Black loses by drops, White by rises. */
function lossOf(i: number, value: number | undefined, prev: number | undefined): number | undefined {
  if (value === undefined || prev === undefined) return undefined;
  const loss = i % 2 === 0 ? prev - value : value - prev;
  return loss >= 2 ? +loss.toFixed(1) : undefined;   // mark only losses of 2+ discs
}

/** Record-table row; info (engine play records) outranks values (analysis). */
export function movesOf(
  v: GameView,
  info: Record<number, MoveInfo>,
  values?: (GraphPoint | undefined)[] | null,
  /** Viewer sign: 1 for Black's view, -1 for White's. Losses are magnitudes. */
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
      // A loss is a magnitude; the same from either view, never negated.
      loss: lossOf(i, value, prev),
      secs: rec?.secs,
      // Source stays an English token; the table owns its wording.
      src: value === undefined ? undefined
        : book ? ((rec?.learned ? 'book_learned' : 'book') satisfies MoveSrc)
        : ((exact ? 'solve' : 'search') satisfies MoveSrc),
    };
  });
}

/** Eval cells on the board; hints attach only to currently legal moves. */
/** Candidate evals for the board.
 *
 * The engine returns mover-view values while the table and graph use
 * Black's view; convert here so "+8" means one thing everywhere.
 * `sign` applies the viewer on top (Black 1 / White -1). The best-move
 * mark is chosen BEFORE conversion — picking the max after negation
 * would frame the worst move on White's turns. */
export function evalsOf(
  hints: Record<number, { value: number; exact: boolean; book: boolean; depth: number }> | null,
  /** Whether Black is to move; false negates mover-view values to Black's view. */
  blackToMove = true,
  /** Viewer sign: 1 for Black's view, -1 for White's. */
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
      // Three sources; only "N plies" is provisional and still growing.
      src: h.book ? { book: true } : h.exact ? { exact: true } : { depth: h.depth },
      best: h.value === best,
    };
  }
  return out;
}

/* ---------------- GGS ---------------- */

/** Connection state; backend and screen spell it differently, so map at the boundary. */
export const connOf = (c: GgsSnapshot['conn'] | undefined): 'offline' | 'connecting' | 'logging-in' | 'online' =>
  c === 'online' ? 'online' : c === 'connecting' ? 'connecting'
    : c === 'logging_in' ? 'logging-in' : 'offline';

/** Left-nav count badges, counted as the existing Nav.tsx does. */
export function navBadges(snap: GgsSnapshot | null, chatUnread: number) {
  return {
    // Count only offers addressed to us (the list carries others' too).
    'ggs-lobby': { count: snap?.offers.filter((o) => o.incoming).length || undefined, alert: true },
    // Match-pair count (synchro's two boards = one pair, grouped by
    // base). A dot marks pairs waiting on us — a bare number cannot
    // distinguish "more games" from "you are being waited on".
    'ggs-play': {
      count: new Set(snap?.matches.map((m) => m.base || m.id) ?? []).size || undefined,
      dot: snap?.matches.some((m) => m.my_color && !m.over && m.turn === m.my_color)
        ? ('bad' as const) : undefined,
    },
    'ggs-chat': { count: chatUnread || undefined, alert: true },
    'ggs-standby': { dot: snap?.standby.enabled ? ('ok' as const) : undefined },
  };
}

/** Whether our GGS game is live; it outranks local starts and analysis. */
export const ggsPlaying = (snap: GgsSnapshot | null): boolean =>
  snap?.matches.some((m) => m.my_color && !m.over) ?? false;

/** Wire log; the backend says `info`, the screen says `app`. */
export const logLinesOf = (log: RawLog[]): LogLine[] =>
  log.map((l) => ({ dir: l.dir === 'info' ? 'app' : l.dir, text: l.text }));
