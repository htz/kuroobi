import React, { useEffect, useLayoutEffect, useState } from 'react';
import { DEFAULT_EXT, gtypeLabel, parseClockSpec, secsText } from './ggs';
import { t } from './i18n';
import type { GgsSnapshot } from './types';
import { RateRow } from './components/ggs';
import { captionStyle, ListRow, marked, picked as pickedStyle, TableHead } from './components/layout';

/** "5 分" for "00:05:00", "4 分 30 秒 (延長 1 分)" for "00:04:30//00:01:00"; anything unusual as GGS wrote it. */
export function clockLabel(time: string): string {
  const c = parseClockSpec(time);
  if (!c) return time;
  const main = secsText(c.main);
  if (c.ext === DEFAULT_EXT) return main;
  return c.ext ? t('ggs.clock_choice.with_ext', { main, ext: secsText(c.ext) }) : t('ggs.clock_choice.no_ext', { main });
}

/** "同期・ランダム14手 · 5 分 · レート戦" */
export const condLine = (gtype: string, time: string, rated: boolean | null): string =>
  [gtypeLabel(gtype), time ? clockLabel(time) : '', rated == null ? '' : rated ? t('ggs.rated') : t('ask.unrated')]
    .filter(Boolean).join(' · ');

/** The body of a screen under its toolbar: nothing scrolls here, only the lists inside. */
export function Page({ children, row }: { children: React.ReactNode; row?: boolean }) {
  return (
    <div style={{
      flex: 1, minHeight: 0, minWidth: 0, overflow: 'hidden', display: 'flex',
      flexDirection: row ? 'row' : 'column',
    }}>{children}</div>
  );
}

/** A padded column of sections inside a Page. */
export function Sections({ children }: { children: React.ReactNode }) {
  return (
    <div style={{
      flex: 1, minHeight: 0, minWidth: 0, overflow: 'hidden', padding: 'var(--sp-4) var(--sp-1) 0',
      display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
    }}>{children}</div>
  );
}

/** A column header row for a GGS table: `TableHead` with the GGS rows' spacing. Cells are given as [label, width or undefined (= the flexible one), align]. */
export function HeadRow({ cols }: { cols: [string, number | undefined, ('left' | 'right' | 'center')?][] }) {
  return (
    <TableHead gap="var(--sp-3)"
               cols={cols.map(([head, w, align]) => ({ head, w, right: align === 'right', center: align === 'center' }))} />
  );
}

/** The scrolling part of a table: only the rows move, the header stays. */
export function Rows({ children }: { children: React.ReactNode }) {
  return <div className="k-scroll" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>{children}</div>;
}

/** A table that fills what is left of its column. */
export function Table({ children }: { children: React.ReactNode }) {
  return <div style={{ height: '100%', display: 'flex', flexDirection: 'column' }}>{children}</div>;
}

/** One cell; `w` undefined makes it the flexible column. */
export function Cell({ w, align, sub, bold, mono, children, sel }: {
  w?: number; align?: 'left' | 'right' | 'center'; sub?: boolean; bold?: boolean; mono?: boolean; sel?: boolean;
  children?: React.ReactNode;
}) {
  return (
    <span className={sel ? 'k-sel' : undefined} style={{
      ...(w === undefined ? { flex: 1, minWidth: 0 } : { width: w, flex: 'none' }),
      textAlign: align ?? 'left', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
      color: sub ? 'var(--sub)' : undefined, fontWeight: bold ? 600 : undefined,
      fontFamily: mono ? 'var(--ff-mono)' : undefined,
    }}>{children}</span>
  );
}

/** A GGS table row. `onClick` makes it a `ListRow` (chosen look, hover, arrow keys); `mark` is the accent bar alone. */
export function Line({ children, onClick, picked, height = 'field', mark }: {
  children: React.ReactNode; onClick?: () => void; picked?: boolean;
  height?: 'row' | 'field' | 'row2'; mark?: boolean;
}) {
  const extra: React.CSSProperties = { gap: 'var(--sp-3)', ...(mark && !picked ? marked(true) : null) };
  // A row that cannot be pressed can still be the one that is meant (the login's row in a table).
  const mine: React.CSSProperties = picked ? pickedStyle(true) : {};
  if (onClick) return <ListRow on={picked} height={height} onClick={onClick} style={extra}>{children}</ListRow>;
  // Not pressable as a whole (its buttons are), so no hover or pointer on the row.
  return (
    <div style={{
      flex: 'none', display: 'flex', alignItems: 'center', width: '100%', height: `var(--h-${height})`,
      padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)', fontSize: 'var(--fs-5)',
      color: 'var(--text)', whiteSpace: 'nowrap', ...extra, ...mine,
    }}>{children}</div>
  );
}

/**
 * An empty list: one quiet line where the first row would be. Empty states come in three sizes:
 * a whole screen (`EmptyState`), one area (`Blank`), and a list inside an area (this).
 */
export function ListNote({ children }: { children: React.ReactNode }) {
  return (
    <span style={{ display: 'block', padding: 'var(--sp-2) var(--sp-3)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
      {children}
    </span>
  );
}

/** A one-line notice across the top of a body (accent, gold or bad on the left edge). */
export function Band({ tone = 'gold', children }: { tone?: 'gold' | 'bad' | 'accent'; children: React.ReactNode }) {
  return (
    <div style={{
      flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', margin: '0 var(--sp-3) var(--sp-2)',
      padding: 'var(--sp-2) var(--sp-3)', borderRadius: 'var(--r-1)', background: 'var(--card)',
      boxShadow: `inset 2px 0 0 var(--${tone})`, fontSize: 'var(--fs-5)', flexWrap: 'wrap', rowGap: 'var(--sp-1)',
    }}>{children}</div>
  );
}

/** Centered "nothing here" text for an empty body. */
export function Blank({ title, body }: { title: string; body?: string }) {
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)', textAlign: 'center' }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', alignItems: 'center' }}>
        <span style={{ fontSize: 'var(--fs-4)', fontWeight: 600 }}>{title}</span>
        {body && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{body}</span>}
      </div>
    </div>
  );
}

/** The list column on the left of a list + detail screen. */
export function ListPane({ head, children, foot, footRow }: {
  head?: React.ReactNode; children: React.ReactNode; foot?: React.ReactNode;
  /** The foot is a full-width row of its own, not a padded button. */
  footRow?: boolean;
}) {
  return (
    <div className="k-listpane" style={{
      flex: 'none', minHeight: 0, display: 'flex', flexDirection: 'column',
      borderRight: '1px solid var(--border)', position: 'relative',
    }}>
      {head}
      <div className="k-scroll" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>{children}</div>
      {foot && <div style={{ flex: 'none', padding: footRow ? 0 : 'var(--sp-2)', borderTop: '1px solid var(--border)' }}>{foot}</div>}
    </div>
  );
}

/** A small caps heading inside a list pane. */
export function ListHead({ children, aside }: { children: React.ReactNode; aside?: React.ReactNode }) {
  return (
    <div style={{
      flex: 'none', height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
      marginTop: 'var(--sp-2)', padding: '0 var(--sp-3)', ...captionStyle,
    }}>
      {children}
      {aside != null && <span style={{ marginLeft: 'auto', fontWeight: 400, letterSpacing: 0 }}>{aside}</span>}
    </div>
  );
}

/** Toolbar title: the bold name and a quieter line after it. */
export function TitleBit({ title, sub }: { title: React.ReactNode; sub?: React.ReactNode }) {
  return (
    <span style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-2)', minWidth: 0, whiteSpace: 'nowrap', overflow: 'hidden' }}>
      <b style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{title}</b>
      {sub && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', overflow: 'hidden', textOverflow: 'ellipsis' }}>{sub}</span>}
    </span>
  );
}

/** Right side of a toolbar. */
export function Right({ children }: { children: React.ReactNode }) {
  return (
    <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', whiteSpace: 'nowrap' }}>
      {children}
    </span>
  );
}

export const signed = (v: number): string => (v > 0 ? '+' : '') + v;

/** What a rating chart shows for the game under the pointer: when, who, the disc result, the rating after it. */
export const rateLabel = (day: string, opp: string, discs: number | null, rating: number): string =>
  [day, opp, discs != null ? signed(discs) : '', rating.toFixed(1)].filter(Boolean).join(' · ');

/** Width of an element, kept up to date. */
// GGS actions a person asked for (accept, watch, resign, ...): a failure must reach the screen.
// The screens cannot raise a toast themselves, so App listens here and shows it.
const failSubs = new Set<(e: unknown) => void>();
export function onGgsFailure(f: (e: unknown) => void): () => void {
  failSubs.add(f);
  return () => { failSubs.delete(f); };
}
export const reportGgsFailure = (e: unknown) => { failSubs.forEach((f) => f(e)); };
/** Runs an action a person asked for; a failure shows as a toast instead of vanishing. */
export function act(p: Promise<unknown>): void { p.catch(reportGgsFailure); }

/** An element's width, measured again whenever the element is swapped (a section that comes and goes). */
export function useWidth(): [(el: HTMLDivElement | null) => void, number] {
  const [el, setEl] = useState<HTMLDivElement | null>(null);
  const [w, setW] = useState(600);
  useLayoutEffect(() => {
    if (!el) return;
    const o = new ResizeObserver(() => setW(Math.max(200, Math.floor(el.getBoundingClientRect().width))));
    o.observe(el);
    return () => o.disconnect();
  }, [el]);
  return [setEl, w];
}

/** The clock, ticking every `ms` while `on`. */
export function useNow(on: boolean, ms: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const id = window.setInterval(() => setNow(Date.now()), ms);
    return () => window.clearInterval(id);
  }, [on, ms]);
  return now;
}

/** The window's height, kept up to date. */
export function useWindowHeight(): number {
  const [h, setH] = useState(() => window.innerHeight);
  useEffect(() => {
    const on = () => setH(window.innerHeight);
    window.addEventListener('resize', on);
    return () => window.removeEventListener('resize', on);
  }, []);
  return h;
}

/** One move on an eval graph: each side's eval from the named player's side, and think time. */
export type TrendPoint = { x: number; mine: number | null; opp: number | null; mySecs: number | null; oppSecs: number | null };

/**
 * A synchro pair's boards added into one series. Each board's last value is held until it reports
 * again, and a move is summed only when every board has a value: one board alone would be a swing
 * of the whole eval the match never had (the other starts from the same place, reversed).
 */
export function sumBoards(parts: TrendPoint[][]): TrendPoint[] {
  if (parts.length < 2) return parts[0] ?? [];
  // The boards are matched by move number, not by position in their lists.
  const xs = [...new Set(parts.flatMap((p) => p.map((q) => q.x)))].sort((a, b) => a - b);
  const byX = parts.map((p) => new Map(p.map((q) => [q.x, q])));
  // A board's value for one line is that side's last report there. Until that side has moved on
  // the board (at move 1 each side has moved on only one of the two), the other side's report on
  // the same board stands in: both are that board seen from the same player.
  const held = (m: Map<number, TrendPoint>, pick: (p: TrendPoint) => number | null,
                other: (p: TrendPoint) => number | null) => {
    let v: number | null = null;
    let w: number | null = null;
    return xs.map((x) => {
      const q = m.get(x);
      const u = q ? pick(q) : null;
      const o = q ? other(q) : null;
      if (u != null) v = u;
      if (o != null) w = o;
      return v ?? w;
    });
  };
  const add = (us: (number | null)[][]) =>
    xs.map((_, i) => (us.some((u) => u[i] == null) ? null : us.reduce((a, u) => a + u[i]!, 0)));
  const mine = add(byX.map((m) => held(m, (q) => q.mine, (q) => q.opp)));
  const opp = add(byX.map((m) => held(m, (q) => q.opp, (q) => q.mine)));
  return xs.map((x, i) => ({
    x, mine: mine[i], opp: opp[i],
    mySecs: byX.map((m) => m.get(x)?.mySecs ?? null).find((v) => v != null) ?? null,
    oppSecs: byX.map((m) => m.get(x)?.oppSecs ?? null).find((v) => v != null) ?? null,
  }));
}

// A rating whose deviation is still this wide is provisional (GGS starts everyone at 350).
const PROVISIONAL_DEV = 100;

/** The ratings GGS keeps for one player, one `RateRow` per pool (random first). */
export function RankRates({ ranks }: { ranks: GgsSnapshot['my_ranks'] }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', padding: '0 var(--sp-3)' }}>
      {ranks.map((r) => (
        <RateRow key={r.gtype} label={r.gtype === '8r' ? t('ggs.gtype.rand_opening') : t('ggs.pool.normal')}
                 rate={{ value: r.rating, dev: r.dev, rank: r.rank, w: r.wins, l: r.losses, d: r.draws,
                         provisional: r.dev > PROVISIONAL_DEV }} />
      ))}
    </div>
  );
}
