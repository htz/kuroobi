// Endgame problems as obf files: each line a board, the side to move, then (optionally) every move's exact value for
// that side, best first. FFO's four sets (problems/fforum-*.obf) are built in; any other obf can be loaded.
import set1 from '../../../problems/fforum-1-19.obf?raw';
import set20 from '../../../problems/fforum-20-39.obf?raw';
import set40 from '../../../problems/fforum-40-59.obf?raw';
import set60 from '../../../problems/fforum-60-79.obf?raw';
import { readRecord } from './record';
import { parseSquare, squareName, type Position } from './othello';

/** `best`, `score` and every move's `values` (squares as the board numbers them) only where the obf gives them. */
export type Problem = {
  no: number; pos: Position; empties: number; best?: string[]; score?: number; values?: { sq: number; score: number }[];
};
/** `name` is what a problem's number follows (FFO #40); `label` what the set is listed as. */
export type ProblemSet = { name: string; label: string; problems: Problem[] };

/** The problems in `text`, numbered from `from`; lines that are not a board are skipped. */
export function readObf(text: string, from = 1): Problem[] {
  const out: Problem[] = [];
  for (const line of text.split(/\r?\n/)) {
    const [board, ...values] = line.split(';').map((s) => s.trim());
    const pos = board ? readRecord(board)?.start : undefined;
    if (!pos || readRecord(board)!.format !== 'board') continue;
    const scored = values.filter(Boolean).map((v) => {
      const [sq, score] = v.split(':');
      return { move: sq.trim().toLowerCase(), score: Number(score) };
    }).filter((s) => /^[a-h][1-8]$/.test(s.move) && Number.isFinite(s.score));
    const score = scored[0]?.score;
    out.push({
      no: from + out.length, pos, empties: pos.cells.filter((c) => c === 0).length,
      ...(scored.length ? {
        score, best: scored.filter((s) => s.score === score).map((s) => s.move),
        values: scored.map((s) => ({ sq: parseSquare(s.move), score: s.score })),
      } : {}),
    });
  }
  return out;
}

const ffo = (text: string, from: number): ProblemSet => {
  const problems = readObf(text, from);
  return { name: 'FFO', label: `FFO #${from}–${from + problems.length - 1}`, problems };
};

export const FFO_SETS: ProblemSet[] = [ffo(set1, 1), ffo(set20, 20), ffo(set40, 40), ffo(set60, 60)];

/** Whether KUROOBI's move and exact value are the problem's answer; undefined where the obf gives none. */
export const solvedRight = (p: Problem, move: number, score: number): boolean | undefined =>
  p.best ? Math.round(score) === p.score && p.best.includes(squareName(move)) : undefined;
