import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from './api';
import { useGame } from './state';
import type { GameView } from './types';
import { Board } from './components/Board';
import { Graph, type GraphPoint } from './components/Graph';
import { Nav, type View } from './components/Nav';
import { Panel } from './components/Panel';
import { SettingsModal } from './components/SettingsModal';

export function App() {
  const g = useGame();
  const [view, setView] = useState<View>('play');
  // 設定ダイアログ。開くときにファイルの状態を取ってから渡す。
  const [settings, setSettings] = useState<[string, string, boolean][] | null>(null);
  const [paste, setPaste] = useState(false);
  const [pasteText, setPasteText] = useState('');

  // ---- 評価値グラフ ----
  const [gvals, setGvals] = useState<(GraphPoint | undefined)[] | null>(null);
  const [gbusy, setGbusy] = useState(false);
  const gseq = useRef(0);
  const lineKey = (v: GameView | null) =>
    v ? v.moves.map((m) => (m == null ? 'p' : m)).join(',') : '';
  const gkey = useRef('');

  // 手順が変わったらグラフは無効
  useEffect(() => {
    const k = lineKey(g.view);
    if (k !== gkey.current) { gkey.current = k; setGvals(null); }
  }, [g.view]);

  // 設定が変わったらエンジンへ流す (値に依存させる。関数を並べると
  // 毎回作り直されて無限に走る)
  const { depth, solve, band } = g.levels;
  useEffect(() => {
    api.setLevels(depth, solve, band).catch(() => {});
  }, [depth, solve, band]);
  useEffect(() => { api.setUseBook(g.useBook).catch(() => {}); }, [g.useBook]);

  // 局面が動いたら採点し直す
  const refresh = g.refreshHints;
  useEffect(() => { void refresh(); }, [g.view, g.autoHint, refresh]);

  const isGgs = view.startsWith('ggs-');

  // ---- エンジンの手番 ----
  // 局面が「エンジンが打つ番」になったら 1 度だけ走る。二重起動を
  // 防ぐため、走っている間は ref で塞ぐ。
  const turnRef = useRef(false);
  const { playing, view: gv, engineSides, setThinking, setThinkSecs, setThinkTotal,
          setMoveSource, setView: applyView, setPlaying, say } = g;
  useEffect(() => {
    if (!playing || !gv || gv.over || turnRef.current) return;
    if (!engineSides().includes(gv.player)) return;

    turnRef.current = true;
    const side = gv.player as 'black' | 'white';
    const t0 = performance.now();
    setThinking(true);
    const timer = window.setInterval(
      () => setThinkSecs((performance.now() - t0) / 1000), 100);

    void (async () => {
      try {
        const r = await api.think();
        setThinkTotal((t) => ({ ...t, [side]: t[side] + r.secs }));
        const next = await api.applyMove(r.pos);
        // 何手目の手だったかを記録する (棋譜に出所を出すため)
        setMoveSource((m) => ({ ...m, [next.cursor]: r.from_book ? 'book' : 'search' }));
        applyView(next);
        say(r.pos === null ? 'パス' : '');
      } catch (e) {
        say('' + e);
        setPlaying(false);
      } finally {
        clearInterval(timer);
        setThinking(false);
        setThinkSecs(0);
        turnRef.current = false;
      }
    })();

    return () => clearInterval(timer);
  }, [playing, gv, engineSides, setThinking, setThinkSecs, setThinkTotal,
      setMoveSource, applyView, setPlaying, say]);

  const updateGraph = useCallback(async () => {
    const v = g.view;
    if (gbusy || !v) return;
    setGbusy(true);
    const seq = ++gseq.current;
    const key = lineKey(v);
    const len = v.moves.length;
    const vals: (GraphPoint | undefined)[] = gvals && gkey.current === key
      ? [...gvals] : new Array(len + 1);
    gkey.current = key;
    await g.pushLevels();
    // 全局面を測るので深さは控えめに
    const depth = Math.min(g.levels.depth, 14);
    for (let n = 0; n <= len; n++) {
      if (seq !== gseq.current) break;
      if (vals[n]) continue;
      if (n < len && v.moves[n] == null) continue;   // パスの手番は測らない
      g.say(`グラフ計算中 ${n}/${len}…`, true);
      try {
        const p = await api.evalAt(n, depth);
        if (seq !== gseq.current) break;
        if (Number.isFinite(p.value)) vals[n] = { value: p.value, exact: p.exact };
        setGvals([...vals]);
      } catch (e) { g.say('' + e); break; }
    }
    if (seq === gseq.current) g.say('');
    setGbusy(false);
  }, [g, gbusy, gvals]);

  const loadText = async (text: string) => {
    try {
      g.setMoveSource({});
      g.setThinkTotal({ black: 0, white: 0 });
      g.setPlaying(false);
      g.setView(await api.loadKifuText(text));
      setPaste(false);
      setPasteText('');
      g.say('棋譜を読み込みました');
    } catch (e) { g.say('' + e); }
  };

  return (
    <>
      <Nav view={view} onView={(v) => {
        setView(v);
        if (v === 'play' || v === 'study') {
          g.setMode(v === 'study' ? 'study' : 'vs');
          if (v === 'study' && g.playing) g.stop();
        }
      }} online={false} onSettings={async () => {
        setSettings(await api.resourceStatus().catch(() => []));
      }} />

      {isGgs ? (
        <div className="placeholder">
          <div className="placeholder-box">
            <div className="placeholder-title">GGS はまだこのアプリに入っていません</div>
            <p className="hint">
              オンライン対局・観戦・ロビー・チャットは、これから実装します。
            </p>
          </div>
        </div>
      ) : (
        <>
          <div id="main">
            <div id="board-wrap">
              <Board
                cells={g.view?.cells ?? new Array(64).fill(0)}
                legal={g.view?.legal ?? []}
                last={g.view?.last ?? null}
                next={g.view ? g.view.moves[g.view.cursor] ?? null : null}
                hints={g.hints}
                disabled={g.thinking}
                onPlay={(sq) => void g.play(sq)}
              />
            </div>
            {g.mode === 'study' && g.view && (
              <div className="card" id="graph-card">
                <div className="row" style={{ alignItems: 'center' }}>
                  <label className="field" style={{ flex: 1, margin: 0 }}>
                    評価値グラフ (黒視点)
                  </label>
                  <button className="btn small" onClick={() => void updateGraph()}>更新</button>
                </div>
                <Graph values={gvals} moves={g.view.moves} cursor={g.view.cursor}
                       busy={gbusy} onJump={(n) => void g.jumpTo(n)} />
              </div>
            )}
          </div>

          <Panel g={g}
                 onStart={() => {
                   if (g.playing) { g.stop(); g.say('対局を停止しました'); }
                   else { g.setPlaying(true); g.say(''); }
                 }}
                 onSave={() => void api.saveKifu().catch((e) => g.say('' + e))}
                 onLoad={() => setPaste(true)} />
        </>
      )}

      {paste && (
        <div id="paste-modal">
          <div className="box">
            <label className="field" style={{ margin: 0 }}>
              棋譜の読み込み — GGF・f5d6… 形式・盤面つきのいずれでも
            </label>
            <textarea value={pasteText} onChange={(e) => setPasteText(e.target.value)}
                      placeholder="f5d6c3d3c4f4f3e3e2…" />
            <div className="row">
              <button className="btn" onClick={async () => {
                try {
                  const v = await api.loadKifu();
                  if (v) { g.setView(v); setPaste(false); }
                } catch (e) { g.say('' + e); }
              }}>ファイルから…</button>
              <button className="btn primary" onClick={() => void loadText(pasteText)}>
                読み込む
              </button>
              <button className="btn" onClick={() => setPaste(false)}>キャンセル</button>
            </div>
          </div>
        </div>
      )}

      {settings && (
        <SettingsModal initial={settings} onClose={() => setSettings(null)}
                       onChanged={() => { void api.hasBook().then(g.setHasBook); }} />
      )}
    </>
  );
}
