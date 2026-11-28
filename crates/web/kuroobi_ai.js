// Kuroobi for web's engine: kuroobi_ai.wasm and a web model, behind a few methods.
// Run it in a Worker: a search holds the thread until it returns.
// Squares are 0..63 (bit n of a side's discs); boards are BigInt pairs.
import { imports as threadImports, memoryLimits, poolControl } from './wasi_threads.js';

/** The single-thread build's (wasm32-unknown-unknown) clock, from the page. */
const imports = {
  env: {
    now_ms: () => performance.now(),
    wall_ms: () => Date.now(),
  },
};

/**
 * A selective solve's ProbCut confidence `t` (sigmas) as the share of a normal distribution within ±t, in whole
 * percent (Abramowitz-Stegun 7.1.26, as the app's `confidence_pct`); 0 when there was none.
 */
function confidence(t) {
  if (!(t > 0)) return 0;
  const x = t / Math.SQRT2;
  const k = 1 / (1 + 0.3275911 * x);
  const poly = k * (0.254829592 + k * (-0.284496736 + k * (1.421413741 + k * (-1.453152027 + k * 1.061405429))));
  return Math.round((1 - poly * Math.exp(-x * x)) * 100);
}

/**
 * The threads a search holds at once: the midgame's and the solver's helpers, a solve's own beside them, and its
 * deadline watch (4 threads held 10, 8 threads 22). The pool keeps a few more.
 */
const poolSize = (threads) => 3 * (threads - 1) + 1 + 4;

export class KuroobiAI {
  /**
   * @param {string | URL | ArrayBuffer | Uint8Array} wasmUrl where kuroobi_ai.wasm is, or its bytes
   * @param {ArrayBuffer | Uint8Array} modelBytes a web model (nnue_to_web's output)
   * @param {{ threads?: number, spawnWorker?: () => Worker }} [options] `spawnWorker`, which starts a pool worker
   *   (kuroobi_ai_thread.js), takes the wasm32-wasip1-threads build: it needs shared memory (a cross-origin isolated
   *   page) and searches on `threads` threads
   */
  static async load(wasmUrl, modelBytes, { threads = 1, spawnWorker } = {}) {
    const x = spawnWorker ? await KuroobiAI.instantiateThreaded(wasmUrl, threads, spawnWorker) : await KuroobiAI.instantiate(wasmUrl);
    const model = modelBytes instanceof Uint8Array ? modelBytes : new Uint8Array(modelBytes);
    const ptr = x.alloc(model.length);
    new Uint8Array(x.memory.buffer, ptr, model.length).set(model);
    const failed = x.load_model(ptr, model.length, threads);
    x.dealloc(ptr, model.length);
    if (failed) throw new Error('kuroobi_ai: not a web model for this build');
    return new KuroobiAI(x);
  }

  static async instantiate(wasmUrl) {
    const { instance } =
      typeof wasmUrl === 'string' || wasmUrl instanceof URL
        ? await WebAssembly.instantiateStreaming(fetch(wasmUrl), imports)
        : await WebAssembly.instantiate(wasmUrl, imports);
    return instance.exports;
  }

  /**
   * The threaded build on shared memory, with its pool started and ready before any search: a search holds
   * threads of the engine's parallel searches and a deadline watch beside them, and one it cannot get panics.
   */
  static async instantiateThreaded(wasmUrl, threads, spawnWorker) {
    const bytes =
      typeof wasmUrl === 'string' || wasmUrl instanceof URL ? new Uint8Array(await (await fetch(wasmUrl)).arrayBuffer())
        : new Uint8Array(wasmUrl instanceof ArrayBuffer ? wasmUrl : wasmUrl.buffer);
    const module = await WebAssembly.compile(bytes);
    const memory = new WebAssembly.Memory(memoryLimits(bytes));
    const workers = poolSize(threads);
    const ctl = poolControl(workers);
    // This instance first: only the first one on a memory initializes it and sets up its own thread's storage.
    const instance = await WebAssembly.instantiate(module, threadImports(memory, ctl));
    instance.exports._initialize();
    await Promise.all(
      Array.from({ length: workers }, (_, slot) => new Promise((resolve, reject) => {
        const w = spawnWorker();
        w.onmessage = () => resolve(undefined);
        w.onerror = (e) => reject(new Error('kuroobi_ai: a pool worker failed: ' + (e.message || e)));
        w.postMessage({ module, memory, ctl, slot });
      })),
    );
    return instance.exports;
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
      selective: confidence(this.x.last_selective()),
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
      selective: confidence(this.x.last_selective()),
      nodes: this.x.last_nodes(),
    };
  }

  /** The position's value for the mover: solved from `solveEmpties` squares left, else a `depth`-deep search. */
  evaluate(depth, solveEmpties) {
    const score = this.x.evaluate(depth >>> 0, solveEmpties >>> 0);
    return {
      score,
      exact: this.x.last_exact() !== 0,
      depth: this.x.last_depth(),
      selective: confidence(this.x.last_selective()),
    };
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
