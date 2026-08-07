import { useEffect } from 'react';
import { Settings } from './Dialogs';
import { usePrefs } from './prefs';
import { useGgs } from './ggs';
import { BoardDefs } from './components/board';

/* 設定の窓。`index.html?w=settings` で開く (main.tsx が振り分ける)。
 *
 * 主画面の `AppFrame` は使わない — あちらは Toast の基準や畳の <pattern> を
 * 抱えていて、この窓には要らないものが多い。要るのは
 *   - 見え方の適用 (usePrefs がテーマと石返しを :root に書く)
 *   - <BoardDefs/> (表示タブに盤の見本を置くときに要る。今は畳の色見本を
 *     出していないが、置き忘れると url(#…) が静かに解決しなくなるので、
 *     盤を出す窓には必ず 1 つ置く決まりにしてある — 規則 54)
 * の 2 つだけ。
 */
export function SettingsWindow() {
  const { prefs, set } = usePrefs();
  // GGS タブの中身に要る。窓が別でも Tauri のイベントは届く
  const ggs = useGgs();

  // Esc で閉じる。窓なので ⌘W でも閉じられるが、覆いから窓に変えた後も
  // 同じ指の動きで閉じられるようにしておく
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || (e.metaKey && e.key.toLowerCase() === 'w')) {
        e.preventDefault();
        window.close();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <>
      <BoardDefs />
      <Settings prefs={prefs} setPref={set} ggs={ggs.snap} />
    </>
  );
}
