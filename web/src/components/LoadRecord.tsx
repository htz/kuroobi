import React from 'react';
import { t, type Lang } from '../i18n';
import { isOver, play, type Position } from '../lib/othello';
import { readRecord, type GameRecord } from '../lib/record';
import { Board } from './Board';
import { Button, Dialog, StoneDot, Tag } from './ui';

const FONT = 'var(--ff-web)';
const CODE = 'var(--ff-code)';
/** A record is a few kilobytes; anything near a megabyte is not one and would choke the text box. */
const FILE_MAX = 1 << 20;

async function fileText(f: File): Promise<{ text: string } | { tooBig: true }> {
  return f.size > FILE_MAX ? { tooBig: true } : { text: await f.text() };
}

/**
 * Pasted text, a chosen file or a dropped one, read as the app's load dialog reads it (lib/record.ts) and shown
 * as the board it ends on before it loads. `initial`, `file`, `big` and `dragging` fix what the canvas draws.
 */
export function LoadRecord({ lang, initial = '', file: file0 = '', big = false, dragging: drag0, inline, onLoad, onClose }: {
  lang: Lang; initial?: string; file?: string; big?: boolean; dragging?: boolean; inline?: boolean;
  onLoad: (r: GameRecord) => void; onClose: () => void;
}) {
  const L = (key: string, vars?: Record<string, string | number>) => t(lang, 'play.' + key, vars);
  const [text, setText] = React.useState(initial);
  const [file, setFile] = React.useState(file0);
  const [tooBig, setTooBig] = React.useState(big);
  const [dragging, setDragging] = React.useState(false);
  const picker = React.useRef<HTMLInputElement>(null);
  React.useEffect(() => { setText(initial); setFile(file0); setTooBig(big); }, [initial, file0, big]);

  const record = React.useMemo(() => (text.trim() ? readRecord(text) : null), [text]);
  const end = React.useMemo<Position | null>(
    () => record && record.moves.reduce<Position>((p, m) => play(p, m)!, record.start), [record]);
  const bad = tooBig || (!!text.trim() && !record);

  const take = async (f: File | undefined) => {
    if (!f) return;
    const got = await fileText(f);
    setFile(f.name);
    setTooBig('tooBig' in got);
    if ('text' in got) setText(got.text);
  };

  const discs = end ? { b: end.cells.filter((c) => c === 1).length, w: end.cells.filter((c) => c === 2).length } : null;
  const where = !end ? '' : isOver(end) ? L('loadOver') : L('toMove', { side: L(end.turn === 1 ? 'black' : 'white') });
  const over = drag0 || dragging;

  return (
    <Dialog title={L('load')} closeLabel={L('close')} onClose={onClose} inline={inline}
      actions={<>
        <Button icon="file" onClick={() => picker.current?.click()}>{L('loadFile')}</Button>
        <span style={{ marginLeft: 'auto', display: 'flex', gap: 8 }}>
          <Button onClick={onClose}>{L('cancel')}</Button>
          <Button kind="primary" disabled={!record || tooBig} onClick={() => record && onLoad(record)}>{L('loadApply')}</Button>
        </span>
      </>}>
      <input ref={picker} type="file" hidden accept=".ggf,.txt,.kifu,.json,text/plain,application/json"
        onChange={(e) => { void take(e.target.files?.[0]); e.target.value = ''; }} />
      <textarea value={text} rows={5} spellCheck={false} aria-label={L('load')} aria-invalid={bad}
        placeholder={L('loadPlaceholder')} className="k-input"
        onChange={(e) => { setText(e.target.value); setFile(''); setTooBig(false); }}
        onDragOver={(e) => { e.preventDefault(); setDragging(true); }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => { e.preventDefault(); setDragging(false); void take(e.dataTransfer.files[0]); }}
        style={{
          width: '100%', minHeight: 132, padding: '10px 12px', boxSizing: 'border-box', resize: 'vertical',
          borderRadius: 10, border: `1px ${over ? 'dashed' : 'solid'} ${over ? 'var(--accent)' : bad ? 'var(--bad)' : 'var(--border)'}`,
          background: over ? 'var(--card)' : 'var(--bg)', color: 'var(--text)', font: `var(--t-small)/1.5 ${CODE}`,
        }} />
      <div style={{ display: 'flex', alignItems: 'center', gap: 16, minHeight: 120 }}>
        <div style={{
          width: 120, height: 120, flex: 'none', borderRadius: 8, overflow: 'hidden', boxSizing: 'border-box',
          // Until something is read the slot shows as a slot, not as a hole in the layout.
          border: end ? 'none' : '1px dashed var(--border)',
        }}>
          {end && <Board lang={lang} cells={end.cells} coords={false} grain={false} disabled
            last={record!.moves.length ? record!.moves[record!.moves.length - 1] : null} />}
        </div>
        <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 8 }}>
          {(record || file) && (
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
              {record && !tooBig && <Tag>{L('format.' + record.format)}</Tag>}
              {file && <span style={{
                minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                font: `var(--t-meta) ${FONT}`, color: 'var(--sub)',
              }}>{file}</span>}
            </div>
          )}
          {bad && <span role="alert" style={{ color: 'var(--bad)' }}>{tooBig ? L('loadTooBig') : L('loadBad')}</span>}
          {end && discs && !tooBig && <>
            <span style={{ display: 'flex', alignItems: 'center', gap: 10, font: `var(--t-small) ${CODE}`, fontVariantNumeric: 'tabular-nums' }}>
              <StoneDot color="b" size={12} /><b>{discs.b}</b>
              <span style={{ color: 'var(--sub)' }}>–</span>
              <b>{discs.w}</b><StoneDot color="w" size={12} />
            </span>
            <span style={{ color: 'var(--sub)' }}>{[L('loadMoves', { n: record!.moves.length }), where].join(' · ')}</span>
          </>}
        </div>
      </div>
    </Dialog>
  );
}
