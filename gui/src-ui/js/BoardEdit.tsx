import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Board, type Cell } from './components/board';
import { BoardFit, Divider, Dock, Section } from './components/layout';
import { Button, Segmented } from './components/primitives';
import { ScoreRow, StoneDot } from './components/data';
import { t } from './i18n';


type Side = 'b' | 'w';
type Pos = { cells: Cell[]; turn: Side };
type Tool = 0 | 1 | 2;   // erase / black / white

const START: Pos = {
  cells: Array.from({ length: 64 }, (_, sq) => (sq === 27 || sq === 36 ? 2 : sq === 28 || sq === 35 ? 1 : 0)),
  turn: 'b',
};

const DIRS = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [1, -1], [-1, 1], [-1, -1]];

export function legalOf(cells: Cell[], side: Side): number[] {
  const me = side === 'b' ? 1 : 2, op = 3 - me, out: number[] = [];
  const at = (f: number, r: number) => (f < 0 || f > 7 || r < 0 || r > 7 ? -1 : cells[f * 8 + r]);
  for (let sq = 0; sq < 64; sq++) {
    if (cells[sq]) continue;
    const f = Math.floor(sq / 8), r = sq % 8;
    if (DIRS.some(([df, dr]) => {
      let x = f + df, y = r + dr, n = 0;
      while (at(x, y) === op) { x += df; y += dr; n++; }
      return n > 0 && at(x, y) === me;
    })) out.push(sq);
  }
  return out;
}

/** The engine's board text: a1, b1 … h8, then the side to move. */
export function boardText({ cells, turn }: Pos): string {
  let s = '';
  for (let i = 0; i < 64; i++) s += '-xo'[cells[(i % 8) * 8 + Math.floor(i / 8)]];
  return s + ' ' + (turn === 'b' ? 'x' : 'o');
}

type EditState = { pos: Pos; past: Pos[]; future: Pos[]; tool: Tool };

export function useBoardEdit() {
  const [st, setSt] = useState<EditState | null>(null);
  // What the press that is down puts on each square it crosses, and the board before it (one undo step).
  const stroke = useRef<{ v: Cell; before: Pos } | null>(null);

  const endStroke = useCallback(() => {
    const s0 = stroke.current;
    stroke.current = null;
    if (s0) setSt((s) => (s && s.pos !== s0.before ? { ...s, past: [...s.past, s0.before], future: [] } : s));
  }, []);
  useEffect(() => {
    window.addEventListener('pointerup', endStroke);
    window.addEventListener('pointercancel', endStroke);
    return () => {
      window.removeEventListener('pointerup', endStroke);
      window.removeEventListener('pointercancel', endStroke);
    };
  }, [endStroke]);

  const open = useCallback((cells: Cell[], player: string) => {
    setSt({ pos: { cells: [...cells], turn: player === 'white' ? 'w' : 'b' }, past: [], future: [], tool: 1 });
  }, []);
  const close = useCallback(() => setSt(null), []);

  // One undo step each: a stroke, a button, a turn change.
  const change = useCallback((next: (p: Pos) => Pos) => setSt((s) => {
    if (!s) return s;
    const pos = next(s.pos);
    if (pos.turn === s.pos.turn && pos.cells.every((c, i) => c === s.pos.cells[i])) return s;
    return { ...s, pos, past: [...s.past, s.pos], future: [] };
  }), []);

  const paint = (sq: number, v: Cell) => setSt((s) => {
    if (!s || s.pos.cells[sq] === v) return s;
    const cells = [...s.pos.cells];
    cells[sq] = v;
    return { ...s, pos: { ...s.pos, cells } };
  });

  const onDown = (sq: number) => {
    if (!st) return;
    endStroke();   // a release outside the window never reached us
    const v: Cell = st.tool !== 0 && st.pos.cells[sq] === st.tool ? 0 : st.tool;
    stroke.current = { v, before: st.pos };
    paint(sq, v);
  };
  const onEnter = (sq: number) => {
    if (stroke.current) paint(sq, stroke.current.v);
  };

  const undo = useCallback(() => setSt((s) => {
    if (!s || !s.past.length) return s;
    return { ...s, pos: s.past[s.past.length - 1], past: s.past.slice(0, -1), future: [s.pos, ...s.future] };
  }), []);
  const redo = useCallback(() => setSt((s) => {
    if (!s || !s.future.length) return s;
    return { ...s, pos: s.future[0], past: [...s.past, s.pos], future: s.future.slice(1) };
  }), []);
  const setTool = useCallback((tool: Tool) => setSt((s) => (s ? { ...s, tool } : s)), []);

  const legal = useMemo(() => (st ? {
    mine: legalOf(st.pos.cells, st.pos.turn),
    theirs: legalOf(st.pos.cells, st.pos.turn === 'b' ? 'w' : 'b'),
  } : null), [st]);

  return {
    st, open, close, undo, redo, setTool, onDown, onEnter,
    setTurn: (turn: Side) => change((p) => ({ ...p, turn })),
    reset: () => change(() => START),
    clear: () => change((p) => ({ ...p, cells: Array(64).fill(0) as Cell[] })),
    swap: () => change((p) => ({
      cells: p.cells.map((c) => (c === 0 ? 0 : 3 - c) as Cell),
      turn: p.turn === 'b' ? 'w' : 'b',
    })),
    legal: legal?.mine ?? [],
    dead: !!legal && !legal.mine.length && !legal.theirs.length,
  };
}

export type BoardEditor = ReturnType<typeof useBoardEdit>;

/** Keys while editing: 1–3 pick the stone, ⌘Z / ⇧⌘Z, Esc leaves, ⌘↩ plays from here. */
export function useEditKeys(ed: BoardEditor, onCancel: () => void, onPlay: () => void) {
  const on = !!ed.st;
  const { setTool, undo, redo } = ed;
  useEffect(() => {
    if (!on) return;
    const onKey = (e: KeyboardEvent) => {
      if (document.querySelector('[data-k-overlay]')) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return;
      const cmd = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      if (cmd && key === 'z') { e.preventDefault(); if (e.shiftKey) redo(); else undo(); return; }
      if (cmd && e.key === 'Enter') { e.preventDefault(); onPlay(); return; }
      if (e.key === 'Escape') { e.preventDefault(); onCancel(); return; }
      if (!cmd && !e.altKey && ['1', '2', '3'].includes(e.key)) {
        setTool(e.key === '1' ? 1 : e.key === '2' ? 2 : 0);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [on, setTool, undo, redo, onCancel, onPlay]);
}

export function EditTools({ ed }: { ed: BoardEditor }) {
  return (
    <>
      <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600, whiteSpace: 'nowrap' }}>{t('edit.title')}</span>
      <Divider />
      <Button variant="ghost" title="⌘Z" disabled={!ed.st?.past.length} onClick={ed.undo}>{t('edit.undo')}</Button>
      <Button variant="ghost" title="⇧⌘Z" disabled={!ed.st?.future.length} onClick={ed.redo}>{t('edit.redo')}</Button>
    </>
  );
}

export function EditBoard({ ed, coords, grain, flip }: {
  ed: BoardEditor; coords: boolean; grain: boolean; flip: boolean;
}) {
  if (!ed.st) return null;
  const { cells, turn } = ed.st.pos;
  const n = (v: Cell) => cells.filter((c) => c === v).length;
  return (
    <div style={{ flex: 1, minHeight: 'calc(200px + var(--h-bar))', display: 'flex', flexDirection: 'column' }}>
      <BoardFit minHeight={200} padding="var(--sp-2) var(--sp-4)">
        <Board cells={cells} legal={ed.legal} turn={turn} coords={coords} grain={grain} flip={flip} disabled
               edit={{ ghost: ed.st.tool, onDown: ed.onDown, onEnter: ed.onEnter }} />
      </BoardFit>
      <ScoreRow black={n(1)} white={n(2)} turn={turn} meta={t('edit.empty', { n: n(0) })} />
    </div>
  );
}

export function EditDock({ ed, open, onCancel, onStudy, onPlay }: {
  ed: BoardEditor; open: boolean; onCancel: () => void; onStudy: () => void; onPlay: () => void;
}) {
  if (!ed.st) return null;
  const { tool, pos } = ed.st;
  const side = t(pos.turn === 'b' ? 'app.color.black' : 'app.color.white');
  const status = ed.dead ? { text: t('edit.dead'), color: 'var(--bad)' }
    : !ed.legal.length ? { text: t('edit.pass', { side }), color: 'var(--gold)' }
    : { text: t('edit.moves', { side, n: ed.legal.length }), color: 'var(--sub)' };
  const tile = (v: Tool, key: string, label: string) => (
    <button type="button" className="k-tile" aria-pressed={tool === v} onClick={() => ed.setTool(v)}>
      <kbd>{key}</kbd>
      {v === 0 ? <span className="k-tile-erase" /> : <StoneDot color={v === 1 ? 'b' : 'w'} size={24} />}
      {label}
    </button>
  );
  return (
    <Dock tabs={[{ id: 'edit', label: '' }]} active="edit" open={open} scroll={false}>
      <div className="k-scroll" style={{ flex: 1, minHeight: 0, paddingTop: 'var(--sp-3)' }}>
        <Section title={t('edit.tool')}>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: 'var(--sp-2)' }}>
            {tile(1, '1', t('edit.black'))}
            {tile(2, '2', t('edit.white'))}
            {tile(0, '3', t('edit.erase'))}
          </div>
        </Section>
        <Section title={t('edit.turn')}>
          <div>
            <Segmented fill value={pos.turn} onChange={ed.setTurn} options={[
              { value: 'b', label: <><StoneDot color="b" />{t('edit.black_turn')}</> },
              { value: 'w', label: <><StoneDot color="w" />{t('edit.white_turn')}</> },
            ]} />
            <div style={{ marginTop: 'var(--sp-2)', fontSize: 'var(--fs-6)', lineHeight: 1.5, color: status.color }}>
              {status.text}
            </div>
          </div>
        </Section>
        <Section title={t('edit.board')}>
          <div className="k-fill" style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: 'var(--sp-2)' }}>
            <Button onClick={ed.reset}>{t('edit.initial')}</Button>
            <Button onClick={ed.clear}>{t('edit.clear')}</Button>
            <Button title={t('edit.swap_title')} onClick={ed.swap}>{t('edit.swap')}</Button>
          </div>
        </Section>
      </div>
      <div className="k-fill" style={{
        flex: 'none', display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 'var(--sp-2)',
        padding: 'var(--sp-3)', borderTop: '1px solid var(--border-weak)',
      }}>
        <div style={{ gridColumn: '1 / -1', display: 'grid' }} className="k-fill">
          <Button variant="primary" title="⌘↩" disabled={ed.dead} onClick={onPlay}>{t('edit.play')}</Button>
        </div>
        <Button variant="ghost" title="Esc" onClick={onCancel}>{t('edit.cancel')}</Button>
        <Button disabled={ed.dead} onClick={onStudy}>{t('edit.study')}</Button>
      </div>
    </Dock>
  );
}
