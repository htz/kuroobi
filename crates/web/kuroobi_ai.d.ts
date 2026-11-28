export interface Choice {
  move: number;
  score: number;
  depth: number;
  exact: boolean;
  /** A selective endgame solve's confidence in %; 0 when it was not one. */
  selective: number;
  /** Positions searched for it, the solve included. */
  nodes: number;
}

export interface Valued {
  move: number;
  score: number;
}

export class KuroobiAI {
  /** `spawnWorker` (starting kuroobi_ai_thread.js) takes the wasm32-wasip1-threads build on shared memory, searching on `threads` threads. */
  static load(
    wasmUrl: string | URL | ArrayBuffer | Uint8Array,
    modelBytes: ArrayBuffer | Uint8Array,
    options?: { threads?: number; spawnWorker?: () => Worker },
  ): Promise<KuroobiAI>;
  newGame(): void;
  setPosition(black: bigint, white: bigint, blackToMove: boolean): void;
  choose(clockMs: number): Choice;
  chooseLevel(depth: number, solveEmpties: number): Choice;
  evaluate(depth: number, solveEmpties: number): { score: number; exact: boolean; depth: number; selective: number };
  analyze(depth: number): Valued[];
  stop(): void;
}
