import { useCallback, useEffect, useState } from 'react';
import { api, emitApp, jsLog, onApp, openWindow } from './api';
import { usePrefs } from './prefs';
import { useActivity, useLearnLog } from './engine';
import { BoardDefs } from './components/board';
import { Body, Main, StatusBar, StatusStat, Toolbar } from './components/layout';
import { Button } from './components/primitives';
import { BookInfo, BookPane, BookTree, useBookBrowse } from './BookScreen';
import { LearnLog } from './LearnLog';
import { Confirm } from './Dialogs';

/* 定石ブラウザと学習ログの窓 (`index.html?w=book`、⌘B で開く)。
 *
 * **設計 §7・§8 と規則の「定石ブラウザ / 学習ログ」の節が揃って
 * 「独立ウィンドウを 1 枚だけ持ち、定石 と 学習ログ をタブで切り替える」**と
 * 言っている。主画面の行き先にしていたので、左メニューが 12 行に膨らみ、
 * 対局を見ながら定石を辿ることもできなかった。
 *
 * タブの帯は設定の窓と同じ形にした。**設計 §7 は窓の帯の中に Segmented を
 * 置いている**が、規則 9 / 75 が「窓の帯には押せるものを 1 つも置かない」と
 * 決めているので、帯を 2 段にして 2 段目に置く (設定の窓と同じ)。
 * 食い違いは SYNC-FROM-IMPL.md §20 に書いた。
 */

const TABS = ['定石', '学習ログ'] as const;
type Tab = typeof TABS[number];

export function BookWindow() {
  const { prefs } = usePrefs();
  const [tab, setTab] = useState<Tab>('定石');
  const b = useBookBrowse(true);
  const cpu = useActivity();
  const { items: learnLog, reload: learnLogReload } = useLearnLog(tab === '学習ログ', !!cpu?.learn);
  const [ask, setAsk] = useState<{ msg: string; done: (ok: boolean) => void } | null>(null);
  const confirm = useCallback(
    (msg: string) => new Promise<boolean>((done) => setAsk({ msg, done })),
    [setAsk]);

  // Esc でも閉じられるようにする (⌘W と同じ)。設定の窓と揃える
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
      if (e.key === 'Escape' || (e.metaKey && e.key.toLowerCase() === 'w')) {
        e.preventDefault();
        window.close();
        return;
      }
      if (tab !== '定石') return;
      // 手順を辿るキー操作は主画面の定石画面から持ってきたもの
      if (e.key === 'ArrowLeft') { e.preventDefault(); b.back(); }
      if (e.key === 'ArrowUp') { e.preventDefault(); b.reset(); }
      // 右は「いちばん値の高い手へ進む」。盤を見ながら本筋をなぞれる
      if (e.key === 'ArrowRight' && b.node?.moves.length) {
        e.preventDefault();
        b.push(b.node.moves[0].pos);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [b, tab]);

  /* 主画面から「この手順で開いて」と言われる (検討の「定石で開く」)。
     窓は別の document なので、状態ではなく報せで渡す。 */
  const goto = b.goto;
  useEffect(() => {
    const off = onApp<string>('book-line', (line) => { goto(line ?? ''); });
    return () => { void off.then((f) => f()); };
  }, [goto]);

  /* 動作確認用。`KUROOBI_AUTOPLAY=book:f5d6` はこの窓が自分で読む —
     主画面から渡すと、窓が立ち上がる前に報せが飛んで取りこぼす。 */
  useEffect(() => {
    void api.autoplay().then((v) => {
      const [who, line] = (v ?? '').split(':');
      if (who === 'book' && line) goto(line);
    }).catch((e) => jsLog('autoplay(book): ' + e));
  }, [goto]);

  /** 手順を主画面の検討へ渡す。窓は開いたままにする (見比べたい)。 */
  const toStudy = (kifu: string, ply?: number) => emitApp('open-study', { kifu, ply });

  return (
    <div style={{
      position: 'relative', height: '100%', display: 'flex', flexDirection: 'column',
      background: 'var(--bg)',
    }}>
      <BoardDefs />
      {/* 窓の帯とタブの帯は設定の窓と同じ 44px の --panel。主画面の帯
          (32px / --bg) とは別物なので、共有の WindowBar は使わない */}
      <div data-tauri-drag-region className="k-drag" style={{
        height: 'var(--h-bar)', flex: 'none', background: 'var(--panel)',
        borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center',
        padding: '0 var(--w-signals)',
      }}>
        <span data-tauri-drag-region style={{
          margin: '0 auto', fontSize: 'var(--fs-4)', fontWeight: 600, color: 'var(--text)',
        }}>{tab}</span>
      </div>
      <div style={{
        flex: 'none', height: 'var(--h-bar)', display: 'flex',
        alignItems: 'center', justifyContent: 'center', gap: 'var(--sp-1)',
        padding: '0 var(--sp-5)', background: 'var(--panel)',
        borderBottom: '1px solid var(--border)',
      }}>
        {TABS.map((v) => {
          const on = tab === v;
          return (
            <button key={v} type="button" className={'k-press' + (on ? ' k-on' : '')}
                    onClick={() => setTab(v)} aria-pressed={on}
                    style={{
                      height: 'var(--h-ctrl)', padding: '0 14px', border: 0,
                      borderRadius: 'var(--r-2)', fontSize: 'var(--fs-5)',
                      background: on ? 'var(--accent-dim)' : 'transparent',
                      color: on ? 'var(--on-accent)' : 'var(--sub)',
                      fontWeight: on ? 600 : 400,
                    }}>{v}</button>
          );
        })}
      </div>

      <Body>
        {/* 設計 §7 は 木 269 / 盤 / この局面 291 の 3 列。木を右のドックに
            入れていたときは盤の右に押し込まれ、枝の広がりが読めなかった */}
        {tab === '定石' && <BookTree b={b} decimals={prefs.decimals} onStudy={toStudy} />}
        <Main>
          {tab === '定石' ? (
            <>
              {/* 設計のツールバーは 手順を検索 / 出所の絞り込み / 深さの上限 /
                  登録局面。前の 3 つはエンジン側に口が無い (SYNC §20)。
                  戻る・最初へ は絵に無いが、木は今いる節より下しか出さないので
                  上へ戻る道がこれしか無い */}
              <Toolbar aux={<span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                              登録局面 <b style={{ color: 'var(--text)', fontWeight: 600 }}>
                                {b.node ? b.node.size.toLocaleString() : '—'}</b>
                            </span>}>
                <Button disabled={!b.line.length} onClick={b.back}>戻る</Button>
                <Button disabled={!b.line.length} onClick={b.reset}>最初へ</Button>
                <span style={{ marginLeft: 'var(--sp-3)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                  {b.line.length ? b.line.length + ' 手目' : '初期局面'}
                </span>
              </Toolbar>
              <BookPane b={b} coords={prefs.coords} grain={prefs.grain} flip={prefs.facing === 'white'}
                        onSettings={() => void openWindow('settings')} />
            </>
          ) : (
            <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-3)' }}>
              <LearnLog items={learnLog}
                onUndo={(e) => void (async () => {
                  if (!await confirm('この対局で書き換えた定石を元に戻します。よろしいですか。')) return;
                  try {
                    await api.learnUndo(e.at, e.kifu);
                    // 済んだことは報せない (規則 34)。行が消えるのが結果そのもの
                    learnLogReload();
                  } catch { /* 失敗はそのまま (窓にトーストの置き場がない) */ }
                })()}
                onOpen={(e, ply) => {
                  // 抽選開局は開始局面を頭に付けないと別の対局になる
                  toStudy(e.start ? e.start + '\n' + e.kifu : e.kifu, ply);
                }} />
            </div>
          )}
        </Main>
        {/* 定石が無いときは空の列も出さない (盤側が報せを出している) */}
        {tab === '定石' && b.node?.size !== 0 && <BookInfo b={b} decimals={prefs.decimals} />}
      </Body>

      <StatusBar right={<StatusStat label="定石" value={b.node ? b.node.size.toLocaleString() + ' 局面' : '—'} />} />

      {ask && (
        <Confirm title="確認" body={ask.msg} ok="続ける"
                 onCancel={() => { ask.done(false); setAsk(null); }}
                 onOk={() => { ask.done(true); setAsk(null); }} />
      )}
    </div>
  );
}
