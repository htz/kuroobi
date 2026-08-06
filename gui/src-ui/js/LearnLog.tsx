import { useState } from 'react';
import type { LearnChange, LearnEntry } from './types';
import { Section } from './components/layout';
import { Button } from './components/primitives';

/* 定石に取り込んだ対局の控え。
 *
 * 取り込みは裏で静かに進み、値を上書きする。変な対局が 1 局混ざるだけで
 * 以後の手が変わるので、**何がどう変わったか**まで辿れないと見つけられない。
 * 対局 → 敗着 → 書き換えた手 (旧→新) が一本で追える形にしてある。
 */

/** その手で損した石差。定石が「もっと良い手があった」と言っている量。 */
const lossOf = (c: LearnChange) => c.best - c.after;

/** いちばん損した手。全部が最善なら無し。 */
function blunderOf(e: LearnEntry): LearnChange | undefined {
  let worst: LearnChange | undefined;
  for (const c of e.changes) {
    if (lossOf(c) > 0.05 && (!worst || lossOf(c) > lossOf(worst))) worst = c;
  }
  return worst;
}

const sign = (v: number) => (v > 0 ? '+' : '') + v.toFixed(1);

export function LearnLog({ items, onOpen, onUndo }: {
  items: LearnEntry[];
  /** 検討で開く。ply を渡すとその手数まで進める */
  onOpen: (e: LearnEntry, ply?: number) => void;
  /** 取り込みを取り消す。 */
  onUndo: (e: LearnEntry) => void;
}) {
  const [open, setOpen] = useState<string>('');
  return (
    <Section title="取り込んだ対局" aside={items.length ? <span>{items.length}</span> : undefined}>
      {items.length === 0 && (
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>まだありません。</span>
      )}
      {items.map((e) => {
        const key = e.at + e.kifu;
        const bad = blunderOf(e);
        const shown = open === key;
        return (
          <div key={key} style={{ display: 'flex', flexDirection: 'column' }}>
            <div className="k-row" style={{
              display: 'flex', alignItems: 'center', height: 'var(--h-row)',
              borderRadius: 'var(--r-2)',
            }}>
              {/* 明細を開く三角と、検討で開く行の本体は兄弟にする
                  (button の中に button は置けない) */}
              <button type="button" className="k-press"
                      onClick={() => setOpen(shown ? '' : key)}
                      title={shown ? '閉じる' : '書き換えた手を見る'}
                      aria-label={shown ? '閉じる' : '書き換えた手を見る'}
                      disabled={!e.changes.length}
                      style={{
                        width: 16, height: 16, flex: 'none', border: 0, padding: 0,
                        background: 'transparent', color: 'var(--sub)',
                        cursor: e.changes.length ? 'pointer' : 'default',
                        fontSize: 'var(--fs-7)', lineHeight: 1, borderRadius: 'var(--r-1)',
                        opacity: e.changes.length ? 1 : 0,
                      }}>{shown ? '▾' : '▸'}</button>
              <button type="button" onClick={() => onOpen(e)} title="検討で開く"
                      style={{
                        flex: 1, minWidth: 0, display: 'flex', alignItems: 'center',
                        gap: 'var(--sp-2)', border: 0, background: 'transparent',
                        cursor: 'pointer', padding: '0 var(--sp-1)',
                        fontSize: 'var(--fs-6)', color: 'var(--text)', textAlign: 'left',
                      }}>
                <span style={{ color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                  {fmtWhen(e.at)}
                </span>
                <span style={{ fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>
                  {e.black}–{e.white}
                </span>
                {/* 相手の名前があれば GGS の対局。無ければローカル */}
                {e.opponent && (
                  <span style={{ color: 'var(--sub)', overflow: 'hidden', textOverflow: 'ellipsis',
                                 whiteSpace: 'nowrap', maxWidth: 80 }}>{e.opponent}</span>
                )}
                <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)',
                               color: bad ? 'var(--bad)' : 'var(--sub)',
                               fontVariantNumeric: 'tabular-nums' }}>
                  {bad ? `${bad.ply} 手 ▼${lossOf(bad).toFixed(1)}` : `${e.positions} 局面`}
                </span>
              </button>
            </div>

            {/* 取り消しは対局単位。1 手だけ戻すと、その局面から根までの
                値がその手を前提にしたままになる (書き戻しは連なっている) */}
            {shown && (
              <div style={{ display: 'flex', gap: 'var(--sp-2)', padding: 'var(--sp-1) 0 var(--sp-1) 20px' }}>
                <Button size="chip" variant="danger" onClick={() => onUndo(e)}>
                  この対局の取り込みを取り消す
                </Button>
              </div>
            )}
            {shown && e.changes.map((c) => (
              <button key={c.ply} type="button" className="k-row"
                      onClick={() => onOpen(e, c.ply)}
                      title="その手の局面を検討で開く"
                      style={{
                        display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                        height: 'var(--h-row)', paddingLeft: 20, border: 0,
                        background: 'transparent', cursor: 'pointer',
                        borderRadius: 'var(--r-2)', textAlign: 'left',
                        fontSize: 'var(--fs-7)', color: 'var(--sub)',
                      }}>
                <span style={{ width: 26, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {c.ply}
                </span>
                <span style={{ width: 24, color: 'var(--text)', fontWeight: 600 }}>{c.mv}</span>
                {/* 旧→新。定石に無かった手は「新規」 */}
                <span style={{ fontVariantNumeric: 'tabular-nums' }}>
                  {c.before === null ? '新規' : sign(c.before)} → <b style={{ color: 'var(--text)' }}>{sign(c.after)}</b>
                </span>
                {lossOf(c) > 0.05 && (
                  <span style={{ marginLeft: 'auto', color: 'var(--bad)',
                                 fontVariantNumeric: 'tabular-nums' }}>
                    ▼{lossOf(c).toFixed(1)}
                  </span>
                )}
              </button>
            ))}
          </div>
        );
      })}
    </Section>
  );
}

/** 取り込んだ時刻。今日のものは時刻だけ、それ以外は日付だけにする —
 *  並べたときに縦が揃い、かつ「さっき入ったもの」がすぐ分かる。 */
export function fmtWhen(secs: number): string {
  const d = new Date(secs * 1000);
  const now = new Date();
  const sameDay = d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth()
    && d.getDate() === now.getDate();
  const p2 = (n: number) => String(n).padStart(2, '0');
  return sameDay ? `${p2(d.getHours())}:${p2(d.getMinutes())}`
    : `${d.getMonth() + 1}/${p2(d.getDate())}`;
}
