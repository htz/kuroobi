// The web's eval graph (design-system/web): the app's (crates/gui/src-ui/js/components/data.tsx) drawn in screen
// pixels, so its text keeps the web's sizes at any width; values are whole numbers.
import React from 'react';
import { t, type Lang } from '../i18n';

/** `selective` is the confidence (%) of a selective endgame solve; it is drawn as a ring between a solve and a search. */
export type GraphPoint = { value: number; exact?: boolean; depth?: number; selective?: number };

const TEXT = 13;
const HEIGHT = 220;
const STEP = 8;

const whole = (v: number) => {
  const r = Math.round(v);
  return (r > 0 ? '+' : r < 0 ? '−' : '') + Math.abs(r);
};

/**
 * The move under a mouse reads out; pressing jumps to it, and dragging with the button down keeps jumping. A finger
 * reads out and jumps the same while it slides sideways, and on a tap; a vertical slide scrolls the page instead.
 * `pointer` is the move pointed at to begin with, for the canvas to draw the readout.
 */
export function EvalGraph({ lang, points, plies, cursor, blunder, busy, onJump, moveName, pointer }: {
  lang: Lang;
  points: (GraphPoint | undefined)[];
  plies: number;
  cursor?: number;
  blunder?: { at: number; loss: number };
  busy?: boolean;
  onJump: (n: number) => void;
  moveName: (n: number) => string | undefined;
  pointer?: number;
}) {
  const G = (key: string, vars?: Record<string, string | number>) => t(lang, 'graph.' + key, vars);
  const [hover, setHover] = React.useState<number | null>(pointer ?? null);
  const jumped = React.useRef<number | null>(null);
  // The pointer pressed on the graph. A finger's events can be captured by the shape it touched rather than the svg,
  // so the press is kept here instead of asked of hasPointerCapture.
  const pressed = React.useRef<number | null>(null);
  // Where a finger came down, until it slides sideways; a slide that goes up or down is the page's to scroll.
  const start = React.useRef<{ x: number; y: number } | null>(null);
  // The panels above the graph change height from move to move (a phone stacks them over it), which would carry the
  // graph away from the finger on it. While pressed, and a moment after, the page scrolls to hold the graph still.
  const hold = React.useRef<{ top: number; until: number; frame: number } | null>(null);
  const holdStill = (el: Element) => {
    if (hold.current) { hold.current.until = Infinity; return; }
    const h = { top: el.getBoundingClientRect().top, until: Infinity, frame: 0 };
    const tick = () => {
      const moved = el.getBoundingClientRect().top - h.top;
      if (Math.abs(moved) >= 1) window.scrollBy(0, moved);
      if (performance.now() < h.until) h.frame = requestAnimationFrame(tick);
      else hold.current = null;
    };
    h.frame = requestAnimationFrame(tick);
    hold.current = h;
  };
  const letGo = () => { if (hold.current) hold.current.until = performance.now() + 600; };
  React.useEffect(() => () => { if (hold.current) cancelAnimationFrame(hold.current.frame); }, []);
  // A readout a finger left behind goes once the record moves elsewhere.
  React.useEffect(() => setHover((h) => (h === null || h === cursor ? h : null)), [cursor]);
  const [W, setW] = React.useState(640);
  const box = React.useRef<HTMLDivElement>(null);
  React.useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(Math.max(240, el.clientWidth)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const upLabel = G('blackAhead'), downLabel = G('whiteAhead'), evenLabel = G('even');
  const labelW = (s: string) => [...s].reduce((n, ch) => n + (ch.charCodeAt(0) < 0x2e80 ? 0.6 : 1) * TEXT, 0);
  // A narrow graph keeps its plot and drops the side labels rather than shrinking them.
  const sideLabels = W >= 420;
  const H = HEIGHT, L = 40, T = 12, B = 30;
  const R = sideLabels ? Math.ceil(Math.max(labelW(upLabel), labelW(downLabel), labelW(evenLabel))) + 16 : 12;
  const defined = points.filter((p): p is GraphPoint => !!p).map(p => Math.abs(p.value));
  const ymax = Math.max(STEP, Math.min(64, Math.ceil((defined.length ? Math.max(...defined) : 0) / STEP) * STEP));
  const clamp = (v: number) => Math.max(-ymax, Math.min(ymax, v));
  const x = (n: number) => L + (W - L - R) * n / plies;
  const y = (v: number) => T + (H - T - B) * (1 - (v + ymax) / (2 * ymax));
  // Points are sized for 9px between moves; a narrower graph (a phone's is about 4.5px) shrinks them so they stay apart.
  const k = Math.min(1, (W - L - R) / plies / 9);
  const dot = (full: number, least: number) => Math.max(least, full * k);

  const rowStep = [1, 2, 4, 8].map(k => STEP * k).find(s => (H - T - B) * s / (2 * ymax) >= TEXT * 1.6) ?? STEP * 8;
  const rows: number[] = [];
  for (let v = -ymax; v <= ymax; v += rowStep) rows.push(v);
  const colStep = (W - L - R) / plies * 10 >= TEXT * 3 ? 10 : 20;
  const cols: number[] = [];
  for (let n = 0; n <= plies; n += colStep) cols.push(n);

  let d = '';
  let prev: { n: number; p: GraphPoint } | null = null;
  points.forEach((p, n) => {
    if (!p) return;
    const at = (k: number, v: number) => x(k).toFixed(1) + ' ' + y(clamp(v)).toFixed(1);
    if (prev) d += 'M' + at(prev.n, prev.p.value) + ' L' + at(n, p.value) + ' ';
    prev = { n, p };
  });
  const drawn = prev !== null;

  const bx = blunder ? x(blunder.at) : 0;
  const bRight = blunder ? bx > W - R - 140 : false;
  const bVal = blunder ? points[blunder.at]?.value : undefined;
  const bHigh = bVal !== undefined && bVal > 0;

  // Off the plot a hover reads nothing, while a drag that has left it holds to the nearest end.
  const plyAt = (e: React.PointerEvent<SVGSVGElement>, held = false) => {
    const r = e.currentTarget.getBoundingClientRect();
    const n = Math.round(((e.clientX - r.left - L) / (W - L - R)) * plies);
    if (held) return Math.max(0, Math.min(plies, n));
    return n >= 0 && n <= plies ? n : null;
  };
  const jump = (n: number | null, el: Element) => {
    if (n === null || n === jumped.current) return;
    holdStill(el);
    jumped.current = n;
    onJump(n);
  };
  const down = (e: React.PointerEvent<SVGSVGElement>) => {
    if (!drawn) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    pressed.current = e.pointerId;
    jumped.current = null;
    const n = plyAt(e);
    setHover(n);
    // A finger waits for a slide or a lift: one that goes on to scroll the page must not move the board.
    if (e.pointerType === 'mouse') { start.current = null; jump(n, e.currentTarget); }
    else start.current = { x: e.clientX, y: e.clientY };
  };
  const move = (e: React.PointerEvent<SVGSVGElement>) => {
    if (pressed.current === e.pointerId && start.current) {
      const dx = Math.abs(e.clientX - start.current.x), dy = Math.abs(e.clientY - start.current.y);
      if (dy >= 8 && dy >= dx) { pressed.current = null; start.current = null; return; }
      if (dx < 8) return;
      start.current = null;
    }
    const held = pressed.current === e.pointerId;
    const n = plyAt(e, held);
    setHover(n);
    if (held) jump(n, e.currentTarget);
  };
  const up = (e: React.PointerEvent<SVGSVGElement>) => {
    if (pressed.current !== e.pointerId) return;
    pressed.current = null;
    start.current = null;
    jump(plyAt(e, true), e.currentTarget);
    letGo();
  };
  const shown = hover !== null ? points[hover] : undefined;
  const shownMove = hover !== null ? moveName(hover) : undefined;

  return (
    <div style={{
      display: 'flex', flexDirection: 'column', gap: 12, padding: 20,
      border: '1px solid var(--border)', borderRadius: 'var(--r-card)', background: 'var(--panel)',
    }}>
      <div style={{
        minHeight: 24, display: 'flex', flexWrap: 'wrap', alignItems: 'center', columnGap: 16, rowGap: 4,
        font: 'var(--t-meta) var(--ff-web)', color: 'var(--sub)',
      }}>
        <span style={{ fontWeight: 700, color: 'var(--text)', fontSize: 'var(--t-small)' }}>{G('title')}</span>
        <Legend tone="text">{G('srcSolve')}</Legend>
        <Legend tone="ring">{G('srcSelective')}</Legend>
        <Legend tone="accent">{G('srcSearch')}</Legend>
        <span style={{ marginLeft: 'auto', minWidth: 140, textAlign: 'right', fontSize: 'var(--t-small)', color: 'var(--text)' }}>
          {/* Every move under the pointer reads out, one without a value as a dash. */}
          {hover !== null && drawn && <>
            {shownMove ? G('plyMove', { n: hover, move: shownMove }) : G('ply', { n: hover })}
            {' '}<b style={{ fontWeight: 700 }}>{shown ? whole(shown.value) : '—'}</b>
            {shown && <span style={{ color: 'var(--sub)' }}>{' '}{shown.exact ? G('srcSolve') : shown.selective ? G('srcSelectiveAt', { p: shown.selective }) : G('srcSearch')}</span>}
          </>}
        </span>
      </div>
      <div ref={box} style={{ position: 'relative', height: H }}>
        <svg width={W} height={H} viewBox={`0 0 ${W} ${H}`} role="img" aria-label={G('aria')}
             style={{ display: 'block', cursor: drawn ? 'pointer' : 'default', touchAction: 'pan-y' }}
             onPointerDown={down} onPointerMove={move} onPointerUp={up}
             onPointerCancel={() => {
               pressed.current = null;
               start.current = null;
               if (hold.current) hold.current.until = 0;
               setHover(null);
             }}
             onPointerLeave={(e) => { if (e.pointerType === 'mouse') setHover(null); }}>
          <rect x={0} y={0} width={W} height={H} rx={10} fill="var(--bg)" />

          {rows.map(v => (
            <g key={'r' + v}>
              <line x1={L} y1={y(v)} x2={W - R} y2={y(v)}
                    stroke={v === 0 ? 'var(--border)' : 'var(--border-weak)'} strokeWidth={1}
                    strokeDasharray={v === 0 ? undefined : '2 3'} />
              <text x={L - 8} y={y(v)} dominantBaseline="central" textAnchor="end" fill="var(--sub)" fontSize={TEXT}>
                {v > 0 ? '+' + v : v === 0 ? '0' : '−' + -v}
              </text>
            </g>
          ))}
          {sideLabels && <>
            <text x={W - R + 10} y={y(ymax)} dominantBaseline="hanging" fill="var(--sub)" fontSize={TEXT}>{upLabel}</text>
            <text x={W - R + 10} y={y(0)} dominantBaseline="central" fill="var(--sub)" fontSize={TEXT}>{evenLabel}</text>
            <text x={W - R + 10} y={y(-ymax)} fill="var(--sub)" fontSize={TEXT}>{downLabel}</text>
          </>}

          {cols.map(n => (
            <g key={'c' + n}>
              {n > 0 && <line x1={x(n)} y1={T} x2={x(n)} y2={H - B} stroke="var(--border-weak)" strokeWidth={1} />}
              <text x={x(n)} y={H - 8} textAnchor="middle" fill="var(--sub)" fontSize={TEXT}>{n}</text>
            </g>
          ))}

          {d && <path d={d.trim()} fill="none" stroke="var(--accent)" strokeWidth={2} strokeLinejoin="round" />}

          {blunder && <>
            <line x1={bx} y1={T} x2={bx} y2={H - B} stroke="var(--bad)" strokeWidth={1.5} />
            <text x={bRight ? bx - 8 : bx + 8} y={bHigh ? H - B - 8 : T + 14}
                  textAnchor={bRight ? 'end' : 'start'} fill="var(--bad)" fontSize={TEXT}>
              {G('blunder', { n: blunder.at, loss: Math.round(blunder.loss) })}
            </text>
          </>}
          {cursor !== undefined && (
            <line x1={x(cursor)} y1={T} x2={x(cursor)} y2={H - B} stroke="var(--accent-dim)" strokeWidth={1} strokeDasharray="3 3" />
          )}

          {points.map((p, n) => p && (
            p.selective && !p.exact
              ? <circle key={n} cx={x(n)} cy={y(clamp(p.value))} r={n === hover ? dot(5.5, 3.5) : dot(4, 2.25)}
                        fill="var(--bg)" stroke="var(--text)" strokeWidth={dot(2, 1.25)} />
              : <circle key={n} cx={x(n)} cy={y(clamp(p.value))} r={n === hover ? dot(6, 4) : p.exact ? dot(4.5, 2.25) : dot(3.5, 1.75)}
                        fill={p.exact ? 'var(--text)' : 'var(--accent)'} stroke="var(--bg)" strokeWidth={dot(1, 0.5)} />
          ))}
          {hover !== null && drawn && (
            <line x1={x(hover)} y1={T} x2={x(hover)} y2={H - B} stroke="var(--sub)" strokeWidth={1} />
          )}
        </svg>
        {!drawn && (
          <div style={{
            position: 'absolute', inset: 0, display: 'grid', placeItems: 'center',
            font: 'var(--t-small) var(--ff-web)', color: 'var(--sub)', pointerEvents: 'none',
          }}>
            {/* The ground under the words keeps the zero line from striking them through. */}
            <span style={{ background: 'var(--bg)', padding: '0 8px' }}>{busy ? G('analyzing') : G('empty')}</span>
          </div>
        )}
      </div>
    </div>
  );
}

function Legend({ tone, children }: { tone: 'text' | 'accent' | 'ring'; children: React.ReactNode }) {
  return (
    <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
      <span style={{
        flex: 'none', width: 8, height: 8, borderRadius: '50%', boxSizing: 'border-box',
        background: tone === 'ring' ? 'transparent' : `var(--${tone})`,
        border: tone === 'ring' ? '2px solid var(--text)' : 'none',
      }} />
      {children}
    </span>
  );
}
