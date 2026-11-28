// The web design system's components (design-system/web/README.md): sized for a web page, never under
// 13px of text, 44px for anything pressed.
import React from 'react';
import { t, type Lang } from '../i18n';

const FONT = 'var(--ff-web)';
const CODE = 'var(--ff-code)';

const ICONS: Record<string, React.ReactNode> = {
  share: <><path d="M12 3v12" /><path d="M7.5 7.5 12 3l4.5 4.5" /><path d="M8 11H6a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-8a1 1 0 0 0-1-1h-2" /></>,
  link: <><path d="M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1" /><path d="M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1" /></>,
  copy: <><rect x="9" y="9" width="12" height="12" rx="2" /><path d="M5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1" /></>,
  x: <path fill="currentColor" stroke="none" transform="translate(2.4 2.4) scale(.8)" d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" />,
  more: <><circle cx="5" cy="12" r="1.2" /><circle cx="12" cy="12" r="1.2" /><circle cx="19" cy="12" r="1.2" /></>,
  open: <><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></>,
  chevron: <path d="m6 9 6 6 6-6" />,
  close: <><path d="M6 6l12 12" /><path d="M18 6 6 18" /></>,
  file: <><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" /></>,
  check: <path d="m5 12.5 4.5 4.5L19 7" />,
  first: <><path d="M6 5v14" /><path d="m18 6-6 6 6 6" /></>,
  prev: <path d="m15 6-6 6 6 6" />,
  next: <path d="m9 6 6 6-6 6" />,
  last: <><path d="M18 5v14" /><path d="m6 6 6 6-6 6" /></>,
  external: <><path d="M14 4h6v6" /><path d="M20 4l-9 9" /><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" /></>,
  view: <><path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" /><circle cx="12" cy="12" r="3" /></>,
  load: <><path d="M12 3v12" /><path d="M7.5 10.5 12 15l4.5-4.5" /><path d="M5 21h14" /></>,
};
export type IconName = keyof typeof ICONS;

/** A line icon in the text's colour, 18px beside a label. */
export function Icon({ name, size = 18 }: { name: IconName; size?: number }) {
  return (
    <svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" style={{ flex: 'none' }}>{ICONS[name]}</svg>
  );
}

export function Button({ kind = 'secondary', icon, disabled, onClick, children, style, ...rest }: {
  kind?: 'primary' | 'secondary';
  icon?: IconName;
  disabled?: boolean;
  onClick?: () => void;
  children: React.ReactNode;
  style?: React.CSSProperties;
} & Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'onClick' | 'children' | 'style'>) {
  const primary = kind === 'primary';
  // A button that cannot be pressed drops its ground and its text goes to `sub`, so it never reads as one that can.
  const ground = disabled ? 'transparent' : primary ? 'var(--accent-dim)' : 'transparent';
  const edge = disabled ? 'var(--border-weak)' : primary ? 'var(--accent-dim)' : 'var(--border)';
  return (
    <button type="button" {...rest} disabled={disabled} onClick={onClick}
      className={disabled ? undefined : primary ? 'w-btn w-btn-primary' : 'w-btn'}
      style={{
        display: 'inline-flex', alignItems: 'center', justifyContent: 'center', gap: 8,
        minHeight: 44, padding: '0 18px', boxSizing: 'border-box',
        borderRadius: 10, border: `1px solid ${edge}`, background: ground,
        color: disabled ? 'var(--sub)' : primary ? 'var(--on-accent)' : 'var(--text)',
        font: `${primary ? 700 : 500} var(--t-small) ${FONT}`, opacity: disabled ? 0.7 : 1,
        cursor: disabled ? 'default' : 'pointer', whiteSpace: 'nowrap', ...style,
      }}>
      {icon && <Icon name={icon} />}
      <span className="w-btn-label">{children}</span>
    </button>
  );
}

/**
 * An item with `checked` is a switch: it stays in the menu when pressed and shows a check while on. One with
 * `external` leaves the site in a new window: the arrow mark at its right, and those words for screen readers.
 */
export type MenuItem = { label: string; icon?: IconName; checked?: boolean; external?: string; onSelect?: () => void };

/** A button that opens a list of actions under it (Share, View); `open` holds it open, for the canvas. */
export function MenuButton({ label, icon, items, open: held }: {
  label: string; icon?: IconName; items: MenuItem[]; open?: boolean;
}) {
  const [openState, setOpen] = React.useState(false);
  const open = held || openState;
  const root = React.useRef<HTMLDivElement>(null);
  const list = React.useRef<HTMLDivElement>(null);
  React.useEffect(() => {
    if (!openState) return;
    list.current?.querySelector<HTMLButtonElement>('[role^=menuitem]')?.focus();
    const away = (e: PointerEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', away);
    return () => document.removeEventListener('pointerdown', away);
  }, [openState]);
  const keys = (e: React.KeyboardEvent) => {
    const all = [...(list.current?.querySelectorAll<HTMLButtonElement>('[role^=menuitem]') ?? [])];
    const i = all.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === 'Escape') { setOpen(false); root.current?.querySelector<HTMLButtonElement>('button')?.focus(); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); all[(i + 1) % all.length]?.focus(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); all[(i - 1 + all.length) % all.length]?.focus(); }
  };
  return (
    <div ref={root} onKeyDown={keys} style={{ position: 'relative', display: 'inline-flex' }}>
      <Button icon={icon} aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!openState)}>{label}</Button>
      {open && (
        <div ref={list} role="menu" aria-label={label} style={{
          position: 'absolute', top: 'calc(100% + 8px)', right: 0, zIndex: 20, minWidth: 240, padding: 6,
          display: 'flex', flexDirection: 'column', boxSizing: 'border-box', borderRadius: 12,
          border: '1px solid var(--border)', background: 'var(--card)', boxShadow: '0 12px 32px rgba(0, 0, 0, .35)',
        }}>
          {items.map((it) => (
            <button key={it.label} type="button" role={it.checked === undefined ? 'menuitem' : 'menuitemcheckbox'}
              aria-checked={it.checked} className="k-row"
              onClick={() => { if (it.checked === undefined) setOpen(false); it.onSelect?.(); }}
              style={{
                display: 'flex', alignItems: 'center', gap: 12, minHeight: 44, padding: '0 12px', border: 0,
                borderRadius: 8, background: 'transparent', color: 'var(--text)', textAlign: 'left',
                font: `var(--t-small) ${FONT}`, whiteSpace: 'nowrap',
              }}>
              {it.icon && <span style={{ display: 'inline-flex', color: 'var(--sub)' }}><Icon name={it.icon} /></span>}
              {it.label}
              {it.external && <>
                <span style={{ marginLeft: 'auto', paddingLeft: 16, display: 'inline-flex', color: 'var(--sub)' }}>
                  <Icon name="external" size={14} />
                </span>
                <span className="w-sr"> ({it.external})</span>
              </>}
              {it.checked !== undefined && (
                <span style={{ marginLeft: 'auto', paddingLeft: 16, display: 'inline-flex', color: 'var(--accent)', visibility: it.checked ? 'visible' : 'hidden' }}>
                  <Icon name="check" />
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * A modal over the page: Escape, the backdrop and the close button all call `onClose`; `actions` sit at the foot.
 * `inline` draws it in place, for the canvas.
 */
export function Dialog({ title, closeLabel, onClose, actions, inline, children }: {
  title: string; closeLabel: string; onClose: () => void; actions?: React.ReactNode; inline?: boolean;
  children: React.ReactNode;
}) {
  const box = React.useRef<HTMLDialogElement>(null);
  const id = React.useId();
  React.useEffect(() => {
    const d = box.current;
    if (!d || inline) return;
    const back = document.activeElement as HTMLElement | null;
    d.showModal();
    return () => { d.close(); back?.focus(); };
  }, [inline]);
  return (
    <dialog ref={box} open={inline || undefined} aria-labelledby={id} className="w-dialog"
      onCancel={(e) => { e.preventDefault(); onClose(); }}
      onClick={(e) => { if (e.target === box.current) onClose(); }}
      style={{
        position: inline ? 'relative' : undefined, width: inline ? '100%' : 'min(560px, calc(100vw - 32px))', maxWidth: 560, maxHeight: 'calc(100dvh - 32px)',
        margin: inline ? 0 : 'auto', padding: 0, boxSizing: 'border-box', borderRadius: 'var(--r-card)',
        border: '1px solid var(--border)', background: 'var(--panel)', color: 'var(--text)',
        boxShadow: '0 24px 64px rgba(0, 0, 0, .45)', font: `var(--t-small) ${FONT}`,
      }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 16, padding: 20 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <h2 id={id} style={{ margin: 0, font: `700 var(--t-body)/1.35 ${FONT}` }}>{title}</h2>
          <button type="button" aria-label={closeLabel} onClick={onClose} className="k-row" style={{
            marginLeft: 'auto', marginRight: -10, width: 44, height: 44, display: 'inline-flex', alignItems: 'center',
            justifyContent: 'center', border: 0, borderRadius: 10, background: 'transparent', color: 'var(--sub)',
          }}><Icon name="close" size={20} /></button>
        </div>
        {children}
        {actions && <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 8 }}>{actions}</div>}
      </div>
    </dialog>
  );
}

/** A short note that something was done (copied), floating at the foot of the screen; the caller removes it. */
export function Toast({ children }: { children: React.ReactNode }) {
  // Its look is the `.w-toast` class (site.css), which the articles' copy button uses without React.
  return (
    <div role="status" className="w-toast">
      <span className="w-toast-icon"><Icon name="check" /></span>
      {children}
    </div>
  );
}

/**
 * A card over the board, the board dimmed behind it: what has to be settled before the board is played
 * (a game's colour and strength). It sits in the board's frame, so it is in view wherever the board is.
 */
export function BoardCover({ title, children }: { title: string; children: React.ReactNode }) {
  const id = React.useId();
  return (
    <div style={{
      position: 'absolute', inset: 0, zIndex: 2, display: 'grid', placeItems: 'center', borderRadius: 14,
      background: 'color-mix(in srgb, var(--bg) 55%, transparent)',
    }}>
      <section aria-labelledby={id} style={{
        width: 'min(380px, calc(100% - 24px))', maxHeight: 'calc(100% - 24px)', overflowY: 'auto', boxSizing: 'border-box',
        display: 'flex', flexDirection: 'column', gap: 16, padding: 20, borderRadius: 'var(--r-card)',
        border: '1px solid var(--border)', background: 'var(--panel)', boxShadow: '0 16px 40px rgba(0, 0, 0, .35)',
      }}>
        <h2 id={id} style={{ margin: 0, font: `700 var(--t-body)/1.35 ${FONT}` }}>{title}</h2>
        {children}
      </section>
    </div>
  );
}

/** Moving through a record from under the board: first, back, where it is, forward, last; 44px each. */
export function StepNav({ lang, at, total, onJump }: { lang: Lang; at: number; total: number; onJump: (n: number) => void }) {
  const S = (key: string) => t(lang, 'step.' + key);
  const step = (icon: IconName, label: string, to: number, off: boolean) => (
    <button type="button" aria-label={label} disabled={off} onClick={() => onJump(to)} className={off ? undefined : 'w-btn'}
      style={{
        width: 44, height: 44, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', borderRadius: 10,
        border: `1px solid ${off ? 'var(--border-weak)' : 'var(--border)'}`, background: 'transparent',
        color: off ? 'var(--sub)' : 'var(--text)', opacity: off ? 0.7 : 1, cursor: off ? 'default' : 'pointer',
      }}><Icon name={icon} size={20} /></button>
  );
  return (
    <nav aria-label={S('label')} style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
      {step('first', S('first'), 0, at === 0)}
      {step('prev', S('prev'), at - 1, at === 0)}
      <span aria-live="polite" style={{
        flex: 1, textAlign: 'center', font: `var(--t-small) ${CODE}`, fontVariantNumeric: 'tabular-nums', color: 'var(--sub)',
      }}><b style={{ color: 'var(--text)', fontWeight: 700 }}>{at}</b> / {total}</span>
      {step('next', S('next'), at + 1, at >= total)}
      {step('last', S('last'), total, at >= total)}
    </nav>
  );
}

/** A small label on a value, such as the format a record was read as. */
export function Tag({ children }: { children: React.ReactNode }) {
  return (
    <span style={{
      display: 'inline-flex', alignItems: 'center', minHeight: 24, padding: '0 10px', borderRadius: 'var(--r-pill)',
      border: '1px solid var(--border)', background: 'var(--card)', font: `500 var(--t-meta) ${FONT}`, whiteSpace: 'nowrap',
    }}>{children}</span>
  );
}

export function Segmented({ label, items }: {
  label: string;
  items: { label: string; href: string; on: boolean }[];
}) {
  return (
    <nav aria-label={label} style={{
      display: 'inline-flex', padding: 3, borderRadius: 10, background: 'var(--panel)', border: '1px solid var(--border)',
    }}>
      {items.map((it) => (
        <a key={it.label} href={it.href} aria-current={it.on ? 'page' : undefined} className="w-plain" style={{
          display: 'inline-flex', alignItems: 'center', minHeight: 44, padding: '0 18px', boxSizing: 'border-box',
          borderRadius: 7, font: `500 var(--t-small) ${FONT}`,
          background: it.on ? 'var(--card)' : 'transparent', color: it.on ? 'var(--text)' : 'var(--sub)',
        }}>{it.label}</a>
      ))}
    </nav>
  );
}

export function RadioCards<T extends string | number>({ legend, name, value, options, onChange, disabled }: {
  legend: string;
  name: string;
  value: T;
  options: { value: T; label: string; stone?: 'b' | 'w'; hint?: string }[];
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
  // Disabled, the chosen card keeps a plain edge and the others go to `border-weak`, so the choice still reads.
  const edge = (on: boolean) => (disabled ? (on ? 'var(--border)' : 'var(--border-weak)') : on ? 'var(--accent)' : 'var(--border)');
  return (
    <fieldset disabled={disabled} style={{ margin: 0, padding: 0, border: 0, opacity: disabled ? 0.7 : 1, display: 'flex', flexDirection: 'column', gap: 8 }}>
      <legend style={{ padding: 0, marginBottom: 8, font: `var(--t-meta) ${FONT}`, color: 'var(--sub)' }}>{legend}</legend>
      <div style={{ display: 'grid', gridTemplateColumns: `repeat(${options.length}, minmax(0, 1fr))`, gap: 8 }}>
        {options.map((o) => (
          <label key={String(o.value)} className="w-choice" style={{
            display: 'flex', alignItems: 'center', gap: 10, minHeight: 44, padding: '0 14px', boxSizing: 'border-box',
            borderRadius: 10, border: `1px solid ${edge(o.value === value)}`,
            boxShadow: o.value === value && !disabled ? 'inset 0 0 0 1px var(--accent)' : 'none',
            background: o.value === value && !disabled ? 'var(--card)' : 'transparent',
            font: `var(--t-small) ${FONT}`, color: disabled ? 'var(--sub)' : 'var(--text)', cursor: disabled ? 'default' : 'pointer',
          }}>
            <input type="radio" name={name} checked={o.value === value} onChange={() => onChange(o.value)}
              className={o.stone ? 'w-sr' : undefined} style={{ accentColor: 'var(--accent)', margin: 0 }} />
            {o.stone && <StoneDot color={o.stone} />}
            {o.label}
            {o.hint && <span style={{ marginLeft: 'auto', font: `var(--t-meta) ${FONT}`, color: 'var(--sub)', whiteSpace: 'nowrap' }}>{o.hint}</span>}
          </label>
        ))}
      </div>
    </fieldset>
  );
}

export function Select({ value, options, onChange, disabled, label }: {
  value: string;
  options: [string, string][];
  onChange?: (v: string) => void;
  disabled?: boolean;
  label?: string;
}) {
  // The browser's own arrow sits flush on the edge at its own size; ours keeps the control's 14px inset.
  return (
    <span style={{ position: 'relative', display: 'flex', width: '100%', opacity: disabled ? 0.7 : 1 }}>
      <select value={value} disabled={disabled} aria-label={label} onChange={(e) => onChange?.(e.target.value)}
        className={disabled ? undefined : 'k-input'}
        style={{
          appearance: 'none', WebkitAppearance: 'none', width: '100%', minHeight: 44, padding: '0 44px 0 14px',
          boxSizing: 'border-box', borderRadius: 10, border: `1px solid ${disabled ? 'var(--border-weak)' : 'var(--border)'}`,
          background: 'var(--bg)', color: disabled ? 'var(--sub)' : 'var(--text)', font: `var(--t-small) ${FONT}`,
          cursor: disabled ? 'default' : 'pointer',
        }}>
        {options.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
      </select>
      <span aria-hidden="true" style={{
        position: 'absolute', right: 14, top: '50%', transform: 'translateY(-50%)', display: 'inline-flex',
        pointerEvents: 'none', color: 'var(--sub)',
      }}><Icon name="chevron" /></span>
    </span>
  );
}

/** A label over a control, as the play screen's panels lay them out. */
export function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <span style={{ font: `var(--t-meta) ${FONT}`, color: 'var(--sub)' }}>{label}</span>
      {children}
    </div>
  );
}

export type Levels = { depth: number; solve: number };

/** The app's levels (crates/gui/src-ui/js/state.ts). */
export const LEVELS: Levels[] = [
  { depth: 1, solve: 2 }, { depth: 2, solve: 4 }, { depth: 4, solve: 8 }, { depth: 6, solve: 10 },
  { depth: 8, solve: 12 }, { depth: 10, solve: 14 }, { depth: 12, solve: 16 }, { depth: 14, solve: 18 },
  { depth: 16, solve: 20 }, { depth: 18, solve: 22 }, { depth: 20, solve: 24 }, { depth: 22, solve: 26 },
  { depth: 24, solve: 26 },
];
export const LEVEL0 = LEVELS[6];
const SOLVE_MAX = 36;

export function clampLevels(v: Levels): Levels {
  const depth = Math.max(1, Math.min(SOLVE_MAX, v.depth));
  return { depth, solve: Math.max(depth, Math.min(SOLVE_MAX, v.solve)) };
}

export const levelLabel = (lang: Lang, v: Levels): string => {
  const i = LEVELS.findIndex((l) => l.depth === v.depth && l.solve === v.solve);
  return i >= 0 ? 'Lv' + (i + 1) : t(lang, 'strength.custom');
};

export function Strength({ lang, label, value, onChange, disabled }: {
  lang: Lang; label: string; value: Levels; onChange: (v: Levels) => void; disabled?: boolean;
}) {
  const S = (key: string, vars?: Record<string, string | number>) => t(lang, 'strength.' + key, vars);
  const [picked, setPicked] = React.useState(false);
  const preset = LEVELS.findIndex((l) => l.depth === value.depth && l.solve === value.solve);
  const custom = picked || preset < 0;
  const numbers = (min: number) => Array.from({ length: SOLVE_MAX - min + 1 }, (_, i) => [String(min + i), String(min + i)] as [string, string]);
  return (
    <Field label={label}>
      <Select label={label} value={custom ? 'custom' : String(preset)} disabled={disabled}
        options={[...LEVELS.map((l, i) => [String(i), S('preset', { n: i + 1, depth: l.depth, solve: l.solve })] as [string, string]),
                  ['custom', S('custom')]]}
        onChange={(s) => {
          if (s === 'custom') { setPicked(true); return; }
          setPicked(false);
          onChange(LEVELS[+s]);
        }} />
      {custom && (
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
          <Field label={S('depth')}>
            <Select label={S('depth')} value={String(value.depth)} options={numbers(1)} disabled={disabled}
              onChange={(n) => onChange(clampLevels({ ...value, depth: +n }))} />
          </Field>
          <Field label={S('solve')}>
            <Select label={S('solve')} value={String(value.solve)} options={numbers(value.depth)} disabled={disabled}
              onChange={(n) => onChange(clampLevels({ ...value, solve: +n }))} />
          </Field>
        </div>
      )}
    </Field>
  );
}

export function Toggle({ checked, onChange, label, disabled }: {
  checked: boolean; onChange?: (v: boolean) => void; label: string; disabled?: boolean;
}) {
  return (
    <label style={{
      display: 'flex', alignItems: 'center', gap: 12, minHeight: 44, font: `var(--t-small) ${FONT}`,
      opacity: disabled ? 0.45 : 1, cursor: disabled ? 'default' : 'pointer',
    }}>
      <span style={{ flex: 1 }}>{label}</span>
      <button type="button" role="switch" aria-checked={checked} disabled={disabled} onClick={() => onChange?.(!checked)}
        style={{
          flex: 'none', width: 44, height: 26, borderRadius: 13, border: 0, padding: 0, position: 'relative',
          background: checked ? 'var(--accent)' : 'var(--border)', cursor: 'inherit',
        }}>
        <span style={{
          position: 'absolute', top: 3, left: checked ? 21 : 3, width: 20, height: 20, borderRadius: '50%',
          background: checked ? 'var(--on-accent)' : 'var(--sub)', transition: 'left var(--dur) var(--ease)',
        }} />
      </button>
    </label>
  );
}

export function Progress({ value, label }: { value: number; label: string }) {
  return (
    <div role="progressbar" aria-label={label} aria-valuenow={Math.round(value * 100)} aria-valuemin={0} aria-valuemax={100}
      style={{ height: 6, borderRadius: 3, background: 'var(--track)', overflow: 'hidden' }}>
      <span style={{ display: 'block', width: Math.round(value * 100) + '%', height: '100%', background: 'var(--accent)' }} />
    </div>
  );
}

export function Busy({ children }: { children: React.ReactNode }) {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8, color: 'var(--accent)', font: `var(--t-small) ${FONT}` }}>
      <span aria-hidden="true" style={{ width: 8, height: 8, borderRadius: '50%', flex: 'none', background: 'var(--accent)' }} />
      {children}
    </span>
  );
}

export function StoneDot({ color, size = 18 }: { color: 'b' | 'w'; size?: 12 | 18 | 28 }) {
  const black = color === 'b';
  return <span aria-hidden="true" style={{
    width: size, height: size, borderRadius: '50%', flex: 'none',
    // A soft highlight keeps a black mark on a near-black ground a filled stone, not a ring.
    background: black ? 'radial-gradient(circle at 35% 30%, var(--stone-black-dot-hi), var(--stone-black) 70%)' : 'var(--stone-white)',
    boxShadow: 'inset 0 0 0 1px ' + (black ? 'var(--stone-black-dot-edge)' : 'var(--stone-white-edge)'),
  }} />;
}

/** `note` sits at the right of the heading (`t-meta`). */
export function Panel({ id, title, note, children }: {
  id: string; title: string; note?: string;
  children: React.ReactNode;
}) {
  return (
    <section aria-labelledby={id} style={{
      display: 'flex', flexDirection: 'column', gap: 16, padding: 20,
      border: '1px solid var(--border)', borderRadius: 'var(--r-card)', background: 'var(--panel)',
    }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, minHeight: 36 }}>
        <h2 id={id} style={{ margin: 0, font: `700 var(--t-body)/1.35 ${FONT}` }}>{title}</h2>
        <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 12 }}>
          {note && <span style={{ font: `var(--t-meta) ${FONT}`, color: 'var(--sub)' }}>{note}</span>}
                  </span>
      </div>
      {children}
    </section>
  );
}

export type ScoreSide = { count: number; turn?: boolean; busy?: string };

/**
 * Both sides' disc counts above the board, large. The side to move stands out: its card is raised, its stone ringed
 * (the ring breathes while KUROOBI thinks); the other side's stone and count fade back. Once the game is over the two
 * cards become one, the `result` beside the final count, so the counts are never said twice. `tone` is the reader's
 * side of it in a game (a win in the accent, a loss in `bad`); `neutral` for a draw and for study.
 */
export function Scoreboard({ lang, black, white, result }: {
  lang: Lang; black: ScoreSide; white: ScoreSide; result?: { title: string; tone: 'win' | 'loss' | 'neutral' };
}) {
  const S = (key: string) => t(lang, 'score.' + key);
  const someone = black.turn || white.turn;
  // At the end the same two cards say who won, at their right where Thinking stands in a game: the winner's raised
  // with Win, the other's back with Loss; the stone and the count never move (site.css .w-score-*).
  const winner = !result ? null : black.count > white.count ? 'b' : white.count > black.count ? 'w' : 'draw';
  const card = (color: 'b' | 'w', side: ScoreSide) => {
    const won = winner === color || winner === 'draw';
    const back = result ? !won : someone && !side.turn;
    // In a game the player's own card carries the tone: filled in accent for a win, a bad edge for a loss.
    const mine = result && ((result.tone === 'win' && winner === color) || (result.tone === 'loss' && winner !== color && winner !== 'draw'));
    const fill = mine && result!.tone === 'win';
    const bad = mine && result!.tone === 'loss';
    const raised = result ? won : side.turn;
    return (
      <div role="group" key={color} className={'w-score' + (result ? ' w-result' : '')}
        aria-label={[S(color === 'b' ? 'black' : 'white'), side.count, !result && side.turn ? S('toMove') : ''].filter((x) => x !== '').join(' ')}
        style={{
          minWidth: 0, borderRadius: 12,
          border: `1px solid ${fill ? 'var(--accent)' : bad ? 'var(--bad)' : raised ? 'var(--border)' : 'var(--border-weak)'}`,
          background: fill ? 'var(--accent-dim)' : raised ? 'var(--card)' : 'transparent',
        }}>
        <div className="w-score-in">
          <span className={'w-score-stone' + (!result && side.turn && side.busy ? ' w-breathe' : '')} style={{
            display: 'inline-flex', borderRadius: '50%', opacity: back ? 0.5 : 1,
            boxShadow: !result && side.turn ? '0 0 0 3px var(--card), 0 0 0 6px var(--accent)' : 'none',
          }}><StoneDot color={color} size={28} /></span>
          <span style={{ font: `700 32px/1 ${CODE}`, fontVariantNumeric: 'tabular-nums', color: fill ? 'var(--on-accent)' : back ? 'var(--sub)' : 'var(--text)' }}>{side.count}</span>
          {result && (
            <span aria-hidden="true" className="w-score-side w-score-res" style={{
              fontFamily: FONT, fontWeight: 700, whiteSpace: 'nowrap',
              color: fill ? 'var(--on-accent)' : bad ? 'var(--bad)' : back ? 'var(--sub)' : 'var(--text)',
            }}>{S(winner === 'draw' ? 'draw' : won ? 'win' : 'loss')}</span>
          )}
          {!result && side.busy && <span role="status" className="w-score-side" style={{ minWidth: 0 }}><Busy><span className="w-score-busy">{side.busy}</span></Busy></span>}
        </div>
      </div>
    );
  };
  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 12 }}>
      {result && <span role="status" className="w-sr">{`${result.title} ${S('black')} ${black.count} – ${white.count} ${S('white')}`}</span>}
      {card('b', black)}
      {card('w', white)}
    </div>
  );
}

/** A whole number with its sign; zero carries none. */
export const wholeEval = (v: number): string => {
  const r = Math.round(v);
  return (r > 0 ? '+' : r < 0 ? '−' : '') + Math.abs(r);
};

export function CandidateList({ moves, empty }: {
  moves: { square: string; score: number }[];
  empty: string;
}) {
  return (
    <ol style={{ margin: 0, padding: 0, listStyle: 'none', display: 'flex', flexDirection: 'column', maxHeight: 320, overflowY: 'auto' }}>
      {moves.length ? moves.map((m) => (
        <li key={m.square} style={{
          display: 'flex', alignItems: 'center', gap: 12, minHeight: 40, borderBottom: '1px solid var(--border-weak)',
          font: `var(--t-small) ${CODE}`, fontVariantNumeric: 'tabular-nums',
        }}>
          <span style={{ fontWeight: 500 }}>{m.square}</span>
          <span style={{ marginLeft: 'auto', color: Math.round(m.score) < 0 ? 'var(--bad)' : 'var(--text)' }}>{wholeEval(m.score)}</span>
        </li>
      )) : (
        <li style={{ minHeight: 40, display: 'flex', alignItems: 'center', font: `var(--t-small) ${FONT}`, color: 'var(--sub)' }}>{empty}</li>
      )}
    </ol>
  );
}

/** What KUROOBI read for a move it played: its value, how deep (or solved), and the work it took. */
export type Reading = { score: number; depth: number; exact: boolean; ms: number; nodes: number; selective?: number };
/**
 * `depth` (or `solved`, or `selective`, a selective solve's confidence in %) is how the analysis read the position
 * after the move, shown beside `score` in study.
 */
export type Move = {
  n: number; move: string; color: 'b' | 'w'; score?: number; loss?: number; depth?: number; solved?: boolean; selective?: number;
  read?: Reading;
};

/** 1,234 · 12.3k · 4.5M: a count kept to a few figures. */
export const compact = (n: number): string =>
  n < 1e4 ? Math.round(n).toLocaleString('en') : n < 1e6 ? (n / 1e3).toFixed(1) + 'k' : (n / 1e6).toFixed(1) + 'M';

/**
 * The record. `columns` says what follows each move: `eval` the analysis's value and depth (study), `none` nothing, `reading`
 * nothing either but a mark on KUROOBI's own moves, each of which opens, when pressed, to what it read: value, depth,
 * time, nodes and NPS. `open` holds one open, for the canvas.
 */
export function KifuTable({ lang, moves, current, onSelect, columns = 'eval', open: held }: {
  lang: Lang; moves: Move[]; current?: number; onSelect?: (n: number) => void; columns?: 'eval' | 'none' | 'reading'; open?: number;
}) {
  const K = (key: string, vars?: Record<string, string | number>) => t(lang, 'kifu.' + key, vars);
  const box = React.useRef<HTMLDivElement>(null);
  const row = React.useRef<HTMLButtonElement>(null);
  const [openState, setOpen] = React.useState<number | null>(null);
  const opened = held ?? openState;
  const openBlock = React.useRef<HTMLDivElement>(null);
  // A row opened at the foot of the box would open out of sight: bring the whole block in.
  React.useEffect(() => {
    const b = box.current, o = openBlock.current;
    if (!b || !o) return;
    const top = o.offsetTop - b.offsetTop, bottom = top + o.offsetHeight;
    if (bottom > b.scrollTop + b.clientHeight) b.scrollTop = Math.min(top, bottom - b.clientHeight);
    else if (top < b.scrollTop) b.scrollTop = top;
  }, [opened]);
  React.useEffect(() => {
    const b = box.current, r = row.current;
    if (!b || !r) return;
    const top = r.offsetTop - b.offsetTop, bottom = top + r.offsetHeight;
    if (top < b.scrollTop) b.scrollTop = top;
    else if (bottom > b.scrollTop + b.clientHeight) b.scrollTop = bottom - b.clientHeight;
  }, [current, moves.length]);
  const cols = columns === 'eval' ? '2.5em 1fr auto 5.5em' : columns === 'reading' ? '2.5em 1fr 18px' : '2.5em 1fr';
  const grid: React.CSSProperties = { display: 'grid', gridTemplateColumns: cols, alignItems: 'center', gap: 12, padding: '0 8px' };
  const right: React.CSSProperties = { textAlign: 'right' };
  return (
    <div style={{ display: 'flex', flexDirection: 'column', minHeight: 0, flex: 1 }}>
      <div style={{ ...grid, minHeight: 32, borderBottom: '1px solid var(--border)', font: `var(--t-meta) ${FONT}`, color: 'var(--sub)' }}>
        <span style={right}>#</span><span>{K('move')}</span>
        {columns === 'eval' && <><span style={right}>{K('eval')}</span><span style={right}>{K('depth')}</span></>}
      </div>
      <div ref={box} style={{ flex: 1, minHeight: 0, overflowY: 'auto', overscrollBehavior: 'contain' }}>
        {!moves.length && (
          <div style={{ padding: '10px 8px', font: `var(--t-small) ${FONT}`, color: 'var(--sub)' }}>{K('empty')}</div>
        )}
        {moves.map((m) => {
          const on = m.n === current;
          const r = columns === 'reading' ? m.read : undefined;
          const expanded = !!r && opened === m.n;
          const press = onSelect ? () => onSelect(m.n) : r ? () => setOpen(expanded ? null : m.n) : undefined;
          const button = (
            <button type="button" ref={on ? row : undefined} onClick={press} disabled={!press}
              aria-current={on || undefined} aria-expanded={r ? expanded : undefined} className={press ? 'k-row' : undefined}
              style={{
                ...grid, width: '100%', minHeight: 44, border: 0, borderBottom: expanded ? 0 : '1px solid var(--border-weak)',
                background: expanded ? 'transparent' : on ? 'var(--card)' : 'transparent',
                boxShadow: on && !expanded ? 'inset 2px 0 0 var(--accent)' : 'none',
                color: 'var(--text)', textAlign: 'left', font: `var(--t-small) ${CODE}`, fontVariantNumeric: 'tabular-nums',
                cursor: press ? 'pointer' : 'default',
              }}>
              <span style={{ ...right, color: 'var(--sub)', fontSize: 'var(--t-meta)' }}>{m.n}</span>
              <span style={{ display: 'flex', alignItems: 'center', gap: 8 }}><StoneDot color={m.color} size={12} />{m.move}</span>
              {columns === 'eval' && (
                <span style={{ display: 'flex', alignItems: 'baseline', gap: 8, justifyContent: 'flex-end' }}>
                  {m.loss ? <span style={{ color: 'var(--bad)', fontSize: 'var(--t-meta)' }}>▼{Math.round(m.loss)}</span> : null}
                  <span>{m.score === undefined ? '' : wholeEval(m.score)}</span>
                </span>
              )}
              {columns === 'eval' && (
                <span style={{ ...right, color: 'var(--sub)' }}>
                  {m.solved ? K('solve') : m.selective ? K('selective', { p: m.selective }) : m.depth ?? ''}
                </span>
              )}
              {columns === 'reading' && (
                <span aria-hidden="true" style={{
                  display: 'inline-flex', color: 'var(--sub)', visibility: r ? 'visible' : 'hidden',
                  transform: expanded ? 'rotate(180deg)' : 'none', transition: 'transform 160ms var(--ease)',
                }}><Icon name="chevron" /></span>
              )}
            </button>
          );
          if (!expanded || !r) return <React.Fragment key={m.n}>{button}</React.Fragment>;
          // The row and what it read are one block: one ground, one hover, one accent edge when it is the current move.
          const fact = (label: string, value: string) => (
            <div key={label} style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 0 }}>
              <dt style={{ font: `var(--t-meta)/1.3 ${FONT}`, color: 'var(--sub)' }}>{label}</dt>
              <dd style={{ margin: 0, font: `var(--t-small)/1.3 ${CODE}`, color: 'var(--text)', whiteSpace: 'nowrap' }}>{value}</dd>
            </div>
          );
          return (
            <div key={m.n} ref={openBlock} className="w-kifu-open" style={{
              background: 'var(--card)', borderBottom: '1px solid var(--border-weak)',
              boxShadow: on ? 'inset 2px 0 0 var(--accent)' : 'none',
            }}>
              {button}
              {/* Same font size as the row, so 2.5em + the row's padding and gap puts it under the move. */}
              <dl style={{
                display: 'grid', gridTemplateColumns: 'repeat(3, minmax(0, 1fr))', gap: '10px 12px', margin: 0,
                padding: '2px 8px 14px calc(2.5em + 20px)', fontSize: 'var(--t-small)', fontVariantNumeric: 'tabular-nums',
              }}>
                {fact(K('evalLabel'), wholeEval(r.score))}
                {fact(K('depthLabel'), r.exact ? K('solve') : r.selective ? K('selective', { p: r.selective }) : String(r.depth))}
                {fact(K('timeLabel'), K('seconds', { s: (r.ms / 1000).toFixed(2) }))}
                {fact(K('nodesLabel'), compact(r.nodes))}
                {fact(K('npsLabel'), compact(r.ms > 0 ? r.nodes / (r.ms / 1000) : 0))}
              </dl>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Best moves named in the answer column before the rest are only counted. */
const ANSWER_MOVES = 2;

/**
 * One problem of a run: its answer when the obf gives one, and once solved what KUROOBI found and what it took;
 * `right` is left out where there is no answer to hold it to.
 */
export type BenchRow = {
  no: number; empties: number; answer?: { moves: string[]; score: number };
  got?: { move: string; score: number; right?: boolean; ms: number; nodes: number };
  running?: boolean;
};

/**
 * The problems of a run (FFO's or an obf of one's own), one row each: number, empties, time, nodes and NPS (first, so a phone shows them without
 * scrolling), then the answer and KUROOBI's move and value (in `bad` when it is not the answer); the totals at the foot
 * once a problem is solved. A row pressed shows its board.
 */
export function BenchTable({ lang, rows, current, onSelect }: {
  lang: Lang; rows: BenchRow[]; current?: number; onSelect?: (i: number) => void;
}) {
  const B = (key: string, vars?: Record<string, string | number>) => t(lang, 'bench.' + key, vars);
  const sec = (ms: number) => t(lang, 'kifu.seconds', { s: (ms / 1000).toFixed(3) });
  const nps = (nodes: number, ms: number) => compact(ms > 0 ? nodes / (ms / 1000) : 0);
  const grid: React.CSSProperties = {
    display: 'grid', gridTemplateColumns: '1.8em 2.5em 5.5em 4.5em 4.5em minmax(10em, 1fr) minmax(6em, 1fr)',
    alignItems: 'center', gap: 8, padding: '0 8px', minWidth: 540, boxSizing: 'border-box',
  };
  const right: React.CSSProperties = { textAlign: 'right' };
  // The measured figures end flush right; the moves that follow start a column of their own.
  const answerGap: React.CSSProperties = { paddingLeft: 12 };
  const clip: React.CSSProperties = { minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' };
  // A problem can have many best moves (FFO #39 has nine): two are named, the rest counted; all of them count as right.
  const answer = (a: { moves: string[]; score: number }) => (
    <>
      {a.moves.slice(0, ANSWER_MOVES).join('/')}
      {a.moves.length > ANSWER_MOVES && <span style={{ color: 'var(--sub)', fontSize: 'var(--t-meta)' }}> {B('more', { n: a.moves.length - ANSWER_MOVES })}</span>}
      {' '}{wholeEval(a.score)}
    </>
  );
  const solved = rows.filter((r) => r.got);
  const ms = solved.reduce((a, r) => a + r.got!.ms, 0);
  const nodes = solved.reduce((a, r) => a + r.got!.nodes, 0);
  const checked = solved.filter((r) => r.got!.right !== undefined);
  const cell: React.CSSProperties = { font: `var(--t-small) ${CODE}`, fontVariantNumeric: 'tabular-nums' };
  return (
    <div style={{ overflowX: 'auto', overscrollBehaviorX: 'contain' }}>
      <div role="table" aria-label={B('results')}>
        <div role="row" style={{ ...grid, minHeight: 32, borderBottom: '1px solid var(--border)', font: `var(--t-meta) ${FONT}`, color: 'var(--sub)' }}>
          {(['no', 'empties', 'time', 'nodes', 'nps', 'answer', 'got'] as const).map((k, i) => (
            <span role="columnheader" key={k} style={i < 5 ? right : i === 5 ? answerGap : undefined}>{B(k)}</span>
          ))}
        </div>
        {!rows.length && <div style={{ padding: '10px 8px', font: `var(--t-small) ${FONT}`, color: 'var(--sub)' }}>{B('empty')}</div>}
        {rows.map((r, i) => {
          const on = i === current;
          const g = r.got;
          return (
            <button type="button" role="row" key={r.no} onClick={onSelect ? () => onSelect(i) : undefined} disabled={!onSelect}
              aria-current={on || undefined} className={onSelect ? 'k-row' : undefined}
              style={{
                ...grid, width: '100%', minHeight: 40, border: 0, borderBottom: '1px solid var(--border-weak)',
                background: on ? 'var(--card)' : 'transparent', boxShadow: on ? 'inset 2px 0 0 var(--accent)' : 'none',
                color: 'var(--text)', textAlign: 'left', cursor: onSelect ? 'pointer' : 'default', ...cell,
              }}>
              <span role="cell" style={{ ...right, color: 'var(--sub)', fontSize: 'var(--t-meta)' }}>{r.no}</span>
              <span role="cell" style={right}>{r.empties}</span>
              <span role="cell" style={right}>{g ? sec(g.ms) : ''}</span>
              <span role="cell" style={right}>{g ? compact(g.nodes) : ''}</span>
              <span role="cell" style={right}>{g ? nps(g.nodes, g.ms) : ''}</span>
              <span role="cell" style={{ ...answerGap, ...clip }} title={r.answer?.moves.join(' ')}>{r.answer ? answer(r.answer) : ''}</span>
              <span role="cell" style={{ ...clip, color: g && g.right === false ? 'var(--bad)' : undefined }}>
                {g ? <>{g.move} {wholeEval(g.score)}{g.right === false && <span className="w-sr"> ({B('wrong')})</span>}</>
                  : r.running ? <span role="status" style={{ display: 'inline-flex' }}><Busy><span className="w-sr">{B('running')}</span></Busy></span> : ''}
              </span>
            </button>
          );
        })}
        {solved.length > 0 && (
          <div role="row" style={{ ...grid, minHeight: 40, borderTop: '1px solid var(--border)', ...cell, fontWeight: 700 }}>
            <span role="cell" style={{ gridColumn: 'span 2', font: `700 var(--t-small) ${FONT}` }}>{B('total')}</span>
            <span role="cell" style={right}>{sec(ms)}</span>
            <span role="cell" style={right}>{compact(nodes)}</span>
            <span role="cell" style={right}>{nps(nodes, ms)}</span>
            <span role="cell" />
            <span role="cell">{checked.length ? `${checked.filter((r) => r.got!.right).length} / ${checked.length}` : ''}</span>
          </div>
        )}
      </div>
    </div>
  );
}
