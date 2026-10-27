import React from 'react';
import { Button } from './primitives';
import { Col, Divider, Empty, TableHead, TableRow, picked as pickedStyle } from './layout';
import { t } from '../i18n';


export type StoneColor = 'b' | 'w';

export type Move = {
  n: number; move: string; color: StoneColor;
  score?: number; loss?: number; secs?: number;
  src?: string;
  pass?: boolean;
};

export type MoveSrc = 'book' | 'book_learned' | 'search' | 'solve';

const SRC_KEY: Record<string, string> = {
  book: 'data.src.book',
  book_learned: 'data.src.book_learned',
  search: 'data.src.search',
  solve: 'data.src.solve',
};

export const srcLabel = (src: string): string => (SRC_KEY[src] ? t(SRC_KEY[src]) : src);
export const srcIsBook = (src: string): boolean => src === 'book' || src === 'book_learned';
const srcIsSolve = (src: string): boolean => src === 'solve';

const moveCols = (): Col[] => [
  { head: '#', w: 22, right: true },
  { head: t('data.moves.header_move'), w: 58 },
  { head: t('data.moves.header_eval'), w: 56, right: true, num: true },
  { head: t('data.moves.header_time'), w: 34, right: true, num: true },
  { head: t('data.moves.header_source'), right: true, clip: true },
];

export function KifuTable({ moves, current, onSelect, decimals }: {
  moves: Move[]; current?: number; onSelect?: (n: number) => void;
  decimals: number;
}) {
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
        {!moves.length && (
          <span style={{ display: 'block', padding: '0 var(--sp-3)' }}>
            <Empty>{t('data.moves.empty')}</Empty>
          </span>
        )}
        {moves.map(m => {
          const played = m.score !== undefined;
          const isCurrent = m.n === current;
          return (
            <TableRow key={m.n} cols={cols} on={isCurrent} muted={!played}
                      innerRef={isCurrent ? row : undefined}
                      onClick={() => onSelect?.(m.n)}>
              <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{m.n}</span>
              <span style={{ fontFamily: 'var(--ff-mono)' }}>
                <StoneDot color={m.color} />
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

export function StoneDot({ color, size = 9 }: { color: StoneColor; size?: number }) {
  const black = color === 'b';
  return <span style={{
    width: size, height: size, borderRadius: '50%', flex: 'none',
    // A black mark gets a soft highlight, so on a near-black ground it reads as a filled stone,
    // not as an empty ring.
    background: black ? 'radial-gradient(circle at 35% 30%, var(--stone-black-dot-hi), var(--stone-black) 70%)' : 'var(--stone-white)',
    boxShadow: 'inset 0 0 0 1px ' + (black ? 'var(--stone-black-dot-edge)' : 'var(--stone-white-edge)'),
  }} />;
}

export type GraphPoint = { value: number; exact?: boolean; book?: boolean };

export function EvalGraph({ points, plies, cursor, blunder, busy, pov, extra, onJump, moveName, open }: {
  points: (GraphPoint | undefined)[];
  plies?: number;
  cursor?: number;
  blunder?: { at: number; loss: number };
  busy?: boolean;
  pov: 'b' | 'w';
  extra: React.ReactNode;
  onJump: (n: number) => void;
  moveName: (n: number) => string | undefined;
  open?: boolean;
}) {
  const [hover, setHover] = React.useState<number | null>(null);
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

  const black = pov === 'b';
  const title = t(black ? 'data.graph.title_black_view' : 'data.graph.title_white_view');
  const upLabel = t(black ? 'data.graph.black_ahead' : 'data.graph.white_ahead');
  const downLabel = t(black ? 'data.graph.white_ahead' : 'data.graph.black_ahead');
  const labelW = (s: string) =>
    [...s].reduce((n, ch) => n + (ch.charCodeAt(0) < 0x2e80 ? 6.1 : 11), 0);
  const W = 800, L = 44, T = 18, B = 26, STEP = 8;
  const R = Math.max(54, Math.ceil(Math.max(
    labelW(upLabel), labelW(downLabel), labelW(t('data.graph.even')),
  )) + 14);
  const NAT = 210, MIN = 120;
  const scale = box.w > 0 ? box.w / W : 0;
  const H = scale > 0 ? box.h / scale : NAT;
  const len = Math.max(1, plies ?? points.length - 1);
  const defined = points.filter((p): p is GraphPoint => !!p).map(p => Math.abs(p.value));
  const ymax = Math.max(STEP, Math.min(64, Math.ceil((defined.length ? Math.max(...defined) : 0) / STEP) * STEP));
  const clamp = (v: number) => Math.max(-ymax, Math.min(ymax, v));
  const x = (n: number) => L + (W - L - R) * n / len;
  const y = (v: number) => T + (H - T - B) * (1 - (v + ymax) / (2 * ymax));

  const rowStep = [1, 2, 4, 8].map(k => STEP * k)
    .find(s => (H - T - B) * s / (2 * ymax) >= 14) ?? STEP * 8;
  const rows: number[] = [];
  for (let v = -ymax; v <= ymax; v += rowStep) rows.push(v);
  const cols: number[] = [];
  for (let n = 0; n <= len; n += 10) cols.push(n);

  let d = '';
  let prev: { n: number; p: GraphPoint } | null = null;
  points.forEach((p, n) => {
    if (!p) return;
    const at = (k: number, v: number) => x(k).toFixed(1) + ' ' + y(clamp(v)).toFixed(1);
    if (prev) {
      d += 'M' + at(prev.n, prev.p.value) + ' L' + at(n, p.value) + ' ';
    }
    prev = { n, p };
  });
  const drawn = prev !== null;

  const bx = blunder ? x(blunder.at) : 0;
  const bRight = blunder ? bx > W - R - 100 : false;
  const bVal = blunder ? points[blunder.at]?.value : undefined;
  const bHigh = bVal !== undefined && bVal > 0;

  const plyAt = (e: React.MouseEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const px = ((e.clientX - r.left) / r.width) * W;
    const n = Math.round(((px - L) / (W - L - R)) * len);
    return n >= 0 && n <= len ? n : null;
  };
  const shown = hover !== null ? points[hover] : undefined;
  const shownMove = hover !== null ? moveName(hover) : undefined;

  return (
    <div className={'k-graph' + (open ? ' k-open' : '')} style={{
      flex: '0 1 auto', minHeight: 0,
      borderTop: '1px solid var(--border)', background: 'var(--panel)',
      padding: 'var(--sp-3) var(--sp-4) var(--sp-4)', flexDirection: 'column', gap: 'var(--sp-2)',
    }}>
      {/* A narrow graph wraps whole items onto a second line; no label breaks inside itself. */}
      <div style={{
        minHeight: 'var(--h-field)',
        display: 'flex', flexWrap: 'wrap', alignItems: 'center', columnGap: 'var(--sp-3)', rowGap: 'var(--sp-1)',
        fontSize: 'var(--fs-6)', color: 'var(--sub)', whiteSpace: 'nowrap',
      }}>
        <span>{title}</span>
        <Legend tone="gold">{t('data.src.book')}</Legend>
        <Legend tone="text">{t('data.src.solve')}</Legend>
        <Legend tone="accent">{t('data.src.search')}</Legend>
        <span style={{ minWidth: 132, color: 'var(--text)', whiteSpace: 'nowrap' }}>
          {/* Every move under the pointer reads out, one without a value as a dash. */}
          {hover !== null && drawn && <>
            {shownMove ? t('data.ply_move', { n: hover, move: shownMove }) : t('data.ply', { n: hover })}
            {' '}<b style={{ fontWeight: 600 }}>
              {shown ? (shown.value > 0 ? '+' : '') + shown.value.toFixed(1) : '—'}
            </b>
            {shown && (
              <span style={{ color: shown.book ? 'var(--gold)' : 'var(--sub)' }}>
                {' '}{t(shown.book ? 'data.src.book' : shown.exact ? 'data.src.solve'
                  : 'data.src.search')}
              </span>
            )}
          </>}
        </span>
        <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>{extra}</span>
      </div>
      <div ref={attach} style={{
        position: 'relative', flexGrow: 0, flexShrink: 1,
        flexBasis: (scale ? NAT * scale : NAT) + 'px',
        minHeight: scale ? MIN * scale : MIN,
      }}>
        <svg viewBox={`0 0 ${W} ${H.toFixed(2)}`} preserveAspectRatio="none"
             style={{ width: '100%', height: '100%', display: 'block' }}
             role="img" aria-label={t('data.graph.aria')}
             onMouseMove={(e) => setHover(plyAt(e))}
             onMouseLeave={() => setHover(null)}
             onClick={(e) => { const n = plyAt(e); if (n !== null) onJump(n); }}>
          <rect x={0} y={0} width={W} height={H} rx={8} fill="var(--bg)" />
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
          {hover !== null && drawn && (
            <line x1={x(hover)} y1={T} x2={x(hover)} y2={H - B}
                  stroke="var(--sub)" strokeWidth={1} />
          )}
        </svg>
        {!drawn && (
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
      <span style={{ flex: 'none', width: 7, height: 7, borderRadius: '50%', background: `var(--${tone})` }} />
      {children}
    </span>
  );
}

export function ScoreRow({ black, white, turn, meta, blackClock, whiteClock }: {
  black: number; white: number;
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
      {(blackClock || whiteClock) && (
        <span style={{ marginLeft: 'auto', display: 'flex', gap: 'var(--sp-4)', fontVariantNumeric: 'tabular-nums' }}>
          {blackClock && <span>{t('data.color.black')} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{blackClock}</b></span>}
          {whiteClock && <span>{t('data.color.white')} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{whiteClock}</b></span>}
        </span>
      )}
    </div>
  );
}

export function PlayerRow({ color, name, rate, meta, clock, active }: {
  color: StoneColor; name: string;
  rate?: number; meta?: React.ReactNode;
  clock?: string; active?: boolean;
}) {
  return (
    // One line at every width: the note, then the name give way (ellipsis); the clock never does.
    <div style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-field)', minWidth: 0,
      fontSize: 'var(--fs-5)', lineHeight: 1.2, whiteSpace: 'nowrap',
    }}>
      <span style={{ flex: 'none', display: 'flex' }}><StoneDot color={color} size={14} /></span>
      <b className="k-sel" style={{ fontSize: 'var(--fs-4)', fontWeight: 600, flex: '0 1 auto', minWidth: '3em', overflow: 'hidden', textOverflow: 'ellipsis' }}>{name}</b>
      {rate !== undefined && (
        <span style={{ flex: 'none', color: 'var(--sub)', fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>
          {rate.toFixed(1)}
        </span>
      )}
      {/* The side to move gets its own mark, so the clock can keep one look for both sides. */}
      {active && (
        <span style={{
          flex: 'none', display: 'inline-flex', alignItems: 'center', height: 'var(--h-chip)',
          // accent-dim, as a primary button: white on the bright accent is 2.7:1 in dark.
          padding: '0 var(--sp-2)', borderRadius: 'var(--r-1)', background: 'var(--accent-dim)',
          color: 'var(--on-accent)', fontSize: 'var(--fs-7)', fontWeight: 600,
        }}>{t('data.to_move')}</span>
      )}
      <span style={{ flex: 1, minWidth: 'var(--sp-2)' }} />
      {/* The note gives way before the name does. */}
      {meta && <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)', flex: '0 1000 auto', minWidth: '2.5em', overflow: 'hidden', textOverflow: 'ellipsis' }}>{meta}</span>}
      {clock && <span style={{
        flex: 'none', display: 'inline-flex', alignItems: 'center', height: 'var(--h-chip)',
        fontSize: 'var(--fs-4)', fontWeight: 600, fontVariantNumeric: 'tabular-nums',
        padding: '0 var(--sp-3)', borderRadius: 'var(--r-pill)',
        background: 'var(--card)', color: 'var(--text)',
        border: '1px solid ' + (active ? 'var(--accent)' : 'var(--border)'),
      }}>{clock}</span>}
    </div>
  );
}

/** About how wide a line of SVG text is at `size` px: Latin narrow, CJK full width. */
const textW = (s: string, size: number): number =>
  [...s].reduce((n, c) => n + (c.charCodeAt(0) < 0x2e80 ? size * 0.58 : size), 0);

export function RateChart({ points, height, width, dates, labels, hover, onHover }: {
  points: number[];
  height: number;
  width: number;
  dates: string[];
  labels: string[];
  hover: number | null;
  onHover: (i: number | null) => void;
}) {
  if (points.length < 2) {
    return (
      <div style={{ height, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        {t('data.rate.empty')}
      </div>
    );
  }
  const padL = 34, padB = 16;
  const w = width, pad = 8;
  const min = Math.min(...points), max = Math.max(...points), span = Math.max(1, max - min);
  const y = (p: number) =>
    height - pad - padB - ((p - min) / span) * (height - pad * 2 - 4 - padB);
  // The last point sits a dot's radius in from the edge, so it is not cut in half.
  const spanX = w - padL - 4;
  const x = (i: number) => padL + (i / (points.length - 1)) * spanX;
  const xy = points.map((p, i) => [x(i), y(p)] as const);
  const last = xy[xy.length - 1];
  const ticks: number[] = [];
  const step = [1, 2, 5, 10, 25, 50, 100, 200, 500, 1000].find((v) => span / v <= 5) ?? 2000;
  for (let v = Math.ceil(min / step) * step; v <= max; v += step) ticks.push(v);
  const pick = (e: React.MouseEvent<SVGSVGElement>): number => {
    const r = e.currentTarget.getBoundingClientRect();
    const scale = Math.min(r.width / w, r.height / height);
    const ox = (r.width - w * scale) / 2;
    const vx = (e.clientX - r.left - ox) / scale;
    const i = Math.round(((vx - padL) / spanX) * (points.length - 1));
    return Math.max(0, Math.min(points.length - 1, i));
  };
  const at = hover !== null && hover >= 0 && hover < points.length ? hover : null;
  return (
    <svg viewBox={'0 0 ' + w + ' ' + height}
         preserveAspectRatio="xMidYMid meet"
         style={{ width: '100%', height, display: 'block' }}
         onMouseMove={(e) => onHover(pick(e))}
         onMouseLeave={() => onHover(null)}>
      {ticks.map((tick) => (
        <g key={tick}>
          <line x1={padL} y1={y(tick)} x2={w} y2={y(tick)} stroke="var(--border-weak)" strokeWidth={1} />
          <text x={padL - 6} y={y(tick) + 3} textAnchor="end"
                fontSize={10} fill="var(--sub)">{tick}</text>
        </g>
      ))}
      {dates.length === points.length && dates.map((d, i) => (
        // First, middle and last; a label the same as the one before it says nothing new.
        ([0, dates.length >> 1, dates.length - 1].includes(i)
          && (i === 0 || d !== dates[i === dates.length - 1 ? dates.length >> 1 : 0])) ? (
          <text key={i} x={x(i)} y={height - 3}
                textAnchor={i === 0 ? 'start' : i === dates.length - 1 ? 'end' : 'middle'}
                fontSize={10} fill="var(--sub)">{d}</text>
        ) : null
      ))}
      <polyline points={xy.map(([px, py]) => px.toFixed(1) + ',' + py.toFixed(1)).join(' ')} fill="none" stroke="var(--accent)" strokeWidth={1.6} />
      <circle cx={last[0]} cy={last[1]} r={3} fill="var(--accent)" />
      {at != null && (() => {
        const [hx, hy] = xy[at];
        const text = labels[at] ?? String(points[at]);
        const tw = textW(text, 11) + 12;
        const bx = Math.min(Math.max(hx - tw / 2, padL), w - tw);
        // The box sits at the top, or under the point when the point is up there itself.
        const by = hy < 26 ? hy + 8 : 0;
        return (
          <g pointerEvents="none">
            <line x1={hx} y1={pad} x2={hx} y2={height - padB} stroke="var(--sub)" strokeWidth={1} strokeDasharray="2 2" />
            <circle cx={hx} cy={hy} r={3.5} fill="var(--bg)" stroke="var(--accent)" strokeWidth={2} />
            <rect x={bx} y={by} width={tw} height={18} rx={3} fill="var(--card)" stroke="var(--border)" />
            <text x={bx + 6} y={by + 12.5} fontSize={11} fill="var(--text)">{text}</text>
          </g>
        );
      })()}
    </svg>
  );
}

const W_OUTCOME = 28, W_DISCS = 44, W_RATING = 60, W_WHEN = 64;
// Table cells never wrap: a narrow column turns a label into a vertical stack of characters.
const ONE_LINE: React.CSSProperties = { whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' };

export function ResultHead({ note, rating }: {
  note?: boolean; rating?: boolean;
}) {
  const cell = (w: number): React.CSSProperties => ({
    width: w, flex: 'none', textAlign: 'right', ...ONE_LINE,
  });
  return (
    <TableHead pad="var(--sp-2)">
      <span style={{ width: W_OUTCOME, flex: 'none', ...ONE_LINE }}>{t('data.result.head_outcome')}</span>
      <span style={{ flex: 1, minWidth: 0, ...ONE_LINE }}>{t('data.result.head_opponent')}</span>
      {note && <span style={{ width: 'var(--w-gtype)', flex: 'none', ...ONE_LINE }}>{t('data.result.head_kind')}</span>}
      <span style={cell(W_DISCS)}>{t('data.result.head_discs')}</span>
      {rating && <span style={cell(W_RATING)}>{t('data.result.head_rating')}</span>}
      <span style={cell(W_WHEN)}>{t('data.result.head_date')}</span>
    </TableHead>
  );
}

export function ResultRow({ win, draw, adjourned, opponent, discs, when, note, rating, dim, picked, onHover, onClick }: {
  win: boolean;
  draw?: boolean;
  adjourned?: boolean;
  opponent: string;
  discs: number;
  when?: string;
  note?: string;
  /** The rating column: undefined = the table has none; null = this game has no rating. */
  rating?: number | null;
  dim?: boolean;
  picked?: boolean;
  onHover?: (on: boolean) => void;
  onClick?: () => void;
}) {
  const body = <>
    <span style={{ width: W_OUTCOME, flex: 'none', ...ONE_LINE,
                   color: adjourned || draw ? 'var(--sub)' : win ? 'var(--ok)' : 'var(--bad)' }}>
      {t(adjourned ? 'data.result.adjourned'
         : draw ? 'data.result.draw' : win ? 'data.result.win' : 'data.result.loss')}
    </span>
    <span className="k-sel" style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
      {opponent}
    </span>
    {note && (
      <span style={{ width: 'var(--w-gtype)', flex: 'none', color: 'var(--sub)',
                     fontSize: 'var(--fs-6)', overflow: 'hidden', textOverflow: 'ellipsis',
                     whiteSpace: 'nowrap' }}>{note}</span>
    )}
    <span style={{ width: W_DISCS, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                   fontVariantNumeric: 'tabular-nums', ...ONE_LINE }}>
      {adjourned ? '\u2014' : discs > 0 ? '+' + discs : discs}
    </span>
    {rating !== undefined && (
      <span style={{ width: W_RATING, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                     fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums', ...ONE_LINE }}>
        {rating == null ? '' : rating.toFixed(1)}
      </span>
    )}
    <span style={{ width: W_WHEN, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                   fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums', ...ONE_LINE }}>{when}</span>
  </>;
  const style: React.CSSProperties = {
    display: 'flex', gap: 'var(--sp-2)', height: 'var(--h-field)', alignItems: 'center',
    width: '100%', fontSize: 'var(--fs-5)', borderBottom: '1px solid var(--border-weak)',
    padding: '0 var(--sp-2)', textAlign: 'left', color: 'var(--text)',
    ...(picked ? pickedStyle(true) : null),
  };
  const hov = onHover
    ? { onMouseEnter: () => onHover(true), onMouseLeave: () => onHover(false) }
    : {};
  return onClick && !dim
    ? <button type="button" className="k-row" onClick={onClick} {...hov}
              style={{ background: 'transparent', ...style, border: 0, cursor: 'pointer' }}>{body}</button>
    : <div style={{ ...style, opacity: dim ? 0.5 : 1 }} {...hov}>{body}</div>;
}

export function MoveScrub({ plies, cursor, blunder, onSeek, nav = true }: {
  plies: number;
  cursor: number;
  blunder?: { at: number; loss: number };
  onSeek: (n: number) => void;
  nav?: boolean;
}) {
  const box = React.useRef<HTMLDivElement>(null);
  if (plies <= 0) return null;

  const at = (n: number) => (n / plies) * 100;
  const seekAt = (clientX: number) => {
    const r = box.current?.getBoundingClientRect();
    if (!r || r.width <= 0) return;
    const t = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    onSeek(Math.round(t * plies));
  };

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
             position: 'relative', height: 'var(--h-field)', flex: 1, minWidth: 0,
             cursor: 'pointer', touchAction: 'none', userSelect: 'none',
           }}>
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

export function EvalTrend({ points, plies, height = 96, title, split, names }: {
  points: {
    x: number; mine: number | null; opp: number | null;
    mySecs?: number | null; oppSecs?: number | null;
  }[];
  plies?: number;
  height?: number;
  title?: string;
  /** The move at which a synchro pair's two boards first played differently. */
  split?: number;
  /** Player names for the two lines (the graph never says "me": GGS screens name everyone). */
  names: { mine: string; opp: string };
}) {
  const [at, setAt] = React.useState<number | null>(null);
  const [w, setW] = React.useState(300);
  const obs = React.useRef<ResizeObserver | null>(null);
  const attach = React.useCallback((el: SVGSVGElement | null) => {
    obs.current?.disconnect();
    obs.current = null;
    if (!el) return;
    const o = new ResizeObserver(() => {
      const cw = el.getBoundingClientRect().width;
      if (cw > 0) setW((v) => (Math.abs(v - cw) < 0.5 ? v : cw));
    });
    o.observe(el);
    obs.current = o;
  }, []);
  const pad = 10, padL = 26, padB = 14, padR = 30;
  const vals = points.flatMap((p) => [p.mine, p.opp]).filter((v): v is number => v != null);
  const lim = Math.max(8, ...vals.map((v) => Math.abs(v)));
  const step = [2, 4, 8, 16, 32].find((v) => lim / v <= 2.5) ?? 32;
  const ticks: number[] = [];
  for (let v = -Math.floor(lim / step) * step; v <= lim; v += step) ticks.push(v);

  // Move n sits at n steps from the left edge, which is move 0; the axis is labelled every 5 moves.
  // Each point goes where its own move number says, so a series that starts late or skips a move
  // still lines up with the axis and with what the pointer reads.
  const slots = Math.max(1, plies ?? 0, ...points.map((p) => p.x));
  const slot = (w - padL - padR) / slots;
  const xAt = (n: number) => padL + n * slot;
  const x = (i: number) => xAt(points[i].x);
  const marks = Array.from({ length: Math.floor(slots / 5) + 1 }, (_, k) => k * 5);
  const maxSecs = Math.max(0, ...points.flatMap((p) => [p.mySecs ?? 0, p.oppSecs ?? 0]));
  const secsTop = [1, 2, 5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 240, 300, 420, 600]
    .find((v) => v >= maxSecs) ?? Math.ceil(maxSecs / 60) * 60;
  // Think time shares the eval frame: mine rises from the 0 line, the opponent's hangs below it.
  const barH = (s: number) => (s / secsTop) * ((height - padB) / 2 - pad);
  const y = (v: number) => (height - padB) / 2 - (v / lim) * ((height - padB) / 2 - pad);
  // A short graph keeps every grid line but labels only as many as fit without touching.
  const every = Math.max(1, Math.ceil(14 / Math.max(1, y(0) - y(step))));

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

  const pickAt = (e: React.MouseEvent<SVGSVGElement>) => {
    if (!points.length) return;
    const r = e.currentTarget.getBoundingClientRect();
    const n = (((e.clientX - r.left) / r.width) * w - padL) / slot;
    let best = 0;
    points.forEach((p, i) => { if (Math.abs(p.x - n) < Math.abs(points[best].x - n)) best = i; });
    setAt(best);
  };

  const cur = at != null ? points[at] : null;
  const fmt = (v: number | null) => (v == null ? '—' : (v > 0 ? '+' : '') + v.toFixed(1));
  const fmtSecs = (v: number | null | undefined) => (v == null ? '—' : `${v.toFixed(1)}${t('unit.s')}`);

  return (
    // overflow hidden keeps the long header from widening whatever holds the graph
    <div style={{ minWidth: 0, overflow: 'hidden' }}>
      <div style={{
        display: 'flex', gap: 'var(--sp-3)', alignItems: 'center', whiteSpace: 'nowrap', overflow: 'hidden',
        fontSize: 'var(--fs-7)', color: 'var(--sub)', height: 'var(--h-head)',
      }}>
        <span style={{ flex: 'none' }}>{title ?? t('data.trend.title')}</span>
        {split != null && (
          <span style={{ flex: 'none', color: 'var(--gold)' }}>{t('data.trend.split', { n: split })}</span>
        )}
        {/* Only the key; the numbers for the move under the pointer float in the graph. */}
        <span style={{ marginLeft: 'auto', display: 'inline-flex', gap: 'var(--sp-3)', minWidth: 0, overflow: 'hidden' }}>
          <span><span style={{ color: 'var(--accent)' }}>—</span> {names.mine}</span>
          <span><span style={{ color: 'var(--sub)' }}>—</span> {names.opp}</span>
        </span>
      </div>
      <svg ref={attach} viewBox={`0 0 ${w} ${height}`}
           style={{ width: '100%', height, display: 'block' }}
           onMouseMove={pickAt} onMouseLeave={() => setAt(null)}>
        {ticks.map((tick) => (
          <g key={tick}>
            <line x1={padL} y1={y(tick)} x2={w - padR} y2={y(tick)}
                  stroke={tick === 0 ? 'var(--border)' : 'var(--border-weak)'} strokeWidth={1} />
            {(tick / step) % every === 0 && (
              <text x={padL - 4} y={y(tick) + 3} textAnchor="end"
                    fontSize={9} fill="var(--sub)">{tick > 0 ? `+${tick}` : tick}</text>
            )}
          </g>
        ))}
        {maxSecs > 0 && (
          <g>
            <text x={w - padR + 4} y={y(lim) + 3} fontSize={9} fill="var(--sub)">{secsTop}{t('unit.s')}</text>
            <text x={w - padR + 4} y={y(0) + 3} fontSize={9} fill="var(--sub)">0{t('unit.s')}</text>
            <text x={w - padR + 4} y={y(-lim) + 3} fontSize={9} fill="var(--sub)">{secsTop}{t('unit.s')}</text>
            {points.map((p, i) => (
              <g key={i}>
                {p.mySecs ? (
                  <rect x={x(i) - slot * 0.35} y={y(0) - barH(p.mySecs)}
                        width={slot * 0.7} height={barH(p.mySecs)} fill="var(--accent)" opacity={0.22} />
                ) : null}
                {p.oppSecs ? (
                  <rect x={x(i) - slot * 0.35} y={y(0)}
                        width={slot * 0.7} height={barH(p.oppSecs)} fill="var(--sub)" opacity={0.3} />
                ) : null}
              </g>
            ))}
          </g>
        )}
        {split != null && split <= slots && (
          <g pointerEvents="none">
            <line x1={xAt(split)} y1={pad} x2={xAt(split)} y2={height - padB}
                  stroke="var(--gold)" strokeWidth={1.5} strokeDasharray="4 3" />
          </g>
        )}
        {marks.map((n) => (
          <g key={n}>
            <line x1={xAt(n)} y1={height - padB} x2={xAt(n)} y2={height - padB + 3}
                  stroke="var(--border)" strokeWidth={1} />
            <text x={xAt(n)} y={height - 3} textAnchor="middle"
                  fontSize={9} fill="var(--sub)">{n}</text>
          </g>
        ))}
        <path d={path((p) => p.opp)} fill="none" stroke="var(--sub)"
              strokeWidth={1.5} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
        <path d={path((p) => p.mine)} fill="none" stroke="var(--accent)"
              strokeWidth={1.5} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
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
        {at != null && cur && (() => {
          // A box beside the pointer's line: the move, then each side's eval and think time.
          const row = (name: string, v: number | null, s: number | null | undefined) =>
            `${name} ${fmt(v)}${s != null ? ` · ${fmtSecs(s)}` : ''}`;
          const lines = [t('data.ply', { n: cur.x }), row(names.mine, cur.mine, cur.mySecs), row(names.opp, cur.opp, cur.oppSecs)];
          const bw = Math.max(...lines.map((l, i) => textW(l, 10) + (i ? 14 : 0))) + 12;
          const bh = 46;
          const hx = x(at);
          const bx = hx + 8 + bw <= w - 2 ? hx + 8 : Math.max(2, hx - 8 - bw);
          return (
            <g pointerEvents="none">
              <rect x={bx} y={2} width={bw} height={bh} rx={3} fill="var(--card)" stroke="var(--border)" />
              {lines.map((l, i) => (
                <g key={i}>
                  {i > 0 && <line x1={bx + 6} y1={9.5 + i * 13} x2={bx + 16} y2={9.5 + i * 13}
                                  stroke={i === 1 ? 'var(--accent)' : 'var(--sub)'} strokeWidth={2} />}
                  <text x={bx + 6 + (i ? 14 : 0)} y={13 + i * 13} fontSize={10}
                        fill={i ? 'var(--text)' : 'var(--sub)'}>{l}</text>
                </g>
              ))}
            </g>
          );
        })()}
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
