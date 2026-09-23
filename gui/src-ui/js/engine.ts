import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ggsApi, onHints, type ActivityView } from './api';
import type { Game } from './state';
import type { GameView, LearnEntry } from './types';
import { t, tErr } from './i18n';


export function useEngineSettings(g: Game) {
  const { depth, solve, band } = g.levels;
  useEffect(() => {
    api.setLevels(depth, solve, band).catch(() => {});
  }, [depth, solve, band]);

  useEffect(() => { api.setUseBook(g.useBook).catch(() => {}); }, [g.useBook]);

  useEffect(() => {
    api.setLearn(g.learnOn).catch(() => {});
    ggsApi.setLearn(g.learnOn).catch(() => {});
  }, [g.learnOn]);
}

export function useHints(g: Game) {
  const refresh = g.refreshHints;
  useEffect(() => { void refresh(); }, [g.view, g.autoHint, refresh]);

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

export function useActivity(): ActivityView | null {
  const [cpu, setCpu] = useState<ActivityView | null>(null);
  useEffect(() => {
    const id = window.setInterval(() => {
      api.activity().then(setCpu).catch(() => {});
    }, 1000);
    return () => clearInterval(id);
  }, []);
  return cpu;
}

export function useEngineTurn(g: Game) {
  const turnRef = useRef(false);
  const {
    playing, view: gv, engineSides, setThinking, setThinkSecs, setThinkTotal,
    setMoveSource, setView: applyView, setPlaying, say, maybeLearn, setStat,
  } = g;

  useEffect(() => {
    if (!playing || !gv) return;
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
        setThinkTotal((tot) => ({ ...tot, [side]: tot[side] + r.secs }));
        const next = await api.applyMove(r.pos);
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
        setStat(r.nodes > 0 ? { nodes: r.nodes, secs: r.secs } : null);
        say('');
      } catch (e) {
        say(tErr(e));
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
      setMoveSource, applyView, setPlaying, say, maybeLearn, setStat]);
}

export type GraphPoint = { value: number; exact: boolean; book: boolean };

const lineKey = (v: GameView | null) =>
  v ? v.moves.map((m) => (m == null ? 'p' : m)).join(',') : '';

export interface AskArgs { title: string; body: string; ok: string; danger?: boolean }
export type Ask = (a: AskArgs) => boolean | Promise<boolean>;
const askDefault: Ask = (a) => window.confirm(a.title + '\n' + a.body);

export function useGraph(g: Game, ggsMatch: boolean, ask: Ask = askDefault) {
  const [values, setValues] = useState<(GraphPoint | undefined)[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [prog, setProg] = useState<{ done: number; total: number } | null>(null);
  const seqRef = useRef(0);
  const keyRef = useRef('');

  useEffect(() => {
    const k = lineKey(g.view);
    if (k !== keyRef.current) { keyRef.current = k; setValues(null); }
  }, [g.view]);

  const update = useCallback(async () => {
    const v = g.view;
    if (!v) { g.say(t('engine.toast.no_record'), 'gold'); return; }
    if (busy) return;
    if (ggsMatch) { g.say(t('engine.toast.no_analysis_during_ggs'), 'gold'); return; }
    if (g.playing || g.thinking) {
      if (!await ask({
        title: t('engine.ask.stop_game_title'),
        body: t('engine.ask.stop_game_body'),
        ok: t('engine.ask.stop_game_ok'),
      })) return;
      g.stop();
    }
    setBusy(true);
    const seq = ++seqRef.current;
    const len = v.moves.length;
    const vals: (GraphPoint | undefined)[] = new Array(len + 1);
    keyRef.current = lineKey(v);
    setValues(null);
    let failed = false;
    await g.pushLevels();
    const depth = Math.min(g.levels.depth, 14);
    for (let n = len; n >= 0; n--) {
      if (seq !== seqRef.current) break;
      if (vals[n]) continue;
      if (n < len && v.moves[n] == null) continue;   // pass turns are not measured
      setProg({ done: len - n + 1, total: len + 1 });
      try {
        const p = await api.evalAt(n, depth);
        if (seq !== seqRef.current) break;
        if (Number.isFinite(p.value)) vals[n] = { value: p.value, exact: p.exact, book: p.from_book };
        setValues([...vals]);
      } catch (e) { g.say(tErr(e)); failed = true; break; }
    }
    if (!failed && seq === seqRef.current) g.say('');
    if (seq === seqRef.current) { setBusy(false); setProg(null); }
  }, [g, busy, ggsMatch, ask]);

  const stop = useCallback(() => {
    seqRef.current++;
    setBusy(false);
    setProg(null);
    void api.stopSearch();
    g.say('');
  }, [g]);

  return { values, busy, prog, update, stop };
}

export function useStartGame(
  g: Game,
  ggsMatch: boolean,
  graph: { busy: boolean; stop: () => void },
  ask: Ask = askDefault,
) {
  return useCallback(async () => {
    if (g.playing) { g.stop(); g.say(''); return; }
    if (ggsMatch) { g.say(t('engine.toast.no_local_during_ggs'), 'gold'); return; }
    if (graph.busy) {
      if (!await ask({
        title: t('engine.ask.stop_analysis_title'),
        body: t('engine.ask.stop_analysis_body'),
        ok: t('engine.ask.stop_analysis_ok'),
      })) return;
      graph.stop();
    }
    g.setPlaying(true);
    g.say('');
  }, [g, ggsMatch, graph, ask]);
}

export function useLearnLog(on: boolean, learning: boolean) {
  const [items, setItems] = useState<LearnEntry[]>([]);
  const wasLearning = useRef(false);
  const reload = useCallback(() => {
    void api.learnLog().then(setItems).catch(() => {});
  }, []);
  useEffect(() => {
    const ended = wasLearning.current && !learning;
    wasLearning.current = learning;
    if (on && (ended || items.length === 0)) reload();
  }, [on, learning, reload]);
  return { items, reload };
}
