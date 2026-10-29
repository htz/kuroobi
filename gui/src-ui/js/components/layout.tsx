import React, { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { Board, BoardDefs, START_CELLS } from './board';
import { Button, Dot, Segmented } from './primitives';
import { IconButton } from './Icons';
import { t, useLang } from '../i18n';
import icon from '../../assets/icon.svg?raw';

export function AppFrame({ children }: { children: React.ReactNode }) {
  return (
    <div style={{
      position: 'relative', height: '100%', display: 'flex', flexDirection: 'column',
      background: 'var(--bg)',
    }}>
      <BoardDefs />
      {children}
    </div>
  );
}

export function WindowBar({ title, sub, right }: {
  title: string;
  sub?: React.ReactNode;
  right?: React.ReactNode;
}) {
  return (
    <div data-tauri-drag-region className="k-drag" style={{
      position: 'relative',
      height: 'var(--h-window)', flex: 'none', background: 'var(--bg)',
      borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center',
      padding: '0 var(--w-signals)', gap: 'var(--sp-2)',
    }}>
      <span data-tauri-drag-region style={{
        margin: '0 auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
        minWidth: 0, fontSize: 'var(--fs-6)',
      }}>
        <span style={{ color: 'var(--text)' }}>{title}</span>
        {sub && (
          <span style={{
            color: 'var(--sub)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{sub}</span>
        )}
      </span>
      {right && (
        <span style={{
          position: 'absolute', right: 'var(--sp-3)', top: 0,
          height: 'var(--h-window)', display: 'flex', alignItems: 'center', gap: 'var(--sp-1)',
        }}>{right}</span>
      )}
    </div>
  );
}

export function Body({ children }: { children: React.ReactNode }) {
  return <div style={{ position: 'relative', flex: 1, display: 'flex', minHeight: 0 }}>{children}</div>;
}

export function Main({ children, inset }: {
  children: React.ReactNode;
  inset?: boolean;
}) {
  return (
    <div className={inset ? 'k-dock-inset' : undefined}
         style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>
      {children}
    </div>
  );
}

const CompactContext = React.createContext(false);

export const useToolbarCompact = (): boolean => React.useContext(CompactContext);

export function Toolbar({ children, aux, end, dock, graph }: {
  children: React.ReactNode;
  aux?: React.ReactNode;
  /** Actions kept at the right end at every width (aux gives way first). */
  end?: React.ReactNode;
  dock?: { open: boolean; onToggle: () => void };
  graph?: { open: boolean; onToggle: () => void };
}) {
  const [group, setGroup] = React.useState<HTMLElement | null>(null);
  const [spacer, setSpacer] = React.useState<HTMLElement | null>(null);
  const [compact, setCompact] = React.useState(false);
  const fullWidth = React.useRef(0);
  const lang = useLang();

  const seenLang = React.useRef(lang);

  React.useEffect(() => {
    if (!group || !spacer) return;
    if (seenLang.current !== lang) { seenLang.current = lang; fullWidth.current = 0; }
    const measure = () => {
      if (!compact) {
        fullWidth.current = group.scrollWidth;
        if (group.scrollWidth > group.clientWidth + 1) setCompact(true);
      } else if (!fullWidth.current) {
        setCompact(false);            // no memory (new language): re-measure full
      } else if (spacer.offsetWidth > fullWidth.current - group.scrollWidth + 12) {
        setCompact(false);
      }
    };
    const ro = new ResizeObserver(measure);
    ro.observe(group);
    ro.observe(spacer);
    return () => ro.disconnect();
  }, [group, spacer, compact, lang]);

  return (
    <div style={{
      height: 'var(--h-bar)', flex: 'none', borderBottom: '1px solid var(--border-weak)',
      display: 'flex', alignItems: 'center', padding: '0 var(--sp-4)', gap: 'var(--sp-2)',
      overflow: 'hidden', minWidth: 0,
    }}>
      <span ref={setGroup} style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                                    minWidth: 0, overflow: 'hidden', flexShrink: 1 }}>
        <CompactContext.Provider value={compact}>{children}</CompactContext.Provider>
      </span>
      <span ref={setSpacer} style={{ flex: 1 }} />
      {end && <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', flex: 'none', whiteSpace: 'nowrap' }}>{end}</span>}
      {aux && <span className="k-toolbar-aux" style={{ alignItems: 'center', gap: 'var(--sp-3)' }}>{aux}</span>}
      {graph && (
        <span className="k-graph-toggle">
          <Button size="chip" variant={graph.open ? 'secondary' : 'ghost'}
                  title={t('ui.toolbar.graph_title')} onClick={graph.onToggle}>{t('ui.toolbar.graph')}</Button>
        </span>
      )}
      {dock && (
        <span className="k-dock-toggle">
          <IconButton name="panel" label={t(dock.open ? 'ui.panel.close' : 'ui.panel.open')}
                      onClick={dock.onToggle} />
        </span>
      )}
    </div>
  );
}

export function Dock({ tabs, active, onTab, children, open, scroll = true }: {
  tabs: { id: string; label: string }[]; active: string; onTab?: (id: string) => void;
  children: React.ReactNode;
  open?: boolean;
  scroll?: boolean;
}) {
  return (
    <aside className={'k-dock' + (open ? ' k-open' : '')} style={{
      flex: 'none', background: 'var(--panel)',
      borderLeft: '1px solid var(--border)', flexDirection: 'column', minHeight: 0,
    }}>
      {/* A lone tab has nothing to switch to; its sections carry their own titles. */}
      {tabs.length > 1 && (
        <div style={{ padding: 'var(--sp-2)', flex: 'none' }}>
          <Segmented fill value={active} onChange={onTab}
                     options={tabs.map(tab => ({ value: tab.id, label: tab.label }))} />
        </div>
      )}
      <div className={scroll ? 'k-scroll' : undefined} style={{
        flex: 1, minHeight: 0, display: scroll ? undefined : 'flex', flexDirection: 'column',
      }}>{children}</div>
    </aside>
  );
}

export function Section({ title, aside, grow, head, children }: {
  title: string; aside?: React.ReactNode; children?: React.ReactNode;
  grow?: boolean;
  head?: React.ReactNode;   // outside the scroller, so a table header stays put
}) {
  return (
    <section style={{
      display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
      padding: '0 var(--sp-3) var(--sp-4)',
      ...(grow ? { flex: 1, minHeight: 0, overflow: 'hidden' } : { flex: 'none' }),
    }}>
      <h3 style={{
        margin: 0, minHeight: aside ? 'var(--h-field)' : 'var(--h-head)',
        display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        paddingBottom: aside ? 'var(--sp-1)' : 0,
        ...captionStyle,
        borderBottom: '1px solid var(--border)',
      }}>{title}{aside && <span style={{
        marginLeft: 'auto', letterSpacing: 0, fontWeight: 400, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
      }}>{aside}</span>}</h3>
      {head}
      {grow
        ? <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>{children}</div>
        : children}
    </section>
  );
}

export function BoardFit({ minHeight = 0, padding, children }: {
  minHeight?: number; padding?: string; children: React.ReactNode;
}) {
  return (
    <div style={{
      flex: 1, minHeight, display: 'grid', placeItems: 'center',
      gridTemplateRows: 'minmax(0, 1fr)', gridTemplateColumns: 'minmax(0, 1fr)',
      padding,
    }}>
      <div style={{ height: '100%', maxHeight: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
        {children}
      </div>
    </div>
  );
}

/**
 * The right side (meters, connection) always shows. The left side is a row of items in priority order;
 * when the window is too narrow, the items that do not fit whole are hidden from the end, never cut.
 */
export function StatusBar({ left, right }: { left?: React.ReactNode; right?: React.ReactNode }) {
  const row = useRef<HTMLDivElement>(null);
  const fit = () => {
    const el = row.current;
    if (!el) return;
    const limit = el.getBoundingClientRect().right;
    let full = false;
    for (const c of Array.from(el.children) as HTMLElement[]) {
      c.style.visibility = '';
      if (full || c.getBoundingClientRect().right > limit + 0.5) { c.style.visibility = 'hidden'; full = true; }
    }
  };
  // Items come and go every render (the thinking seconds tick), and the window can be resized.
  useLayoutEffect(fit);
  useEffect(() => {
    const el = row.current;
    if (!el) return;
    const o = new ResizeObserver(() => fit());
    o.observe(el);
    return () => o.disconnect();
  }, []);
  return (
    <div style={{
      height: 'var(--h-status)', flex: 'none', background: 'var(--bg)', borderTop: '1px solid var(--border)',
      display: 'flex', alignItems: 'center', gap: 'var(--sp-4)', padding: '0 var(--sp-4)',
      fontSize: 'var(--fs-6)', color: 'var(--sub)', whiteSpace: 'nowrap',
      fontVariantNumeric: 'tabular-nums',
    }}>
      <div ref={row} style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-4)', overflow: 'hidden' }}>
        {left}
      </div>
      <span style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-4)' }}>{right}</span>
    </div>
  );
}

export function Modal({ title, sub, body, actions, width = 'var(--w-modal)', onClose, scroll, band, children }: {
  title: string;
  sub?: React.ReactNode;
  body?: React.ReactNode;
  actions?: React.ReactNode;
  width?: string;
  onClose?: () => void;
  scroll?: boolean;
  band?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <div role="dialog" aria-modal style={{
      width, maxHeight: '88vh',
      borderRadius: 'var(--r-4)', background: 'var(--bg)',
      border: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
      display: 'flex', flexDirection: 'column', overflow: 'hidden',
    }}>
      <div style={{
        height: 'var(--h-bar)', flex: 'none', background: 'var(--panel)',
        borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center',
        padding: '0 var(--sp-2)',
      }}>
        <span style={{ width: 32, flex: 'none' }} />
        <div style={{ flex: 1, minWidth: 0, textAlign: 'center' }}>
          <div className="k-sel" style={{
            fontSize: 'var(--fs-4)', fontWeight: 600, color: 'var(--text)',
            overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{title}</div>
          {sub && <div style={{
            fontSize: 'var(--fs-7)', color: 'var(--sub)',
            overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{sub}</div>}
        </div>
        <span style={{ width: 32, flex: 'none', display: 'grid', placeItems: 'center' }}>
          {onClose && <IconButton name="close" label={t('ui.close')} onClick={onClose} />}
        </span>
      </div>

      {band && (
        <div style={{
          flex: 'none', height: 'var(--h-bar)', background: 'var(--card)',
          borderBottom: '1px solid var(--border)', padding: '0 var(--sp-4)',
          display: 'flex', alignItems: 'center',
        }}>{band}</div>
      )}

      <div className={scroll ? 'k-scroll' : undefined} style={{
        flex: scroll ? 1 : 'none', minHeight: 0, background: 'var(--bg)',
        padding: 'var(--sp-4) var(--sp-5)',
        display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
      }}>
        {body && <div style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)', lineHeight: 1.7 }}>{body}</div>}
        {children}
      </div>

      {actions && (
        <div style={{
          flex: 'none', display: 'flex', gap: 'var(--sp-2)', alignItems: 'center',
          padding: 'var(--sp-3) var(--sp-5)', borderTop: '1px solid var(--border)',
          background: 'var(--panel)',
        }}>{actions}</div>
      )}
    </div>
  );
}

export function List({ children }: { children: React.ReactNode }) {
  return <div style={{ display: 'flex', flexDirection: 'column' }}>{children}</div>;
}

export function Busy({ children }: { children: React.ReactNode }) {
  return (
    <span style={{
      display: 'flex', alignItems: 'center',
      gap: 'var(--sp-2)', color: 'var(--accent)',
    }}>
      <Dot />{children}
    </span>
  );
}

export function KeyValue({ label, value, big }: {
  label: string;
  value: React.ReactNode;
  big?: boolean;
}) {
  const shown = typeof value === 'number' ? value.toLocaleString()
    : value === undefined ? '—' : value;
  return (
    <div style={{ display: 'flex', alignItems: 'center', fontSize: 'var(--fs-5)' }}>
      <span style={{ color: 'var(--sub)' }}>{label}</span>
      <span style={{
        marginLeft: 'auto',
        ...(big
          ? { fontSize: 'var(--fs-3)', fontWeight: 600, fontVariantNumeric: 'tabular-nums' }
          : { overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }),
      }}>{shown}</span>
    </div>
  );
}

export function Divider() {
  return <span style={{
    width: 1, height: 20, flex: 'none',
    margin: '0 var(--sp-1)', background: 'var(--border)',
  }} />;
}

export function Note({ children }: { children: React.ReactNode }) {
  return <p style={{
    margin: 0, maxWidth: 'var(--w-text)',
    fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8,
  }}>{children}</p>;
}

export interface Col {
  head?: React.ReactNode;
  w?: number;
  right?: boolean;
  center?: boolean;
  num?: boolean;
  clip?: boolean;
}

function cell(c: Col, head?: boolean): React.CSSProperties {
  const size = c.w === undefined ? { flex: 1, minWidth: 0 } : { width: c.w, flex: 'none' };
  if (!head && c.clip) {
    return {
      ...size,
      overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
      textAlign: c.right ? 'right' : c.center ? 'center' : undefined,
    };
  }
  return {
    ...size,
    display: 'flex', alignItems: 'center', gap: 'var(--sp-0)',
    justifyContent: c.right ? 'flex-end' : c.center ? 'center' : 'flex-start',
    whiteSpace: 'nowrap', ...(head ? { overflow: 'hidden' } : null),
    ...(!head && c.num ? { fontVariantNumeric: 'tabular-nums' } : null),
  };
}

export function TableHead({ cols, pad = 'var(--sp-3)', gap = 'var(--sp-2)', children }: {
  cols?: Col[];
  pad?: string;
  /** The space between columns; the same as the rows' under it. */
  gap?: string;
  children?: React.ReactNode;
}) {
  return (
    <div style={{
      flex: 'none', height: 'var(--h-head)', display: 'flex', alignItems: 'center',
      gap, padding: `0 ${pad}`,
      borderBottom: '1px solid var(--border)',
      ...captionStyle,
    }}>
      {cols ? cols.map((c, i) => <span key={i} style={cell(c, true)}>{c.head}</span>) : children}
    </div>
  );
}

/**
 * A popover that closes like the others (the account menu, the opponent picker): a press outside
 * it or Escape. `refs` are the popover and the button that opens it (pressing that button
 * toggles it on its own).
 */
export function useDismiss(open: boolean, close: () => void, ...refs: RefObject<HTMLElement | null>[]) {
  const shut = useRef(close);
  useLayoutEffect(() => { shut.current = close; });
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!refs.some((r) => r.current?.contains(e.target as Node))) shut.current();
    };
    const esc = (e: KeyboardEvent) => { if (e.key === 'Escape') { e.stopPropagation(); shut.current(); } };
    window.addEventListener('mousedown', away);
    window.addEventListener('keydown', esc, true);
    return () => { window.removeEventListener('mousedown', away); window.removeEventListener('keydown', esc, true); };
    // The refs are stable objects; only opening and closing changes what to listen for.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
}

/** A control with its name above it (forms in a narrow box: login, server editing). */
export function Field({ label, help, children }: { label: string; help?: string; children: React.ReactNode }) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
      {label}
      {children}
      {help && <span>{help}</span>}
    </label>
  );
}

/** How long a short word after an action ("Copied", "Applied") stays. */
export const FLASH_MS = 2500;
export type Flash = { text: string; bad: boolean } | null;

/** A short word after an action, shown for a moment: done in a quiet colour, failed in bad. */
export function useFlash(): [Flash, (text: string, bad?: boolean) => void] {
  const [note, setNote] = useState<Flash>(null);
  const timer = useRef<number | undefined>(undefined);
  const flash = useCallback((text: string, bad = false) => {
    setNote({ text, bad });
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setNote(null), FLASH_MS);
  }, []);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return [note, flash];
}

export function FlashNote({ note }: { note: Flash }) {
  return (
    <span style={{ minWidth: 0, fontSize: 'var(--fs-6)', color: note?.bad ? 'var(--bad)' : 'var(--sub)',
                   overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{note?.text}</span>
  );
}

/** The box of every menu or picker floating over the screen (account menu, opponent picker, ...). */
export const popoverStyle: React.CSSProperties = {
  display: 'flex', flexDirection: 'column', border: '1px solid var(--border)', borderRadius: 'var(--r-4)',
  background: 'var(--bg)', boxShadow: 'var(--sh-2)', overflow: 'hidden',
};

/** The small heading over a group: tiny, bold, spaced and quiet. Section, table and list heads use it. */
export const captionStyle: React.CSSProperties = {
  fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)', whiteSpace: 'nowrap',
};
export function Caption({ children }: { children: React.ReactNode }) {
  return <span style={captionStyle}>{children}</span>;
}

/** The 2px accent bar on a row's left: a mark on its own ("current", "for you"), or part of `picked`. */
export const marked = (on: boolean): React.CSSProperties => ({
  boxShadow: on ? 'inset 2px 0 0 var(--accent)' : 'none',
});

/** The one look of a chosen row in every list: the card ground and the accent bar. */
export const picked = (on: boolean): React.CSSProperties => ({
  background: on ? 'var(--card)' : 'transparent',
  ...marked(on),
});

/**
 * A row you choose in a list: one line (`field`/`row`) or a title over a note (`row2`, with
 * `TwoLines`). It carries the chosen look, the hover, and the arrow keys (main.tsx).
 */
export function ListRow({ on = false, height = 'field', rule = true, onClick, title, children, style }: {
  on?: boolean;
  height?: 'field' | 'row' | 'row2';
  /** A hairline under the row; off where the rows sit on their own (setting categories). */
  rule?: boolean;
  onClick: () => void;
  title?: string;
  children: React.ReactNode;
  style?: React.CSSProperties;
}) {
  return (
    <button type="button" onClick={onClick} title={title} aria-current={on || undefined}
            className={'k-row' + (on ? ' k-on' : '')}
            style={{
              flex: 'none', width: '100%', height: `var(--h-${height})`, padding: '0 var(--sp-3)',
              display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', border: 0, textAlign: 'left',
              borderBottom: rule ? '1px solid var(--border-weak)' : 0, color: 'var(--text)',
              fontSize: 'var(--fs-5)', whiteSpace: 'nowrap', ...picked(on), ...style,
            }}>
      {children}
    </button>
  );
}

/** The two lines of a `row2` row: a title, and a quieter note under it. Both cut with an ellipsis. */
export function TwoLines({ top, bottom }: { top: React.ReactNode; bottom?: React.ReactNode }) {
  return (
    <span style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', lineHeight: 1.3 }}>
      <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)', minWidth: 0, overflow: 'hidden' }}>{top}</span>
      {bottom != null && (
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', overflow: 'hidden', textOverflow: 'ellipsis' }}>{bottom}</span>
      )}
    </span>
  );
}

export function TableRow({ cols, on, pad = 'var(--sp-3)', fs = 'var(--fs-5)', muted, onClick, innerRef, children }: {
  cols: Col[];
  on?: boolean;
  pad?: string;
  fs?: string;
  muted?: boolean;
  onClick?: () => void;
  innerRef?: React.Ref<HTMLButtonElement>;
  children: React.ReactNode;
}) {
  return (
    <button type="button" onClick={onClick} ref={innerRef}
      aria-current={on || undefined}
      className={'k-row' + (on ? ' k-on' : '')}
      style={{
        width: '100%', border: 0, textAlign: 'left',
        display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
        height: 'var(--h-row)', padding: `0 ${pad}`, fontSize: fs,
        fontVariantNumeric: 'tabular-nums',
        borderBottom: '1px solid var(--border-weak)',
        color: muted ? 'var(--sub)' : 'var(--text)',
        ...picked(!!on),
      }}>
      {React.Children.toArray(children).map((ch, i) => (
        <span key={i} style={cell(cols[i] ?? {})}>{ch}</span>
      ))}
    </button>
  );
}

export function Empty({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ padding: 'var(--sp-3) 0', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
      {children}
    </div>
  );
}

export function EmptyState({ title, body, actions, visual }: {
  title: string; body?: React.ReactNode; actions?: React.ReactNode; visual?: React.ReactNode;
}) {
  const side = !!visual;
  return (
    <div style={{ flex: 1, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)' }}>
      <div style={{
        maxWidth: side ? undefined : 420,
        textAlign: side ? 'left' : 'center',
        display: 'flex',
        flexDirection: side ? 'row' : 'column',
        alignItems: 'center',
        gap: 'var(--sp-5)',
      }}>
        {visual ?? (
          <span aria-hidden style={{ width: 64, height: 64, borderRadius: 'var(--r-4)', overflow: 'hidden', opacity: .5, display: 'block' }}
                dangerouslySetInnerHTML={{ __html: icon }} />
        )}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)', maxWidth: side ? 300 : undefined }}>
          <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{title}</div>
          {body && <div style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)', lineHeight: 1.8, textWrap: 'balance' }}>{body}</div>}
          {side && actions && <div style={{ display: 'flex', gap: 'var(--sp-2)', marginTop: 'var(--sp-2)' }}>{actions}</div>}
        </div>
        {!side && actions && <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>{actions}</div>}
      </div>
    </div>
  );
}

export function EmptyBoard() {
  return (
    <div aria-hidden style={{
      width: 150, flex: 'none', padding: 'var(--sp-1)',
      borderRadius: 'var(--r-4)', background: 'var(--panel)',
    }}>
      <div style={{ borderRadius: 'var(--r-1)', overflow: 'hidden' }}>
        <Board cells={START_CELLS} coords={false} disabled />
      </div>
    </div>
  );
}

const FOCUSABLE =
  'textarea:not(:disabled), input:not(:disabled), button:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])';
const FIRST_STOP = 'textarea:not(:disabled), input:not(:disabled), select:not(:disabled)';

export function Overlay({ onClose, children }: { onClose?: () => void; children: React.ReactNode }) {
  const root = React.useRef<HTMLDivElement>(null);
  React.useEffect(() => {
    if (!onClose) return;
    // Only the topmost one closes (a question asked over the game record viewer).
    const on = (e: KeyboardEvent) => {
      const all = document.querySelectorAll('[data-k-overlay]');
      if (e.key === 'Escape' && all[all.length - 1] === root.current) onClose();
    };
    window.addEventListener('keydown', on);
    return () => window.removeEventListener('keydown', on);
  }, [onClose]);

  const box = React.useRef<HTMLDivElement>(null);
  React.useEffect(() => {
    const el = box.current;
    const back = document.activeElement as HTMLElement | null;
    const items = () => [...(el?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])]
      .filter((x) => x.offsetParent !== null);
    (el?.querySelector<HTMLElement>(FIRST_STOP) ?? items()[0] ?? el)?.focus();
    const on = (e: KeyboardEvent) => {
      if (e.key !== 'Tab' || !el) return;
      const list = items();
      if (!list.length) return;
      const a = document.activeElement;
      const out = !el.contains(a);
      if (e.shiftKey ? (out || a === list[0]) : (out || a === list[list.length - 1])) {
        e.preventDefault();
        (e.shiftKey ? list[list.length - 1] : list[0]).focus();
      }
    };
    window.addEventListener('keydown', on);
    return () => { window.removeEventListener('keydown', on); back?.focus?.(); };
  }, []);

  const pressedScrim = React.useRef(false);

  return (
    <div ref={root} data-k-overlay=""
      onMouseDown={(e) => { pressedScrim.current = e.target === e.currentTarget; }}
      onMouseUp={(e) => {
        const upOnScrim = e.target === e.currentTarget;
        if (pressedScrim.current && upOnScrim) onClose?.();
        pressedScrim.current = false;
      }}
      style={{
        position: 'absolute', inset: 0, zIndex: 30,
        background: 'var(--scrim)', display: 'grid', placeItems: 'center',
        padding: 'var(--sp-5)',
      }}>
      <div ref={box} tabIndex={-1}>{children}</div>
    </div>
  );
}

/**
 * The details of a picked row, slid in from the right over the screen (players, results). It is there
 * only while something is picked; a click outside it, its close button or Esc closes it. The click
 * outside still does what it was for (another row opens its own details, Nav changes screen), and a
 * dialog opened from the drawer (a request, a game record) is not outside it.
 */
export function Drawer({ title, tools, label, onClose, children }: {
  title: React.ReactNode; tools?: React.ReactNode; label: string;
  onClose: () => void; children: React.ReactNode;
}) {
  const box = useRef<HTMLElement>(null);
  const shut = useRef(onClose);
  useLayoutEffect(() => { shut.current = onClose; });
  useEffect(() => {
    const overlay = () => document.querySelector('[data-k-overlay]') !== null;
    const key = (e: KeyboardEvent) => { if (e.key === 'Escape' && !overlay()) shut.current(); };
    const press = (e: MouseEvent) => {
      const at = e.target as Element | null;
      if (overlay() || !at || box.current?.contains(at) || at.closest('[data-k-toasts]')) return;
      shut.current();
    };
    window.addEventListener('keydown', key);
    window.addEventListener('mousedown', press, true);
    return () => { window.removeEventListener('keydown', key); window.removeEventListener('mousedown', press, true); };
  }, []);
  // Placed against the window, so the screen that opens it can sit anywhere inside the frame.
  const edge = { position: 'fixed', top: 'var(--h-window)', bottom: 'var(--h-status)' } as const;
  return (
    <aside ref={box} role="dialog" aria-label={label} style={{
      ...edge, right: 0, zIndex: 26, width: 'var(--w-drawer)', maxWidth: 'calc(100% - 120px)',
      display: 'flex', flexDirection: 'column', background: 'var(--panel)',
      borderLeft: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
    }}>
      <div style={{
        flex: 'none', height: 'var(--h-bar)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
        padding: '0 var(--sp-2) 0 var(--sp-3)', borderBottom: '1px solid var(--border)', whiteSpace: 'nowrap',
      }}>
        <div style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                      fontSize: 'var(--fs-3)', fontWeight: 600, overflow: 'hidden' }}>{title}</div>
        {tools}
        <IconButton name="close" label={t('ui.close')} onClick={onClose} />
      </div>
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>{children}</div>
    </aside>
  );
}

/** The yes/no question before a step that cannot be taken back (resign, a new game, a delete). */
export function Confirm({ title, body, ok, danger, onOk, onCancel }: {
  title: string; body?: React.ReactNode; ok: string; danger?: boolean;
  onOk: () => void; onCancel: () => void;
}) {
  return (
    <Overlay onClose={onCancel}>
      <Modal title={title} body={body} onClose={onCancel} actions={<>
        <span style={{ marginLeft: 'auto' }} />
        <Button size="field" onClick={onCancel}>{t('dialog.cancel')}</Button>
        <Button size="field" variant={danger ? 'danger' : 'primary'} onClick={onOk}>{ok}</Button>
      </>} />
    </Overlay>
  );
}
