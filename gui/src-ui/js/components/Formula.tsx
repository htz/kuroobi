// 対局の申し込みを自動で受ける / 断る条件を組む。
//
// GGS の条件式は入れ子の論理式で、**構造がそのまま意味**になっている
// (`!saved & (size!=8 | anti | …) | !rated`)。文字で打たせると括弧の対応を
// 目で追う羽目になるので、木のまま組ませる。保存するときだけ文字列に戻す。
//
// 逃げ道として生の式も出す。GGS には画面に載せていない変数もあるし、他所で
// 組んだ式をそのまま貼りたいこともある。
import { useState } from 'react';
import { FORMULA_OPS, FORMULA_VARS, condToSrc, parseCond, varOf } from '../ggs';
import type { Cond, FormulaOp } from '../ggs';
import { IconButton } from './Icons';

const COLORS: [string, string][] = [['?', 'おまかせ'], ['b', '黒'], ['w', '白']];

/** 新しい条件の既定。最初の変数を素で置く。 */
const newAtom = (): Cond => ({ kind: 'atom', name: 'rated', op: '=', val: '', neg: false });
const newGroup = (): Cond => ({ kind: 'all', kids: [newAtom()] });

/// 木の n 番目を差し替える / 取り除く。React の state は作り替えて渡す。
function replaceKid(node: Cond, i: number, next: Cond | null): Cond {
  if (node.kind === 'atom') return node;
  const kids = node.kids.slice();
  if (next) kids[i] = next;
  else kids.splice(i, 1);
  return { ...node, kids };
}

function AtomRow({ cond, onChange }: { cond: Cond; onChange: (c: Cond) => void }) {
  if (cond.kind !== 'atom') return null;
  const v = varOf(cond.name);
  const set = (p: Partial<Extract<Cond, { kind: 'atom' }>>) => onChange({ ...cond, ...p });
  return (
    <>
      <div className="selwrap cond-var">
        <select value={cond.name} onChange={(e) => {
          const nv = varOf(e.target.value);
          // 型が変わると比較や値の意味が変わる。既定へ戻す
          set({
            name: e.target.value,
            val: nv?.type === 'num' ? String(nv.def ?? 0) : nv?.type === 'color' ? '?' : '',
            op: '=',
            neg: false,
          });
        }}>
          {FORMULA_VARS.map((x) => <option key={x.name} value={x.name}>{x.label}</option>)}
        </select>
      </div>

      {/* 真偽はそのものか否定か。比較の枠を出しても選ぶものが無い */}
      {(!v || v.type === 'bool') && (
        <div className="selwrap cond-op">
          <select value={cond.neg ? 'no' : 'yes'}
                  onChange={(e) => set({ neg: e.target.value === 'no' })}>
            <option value="yes">である</option>
            <option value="no">ではない</option>
          </select>
        </div>
      )}

      {v?.type === 'num' && (
        <>
          <div className="selwrap cond-op">
            <select value={cond.op} onChange={(e) => set({ op: e.target.value as FormulaOp })}>
              {FORMULA_OPS.map((o) => <option key={o} value={o}>{o}</option>)}
            </select>
          </div>
          <input type="number" className="cond-num" value={cond.val}
                 onChange={(e) => set({ val: e.target.value })} />
          {v.unit && <span className="cond-unit">{v.unit}</span>}
        </>
      )}

      {v?.type === 'color' && (
        <>
          <div className="selwrap cond-op">
            <select value={cond.op} onChange={(e) => set({ op: e.target.value as FormulaOp })}>
              <option value="=">である</option>
              <option value="≠">ではない</option>
            </select>
          </div>
          <div className="selwrap cond-val">
            <select value={cond.val || '?'} onChange={(e) => set({ val: e.target.value })}>
              {COLORS.map(([c, l]) => <option key={c} value={c}>{l}</option>)}
            </select>
          </div>
        </>
      )}
    </>
  );
}

/// 束 1 つ。中に条件と、さらに束を置ける。
function Group({ cond, onChange, onRemove, top }: {
  cond: Cond; onChange: (c: Cond) => void; onRemove?: () => void; top?: boolean;
}) {
  if (cond.kind === 'atom') return null;
  return (
    <div className={'cond-group' + (top ? ' top' : '')}>
      <div className="cond-head">
        <div className="selwrap cond-join">
          <select value={cond.kind}
                  onChange={(e) => onChange({ ...cond, kind: e.target.value as 'all' | 'any' })}>
            <option value="all">すべて満たす</option>
            <option value="any">次のどれか</option>
          </select>
        </div>
        <span className="spacer" />
        {onRemove && <IconButton name="close" label="この束を取り除く" size={13} onClick={onRemove} />}
      </div>

      <div className="cond-kids">
        {cond.kids.map((k, i) => (
          <div key={i} className={'cond-row' + (k.kind === 'atom' ? '' : ' nest')}>
            {k.kind === 'atom' ? (
              <>
                <AtomRow cond={k} onChange={(c) => onChange(replaceKid(cond, i, c))} />
                <span className="spacer" />
                <IconButton name="close" label="この条件を取り除く" size={13}
                            onClick={() => onChange(replaceKid(cond, i, null))} />
              </>
            ) : (
              <Group cond={k}
                     onChange={(c) => onChange(replaceKid(cond, i, c))}
                     onRemove={() => onChange(replaceKid(cond, i, null))} />
            )}
          </div>
        ))}
        <div className="cond-add">
          <button className="btn small"
                  onClick={() => onChange({ ...cond, kids: [...cond.kids, newAtom()] })}>
            + 条件
          </button>
          <button className="btn small"
                  onClick={() => onChange({ ...cond, kids: [...cond.kids, newGroup()] })}>
            + 束
          </button>
        </div>
      </div>
    </div>
  );
}

export interface FormulaEditorProps {
  /** サーバーに入っている式 (GGS の記法)。空なら「指定なし」。 */
  value: string;
  /** 保存する。空文字は条件の解除。 */
  onSave: (src: string) => void;
}

export function FormulaEditor({ value, onSave }: FormulaEditorProps) {
  // 開いたときのサーバーの値から始める。以後は手元の木が正
  const [cond, setCond] = useState<Cond | null>(() => parseCond(value));
  const [raw, setRaw] = useState(false);
  const [text, setText] = useState(value);

  const src = cond ? condToSrc(cond) : '';

  if (raw) {
    return (
      <div className="cond-editor">
        <p className="hint">
          GGS の記法をそのまま書きます。<code>&amp;</code> が「かつ」、
          <code>|</code> が「または」、<code>!</code> が否定。
        </p>
        <textarea className="cond-raw" value={text} rows={3}
                  onChange={(e) => setText(e.target.value)} />
        <div className="row actions">
          <button className="btn" onClick={() => { setCond(parseCond(text)); setRaw(false); }}>
            組み立てに戻る
          </button>
          <span className="spacer" />
          <button className="btn primary fix" onClick={() => onSave(text.trim())}>反映する</button>
        </div>
      </div>
    );
  }

  return (
    <div className="cond-editor">
      {cond ? (
        <Group cond={cond.kind === 'atom' ? { kind: 'all', kids: [cond] } : cond}
               onChange={setCond} top />
      ) : (
        <p className="hint">条件を付けていません (申し込みごとに自分で判断します)。</p>
      )}

      {/* 何がサーバーへ行くのかを隠さない。GGS の記法を知っている人には
          これが一番早い確認になる */}
      {src && <div className="cond-src"><span className="k">送る式</span><code>{src}</code></div>}

      <div className="row actions">
        {!cond && (
          <button className="btn" onClick={() => setCond(newGroup())}>条件を付ける</button>
        )}
        {cond && (
          <button className="btn" onClick={() => setCond(null)}>条件を外す</button>
        )}
        <button className="btn ghost" onClick={() => { setText(src); setRaw(true); }}>
          式を直接書く
        </button>
        <span className="spacer" />
        <button className="btn primary fix" onClick={() => onSave(src)}>反映する</button>
      </div>
    </div>
  );
}
