import React from 'react';

/* KUROOBI board
 * The board is one SVG: viewBox 880, PAD 40, CELL 100 (same geometry
 * as the previous Board.tsx).
 *
 * The ground is tatami: each cell is a borderless half-mat with grain
 * direction alternating checkerboard-style (ryukyu layout). The grain
 * stays within cells, never touching the grid, and is kept faint so it
 * never competes with discs or hint numbers.
 *
 * A checkerboard cannot be a CSS gradient (linear gradients cannot
 * alternate in 2D), and per-cell elements would mean 448 lines; a 2x2
 * <pattern> tile reduces it to 28, rasterized once no matter how many
 * boards are shown.
 */

const CELL = 100;
const PAD = 40;
const PITCH = 12.5;          // grain spacing; 7 lines per cell
const GRAIN_OPACITY = 0.07;
const SIZE = PAD * 2 + CELL * 8;   // 880

/* The tatami pattern; AppFrame renders it itself (when placement was
 * a convention, forgetting it silently lost the tatami). */
const TATAMI_ID = 'kb-tatami';

export function BoardDefs() {
  const lines: React.ReactNode[] = [];
  const d: number[] = [];
  for (let i = 1; i * PITCH < CELL; i++) d.push(i * PITCH);
  const push = (key: string, x1: number, y1: number, x2: number, y2: number) =>
    lines.push(<line key={key} x1={x1} y1={y1} x2={x2} y2={y2}
      stroke="var(--grain)" strokeOpacity={GRAIN_OPACITY} strokeWidth={2.4} />);

  // The tile's four cells; vertical = (f + r) % 2 === 1.
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

export type Cell = 0 | 1 | 2;            // 0 empty / 1 black / 2 white

/* Values grow under deepening, so sources are three-way: book / solve
 * / "N plies" (current depth). Only "N plies" values are still moving
 * — without the tag you cannot tell settled from provisional. Kept as
 * a union, not a string. */
export type EvalSource = { book: true } | { exact: true } | { depth: number };
export type EvalInfo = { score: number; src: EvalSource; best?: boolean };

const sourceLabel = (s: EvalSource) =>
  'book' in s ? '定石' : 'exact' in s ? '読切' : `${s.depth} 手`;

const cx = (i: number) => PAD + i * CELL + CELL / 2;
const fr = (sq: number): [number, number] => [Math.floor(sq / 8), sq % 8];

export function Board({ cells, legal = [], evals, last, next, coords = true, grain = true, flip = false, disabled, onPlay }: {
  cells: Cell[];                                   // 64（sq = file*8 + rank）
  legal?: number[];
  evals?: Record<number, EvalInfo>;
  last?: number | null;
  next?: number | null;                            // next move in the record; gold dashed ring
  coords?: boolean;
  /** The tatami grain; subtle, but some want it off. */
  grain?: boolean;
  /** Flip the board (White at the bottom); indices stay, only render
   *  positions swap. */
  flip?: boolean;
  disabled?: boolean;
  onPlay?: (sq: number) => void;
}) {
  const legalSet = new Set(legal);
  // Only render positions flip; flipping indices would move plays.
  const at = (sq: number): [number, number] => {
    const [f, r] = fr(flip ? 63 - sq : sq);
    return [cx(f), cx(r)];
  };
  return (
    /* Fill both dimensions and let preserveAspectRatio keep the
       ratio. height:'auto' sizes by width alone — tall containers
       top-align the board, wide ones overflow it; 100%/100% centers
       in both cases. */
    <svg viewBox={`0 0 ${SIZE} ${SIZE}`} style={{ width: '100%', height: '100%', display: 'block' }}
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
        // Mixing color into the key makes flipped discs new elements,
        // so the .k-flip animation runs exactly once on redraw — no
        // previous-position diffing needed. Newly placed discs spin
        // too (when jumping positions the whole board flips, so a
        // non-spinning placement would look odd); the flip duration
        // preference can zero it.
        if (v !== 0) return <Stone key={sq + ':' + v} x={x} y={y} color={v as 1 | 2} last={last === sq} />;
        if (!legalSet.has(sq)) return null;
        const ev = evals?.[sq];
        return (
          // k-cell hover lives in base.css; only legal cells react.
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

/* Flat discs: a faint shadow and thin rim; the last move gets a ring. */
export function Stone({ x, y, color, last }: { x: number; y: number; color: 1 | 2; last?: boolean }) {
  const black = color === 1;
  return (
    // Rotate about the cell center; the default SVG origin would
    // swing discs around the corner of the screen.
    <g className="k-flip" style={{ transformOrigin: `${x}px ${y}px` }}>
      <circle cx={x} cy={y + 2} r={40} fill="var(--stone-shadow)" />
      <circle cx={x} cy={y} r={40}
              fill={black ? 'var(--stone-black)' : 'var(--stone-white)'}
              stroke={black ? 'var(--stone-black-edge)' : 'var(--stone-white-edge)'} strokeWidth={2} />
      {last && <circle cx={x} cy={y} r={9} fill="none" stroke="var(--accent)" strokeWidth={4} />}
    </g>
  );
}

/* The number (discs) is primary, the source secondary; only the best
 * move gets a frame and --gold. */
function EvalCell({ x, y, info }: { x: number; y: number; info: EvalInfo }) {
  const { score, src, best } = info;
  const label = sourceLabel(src);
  // "N plies" is provisional and rendered weaker. Book values use the
  // board-specific token instead of --gold, which sinks into the light
  // theme's green.
  const srcColor = 'book' in src ? 'var(--board-eval-book)' : 'exact' in src ? 'var(--board-eval-strong)' : 'var(--board-eval-weak)';
  return (
    <g>
      <circle cx={x} cy={y} r={30}
              fill={best ? 'color-mix(in srgb, var(--gold) 14%, transparent)' : 'var(--board-eval-bg)'}
              stroke={best ? 'var(--gold)' : 'var(--board-eval-edge)'} strokeWidth={best ? 2 : 1} />
      {/* Board numbers are integers: a 60px disc fits 3 chars (±64),
          and 0.1-disc differences belong to the graph and table. */}
      <text x={x} y={y + 2} textAnchor="middle" fontSize={24}
            fill={best ? 'var(--gold)' : score < 0 ? 'var(--bad)' : 'var(--board-eval-text)'}
            fontWeight={best ? 700 : 400}>
        {(score > 0 ? '+' : '') + Math.round(score)}
      </text>
      <text x={x} y={y + 22} textAnchor="middle" fontSize={13} fill={srcColor}>{label}</text>
    </g>
  );
}
