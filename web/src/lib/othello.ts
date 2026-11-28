/** 0 empty, 1 black, 2 white; square `file*8 + rank` (a1 = 0, a2 = 1, b1 = 8), as the app's board takes it. */
export type Cell = 0 | 1 | 2;
export type Side = 1 | 2;

export const START: readonly Cell[] = Array.from({ length: 64 }, (_, sq) =>
  sq === 28 || sq === 35 ? 1 : sq === 27 || sq === 36 ? 2 : 0,
);

const DIRS = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]] as const;

export function flips(cells: readonly Cell[], sq: number, me: Side): number[] {
  if (cells[sq]) return [];
  const f0 = sq >> 3, r0 = sq & 7, op = 3 - me, out: number[] = [];
  for (const [df, dr] of DIRS) {
    const run: number[] = [];
    let f = f0 + df, r = r0 + dr;
    while (f >= 0 && f < 8 && r >= 0 && r < 8 && cells[f * 8 + r] === op) {
      run.push(f * 8 + r);
      f += df;
      r += dr;
    }
    if (run.length && f >= 0 && f < 8 && r >= 0 && r < 8 && cells[f * 8 + r] === me) out.push(...run);
  }
  return out;
}

export const legal = (cells: readonly Cell[], me: Side): number[] =>
  Array.from({ length: 64 }, (_, i) => i).filter((i) => flips(cells, i, me).length > 0);

export type Position = { cells: Cell[]; turn: Side };

/** The position after `sq`, with the turn passed on when the opponent has no move; null if `sq` is illegal. */
export function play(p: Position, sq: number): Position | null {
  const f = flips(p.cells, sq, p.turn);
  if (!f.length) return null;
  const cells = p.cells.slice();
  cells[sq] = p.turn;
  for (const x of f) cells[x] = p.turn;
  const next = (3 - p.turn) as Side;
  return { cells, turn: legal(cells, next).length || !legal(cells, p.turn).length ? next : p.turn };
}

export const isOver = (p: Position): boolean => !legal(p.cells, 1).length && !legal(p.cells, 2).length;

export const squareName = (sq: number): string => 'abcdefgh'[sq >> 3] + ((sq & 7) + 1);

export const parseSquare = (s: string): number => 'abcdefgh'.indexOf(s[0]) * 8 + (Number(s[1]) - 1);

/** Plays a record such as `f5d6c3`, stopping at the first illegal move. */
export function replay(record: string): { pos: Position; moves: number[] } {
  let pos: Position = { cells: START.slice(), turn: 1 };
  const moves: number[] = [];
  for (let i = 0; i + 1 < record.length; i += 2) {
    const sq = parseSquare(record.slice(i, i + 2));
    const next = play(pos, sq);
    if (!next) break;
    pos = next;
    moves.push(sq);
  }
  return { pos, moves };
}

/** The bitboards the engine takes: bit `n` is square `n`. */
export function bits(cells: readonly Cell[]): { black: bigint; white: bigint } {
  let black = 0n, white = 0n;
  cells.forEach((c, i) => {
    if (c === 1) black |= 1n << BigInt(i);
    else if (c === 2) white |= 1n << BigInt(i);
  });
  return { black, white };
}
