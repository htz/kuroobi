import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { jsLog } from './api';

/* Single window. Settings returned to an overlay (2026-08-08): only
 * the Display tab benefited from a window, which kept overlapping the
 * board and dragged in cross-window sync. */
/* The WebView console is invisible; exceptions go to the backend log
   (/tmp/kuroobi_js.log) — the only clue when the window goes black. */
window.addEventListener('error', (e) => jsLog('window.error: ' + (e.error?.stack ?? e.message)));
window.addEventListener('unhandledrejection', (e) => jsLog('unhandled: ' + String(e.reason)));

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
