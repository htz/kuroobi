import React from 'react';

/* KUROOBI primitives
 * 色と寸法は必ずトークン経由。size は h-ctrl / h-field / h-chip の 3 段だけ。
 *
 * 押せる感じ（hover / active）は base.css の状態の層が持つ。
 * インライン style では :hover が書けないので、部品は k-press / k-row /
 * k-input を付ける。見た目そのものはインライン、変化だけがクラス。
 * className を受け取れるようにしてあるので、画面側で足すこともできる。
 */

type Size = 'chip' | 'ctrl' | 'field';
const H: Record<Size, string> = { chip: 'var(--h-chip)', ctrl: 'var(--h-ctrl)', field: 'var(--h-field)' };
const PAD: Record<Size, string> = { chip: '0 10px', ctrl: '0 12px', field: '0 14px' };
const FS: Record<Size, string> = { chip: 'var(--fs-6)', ctrl: 'var(--fs-5)', field: 'var(--fs-4)' };
const R: Record<Size, string> = { chip: 'var(--r-1)', ctrl: 'var(--r-2)', field: 'var(--r-3)' };

const cx = (...v: (string | false | undefined)[]) => v.filter(Boolean).join(' ');

export type ButtonProps = {
  variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
  size?: Size;
  disabled?: boolean;
  children?: React.ReactNode;
  onClick?: () => void;
  title?: string;
  className?: string;
};

export function Button({ variant = 'secondary', size = 'ctrl', disabled, children, onClick, title, className }: ButtonProps) {
  const skin: React.CSSProperties =
    variant === 'primary' ? { background: 'var(--accent-dim)', border: '1px solid var(--accent)', color: 'var(--on-accent)', fontWeight: 600 }
    : variant === 'danger' ? { background: 'transparent', border: '1px solid var(--bad)', color: 'var(--bad)' }
    : variant === 'ghost' ? { background: 'transparent', border: '1px solid var(--border)', color: 'var(--sub)' }
    : { background: 'var(--card)', border: '1px solid var(--border)', color: 'var(--text)' };
  return (
    <button
      type="button" onClick={onClick} disabled={disabled} title={title}
      // primary は既に濃いので hover を控えめにする（k-on）
      className={cx('k-press', variant === 'primary' && 'k-on', className)}
      style={{
        height: H[size], padding: PAD[size], borderRadius: R[size], fontSize: FS[size],
        display: 'inline-grid', placeItems: 'center', whiteSpace: 'nowrap',
        opacity: disabled ? 0.4 : 1, ...skin,
      }}
    >{children}</button>
  );
}

/* IconButton は Icons.tsx にある（絵だけのボタンは 32px 角の当たり・title と
 * aria-label 必須）。同名の部品を 2 か所に置くと import 元を間違えるので、
 * ここには置かない。任意の子を入れたいときも Icons 側を使う。 */

/* 2〜4 個の短い選択肢。5 個以上は Select にする。
 * 「数枚から 1 枚を選ぶ」列はこれ 1 つ。Dock のタブもこれを使う（fill）。
 * 選択中は --card で浮かせる — 塗り（--accent-dim）は左メニューの現在地に
 * 取ってあるので、青地を画面に 2 か所出すと「いまどこにいるか」が薄まる。 */
export function Segmented<T extends string>({ value, options, onChange, size = 'ctrl', fill, disabled, className }: {
  value: T;
  /** label は文字とは限らない — 担当の駒には石を添える (規則 59)。 */
  options: { value: T; label: React.ReactNode }[];
  onChange?: (v: T) => void;
  size?: Size;
  /** 器の幅いっぱいに等分する（Dock のタブ） */
  fill?: boolean;
  /** 選べない状態（定石ファイルが無い、など）。押せないことを見た目でも示す */
  disabled?: boolean;
  className?: string;
}) {
  return (
    <div role="radiogroup" aria-disabled={disabled || undefined} className={className} style={{
      height: H[size], display: fill ? 'flex' : 'inline-flex', gap: 2, padding: 2,
      background: 'var(--bg)', border: '1px solid var(--border)', borderRadius: 'var(--r-3)',
      opacity: disabled ? 0.4 : 1,
    }}>
      {options.map(o => {
        const on = value === o.value;
        return (
          <button key={o.value} type="button" role="radio" aria-checked={on} disabled={disabled}
            onClick={() => onChange?.(o.value)}
            className={cx('k-press', on && 'k-on')}
            style={{
              flex: fill ? 1 : 'none', padding: '0 12px', border: 0, borderRadius: 'var(--r-1)', fontSize: FS[size],
              background: on ? 'var(--card)' : 'transparent',
              color: on ? 'var(--text)' : 'var(--sub)', fontWeight: on ? 600 : 400,
              display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 6,
              whiteSpace: 'nowrap',
            }}
          >{o.label}</button>
        );
      })}
    </div>
  );
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange?: (v: boolean) => void; label?: string }) {
  return (
    <label style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', fontSize: 'var(--fs-5)' }}>
      {label && <span>{label}</span>}
      <button type="button" role="switch" aria-checked={checked} onClick={() => onChange?.(!checked)}
        className="k-press"
        style={{
          marginLeft: 'auto', flex: 'none', width: 34, height: 20, borderRadius: 'var(--r-pill)', border: 0,
          background: checked ? 'var(--accent)' : 'var(--border)', position: 'relative',
        }}>
        <span style={{
          position: 'absolute', top: 2, left: checked ? 16 : 2, width: 16, height: 16, borderRadius: '50%',
          background: checked ? 'var(--on-accent)' : 'var(--sub)', transition: 'left var(--dur) var(--ease)',
        }} />
      </button>
    </label>
  );
}

export function Slider({ value, min, max, onChange }: { value: number; min: number; max: number; onChange?: (v: number) => void }) {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <div style={{ padding: '6px var(--sp-2) 0' }}>
      <div style={{ height: 4, borderRadius: 3, background: 'var(--track)', position: 'relative' }}>
        <span style={{ position: 'absolute', inset: '0 auto 0 0', width: pct + '%', borderRadius: 3, background: 'var(--accent)' }} />
        <input type="range" min={min} max={max} value={value} onChange={e => onChange?.(+e.target.value)}
          style={{ position: 'absolute', inset: '-8px 0', width: '100%', opacity: 0, cursor: 'default' }} />
        <span style={{
          position: 'absolute', left: pct + '%', top: -6, width: 16, height: 16, borderRadius: '50%',
          background: 'var(--text)', transform: 'translateX(-8px)', boxShadow: 'var(--sh-1)', pointerEvents: 'none',
        }} />
      </div>
    </div>
  );
}

/* 5 個以上の選択肢。ネイティブの <select> を透明にして重ね、見た目だけ自前で描く
 * （macOS のポップアップの見た目はテーマに合わないが、開いたときの挙動は
 *  ネイティブが正しい ＝ キーボード・スクロール・画面端の折り返し）。
 *
 * ⚠ 外側に position:relative、重ねる select に inset:0 が必須。
 * relative が無いと absolute の基準が AppFrame になって select が画面の隅へ飛び、
 * width/height 0 だと飛んだ先でも当たりが無い（＝選べない）。 */
export function Select({ value, options, onChange, size = 'field', width, disabled, className }: {
  value: string;
  options: [string, string][];        // [値, 表示]
  onChange?: (v: string) => void;
  size?: Size;
  width?: number;                     // 最小幅。揃えたい列で使う
  disabled?: boolean;
  className?: string;
}) {
  const label = options.find(([v]) => v === value)?.[1] ?? value;
  return (
    <span className={cx('k-press', 'k-input', className)} style={{
      position: 'relative',            /* ← 外さない */
      height: H[size], minWidth: width, padding: PAD[size], borderRadius: R[size],
      background: 'var(--bg)', border: '1px solid var(--border)',
      display: 'inline-flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: FS[size],
      opacity: disabled ? 0.4 : 1,
    }}>
      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{label}</span>
      <span style={{ marginLeft: 'auto', color: 'var(--sub)', flex: 'none' }}>▾</span>
      <select value={value} disabled={disabled} onChange={e => onChange?.(e.target.value)}
        style={{ position: 'absolute', inset: 0, opacity: 0, width: '100%', height: '100%' }}>
        {options.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
      </select>
    </span>
  );
}

/* 打ち込む欄。onChange を渡さなければ読むだけになる（readOnly が付く）ので、
 * 表示専用の使い方も同じ部品でできる。 */
export function TextField({ value, onChange, mono, invalid, placeholder, readOnly, numeric, password, align, width, className, title }: {
  value?: string;
  onChange?: (v: string) => void;
  mono?: boolean;
  invalid?: boolean;
  placeholder?: string;
  readOnly?: boolean;
  numeric?: boolean;                  // 数字だけ通す（条件式の値・秒数）
  password?: boolean;                 // 伏せ字（GGS のログインだけ）
  align?: 'left' | 'right';
  width?: number;
  className?: string;
  /** 触れる範囲など、常時は出さない補足。 */
  title?: string;
}) {
  const ro = readOnly ?? !onChange;
  return (
    <input
      value={value} placeholder={placeholder} readOnly={ro} title={title}
      type={password ? 'password' : undefined}
      inputMode={numeric ? 'numeric' : undefined}
      onChange={ro ? undefined : e => onChange?.(numeric ? e.target.value.replace(/[^\d-]/g, '') : e.target.value)}
      className={cx('k-input', className)}
      style={{
        flex: width ? 'none' : 1, width, minWidth: 0,
        height: 'var(--h-field)', padding: '0 var(--sp-3)', borderRadius: 'var(--r-3)',
        background: 'var(--bg)', border: '1px solid ' + (invalid ? 'var(--bad)' : 'var(--border)'),
        fontFamily: mono ? 'var(--ff-mono)' : 'var(--ff)', fontSize: 'var(--fs-5)',
        textAlign: align ?? 'left',
      }} />
  );
}

export function Badge({ tone = 'sub', children }: { tone?: 'sub' | 'accent' | 'ok' | 'bad' | 'gold'; children: React.ReactNode }) {
  const c = 'var(--' + (tone === 'sub' ? 'sub' : tone) + ')';
  return (
    <span style={{
      padding: '0 5px', borderRadius: 'var(--r-pill)', fontSize: 'var(--fs-7)',
      background: tone === 'bad' ? 'var(--bad)' : 'transparent',
      color: tone === 'bad' ? 'var(--on-bad)' : c,
      boxShadow: tone === 'bad' ? 'none' : 'inset 0 0 0 1px ' + c,
    }}>{children}</span>
  );
}

/* 進行中を示す唯一の部品。％が分かるときだけ使い、分からないときは Dot で足りる */
export function Progress({ value }: { value: number }) {
  return (
    <div style={{ height: 4, borderRadius: 3, background: 'var(--track)', overflow: 'hidden' }}>
      <span style={{ display: 'block', width: Math.round(value * 100) + '%', height: '100%', background: 'var(--accent)' }} />
    </div>
  );
}

export function Dot({ tone = 'accent' }: { tone?: 'accent' | 'ok' | 'bad' | 'sub' | 'gold' }) {
  return <span style={{ width: 6, height: 6, borderRadius: '50%', flex: 'none', background: 'var(--' + tone + ')' }} />;
}
