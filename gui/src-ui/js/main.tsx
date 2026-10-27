import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { jsLog } from './api';

window.addEventListener('error', (e) => jsLog('window.error: ' + (e.error?.stack ?? e.message)));
window.addEventListener('unhandledrejection', (e) => jsLog('unhandled: ' + String(e.reason)));

// In any list of rows, the up and down keys move to the neighbouring row and pick it, as a click
// would; in a menu they only move. WebKit does not focus a button on click (and a ring on every
// clicked row would be noise), so the last clicked row is remembered and the keys start from it.
let lastRow: HTMLElement | null = null;
const rowOf = (n: EventTarget | null) => (n instanceof HTMLElement ? n.closest<HTMLElement>('button.k-row') : null);
// The nav is left out: after picking a screen, the arrow keys belong to that screen.
document.addEventListener('click', (e) => {
  const row = rowOf(e.target);
  lastRow = row?.closest('.k-nav') ? null : row;
});
document.addEventListener('keydown', (e) => {
  if ((e.key !== 'ArrowDown' && e.key !== 'ArrowUp') || e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return;
  const focused = document.activeElement;
  const typing = focused instanceof HTMLInputElement || focused instanceof HTMLTextAreaElement || focused instanceof HTMLSelectElement;
  const row = rowOf(e.target) ?? (!typing && lastRow?.isConnected ? lastRow : null);
  if (!row) return;
  const list = row.closest('.k-scroll') ?? row.parentElement;
  if (!list) return;
  const rows = [...list.querySelectorAll<HTMLElement>('button.k-row')].filter((r) => r.tabIndex >= 0 && r.offsetParent);
  const next = rows[rows.indexOf(row) + (e.key === 'ArrowDown' ? 1 : -1)];
  e.preventDefault();
  e.stopPropagation();
  if (!next) return;
  lastRow = next;
  next.focus();
  next.scrollIntoView({ block: 'nearest' });
  if (row.getAttribute('role') !== 'menuitem') next.click();
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
