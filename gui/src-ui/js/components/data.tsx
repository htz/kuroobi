import React from 'react';
import { Badge, Button, Dot } from './primitives';
import { Col, Divider, Empty, TableHead, TableRow, picked as pickedStyle } from './layout';
import { t } from '../i18n';

/* KUROOBI data — move table, eval graph, player rows, ratings.
 * Table rows are --h-row; column widths fixed, values right-aligned.
 * Loss markers (▼n) live in the eval column, never the source column. */

/** Stone color as drawn on screen. The engine's 1|2 is converted at
 *  the boundary and never reaches the UI (eases a future viewpoint
 *  toggle). GGS wire colors are ggs.ts's colorChoices() (* / O / ?) —
 *  a different thing. */
export type StoneColor = 'b' | 'w';
export const toStoneColor = (n: 1 | 2): StoneColor => (n === 1 ? 'b' : 'w');

export type Move = {
  n: number; move: string; color: StoneColor;
  score?: number; loss?: number; secs?: number;
  /** Source token; see MoveSrc. "book_learned" marks branches learned
   *  from play — the current table distinguishes it, so keep it. */
  src?: string;
/** A skipped turn with no legal move; move is unused. */
  pass?: boolean;
};

/** Move sources adapt.ts produces. The table translates these four
 *  and renders anything else verbatim, so an unknown token degrades to
 *  its own text instead of a missing-key marker. */
export type MoveSrc = 'book' | 'book_learned' | 'search' | 'solve';

const SRC_KEY: Record<string, string> = {
  book: 'data.src.book',
  book_learned: 'data.src.book_learned',
  search: 'data.src.search',
  solve: 'data.src.solve',
};

/** Display text for a move source. */
export const srcLabel = (src: string): string => (SRC_KEY[src] ? t(SRC_KEY[src]) : src);
/** Book sources are gold; solved ones take plain text (rule 19). */
export const srcIsBook = (src: string): boolean => src === 'book' || src === 'book_learned';
export const srcIsSolve = (src: string): boolean => src === 'solve';

/** Move-table columns — head and rows read the SAME array. Eval and
 *  time are vertically compared numbers (tabular); source takes the
 *  remainder. A function, not a constant: the heads are translated at
 *  render, or a language switch never reaches them. */
const moveCols = (): Col[] => [
  { head: '#', w: 22, right: true },
  { head: t('data.moves.header_move'), w: 58 },
  { head: t('data.moves.header_eval'), w: 56, right: true, num: true },
  { head: t('data.moves.header_time'), w: 34, right: true, num: true },
  { head: t('data.moves.header_source'), right: true },
];

export function KifuTable({ moves, current, onSelect, decimals = 1 }: {
  moves: Move[]; current?: number; onSelect?: (n: number) => void;
  /** Eval decimal places (settings, Display tab). */
  decimals?: number;
}) {
  // Follow the current move: rows grow every move during play and the
  // current one drifts out of view. scrollIntoView moves ancestors too
  // (the whole panel scrolled), so compute within the frame.
  const box = React.useRef<HTMLDivElement>(null);
  const row = React.useRef<HTMLButtonElement>(null);
  React.useEffect(() => {
    const b = box.current, r = row.current;
    if (!b || !r) return;
    const top = r.offsetTop, bottom = top + r.offsetHeight;
    if (top < b.scrollTop) b.scrollTop = top;
    else if (bottom > b.scrollTop + b.clientHeight) b.scrollTop = bottom - b.clientHeight;
  }, [current, moves.length]);
  const cols = moveCols();
  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      <TableHead cols={cols} />
      <div className="k-scroll" ref={box} style={{ flex: 1, minHeight: 0 }}>
        {/* Never show bare column headers with zero moves: pre-game
            this frame is always empty and read as broken-or-empty. */}
        {!moves.length && (
          <span style={{ display: 'block', padding: '0 var(--sp-3)' }}>
            <Empty>{t('data.moves.empty')}</Empty>
          </span>
        )}
        {moves.map(m => {
          const played = m.score !== undefined;
          const isCurrent = m.n === current;
          return (
            // Rows are choices, so button — study mode arrows through
            // them, which requires the table to be focusable (a div
            // never enters the Tab ring). Eval/time are vertically
            // compared digit columns.
            <TableRow key={m.n} cols={cols} on={isCurrent} muted={!played}
                      innerRef={isCurrent ? row : undefined}
                      onClick={() => onSelect?.(m.n)}>
              <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{m.n}</span>
              <span style={{ fontFamily: 'var(--ff-mono)' }}>
                <StoneDot color={m.color} />
                {/* Pass is not a coordinate: drop the monospace, dim
                    with --sub, and don't try to align it. */}
                {m.pass
                  ? <span style={{ fontFamily: 'inherit', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('data.moves.pass')}</span>
                  : m.move}
              </span>
              <span style={{ display: 'flex', justifyContent: 'flex-end', gap: 5 }}>
                {m.loss ? <span style={{ fontSize: 'var(--fs-7)', color: 'var(--bad)' }}>▼{m.loss}</span> : null}
                <span>{m.score === undefined ? '' : (m.score > 0 ? '+' : '') + m.score.toFixed(decimals)}</span>
              </span>
              <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{m.secs?.toFixed(1) ?? ''}</span>
              <span style={{
                fontSize: 'var(--fs-6)',
                color: m.src && srcIsBook(m.src) ? 'var(--gold)'
                  : m.src && srcIsSolve(m.src) ? 'var(--text)' : 'var(--sub)',
              }}>{m.src ? srcLabel(m.src) : ''}</span>
            </TableRow>
          );
        })}
      </div>
    </div>
  );
}

/* Stone dot for table rows, player rows and legends. Board stones are
 * board.tsx's Stone. */
export function StoneDot({ color, size = 9 }: { color: StoneColor; size?: number }) {
  const black = color === 'b';
  return <span style={{
    width: size, height: size, borderRadius: '50%', flex: 'none',
    background: black ? 'var(--stone-black)' : 'var(--stone-white)',
    boxShadow: 'inset 0 0 0 1px ' + (black ? 'var(--stone-black-edge)' : 'var(--stone-white-edge)'),
  }} />;
}

/* Not boxed: a full-width band under the board.
 * Y is disc difference (0 even, up = black better) with dashes every
 * 8 discs; X is move number with rules every 10. Drop either and "how
 * many discs at which move" becomes unreadable. Dots are colored by
 * source — book --gold / solved --text / search --accent. */
export type GraphPoint = { value: number; exact?: boolean; book?: boolean };

export function EvalGraph({ points, plies, cursor, blunder, busy, pov = 'b', extra, onJump, moveName, open }: {
  points: (GraphPoint | undefined)[];
  plies?: number;
  cursor?: number;
  blunder?: { at: number; loss: number };
  /** Analyzing; changes the center message to distinguish from
   *  not-yet-computed. */
  busy?: boolean;
  /** Viewpoint; swaps the heading and edge labels (values arrive
   *  already negated by the caller). */
  pov?: 'b' | 'w';
  /** Right edge of the heading row (analysis progress, stop button). */
  extra?: React.ReactNode;
  /** Jump to the clicked move; an existing affordance, keep it. */
  onJump?: (n: number) => void;
  /** Move name at that ply ("f5"), shown in the heading readout. */
  moveName?: (n: number) => string | undefined;
  /** Open state in the lowest tier (<=620px); the collapsing lives in
   *  base.css. The toggle is in the Toolbar — the heading row itself
   *  collapses away, so it cannot live here. */
  open?: boolean;
}) {
  /* Hovered ply, readable without clicking — dot positions alone
     forced click-and-check. Shown in the heading row (rule 19: never
     overlay the plot area). */
  const [hover, setHover] = React.useState<number | null>(null);
  /* Measured size: the viewBox height derives from the container.
     With width 100% / height auto the aspect is fixed — the band
     stayed 266px while the board crushed to 82px (rule 7 keeps the
     board last). The width scale (px / W) is unchanged, so text and
     padding keep their size; only the drawable height shrinks.
     ResizeObserver attaches in the ref callback (setState in an
     effect trips the React Compiler lint). */
  const [box, setBox] = React.useState({ w: 0, h: 0 });
  const obs = React.useRef<ResizeObserver | null>(null);
  const attach = React.useCallback((el: HTMLDivElement | null) => {
    obs.current?.disconnect();
    obs.current = null;
    if (!el) return;
    const o = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      setBox((b) => (Math.abs(b.w - r.width) < 0.5 && Math.abs(b.h - r.height) < 0.5
        ? b : { w: r.width, h: r.height }));
    });
    o.observe(el);
    obs.current = o;
  }, []);

  /* Viewpoint: the caller negates values (in one place, adapt) or the
     table and graph disagree on sign. */
  const black = pov === 'b';
  const title = t(black ? 'data.graph.title_black_view' : 'data.graph.title_white_view');
  /** Top edge is the viewpoint side ahead, bottom edge the other. */
  const upLabel = t(black ? 'data.graph.black_ahead' : 'data.graph.white_ahead');
  const downLabel = t(black ? 'data.graph.white_ahead' : 'data.graph.black_ahead');
  const W = 800, L = 44, R = 54, T = 18, B = 26, STEP = 8;
  /** Natural height and the floor below which we stop shrinking. */
  const NAT = 210, MIN = 120;
  const scale = box.w > 0 ? box.w / W : 0;
  /* Min/max live on the container in px (scale depends on width only,
     so no cycle). Rounding H afterwards skews the x/y scales and
     distorts text. */
  const H = scale > 0 ? box.h / scale : NAT;
  const len = Math.max(1, plies ?? points.length - 1);
  const defined = points.filter((p): p is GraphPoint => !!p).map(p => Math.abs(p.value));
  const ymax = Math.max(STEP, Math.min(64, Math.ceil((defined.length ? Math.max(...defined) : 0) / STEP) * STEP));
  const clamp = (v: number) => Math.max(-ymax, Math.min(ymax, v));
  const x = (n: number) => L + (W - L - R) * n / len;
  const y = (v: number) => T + (H - T - B) * (1 - (v + ymax) / (2 * ymax));

  /* Gridline spacing: in a short band 8-disc lines collide — text
     never shrinks, so thin the spacing instead (16 -> 32 -> 64). */
  const rowStep = [1, 2, 4, 8].map(k => STEP * k)
    .find(s => (H - T - B) * s / (2 * ymax) >= 14) ?? STEP * 8;
  const rows: number[] = [];
  for (let v = -ymax; v <= ymax; v += rowStep) rows.push(v);
  const cols: number[] = [];
  for (let n = 0; n <= len; n += 10) cols.push(n);

  let d = '', pen = false;
  points.forEach((p, n) => {
    if (!p) return;
    d += (pen ? 'L' : 'M') + x(n).toFixed(1) + ' ' + y(clamp(p.value)).toFixed(1) + ' ';
    pen = true;
  });

  // Losing moves cluster near the end; the ~90px label flips to the
  // line's left when close to the right edge (else it clips).
  const bx = blunder ? x(blunder.at) : 0;
  const bRight = blunder ? bx > W - R - 100 : false;
  // If the losing move's value sits in the top half, the label goes to
  // the bottom (and vice versa) — blunders usually hug an edge.
  const bVal = blunder ? points[blunder.at]?.value : undefined;
  const bHigh = bVal !== undefined && bVal > 0;

  /** Ply from container x; the viewBox is wider than the container,
   *  so rescale by measured size. */
  const plyAt = (e: React.MouseEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const px = ((e.clientX - r.left) / r.width) * W;
    const n = Math.round(((px - L) / (W - L - R)) * len);
    return n >= 0 && n <= len ? n : null;
  };
  const shown = hover !== null ? points[hover] : undefined;
  const shownMove = hover !== null ? moveName?.(hover) : undefined;

  return (
    /* No display here: the collapsed tier (max-height 620) toggles it,
       and inline styles escape the media query — exactly the bug
       base.css warns about (toggle visible, graph stuck). */
    <div className={'k-graph' + (open ? ' k-open' : '')} style={{
      /* Shrinkable; this collapses before the board does, so never
         `none` (rule 7). */
      flex: '0 1 auto', minHeight: 0,
      borderTop: '1px solid var(--border)', background: 'var(--panel)',
      padding: 'var(--sp-3) var(--sp-4) var(--sp-4)', flexDirection: 'column', gap: 'var(--sp-2)',
    }}>
      {/* The legend stays outside the plot (heading row): overlaid it
          collides with data and shrinks unreadable. */}
      {/* Taller band when actions sit on it — at --h-head (20px) they
          sink into the rule (same story as Section's aside). */}
      <div style={{
        minHeight: extra ? 'var(--h-field)' : 'var(--h-head)',
        display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>
        <span>{title}</span>
        <Legend tone="gold">{t('data.src.book')}</Legend>
        <Legend tone="text">{t('data.src.solve')}</Legend>
        <Legend tone="accent">{t('data.src.search')}</Legend>
        {/* Hover readout in a fixed-width slot; variable width shoves
            the legend around. */}
        <span style={{ minWidth: 132, color: 'var(--text)', whiteSpace: 'nowrap' }}>
          {hover !== null && shown && <>
            {shownMove ? t('data.ply_move', { n: hover, move: shownMove }) : t('data.ply', { n: hover })}
            {' '}<b style={{ fontWeight: 600 }}>
              {shown.value > 0 ? '+' : ''}{shown.value.toFixed(1)}
            </b>
            <span style={{ color: shown.book ? 'var(--gold)' : 'var(--sub)' }}>
              {' '}{t(shown.book ? 'data.src.book' : shown.exact ? 'data.src.solve' : 'data.src.search')}
            </span>
          </>}
        </span>
        {extra && <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>{extra}</span>}
      </div>
      {/* Explicit base height: with svg at height:100% the content's
          height is 0, and auto keeps the band at minimum forever.
          Base = natural height, shrinkable to the floor. */}
      <div ref={attach} style={{
        position: 'relative', flexGrow: 0, flexShrink: 1,
        flexBasis: (scale ? NAT * scale : NAT) + 'px',
        minHeight: scale ? MIN * scale : MIN,
      }}>
        {/* Jump to the clicked ply; rescale from measured size. */}
        <svg viewBox={`0 0 ${W} ${H.toFixed(2)}`} preserveAspectRatio="none"
             style={{ width: '100%', height: '100%', display: 'block' }}
             role="img" aria-label={t('data.graph.aria')}
             onMouseMove={(e) => setHover(plyAt(e))}
             onMouseLeave={() => setHover(null)}
             onClick={onJump && ((e) => { const n = plyAt(e); if (n !== null) onJump(n); })}>
          <rect x={0} y={0} width={W} height={H} rx={8} fill="var(--bg)" />
          {/* The unit escapes to the top-left (it collides with +16 in
              the tick column). */}
          <text x={10} y={12} fill="var(--sub)" fontSize={10}>{t('data.graph.unit_discs')}</text>

          {rows.map(v => (
            <g key={'r' + v}>
              <line x1={L} y1={y(v)} x2={W - R} y2={y(v)}
                    stroke={v === 0 ? 'var(--border)' : 'var(--border-weak)'} strokeWidth={1}
                    strokeDasharray={v === 0 ? undefined : '2 3'} />
              <text x={L - 6} y={y(v) + 4} textAnchor="end" fill="var(--sub)" fontSize={11}>
                {v > 0 ? '+' + v : v === 0 ? '0' : '−' + -v}
              </text>
            </g>
          ))}
          {/* Edge labels swap with the viewpoint too; negating values
              alone makes white-view lines rise toward "black better". */}
          <text x={W - R + 8} y={y(ymax) + 12} fill="var(--sub)" fontSize={11}>{upLabel}</text>
          <text x={W - R + 8} y={y(0) + 4} fill="var(--sub)" fontSize={11}>{t('data.graph.even')}</text>
          <text x={W - R + 8} y={y(-ymax) - 6} fill="var(--sub)" fontSize={11}>{downLabel}</text>

          {cols.map(n => (
            <g key={'c' + n}>
              {n > 0 && <line x1={x(n)} y1={T} x2={x(n)} y2={H - B} stroke="var(--border-weak)" strokeWidth={1} />}
              <text x={x(n)} y={H - 8} textAnchor="middle" fill="var(--sub)" fontSize={11}>{n}</text>
            </g>
          ))}
          <text x={W - R + 8} y={H - 8} fill="var(--sub)" fontSize={10}>{t('data.graph.axis_moves')}</text>

          {d && <path d={d.trim()} fill="none" stroke="var(--accent)" strokeWidth={2} strokeLinejoin="round" />}

          {blunder && <>
            <line x1={bx} y1={T} x2={bx} y2={H - B} stroke="var(--bad)" strokeWidth={1.5} />
            {/* Pinned to the top it collides with hugging lines (big
                leads); flip to the line's other side — rule 52,
                applied vertically. */}
            <text x={bRight ? bx - 7 : bx + 7} y={bHigh ? H - B - 6 : T + 11}
                  textAnchor={bRight ? 'end' : 'start'} fill="var(--bad)" fontSize={11}>
              {t('data.blunder', { n: blunder.at, loss: blunder.loss })}
            </text>
          </>}
          {cursor !== undefined && (
            <line x1={x(cursor)} y1={T} x2={x(cursor)} y2={H - B}
                  stroke="var(--accent-dim)" strokeWidth={1} strokeDasharray="3 3" />
          )}

          {points.map((p, n) => p && (
            <circle key={n} cx={x(n)} cy={y(clamp(p.value))}
                    r={n === hover ? 5 : p.exact || p.book ? 4 : 3}
                    fill={p.book ? 'var(--gold)' : p.exact ? 'var(--text)' : 'var(--accent)'}
                    stroke="var(--bg)" strokeWidth={1} />
          ))}
          {/* Hover vline: thin solid, distinct from the current-move
              dashed --accent-dim line. */}
          {hover !== null && shown && (
            <line x1={x(hover)} y1={T} x2={x(hover)} y2={H - B}
                  stroke="var(--sub)" strokeWidth={1} />
          )}
        </svg>
        {/* A lineless graph must say why, or it reads as broken. */}
        {!d && (
          <div style={{
            position: 'absolute', inset: 0, display: 'grid', placeItems: 'center',
            fontSize: 'var(--fs-5)', color: 'var(--sub)',
          }}>{busy ? t('data.graph.analyzing') : t('data.graph.empty')}</div>
        )}
      </div>
    </div>
  );
}

function Legend({ tone, children }: { tone: 'gold' | 'text' | 'accent'; children: React.ReactNode }) {
  return (
    <span style={{ display: 'flex', alignItems: 'center', gap: 5 }}>
      <span style={{ width: 7, height: 7, borderRadius: '50%', background: `var(--${tone})` }} />
      {children}
    </span>
  );
}

/* Player row, flush against the board; shared by human, engine and
 * GGS. Height is --h-field (32px) everywhere so rows match across
 * screens.
 *
 * dev (±deviation) accompanies rate only for GGS — and there it is
 * mandatory, the number means nothing without it. Only GGS makes the
 * name clickable (profile). */
/* One row under the board with disc counts and clocks.
 *
 * Never one player above and one below: sandwiching the board costs
 * two rows of height (rule 77) and makes the eye cross the board.
 * Side-by-side numbers compare better; the turn is shown by
 * brightening the stone.
 *
 * Dimensions measured from the design — --h-bar (44px) band,
 * --border-weak top rule, --sp-4 sides, 20px gaps, 14px stones,
 * --fs-1 700 numbers. */
export function ScoreRow({ black, white, turn, meta, blackClock, whiteClock }: {
  black: number; white: number;
  /** Side to move; undefined after the game. */
  turn?: 'b' | 'w';
  meta?: React.ReactNode;
  blackClock?: string; whiteClock?: string;
}) {
  const side = (c: 'b' | 'w', n: number) => (
    <span style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
      opacity: turn === undefined || turn === c ? 1 : 0.55,
    }}>
      <StoneDot color={c} size={14} />
      <b style={{ fontSize: 'var(--fs-1)', fontWeight: 700, color: 'var(--text)' }}>{n}</b>
    </span>
  );
  return (
    <div style={{
      flex: 'none', height: 'var(--h-bar)', display: 'flex', alignItems: 'center',
      padding: '0 var(--sp-4)', gap: 20, borderTop: '1px solid var(--border-weak)',
      fontSize: 'var(--fs-5)', color: 'var(--sub)',
    }}>
      {side('b', black)}
      {side('w', white)}
      {meta && <>
        <Divider />
        <span>{meta}</span>
      </>}
      {/* Clocks rewrite every second; shifting digits shake the disc
          counts too. */}
      {(blackClock || whiteClock) && (
        <span style={{ marginLeft: 'auto', display: 'flex', gap: 'var(--sp-4)', fontVariantNumeric: 'tabular-nums' }}>
          {blackClock && <span>{t('data.color.black')} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{blackClock}</b></span>}
          {whiteClock && <span>{t('data.color.white')} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{whiteClock}</b></span>}
        </span>
      )}
    </div>
  );
}

export function PlayerRow({ color, name, rate, dev, meta, clock, active, discs, onName }: {
  color: StoneColor; name: string;
  rate?: number; dev?: number; meta?: React.ReactNode;
  clock?: string; active?: boolean; discs?: number;
  onName?: () => void;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)', fontSize: 'var(--fs-5)' }}>
      <StoneDot color={color} size={14} />
      {/* span when not clickable; a dead button still enters the Tab
          ring. */}
      {onName
        ? <button type="button" onClick={onName} className="k-link k-sel" style={{
            border: 0, background: 'transparent', padding: 0, fontSize: 'var(--fs-4)', fontWeight: 600, color: 'var(--text)',
            borderBottom: '1px solid color-mix(in srgb, var(--accent) 40%, transparent)',
          }}>{name}</button>
        : <b className="k-sel" style={{ fontSize: 'var(--fs-4)', fontWeight: 600 }}>{name}</b>}
      {rate !== undefined && (
        <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>
          {rate.toFixed(1)}{dev !== undefined && <span style={{ opacity: .7, marginLeft: 4 }}>±{dev}</span>}
        </span>
      )}
      {/* Disc count stays pinned next to the name; variable-width meta
          goes right, or the always-on count jitters. */}
      {discs !== undefined && <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700 }}>{discs}</span>}
      <span style={{ flex: 1 }} />
      {meta && <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{meta}</span>}
      {clock && <span style={{
        fontSize: 'var(--fs-4)', fontWeight: active ? 700 : 400,
        padding: '3px 12px', borderRadius: 'var(--r-pill)',
        background: active ? 'var(--accent)' : 'var(--bg)',
        color: active ? 'var(--on-accent)' : 'var(--sub)',
      }}>{clock}</span>}
    </div>
  );
}

/* Rating history; popover-only, never permanent.
 *
 * Up-or-down is all this place needs, so no axes. If ticks are ever
 * added, build them like EvalGraph (three bare gridlines pretending
 * to be an axis is the worst option). The current value belongs to
 * the adjacent RateRow.
 *
 * ⚠ Keep the viewBox width at or below the container's measured
 * width: wider shrinks the whole svg and 10px text renders at 6px
 * (the token floor is --fs-7 = 10px). This one assumes the 268px
 * dock, hence 300. */
export function RateChart({ points, height = 74, width = 300, axes, dates, labels, hover, onHover }: {
  points: number[];
  height?: number;
  /** viewBox width — match the container: at 300 in a wide container,
   *  preserveAspectRatio="none" stretches the stroke widths. */
  width?: number;
  /** Show axes (starring roles like the results screen); default off.
   *
   *  One component, not two: only the reading depth differs, and a
   *  separate RateChartFull would double-maintain colors and empty
   *  states (rules 50/56; settled with design 2026-08-10). */
  axes?: boolean;
  /** X labels, axes mode only; one per point. */
  dates?: string[];
  /** Hover text per point ("8/20 17:36 · piglet · +2"). */
  labels?: string[];
  /** Hovered index, owned outside — it must match the table below. */
  hover?: number | null;
  /** Hover callback for whoever links points to table rows. */
  onHover?: (i: number | null) => void;
}) {
  // Always empty for new players; one point cannot make a line.
  if (points.length < 2) {
    return (
      <div style={{ height, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        {t('data.rate.empty')}
      </div>
    );
  }
  /* Axes mode reserves label space: 4-digit ratings left, dates
     below. */
  const padL = axes ? 34 : 0, padB = axes ? 16 : 0;
  const w = width, pad = 8;
  const min = Math.min(...points), max = Math.max(...points), span = Math.max(1, max - min);
  const y = (p: number) =>
    height - pad - padB - ((p - min) / span) * (height - pad * 2 - 4 - padB);
  const x = (i: number) => padL + (i / (points.length - 1)) * (w - padL);
  const xy = points.map((p, i) => [x(i), y(p)] as const);
  const last = xy[xy.length - 1];
  /* Y step: a fixed step collapses — at 25 a 200-point swing crammed
     nine ticks into 180px. Pick a step yielding ~5 ticks. */
  const ticks: number[] = [];
  if (axes) {
    const step = [1, 2, 5, 10, 25, 50, 100, 200, 500, 1000].find((v) => span / v <= 5) ?? 2000;
    for (let v = Math.ceil(min / step) * step; v <= max; v += step) ticks.push(v);
  }
  /* Hovered point from viewBox coords, rescaled from measured size —
     `meet` adds side margins when aspect ratios differ, so dividing
     by container width drifts. */
  const pick = (e: React.MouseEvent<SVGSVGElement>): number => {
    const r = e.currentTarget.getBoundingClientRect();
    const scale = axes ? Math.min(r.width / w, r.height / height) : r.width / w;
    const ox = axes ? (r.width - w * scale) / 2 : 0;
    const vx = (e.clientX - r.left - ox) / scale;
    const i = Math.round(((vx - padL) / (w - padL)) * (points.length - 1));
    return Math.max(0, Math.min(points.length - 1, i));
  };
  const at = hover != null && hover >= 0 && hover < points.length ? hover : null;
  return (
    /* No gridlines (rule 56): axes are deliberately omitted here, and
       the rule names three bare gridlines as the worst pretense of an
       axis. Unlabeled lines look readable without being readable. */
    <svg viewBox={'0 0 ' + w + ' ' + height}
         preserveAspectRatio={axes ? 'xMidYMid meet' : 'none'}
         style={{ width: '100%', height, display: 'block' }}
         onMouseMove={onHover ? (e) => onHover(pick(e)) : undefined}
         onMouseLeave={onHover ? () => onHover(null) : undefined}>
      {ticks.map((tick) => (
        <g key={tick}>
          <line x1={padL} y1={y(tick)} x2={w} y2={y(tick)} stroke="var(--border-weak)" strokeWidth={1} />
          <text x={padL - 6} y={y(tick) + 3} textAnchor="end"
                fontSize={10} fill="var(--sub)">{tick}</text>
        </g>
      ))}
      {axes && dates && dates.length === points.length && dates.map((d, i) => (
        // Ends and center only; all of them would collide.
        (i === 0 || i === dates.length - 1 || i === (dates.length >> 1)) ? (
          <text key={i} x={x(i)} y={height - 3}
                textAnchor={i === 0 ? 'start' : i === dates.length - 1 ? 'end' : 'middle'}
                fontSize={10} fill="var(--sub)">{d}</text>
        ) : null
      ))}
      <polyline points={xy.map(([px, py]) => px.toFixed(1) + ',' + py.toFixed(1)).join(' ')} fill="none" stroke="var(--accent)" strokeWidth={1.6} />
      <circle cx={last[0]} cy={last[1]} r={3} fill="var(--accent)" />
      {at != null && (() => {
        const [hx, hy] = xy[at];
        const text = labels?.[at] ?? String(points[at]);
        // Text width is estimated (6px half / 11px full width); edge
        // labels nudge inward.
        const tw = [...text].reduce((n, c) => n + (c.charCodeAt(0) < 256 ? 6 : 11), 0) + 12;
        const bx = Math.min(Math.max(hx - tw / 2, padL), w - tw);
        return (
          <g pointerEvents="none">
            <line x1={hx} y1={pad} x2={hx} y2={height - padB} stroke="var(--sub)" strokeWidth={1} strokeDasharray="2 2" />
            <circle cx={hx} cy={hy} r={3.5} fill="var(--bg)" stroke="var(--accent)" strokeWidth={2} />
            {/* Ground --card, text --text. A wrong var name (--fg does
                not exist) falls back to SVG black on a dark ground. */}
            <rect x={bx} y={0} width={tw} height={18} rx={3} fill="var(--card)" stroke="var(--border)" />
            <text x={bx + 6} y={12.5} fontSize={11} fill="var(--text)">{text}</text>
          </g>
        );
      })()}
    </svg>
  );
}

export function ResultRow({ win, draw, opponent, discs, when, note, rating, dim, picked, onHover, onClick }: {
  win: boolean;
  /** Draw; rounding to win/lose turns 0-disc games into losses. */
  draw?: boolean;
  opponent: string;
  discs: number;
  /** End time (display string). */
  when?: string;
  /** Secondary info after the name (game format etc.). */
  note?: string;
  /** Own rating after the game. */
  rating?: number | null;
  /** A game with no record anywhere; visibly unclickable. */
  dim?: boolean;
  /** Game hovered in the graph — table and graph point at the same
   *  one. */
  picked?: boolean;
  /** Hover callback (lights the graph's dot). */
  onHover?: (on: boolean) => void;
  onClick?: () => void;
}) {
  const body = <>
    <span style={{ width: 24, flex: 'none', color: draw ? 'var(--sub)' : win ? 'var(--ok)' : 'var(--bad)' }}>
      {t(draw ? 'data.result.draw' : win ? 'data.result.win' : 'data.result.loss')}
    </span>
    <span className="k-sel" style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
      {opponent}
    </span>
    {note && (
      <span style={{ width: 'var(--w-gtype)', flex: 'none', color: 'var(--sub)',
                     fontSize: 'var(--fs-6)', overflow: 'hidden', textOverflow: 'ellipsis',
                     whiteSpace: 'nowrap' }}>{note}</span>
    )}
    <span style={{ width: 44, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                   fontVariantNumeric: 'tabular-nums' }}>
      {discs > 0 ? '+' + discs : discs}
    </span>
    {rating != null && (
      <span style={{ width: 60, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                     fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>
        {rating.toFixed(1)}
      </span>
    )}
    {when && (
      <span style={{ width: 64, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                     fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>{when}</span>
    )}
  </>;
  const style: React.CSSProperties = {
    display: 'flex', gap: 'var(--sp-2)', height: 'var(--h-field)', alignItems: 'center',
    width: '100%', fontSize: 'var(--fs-5)', borderBottom: '1px solid var(--border-weak)',
    padding: '0 var(--sp-2)', textAlign: 'left', color: 'var(--text)',
    // Selection styling is unified (layout.tsx's picked).
    ...(picked ? pickedStyle(true) : null),
  };
  const hov = onHover
    ? { onMouseEnter: () => onHover(true), onMouseLeave: () => onHover(false) }
    : {};
  // span when not clickable; a dead button still enters the Tab ring.
  return onClick && !dim
    ? <button type="button" className="k-row" onClick={onClick} title={t('data.result.open_in_study')} {...hov}
              style={{ ...style, border: 0, background: 'transparent', cursor: 'pointer' }}>{body}</button>
    : <div style={{ ...style, opacity: dim ? 0.5 : 1 }} {...hov}>{body}</div>;
}

export { Badge, Dot };

/* Move scrubber, full width under the board.
 *
 * The eval graph can jump too, but only after analysis; a freshly
 * loaded record had nothing but back/forward mashing.
 *
 * What looks draggable must drag — click and drag both land on the
 * same ply. */
export function MoveScrub({ plies, cursor, blunder, onSeek, nav = true }: {
  /** Total plies; 0 hides the strip. */
  plies: number;
  cursor: number;
  /** Losing move, marked red so it is findable without touching the
   *  board. */
  blunder?: { at: number; loss: number };
  onSeek: (n: number) => void;
  /** Show step buttons in the strip. false when the caller already
   *  has a step row (the record viewer's §9 layout). Default on —
   *  the strip alone loses single-step movement. */
  nav?: boolean;
}) {
  /* Step buttons sit at this strip's left edge (design §2: four 24px
     squares in the 32px strip, out of the toolbar). Navigation tools
     stay together — dragging and stepping are the same operation, and
     apart, ▶'s effect appears somewhere else. */
  const box = React.useRef<HTMLDivElement>(null);
  if (plies <= 0) return null;

  const at = (n: number) => (n / plies) * 100;
  // Snap click/drag to the nearest ply; ends clamp to 0 and plies.
  const seekAt = (clientX: number) => {
    const r = box.current?.getBoundingClientRect();
    if (!r || r.width <= 0) return;
    const t = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    onSeek(Math.round(t * plies));
  };

  // Ticks every ply; every 10th is longer and numbered.
  const ticks = Array.from({ length: plies + 1 }, (_, i) => i);

  const step = (n: number) => onSeek(Math.max(0, Math.min(plies, n)));

  return (
    <div style={{
      padding: '0 var(--sp-4)', flex: 'none',
      display: 'flex', alignItems: 'center', gap: 'var(--sp-2h)',
    }}>
      {nav && (
        <span style={{ display: 'flex', gap: 4, flex: 'none' }}>
          <Button square size="row" title={t('data.scrub.first')} disabled={cursor === 0} onClick={() => step(0)}>|◀</Button>
          <Button square size="row" title={t('data.scrub.prev')} disabled={cursor === 0} onClick={() => step(cursor - 1)}>◀</Button>
          <Button square size="row" title={t('data.scrub.next')} disabled={cursor >= plies} onClick={() => step(cursor + 1)}>▶</Button>
          <Button square size="row" title={t('data.scrub.last')} disabled={cursor >= plies} onClick={() => step(plies)}>▶|</Button>
        </span>
      )}
      <div ref={box} role="slider" aria-label={t('data.scrub.aria')} aria-valuemin={0}
           aria-valuemax={plies} aria-valuenow={cursor} tabIndex={0}
           onPointerDown={(e) => {
             (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
             seekAt(e.clientX);
           }}
           onPointerMove={(e) => { if (e.buttons & 1) seekAt(e.clientX); }}
           style={{
             // Height is --h-field; the every-10 numbers live outside
             // (the 12px row below), so ticks and the 16px thumb fit
             // in 32px.
             position: 'relative', height: 'var(--h-field)', flex: 1, minWidth: 0,
             cursor: 'pointer', touchAction: 'none', userSelect: 'none',
           }}>
        {/* The groove and the filled-so-far part. */}
        <div style={{
          position: 'absolute', left: 0, right: 0, top: 11, height: 2,
          background: 'var(--track)', borderRadius: 1,
        }} />
        <div style={{
          position: 'absolute', left: 0, width: at(cursor) + '%', top: 11, height: 2,
          background: 'var(--accent)', borderRadius: 1,
        }} />

        {ticks.map((i) => {
          const ten = i % 10 === 0;
          return (
            <div key={i} style={{
              position: 'absolute', left: at(i) + '%', top: ten ? 6 : 8,
              width: 1, height: ten ? 12 : 8, marginLeft: -0.5,
              background: ten ? 'var(--border)' : 'var(--border-weak)',
            }} />
          );
        })}

        {/* Losing move: thicker, red, extended upward. */}
        {blunder && blunder.at <= plies && (
          <div title={t('data.blunder', { n: blunder.at, loss: blunder.loss })} style={{
            position: 'absolute', left: at(blunder.at) + '%', top: 3,
            width: 2, height: 18, marginLeft: -1, background: 'var(--bad)',
          }} />
        )}

        {ticks.filter((i) => i % 10 === 0).map((i) => (
          <span key={'n' + i} style={{
            position: 'absolute', left: at(i) + '%', top: 20,
            transform: 'translateX(-50%)',
            fontSize: 'var(--fs-7)', color: 'var(--sub)', lineHeight: 1,
          }}>{i}</span>
        ))}

        {/* The thumb: stone-like coloring would read as a board piece,
            so --card fill + --accent ring says "control". */}
        <div style={{
          position: 'absolute', left: at(cursor) + '%', top: 4,
          width: 16, height: 16, marginLeft: -8, borderRadius: '50%',
          background: 'var(--card)', border: '2px solid var(--accent)',
          boxShadow: 'var(--sh-1)', pointerEvents: 'none',
        }} />
      </div>
    </div>
  );
}

/* ---------------- Reported-eval trend ----------------
 *
 * Both players' "how many discs I think I'm winning by", side by side.
 *
 * Both lines are normalized to MY viewpoint: reported values are from
 * the mover's side, and raw overlay makes mirror images you must flip
 * in your head. Normalized, the gap between lines IS where the
 * players disagree.
 *
 * If ticks exist they must be readable (rule 56). Y is disc-diff
 * ticks, X is plies, hover shows both values at that move. A
 * two-bare-lines version shipped once; never again. */
export function EvalTrend({ points, height = 96 }: {
  /** `{ x: ply, mine: value | null, opp: value | null }` (own view). */
  points: { x: number; mine: number | null; opp: number | null }[];
  height?: number;
}) {
  const [at, setAt] = React.useState<number | null>(null);
  const has = points.some((p) => p.mine != null || p.opp != null);
  // Draw even a single point; dots removed the two-point wait.
  if (!points.length || !has) {
    return (
      <div style={{
        height, display: 'grid', placeItems: 'center',
        fontSize: 'var(--fs-7)', color: 'var(--sub)',
      }}>
        {t('data.trend.empty')}
      </div>
    );
  }
  const w = 300, pad = 10, padL = 26, padB = 14;
  /* Y always brackets 0 — with the win/lose boundary off-frame you
     cannot tell which side a line is on. Show at least ±8 discs to
     damp jitter; pick a ~5-tick step (same idea as RateChart). */
  const vals = points.flatMap((p) => [p.mine, p.opp]).filter((v): v is number => v != null);
  const lim = Math.max(8, ...vals.map((v) => Math.abs(v)));
  const step = [2, 4, 8, 16, 32].find((v) => lim / v <= 2.5) ?? 32;
  const ticks: number[] = [];
  for (let v = -Math.floor(lim / step) * step; v <= lim; v += step) ticks.push(v);

  const span = Math.max(1, points.length - 1);
  const x = (i: number) => padL + (i / span) * (w - padL - pad);
  const y = (v: number) => (height - padB) / 2 - (v / lim) * ((height - padB) / 2 - pad);

  /** Connect only the plies that player reported.
   *
   * Never cut the line at missing values: turns alternate, so each
   * side has values only every other ply — cutting left nothing but
   * dots. Skip the gaps and connect that player's own reports. */
  const path = (pick: (p: (typeof points)[number]) => number | null) => {
    let d = '';
    let pen = false;
    points.forEach((p, i) => {
      const v = pick(p);
      if (v == null) return;
      d += `${pen ? 'L' : 'M'}${x(i).toFixed(1)},${y(v).toFixed(1)}`;
      pen = true;
    });
    return d;
  };

  /** Hovered ply from viewBox coords (preserveAspectRatio="none"). */
  const pickAt = (e: React.MouseEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const vx = ((e.clientX - r.left) / r.width) * w;
    const i = Math.round(((vx - padL) / (w - padL - pad)) * (points.length - 1));
    setAt(Math.max(0, Math.min(points.length - 1, i)));
  };

  const cur = at != null ? points[at] : null;
  const fmt = (v: number | null) => (v == null ? '—' : (v > 0 ? '+' : '') + v.toFixed(1));

  return (
    <div>
      {/* The legend is required; never make color alone say who is
          who. */}
      <div style={{
        display: 'flex', gap: 'var(--sp-3)', alignItems: 'center',
        fontSize: 'var(--fs-7)', color: 'var(--sub)', height: 'var(--h-head)',
      }}>
        <span>{t('data.trend.title')}</span>
        <span style={{ marginLeft: 'auto', display: 'inline-flex', gap: 'var(--sp-3)' }}>
          <span><span style={{ color: 'var(--accent)' }}>—</span> {t('data.trend.mine')} {cur ? fmt(cur.mine) : ''}</span>
          <span><span style={{ color: 'var(--sub)' }}>—</span> {t('data.trend.opp')} {cur ? fmt(cur.opp) : ''}</span>
          <span>{cur ? t('data.ply', { n: cur.x }) : t('data.trend.hint')}</span>
        </span>
      </div>
      <svg viewBox={`0 0 ${w} ${height}`} preserveAspectRatio="none"
           style={{ width: '100%', height, display: 'block' }}
           onMouseMove={pickAt} onMouseLeave={() => setAt(null)}>
        {ticks.map((tick) => (
          <g key={tick}>
            <line x1={padL} y1={y(tick)} x2={w - pad} y2={y(tick)}
                  stroke={tick === 0 ? 'var(--line)' : 'var(--border-weak)'} strokeWidth={1} />
            <text x={padL - 4} y={y(tick) + 3} textAnchor="end"
                  fontSize={9} fill="var(--sub)">{tick > 0 ? `+${tick}` : tick}</text>
          </g>
        ))}
        {/* X labels at ends and center only. */}
        {[0, points.length >> 1, points.length - 1].map((i, k) => (
          <text key={k} x={x(i)} y={height - 3}
                textAnchor={k === 0 ? 'start' : k === 2 ? 'end' : 'middle'}
                fontSize={9} fill="var(--sub)">{points[i].x}</text>
        ))}
        <path d={path((p) => p.opp)} fill="none" stroke="var(--sub)"
              strokeWidth={1.5} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
        <path d={path((p) => p.mine)} fill="none" stroke="var(--accent)"
              strokeWidth={1.5} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
        {/* Dots too: a line needs two points, so the opening plies
            would render nothing (own line absent until the second own
            report = blank through ply 4). Dots show from the first
            report. */}
        {points.map((p, i) => (
          <g key={i}>
            {p.opp != null && (
              <circle cx={x(i)} cy={y(p.opp)} r={1.6} fill="var(--sub)" />
            )}
            {p.mine != null && (
              <circle cx={x(i)} cy={y(p.mine)} r={1.6} fill="var(--accent)" />
            )}
          </g>
        ))}
        {at != null && (
          <g pointerEvents="none">
            <line x1={x(at)} y1={pad} x2={x(at)} y2={height - padB}
                  stroke="var(--sub)" strokeWidth={1} strokeDasharray="2 2" />
            {points[at].opp != null && (
              <circle cx={x(at)} cy={y(points[at].opp)} r={3}
                      fill="var(--bg)" stroke="var(--sub)" strokeWidth={2} />
            )}
            {points[at].mine != null && (
              <circle cx={x(at)} cy={y(points[at].mine)} r={3}
                      fill="var(--bg)" stroke="var(--accent)" strokeWidth={2} />
            )}
          </g>
        )}
      </svg>
    </div>
  );
}
