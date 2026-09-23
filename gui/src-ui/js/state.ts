import { useCallback, useEffect, useRef, useState } from 'react';
import { api, jsLog } from './api';
import type { ClockView, GameView, SearchStat } from './types';
import { t, tErr } from './i18n';

const INTERNAL = new Set([
  'stopped', 'position changed', 'out of range',
  'InvalidPosition', 'NotPlayable', 'Occupied', 'NoMoves', 'GameOver',
]);

const INTERNAL_PREFIX = [
  'move index ',
  'hash does not match',
];

export type ToastTone = 'bad' | 'gold';

export interface Toast { id: number; text: string; tone: ToastTone }

const TOAST_MS = 5000;

export type AppMode = 'vs' | 'study';
export type EngineSide = 'black' | 'white' | 'both' | 'off';
export type MoveSource = 'book' | 'search';

export interface MoveInfo {
  source: MoveSource;
  value: number;
  exact: boolean;
  learned: boolean;
  secs: number;
}

export const LEVELS = [
  { name: 'Lv1', depth: 1, solve: 2, band: 0 },
  { name: 'Lv2', depth: 2, solve: 4, band: 0 },
  { name: 'Lv3', depth: 4, solve: 8, band: 0 },
  { name: 'Lv4', depth: 6, solve: 10, band: 0 },
  { name: 'Lv5', depth: 8, solve: 12, band: 0 },
  { name: 'Lv6', depth: 10, solve: 14, band: 0 },
  { name: 'Lv7', depth: 12, solve: 16, band: 0 },
  { name: 'Lv8', depth: 14, solve: 18, band: 0 },
  { name: 'Lv9', depth: 16, solve: 20, band: 0 },
  { name: 'Lv10', depth: 18, solve: 22, band: 6 },
  { name: 'Lv11', depth: 20, solve: 24, band: 6 },
  { name: 'Lv12', depth: 22, solve: 26, band: 6 },
  { name: 'Lv13', depth: 24, solve: 26, band: 8 },
] as const;

export interface Levels { depth: number; solve: number; band: number }

export const SOLVE_MAX = 36;

export function clampLevels(v: Levels): Levels {
  const depth = Math.max(1, Math.min(SOLVE_MAX, v.depth));
  return { depth, solve: Math.max(depth, Math.min(SOLVE_MAX, v.solve)), band: v.band };
}


export interface Hints {
  [sq: number]: { value: number; exact: boolean; book: boolean; depth: number };
}

export function useGame(clockSecs = 0) {
  const [view, setView] = useState<GameView | null>(null);
  const [mode, setMode] = useState<AppMode>('vs');
  const [side, setSide] = useState<EngineSide>('white');
  const [levelRaw, setLevel] = useState<number | 'custom'>(6);
  const level: number | 'custom' =
    levelRaw === 'custom' ? 'custom' : Math.max(0, Math.min(LEVELS.length - 1, levelRaw));
  const [custom, setCustom] = useState<Levels>({ depth: 12, solve: 18, band: 0 });
  const [useBook, setUseBook] = useState(true);
  const [hasBook, setHasBook] = useState(true);
  const [learnOn, setLearnOn] = useState(true);
  const [autoHint, setAutoHintRaw] = useState(false);
  const [hints, setHints] = useState<Hints | null>(null);
  const [stat, setStat] = useState<SearchStat | null>(null);
  const [playing, setPlaying] = useState(false);
  const [thinking, setThinking] = useState(false);
  const [thinkSecs, setThinkSecs] = useState(0);          // elapsed while thinking
  const [thinkTotal, setThinkTotal] = useState({ black: 0, white: 0 });
  const [moveSource, setMoveSource] = useState<Record<number, MoveInfo>>({});
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastId = useRef(0);

  const hintSeq = useRef(0);

  const levels: Levels = level === 'custom' ? custom : LEVELS[level];

  const engineSides = useCallback((): string[] => {
    if (mode === 'study') return [];      // the engine never plays in study
    if (side === 'both') return ['black', 'white'];
    if (side === 'off') return [];        // both sides are human
    return [side];
  }, [mode, side]);

  const dismiss = useCallback((id: number) => {
    setToasts((list) => list.filter((x) => x.id !== id));
  }, []);

  const say = useCallback((s: string, tone: ToastTone = 'bad') => {
    if (!s) return;
    if (INTERNAL.has(s) || INTERNAL_PREFIX.some((p) => s.startsWith(p))) {
      jsLog('internal code (never shown on screen): ' + s);
      return;
    }
    const id = ++toastId.current;
    setToasts((list) => [...list.filter((x) => x.text !== s), { id, text: s, tone }]);
    window.setTimeout(() => dismiss(id), TOAST_MS);
  }, [dismiss]);

  const pushLevels = useCallback(async () => {
    await api.setLevels(levels.depth, levels.solve, levels.band).catch(() => {});
  }, [levels.depth, levels.solve, levels.band]);

  useEffect(() => {
    void (async () => {
      try {
        setView(await api.state());
        setHasBook(await api.hasBook());
      } catch (e) {
        say(tErr(e));
      }
    })();
  }, [say]);

  const refreshHints = useCallback(async (v: GameView | null = view) => {
    if (thinking) return;
    await api.stopSearch().catch(() => {});
    if (!v || v.over) return;
    if (playing && engineSides().includes(v.player)) return;   // the engine's turn
    if (!autoHint) {
      if (playing) api.ponderLive().catch(() => {});
      return;
    }
    hintSeq.current++;
    try {
      await pushLevels();
      await api.analyzeLive();
    } catch (e) {
      say(tErr(e));
    }
  }, [view, autoHint, thinking, playing, engineSides, pushLevels, say]);

  const setAutoHint = useCallback((on: boolean) => {
    setAutoHintRaw(on);
    if (!on) { setHints(null); setStat(null); }
  }, []);

  const apply = useCallback((v: GameView) => {
    setView(v);
    setHints(null);
    setStat(null);
  }, []);

  const maybeLearn = useCallback((v: GameView) => {
    if (!v.over || mode !== 'vs' || !learnOn) return;
    const mine = side === 'black' ? 'w' : side === 'white' ? 'b' : '';
    api.learnGame(mine).catch(() => {});
  }, [mode, learnOn, side]);

  const play = useCallback(async (sq: number) => {
    if (thinking) return;
    if (playing && view && engineSides().includes(view.player)) return;
    try {
      const v = await api.play(sq);
      apply(v);
      maybeLearn(v);
    } catch (e) { say(tErr(e)); }
  }, [thinking, playing, view, engineSides, apply, maybeLearn, say]);

  const [clock, setClock] = useState<ClockView | null>(null);
  useEffect(() => {
    if (!clockSecs || !playing) return;
    const id = setInterval(() => {
      void api.clocks().then((c) => {
        setClock(c);
        if (c.lost) {
          setPlaying(false);
          say(c.lost === 'black'
            ? t('engine.toast.black_flagged') : t('engine.toast.white_flagged'), 'gold');
        }
      }).catch(() => {});
    }, 1000);
    return () => clearInterval(id);
  }, [clockSecs, playing, say]);

  const newGame = useCallback(async () => {
    hintSeq.current++;
    api.stopSearch().catch(() => {});
    void pushLevels();
    setMoveSource({});
    setThinkTotal({ black: 0, white: 0 });
    setPlaying(false);
    apply(await api.newGame());
    setClock(await api.setClock(clockSecs).catch(() => null));
    say('');
  }, [apply, pushLevels, say, clockSecs]);

  const undo = useCallback(async () => {
    if (thinking) return;
    hintSeq.current++;
    try { apply(await api.undo()); } catch (e) { say(tErr(e)); }
  }, [thinking, apply, say]);

  const jumpTo = useCallback(async (n: number) => {
    if (thinking) return;
    hintSeq.current++;
    api.stopSearch().catch(() => {});
    if (playing) setPlaying(false);   // scrubbing the record ends the live game
    try { apply(await api.goto(n)); } catch (e) { say(tErr(e)); }
  }, [thinking, playing, apply, say]);

  const stop = useCallback(() => {
    setPlaying(false);
    api.stopSearch().catch(() => {});
  }, []);

  return {
    view, setView: apply,
    mode, setMode,
    side, setSide,
    level, setLevel, custom, setCustom, levels,
    useBook, setUseBook, hasBook, setHasBook,
    learnOn, setLearnOn, maybeLearn,
    autoHint, setAutoHint,
    hints, setHints,
    stat, setStat,
    playing, setPlaying,
    thinking, setThinking,
    thinkSecs, setThinkSecs,
    thinkTotal, setThinkTotal,
    moveSource, setMoveSource,
    toasts, say, dismiss,
    engineSides, pushLevels, refreshHints,
    play, newGame, undo, jumpTo, stop,
    clockSecs, clock,
    hintSeq,
  };
}

export type Game = ReturnType<typeof useGame>;
