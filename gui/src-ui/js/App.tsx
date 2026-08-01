import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ggsApi } from './api';
import { useGame } from './state';
import { useGgs } from './ggs';
import type { GameView } from './types';
import { Board } from './components/Board';
import { GgsView } from './components/GgsView';
import { Graph, type GraphPoint } from './components/Graph';
import { Nav, type View } from './components/Nav';
import { Panel } from './components/Panel';
import { SettingsModal } from './components/SettingsModal';

export function App() {
  const g = useGame();
  const ggs = useGgs();
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

  // ---- GGS ----
  const conn = ggs.snap?.conn;
  // チャットの未読数。開いている間は 0 で、離れるときに既読位置を進める。
  const chatTotal = ggs.snap?.chat.length ?? 0;
  const [chatSeen, setChatSeen] = useState(0);
  const chatUnread = view === 'ggs-chat' ? 0 : Math.max(0, chatTotal - chatSeen);

  // 進行中の対局は定期的に取り直す (他人の対局は開始通知が来ないことがある)
  useEffect(() => {
    if (conn !== 'online') return;
    const t = window.setInterval(() => { ggsApi.listMatches().catch(() => {}); }, 60000);
    return () => clearInterval(t);
  }, [conn]);

  // 画面確認用 (KUROOBI_GGS_AUTOVIEW): GGS の画面はそちらを開くまで
  // マウントされないので、指定があればまず GGS 側へ切り替える。
  // 個別の画面への移動は GgsView がマウント後に行う。
  useEffect(() => {
    void ggsApi.autoview().then((v) => { if (v) setView('ggs-play'); }).catch(() => {});
  }, []);


  // 動作確認用: KUROOBI_AUTOPLAY=both:11 のように指定すると自動で対局を始める
  const started = useRef(false);
  const { setPlaying: begin, setSide, setLevel } = g;
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void api.autoplay().then((v) => {
      if (!v) return;
      const [who, lv] = v.split(':');
      if (who === 'both') setSide('both');
      if (lv !== undefined && Number.isFinite(+lv)) setLevel(+lv);
      begin(true);
    }).catch(() => {});
  }, [begin, setSide, setLevel]);

  // ---- エンジンの手番 ----
  // 局面が「エンジンが打つ番」になったら 1 度だけ走る。二重起動を
  // 防ぐため、走っている間は ref で塞ぐ。
  const turnRef = useRef(false);
  const { playing, view: gv, engineSides, setThinking, setThinkSecs, setThinkTotal,
          setMoveSource, setView: applyView, setPlaying, say, setLastEval, setMode } = g;
  useEffect(() => {
    if (!playing || !gv) return;
    // 終局したら自分で止まる (停止を押させない)
    if (gv.over) { setPlaying(false); return; }
    if (turnRef.current) return;
    if (!engineSides().includes(gv.player)) return;

    turnRef.current = true;
    const side = gv.player as 'black' | 'white';
    const t0 = performance.now();
    setThinking(true);
    const timer = window.setInterval(
      () => setThinkSecs((performance.now() - t0) / 1000), 50);

    void (async () => {
      try {
        const r = await api.think();
        setThinkTotal((t) => ({ ...t, [side]: t[side] + r.secs }));
        const next = await api.applyMove(r.pos);
        // 何手目の手だったかを記録する (棋譜に出所を出すため)
        setMoveSource((m) => ({ ...m, [next.cursor]: r.from_book ? 'book' : 'search' }));
        applyView(next);
        // どのくらい良いと見て指したかを残す
        setLastEval(Number.isFinite(r.value)
          ? `エンジン評価: ${r.value > 0 ? '+' : ''}${
              r.exact ? r.value.toFixed(0) : r.value.toFixed(1)} 石`
            + (r.exact ? ' (完全読み)' : (r.from_book ? ' (定石)' : ''))
          : '');
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
      setMoveSource, applyView, setPlaying, say, setLastEval]);

  // GGS の棋譜 (GGF か着手列) を検討画面で開く。アプリ内で完結する。
  const openStudy = useCallback(async (kifu: string) => {
    try {
      const v = await api.loadKifuText(kifu);
      setMoveSource({});
      setThinkTotal({ black: 0, white: 0 });
      setLastEval('');
      setPlaying(false);
      setMode('study');
      applyView(v);
      setView('study');
      say('棋譜を読み込みました');
    } catch (e) { say('' + e); }
  }, [setMoveSource, setThinkTotal, setLastEval, setPlaying, setMode, applyView, say]);

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
      g.setLastEval('');
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
        if (view === 'ggs-chat' && v !== 'ggs-chat') setChatSeen(chatTotal);
        setView(v);
        if (v === 'play' || v === 'study') {
          g.setMode(v === 'study' ? 'study' : 'vs');
          if (v === 'study' && g.playing) g.stop();
        }
      }} online={conn === 'online'} badges={{
        'ggs-lobby': ggs.snap?.offers.filter((o) => o.incoming).length ?? 0,
        'ggs-play': ggs.snap?.matches.length ?? 0,
        'ggs-chat': chatUnread,
      }} onSettings={async () => {
        setSettings(await api.resourceStatus().catch(() => []));
      }} />

      {isGgs ? (
        <GgsView view={view} onView={setView} snap={ggs.snap} patch={ggs.patch}
                 onOpenStudy={(kifu) => void openStudy(kifu)} />
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
