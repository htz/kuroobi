import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { jsLog } from './api';

/* 窓は 1 つだけ。設定は主画面の覆いに戻した (2026-08-08) —
 * 窓である値打ちは「表示」タブだけが持っていたのに、1240 幅の主画面では
 * 盤に重なるのを避けられず、そのために窓またぎの同期まで抱えていた。
 */
/* WebView のコンソールは見えないので、落ちたらバックエンドのログへ送る
   (/tmp/kuroobi_js.log)。窓が真っ黒になったときの唯一の手がかりになる。 */
window.addEventListener('error', (e) => jsLog('window.error: ' + (e.error?.stack ?? e.message)));
window.addEventListener('unhandledrejection', (e) => jsLog('unhandled: ' + String(e.reason)));

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
