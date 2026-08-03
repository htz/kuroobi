import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ggsApi, jsLog, type ActivityView } from './api';
import { useGame } from './state';
import { useGgs } from './ggs';
import type { GameView } from './types';
import { Board } from './components/Board';
import { GgsView } from './components/GgsView';
import { Graph, type GraphPoint } from './components/Graph';
import { Nav, type View } from './components/Nav';
import { Panel, ScoreBar } from './components/Panel';
import { SettingsModal } from './components/SettingsModal';
import { Modal } from './components/Modal';
import { Icon } from './components/Icons';

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
  // 学習の取り込みはローカル・GGS 共通の設定 (歯車)。両方へ流す
  useEffect(() => {
    api.setLearn(g.learnOn).catch(() => {});
    ggsApi.setLearn(g.learnOn).catch(() => {});
  }, [g.learnOn]);

  // 局面が動いたら評価値を出し直す
  const refresh = g.refreshHints;
  useEffect(() => { void refresh(); }, [g.view, g.autoHint, refresh]);

  // ---- CPU の稼働状況 (ナビの常時表示) ----
  const [cpu, setCpu] = useState<ActivityView | null>(null);
  useEffect(() => {
    const t = window.setInterval(() => {
      api.activity().then(setCpu).catch(() => {});
    }, 1000);
    return () => clearInterval(t);
  }, []);
  // GGS の自分の対局が進行中か (最優先なのでローカルの開始を断る)
  const ggsMatch = ggs.snap?.matches.some((m) => m.my_color && !m.over) ?? false;

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


  // 動作確認用: KUROOBI_AUTOPLAY=both:11 のように指定すると自動で対局を始める。
  // "study" なら適当な棋譜を読み込んで検討画面を開き、"settings" なら
  // 設定 (歯車) を開く (どちらも見た目の確認用)。
  const started = useRef(false);
  const { setPlaying: begin, setSide, setLevel, setAutoHint,
          playing, view: gv, engineSides, setThinking, setThinkSecs, setThinkTotal,
          setMoveSource, setView: applyView, setPlaying, say, setLastEval, setMode } = g;
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void api.autoplay().then(async (v) => {
      if (!v) return;
      const [who, lv] = v.split(':');
      if (who === 'settings') {
        setSettings(await api.resourceStatus().catch(() => []));
        return;
      }
      if (who === 'study') {
        // 起動時の状態取得と前後すると初期局面で上書きされるので一拍おく
        await new Promise((r) => setTimeout(r, 500));
        setView('study');
        setMode('study');
        applyView(await api.loadKifuText(
          'e6f4c3d6f6e7f5g5e3g4c7d3f3c4c6c5b4b6d7b5c2a3f8e8d8c8b8d2g3e2'));
        // "study:hint" は序盤 (定石内) の局面で評価値表示まで自動で入れる
        if (lv === 'hint') {
          applyView(await api.goto(8));
          setAutoHint(true);
        }
        return;
      }
      if (who === 'both') setSide('both');
      if (lv !== undefined && Number.isFinite(+lv)) setLevel(+lv);
      begin(true);
    }).catch((e) => jsLog('autoplay: ' + e));
  }, [begin, setSide, setLevel, setMode, applyView, setAutoHint]);

  // ---- エンジンの手番 ----
  // 局面が「エンジンが打つ番」になったら 1 度だけ走る。二重起動を
  // 防ぐため、走っている間は ref で塞ぐ。
  const turnRef = useRef(false);
  const maybeLearn = g.maybeLearn;
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
        // 何手目の手だったかを記録する (棋譜の表に出所と評価を出すため)。
        // next.cursor は強制パスの先まで進んでいることがあるので、
        // 指す前の局面から数える。値は手番視点なので黒視点へ揃える
        setMoveSource((m) => ({
          ...m,
          [gv.cursor + 1]: {
            source: r.from_book ? 'book' : 'search',
            value: side === 'white' ? -r.value : r.value,
            exact: r.exact,
            learned: r.learned,
            secs: r.secs,
          },
        }));
        applyView(next);
        maybeLearn(next);
        // どのくらい良いと見て指したかを残す
        setLastEval(Number.isFinite(r.value)
          ? `KUROOBI の評価: ${r.value > 0 ? '+' : ''}${
              r.exact ? r.value.toFixed(0) : r.value.toFixed(1)} 石`
            + (r.exact ? ' (完全読み)'
              : r.from_book && r.learned ? ' (定石·実戦学習)'
              : r.from_book ? ' (定石)' : '')
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
      setMoveSource, applyView, setPlaying, say, setLastEval, maybeLearn]);

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
    // 押しても何も起きない、という状態を作らない。始められない理由は必ず出す
    if (!v) { g.say('棋譜がありません'); return; }
    if (gbusy) { g.say('分析中です'); return; }
    // CPU を食い合う機能は同時に動かさない。GGS 対局は最優先なので断り、
    // ローカル対局が進行中なら確認の上で停止してから始める
    if (ggsMatch) { g.say('GGS 対局中は分析を控えます'); return; }
    if (g.playing || g.thinking) {
      if (!window.confirm('対局が進行中です。停止して分析しますか？')) return;
      g.stop();
    }
    setGbusy(true);
    const seq = ++gseq.current;
    const key = lineKey(v);
    const len = v.moves.length;
    // 押されたら必ず全局面を測り直す。前の結果を引き継ぐと、埋まっている
    // ときに 1 局面も動かず「押しても何も起きない」ことになる。強さを
    // 変えた後に測り直せないのも困る
    const vals: (GraphPoint | undefined)[] = new Array(len + 1);
    gkey.current = key;
    setGvals(null);
    let failed = false;
    await g.pushLevels();
    // 全局面を測るので深さは控えめに
    const depth = Math.min(g.levels.depth, 14);
    for (let n = 0; n <= len; n++) {
      if (seq !== gseq.current) break;
      if (vals[n]) continue;
      if (n < len && v.moves[n] == null) continue;   // パスの手番は測らない
      g.say(`分析中 ${n}/${len}…`, true);
      try {
        const p = await api.evalAt(n, depth);
        if (seq !== gseq.current) break;
        if (Number.isFinite(p.value)) vals[n] = { value: p.value, exact: p.exact, book: p.from_book };
        setGvals([...vals]);
      } catch (e) { g.say('' + e); failed = true; break; }
    }
    // 失敗したときは理由を残す。ここで消すと「押しても何も起きない」ように見える
    if (!failed && seq === gseq.current) g.say('');
    setGbusy(false);
  }, [g, gbusy, ggsMatch]);

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
        // 手合いの「組」数。同期対局は 2 局で 1 組なので base でまとめる
        'ggs-play': new Set(ggs.snap?.matches.map((m) => m.base || m.id) ?? []).size,
        'ggs-chat': chatUnread,
      }} cpu={cpu} onSettings={async () => {
        setSettings(await api.resourceStatus().catch(() => []));
      }} />

      {isGgs ? (
        <GgsView view={view} onView={setView} snap={ggs.snap} patch={ggs.patch}
                 onOpenStudy={(kifu) => void openStudy(kifu)} />
      ) : (
        <>
          <div id="main">
            <ScoreBar g={g} />
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
                    評価値グラフ (黒視点) — <span style={{ color: 'var(--gold)' }}>●</span>定石{' '}
                    <span style={{ color: 'var(--text)' }}>●</span>読切{' '}
                    <span style={{ color: 'var(--accent)' }}>●</span>探索
                  </label>
                  <button className="btn small" onClick={() => void updateGraph()}>
                    <Icon name="refresh" size={14} />分析
                  </button>
                </div>
                <Graph values={gvals} moves={g.view.moves} cursor={g.view.cursor}
                       busy={gbusy} onJump={(n) => void g.jumpTo(n)} />
              </div>
            )}
          </div>

          <Panel g={g} gvals={gvals}
                 onStart={() => {
                   if (g.playing) { g.stop(); g.say('対局を停止しました'); return; }
                   // CPU を食い合う機能は同時に動かさない。GGS 対局は最優先、
                   // 分析中は確認してから止める
                   if (ggsMatch) { g.say('GGS 対局中はローカル対局を開始できません'); return; }
                   if (gbusy) {
                     if (!window.confirm('評価値グラフを分析中です。停止して対局を始めますか？')) return;
                     gseq.current++;
                     setGbusy(false);
                     void api.stopSearch();
                   }
                   g.setPlaying(true);
                   g.say('');
                 }}
                 onSave={() => void api.saveKifu().catch((e) => g.say('' + e))}
                 onLoad={() => setPaste(true)} />
        </>
      )}

      {paste && (
        <Modal title="棋譜の読み込み" onClose={() => setPaste(false)}
               subtitle="GGF・f5d6… 形式・盤面つきのいずれでも"
               actions={<>
                 <button className="btn" onClick={async () => {
                   try {
                     const v = await api.loadKifu();
                     if (v) { g.setView(v); setPaste(false); }
                   } catch (e) { g.say('' + e); }
                 }}>ファイルから…</button>
                 <span className="spacer" />
                 <button className="btn ghost" onClick={() => setPaste(false)}>キャンセル</button>
                 <button className="btn primary" onClick={() => void loadText(pasteText)}>
                   読み込む
                 </button>
               </>}>
          <textarea value={pasteText} onChange={(e) => setPasteText(e.target.value)}
                    placeholder="f5d6c3d3c4f4f3e3e2…" />
        </Modal>
      )}

      {settings && (
        <SettingsModal initial={settings} learnOn={g.learnOn} onLearn={g.setLearnOn}
                       onClose={() => setSettings(null)}
                       onChanged={() => { void api.hasBook().then(g.setHasBook); }} />
      )}
    </>
  );
}
