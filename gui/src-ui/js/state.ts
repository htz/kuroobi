// 対局の状態と、エンジンとのやり取り。画面はここから読むだけにする。
//
// 手書きの DOM 更新をやめた理由がここにある。状態と表示の同期を手で書いて
// いたときは、担当の選択肢が消える・レートが空欄になる・hidden が効かない
// といった取りこぼしが繰り返し起きた。状態を 1 か所に集めて、画面は
// そこから導くだけにする。
import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from './api';
import type { GameView, HintView } from './types';

export type AppMode = 'vs' | 'study';
export type EngineSide = 'black' | 'white' | 'both' | 'off';
export type MoveSource = 'book' | 'search';

/** エンジンが指した手の記録 (棋譜の表に出す)。value は黒視点の石差。 */
export interface MoveInfo {
  source: MoveSource;
  value: number;
  exact: boolean;
  learned: boolean;
  /** この手に使った時間 (秒)。 */
  secs: number;
}

/** 強さのプリセット。カスタムを選ぶと下の 3 つを直接いじる。 */
export const LEVELS = [
  { name: 'Lv1 (入門)', depth: 1, solve: 2, band: 0 },
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
  { name: 'Lv12 (GGS 設定)', depth: 22, solve: 26, band: 6 },
  { name: 'Lv13 (全力)', depth: 24, solve: 26, band: 8 },
] as const;

export interface Levels { depth: number; solve: number; band: number }

export interface Hints { [sq: number]: { value: number; exact: boolean; book: boolean } }

export function useGame() {
  const [view, setView] = useState<GameView | null>(null);
  const [mode, setMode] = useState<AppMode>('vs');
  const [side, setSide] = useState<EngineSide>('white');
  const [level, setLevel] = useState<number | 'custom'>(6);
  const [custom, setCustom] = useState<Levels>({ depth: 12, solve: 18, band: 0 });
  const [useBook, setUseBook] = useState(true);
  const [hasBook, setHasBook] = useState(true);
  // 終局した対局を定石の学習に取り込むか (バックエンドの既定も on)
  const [learnOn, setLearnOn] = useState(true);
  const [autoHint, setAutoHint] = useState(false);
  const [hints, setHints] = useState<Hints | null>(null);
  const [playing, setPlaying] = useState(false);
  const [thinking, setThinking] = useState(false);
  const [thinkSecs, setThinkSecs] = useState(0);          // 思考中の経過
  const [thinkTotal, setThinkTotal] = useState({ black: 0, white: 0 });
  const [moveSource, setMoveSource] = useState<Record<number, MoveInfo>>({});
  const [status, setStatus] = useState('');
  const [spin, setSpin] = useState(false);
  // エンジンが直前に指した手の評価 (「エンジン評価: +2.5 石」と出す)
  const [lastEval, setLastEval] = useState('');

  // 解析結果は返ってきた時点で局面が進んでいることがある。世代で捨てる。
  const hintSeq = useRef(0);

  const levels: Levels = level === 'custom' ? custom : LEVELS[level];

  const engineSides = useCallback((): string[] => {
    if (mode === 'study') return [];      // 検討モードではエンジンは打たない
    if (side === 'both') return ['black', 'white'];
    if (side === 'off') return [];        // 人が両方を打つ
    return [side];
  }, [mode, side]);

  const say = useCallback((s: string, spinning = false) => {
    setStatus(s);
    setSpin(spinning);
  }, []);

  const pushLevels = useCallback(async () => {
    await api.setLevels(levels.depth, levels.solve, levels.band).catch(() => {});
  }, [levels.depth, levels.solve, levels.band]);

  // ---- 起動時 ----
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

  // ---- 常時ヒント ----
  const refreshHints = useCallback(async (v: GameView | null = view) => {
    if (!autoHint || !v || v.over || thinking) return;
    if (playing && engineSides().includes(v.player)) return;   // エンジンが打つ番
    const seq = ++hintSeq.current;
    const at = v.kifu;
    say('解析中…', true);
    try {
      await pushLevels();
      const hs: HintView[] = await api.analyze(levels.depth);
      if (seq !== hintSeq.current) return;
      const next: Hints = {};
      for (const h of hs) {
        if (!Number.isFinite(h.value)) continue;
        next[h.pos] = { value: h.value, exact: h.exact, book: h.from_book };
      }
      setHints(Object.keys(next).length ? next : null);
      void at;
      say('');
    } catch (e) {
      if (seq === hintSeq.current) say('' + e);
    }
  }, [view, autoHint, thinking, playing, engineSides, pushLevels, levels.depth, say]);

  const apply = useCallback((v: GameView) => {
    setView(v);
    setHints(null);
  }, []);

  // 手が指されて終局したら、その対局を定石の学習に取り込む
  // (読み込んだだけの棋譜では呼ばれない)
  const maybeLearn = useCallback((v: GameView) => {
    if (!v.over || mode !== 'vs' || !learnOn) return;
    api.learnGame()
      .then(() => say('終局した対局を定石の学習に取り込みます (裏で実行)'))
      .catch(() => {});
  }, [mode, learnOn, say]);

  // ---- 人が打つ ----
  const play = useCallback(async (sq: number) => {
    if (thinking) return;
    // 対局中にエンジンが受け持つ手番なら、人は打てない
    if (playing && view && engineSides().includes(view.player)) return;
    try {
      const v = await api.play(sq);
      apply(v);
      maybeLearn(v);
    } catch (e) { say('' + e); }
  }, [thinking, playing, view, engineSides, apply, maybeLearn, say]);

  const newGame = useCallback(async () => {
    hintSeq.current++;
    // 進行中の探索は打ち切る。強さの反映は待たない — 待つとエンジンの
    // ロックが空くまで盤面が作られず、押した感触が無くなる
    api.stopSearch().catch(() => {});
    void pushLevels();
    setMoveSource({});
    setThinkTotal({ black: 0, white: 0 });
    setLastEval('');
    setPlaying(false);
    apply(await api.newGame());
    say('「▶ 対局開始」で対局、そのまま打てば検討モード');
  }, [apply, pushLevels, say]);

  const undo = useCallback(async () => {
    if (thinking) return;
    hintSeq.current++;
    try { apply(await api.undo()); } catch (e) { say('' + e); }
  }, [thinking, apply, say]);

  const jumpTo = useCallback(async (n: number) => {
    if (thinking) return;
    hintSeq.current++;
    api.stopSearch().catch(() => {});
    if (playing) { setPlaying(false); say('棋譜を移動したため停止しました'); }
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
    playing, setPlaying,
    thinking, setThinking,
    thinkSecs, setThinkSecs,
    thinkTotal, setThinkTotal,
    moveSource, setMoveSource,
    status, spin, say,
    lastEval, setLastEval,
    engineSides, pushLevels, refreshHints,
    play, newGame, undo, jumpTo, stop,
    hintSeq,
  };
}

export type Game = ReturnType<typeof useGame>;
