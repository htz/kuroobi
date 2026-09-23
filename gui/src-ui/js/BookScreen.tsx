import { useCallback, useEffect, useMemo, useState } from 'react';
import { api } from './api';
import { sqName } from './adapt';
import type { BookNode } from './types';
import { Board, type EvalInfo } from './components/board';
import { ScoreRow } from './components/data';
import { Col, Empty, EmptyState, List, Section, TableHead } from './components/layout';
import { Button } from './components/primitives';
import { t, tErr } from './i18n';


const keyOf = (line: number[]) => line.map(sqName).join('');

export interface BookBrowse {
  line: number[];
  node: BookNode | null;
  nodes: Record<string, BookNode>;
  open: ReadonlySet<string>;
  toggle: (key: string) => void;
  err: string;
  push: (sq: number) => void;
  back: () => void;
  reset: () => void;
  goto: (kifu: string) => void;
}

const C_VALUE = { w: 44 };
const C_GAMES = { w: 58 };
const treeCols = (): Col[] => [
  { head: t('book.col.line') },
  { head: t('book.col.value'), ...C_VALUE, right: true, num: true },
  { head: t('book.col.games'), ...C_GAMES, right: true, num: true },
];

export function useBookBrowse(on: boolean): BookBrowse {
  const [line, setLine] = useState<number[]>([]);
  const [nodes, setNodes] = useState<Record<string, BookNode>>({});
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set<string>());
  const [err, setErr] = useState('');

  const root = keyOf(line);

  const want = useMemo(() => {
    const need = new Set<string>([root]);
    const addKids = (k: string) => {
      const n = nodes[k];
      if (!n) return;
      for (const m of n.moves) need.add(k + sqName(m.pos));
    };
    addKids(root);
    for (const k of open) {
      need.add(k);
      addKids(k);
    }
    return [...need].filter((k) => !(k in nodes));
  }, [root, open, nodes]);

  useEffect(() => {
    if (!on || want.length === 0) return;
    let alive = true;
    void Promise.all(want.map((k) => api.bookNode(k).then((n) => [k, n] as const)))
      .then((pairs) => {
        if (!alive) return;
        setNodes((prev) => ({ ...prev, ...Object.fromEntries(pairs) }));
        setErr('');
      })
      .catch((e) => { if (alive) setErr(tErr(e)); });
    return () => { alive = false; };
  }, [on, want]);

  const toggle = useCallback((key: string) => {
    setOpen((prev) => {
      const next = new Set(prev);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }, []);

  return {
    line,
    node: nodes[root] ?? null,
    nodes,
    open,
    toggle,
    err,
    push: useCallback((sq: number) => setLine((l) => [...l, sq]), []),
    back: useCallback(() => setLine((l) => l.slice(0, -1)), []),
    reset: useCallback(() => setLine([]), []),
    goto: useCallback((kifu: string) => setLine(parseLine(kifu)), []),
  };
}

export function BookPane({ b, coords, grain, flip, onSettings }: {
  b: BookBrowse; coords: boolean; grain: boolean; flip: boolean;
  onSettings: () => void;
}) {
  const n = b.node;
  if (n && n.size === 0) {
    return (
      <EmptyState title={t('book.empty.no_book_title')}
                  body={t('book.empty.no_book_body')}
                  actions={<Button size="field" variant="primary"
                                   onClick={onSettings}>{t('book.empty.open_settings')}</Button>} />
    );
  }
  const evals: Record<number, EvalInfo> = {};
  n?.moves.forEach((m, i) => {
    evals[m.pos] = { score: m.value, src: { book: true }, best: i === 0 };
  });
  return (
    <div style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
          <div style={{
            flex: 1, minHeight: 0, display: 'grid', placeItems: 'center',
            gridTemplateRows: 'minmax(0, 1fr)', gridTemplateColumns: 'minmax(0, 1fr)',
            padding: 'var(--sp-2) var(--sp-4)',
          }}>
              <div style={{ height: '100%', maxHeight: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
          <Board cells={(n?.cells ?? INITIAL) as (0 | 1 | 2)[]}
                 legal={n?.moves.map((m) => m.pos) ?? []}
                 last={b.line.length ? b.line[b.line.length - 1] : null}
                 evals={evals} coords={coords} grain={grain} flip={flip}
                 onPlay={(sq) => { if (n?.moves.some((m) => m.pos === sq)) b.push(sq); }} />
        </div>
      </div>
      <ScoreRow black={n?.black ?? 2} white={n?.white ?? 2}
                turn={n?.player === 'white' ? 'w' : 'b'}
                meta={b.err || (n && n.moves.length === 0 ? t('book.meta.end_of_line') : undefined)} />
    </div>
  );
}

interface Row {
  key: string;
  depth: number;
  name: string;
  value: number;
  games: number;
  child?: BookNode;
}

function rowsOf(b: BookBrowse, key: string, depth: number, out: Row[]): void {
  const n = b.nodes[key];
  if (!n) return;
  for (const m of n.moves) {
    const k = key + sqName(m.pos);
    out.push({ key: k, depth, name: sqName(m.pos), value: m.value, games: m.games, child: b.nodes[k] });
    if (b.open.has(k)) rowsOf(b, k, depth + 1, out);
  }
}

export function BookDock({ b, decimals = 1 }: { b: BookBrowse; decimals?: number }) {
  const n = b.node;

  return (
    <>
      <Section title={t('book.section.position')}
               aside={<span>{t('book.position.plies', { n: b.line.length })}</span>}>
        <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.9, wordBreak: 'break-all' }}>
          {b.line.length ? b.line.map(sqName).join(' ') : t('book.position.initial')}
        </div>
        {n?.value != null && (
          <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)' }}>
            <b style={{ fontSize: 'var(--fs-1)', fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>
              {n.value > 0 ? '+' : ''}{n.value.toFixed(decimals)}
            </b>
            <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
              {n.depth != null ? t('book.position.disc_diff_depth', { n: n.depth })
                : t('book.position.disc_diff')}
            </span>
          </div>
        )}
        {n?.value != null && (
          <div style={{ display: 'flex', alignItems: 'center', fontSize: 'var(--fs-5)' }}>
            <span style={{ color: 'var(--sub)' }}>{t('book.position.source')}</span>
            <span style={{ marginLeft: 'auto', flex: 'none', whiteSpace: 'nowrap',
                           color: n.learned ? 'var(--gold)' : undefined }}>
              {n.learned ? t('book.source.learned') : 'book.txt'}
            </span>
          </div>
        )}
      </Section>

      <Section title={t('book.section.next')} aside={n ? <span>{n.moves.length}</span> : undefined}>
        {!n || n.moves.length === 0 ? (
          <Empty>{n ? t('book.empty.end_of_line') : t('book.empty.not_in_book')}</Empty>
        ) : (
          <List>
            {n?.moves.map((m) => (
              <button key={m.pos} type="button" className="k-row" onClick={() => b.push(m.pos)}
                      title={t('book.row.play_move')}
                      style={{
                        display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                        height: 'var(--h-row)', border: 0, background: 'transparent',
                        borderBottom: '1px solid var(--border-weak)', borderRadius: 'var(--r-2)',
                        padding: '0 var(--sp-1)', fontSize: 'var(--fs-6)',
                        color: 'var(--text)', textAlign: 'left', cursor: 'pointer',
                      }}>
                <span style={{ flex: 1, fontWeight: 600 }}>{sqName(m.pos)}</span>
                <span style={{ width: C_VALUE.w, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {m.value > 0 ? '+' : ''}{m.value.toFixed(decimals)}
                </span>
                <span style={{ width: C_GAMES.w, textAlign: 'right', fontSize: 'var(--fs-7)',
                               color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                  {m.games.toLocaleString()}
                </span>
              </button>
            ))}
          </List>
        )}
      </Section>
    </>
  );
}

export function BookTree({ b, decimals = 1, onStudy }: {
  b: BookBrowse; decimals?: number;
  onStudy?: (kifu: string) => void;
}) {
  const root = keyOf(b.line);
  const rows: Row[] = [];
  rowsOf(b, root, 0, rows);

  return (
    <div style={{
      width: 'var(--w-book-tree)', flex: 'none', minHeight: 0,
      borderRight: '1px solid var(--border)', display: 'flex', flexDirection: 'column',
    }}>
      <TableHead cols={treeCols()} />
      <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: '0 var(--sp-3)' }}>
        {rows.length === 0 && <Empty>{t('book.empty.end_of_line')}</Empty>}
        {rows.map((r) => (
          <BookRow key={r.key} r={r} open={b.open.has(r.key)} decimals={decimals}
                   onToggle={() => b.toggle(r.key)}
                   onGo={() => b.goto(r.key)} />
        ))}
      </div>
      {onStudy && (
        <div style={{
          flex: 'none', padding: 'var(--sp-2) var(--sp-3)',
          borderTop: '1px solid var(--border-weak)',
        }}>
          <Button disabled={!b.line.length}
                  onClick={() => onStudy(b.line.map(sqName).join(''))}>{t('book.open_in_study')}</Button>
        </div>
      )}
    </div>
  );
}


function BookRow({ r, open, onToggle, onGo, decimals = 1 }: {
  r: Row; open: boolean; onToggle: () => void; onGo: () => void; decimals?: number;
}) {
  const leaf = r.child && r.child.moves.length === 0;
  return (
    <div className="k-row" style={{
      display: 'flex', alignItems: 'center', height: 'var(--h-row)',
      paddingLeft: r.depth * 16, borderRadius: 'var(--r-2)',
      borderBottom: '1px solid var(--border-weak)',
    }}>
      {leaf ? (
        <span style={{ width: 22, flex: 'none' }} />
      ) : (
        <button type="button" className="k-press" onClick={onToggle}
                title={open ? t('book.tree.collapse') : t('book.tree.expand')}
                aria-label={open ? t('book.tree.collapse') : t('book.tree.expand')}
                style={{
                  width: 22, height: 'var(--h-row)', flex: 'none', border: 0, padding: 0,
                  background: 'transparent', color: 'var(--sub)',
                  fontSize: 'var(--fs-3)', lineHeight: 1, borderRadius: 'var(--r-1)',
                }}>{open ? '▾' : '▸'}</button>
      )}
      <button type="button" onClick={onGo} className="k-row"
              title={t('book.row.goto')}
              style={{
                flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                border: 0, background: 'transparent', padding: '0 var(--sp-1)',
                fontSize: 'var(--fs-6)', color: 'var(--text)', textAlign: 'left',
              }}>
        <span style={{ width: 30, fontWeight: 600 }}>{r.name}</span>
        <span style={{ width: C_VALUE.w, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
          {r.value > 0 ? '+' : ''}{r.value.toFixed(decimals)}
        </span>
        <span style={{ width: C_GAMES.w, textAlign: 'right', fontSize: 'var(--fs-7)',
                       color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
          {r.games.toLocaleString()}
        </span>
        <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)', flex: 'none',
                       whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis',
                       color: r.child?.learned ? 'var(--gold)' : 'var(--sub)' }}>
          {r.child ? (r.child.learned ? t('book.src.learned') : t('book.src.book')) : ''}
        </span>
      </button>
    </div>
  );
}

const INITIAL = Array.from({ length: 64 }, (_, i) =>
  i === 3 * 8 + 4 || i === 4 * 8 + 3 ? 1 : i === 3 * 8 + 3 || i === 4 * 8 + 4 ? 2 : 0);

function parseLine(kifu: string): number[] {
  const out: number[] = [];
  const s = kifu.toLowerCase();
  for (let i = 0; i + 1 < s.length; i += 2) {
    const f = 'abcdefgh'.indexOf(s[i]);
    const r = +s[i + 1] - 1;
    if (f >= 0 && r >= 0 && r < 8) out.push(f * 8 + r);
  }
  return out;
}
