// Game state and engine interaction; screens only read from here.
//
// This is why hand-written DOM updates were abandoned: manual
// state/display sync kept dropping things (vanishing options, blank
// ratings, dead hidden flags). One source of truth, screens derive.
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, jsLog } from './api';
import type { ClockView, GameView, SearchStat } from './types';

/// Internal engine codes, never displayed: each is the result of the
/// user's own action (`stopped` after stop, `NotPlayable` on an
/// illegal square) and showing them makes normal actions look like
/// failures. Logged for debugging. Sources: gui/src/main.rs and the
/// MoveError/GameError Debug output.
const INTERNAL = new Set([
  'stopped', 'position changed', 'out of range',
  'InvalidPosition', 'NotPlayable', 'Occupied', 'NoMoves', 'GameOver',
]);

/** Codes with embedded numbers, matched by prefix (`move index 5 out
 *  of range`). Internal state drift, useless to the reader — unlike
 *  fixable input errors, which have display-language messages. */
const INTERNAL_PREFIX = [
  'move index ',
  'hash does not match',
];

/** Notice kinds: `bad` = failed, `gold` = not a failure but why the
 *  action did not proceed. Nothing else — success is self-evident. */
export type ToastTone = 'bad' | 'gold';

/** Short on-screen notice; only failures and blocked-action reasons. */
export interface Toast { id: number; text: string; tone: ToastTone }

/// Auto-dismiss time: long enough to read, short enough not to squat.
const TOAST_MS = 5000;

export type AppMode = 'vs' | 'study';
export type EngineSide = 'black' | 'white' | 'both' | 'off';
export type MoveSource = 'book' | 'search';

/** Engine move record (for the table); value is Black-view discs. */
export interface MoveInfo {
  source: MoveSource;
  value: number;
  exact: boolean;
  learned: boolean;
  /** Seconds spent on this move. */
  secs: number;
}

/** Strength presets; custom edits the three knobs directly. */
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

/** Solve-entry ceiling; beyond this is not solvable in realistic time. */
export const SOLVE_MAX = 36;

/// Solve entry must be >= depth: otherwise a span exists where the
/// midgame reads past the end of the game without solving — burning
/// depth on MPC-pruned inexact values in solvable positions. The
/// depth cap aligns with the solve for the same reason.
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
  /* Clamp the level at the source: an undefined LEVELS[level] crashes
     the render (blank window) at the next `.depth`/`.name` read. The
     screenshot entry point once passed an out-of-range level, and
     clamping at use sites provably misses one (fixing `levels` alone
     left the title crashing). */
  const [levelRaw, setLevel] = useState<number | 'custom'>(6);
  const level: number | 'custom' =
    levelRaw === 'custom' ? 'custom' : Math.max(0, Math.min(LEVELS.length - 1, levelRaw));
  const [custom, setCustom] = useState<Levels>({ depth: 12, solve: 18, band: 0 });
  const [useBook, setUseBook] = useState(true);
  const [hasBook, setHasBook] = useState(true);
  // Whether finished games feed learning (backend default is on too).
  const [learnOn, setLearnOn] = useState(true);
  const [autoHint, setAutoHintRaw] = useState(false);
  const [hints, setHints] = useState<Hints | null>(null);
  // Search workload (under the board): live during analysis, last
  // move's numbers during games. Null removes the row entirely —
  // stale numbers read as "still running".
  const [stat, setStat] = useState<SearchStat | null>(null);
  const [playing, setPlaying] = useState(false);
  const [thinking, setThinking] = useState(false);
  const [thinkSecs, setThinkSecs] = useState(0);          // elapsed while thinking
  const [thinkTotal, setThinkTotal] = useState({ black: 0, white: 0 });
  const [moveSource, setMoveSource] = useState<Record<number, MoveInfo>>({});
  // Notices float (toasts): an inline row used to bounce the whole
  // layout on every show/hide. Progress reports don't belong here —
  // each feature's own section carries them.
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastId = useRef(0);
  // The engine's last-move eval (rendered as its evaluation line).

  // Results can arrive after the position moved; generations discard them.
  const hintSeq = useRef(0);

  const levels: Levels = level === 'custom' ? custom : LEVELS[level];

  const engineSides = useCallback((): string[] => {
    if (mode === 'study') return [];      // 検討モードではエンジンは打たない
    if (side === 'both') return ['black', 'white'];
    if (side === 'off') return [];        // 人が両方を打つ
    return [side];
  }, [mode, side]);

  const dismiss = useCallback((id: number) => {
    setToasts((t) => t.filter((x) => x.id !== id));
  }, []);

  const say = useCallback((s: string, tone: ToastTone = 'bad') => {
    // Empty strings were the old "clear previous message"; toasts
    // self-dismiss, so it is vestigial.
    if (!s) return;
    if (INTERNAL.has(s) || INTERNAL_PREFIX.some((p) => s.startsWith(p))) {
      jsLog('内部の符丁 (画面には出さない): ' + s);
      return;
    }
    const id = ++toastId.current;
    // The same text can repeat (same failure per move); replace
    // rather than stack.
    setToasts((t) => [...t.filter((x) => x.text !== s), { id, text: s, tone }]);
    window.setTimeout(() => dismiss(id), TOAST_MS);
  }, [dismiss]);

  const pushLevels = useCallback(async () => {
    await api.setLevels(levels.depth, levels.solve, levels.band).catch(() => {});
  }, [levels.depth, levels.solve, levels.band]);

  // ---- Startup ----
  useEffect(() => {
    void (async () => {
      try {
        setView(await api.state());
        setHasBook(await api.hasBook());
      } catch (e) {
        say('' + e);
      }
    })();
  }, [say]);

  // ---- Live hints ----
  // Keep deepening; results arrive per pass via events and replace
  // shallower ones, so the numbers tighten while you watch.
  const refreshHints = useCallback(async (v: GameView | null = view) => {
    // Never even send a stop while thinking: this function depends on
    // `thinking`, so the effect re-runs when the engine starts, and a
    // stop here would flag the ENGINE'S OWN search — reliably killing
    // the first post-book move (book moves returned fast enough to
    // win the race). The previous position's analysis was already
    // stopped by the previous invocation.
    if (thinking) return;
    /* Stop the previous analysis AND await it: fire-and-forget lets
       the stop land after the next search's stop.reset(), killing it
       at birth. Analysis dodged this by a coincidental await; ponder
       called directly and never ran once. */
    await api.stopSearch().catch(() => {});
    if (!v || v.over) return;
    if (playing && engineSides().includes(v.player)) return;   // the engine's turn
    /* Ponder instead when evals are off: warming the next position
       makes replies faster (fixed depth: 1/3 the time). Both contend
       for the same engine, so never run both — the on-screen numbers
       win. Games only; study never gets a next turn. */
    if (!autoHint) {
      if (playing) api.ponderLive().catch(() => {});
      return;
    }
    hintSeq.current++;
    try {
      await pushLevels();
      await api.analyzeLive();
    } catch (e) {
      say('' + e);
    }
  }, [view, autoHint, thinking, playing, engineSides, pushLevels, say]);

  /** Toggle the eval display; turning it off clears current values. */
  const setAutoHint = useCallback((on: boolean) => {
    setAutoHintRaw(on);
    if (!on) { setHints(null); setStat(null); }
  }, []);

  const apply = useCallback((v: GameView) => {
    setView(v);
    setHints(null);
    setStat(null);
  }, []);

  // Import played-to-the-end games into learning (never merely
  // loaded records).
  const maybeLearn = useCallback((v: GameView) => {
    if (!v.over || mode !== 'vs' || !learnOn) return;
    // Progress shows in the nav's learning counter; no toast.
    /* The human's color is the complement of KUROOBI's side; both/
       none is undecidable and sends empty (excluded from filters). */
    const mine = side === 'black' ? 'w' : side === 'white' ? 'b' : '';
    api.learnGame(mine).catch(() => {});
  }, [mode, learnOn, side]);

  // ---- Human moves ----
  const play = useCallback(async (sq: number) => {
    if (thinking) return;
    // Humans cannot move on the engine's turns.
    if (playing && view && engineSides().includes(view.player)) return;
    try {
      const v = await api.play(sq);
      apply(v);
      maybeLearn(v);
    } catch (e) { say('' + e); }
  }, [thinking, playing, view, engineSides, apply, maybeLearn, say]);

  /* Clock: 0 = none. Rust routes the same timectl as GGS; this side
     only passes seconds. Re-initialized every new game — inheriting
     the previous remainder would start games already flagged. */
  const [clock, setClock] = useState<ClockView | null>(null);
  /* Rust subtracts the running turn; this side polls once a second,
     and only during games (idle polls return static values). */
  useEffect(() => {
    if (!clockSecs || !playing) return;
    const id = setInterval(() => {
      void api.clocks().then((c) => {
        setClock(c);
        /* Stop on a flag — otherwise the flagged side keeps playing
           under a frozen zero clock. */
        if (c.lost) {
          setPlaying(false);
          say(c.lost === 'black' ? '黒の時間切れ' : '白の時間切れ', 'gold');
        }
      }).catch(() => {});
    }, 1000);
    return () => clearInterval(id);
  }, [clockSecs, playing, say]);

  const newGame = useCallback(async () => {
    hintSeq.current++;
    // Abort the running search; don't await the strength push (the
    // board would stall until the engine lock frees).
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
    try { apply(await api.undo()); } catch (e) { say('' + e); }
  }, [thinking, apply, say]);

  const jumpTo = useCallback(async (n: number) => {
    if (thinking) return;
    hintSeq.current++;
    api.stopSearch().catch(() => {});
    if (playing) setPlaying(false);   // scrubbing the record ends the live game
    try { apply(await api.goto(n)); } catch (e) { say('' + e); }
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
