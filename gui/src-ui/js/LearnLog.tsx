import { useEffect, useState } from 'react';
import { api, type KifuFrame } from './api';
import type { LearnChange, LearnEntry } from './types';
import { Board } from './components/board';
import { Col, Empty, KeyValue, List, Section, TableHead, TableRow } from './components/layout';
import { Button, Segmented, Select } from './components/primitives';
import { t } from './i18n';


const lossOf = (c: LearnChange) => c.best - c.after;

const logCols = (): Col[] => [
  { head: t('learn.col.game'), clip: true },
  { head: t('learn.col.discs'), w: 52, right: true, num: true },
  { head: t('learn.col.positions'), w: 64, right: true, num: true },
];

function blunderOf(e: LearnEntry): LearnChange | undefined {
  let worst: LearnChange | undefined;
  for (const c of e.changes) {
    if (lossOf(c) > 0.05 && (!worst || lossOf(c) > lossOf(worst))) worst = c;
  }
  return worst;
}

const sign = (v: number) => (v > 0 ? '+' : '') + v.toFixed(1);
const keyOf = (e: LearnEntry) => e.at + e.kifu;

export function LearnLog({ items: all, onOpen, onUndo, onBook }: {
  items: LearnEntry[];
  onOpen: (e: LearnEntry, ply?: number) => void;
  onUndo: (e: LearnEntry) => void;
  onBook?: (kifu: string) => void;
}) {
  const [sel, setSel] = useState('');
  const [days, setDays] = useState('0');
  const [only, setOnly] = useState<'all' | 'lost'>('all');
  const [now] = useState(() => Date.now());
  const lostOf = (e: LearnEntry) =>
    e.my_color === 'b' ? e.black < e.white
      : e.my_color === 'w' ? e.white < e.black
        : false;
  const byDays = days === '0' ? all
    : all.filter((e) => e.at * 1000 >= now - Number(days) * 86400_000);
  const items = only === 'lost' ? byDays.filter(lostOf) : byDays;
  const hidden = all.length - items.length;

  const cur = items.find((e) => keyOf(e) === sel) ?? items[0];
  const bad = cur ? blunderOf(cur) : undefined;

  const [frames, setFrames] = useState<{ text: string; f: KifuFrame[] }>({ text: '', f: [] });
  const text = cur ? (cur.start ? cur.start + '\n' + cur.kifu : cur.kifu) : '';
  useEffect(() => {
    if (!text) return;
    let alive = true;
    void api.previewKifu(text)
      .then((f) => { if (alive) setFrames({ text, f }); })
      .catch(() => { if (alive) setFrames({ text, f: [] }); });
    return () => { alive = false; };
  }, [text]);
  const shown = frames.text === text ? frames.f : [];
  const frame = shown.length
    ? shown[Math.min(bad?.ply ?? shown.length - 1, shown.length - 1)]
    : null;

  if (!items.length || !cur) {
    return (
      <Section title={t('learn.section.imported')}>
        {hidden > 0 ? (
          <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
            <Empty>{t('learn.empty.filtered', { n: hidden })}</Empty>
            <Button size="ctrl" onClick={() => setDays('0')}>{t('learn.empty.show_all')}</Button>
          </div>
        ) : (
          <Empty>{t('learn.empty.none')}</Empty>
        )}
      </Section>
    );
  }

  const cols = logCols();
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <div style={{
        width: 'var(--w-book-tree)', flex: 'none', minHeight: 0,
        borderRight: '1px solid var(--border)', display: 'flex', flexDirection: 'column',
      }}>
        <div style={{
          height: 'var(--h-field)', flex: 'none', display: 'flex', alignItems: 'center',
          padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
        }}>
          <Segmented value={only} onChange={(v) => setOnly(v as 'all' | 'lost')}
                     options={[{ value: 'all', label: t('learn.filter.all') },
                               { value: 'lost', label: t('learn.filter.lost') }]} />
          <span style={{ width: 'var(--sp-2)' }} />
          <Select size="ctrl" value={days} onChange={setDays} options={[
            ['0', t('learn.filter.all')], ['7', t('learn.filter.days_7')],
            ['30', t('learn.filter.days_30')], ['90', t('learn.filter.days_90')],
          ]} />
        </div>
        <TableHead cols={cols} />
        <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: '0 var(--sp-3)' }}>
          <List>
            {items.map((e) => {
              const on = keyOf(e) === keyOf(cur);
              return (
                <TableRow key={keyOf(e)} cols={cols} on={on} pad="var(--sp-1)" fs="var(--fs-6)"
                          onClick={() => setSel(keyOf(e))}>
                  <span>
                    <span style={{ color: 'var(--sub)' }}>{fmtWhen(e.at)}</span>
                    {' '}{e.opponent || 'KUROOBI'}
                  </span>
                  <span>{e.black}–{e.white}</span>
                  <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{e.positions}</span>
                </TableRow>
              );
            })}
          </List>
        </div>
        <div style={{
          flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
          padding: 'var(--sp-2) var(--sp-3)', borderTop: '1px solid var(--border)',
          fontSize: 'var(--fs-6)', color: 'var(--sub)',
        }}>
          <span>{hidden > 0 ? t('learn.total.games_hidden', { n: items.length, hidden })
            : t('learn.total.games', { n: items.length })}</span>
          <span style={{
            marginLeft: 'auto', color: 'var(--text)', fontWeight: 600,
            fontVariantNumeric: 'tabular-nums',
          }}>{items.reduce((n, e) => n + e.positions, 0).toLocaleString()}</span>
          <span>{t('learn.col.positions')}</span>
        </div>
      </div>

      <BlunderPane cur={cur} bad={bad} frame={frame} onOpen={onOpen} />

      <div className="k-scroll" style={{
        width: 'var(--w-dock)', flex: 'none', minHeight: 0,
        borderLeft: '1px solid var(--border)', padding: 'var(--sp-3) 0',
      }}>
        <Section title={t('learn.section.game')}>
          <KeyValue label={t('learn.game.opponent')} value={cur.opponent || 'KUROOBI'} />
          <KeyValue label={t('learn.col.discs')} value={`${cur.black} – ${cur.white}`} />
          <KeyValue label={t('learn.game.imported')}
                    value={t('learn.game.imported_value', { n: cur.positions, when: fmtWhen(cur.at) })} />
        </Section>

        <Section title={t('learn.section.changes')} aside={<span>{cur.changes.length}</span>}>
          {!cur.changes.length && (
            <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
              {t('learn.changes.old_entry')}
            </span>
          )}
          <List>
            {cur.changes.map((c) => (
              <button key={c.ply} type="button" className="k-row"
                      onClick={() => onOpen(cur, c.ply)}
                      title={t('learn.changes.open_move')}
                      style={{
                        display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                        height: 'var(--h-row)', border: 0, background: 'transparent',
                        borderBottom: '1px solid var(--border-weak)', borderRadius: 'var(--r-2)',
                        padding: '0 var(--sp-1)', cursor: 'pointer', textAlign: 'left',
                        fontSize: 'var(--fs-6)', color: 'var(--sub)',
                      }}>
                <span style={{ width: 26, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {c.ply}
                </span>
                <span style={{ width: 24, color: 'var(--text)', fontWeight: 600 }}>{c.mv}</span>
                <span style={{ marginLeft: 'auto', fontVariantNumeric: 'tabular-nums' }}>
                  {c.before === null ? t('learn.changes.new') : sign(c.before)}
                  {' → '}
                  <b style={{ color: 'var(--text)' }}>{sign(c.after)}</b>
                </span>
              </button>
            ))}
          </List>
        </Section>

        <div style={{ display: 'flex', gap: 'var(--sp-2)', padding: '0 var(--sp-3)' }}>
          {onBook && <Button onClick={() => onBook(cur.kifu)}>{t('learn.action.show_in_book')}</Button>}
          <Button variant="danger" disabled={!cur.changes.length}
                  onClick={() => onUndo(cur)}>{t('learn.action.undo')}</Button>
        </div>
      </div>
    </div>
  );
}


function fmtWhen(secs: number): string {
  const d = new Date(secs * 1000);
  const now = new Date();
  const sameDay = d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth()
    && d.getDate() === now.getDate();
  const p2 = (n: number) => String(n).padStart(2, '0');
  return sameDay ? `${p2(d.getHours())}:${p2(d.getMinutes())}`
    : `${d.getMonth() + 1}/${p2(d.getDate())}`;
}

function BlunderPane({ cur, bad, frame, onOpen }: {
  cur: LearnEntry;
  bad: LearnChange | undefined;
  frame: KifuFrame | null;
  onOpen: (e: LearnEntry, ply?: number) => void;
}) {
  return (
    <div style={{
      flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column',
      padding: 'var(--sp-4)', gap: 'var(--sp-3)',
    }}>
      <div style={{
        flex: 1, minHeight: 0, display: 'grid', placeItems: 'center',
        gridTemplateRows: 'minmax(0, 1fr)', gridTemplateColumns: 'minmax(0, 1fr)',
      }}>
        <div style={{ height: '100%', maxHeight: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
          {frame && (
            <Board cells={frame.cells as (0 | 1 | 2)[]} last={frame.last} coords={false} disabled />
          )}
        </div>
      </div>
      {bad ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-2)' }}>
            <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>
              {t('learn.blunder.move', { n: bad.ply, move: bad.mv })}
            </span>
            <span style={{ color: 'var(--bad)', fontVariantNumeric: 'tabular-nums' }}>
              ▼{lossOf(bad).toFixed(1)}
            </span>
            <span style={{ marginLeft: 'auto' }}>
              <Button onClick={() => onOpen(cur, bad.ply)}>{t('learn.blunder.open_study')}</Button>
            </span>
          </div>
          <span style={{
            maxWidth: 'var(--w-text)', fontSize: 'var(--fs-6)',
            color: 'var(--sub)', lineHeight: 1.8,
          }}>
            {t('learn.blunder.note', { best: sign(bad.best), after: sign(bad.after) })}
          </span>
        </div>
      ) : (
        <Empty>{t('learn.blunder.none')}</Empty>
      )}
    </div>
  );
}
