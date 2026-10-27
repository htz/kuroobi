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

type ToastTone = 'bad' | 'gold';

interface Toast { id: number; text: string; tone: ToastTone }

const TOAST_MS = 5000;

type AppMode = 'vs' | 'study';
export type EngineSide = 'black' | 'white' | 'both' | 'off';
type MoveSource = 'book' | 'search';

export interface MoveInfo {
  source: MoveSource;
  value: number;
  exact: boolean;
  learned: boolean;
  secs: number;
}

export const LEVELS = [
  { depth: 1, solve: 2 },
  { depth: 2, solve: 4 },
  { depth: 4, solve: 8 },
  { depth: 6, solve: 10 },
  { depth: 8, solve: 12 },
  { depth: 10, solve: 14 },
  { depth: 12, solve: 16 },
  { depth: 14, solve: 18 },
  { depth: 16, solve: 20 },
  { depth: 18, solve: 22 },
  { depth: 20, solve: 24 },
  { depth: 22, solve: 26 },
  { depth: 24, solve: 26 },
] as const;

export interface Levels { depth: number; solve: number }

export const SOLVE_MAX = 36;

export function clampLevels(v: Levels): Levels {
  const depth = Math.max(1, Math.min(SOLVE_MAX, v.depth));
  return { depth, solve: Math.max(depth, Math.min(SOLVE_MAX, v.solve)) };
}

interface Strength { level: number | 'custom'; custom: Levels; useBook: boolean }
const STRENGTH0: Strength = { level: 6, custom: { depth: 12, solve: 18 }, useBook: true };

export type Hints = Record<number, { value: number; exact: boolean; book: boolean; depth: number }>;

export function useGame(clockSecs: number, learnOn: boolean) {
  const [view, setView] = useState<GameView | null>(null);
  const [mode, setMode] = useState<AppMode>('vs');
  const [side, setSide] = useState<EngineSide>('white');
  // Playing and study keep their own strength and book switch: the opponent's strength is not
  // the analysis strength. The values below are the current mode's, so the engine gets them on a switch.
  const [strength, setStrength] = useState<Record<AppMode, Strength>>({ vs: STRENGTH0, study: STRENGTH0 });
  const { level: levelRaw, custom, useBook } = strength[mode];
  const level: number | 'custom' =
    levelRaw === 'custom' ? 'custom' : Math.max(0, Math.min(LEVELS.length - 1, levelRaw));
  const setOwn = useCallback((part: Partial<Strength>) => {
    setStrength((s) => ({ ...s, [mode]: { ...s[mode], ...part } }));
  }, [mode]);
  const setLevel = useCallback((l: number | 'custom') => setOwn({ level: l }), [setOwn]);
  const setCustom = useCallback((c: Levels) => setOwn({ custom: c }), [setOwn]);
  const setUseBook = useCallback((on: boolean) => setOwn({ useBook: on }), [setOwn]);
  const [hasBook, setHasBook] = useState(true);
  // The board's evals are kept per mode too: hidden while playing, shown when studying.
  const [hintOn, setHintOn] = useState<Record<AppMode, boolean>>({ vs: false, study: true });
  const autoHint = hintOn[mode];
  const [hints, setHints] = useState<Hints | null>(null);
  const [stat, setStat] = useState<SearchStat | null>(null);
  const [playing, setPlaying] = useState(false);
  const [thinking, setThinking] = useState(false);
  const [thinkSecs, setThinkSecs] = useState(0);
  const [thinkTotal, setThinkTotal] = useState({ black: 0, white: 0 });
  const [moveSource, setMoveSource] = useState<Record<number, MoveInfo>>({});
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastId = useRef(0);

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
    await api.setLevels(levels.depth, levels.solve).catch(() => {});
  }, [levels.depth, levels.solve]);

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

  // The board's live evals run only while the board is on screen (App says when it is not).
  const [boardShown, setBoardShown] = useState(true);
  const refreshHints = useCallback(async () => {
    if (thinking || !boardShown) return;
    await api.stopSearch().catch(() => {});
    const v = view;
    if (!v || v.over) return;
    if (playing && engineSides().includes(v.player)) return;
    if (!autoHint) return;
    try {
      await pushLevels();
      await api.analyzeLive();
    } catch (e) {
      // During a GGS game the engine is the game's: the evals just stay off, without a message.
      if (String(e) !== 'err.busy_ggs_analysis') say(tErr(e));
    }
  }, [view, autoHint, thinking, boardShown, playing, engineSides, pushLevels, say]);

  const setAutoHint = useCallback((on: boolean) => {
    setHintOn((h) => ({ ...h, [mode]: on }));
    if (!on) { setHints(null); setStat(null); }
  }, [mode]);

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

  // A game paused by a click on a past move goes on once a move is played there (a take-back to
  // that move), so KUROOBI answers it.
  const resume = useRef(false);
  const play = useCallback(async (sq: number) => {
    if (thinking) return;
    if (playing && view && engineSides().includes(view.player)) return;
    try {
      const v = await api.play(sq);
      apply(v);
      maybeLearn(v);
      if (resume.current) { resume.current = false; if (!v.over) setPlaying(true); }
    } catch (e) { say(tErr(e)); }
  }, [thinking, playing, view, engineSides, apply, maybeLearn, say]);

  const [clock, setClock] = useState<ClockView | null>(null);
  // The clock runs only while the game is played; any move in the record moves the times with it.
  const clockOn = clockSecs > 0;
  useEffect(() => {
    if (clockOn) void api.runClock(playing).then(setClock).catch(() => {});
  }, [clockOn, playing]);
  useEffect(() => {
    if (clockOn) void api.clocks().then(setClock).catch(() => {});
  }, [clockOn, view]);
  // The saved clock goes to the engine at start; it holds none until told.
  useEffect(() => {
    void api.setClock(clockSecs).then(setClock).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
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
    api.stopSearch().catch(() => {});
    void pushLevels();
    setMoveSource({});
    setThinkTotal({ black: 0, white: 0 });
    setPlaying(false);
    resume.current = false;
    apply(await api.newGame());
    setClock(await api.setClock(clockSecs).catch(() => null));
  }, [apply, pushLevels, clockSecs]);

  // Choosing a clock before the first move sets it now rather than at the next new game.
  const resetClock = useCallback(async (secs: number) => {
    setClock(await api.setClock(secs).catch(() => null));
  }, []);

  const undo = useCallback(async () => {
    if (thinking) return;
    // Back to before the person's own last move: KUROOBI's replies and passes in between go with it.
    // Each undo takes one placed move (and a pass before it). The side to move is then the one that
    // placed it, unless it has no move: that side had passed, so the other one placed it.
    const human = mode === 'study' ? ['black', 'white']
      : side === 'black' ? ['white'] : side === 'white' ? ['black'] : side === 'off' ? ['black', 'white'] : [];
    // A position whose side to move has no move is never where it stops (that side passes there).
    const resting = (v: GameView) => v.over || v.legal.length > 0;
    const from = view?.move_count ?? 0;
    try {
      let v = await api.undo();
      while (v.move_count > 0 && (!resting(v) || (human.length > 0 && !human.includes(v.player)))) {
        v = await api.undo();
      }
      // Nothing of the person's to take back (KUROOBI moved first): leave the game as it was.
      if (human.length > 0 && !human.includes(v.player)) v = await api.goto(from);
      apply(v);
    } catch (e) { say(tErr(e)); }
  }, [thinking, mode, side, view, apply, say]);

  const jumpTo = useCallback(async (n: number) => {
    if (thinking) return;
    api.stopSearch().catch(() => {});
    // Scrubbing the record pauses the live game; a move played there takes it on from that move.
    if (playing) { setPlaying(false); resume.current = true; }
    try { apply(await api.goto(n)); } catch (e) { say(tErr(e)); }
  }, [thinking, playing, apply, say]);

  const stop = useCallback(() => {
    setPlaying(false);
    resume.current = false;
    api.stopSearch().catch(() => {});
  }, []);

  return {
    view, setView: apply,
    setMode,
    side, setSide,
    level, setLevel, setCustom, levels,
    useBook, setUseBook, hasBook, setHasBook,
    learnOn, maybeLearn,
    autoHint, setAutoHint,
    hints, setHints,
    stat, setStat,
    playing, setPlaying,
    thinking, setThinking,
    thinkSecs, setThinkSecs,
    thinkTotal, setThinkTotal,
    moveSource, setMoveSource,
    toasts, say, dismiss,
    engineSides, pushLevels, refreshHints, setBoardShown,
    play, newGame, undo, jumpTo, stop,
    clockSecs, clock, resetClock,
  };
}

export type Game = ReturnType<typeof useGame>;
