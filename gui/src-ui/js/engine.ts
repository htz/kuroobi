import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ggsApi, onHints, type ActivityView } from './api';
import type { Game } from './state';
import type { GameView } from './types';

/* エンジンとのやりとり。画面には依存しない。
 *
 * もとは App.tsx に描画と同居していた。新しいデザインへ載せ替える間は
 * 画面が 2 つ並ぶので、同居したままだと同じ手順を 2 か所に書くことになる。
 * ここに集めて両方から使う。**移すときに挙動は変えていない** — 変えると、
 * 見え方の違いがデザインのせいか手順のせいか分からなくなる。
 */

/** 設定をエンジンへ流す。値に依存させる (関数を並べると毎回作り直されて無限に走る)。 */
export function useEngineSettings(g: Game) {
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
}

/** 局面が動いたら評価値を出し直す + 反復深化の途中経過を受ける。 */
export function useHints(g: Game) {
  const refresh = g.refreshHints;
  useEffect(() => { void refresh(); }, [g.view, g.autoHint, refresh]);

  // 反復深化の段が終わるたびに届く。深いものが来たら置き換えるだけ。
  const setHints = g.setHints;
  const setStat = g.setStat;
  useEffect(() => {
    let off: (() => void) | undefined;
    void onHints((_depth, hs, nodes, secs) => {
      const next: Record<number, { value: number; exact: boolean; book: boolean; depth: number }> = {};
      for (const h of hs) {
        if (!Number.isFinite(h.value)) continue;
        next[h.pos] = { value: h.value, exact: h.exact, book: h.from_book, depth: h.depth };
      }
      setHints(Object.keys(next).length ? next : null);
      setStat({ nodes, secs });
    }).then((f) => { off = f; }).catch(() => {});
    return () => off?.();
  }, [setHints, setStat]);
}

/** CPU の稼働状況 (左メニューに常時出す)。 */
export function useActivity(): ActivityView | null {
  const [cpu, setCpu] = useState<ActivityView | null>(null);
  useEffect(() => {
    const t = window.setInterval(() => {
      api.activity().then(setCpu).catch(() => {});
    }, 1000);
    return () => clearInterval(t);
  }, []);
  return cpu;
}

/** エンジンの手番。打つ番になったら考えて指す。 */
export function useEngineTurn(g: Game) {
  const turnRef = useRef(false);
  const {
    playing, view: gv, engineSides, setThinking, setThinkSecs, setThinkTotal,
    setMoveSource, setView: applyView, setPlaying, say, setLastEval, maybeLearn, setStat,
  } = g;

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
        // 働きぶりは局面を進めた後に立てる (applyView が消しにかかるため)。
        // 人が打つまで直前の 1 手ぶんが盤の下に残る
        setStat(r.nodes > 0 ? { nodes: r.nodes, secs: r.secs } : null);
        // どのくらい良いと見て指したかを残す。
        // 見出し (「KUROOBI の評価」) は付けない。狭い幅では見出しだけ畳んで
        // 数字を残したいので、出し分けは表示側の仕事にする
        setLastEval(Number.isFinite(r.value)
          ? `${r.value > 0 ? '+' : ''}${
              r.exact ? r.value.toFixed(0) : r.value.toFixed(1)} 石`
            + (r.exact ? ' (完全読み)'
              : r.from_book && r.learned ? ' (定石·実戦学習)'
              : r.from_book ? ' (定石)' : '')
          : '');
        say('');
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
      setMoveSource, applyView, setPlaying, say, setLastEval, maybeLearn, setStat]);
}

/** 評価値グラフの 1 点。
 *  `exact` / `book` は必須にする — 測れた点は必ずどちらか分かるし、
 *  省略可にすると受け取る側 (Graph.tsx) の型と食い違う。 */
export type GraphPoint = { value: number; exact: boolean; book: boolean };

/** 手順の指紋。手順が変わったら測り直しになるので、同じかどうかだけ見る。 */
const lineKey = (v: GameView | null) =>
  v ? v.moves.map((m) => (m == null ? 'p' : m)).join(',') : '';

/** 「はい / いいえ」を聞く。既定はブラウザのダイアログ。
 *  engine.ts は画面を持たない約束なので、見た目のあるものを出したい画面は
 *  自前の確認を渡す (v2 は Overlay の中のモーダル)。 */
export type Ask = (message: string) => boolean | Promise<boolean>;
const askDefault: Ask = (m) => window.confirm(m);

/** 評価値グラフ。全局面を測る。
 *  `ggsMatch` は GGS の自分の対局が進行中か — GGS は最優先なので分析を断る。
 *  GGS の状態は画面側が持つので引数で渡す。 */
export function useGraph(g: Game, ggsMatch: boolean, ask: Ask = askDefault) {
  const [values, setValues] = useState<(GraphPoint | undefined)[] | null>(null);
  const [busy, setBusy] = useState(false);
  /** 分析の進み具合 (測った局面 / 全局面)。走っていない間は null。 */
  const [prog, setProg] = useState<{ done: number; total: number } | null>(null);
  const seqRef = useRef(0);
  const keyRef = useRef('');

  // 手順が変わったらグラフは無効
  useEffect(() => {
    const k = lineKey(g.view);
    if (k !== keyRef.current) { keyRef.current = k; setValues(null); }
  }, [g.view]);

  const update = useCallback(async () => {
    const v = g.view;
    // 押しても何も起きない、という状態を作らない。始められない理由は必ず出す
    if (!v) { g.say('棋譜がありません', 'gold'); return; }
    // 二重に走らせない。ボタンは走っている間「分析停止」に変わるので人には
    // 踏めず、言葉にする相手がいない (踏めるのは自動起動の経路だけ)
    if (busy) return;
    // CPU を食い合う機能は同時に動かさない。GGS 対局は最優先なので断り、
    // ローカル対局が進行中なら確認の上で停止してから始める
    if (ggsMatch) { g.say('GGS 対局中は分析を控えます', 'gold'); return; }
    if (g.playing || g.thinking) {
      if (!await ask('対局が進行中です。停止して分析しますか？')) return;
      g.stop();
    }
    setBusy(true);
    const seq = ++seqRef.current;
    const len = v.moves.length;
    // 押されたら必ず全局面を測り直す。前の結果を引き継ぐと、埋まっている
    // ときに 1 局面も動かず「押しても何も起きない」ことになる。強さを
    // 変えた後に測り直せないのも困る
    const vals: (GraphPoint | undefined)[] = new Array(len + 1);
    keyRef.current = lineKey(v);
    setValues(null);
    let failed = false;
    await g.pushLevels();
    // 全局面を測るので深さは控えめに
    const depth = Math.min(g.levels.depth, 14);
    // 終局から逆向きに測る。終盤ほど空きが少なく読み切りで即確定するので
    // 先に片づき、グラフは右から埋まっていく。加えて中盤の置換表は局面を
    // またいで残るので (消しているのは終盤表だけ)、手前の局面の探索が
    // いま測ったばかりの先の局面のエントリをそのまま通れる。
    for (let n = len; n >= 0; n--) {
      if (seq !== seqRef.current) break;
      if (vals[n]) continue;
      if (n < len && v.moves[n] == null) continue;   // パスの手番は測らない
      setProg({ done: len - n + 1, total: len + 1 });
      try {
        const p = await api.evalAt(n, depth);
        if (seq !== seqRef.current) break;
        if (Number.isFinite(p.value)) vals[n] = { value: p.value, exact: p.exact, book: p.from_book };
        setValues([...vals]);
      } catch (e) { g.say('' + e); failed = true; break; }
    }
    // 失敗したときは理由を残す。ここで消すと「押しても何も起きない」ように見える
    if (!failed && seq === seqRef.current) g.say('');
    // 世代が変わっている = 止められて次の分析が始まっている。ここで false に
    // すると、始まったばかりの分析の「動いている」印を消してしまう
    if (seq === seqRef.current) { setBusy(false); setProg(null); }
  }, [g, busy, ggsMatch, ask]);

  /// 分析の停止。押し直しても続きからにはならない (毎回すべて測り直す作りで、
  /// 前の結果を引き継ぐと埋まっているときに 1 局面も動かなくなる) ので、
  /// 「中断」ではなく「停止」と呼ぶ。測り終えたぶんはグラフに残す。
  const stop = useCallback(() => {
    seqRef.current++;
    setBusy(false);
    setProg(null);
    void api.stopSearch();
    g.say('');
  }, [g]);

  return { values, busy, prog, update, stop };
}

/** 対局の開始 / 停止。CPU を食い合う機能は同時に動かさない。 */
export function useStartGame(
  g: Game,
  ggsMatch: boolean,
  graph: { busy: boolean; stop: () => void },
  ask: Ask = askDefault,
) {
  return useCallback(async () => {
    // 停止したことは伝えない。押した本人がやったことで、ボタンも
    // 「対局開始」に戻るので、言葉で言い直す意味がない
    if (g.playing) { g.stop(); g.say(''); return; }
    // GGS 対局は最優先、分析中は確認してから止める
    if (ggsMatch) { g.say('GGS 対局中はローカル対局を開始できません', 'gold'); return; }
    if (graph.busy) {
      if (!await ask('評価値グラフを分析中です。停止して対局を始めますか？')) return;
      graph.stop();
    }
    g.setPlaying(true);
    g.say('');
  }, [g, ggsMatch, graph, ask]);
}
