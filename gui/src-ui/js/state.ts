// 対局の状態と、エンジンとのやり取り。画面はここから読むだけにする。
//
// 手書きの DOM 更新をやめた理由がここにある。状態と表示の同期を手で書いて
// いたときは、担当の選択肢が消える・レートが空欄になる・hidden が効かない
// といった取りこぼしが繰り返し起きた。状態を 1 か所に集めて、画面は
// そこから導くだけにする。
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, jsLog } from './api';
import type { GameView, SearchStat } from './types';

/// エンジンが返す**内部の符丁**。画面には出さない。
///
/// どれも「押した本人がやったこと」の結果で、読み手に用がない — 停止を
/// 押せば `stopped` は必ず返るし、打てないマスを押せば `NotPlayable` が
/// 返る (盤を見れば分かる)。これを出すと、正常な操作のたびに異常が起きた
/// ように見える。調べられるようログには残す。
///
/// 出どころ: `stopped` / `position changed` / `out of range` は
/// `gui/src/main.rs`、残りは `MoveError` / `GameError` の Debug 出力。
const INTERNAL = new Set([
  'stopped', 'position changed', 'out of range',
  'InvalidPosition', 'NotPlayable', 'Occupied', 'NoMoves', 'GameOver',
]);

/** 画面に出す短い報せ。出るのは失敗と、押したのに進まない理由だけ。 */
export interface Toast { id: number; text: string }

/// 消えるまでの時間。読み切れる長さと、居座って邪魔にならない長さの兼ね合い。
const TOAST_MS = 5000;

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

/** 読切 (空きマス数) の上限。これ以上は現実的な時間で解けない。 */
export const SOLVE_MAX = 36;

/// 読切は深さ以上でなければならない。深さのほうが大きいと、中盤探索が
/// 終局を跨いで読むだけの区間ができる — 読み切れる局面なのに読み切りに
/// 入らず、MPC で枝を刈った不正確な値のまま深さだけを費やす。
/// 深さの上限も読切に合わせる (それより深くしても選べる読切が無い)。
export function clampLevels(v: Levels): Levels {
  const depth = Math.max(1, Math.min(SOLVE_MAX, v.depth));
  return { depth, solve: Math.max(depth, Math.min(SOLVE_MAX, v.solve)), band: v.band };
}


export interface Hints {
  [sq: number]: { value: number; exact: boolean; book: boolean; depth: number };
}

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
  const [autoHint, setAutoHintRaw] = useState(false);
  const [hints, setHints] = useState<Hints | null>(null);
  // 探索の働きぶり (盤の下)。分析中はライブで伸び、対局では直前の 1 手ぶんが
  // 残る。動いていないときは null にして行ごと消す — 古い数字を残すと
  // 「まだ動いている」と誤解する。
  const [stat, setStat] = useState<SearchStat | null>(null);
  const [playing, setPlaying] = useState(false);
  const [thinking, setThinking] = useState(false);
  const [thinkSecs, setThinkSecs] = useState(0);          // 思考中の経過
  const [thinkTotal, setThinkTotal] = useState({ black: 0, white: 0 });
  const [moveSource, setMoveSource] = useState<Record<number, MoveInfo>>({});
  // 報せは浮かせる (トースト)。盤の下に行を置いていたときは、出入りのたびに
  // 帯の高さが動いて画面全体が跳ねていた。作業が進んでいることの報せはここに
  // 入れない — それはその作業を出している場所が自分で持つ (分析の進み具合なら
  // 評価値グラフの節)
  const [toasts, setToasts] = useState<Toast[]>([]);
  const toastId = useRef(0);
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

  const dismiss = useCallback((id: number) => {
    setToasts((t) => t.filter((x) => x.id !== id));
  }, []);

  const say = useCallback((s: string) => {
    // 空文字は「前の伝言を消す」の名残。浮かせる形では自分で消えるので用がない
    if (!s) return;
    if (INTERNAL.has(s)) { jsLog('内部の符丁 (画面には出さない): ' + s); return; }
    const id = ++toastId.current;
    // 同じ文が続けて出ることがある (局面を動かすたびに同じ失敗をする、など)。
    // 積み上げずに出し直す
    setToasts((t) => [...t.filter((x) => x.text !== s), { id, text: s }]);
    window.setTimeout(() => dismiss(id), TOAST_MS);
  }, [dismiss]);

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
  // 反復深化で回し続ける。結果は段ごとにイベントで届き、購読側が入れる。
  // 深い答えが出るたびに置き換わるので、見ているあいだ値が締まっていく。
  const refreshHints = useCallback(async (v: GameView | null = view) => {
    // 思考中は停止すら送らない。この関数は thinking を見ているので、
    // エンジンが思考を始めた時点で作り直され、購読側の effect が再び走る。
    // そこで停止を送ると**エンジン自身の探索**にフラグが立ち、「停止された
    // 結果は捨てる」の判定に当たって対局が止まる (定石の間は即座に返るので
    // 競争に勝ち、定石を抜けた最初の探索手だけが確実に落ちていた)。
    // 前の局面の分析は、この 1 つ前 — 局面が動いた時点の呼び出し — が
    // 既に止めている。
    if (thinking) return;
    api.stopSearch().catch(() => {});     // 前の局面の分析を止める
    if (!autoHint || !v || v.over) return;
    if (playing && engineSides().includes(v.player)) return;   // エンジンが打つ番
    hintSeq.current++;
    try {
      await pushLevels();
      await api.analyzeLive();
    } catch (e) {
      say('' + e);
    }
  }, [view, autoHint, thinking, playing, engineSides, pushLevels, say]);

  /** 評価値の表示を切り替える。切ったらいま出ている値も消す。 */
  const setAutoHint = useCallback((on: boolean) => {
    setAutoHintRaw(on);
    if (!on) { setHints(null); setStat(null); }
  }, []);

  const apply = useCallback((v: GameView) => {
    setView(v);
    setHints(null);
    setStat(null);
  }, []);

  // 手が指されて終局したら、その対局を定石の学習に取り込む
  // (読み込んだだけの棋譜では呼ばれない)
  const maybeLearn = useCallback((v: GameView) => {
    if (!v.over || mode !== 'vs' || !learnOn) return;
    // 取り込みの進み具合は左メニュー下の「学習 n/m」に出るので、
    // ここで言葉にはしない
    api.learnGame().catch(() => {});
  }, [mode, learnOn]);

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
    say('');
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
    if (playing) setPlaying(false);   // 棋譜を動かしたら対局は続けられない
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
    lastEval, setLastEval,
    engineSides, pushLevels, refreshHints,
    play, newGame, undo, jumpTo, stop,
    hintSeq,
  };
}

export type Game = ReturnType<typeof useGame>;
