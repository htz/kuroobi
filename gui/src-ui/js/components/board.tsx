import React from 'react';

/* KUROOBI board
 * 盤は 1 枚の SVG。viewBox 880、PAD 40、CELL 100（現行 Board.tsx と同じ幾何）。
 *
 * 地は畳。1 マスを縁なし半畳 1 枚と見て、市松に藺草の目の向きを変える
 * （琉球畳の敷き方）。目はマスの中で完結するので、盤の格子線とは干渉しない。
 * 薄くしてあるのは、石とヒントの数字より前に出てはいけないから。
 *
 * 市松は CSS グラデーションでは描けない（線形勾配は 2 次元の交互配置を
 * 表せない）。かつマスごとに要素を置くと 448 本の線になるので、
 * 2×2 マス＝200×200 を 1 タイルにした <pattern> にまとめる → 28 本。
 * <pattern> は 1 回ラスタライズされるだけなので、盤が何面出ても軽い。
 */

const CELL = 100;
const PAD = 40;
const PITCH = 12.5;          // 藺草の目の間隔。1 マスに 7 本
const GRAIN_OPACITY = 0.07;
const SIZE = PAD * 2 + CELL * 8;   // 880

/* 畳の pattern。AppFrame が自分で描くので、画面側で置く必要はない。
 * （運用の約束にしていたときは、置き忘れると静かに畳が消えていた） */
export const TATAMI_ID = 'kb-tatami';

export function BoardDefs() {
  const lines: React.ReactNode[] = [];
  const d: number[] = [];
  for (let i = 1; i * PITCH < CELL; i++) d.push(i * PITCH);
  const push = (key: string, x1: number, y1: number, x2: number, y2: number) =>
    lines.push(<line key={key} x1={x1} y1={y1} x2={x2} y2={y2}
      stroke="var(--grain)" strokeOpacity={GRAIN_OPACITY} strokeWidth={2.4} />);

  // タイル内の 4 マス。vertical = (f + r) % 2 === 1
  d.forEach(v => push('a' + v, 0, v, CELL, v));                       // (0,0) 横目
  d.forEach(v => push('b' + v, CELL + v, 0, CELL + v, CELL));         // (1,0) 縦目
  d.forEach(v => push('c' + v, v, CELL, v, CELL * 2));                // (0,1) 縦目
  d.forEach(v => push('d' + v, CELL, CELL + v, CELL * 2, CELL + v));  // (1,1) 横目

  return (
    <svg width={0} height={0} style={{ position: 'absolute' }} aria-hidden>
      <defs>
        <pattern id={TATAMI_ID} width={CELL * 2} height={CELL * 2}
                 patternUnits="userSpaceOnUse" x={PAD} y={PAD}>
          {lines}
        </pattern>
      </defs>
    </svg>
  );
}

export type Cell = 0 | 1 | 2;            // 0 空 / 1 黒 / 2 白

/* 反復深化で数字が育つので、出所は 3 種。
 * 定石 / 読切 / 「N 手」（いま何手先まで読めているか）。
 * 「N 手」が出ているときだけ値が動いている — この表示がないと
 * 確定した値なのか途中なのか分からない。string に逃がさない。 */
export type EvalSource = { book: true } | { exact: true } | { depth: number };
export type EvalInfo = { score: number; src: EvalSource; best?: boolean };

export const sourceLabel = (s: EvalSource) =>
  'book' in s ? '定石' : 'exact' in s ? '読切' : `${s.depth} 手`;

const cx = (i: number) => PAD + i * CELL + CELL / 2;
const fr = (sq: number): [number, number] => [Math.floor(sq / 8), sq % 8];

export function Board({ cells, legal = [], evals, last, next, coords = true, grain = true, flip = false, disabled, onPlay }: {
  cells: Cell[];                                   // 64（sq = file*8 + rank）
  legal?: number[];
  evals?: Record<number, EvalInfo>;
  last?: number | null;
  next?: number | null;                            // 棋譜上で次に指された手。金の破線
  coords?: boolean;
  /** 畳の藺草の目。薄いので普段は気にならないが、消したい人もいる */
  grain?: boolean;
  /** 盤を回す (白を下にする)。マスの番号はそのまま、描く場所だけ入れ替える */
  flip?: boolean;
  disabled?: boolean;
  onPlay?: (sq: number) => void;
}) {
  const legalSet = new Set(legal);
  // 回すのは「どこに描くか」だけ。番号を裏返すと、打った手が別のマスになる
  const at = (sq: number): [number, number] => {
    const [f, r] = fr(flip ? 63 - sq : sq);
    return [cx(f), cx(r)];
  };
  return (
    <svg viewBox={`0 0 ${SIZE} ${SIZE}`} style={{ width: '100%', height: 'auto', display: 'block' }}
         role="img" aria-label="盤面">
      <rect x={0} y={0} width={SIZE} height={SIZE} rx={14} fill="var(--card)" />
      <rect x={PAD - 6} y={PAD - 6} width={812} height={812} rx={6} fill="var(--board-dark)" />
      <rect x={PAD} y={PAD} width={800} height={800} fill="var(--board)" />
      {grain && <rect x={PAD} y={PAD} width={800} height={800} fill={`url(#${TATAMI_ID})`} />}

      {Array.from({ length: 9 }, (_, i) => (
        <g key={'g' + i}>
          <line x1={PAD + i * CELL} y1={PAD} x2={PAD + i * CELL} y2={PAD + 800} stroke="var(--line)" strokeWidth={2} />
          <line x1={PAD} y1={PAD + i * CELL} x2={PAD + 800} y2={PAD + i * CELL} stroke="var(--line)" strokeWidth={2} />
        </g>
      ))}
      {([[2, 2], [2, 6], [6, 2], [6, 6]] as [number, number][]).map(([a, b]) => (
        <circle key={'d' + a + b} cx={PAD + a * CELL} cy={PAD + b * CELL} r={5} fill="var(--line)" />
      ))}
      {coords && Array.from({ length: 8 }, (_, i) => (
        <g key={'l' + i}>
          <text x={cx(i)} y={27} textAnchor="middle" fill="var(--sub)" fontSize={19}>{'abcdefgh'[flip ? 7 - i : i]}</text>
          <text x={21} y={PAD + i * CELL + 57} textAnchor="middle" fill="var(--sub)" fontSize={19}>{flip ? 8 - i : i + 1}</text>
        </g>
      ))}

      {cells.map((v, sq) => {
        const [x, y] = at(sq);
        if (v !== 0) return <Stone key={sq} x={x} y={y} color={v as 1 | 2} last={last === sq} />;
        if (!legalSet.has(sq)) return null;
        const ev = evals?.[sq];
        return (
          // k-cell は base.css 側で hover を持つ。合法手のマスだけ反応する
          <g key={sq} className={disabled ? undefined : 'k-cell'}
             onClick={disabled ? undefined : () => onPlay?.(sq)}>
            <circle cx={x} cy={y} r={46} fill="transparent" />
            {next === sq && <circle cx={x} cy={y} r={38} fill="none" stroke="var(--gold)" strokeWidth={2.5} strokeDasharray="6 5" />}
            {ev
              ? <g className="k-eval" style={{ opacity: .88 }}><EvalCell x={x} y={y} info={ev} /></g>
              : <circle className="k-legal" cx={x} cy={y} r={7} fill="var(--board-hint)" opacity={.5} />}
          </g>
        );
      })}
    </svg>
  );
}

/* フラットな円盤。薄い落ち影と細いリムだけ。最終手は輪で囲む */
export function Stone({ x, y, color, last }: { x: number; y: number; color: 1 | 2; last?: boolean }) {
  const black = color === 1;
  return (
    <g>
      <circle cx={x} cy={y + 2} r={40} fill="var(--stone-shadow)" />
      <circle cx={x} cy={y} r={40}
              fill={black ? 'var(--stone-black)' : 'var(--stone-white)'}
              stroke={black ? 'var(--stone-black-edge)' : 'var(--stone-white-edge)'} strokeWidth={2} />
      {last && <circle cx={x} cy={y} r={9} fill="none" stroke="var(--accent)" strokeWidth={4} />}
    </g>
  );
}

/* 数値（石差）が主、出所が副。最善だけ枠と --gold を持つ */
export function EvalCell({ x, y, info }: { x: number; y: number; info: EvalInfo }) {
  const { score, src, best } = info;
  const label = sourceLabel(src);
  // 「N 手」は途中の値なので、定石・読切より弱く出す。
  // 定石は --gold ではなく盤専用の --board-eval-book を使う — 文字は 100% の
  // 濃さで畳に乗るので、ライトでは --gold (#a07b1e) が地の緑と同化して読めない
  const srcColor = 'book' in src ? 'var(--board-eval-book)' : 'exact' in src ? 'var(--board-eval-strong)' : 'var(--board-eval-weak)';
  return (
    <g>
      <circle cx={x} cy={y} r={30}
              fill={best ? 'color-mix(in srgb, var(--gold) 14%, transparent)' : 'var(--board-eval-bg)'}
              stroke={best ? 'var(--gold)' : 'var(--board-eval-edge)'} strokeWidth={best ? 2 : 1} />
      {/* 盤上の数字は整数。60px の円に収まる桁は 3 文字（±64）までで、
          0.1 石の差はグラフと棋譜表で見れば足りる */}
      <text x={x} y={y + 2} textAnchor="middle" fontSize={24}
            fill={best ? 'var(--gold)' : score < 0 ? 'var(--bad)' : 'var(--board-eval-text)'}
            fontWeight={best ? 700 : 400}>
        {(score > 0 ? '+' : '') + Math.round(score)}
      </text>
      <text x={x} y={y + 22} textAnchor="middle" fontSize={13} fill={srcColor}>{label}</text>
    </g>
  );
}
