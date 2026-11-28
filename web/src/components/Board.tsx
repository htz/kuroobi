// The web's board (design-system/web): the app's drawing (crates/gui/src-ui/js/components/board.tsx) with web text.
// No text falls under TEXT_MIN screen pixels; an eval circle shows a whole-number value, marked when solved or from the book.
import React from 'react';
import { t, type Lang } from '../i18n';
import type { Cell } from '../lib/othello';

const CELL = 100;
const PAD = 40;
const PITCH = 12.5;
const GRAIN_OPACITY = 0.07;
const SPAN = CELL * 8;
const SIZE = PAD * 2 + SPAN;
const TEXT_MIN = 13;
/** A stone turns over in FLIP_MS, each one FLIP_STEP_MS after the one nearer the stone just played. */
export const FLIP_MS = 320;
const FLIP_STEP_MS = 60;
const PLACE_MS = 200;

type Motion = { placed: number; delay: Map<number, number>; until: number };

/** What changed between two boards, when it is one move: the stone placed and each turned stone's delay. */
function motionOf(prev: readonly Cell[] | null, next: readonly Cell[]): Motion | null {
  if (!prev) return null;
  const placed: number[] = [];
  const turned: number[] = [];
  for (let sq = 0; sq < 64; sq++) {
    if (prev[sq] === next[sq]) continue;
    if (prev[sq] === 0 && next[sq] !== 0) placed.push(sq);
    else if (prev[sq] !== 0 && next[sq] !== 0) turned.push(sq);
    else return null;
  }
  if (placed.length !== 1 || !turned.length) return null;
  const p = placed[0];
  const delay = new Map<number, number>();
  let last = 0;
  for (const sq of turned) {
    const ring = Math.max(Math.abs((sq >> 3) - (p >> 3)), Math.abs((sq & 7) - (p & 7)));
    const d = PLACE_MS / 2 + (ring - 1) * FLIP_STEP_MS;
    delay.set(sq, d);
    last = Math.max(last, d);
  }
  return { placed: p, delay, until: last + FLIP_MS };
}

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

/** `score` is the side to move's; the best one is drawn bold in the best colour. */
/** `src` marks where a value came from: a check for a solve to the end, a book for the opening book; a search has none. */
export type EvalInfo = { score: number; best?: boolean; src?: 'solve' | 'book' };

const evalText = (score: number) => {
  const r = Math.round(score);
  return (r > 0 ? '+' : r < 0 ? '−' : '') + Math.abs(r);
};

const cx = (i: number) => PAD + i * CELL + CELL / 2;
const fr = (sq: number): [number, number] => [Math.floor(sq / 8), sq % 8];

export function Board({ lang, cells, legal = [], evals, last, turn = 'b', coords, grain = true, flip = false, disabled, onPlay }: {
  lang: Lang;
  cells: readonly Cell[];
  legal?: number[];
  evals?: Record<number, EvalInfo>;
  last?: number | null;
  turn?: 'b' | 'w';
  coords: boolean;
  grain?: boolean;
  flip?: boolean;
  disabled?: boolean;
  onPlay?: (sq: number) => void;
}) {
  // A move is shown as it is played: the stone placed, then the stones it turns, nearest first. Until they settle
  // the board shows no legal marks and takes no clicks.
  const prev = React.useRef<readonly Cell[] | null>(null);
  const motion = React.useMemo(() => motionOf(prev.current, cells), [cells]);
  const [settled, setSettled] = React.useState(true);
  React.useLayoutEffect(() => {
    prev.current = cells;
    if (!motion) return;
    setSettled(false);
    const id = setTimeout(() => setSettled(true), motion.until);
    return () => clearTimeout(id);
  }, [cells, motion]);
  const quiet = !settled;
  const legalSet = new Set(quiet ? [] : legal);
  const at = (sq: number): [number, number] => {
    const [f, r] = fr(flip ? 63 - sq : sq);
    return [cx(f), cx(r)];
  };
  // The board scales as a whole, so on a small one its text grows back to at least TEXT_MIN screen pixels.
  const [px, setPx] = React.useState(SIZE);
  const svg = React.useRef<SVGSVGElement>(null);
  React.useLayoutEffect(() => {
    const el = svg.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setPx(Math.min(el.clientWidth, el.clientHeight) || SIZE));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const unit = SIZE / px;
  const coordSize = Math.max(19, TEXT_MIN * unit);
  const evalSize = Math.max(26, TEXT_MIN * unit);
  // A value's mark stays at least 12px across on screen, so its glyph reads on a phone.
  const markR = Math.max(11, 6 * unit);
  return (
    <svg ref={svg} viewBox={`0 0 ${SIZE} ${SIZE}`} style={{ width: '100%', height: '100%', display: 'block' }}
         role="img" aria-label={t(lang, 'board.aria')}>
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
        if (v !== 0) {
          const anim = motion?.placed === sq ? 'place' : motion?.delay.get(sq);
          return <Stone key={sq + ':' + v} x={x} y={y} color={v} last={last === sq} anim={anim} />;
        }
        if (!legalSet.has(sq)) return null;
        const ev = evals?.[sq];
        return (
          <g key={sq} className={disabled ? undefined : 'k-cell'}
             onClick={disabled ? undefined : () => onPlay?.(sq)}>
            <circle cx={x} cy={y} r={46} fill="transparent" />
            {ev
              ? <g className="k-eval"><EvalCell x={x} y={y} info={ev} side={turn} size={evalSize} mark={markR} /></g>
              : <circle className="k-legal" cx={x} cy={y} r={9} fill={`var(--mark-${turn}-dot)`}
                        stroke={`var(--mark-${turn}-dot-edge)`} strokeWidth={1.5} opacity={.85} />}
          </g>
        );
      })}
    </svg>
  );
}

/**
 * `anim`: 'place' for the stone just played; for a stone it turned, the ms to wait before turning, during which the
 * stone still shows its old colour; nothing for a stone that stayed.
 */
function Stone({ x, y, color, last, anim }: { x: number; y: number; color: 1 | 2; last?: boolean; anim?: 'place' | number }) {
  const at: React.CSSProperties = { transformOrigin: `${x}px ${y}px`, transformBox: 'view-box' };
  if (anim === 'place') {
    return <g style={{ ...at, animation: `w-place ${PLACE_MS}ms var(--ease) both` }}><Disc x={x} y={y} color={color} last={last} /></g>;
  }
  if (anim !== undefined) {
    const half = FLIP_MS / 2;
    return (
      <>
        <g style={{ ...at, animation: `w-flip-out ${half}ms ease-in ${anim}ms both` }}><Disc x={x} y={y} color={color === 1 ? 2 : 1} /></g>
        <g style={{ ...at, animation: `w-flip-in ${half}ms ease-out ${anim + half}ms both` }}><Disc x={x} y={y} color={color} last={last} /></g>
      </>
    );
  }
  return <g><Disc x={x} y={y} color={color} last={last} /></g>;
}

function Disc({ x, y, color, last }: { x: number; y: number; color: 1 | 2; last?: boolean }) {
  const black = color === 1;
  return (
    <>
      <circle cx={x} cy={y + 2} r={40} fill="var(--stone-shadow)" />
      <circle cx={x} cy={y} r={40}
              fill={black ? 'var(--stone-black)' : 'var(--stone-white)'}
              stroke={black ? 'var(--stone-black-edge)' : 'var(--stone-white-edge)'} strokeWidth={2} />
      {last && <circle cx={x} cy={y} r={9} fill="none" stroke="var(--accent)" strokeWidth={4} />}
    </>
  );
}

function EvalCell({ x, y, info, side, size, mark }: { x: number; y: number; info: EvalInfo; side: 'b' | 'w'; size: number; mark: number }) {
  const { score, best, src } = info;
  const px = x + 26, py = y - 26;
  const num = evalText(score);
  const c = (k: string) => `var(--mark-${side}-${k})`;
  return (
    <g>
      <circle cx={x} cy={y} r={36} fill={c('bg')}
              stroke={best ? c('best') : c('edge')} strokeWidth={best ? 3 : 1.5} />
      <text x={x} y={y} textAnchor="middle" dominantBaseline="central" fontSize={size}
            fill={best ? c('best') : Math.round(score) < 0 ? c('neg') : c('text')}
            fontWeight={best ? 700 : 500}>
        {num}
      </text>
      {src && <EvalMark x={px} y={py} r={mark} src={src} />}
    </g>
  );
}

/** A badge drawn on a 20-unit grid and scaled to `r`: a check on `text` for a solve, an open book on `accent` for the book. */
function EvalMark({ x, y, r, src }: { x: number; y: number; r: number; src: 'solve' | 'book' }) {
  return (
    <g transform={`translate(${x} ${y}) scale(${r / 10})`}>
      <circle r={10} fill={src === 'solve' ? 'var(--text)' : 'var(--accent)'} stroke="var(--bg)" strokeWidth={1.5} />
      {src === 'solve'
        ? <path d="M-4.6 0.4 L-1.4 3.6 L4.8 -3.2" fill="none" stroke="var(--bg)" strokeWidth={2.4}
                strokeLinecap="round" strokeLinejoin="round" />
        : <path d="M0 -3 C-1.8 -4.4 -3.8 -4.6 -5.6 -4 V4 C-3.8 3.4 -1.8 3.6 0 5 C1.8 3.6 3.8 3.4 5.6 4 V-4 C3.8 -4.6 1.8 -4.4 0 -3 Z M0 -3 V5"
                fill="none" stroke="var(--bg)" strokeWidth={1.5} strokeLinejoin="round" />}
    </g>
  );
}

/**
 * What the marks on the board's evals mean, as a row of samples and names: a check for a solve and a book for the
 * opening book (`book`, once the values can come from it). Only marks the board draws; a search has none.
 */
export function EvalLegend({ lang, book }: { lang: Lang; book?: boolean }) {
  const G = (key: string) => t(lang, 'graph.' + key);
  const sample = (src: 'solve' | 'book') => (
    <svg width={16} height={16} viewBox="-11 -11 22 22" aria-hidden="true" style={{ flex: 'none' }}>
      <EvalMark x={0} y={0} r={10} src={src} />
    </svg>
  );
  const item = (src: 'solve' | 'book', label: string) => (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>{sample(src)}{label}</span>
  );
  return (
    <div className="w-eval-legend" style={{
      display: 'flex', flexWrap: 'wrap', columnGap: 16, rowGap: 4,
      font: 'var(--t-meta) var(--ff-web)', color: 'var(--sub)',
    }}>
      {item('solve', G('srcSolve'))}
      {book && item('book', G('srcBook'))}
    </div>
  );
}

