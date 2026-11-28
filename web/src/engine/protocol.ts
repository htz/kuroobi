/** What the page asks the engine worker, and what comes back. Boards travel as cells; the worker makes the bitboards. */
import type { Cell, Side } from '../lib/othello';

/** A strength as the app's levels set it: a search depth, and the exact solve from so many squares left. */
export type Level = { depth: number; solve: number };

export type Request =
  | { id: number; kind: 'choose'; cells: Cell[]; turn: Side; level: Level }
  | { id: number; kind: 'evaluate'; cells: Cell[]; turn: Side; level: Level }
  | { id: number; kind: 'analyze'; cells: Cell[]; turn: Side; depth: number }
  | { id: number; kind: 'newGame' };

/** KUROOBI's move, and what it took: `nodes` searched in `ms` milliseconds. */
/** `selective`: a selective endgame solve's confidence in %, 0 when it was not one. */
export type Choice = { move: number; score: number; depth: number; exact: boolean; selective: number; nodes: number; ms: number };
export type Valued = { move: number; score: number };
/** `depth` is how deep the search reached; 0 when the value was solved. */
export type Value = { score: number; exact: boolean; depth: number; selective: number };

export type Reply =
  | { kind: 'progress'; loaded: number; total: number }
  | { kind: 'ready' }
  | { kind: 'failed'; message: string }
  | { id: number; kind: 'choose'; choice: Choice }
  | { id: number; kind: 'evaluate'; value: Value }
  | { id: number; kind: 'analyze'; moves: Valued[] }
  | { id: number; kind: 'newGame' };

/** The wasm is the site's own, named by its hash at build time (scripts/engine.sh puts it there). */
export const WASM_URL = `/engine/kuroobi_ai-${__WASM_HASH__}.wasm`;
/**
 * The model is deployed apart from the site: this small file, never cached, names the current one
 * (a path carrying its hash, so a browser fetches the model again only when it changed) and its size.
 */
export const MODEL_INDEX = '/engine/model.json';
export type ModelIndex = { url: string; bytes: number };
