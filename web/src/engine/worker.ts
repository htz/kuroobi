/// <reference lib="webworker" />
import { KuroobiAI } from '../../../crates/web/kuroobi_ai.js';
import { bits } from '../lib/othello';
import { MODEL_INDEX, WASM_MT_URL, WASM_URL, type ModelIndex, type Reply, type Request } from './protocol';

declare const self: DedicatedWorkerGlobalScope;
const post = (r: Reply) => self.postMessage(r);

/** The model from the browser's cache, or fetched with progress and then kept there. */
async function model(): Promise<Uint8Array> {
  const index = await fetch(MODEL_INDEX, { cache: 'no-cache' });
  if (!index.ok) throw new Error(`${MODEL_INDEX}: ${index.status}`);
  const { url: MODEL_URL, bytes: total } = (await index.json()) as ModelIndex;
  const cache = 'caches' in self ? await caches.open('kuroobi-model').catch(() => null) : null;
  const hit = await cache?.match(MODEL_URL);
  if (hit) return new Uint8Array(await hit.arrayBuffer());
  // Only one model is kept: an older version goes once this one is fetched.
  for (const old of (await cache?.keys()) ?? []) await cache?.delete(old);
  const res = await fetch(MODEL_URL);
  if (!res.ok || !res.body) throw new Error(`${MODEL_URL}: ${res.status}`);
  const parts: Uint8Array[] = [];
  let loaded = 0;
  const reader = res.body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    parts.push(value);
    loaded += value.length;
    post({ kind: 'progress', loaded, total });
  }
  const bytes = new Uint8Array(loaded);
  let at = 0;
  for (const p of parts) {
    bytes.set(p, at);
    at += p.length;
  }
  await cache?.put(MODEL_URL, new Response(bytes)).catch(() => undefined);
  return bytes;
}

/**
 * Search threads: all but two of the cores, at most 8, as FlipSight's engine takes them. Safari reports 4 cores
 * whatever the device has, and an iPhone or a Mac has at least 6.
 */
function threads(): number {
  const cores = navigator.hardwareConcurrency || 1;
  const ua = navigator.userAgent;
  if (cores === 4 && /AppleWebKit/.test(ua) && !/Chrome\/|Chromium\/|Edg\/|Firefox\//.test(ua)) return 6;
  return cores > 9 ? 8 : Math.max(1, cores - 2);
}

/** The threaded engine where the page can share memory with workers, else the single-threaded one. */
function load(m: Uint8Array): Promise<KuroobiAI> {
  if (!self.crossOriginIsolated || threads() < 2) return KuroobiAI.load(WASM_URL, m);
  return KuroobiAI.load(WASM_MT_URL, m, {
    threads: threads(),
    spawnWorker: () => new Worker(new URL('../../../crates/web/kuroobi_ai_thread.js', import.meta.url), { type: 'module' }),
  });
}

const ready: Promise<KuroobiAI> = model().then(load);
ready.then(
  () => post({ kind: 'ready' }),
  (e: unknown) => post({ kind: 'failed', message: String(e) }),
);

self.onmessage = async (e: MessageEvent<Request>) => {
  const req = e.data;
  const ai = await ready;
  if (req.kind === 'newGame') {
    ai.newGame();
    post({ id: req.id, kind: 'newGame' });
    return;
  }
  const { black, white } = bits(req.cells);
  ai.setPosition(black, white, req.turn === 1);
  if (req.kind === 'choose') {
    const t0 = performance.now();
    const c = ai.chooseLevel(req.level.depth, req.level.solve);
    post({ id: req.id, kind: 'choose', choice: { ...c, ms: performance.now() - t0 } });
  }
  else if (req.kind === 'evaluate') post({ id: req.id, kind: 'evaluate', value: ai.evaluate(req.level.depth, req.level.solve) });
  else post({ id: req.id, kind: 'analyze', moves: ai.analyze(req.depth) });
};
