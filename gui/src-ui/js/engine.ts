import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ggsApi, onHints, type ActivityView } from './api';
import type { Game } from './state';
import type { GameView, LearnEntry } from './types';
import { t, tErr } from './i18n';

/* Engine interaction, screen-independent. Extracted from App.tsx so
 * both UIs could share it during the redesign; behavior deliberately
 * unchanged in the move. */

/** Push settings to the engine; depends on values (function deps would
 * rebuild and loop forever). */
export function useEngineSettings(g: Game) {
  const { depth, solve, band } = g.levels;
  useEffect(() => {
    api.setLevels(depth, solve, band).catch(() => {});
  }, [depth, solve, band]);

  useEffect(() => { api.setUseBook(g.useBook).catch(() => {}); }, [g.useBook]);

  // Learning import is one shared setting; push to both sides.
  useEffect(() => {
    api.setLearn(g.learnOn).catch(() => {});
    ggsApi.setLearn(g.learnOn).catch(() => {});
  }, [g.learnOn]);
}

/** Re-evaluate on position changes and receive deepening progress. */
export function useHints(g: Game) {
  const refresh = g.refreshHints;
  useEffect(() => { void refresh(); }, [g.view, g.autoHint, refresh]);

  // Arrives per finished pass; deeper results simply replace.
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

/** CPU activity (always shown in the left nav). */
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

/** The engine's turn: think and play when it comes up. */
export function useEngineTurn(g: Game) {
  const turnRef = useRef(false);
  const {
    playing, view: gv, engineSides, setThinking, setThinkSecs, setThinkTotal,
    setMoveSource, setView: applyView, setPlaying, say, maybeLearn, setStat,
  } = g;

  useEffect(() => {
    if (!playing || !gv) return;
    // Stops itself at game over (no stop button needed).
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
        // Record which move this was (for the table's source/eval
        // columns). next.cursor may sit past a forced pass, so count
        // from the pre-move position; convert to Black's view.
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
        // Set the workload after advancing (applyView clears it); the
        // last move's numbers linger until the human plays.
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

/** One eval-graph point. `exact`/`book` are required — every measured
 *  point knows both, and optionality would clash with Graph.tsx. */
export type GraphPoint = { value: number; exact: boolean; book: boolean };

/** Line fingerprint; only equality matters (a changed line re-measures). */
const lineKey = (v: GameView | null) =>
  v ? v.moves.map((m) => (m == null ? 'p' : m)).join(',') : '';

/** Yes/no prompt; defaults to the browser dialog. engine.ts owns no
 *  UI, so screens wanting styled dialogs pass their own. */
/* Confirmation shape: the title states what will happen, and `ok` is
 * named after the action itself — "Confirm / OK?" tells the user
 * nothing before clicking. */
export interface AskArgs { title: string; body: string; ok: string; danger?: boolean }
export type Ask = (a: AskArgs) => boolean | Promise<boolean>;
const askDefault: Ask = (a) => window.confirm(a.title + '\n' + a.body);

/** Eval graph: measures every position. `ggsMatch` = our GGS game is
 *  live (GGS outranks analysis); passed in since the screen owns it. */
export function useGraph(g: Game, ggsMatch: boolean, ask: Ask = askDefault) {
  const [values, setValues] = useState<(GraphPoint | undefined)[] | null>(null);
  const [busy, setBusy] = useState(false);
  /** Analysis progress (measured / total); null while idle. */
  const [prog, setProg] = useState<{ done: number; total: number } | null>(null);
  const seqRef = useRef(0);
  const keyRef = useRef('');

  // A changed line invalidates the graph.
  useEffect(() => {
    const k = lineKey(g.view);
    if (k !== keyRef.current) { keyRef.current = k; setValues(null); }
  }, [g.view]);

  const update = useCallback(async () => {
    const v = g.view;
    // Never a dead button: always state why it cannot start.
    if (!v) { g.say(t('engine.toast.no_record'), 'gold'); return; }
    // No double runs. The button reads "stop" while running so humans
    // cannot hit this; only the auto-start path can.
    if (busy) return;
    // CPU-hungry features never overlap: refuse during GGS games,
    // confirm-then-stop a local game.
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
    // Every press re-measures everything: reusing results would make
    // a full graph do nothing, and strength changes could never
    // re-measure.
    const vals: (GraphPoint | undefined)[] = new Array(len + 1);
    keyRef.current = lineKey(v);
    setValues(null);
    let failed = false;
    await g.pushLevels();
    // Modest depth — every position gets measured.
    const depth = Math.min(g.levels.depth, 14);
    // Measure backwards from the end: endgame positions solve
    // instantly, so the graph fills from the right — and the midgame
    // table persists across positions, letting earlier searches reuse
    // the just-measured later entries.
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
    // Keep the failure reason; clearing it here looks like a dead button.
    if (!failed && seq === seqRef.current) g.say('');
    // A new generation means another analysis started; clearing here
    // would kill its running indicator.
    if (seq === seqRef.current) { setBusy(false); setProg(null); }
  }, [g, busy, ggsMatch, ask]);

  /// Stop the analysis. Restarting re-measures everything (by design),
  /// so this is "stop", not "pause"; finished points stay on the graph.
  const stop = useCallback(() => {
    seqRef.current++;
    setBusy(false);
    setProg(null);
    void api.stopSearch();
    g.say('');
  }, [g]);

  return { values, busy, prog, update, stop };
}

/** Game start/stop; CPU-hungry features never overlap. */
export function useStartGame(
  g: Game,
  ggsMatch: boolean,
  graph: { busy: boolean; stop: () => void },
  ask: Ask = askDefault,
) {
  return useCallback(async () => {
    // No toast for stopping — the user did it and the button reverts.
    if (g.playing) { g.stop(); g.say(''); return; }
    // GGS outranks; a running analysis stops after confirmation.
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

/* Imported-game log. Re-read only on open and when learning finishes;
 * polling would touch the file even with nothing imported. */
export function useLearnLog(on: boolean, learning: boolean) {
  const [items, setItems] = useState<LearnEntry[]>([]);
  // Keep the previous value to catch "learning just finished".
  const wasLearning = useRef(false);
  const reload = useCallback(() => {
    void api.learnLog().then(setItems).catch(() => {});
  }, []);
  useEffect(() => {
    const ended = wasLearning.current && !learning;
    wasLearning.current = learning;
    if (on && (ended || items.length === 0)) reload();
    // items in the deps would loop forever on empty results.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [on, learning, reload]);
  return { items, reload };
}
