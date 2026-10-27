import React from 'react';
import { t } from '../i18n';


const CELL = 100;
const PAD = 40;
const PITCH = 12.5;          // grain spacing; 7 lines per cell
const GRAIN_OPACITY = 0.07;
const SPAN = CELL * 8;
const SIZE = PAD * 2 + SPAN;
const COORD_MIN = 11;        // screen px

const TATAMI_ID = 'kb-tatami';

export function BoardDefs() {
  const lines: React.ReactNode[] = [];
  const d: number[] = [];
  for (let i = 1; i * PITCH < CELL; i++) d.push(i * PITCH);
  const push = (key: string, x1: number, y1: number, x2: number, y2: number) =>
    lines.push(<line key={key} x1={x1} y1={y1} x2={x2} y2={y2}
      stroke="var(--grain)" strokeOpacity={GRAIN_OPACITY} strokeWidth={2.4} />);

  d.forEach(v => push('a' + v, 0, v, CELL, v));
  d.forEach(v => push('b' + v, CELL + v, 0, CELL + v, CELL));
  d.forEach(v => push('c' + v, v, CELL, v, CELL * 2));
  d.forEach(v => push('d' + v, CELL, CELL + v, CELL * 2, CELL + v));

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

type EvalSource = { book: true } | { exact: true } | { depth: number };
export type EvalInfo = { score: number; src: EvalSource; best?: boolean };

const sourceLabel = (s: EvalSource) =>
  'book' in s ? t('ui.board.src_book')
  : 'exact' in s ? t('ui.board.src_solve')
  : t('ui.board.src_depth', { n: s.depth });

const evalText = (score: number, exact: boolean) => {
  const body = exact ? String(Math.round(score)) : score.toFixed(1);
  const zero = Number(body) === 0;
  return (Number(body) > 0 ? '+' : '') + (zero ? body.replace('-', '') : body);
};

const numSize = (s: string) => (s.length <= 3 ? 24 : s.length === 4 ? 21 : 17);

const cx = (i: number) => PAD + i * CELL + CELL / 2;
const fr = (sq: number): [number, number] => [Math.floor(sq / 8), sq % 8];

export function Board({ cells, legal = [], evals, last, coords, grain = true, flip = false, disabled, onPlay }: {
  cells: Cell[];                                   // 64 (sq = file*8 + rank)
  legal?: number[];
  evals?: Record<number, EvalInfo>;
  last?: number | null;
  coords: boolean;
  grain?: boolean;
  flip?: boolean;
  disabled?: boolean;
  onPlay?: (sq: number) => void;
}) {
  const legalSet = new Set(legal);
  const at = (sq: number): [number, number] => {
    const [f, r] = fr(flip ? 63 - sq : sq);
    return [cx(f), cx(r)];
  };
  // The board scales as a whole, so on a small one the coordinates would shrink below reading
  // size: they grow back to at least COORD_MIN screen pixels (the margin has room for that).
  const [px, setPx] = React.useState(SIZE);
  const svg = React.useRef<SVGSVGElement>(null);
  React.useLayoutEffect(() => {
    const el = svg.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setPx(Math.min(el.clientWidth, el.clientHeight) || SIZE));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const coordSize = Math.min(30, Math.max(19, COORD_MIN * SIZE / px));
  return (
    <svg ref={svg} viewBox={`0 0 ${SIZE} ${SIZE}`} style={{ width: '100%', height: '100%', display: 'block' }}
         role="img" aria-label={t('ui.board.aria')}>
      <rect x={0} y={0} width={SIZE} height={SIZE} rx={14} fill="var(--card)" />
      <rect x={PAD - 6} y={PAD - 6} width={SPAN + 12} height={SPAN + 12} rx={6} fill="var(--board-dark)" />
      <rect x={PAD} y={PAD} width={SPAN} height={SPAN} fill="var(--board)" />
      {grain && <rect x={PAD} y={PAD} width={SPAN} height={SPAN} fill={`url(#${TATAMI_ID})`} />}

      {Array.from({ length: 9 }, (_, i) => (
        <g key={'g' + i}>
          <line x1={PAD + i * CELL} y1={PAD} x2={PAD + i * CELL} y2={PAD + SPAN} stroke="var(--line)" strokeWidth={2} />
          <line x1={PAD} y1={PAD + i * CELL} x2={PAD + SPAN} y2={PAD + i * CELL} stroke="var(--line)" strokeWidth={2} />
        </g>
      ))}
      {([[2, 2], [2, 6], [6, 2], [6, 6]] as [number, number][]).map(([a, b]) => (
        <circle key={'d' + a + b} cx={PAD + a * CELL} cy={PAD + b * CELL} r={5} fill="var(--line)" />
      ))}
      {coords && Array.from({ length: 8 }, (_, i) => (
        <g key={'l' + i}>
          <text x={cx(i)} y={PAD - 13} textAnchor="middle" fill="var(--sub)" fontSize={coordSize}>{'abcdefgh'[flip ? 7 - i : i]}</text>
          <text x={PAD / 2 + 1} y={PAD + i * CELL + CELL / 2} dominantBaseline="central" textAnchor="middle" fill="var(--sub)" fontSize={coordSize}>{flip ? 8 - i : i + 1}</text>
        </g>
      ))}

      {cells.map((v, sq) => {
        const [x, y] = at(sq);
        if (v !== 0) return <Stone key={sq + ':' + v} x={x} y={y} color={v as 1 | 2} last={last === sq} />;
        if (!legalSet.has(sq)) return null;
        const ev = evals?.[sq];
        return (
          <g key={sq} className={disabled ? undefined : 'k-cell'}
             onClick={disabled ? undefined : () => onPlay?.(sq)}>
            <circle cx={x} cy={y} r={46} fill="transparent" />
            {ev
              ? <g className="k-eval" style={{ opacity: .88 }}><EvalCell x={x} y={y} info={ev} /></g>
              : <circle className="k-legal" cx={x} cy={y} r={7} fill="var(--board-hint)" opacity={.5} />}
          </g>
        );
      })}
    </svg>
  );
}

function Stone({ x, y, color, last }: { x: number; y: number; color: 1 | 2; last?: boolean }) {
  const black = color === 1;
  return (
    <g className="k-flip" style={{ transformOrigin: `${x}px ${y}px` }}>
      <circle cx={x} cy={y + 2} r={40} fill="var(--stone-shadow)" />
      <circle cx={x} cy={y} r={40}
              fill={black ? 'var(--stone-black)' : 'var(--stone-white)'}
              stroke={black ? 'var(--stone-black-edge)' : 'var(--stone-white-edge)'} strokeWidth={2} />
      {last && <circle cx={x} cy={y} r={9} fill="none" stroke="var(--accent)" strokeWidth={4} />}
    </g>
  );
}

function EvalCell({ x, y, info }: { x: number; y: number; info: EvalInfo }) {
  const { score, src, best } = info;
  const label = sourceLabel(src);
  const num = evalText(score, 'exact' in src);
  const srcColor = 'book' in src ? 'var(--board-eval-book)'
    : 'exact' in src ? 'var(--board-eval-strong)'
    : 'var(--board-eval-weak)';
  return (
    <g>
      <circle cx={x} cy={y} r={30} fill="var(--board-eval-bg)"
              stroke={best ? 'var(--board-eval-best)' : 'var(--board-eval-edge)'} strokeWidth={best ? 2.5 : 1} />
      <text x={x} y={y + 2} textAnchor="middle" fontSize={numSize(num)}
            fill={best ? 'var(--board-eval-best)' : score < 0 ? 'var(--board-eval-neg)' : 'var(--board-eval-text)'}
            fontWeight={best ? 700 : 400}>
        {num}
      </text>
      <text x={x} y={y + 22} textAnchor="middle" fontSize={13} fill={srcColor}>{label}</text>
    </g>
  );
}
