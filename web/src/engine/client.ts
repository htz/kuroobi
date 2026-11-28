import type { Cell, Side } from '../lib/othello';
import type { Choice, Level, Reply, Request, Valued, Value } from './protocol';

type Dist<T> = T extends unknown ? Omit<T, 'id'> : never;

/**
 * The engine in a worker. A search holds the worker until it returns, so stopping one means
 * replacing the worker: the new one reads the model back from the browser's cache.
 */
export class Engine {
  private worker!: Worker;
  private next = 1;
  private pending = new Map<number, (r: Reply) => void>();
  onProgress: (loaded: number, total: number) => void = () => {};
  ready!: Promise<void>;
  /** The threads it searches on, known once ready. */
  threads = 1;

  constructor() {
    this.start();
  }

  private start() {
    this.worker = new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });
    this.ready = new Promise((resolve, reject) => {
      this.worker.onmessage = (e: MessageEvent<Reply>) => {
        const r = e.data;
        if (r.kind === 'progress') this.onProgress(r.loaded, r.total);
        else if (r.kind === 'ready') {
          this.threads = r.threads;
          resolve();
        }
        else if (r.kind === 'failed') reject(new Error(r.message));
        else {
          this.pending.get(r.id)?.(r);
          this.pending.delete(r.id);
        }
      };
    });
  }

  /** Ends whatever the engine is doing; replies still owed never come. */
  restart(): Promise<void> {
    this.worker.terminate();
    this.pending.clear();
    this.start();
    return this.ready;
  }

  private ask(req: Dist<Request>): Promise<Reply> {
    const id = this.next++;
    return new Promise((resolve) => {
      this.pending.set(id, resolve);
      this.worker.postMessage({ ...req, id });
    });
  }

  async choose(cells: Cell[], turn: Side, level: Level): Promise<Choice> {
    const r = await this.ask({ kind: 'choose', cells, turn, level });
    return (r as Extract<Reply, { kind: 'choose' }>).choice;
  }

  async evaluate(cells: Cell[], turn: Side, level: Level): Promise<Value> {
    const r = await this.ask({ kind: 'evaluate', cells, turn, level });
    return (r as Extract<Reply, { kind: 'evaluate' }>).value;
  }

  async analyze(cells: Cell[], turn: Side, depth: number): Promise<Valued[]> {
    const r = await this.ask({ kind: 'analyze', cells, turn, depth });
    return (r as Extract<Reply, { kind: 'analyze' }>).moves;
  }

  async newGame(): Promise<void> {
    await this.ask({ kind: 'newGame' });
  }
}
