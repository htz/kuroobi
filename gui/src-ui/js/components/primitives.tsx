import React from 'react';


type Size = 'chip' | 'row' | 'ctrl' | 'field';
const H: Record<Size, string> = { chip: 'var(--h-chip)', row: 'var(--h-row)', ctrl: 'var(--h-ctrl)', field: 'var(--h-field)' };
const PAD: Record<Size, string> = { chip: '0 10px', row: '0 10px', ctrl: '0 12px', field: '0 14px' };
const FS: Record<Size, string> = { chip: 'var(--fs-6)', row: 'var(--fs-7)', ctrl: 'var(--fs-5)', field: 'var(--fs-4)' };
const R: Record<Size, string> = { chip: 'var(--r-1)', row: 'var(--r-1)', ctrl: 'var(--r-2)', field: 'var(--r-3)' };

const cx = (...v: (string | false | undefined)[]) => v.filter(Boolean).join(' ');

type ButtonProps = {
  variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
  size?: Size;
  disabled?: boolean;
  children?: React.ReactNode;
  onClick?: () => void;
  title?: string;
  className?: string;
  square?: boolean;
};

export function Button({ variant = 'secondary', size = 'ctrl', disabled, children, onClick, title, className, square }: ButtonProps) {
  const skin: React.CSSProperties =
    variant === 'primary' ? { background: 'var(--accent-dim)', border: '1px solid var(--accent)', color: 'var(--on-accent)', fontWeight: 600 }
    : variant === 'danger' ? { background: 'transparent', border: '1px solid var(--bad)', color: 'var(--bad)' }
    : variant === 'ghost' ? { background: 'transparent', border: '1px solid var(--border)', color: 'var(--sub)' }
    : { background: 'var(--card)', border: '1px solid var(--border)', color: 'var(--text)' };
  return (
    <button
      type="button" onClick={onClick} disabled={disabled} title={title}
      className={cx('k-press', variant === 'primary' && 'k-on', className)}
      style={{
        height: H[size],
        width: square ? H[size] : undefined, flex: square ? 'none' : undefined,
        padding: square ? 0 : variant === 'primary' && size === 'ctrl' ? '0 14px' : PAD[size],
        borderRadius: R[size], fontSize: FS[size],
        display: 'inline-grid', placeItems: 'center', whiteSpace: 'nowrap',
        opacity: disabled ? 0.4 : 1, ...skin,
      }}
    >{children}</button>
  );
}


export function Segmented<T extends string>({ value, options, onChange, fill, disabled }: {
  value: T;
  options: { value: T; label: React.ReactNode }[];
  onChange?: (v: T) => void;
  fill?: boolean;
  disabled?: boolean;
}) {
  return (
    <div role="radiogroup" aria-disabled={disabled || undefined} className="k-seg" style={{
      height: H.ctrl, display: fill ? 'flex' : 'inline-flex', gap: 2, padding: 2,
      background: 'var(--bg)', border: '1px solid var(--border)', borderRadius: 'var(--r-2)',
      opacity: disabled ? 0.4 : 1,
    }}>
      {options.map(o => {
        const on = value === o.value;
        return (
          <button key={o.value} type="button" role="radio" aria-checked={on} disabled={disabled}
            onClick={() => onChange?.(o.value)}
            className={cx('k-press', on && 'k-on', on && 'k-seg-on')}
            style={{
              flex: fill ? 1 : 'none', padding: '0 12px', borderRadius: 'var(--r-1)', fontSize: FS.ctrl,
              border: 0,
              background: on ? 'var(--card)' : 'transparent',
              color: on ? 'var(--text)' : 'var(--sub)',
              fontWeight: on ? 600 : 400,
              display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 'var(--sp-0)',
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


export function Select({ value, options, onChange, size = 'field', width, title }: {
  value: string;
  options: [string, string][];        // [value, label]
  onChange?: (v: string) => void;
  size?: Size;
  width?: number;                     // min width, for aligned columns
  title?: string;
}) {
  const label = options.find(([v]) => v === value)?.[1] ?? value;
  return (
    <span className="k-press k-input" style={{
      position: 'relative',            // anchors the invisible <select> laid over it
      height: H[size], minWidth: width, maxWidth: '100%',
      padding: PAD[size], borderRadius: R[size],
      background: 'var(--bg)', border: '1px solid var(--border)',
      display: 'inline-flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: FS[size],
      opacity: 1,
    }}>
      <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis',
                     whiteSpace: 'nowrap' }}>{label}</span>
      <span style={{ marginLeft: 'auto', color: 'var(--sub)', flex: 'none' }}>▾</span>
      <select value={value} title={title} aria-label={title}
        onChange={e => onChange?.(e.target.value)}
        style={{ position: 'absolute', inset: 0, opacity: 0, width: '100%', height: '100%' }}>
        {options.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
      </select>
    </span>
  );
}

export function TextArea({ value, onChange, mono, placeholder, rows }: {
  value: string;
  onChange: (v: string) => void;
  mono?: boolean;
  placeholder?: string;
  rows: number;
}) {
  return (
    <textarea
      value={value} placeholder={placeholder} rows={rows}
      spellCheck={false} autoCapitalize="off" autoCorrect="off"
      onChange={(e) => onChange(e.target.value)}
      className="k-input"
      style={{
        width: '100%', minWidth: 0, resize: 'vertical',
        padding: 'var(--sp-3)', borderRadius: 'var(--r-3)',
        background: 'var(--bg)', border: '1px solid var(--border)',
        fontFamily: mono ? 'var(--ff-mono)' : 'var(--ff)', fontSize: 'var(--fs-6)',
        lineHeight: 1.6,
      }} />
  );
}

export function TextField({ value, onChange, mono, invalid, placeholder, numeric, password, align, width, onEnter }: {
  value?: string;
  onChange?: (v: string) => void;
  mono?: boolean;
  invalid?: boolean;
  placeholder?: string;
  numeric?: boolean;
  password?: boolean;
  align?: 'left' | 'right';
  width?: number;
  onEnter?: () => void;
}) {
  const ro = !onChange;
  return (
    <input
      value={value} placeholder={placeholder} readOnly={ro}
      onKeyDown={onEnter && ((e) => { if (e.key === 'Enter' && !e.nativeEvent.isComposing) { e.preventDefault(); onEnter(); } })}
      type={password ? 'password' : undefined}
      inputMode={numeric ? 'numeric' : undefined}
      onChange={ro ? undefined : e => onChange?.(numeric ? e.target.value.replace(/[^\d-]/g, '') : e.target.value)}
      className="k-input"
      style={{
        flexGrow: width ? 0 : 1, flexShrink: 1, flexBasis: 'auto', width, minWidth: 0,
        height: 'var(--h-field)', padding: '0 var(--sp-3)', borderRadius: 'var(--r-3)',
        background: 'var(--bg)', border: '1px solid ' + (invalid ? 'var(--bad)' : 'var(--border)'),
        fontFamily: mono ? 'var(--ff-mono)' : 'var(--ff)', fontSize: 'var(--fs-5)',
        textAlign: align ?? 'left',
      }} />
  );
}

export function Badge({ children }: { children: React.ReactNode }) {
  return (
    <span style={{
      padding: '0 5px', borderRadius: 'var(--r-pill)', fontSize: 'var(--fs-7)',
      background: 'var(--bad)', color: 'var(--on-bad)', boxShadow: 'none',
    }}>{children}</span>
  );
}

export function Progress({ value }: { value: number }) {
  return (
    <div style={{ height: 4, borderRadius: 'var(--r-0)', background: 'var(--track)', overflow: 'hidden' }}>
      <span style={{ display: 'block', width: Math.round(value * 100) + '%', height: '100%', background: 'var(--accent)' }} />
    </div>
  );
}

export function Dot({ tone = 'accent' }: { tone?: 'accent' | 'ok' | 'sub' | 'gold' }) {
  return <span style={{ width: 6, height: 6, borderRadius: '50%', flex: 'none', background: 'var(--' + tone + ')' }} />;
}
