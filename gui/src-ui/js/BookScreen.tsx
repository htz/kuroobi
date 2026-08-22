import { useCallback, useEffect, useMemo, useState } from 'react';
import { api } from './api';
import { sqName } from './adapt';
import type { BookNode } from './types';
import { Board, type EvalInfo } from './components/board';
import { ScoreRow } from './components/data';
import { Col, Empty, EmptyState, List, Section, TableHead } from './components/layout';
import { Button } from './components/primitives';

/* Book browsing.
 *
 * Holds its own state, separate from games — a game board moving while
 * browsing would confuse which is which. Only the line is remembered
 * here; boards come from the backend replaying it (board rules never
 * mirror into JS). The list is a tree: a single level of candidates
 * hides how lines branch. */

/** The line as "f5d6" text; keys tree nodes. */
const keyOf = (line: number[]) => line.map(sqName).join('');

export interface BookBrowse {
  /** The line from the start position. */
  line: number[];
  /** The position being viewed. */
  node: BookNode | null;
  /** Fetched nodes, keyed by line text; renders the tree. */
  nodes: Record<string, BookNode>;
  /** Expanded nodes. */
  open: ReadonlySet<string>;
  toggle: (key: string) => void;
  err: string;
  push: (sq: number) => void;
  back: () => void;
  reset: () => void;
/** Replace the whole line (tree-row jumps and the smoke-test entry). */
  goto: (kifu: string) => void;
}

/** Tree columns; header and rows read the same array. Rows are
 *  hand-built (they carry the disclosure triangle) but take widths
 *  from here — the 44/58 pair used to be written twice. */
const TREE_COLS: Col[] = [
  { head: '手順' },
  { head: '評価', w: 44, right: true, num: true },
  { head: '出現', w: 58, right: true, num: true },
];
const [, C_VALUE, C_GAMES] = TREE_COLS;

export function useBookBrowse(on: boolean): BookBrowse {
  const [line, setLine] = useState<number[]>([]);
  const [nodes, setNodes] = useState<Record<string, BookNode>>({});
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set<string>());
  const [err, setErr] = useState('');

  const root = keyOf(line);

  /* Fetch the current node, expanded nodes, and children of visible
   * rows — the source column (file vs learned) needs the child node,
   * and leaving it blank until expansion would defeat the column. */
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
      .catch((e) => { if (alive) setErr('' + e); });
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

/** The board; only book moves are playable — leaving the book has no
 *  place here. */
export function BookPane({ b, coords, grain, flip, onSettings }: {
  b: BookBrowse; coords: boolean; grain: boolean; flip: boolean;
  /** Entry to the fix when no book file exists. */
  onSettings: () => void;
}) {
  const n = b.node;
  /* With no book at all, an empty board would lie ("no book beyond
   * this position") — show the disabled state with its remedy. */
  if (n && n.size === 0) {
    return (
      <EmptyState title="定石がありません"
                  body="定石ファイルを読み込めていません。設定から場所を指定できます。"
                  actions={<Button size="field" variant="primary" onClick={onSettings}>設定を開く</Button>} />
    );
  }
  // Candidates render as eval cells; the best gets the gold ring.
  const evals: Record<number, EvalInfo> = {};
  n?.moves.forEach((m, i) => {
    evals[m.pos] = { score: m.value, src: { book: true }, best: i === 0 };
  });
  return (
    // Children own the horizontal padding (the disc row's keyline
    // must reach the edges). minWidth:0 is required — the default
    // auto refuses to shrink below the board's intrinsic width.
    <div style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      {/* minmax(0,1fr) pins the row height; auto sizes by content and
          leaves height:100% without a basis, so the svg renders at its
          intrinsic 880px and overflows downward — noticed only when it
          covers the rows below. */}
          <div style={{
            flex: 1, minHeight: 0, display: 'grid', placeItems: 'center',
            gridTemplateRows: 'minmax(0, 1fr)', gridTemplateColumns: 'minmax(0, 1fr)',
            padding: 'var(--sp-2) var(--sp-4)',
          }}>
        {/* maxHeight too: wide containers are caught by maxWidth, but
            short ones let the grid row size to content and overflow
            downward the same way. */}
              <div style={{ height: '100%', maxHeight: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
          <Board cells={(n?.cells ?? INITIAL) as (0 | 1 | 2)[]}
                 legal={n?.moves.map((m) => m.pos) ?? []}
                 last={b.line.length ? b.line[b.line.length - 1] : null}
                 evals={evals} coords={coords} grain={grain} flip={flip}
                 onPlay={(sq) => { if (n?.moves.some((m) => m.pos === sq)) b.push(sq); }} />
        </div>
      </div>
      {/* The same disc row as play/study — moving it per screen makes
          the eye hunt. */}
      <ScoreRow black={n?.black ?? 2} white={n?.white ?? 2}
                turn={n?.player === 'white' ? 'w' : 'b'}
                meta={b.err || (n && n.moves.length === 0 ? 'この局面から先は定石にありません' : undefined)} />
    </div>
  );
}

/** One tree row. */
interface Row {
  key: string;
  depth: number;
  name: string;
  value: number;
  games: number;
  /** The node after this move; undefined until fetched. */
  child?: BookNode;
}

/** Flatten the tree below the current node, expanded branches only. */
function rowsOf(b: BookBrowse, key: string, depth: number, out: Row[]): void {
  const n = b.nodes[key];
  if (!n) return;
  for (const m of n.moves) {
    const k = key + sqName(m.pos);
    out.push({ key: k, depth, name: sqName(m.pos), value: m.value, games: m.games, child: b.nodes[k] });
    if (b.open.has(k)) rowsOf(b, k, depth + 1, out);
  }
}

/** The line and the tree below it. */
export function BookDock({ b, decimals = 1 }: { b: BookBrowse; decimals?: number }) {
  const n = b.node;

  return (
    <>
      <Section title="この局面" aside={<span>{b.line.length} 手</span>}>
        <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.9, wordBreak: 'break-all' }}>
          {b.line.length ? b.line.map(sqName).join(' ') : '初期局面'}
        </div>
        {/* The position's own value, with its search depth — without
            it, shallow and deep branches wear the same face. */}
        {n?.value != null && (
          <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)' }}>
            <b style={{ fontSize: 'var(--fs-1)', fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>
              {n.value > 0 ? '+' : ''}{n.value.toFixed(decimals)}
            </b>
            <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
              石差{n.depth != null && ` · ${n.depth} 手読み`}
            </span>
          </div>
        )}
        {/* Source: file values and game-learned values carry different
            weight (learned ones may rest on a handful of games). */}
        {n?.value != null && (
          <div style={{ display: 'flex', alignItems: 'center', fontSize: 'var(--fs-5)' }}>
            <span style={{ color: 'var(--sub)' }}>出所</span>
            <span style={{ marginLeft: 'auto', color: n.learned ? 'var(--gold)' : undefined }}>
              {n.learned ? '実戦の学習' : 'book.txt'}
            </span>
          </div>
        )}
      </Section>

      {/* The tree lives in the left column; this table shows only the
          next move — two copies of one list compete. */}
      <Section title="次の手" aside={n ? <span>{n.moves.length}</span> : undefined}>
        {/* Render the empty state even without `n` — `n && ...` left
            not-in-book positions as a bare heading that looked like a
            load failure. "No continuation" and "not in the book" get
            distinct wording. */}
        {!n || n.moves.length === 0 ? (
          /* Never a raw span; empty sections use Empty, which owns
             its color and spacing. */
          <Empty>{n ? 'この局面から先は定石にありません。'
                    : 'この局面は定石に載っていません。'}</Empty>
        ) : (
          <List>
            {n?.moves.map((m) => (
              <button key={m.pos} type="button" className="k-row" onClick={() => b.push(m.pos)}
                      title="押すとこの手へ進む"
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

/* Left column: the book tree (269px, indented 24px rows). Inside the
 * dock it shared a frame with the position pane and branches were
 * unreadable. The design assumed a nav-less window; on the main
 * screen the board keeps 473px instead of 560 — readable branches
 * won. */
export function BookTree({ b, decimals = 1, onStudy }: {
  b: BookBrowse; decimals?: number;
  /** Open the current line in study (design puts this at the column
   *  bottom). */
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
      {/* Column header; same 20px + keyline as Section, with the two
          numeric columns right-aligned. */}
      <TableHead cols={TREE_COLS} />
      <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: '0 var(--sp-3)' }}>
        {rows.length === 0 && <Empty>この局面から先は定石にありません。</Empty>}
        {rows.map((r) => (
          <BookRow key={r.key} r={r} open={b.open.has(r.key)} decimals={decimals}
                   onToggle={() => b.toggle(r.key)}
                   onGo={() => b.goto(r.key)} />
        ))}
      </div>
      {/* Column bottom: hand the traced line to study — questioning a
          branch naturally leads to reading it yourself. Disabled with
          nothing traced. */}
      {onStudy && (
        <div style={{
          flex: 'none', padding: 'var(--sp-2) var(--sp-3)',
          borderTop: '1px solid var(--border-weak)',
        }}>
          <Button disabled={!b.line.length}
                  onClick={() => onStudy(b.line.map(sqName).join(''))}>検討で開く</Button>
        </div>
      )}
    </div>
  );
}


function BookRow({ r, open, onToggle, onGo, decimals = 1 }: {
  r: Row; open: boolean; onToggle: () => void; onGo: () => void; decimals?: number;
}) {
  // Continuations are unknown until fetched; fetched-and-empty hides
  // the triangle.
  const leaf = r.child && r.child.moves.length === 0;
  return (
    // Row body and triangle are siblings (no button inside a button).
    // Indent is 16px per level (12px made level 3 align with its
    // parent); the bottom keyline separates the 24px rows.
    <div className="k-row" style={{
      display: 'flex', alignItems: 'center', height: 'var(--h-row)',
      paddingLeft: r.depth * 16, borderRadius: 'var(--r-2)',
      borderBottom: '1px solid var(--border-weak)',
    }}>
      {/* The triangle's hit area spans the row height, and the glyph
          itself is enlarged — a big-enough target still reads as
          unpressable when the drawing is tiny. */}
      {leaf ? (
        <span style={{ width: 22, flex: 'none' }} />
      ) : (
        <button type="button" className="k-press" onClick={onToggle}
                title={open ? '閉じる' : '先を開く'} aria-label={open ? '閉じる' : '先を開く'}
                style={{
                  width: 22, height: 'var(--h-row)', flex: 'none', border: 0, padding: 0,
                  background: 'transparent', color: 'var(--sub)',
                  fontSize: 'var(--fs-3)', lineHeight: 1, borderRadius: 'var(--r-1)',
                }}>{open ? '▾' : '▸'}</button>
      )}
      {/* Attach the state layer: the triangle glowed on hover while
          the name — equally pressable — did not react. */}
      <button type="button" onClick={onGo} className="k-row"
              title="押すとこの局面へ"
              style={{
                flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                border: 0, background: 'transparent', padding: '0 var(--sp-1)',
                fontSize: 'var(--fs-6)', color: 'var(--text)', textAlign: 'left',
              }}>
        <span style={{ width: 30, fontWeight: 600 }}>{r.name}</span>
        <span style={{ width: C_VALUE.w, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
          {r.value > 0 ? '+' : ''}{r.value.toFixed(decimals)}
        </span>
        {/* Thousands separators — five-plus digits are unreadable raw
            (the design writes 12,480 too). */}
        <span style={{ width: C_GAMES.w, textAlign: 'right', fontSize: 'var(--fs-7)',
                       color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
          {r.games.toLocaleString()}
        </span>
        {/* Source; game-learned branches are also color-coded. */}
        <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)',
                       color: r.child?.learned ? 'var(--gold)' : 'var(--sub)' }}>
          {r.child ? (r.child.learned ? '定石·学' : '定石') : ''}
        </span>
      </button>
    </div>
  );
}

const INITIAL = Array.from({ length: 64 }, (_, i) =>
  i === 3 * 8 + 4 || i === 4 * 8 + 3 ? 1 : i === 3 * 8 + 3 || i === 4 * 8 + 4 ? 2 : 0);

/** "f5d6" -> square indices; unreadable characters are dropped. */
function parseLine(kifu: string): number[] {
  const out: number[] = [];
  const t = kifu.toLowerCase();
  for (let i = 0; i + 1 < t.length; i += 2) {
    const f = 'abcdefgh'.indexOf(t[i]);
    const r = +t[i + 1] - 1;
    if (f >= 0 && r >= 0 && r < 8) out.push(f * 8 + r);
  }
  return out;
}
