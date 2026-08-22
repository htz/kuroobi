import React from 'react';

/* KUROOBI primitives
 * Colors and dimensions always go through tokens; sizes come from the
 * design's ladder (Size below).
 *
 * Hover/active states live in base.css (inline styles cannot express
 * :hover), so components attach k-press / k-row / k-input: looks are
 * inline, transitions are classes. className passes through.
 */

/* Sizes from the 44/32/28/24/20 ladder. `row` (24) exists only for
   the move-strip stepper buttons — the strip is 32px, and 28 would
   leave 2px of breathing room. */
type Size = 'chip' | 'row' | 'ctrl' | 'field';
const H: Record<Size, string> = { chip: 'var(--h-chip)', row: 'var(--h-row)', ctrl: 'var(--h-ctrl)', field: 'var(--h-field)' };
const PAD: Record<Size, string> = { chip: '0 10px', row: '0 10px', ctrl: '0 12px', field: '0 14px' };
const FS: Record<Size, string> = { chip: 'var(--fs-6)', row: 'var(--fs-7)', ctrl: 'var(--fs-5)', field: 'var(--fs-4)' };
const R: Record<Size, string> = { chip: 'var(--r-1)', row: 'var(--r-1)', ctrl: 'var(--r-2)', field: 'var(--r-3)' };

const cx = (...v: (string | false | undefined)[]) => v.filter(Boolean).join(' ');

export type ButtonProps = {
  variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
  size?: Size;
  disabled?: boolean;
  children?: React.ReactNode;
  onClick?: () => void;
  title?: string;
  className?: string;
  /** Square variant for single-glyph stepper buttons (record viewer);
   *  pictorial buttons use Icons' IconButton instead. */
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
      // primary is already saturated; keep hover subtle (k-on).
      className={cx('k-press', variant === 'primary' && 'k-on', className)}
      style={{
        // Primary gets 14px side padding (per the design): the call to
        // action differs in width, not just color.
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

/* IconButton lives in Icons.tsx (32px hit target, title and
 * aria-label required). Not duplicated here — a same-named component
 * in two places invites wrong imports. */

/* 2-4 short options; five or more become a Select. The dock tabs use
 * this too (fill). Selection lifts with --card, not the accent fill —
 * that is reserved for the nav's current location, and two blue
 * patches would dilute "where am I". */
export function Segmented<T extends string>({ value, options, onChange, size = 'ctrl', fill, disabled, solid, className }: {
  value: T;
  /** Labels need not be text — the side picker adds stone dots. */
  options: { value: T; label: React.ReactNode }[];
  onChange?: (v: T) => void;
  size?: Size;
  /** Fill the container evenly (dock tabs). */
  fill?: boolean;
  /** Disabled (e.g. no book file); visibly unpressable. */
  disabled?: boolean;
  /** Fill-style selection; only for windows without the nav (settings)
   *  — on the main screen it would double the location accent. */
  solid?: boolean;
  className?: string;
}) {
  return (
    /* size is the CONTAINER height. It was once changed to the chip
       height based on a stale design capture and reverted; the current
       design (28/22/2, radii 7/5) matches the implementation.
       Re-capture before measuring. */
    <div role="radiogroup" aria-disabled={disabled || undefined} className={cx('k-seg', className)} style={{
      // Outer radius --r-2 (7px) vs inner --r-1 (5px); 8px would open
      // a 3px gap and thicken the rim.
      height: H[size], display: fill ? 'flex' : 'inline-flex', gap: 2, padding: 2,
      background: 'var(--bg)', border: '1px solid var(--border)', borderRadius: 'var(--r-2)',
      opacity: disabled ? 0.4 : 1,
    }}>
      {options.map(o => {
        const on = value === o.value;
        return (
          <button key={o.value} type="button" role="radio" aria-checked={on} disabled={disabled}
            onClick={() => onChange?.(o.value)}
            className={cx('k-press', on && 'k-on', on && !solid && 'k-seg-on')}
            style={{
              flex: fill ? 1 : 'none', padding: '0 12px', borderRadius: 'var(--r-1)', fontSize: FS[size],
              // In light mode --card vs --bg barely differ, so the
              // selected chip gets a 1px inner keyline via base.css
              // (light only, .k-seg-on); unselected chips carry an
              // equal-width transparent border so text never shifts.
              border: 0,
              background: on ? (solid ? 'var(--accent-dim)' : 'var(--card)') : 'transparent',
              color: on ? (solid ? 'var(--on-accent)' : 'var(--text)') : 'var(--sub)',
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

/* Slider was removed (2026-08-08): once custom strength became three
 * Selects, nothing used it. Catalog-only components are not kept. */

/* Five-plus options: a transparent native <select> overlays a custom
 * face (native popup behavior — keyboard, scrolling, edge wrapping —
 * is correct even if its look is not).
 *
 * The wrapper MUST be position:relative with the select at inset:0;
 * without relative the select flies to the AppFrame corner, and with
 * zero size it has no hit area there either. */
export function Select({ value, options, onChange, size = 'field', width, disabled, className }: {
  value: string;
  options: [string, string][];        // [value, label]
  onChange?: (v: string) => void;
  size?: Size;
  width?: number;                     // min width, for aligned columns
  disabled?: boolean;
  className?: string;
}) {
  const label = options.find(([v]) => v === value)?.[1] ?? value;
  return (
    <span className={cx('k-press', 'k-input', className)} style={{
      position: 'relative',            /* never drop this */
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

/* Text input; omitting onChange makes it readOnly, so display-only
 * uses share the component. */
export function TextField({ value, onChange, mono, invalid, placeholder, readOnly, numeric, password, align, width, className, title, onEnter }: {
  value?: string;
  onChange?: (v: string) => void;
  mono?: boolean;
  invalid?: boolean;
  placeholder?: string;
  readOnly?: boolean;
  numeric?: boolean;                  // digits only (formula values, seconds)
  password?: boolean;                 // masked (GGS login only)
  align?: 'left' | 'right';
  width?: number;
  className?: string;
  /** Occasional hints (ranges etc.), not always shown. */
  title?: string;
  /** Enter-to-submit fields (chat, console); mirrors the send button. */
  onEnter?: () => void;
}) {
  const ro = readOnly ?? !onChange;
  return (
    <input
      value={value} placeholder={placeholder} readOnly={ro} title={title}
      onKeyDown={onEnter && ((e) => { if (e.key === 'Enter' && !e.nativeEvent.isComposing) { e.preventDefault(); onEnter(); } })}
      type={password ? 'password' : undefined}
      inputMode={numeric ? 'numeric' : undefined}
      onChange={ro ? undefined : e => onChange?.(numeric ? e.target.value.replace(/[^\d-]/g, '') : e.target.value)}
      className={cx('k-input', className)}
      style={{
        /* Never `flex: 1`: the shorthand sets flex-basis: 0% and
           crushes the height inside column layouts (32px fields
           rendered at 20px). Only the width should grow. */
        flexGrow: width ? 0 : 1, flexShrink: 1, flexBasis: 'auto', width, minWidth: 0,
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

/* The sole progress component; only for known percentages (unknown
 * progress is just a Dot). */
export function Progress({ value }: { value: number }) {
  return (
    <div style={{ height: 4, borderRadius: 'var(--r-0)', background: 'var(--track)', overflow: 'hidden' }}>
      <span style={{ display: 'block', width: Math.round(value * 100) + '%', height: '100%', background: 'var(--accent)' }} />
    </div>
  );
}

export function Dot({ tone = 'accent' }: { tone?: 'accent' | 'ok' | 'bad' | 'sub' | 'gold' }) {
  return <span style={{ width: 6, height: 6, borderRadius: '50%', flex: 'none', background: 'var(--' + tone + ')' }} />;
}
