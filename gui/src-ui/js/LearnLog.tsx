import { useEffect, useState } from 'react';
import { api, type KifuFrame } from './api';
import type { LearnChange, LearnEntry } from './types';
import { Board } from './components/board';
import { Empty, List, Section, TableHead, TableRow } from './components/layout';
import { Button } from './components/primitives';

/* 定石に取り込んだ対局の控え。設計 §8 の三面。
 *
 * 取り込みは裏で静かに進み、値を上書きする。変な対局が 1 局混ざるだけで
 * 以後の手が変わるので、**何がどう変わったか**まで辿れないと見つけられない。
 * 対局 → 敗着 → 書き換えた手 (旧→新) → 取り消し が一本で追える形にしてある。
 *
 * 左に対局の一覧、中央に敗着の局面、右にその対局の明細。行を開いて中身を
 * 出す形にしていたが、**盤を置く場所が無く**、どの手で損したのかを数字だけで
 * 読ませていた。
 *
 * **絵にあって作れないもの** (`notes/design-sync-from-impl.md` に数え上げた):
 * この対局の評価値グラフ (控えに 1 手ごとの評価値が無い) / 取り込みの状態
 * 「完了」/「対象外」の絞り込み / 勝敗 (自分がどちらの色だったかを控えが
 * 持っていないので石数だけ出す)。
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
const keyOf = (e: LearnEntry) => e.at + e.kifu;

export function LearnLog({ items, onOpen, onUndo, onBook }: {
  items: LearnEntry[];
  /** 検討で開く。ply を渡すとその手数まで進める */
  onOpen: (e: LearnEntry, ply?: number) => void;
  /** 取り込みを取り消す。 */
  onUndo: (e: LearnEntry) => void;
  /** その手順を定石の枚で開く。 */
  onBook?: (kifu: string) => void;
}) {
  const [sel, setSel] = useState('');
  const cur = items.find((e) => keyOf(e) === sel) ?? items[0];
  const bad = cur ? blunderOf(cur) : undefined;

  /* 敗着の局面を出すために棋譜を 1 手ずつ開く。盤の規則は JS に写さず、
     バックエンドに再生させる (対局・検討・定石と同じ考え方)。 */
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
  // 前の対局の盤を出したままにしない (棋譜が変われば取り直すまで空)
  const shown = frames.text === text ? frames.f : [];
  const frame = shown.length
    ? shown[Math.min(bad?.ply ?? shown.length - 1, shown.length - 1)]
    : null;

  if (!items.length || !cur) {
    return (
      <Section title="取り込んだ対局">
        <Empty>まだありません。</Empty>
      </Section>
    );
  }

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      {/* 左 — 対局の一覧。設計 §8 は 269px の表 */}
      <div style={{
        width: 'var(--w-book-tree)', flex: 'none', minHeight: 0,
        borderRight: '1px solid var(--border)', display: 'flex', flexDirection: 'column',
      }}>
        <TableHead>
          <span style={{ flex: 1 }}>対局</span>
          <span style={{ width: 52, textAlign: 'right' }}>石数</span>
          <span style={{ width: 36, textAlign: 'right' }}>局面</span>
        </TableHead>
        <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: '0 var(--sp-3)' }}>
          <List>
            {items.map((e) => {
              const on = keyOf(e) === keyOf(cur);
              return (
                <TableRow key={keyOf(e)} on={on} pad="var(--sp-1)" fs="var(--fs-6)"
                          onClick={() => setSel(keyOf(e))}>
                  <span style={{
                    flex: 1, minWidth: 0, overflow: 'hidden',
                    textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                  }}>
                    <span style={{ color: 'var(--sub)' }}>{fmtWhen(e.at)}</span>
                    {' '}{e.opponent || 'KUROOBI'}
                  </span>
                  <span style={{ width: 52, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                    {e.black}–{e.white}
                  </span>
                  <span style={{
                    width: 36, textAlign: 'right', fontSize: 'var(--fs-7)',
                    color: 'var(--sub)', fontVariantNumeric: 'tabular-nums',
                  }}>{e.positions}</span>
                </TableRow>
              );
            })}
          </List>
        </div>
        {/* 絵は一覧の下に合計を置く。定石が何局面ぶん動いたかが一目で分かる */}
        <div style={{
          flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
          padding: 'var(--sp-2) var(--sp-3)', borderTop: '1px solid var(--border)',
          fontSize: 'var(--fs-6)', color: 'var(--sub)',
        }}>
          <span>{items.length} 局の合計</span>
          <span style={{
            marginLeft: 'auto', color: 'var(--text)', fontWeight: 600,
            fontVariantNumeric: 'tabular-nums',
          }}>{items.reduce((n, e) => n + e.positions, 0).toLocaleString()}</span>
          <span>局面</span>
        </div>
      </div>

      {/* 中央 — 敗着の局面。数字だけで「どの手で損したか」を読ませない */}
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
                {bad.ply} 手目 {bad.mv}
              </span>
              <span style={{ color: 'var(--bad)', fontVariantNumeric: 'tabular-nums' }}>
                ▼{lossOf(bad).toFixed(1)}
              </span>
              <span style={{ marginLeft: 'auto' }}>
                <Button onClick={() => onOpen(cur, bad.ply)}>検討で開く</Button>
              </span>
            </div>
            <span style={{
              maxWidth: 'var(--w-text)', fontSize: 'var(--fs-6)',
              color: 'var(--sub)', lineHeight: 1.8,
            }}>
              この局面の評価を終局の石差で上書きし、根まで書き戻しました。
              定石は「もっと良い手があった」と言っています
              ({sign(bad.best)} に対して {sign(bad.after)})。
            </span>
          </div>
        ) : (
          /* 同上 (規則 91) */
          <Empty>この対局に大きく損した手はありません。</Empty>
        )}
      </div>

      {/* 右 — この対局の明細。設計 §8 は 291px (ドックと同じ幅) */}
      <div className="k-scroll" style={{
        width: 'var(--w-dock)', flex: 'none', minHeight: 0,
        borderLeft: '1px solid var(--border)', padding: 'var(--sp-3) 0',
      }}>
        <Section title="この対局">
          <Fact label="相手" value={cur.opponent || 'KUROOBI'} />
          <Fact label="石数" value={`${cur.black} – ${cur.white}`} />
          <Fact label="取り込み" value={`${cur.positions} 局面 · ${fmtWhen(cur.at)}`} />
        </Section>

        <Section title="更新した定石" aside={<span>{cur.changes.length}</span>}>
          {!cur.changes.length && (
            <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
              明細の無い古い控えです。
            </span>
          )}
          <List>
            {cur.changes.map((c) => (
              <button key={c.ply} type="button" className="k-row"
                      onClick={() => onOpen(cur, c.ply)}
                      title="その手の局面を検討で開く"
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
                {/* 旧→新。定石に無かった手は「新規」 */}
                <span style={{ marginLeft: 'auto', fontVariantNumeric: 'tabular-nums' }}>
                  {c.before === null ? '新規' : sign(c.before)}
                  {' → '}
                  <b style={{ color: 'var(--text)' }}>{sign(c.after)}</b>
                </span>
              </button>
            ))}
          </List>
        </Section>

        <div style={{ display: 'flex', gap: 'var(--sp-2)', padding: '0 var(--sp-3)' }}>
          {onBook && <Button onClick={() => onBook(cur.kifu)}>定石で見る</Button>}
          {/* 取り消しは対局単位。1 手だけ戻すと、その局面から根までの値が
              その手を前提にしたままになる (書き戻しは連なっている)。
              **書き換えの明細が無い対局は押せなくする** — 押しても
              「戻せません」で返ってくるだけで、理由はすぐ上の
              「更新した定石」が空なことで見えている (規則 61) */}
          <Button variant="danger" disabled={!cur.changes.length}
                  onClick={() => onUndo(cur)}>取り消す</Button>
        </div>
      </div>
    </div>
  );
}

/** 「名前 / 値」の 1 行。値を右へ寄せて縦を揃える。 */
function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', fontSize: 'var(--fs-5)' }}>
      <span style={{ color: 'var(--sub)' }}>{label}</span>
      <span style={{
        marginLeft: 'auto', overflow: 'hidden',
        textOverflow: 'ellipsis', whiteSpace: 'nowrap',
      }}>{value}</span>
    </div>
  );
}

/** 取り込んだ時刻。今日のものは時刻だけ、それ以外は日付だけにする —
 *  並べたときに縦が揃い、かつ「さっき入ったもの」がすぐ分かる。 */
function fmtWhen(secs: number): string {
  const d = new Date(secs * 1000);
  const now = new Date();
  const sameDay = d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth()
    && d.getDate() === now.getDate();
  const p2 = (n: number) => String(n).padStart(2, '0');
  return sameDay ? `${p2(d.getHours())}:${p2(d.getMinutes())}`
    : `${d.getMonth() + 1}/${p2(d.getDate())}`;
}
