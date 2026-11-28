import React from 'react';
import { Engine } from './client';

// One engine for the visit: the router swaps pages without reloading, so play, study and the benchmark share the
// worker and the evaluation it has read instead of each loading them again.
let shared: Engine | null = null;
let sharedReady = false;
let sharedFailed: string | null = null;

/** The engine, made once in the browser; the server render never starts a worker. */
export function useEngine(enabled: boolean) {
  const [engine, setEngine] = React.useState<Engine | null>(null);
  const [progress, setProgress] = React.useState<{ loaded: number; total: number } | null>(null);
  const [ready, setReady] = React.useState(false);
  const [failed, setFailed] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (!enabled) return;
    if (!shared) {
      shared = new Engine();
      shared.ready.then(() => { sharedReady = true; }, (err: Error) => { sharedFailed = err.message; });
    }
    const e = shared;
    e.onProgress = (loaded, total) => setProgress({ loaded, total });
    if (sharedReady) setReady(true);
    else if (sharedFailed) setFailed(sharedFailed);
    else e.ready.then(() => setReady(true), (err: Error) => setFailed(err.message));
    setEngine(e);
    // A page left mid-search would keep the next one waiting behind that search.
    return () => {
      e.onProgress = () => {};
      if (e.busy) {
        sharedReady = false;
        e.restart().then(() => { sharedReady = true; }, () => {});
      }
    };
  }, [enabled]);
  return { engine, progress, ready, failed };
}
