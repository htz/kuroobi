// Kuroobi for web's engine: kuroobi_ai.wasm and a web model, behind a few methods.
// Run it in a Worker: a search holds the thread until it returns.
// Squares are 0..63 (bit n of a side's discs); boards are BigInt pairs.

const imports = {
  env: {
    now_ms: () => performance.now(),
    wall_ms: () => Date.now(),
  },
};

export class KuroobiAI {
  /**
   * @param {string | URL | ArrayBuffer | Uint8Array} wasmUrl where kuroobi_ai.wasm is, or its bytes
   * @param {ArrayBuffer | Uint8Array} modelBytes a web model (nnue_to_web's output)
   */
  static async load(wasmUrl, modelBytes) {
    const { instance } =
      typeof wasmUrl === 'string' || wasmUrl instanceof URL
        ? await WebAssembly.instantiateStreaming(fetch(wasmUrl), imports)
        : await WebAssembly.instantiate(wasmUrl, imports);
    const x = instance.exports;
    const model = modelBytes instanceof Uint8Array ? modelBytes : new Uint8Array(modelBytes);
    const ptr = x.alloc(model.length);
    new Uint8Array(x.memory.buffer, ptr, model.length).set(model);
    const failed = x.load_model(ptr, model.length);
    x.dealloc(ptr, model.length);
    if (failed) throw new Error('kuroobi_ai: not a web model for this build');
    return new KuroobiAI(x);
  }

  constructor(exports) {
    this.x = exports;
  }

  newGame() {
    this.x.new_game();
  }

  /** @param {bigint} black @param {bigint} white @param {boolean} blackToMove */
  setPosition(black, white, blackToMove) {
    this.x.set_position(BigInt.asUintN(64, black), BigInt.asUintN(64, white), blackToMove ? 1 : 0);
  }

  /** The move to play with `clockMs` left; `move` is -1 when there is none to play. */
  choose(clockMs) {
    const move = this.x.choose(clockMs >>> 0);
    return {
      move,
      score: this.x.last_score(),
      depth: this.x.last_depth(),
      exact: this.x.last_exact() !== 0,
      nodes: this.x.last_nodes(),
    };
  }

  /** The move at a fixed strength: a `depth`-deep search, or the exact solve from `solveEmpties` squares left. */
  chooseLevel(depth, solveEmpties) {
    const move = this.x.choose_level(depth >>> 0, solveEmpties >>> 0);
    return {
      move,
      score: this.x.last_score(),
      depth: this.x.last_depth(),
      exact: this.x.last_exact() !== 0,
      nodes: this.x.last_nodes(),
    };
  }

  /** The position's value for the mover: solved from `solveEmpties` squares left, else a `depth`-deep search. */
  evaluate(depth, solveEmpties) {
    const score = this.x.evaluate(depth >>> 0, solveEmpties >>> 0);
    return { score, exact: this.x.last_exact() !== 0, depth: this.x.last_depth() };
  }

  /** Every legal move valued at `depth`, best first. */
  analyze(depth) {
    const n = this.x.analyze(depth >>> 0);
    const out = [];
    for (let i = 0; i < n; i++) {
      out.push({ move: this.x.analysis_move(i), score: this.x.analysis_score(i) });
    }
    return out;
  }

  stop() {
    this.x.stop();
  }
}
