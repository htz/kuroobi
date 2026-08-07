import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { SettingsWindow } from './SettingsWindow';
import { jsLog } from './api';

/* 付属ウィンドウ (設定・定石) も同じ index.html を読み、`?w=` で中身を
 * 出し分ける。窓ごとにバンドルを分けると、同じ部品を二重に読み込むうえに
 * ビルドの入口が増える。 */
/* WebView のコンソールは見えないので、落ちたらバックエンドのログへ送る
   (/tmp/kuroobi_js.log)。窓が真っ黒になったときの唯一の手がかりになる。 */
window.addEventListener('error', (e) => jsLog('window.error: ' + (e.error?.stack ?? e.message)));
window.addEventListener('unhandledrejection', (e) => jsLog('unhandled: ' + String(e.reason)));

const w = new URLSearchParams(location.search).get('w');

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {w === 'settings' ? <SettingsWindow /> : <App />}
  </StrictMode>,
);
