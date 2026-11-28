// The record formats the app's load dialog reads (crates/gui/src/main.rs `read_record`), in the same order.
import { legal, play, START, type Cell, type Position } from './othello';

export type RecordFormat = 'moves' | 'board' | 'board_moves' | 'ggf' | 'quest';
export type GameRecord = { start: Position; moves: number[]; format: RecordFormat };

/** `-` `.` empty, `X` `x` `B` `b` `0` `*` black, `O` `o` `W` `w` `1` white. */
function boardCell(c: string): Cell | undefined {
  if (c === '-' || c === '.') return 0;
  if ('XxBb0*'.includes(c)) return 1;
  if ('OoWw1'.includes(c)) return 2;
  return undefined;
}

/** 64 squares a1, b1 … h8 then the side to move; the board's cells are `file*8 + rank`. */
function boardOf(squares: Cell[]): Position {
  const cells = START.map(() => 0 as Cell);
  for (let i = 0; i < 64; i++) cells[(i & 7) * 8 + (i >> 3)] = squares[i];
  const turn = squares[64] === 2 ? 2 : 1;
  const other = turn === 1 ? 2 : 1;
  // A side with no move on a board as written has passed already.
  return { cells, turn: !legal(cells, turn).length && legal(cells, other).length ? other : turn };
}

/** The board as the app writes it (`x`, `o`, `-` and the side to move), for a URL. */
export function boardText(p: Position): string {
  let s = '';
  for (let i = 0; i < 64; i++) s += '-xo'[p.cells[(i & 7) * 8 + (i >> 3)]];
  return s + (p.turn === 1 ? 'x' : 'o');
}

export const isStart = (p: Position): boolean => p.turn === 1 && p.cells.every((c, i) => c === START[i]);

/** A board written from the start of some line, spaces and line breaks allowed in it, and the text left around it. */
function findBoard(text: string): { board: Position; rest: string } | null {
  const starts = [0];
  for (let i = text.indexOf('\n'); i >= 0; i = text.indexOf('\n', i + 1)) starts.push(i + 1);
  for (const start of starts) {
    const squares: Cell[] = [];
    for (let i = start; i < text.length; i++) {
      const c = text[i];
      if (/\s/.test(c)) continue;
      const b = boardCell(c);
      if (b === undefined || (squares.length === 64 && b === 0)) break;
      squares.push(b);
      if (squares.length === 65) return { board: boardOf(squares), rest: text.slice(0, start) + '\n' + text.slice(i + 1) };
    }
  }
  return null;
}

/** Every a1–h8 in the text, whatever is between them (move numbers, spaces, capitals). */
function squaresIn(text: string): string[] {
  return text.toLowerCase().match(/[a-h][1-8]/g) ?? [];
}

function ggf(text: string): { start: Position | null; moves: string[] } | null {
  const open = text.indexOf('(;');
  if (open < 0) return null;
  const rest = text.slice(open + 2);
  const close = rest.indexOf(';)');
  const body = close < 0 ? rest : rest.slice(0, close);
  let start: Position | null = null;
  const moves: string[] = [];
  for (const [, name, value] of body.matchAll(/([A-Z]+)\[([^\]]*)\]/g)) {
    if (name === 'BO') {
      const cs = [...value.replace(/^8+/, '').replace(/\s/g, '')].map(boardCell);
      if (cs.length === 65 && cs.every((c) => c !== undefined)) start = boardOf(cs as Cell[]);
    } else if (name === 'B' || name === 'W') {
      const mv = value.split('/')[0].trim().toLowerCase();
      if (mv.length === 2 && mv !== 'pa') moves.push(mv);
    }
  }
  return start || moves.length ? { start, moves } : null;
}

/** Othello Quest's saved game (JSON): the moves of `position.moves[].m`, in order. */
function quest(text: string): string[] | null {
  let v: unknown;
  try {
    v = JSON.parse(text.trim());
  } catch {
    return null;
  }
  const find = (x: unknown): unknown[] | undefined => {
    if (Array.isArray(x)) {
      for (const e of x) {
        const m = find(e);
        if (m) return m;
      }
      return undefined;
    }
    if (!x || typeof x !== 'object') return undefined;
    const o = x as Record<string, unknown>;
    if (Array.isArray(o.moves) && o.moves.some((e) => e && typeof e === 'object' && 'm' in e)) return o.moves;
    for (const e of Object.values(o)) {
      const m = find(e);
      if (m) return m;
    }
    return undefined;
  };
  const ms = (find(v) ?? [])
    .map((e) => (e as { m?: unknown }).m)
    .filter((m): m is string => typeof m === 'string')
    .map((m) => m.toLowerCase())
    .filter((m) => m.length === 2 && m !== 'pa' && m !== 'ps');
  return ms.length ? ms : null;
}

/** Every move played from `start`; null when one of them cannot be played there. */
function playOut(start: Position, names: string[]): number[] | null {
  let p = start;
  const out: number[] = [];
  for (const n of names) {
    const sq = 'abcdefgh'.indexOf(n[0]) * 8 + (Number(n[1]) - 1);
    const next = play(p, sq);
    if (!next) return null;
    p = next;
    out.push(sq);
  }
  return out;
}

function game(start: Position | null, names: string[], format: RecordFormat): GameRecord | null {
  const s = start ?? { cells: START.slice(), turn: 1 as const };
  const moves = playOut(s, names);
  return moves ? { start: s, moves, format } : null;
}

/** The game in `text`, told apart by its look: GGF, Othello Quest, a board with or without moves, or moves alone. */
export function readRecord(text: string): GameRecord | null {
  const g = ggf(text);
  if (g) return game(g.start, g.moves, 'ggf');
  const q = quest(text);
  if (q) return game(null, q, 'quest');
  const b = findBoard(text);
  if (b) {
    const names = squaresIn(b.rest);
    return game(b.board, names, names.length ? 'board_moves' : 'board');
  }
  const names = squaresIn(text);
  return names.length ? game(null, names, 'moves') : null;
}
