import { useSyncExternalStore } from 'react';
import { FLASH_MS } from './components/layout';

// Settings apply as soon as they change; the settings toolbar says so for a moment afterwards.
let visible = false;
let timer: number | undefined;
const subs = new Set<() => void>();
const notify = () => subs.forEach((f) => f());

export function markApplied(): void {
  visible = true;
  window.clearTimeout(timer);
  timer = window.setTimeout(() => { visible = false; notify(); }, FLASH_MS);
  notify();
}

export function useApplied(): boolean {
  return useSyncExternalStore((f) => { subs.add(f); return () => { subs.delete(f); }; }, () => visible);
}
