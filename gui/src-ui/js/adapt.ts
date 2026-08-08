import type { GameView, GgsSnapshot, LogLine as RawLog } from './types';
import type { MoveInfo } from './state';
import type { Cell, EvalInfo } from './components/board';
import type { GraphPoint, Move, StoneColor } from './components/data';
import type { LogLine } from './components/ggs';

/* エンジンの状態 → 画面の形。
 *
 * 変換をここ 1 か所に集める。画面ごとに書くと、同じ「何手目は何色か」を
 * 別々に導いて必ずずれる (旧 UI では棋譜表とグラフで別々に持っていた)。
 *
 * 導き方は現行の Kifu.tsx と同じにしてある。挙動を変えるのは載せ替えが
 * 済んでから — いま変えると、差分がデザインのせいか変換のせいか分からなくなる。
 */

export const sqName = (sq: number): string => 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);

/** パス込みで 1 手ごとに手番が入れ替わるので、奇数手 (i が偶数) が黒。 */
const colorOf = (i: number): StoneColor => (i % 2 === 0 ? 'b' : 'w');

/** 盤の 64 マス。エンジンは number[] で返すが、値は 0/1/2 しか入らない。 */
export const cellsOf = (v: GameView): Cell[] => v.cells as Cell[];

/** 損した手の印。値は黒視点なので、黒番は下げ幅・白番は上げ幅が損。 */
function lossOf(i: number, value: number | undefined, prev: number | undefined): number | undefined {
  if (value === undefined || prev === undefined) return undefined;
  const loss = i % 2 === 0 ? prev - value : value - prev;
  return loss >= 2 ? +loss.toFixed(1) : undefined;   // 2 石以上だけ印を出す
}

/** 棋譜表の行。info (エンジンが指した記録) を values (分析) より優先する。 */
export function movesOf(
  v: GameView,
  info: Record<number, MoveInfo>,
  values?: (GraphPoint | undefined)[] | null,
  /** 見る側。黒視点なら 1、白視点なら -1。損 (▼n) は量なので符号を持たない */
  sign: 1 | -1 = 1,
): Move[] {
  const shown = v.moves.map((_, i) => (info[i + 1] ? info[i + 1].value : values?.[i + 1]?.value));
  return v.moves.map((m, i) => {
    const n = i + 1;
    const rec = info[n];
    const gp = values?.[n];
    const value = shown[i];
    const exact = rec ? rec.exact : gp?.exact ?? false;
    const book = rec ? rec.source === 'book' : gp?.book ?? false;
    const prev = i === 0 ? values?.[0]?.value ?? 0 : shown[i - 1];
    return {
      n,
      move: m == null ? '' : sqName(m),
      pass: m == null,
      color: colorOf(i),
      score: value === undefined ? undefined : value * sign,
      // 損は「その手で減らした量」。どちらから見ても同じ大きさなので反さない
      loss: lossOf(i, value, prev),
      secs: rec?.secs,
      src: value === undefined ? undefined
        : book ? (rec?.learned ? '定石·学' : '定石')
        : exact ? '読切' : '探索',
    };
  });
}

/** 盤に載せる評価値マス。ヒントは「いま打てる手」にだけ付く。 */
/** 盤に出す候補手の評価。
 *
 * **エンジンが返すのは手番視点** (`HintView.value`) だが、棋譜表とグラフは
 * 黒視点で揃えてある。同じ画面で `+8` の意味が場所によって変わっていたので、
 * ここで黒視点へ直す (設計の設定「表示」の注記 —「棋譜・グラフ・盤のすべて」)。
 * `sign` はそのうえでの見る側 (黒 = 1 / 白 = -1)。
 *
 * **最善の印は直す前の値で選ぶ。** 符号を返してから最大を取ると、白番では
 * いちばん悪い手に枠が付く。 */
export function evalsOf(
  hints: Record<number, { value: number; exact: boolean; book: boolean; depth: number }> | null,
  /** 手番が黒か。false なら手番視点の値を反転して黒視点にする */
  blackToMove = true,
  /** 見る側。黒視点なら 1、白視点なら -1 */
  sign: 1 | -1 = 1,
): Record<number, EvalInfo> | undefined {
  if (!hints) return undefined;
  const out: Record<number, EvalInfo> = {};
  let best = -Infinity;
  for (const h of Object.values(hints)) best = Math.max(best, h.value);
  const flip = (blackToMove ? 1 : -1) * sign;
  for (const [sq, h] of Object.entries(hints)) {
    out[+sq] = {
      score: h.value * flip,
      // 出所は 3 種。「N 手」だけが途中の値で、これが出ている間は数字が育つ
      src: h.book ? { book: true } : h.exact ? { exact: true } : { depth: h.depth },
      best: h.value === best,
    };
  }
  return out;
}

/* ---------------- GGS ---------------- */

/** 接続の状態。バックエンドの綴りと画面の綴りが違うので境目で直す。 */
export const connOf = (c: GgsSnapshot['conn'] | undefined): 'offline' | 'connecting' | 'logging-in' | 'online' =>
  c === 'online' ? 'online' : c === 'connecting' ? 'connecting'
    : c === 'logging_in' ? 'logging-in' : 'offline';

/** 左メニューの件数バッジ。数え方は現行の Nav.tsx と同じ。 */
export function navBadges(snap: GgsSnapshot | null, chatUnread: number) {
  return {
    // 自分宛の申し込みだけ数える (一覧には他人宛も流れる)
    'ggs-lobby': { count: snap?.offers.filter((o) => o.incoming).length || undefined, alert: true },
    // 手合いの「組」数。同期対局は 2 局で 1 組なので base でまとめる。
    // 自分の手番が来ている組があれば点も出す — 数字だけだと「増えた」と
    // 「自分が待たれている」が同じ見え方になる
    'ggs-play': {
      count: new Set(snap?.matches.map((m) => m.base || m.id) ?? []).size || undefined,
      dot: snap?.matches.some((m) => m.my_color && !m.over && m.turn === m.my_color)
        ? ('bad' as const) : undefined,
    },
    'ggs-chat': { count: chatUnread || undefined, alert: true },
    'ggs-standby': { dot: snap?.standby.enabled ? ('ok' as const) : undefined },
  };
}

/** 自分の GGS 対局が進行中か。最優先なのでローカルの開始と分析を断る。 */
export const ggsPlaying = (snap: GgsSnapshot | null): boolean =>
  snap?.matches.some((m) => m.my_color && !m.over) ?? false;

/** 通信ログ。バックエンドは `info`、画面は `app` と呼ぶ。 */
export const logLinesOf = (log: RawLog[]): LogLine[] =>
  log.map((l) => ({ dir: l.dir === 'info' ? 'app' : l.dir, text: l.text }));
