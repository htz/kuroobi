import React from 'react';
import { Engine } from './client';

/** The engine, made once in the browser; the server render never starts a worker. */
export function useEngine(enabled: boolean) {
  const [engine, setEngine] = React.useState<Engine | null>(null);
  const [progress, setProgress] = React.useState<{ loaded: number; total: number } | null>(null);
  const [ready, setReady] = React.useState(false);
  const [failed, setFailed] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (!enabled) return;
    const e = new Engine();
    e.onProgress = (loaded, total) => setProgress({ loaded, total });
    e.ready.then(() => setReady(true), (err: Error) => setFailed(err.message));
    setEngine(e);
  }, [enabled]);
  return { engine, progress, ready, failed };
}
