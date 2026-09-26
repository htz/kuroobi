import React from 'react';
import { Badge, Button, Dot } from './primitives';
import { Col, Divider, Empty, TableHead, TableRow, picked as pickedStyle } from './layout';
import { t } from '../i18n';


export type StoneColor = 'b' | 'w';
export const toStoneColor = (n: 1 | 2): StoneColor => (n === 1 ? 'b' : 'w');

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
export const srcIsSolve = (src: string): boolean => src === 'solve';

const moveCols = (): Col[] => [
  { head: '#', w: 22, right: true },
  { head: t('data.moves.header_move'), w: 58 },
  { head: t('data.moves.header_eval'), w: 56, right: true, num: true },
  { head: t('data.moves.header_time'), w: 34, right: true, num: true },
  { head: t('data.moves.header_source'), right: true, clip: true },
];

export function KifuTable({ moves, current, onSelect, decimals = 1 }: {
  moves: Move[]; current?: number; onSelect?: (n: number) => void;
  decimals?: number;
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
    background: black ? 'var(--stone-black)' : 'var(--stone-white)',
    boxShadow: 'inset 0 0 0 1px ' + (black ? 'var(--stone-black-edge)' : 'var(--stone-white-edge)'),
  }} />;
}

export type GraphPoint = { value: number; exact?: boolean; book?: boolean };

export function EvalGraph({ points, plies, cursor, blunder, busy, pov = 'b', extra, onJump, moveName, open }: {
  points: (GraphPoint | undefined)[];
  plies?: number;
  cursor?: number;
  blunder?: { at: number; loss: number };
  busy?: boolean;
  pov?: 'b' | 'w';
  extra?: React.ReactNode;
  onJump?: (n: number) => void;
  moveName?: (n: number) => string | undefined;
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

  let d = '', pen = false;
  points.forEach((p, n) => {
    if (!p) return;
    d += (pen ? 'L' : 'M') + x(n).toFixed(1) + ' ' + y(clamp(p.value)).toFixed(1) + ' ';
    pen = true;
  });

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
  const shownMove = hover !== null ? moveName?.(hover) : undefined;

  return (
    <div className={'k-graph' + (open ? ' k-open' : '')} style={{
      flex: '0 1 auto', minHeight: 0,
      borderTop: '1px solid var(--border)', background: 'var(--panel)',
      padding: 'var(--sp-3) var(--sp-4) var(--sp-4)', flexDirection: 'column', gap: 'var(--sp-2)',
    }}>
      <div style={{
        minHeight: extra ? 'var(--h-field)' : 'var(--h-head)',
        display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>
        <span>{title}</span>
        <Legend tone="gold">{t('data.src.book')}</Legend>
        <Legend tone="text">{t('data.src.solve')}</Legend>
        <Legend tone="accent">{t('data.src.search')}</Legend>
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
             onClick={onJump && ((e) => { const n = plyAt(e); if (n !== null) onJump(n); })}>
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
          {hover !== null && shown && (
            <line x1={x(hover)} y1={T} x2={x(hover)} y2={H - B}
                  stroke="var(--sub)" strokeWidth={1} />
          )}
        </svg>
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

export function PlayerRow({ color, name, rate, dev, meta, clock, active, discs, onName }: {
  color: StoneColor; name: string;
  rate?: number; dev?: number; meta?: React.ReactNode;
  clock?: string; active?: boolean; discs?: number;
  onName?: () => void;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)', fontSize: 'var(--fs-5)' }}>
      <StoneDot color={color} size={14} />
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

export function RateChart({ points, height = 74, width = 300, axes, dates, labels, hover, onHover }: {
  points: number[];
  height?: number;
  width?: number;
  axes?: boolean;
  dates?: string[];
  labels?: string[];
  hover?: number | null;
  onHover?: (i: number | null) => void;
}) {
  if (points.length < 2) {
    return (
      <div style={{ height, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        {t('data.rate.empty')}
      </div>
    );
  }
  const padL = axes ? 34 : 0, padB = axes ? 16 : 0;
  const w = width, pad = 8;
  const min = Math.min(...points), max = Math.max(...points), span = Math.max(1, max - min);
  const y = (p: number) =>
    height - pad - padB - ((p - min) / span) * (height - pad * 2 - 4 - padB);
  const x = (i: number) => padL + (i / (points.length - 1)) * (w - padL);
  const xy = points.map((p, i) => [x(i), y(p)] as const);
  const last = xy[xy.length - 1];
  const ticks: number[] = [];
  if (axes) {
    const step = [1, 2, 5, 10, 25, 50, 100, 200, 500, 1000].find((v) => span / v <= 5) ?? 2000;
    for (let v = Math.ceil(min / step) * step; v <= max; v += step) ticks.push(v);
  }
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
        const tw = [...text].reduce((n, c) => n + (c.charCodeAt(0) < 256 ? 6 : 11), 0) + 12;
        const bx = Math.min(Math.max(hx - tw / 2, padL), w - tw);
        return (
          <g pointerEvents="none">
            <line x1={hx} y1={pad} x2={hx} y2={height - padB} stroke="var(--sub)" strokeWidth={1} strokeDasharray="2 2" />
            <circle cx={hx} cy={hy} r={3.5} fill="var(--bg)" stroke="var(--accent)" strokeWidth={2} />
            <rect x={bx} y={0} width={tw} height={18} rx={3} fill="var(--card)" stroke="var(--border)" />
            <text x={bx + 6} y={12.5} fontSize={11} fill="var(--text)">{text}</text>
          </g>
        );
      })()}
    </svg>
  );
}

/// Widths mirror `ResultRow`.
export function ResultHead({ note, rating, when }: {
  note?: boolean; rating?: boolean; when?: boolean;
}) {
  const cell = (w: number | string): React.CSSProperties => ({
    width: w, flex: 'none', textAlign: 'right',
  });
  return (
    <TableHead pad="var(--sp-2)">
      <span style={{ width: 24, flex: 'none' }}>{t('data.result.head_outcome')}</span>
      <span style={{ flex: 1, minWidth: 0 }}>{t('data.result.head_opponent')}</span>
      {note && <span style={{ width: 'var(--w-gtype)', flex: 'none' }}>{t('data.result.head_kind')}</span>}
      <span style={cell(44)}>{t('data.result.head_discs')}</span>
      {rating && <span style={cell(60)}>{t('data.result.head_rating')}</span>}
      {when && <span style={cell(64)}>{t('data.result.head_date')}</span>}
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
  rating?: number | null;
  dim?: boolean;
  picked?: boolean;
  onHover?: (on: boolean) => void;
  onClick?: () => void;
}) {
  const body = <>
    <span style={{ width: 24, flex: 'none',
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
    <span style={{ width: 44, flex: 'none', textAlign: 'right', color: 'var(--sub)',
                   fontVariantNumeric: 'tabular-nums' }}>
      {adjourned ? '\u2014' : discs > 0 ? '+' + discs : discs}
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
    ...(picked ? pickedStyle(true) : null),
  };
  const hov = onHover
    ? { onMouseEnter: () => onHover(true), onMouseLeave: () => onHover(false) }
    : {};
  return onClick && !dim
    ? <button type="button" className="k-row" onClick={onClick} title={t('data.result.open_in_study')} {...hov}
              style={{ ...style, border: 0, background: 'transparent', cursor: 'pointer' }}>{body}</button>
    : <div style={{ ...style, opacity: dim ? 0.5 : 1 }} {...hov}>{body}</div>;
}

export { Badge, Dot };

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

export function EvalTrend({ points, height = 96 }: {
  points: { x: number; mine: number | null; opp: number | null }[];
  height?: number;
}) {
  const [at, setAt] = React.useState<number | null>(null);
  const has = points.some((p) => p.mine != null || p.opp != null);
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
  const vals = points.flatMap((p) => [p.mine, p.opp]).filter((v): v is number => v != null);
  const lim = Math.max(8, ...vals.map((v) => Math.abs(v)));
  const step = [2, 4, 8, 16, 32].find((v) => lim / v <= 2.5) ?? 32;
  const ticks: number[] = [];
  for (let v = -Math.floor(lim / step) * step; v <= lim; v += step) ticks.push(v);

  const span = Math.max(1, points.length - 1);
  const x = (i: number) => padL + (i / span) * (w - padL - pad);
  const y = (v: number) => (height - padB) / 2 - (v / lim) * ((height - padB) / 2 - pad);

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
    const r = e.currentTarget.getBoundingClientRect();
    const vx = ((e.clientX - r.left) / r.width) * w;
    const i = Math.round(((vx - padL) / (w - padL - pad)) * (points.length - 1));
    setAt(Math.max(0, Math.min(points.length - 1, i)));
  };

  const cur = at != null ? points[at] : null;
  const fmt = (v: number | null) => (v == null ? '—' : (v > 0 ? '+' : '') + v.toFixed(1));

  return (
    <div>
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
        {[0, points.length >> 1, points.length - 1].map((i, k) => (
          <text key={k} x={x(i)} y={height - 3}
                textAnchor={k === 0 ? 'start' : k === 2 ? 'end' : 'middle'}
                fontSize={9} fill="var(--sub)">{points[i].x}</text>
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
