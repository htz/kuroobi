import React from 'react';
import { Board, BoardDefs } from './board';
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

export function Toolbar({ children, aux, dock, graph }: {
  children: React.ReactNode;
  aux?: React.ReactNode;
  dock?: { open: boolean; onToggle: () => void; label?: string };
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
      {aux && <span className="k-toolbar-aux" style={{ alignItems: 'center', gap: 'var(--sp-3)' }}>{aux}</span>}
      {graph && (
        <span className="k-graph-toggle">
          <Button size="chip" variant={graph.open ? 'secondary' : 'ghost'}
                  title={t('ui.toolbar.graph_title')} onClick={graph.onToggle}>{t('ui.toolbar.graph')}</Button>
        </span>
      )}
      {dock && (
        <span className="k-dock-toggle">
          <IconButton name="panel" label={dock.label ?? t(dock.open ? 'ui.panel.close' : 'ui.panel.open')}
                      onClick={dock.onToggle} />
        </span>
      )}
    </div>
  );
}

export function Dock({ tabs, active, onTab, children, open, scroll = true }: {
  tabs: string[]; active: string; onTab?: (t: string) => void; children: React.ReactNode;
  open?: boolean;
  scroll?: boolean;
}) {
  return (
    <aside className={'k-dock' + (open ? ' k-open' : '')} style={{
      flex: 'none', background: 'var(--panel)',
      borderLeft: '1px solid var(--border)', flexDirection: 'column', minHeight: 0,
    }}>
      <div style={{ padding: 'var(--sp-2)', flex: 'none' }}>
        <Segmented fill value={active} onChange={onTab}
                   options={tabs.map(name => ({ value: name, label: name }))} />
      </div>
      <div className={scroll ? 'k-scroll' : undefined} style={{
        flex: 1, minHeight: 0, display: scroll ? undefined : 'flex', flexDirection: 'column',
      }}>{children}</div>
    </aside>
  );
}

export function Section({ title, aside, grow, children }: {
  title: string; aside?: React.ReactNode; children?: React.ReactNode;
  grow?: boolean;
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
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)',
        borderBottom: '1px solid var(--border)',
      }}>{title}{aside && <span style={{
        marginLeft: 'auto', letterSpacing: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
      }}>{aside}</span>}</h3>
      {grow
        ? <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>{children}</div>
        : children}
    </section>
  );
}

export function StatusBar({ left, right }: { left?: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div style={{
      height: 'var(--h-status)', flex: 'none', background: 'var(--bg)', borderTop: '1px solid var(--border)',
      display: 'flex', alignItems: 'center', gap: 'var(--sp-4)', padding: '0 var(--sp-4)',
      fontSize: 'var(--fs-6)', color: 'var(--sub)', whiteSpace: 'nowrap',
      fontVariantNumeric: 'tabular-nums',
    }}>
      {left}
      <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>{right}</span>
    </div>
  );
}

export function StatusStat({ label, value, unit }: { label?: string; value: React.ReactNode; unit?: string }) {
  return (
    <span>{label && label + ' '}<b style={{ color: 'var(--text)', fontWeight: 600 }}>{value}</b>{unit && ' ' + unit}</span>
  );
}

export function BottomPanel({ tabs, active, onTab, onClose, height = 240, children }: {
  tabs: { id: string; label: string; unread?: number }[];
  active: string; onTab?: (id: string) => void; onClose?: () => void;
  height?: number; children: React.ReactNode;
}) {
  return (
    <div style={{
      flex: 'none', height: Math.max(180, Math.min(420, height)),
      background: 'var(--panel)', borderTop: '1px solid var(--border)',
      display: 'flex', flexDirection: 'column',
    }}>
      <div style={{
        height: 'var(--h-field)', flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
        padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
      }}>
        {tabs.map(tab => {
          const on = active === tab.id;
          return (
            <button key={tab.id} type="button" onClick={() => onTab?.(tab.id)}
              aria-pressed={on} className={'k-press' + (on ? ' k-on' : '')}
              style={{
                height: 'var(--h-chip)', padding: '0 var(--sp-3)', border: 0, borderRadius: 'var(--r-1)', fontSize: 'var(--fs-6)',
                background: on ? 'var(--card)' : 'transparent',
                color: on ? 'var(--text)' : 'var(--sub)', fontWeight: on ? 600 : 400,
                display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
              }}>
              {tab.label}
              {tab.unread ? <span style={{ background: 'var(--bad)', color: 'var(--on-bad)', borderRadius: 'var(--r-pill)', padding: '0 var(--sp-1)', fontSize: 'var(--fs-7)' }}>{tab.unread}</span> : null}
            </button>
          );
        })}
        {onClose && (
          <span style={{ marginLeft: 'auto' }}>
            <IconButton name="close" label={t('ui.panel.close')} onClick={onClose} size={14} />
          </span>
        )}
      </div>
      <div style={{ flex: 1, display: 'flex', minHeight: 0 }}>{children}</div>
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
  num?: boolean;
  clip?: boolean;
}

function cell(c: Col, head?: boolean): React.CSSProperties {
  if (!head && c.clip) {
    return {
      ...(c.w === undefined ? { flex: 1, minWidth: 0 } : { width: c.w, flex: 'none' }),
      overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
      textAlign: c.right ? 'right' : undefined,
    };
  }
  return {
    ...(c.w === undefined ? { flex: 1, minWidth: 0 } : { width: c.w, flex: 'none' }),
    display: 'flex', alignItems: 'center', gap: 'var(--sp-0)',
    justifyContent: c.right ? 'flex-end' : 'flex-start',
    whiteSpace: 'nowrap',
    ...(!head && c.num ? { fontVariantNumeric: 'tabular-nums' } : null),
  };
}

export function TableHead({ cols, pad = 'var(--sp-3)', children }: {
  cols?: Col[];
  pad?: string;
  children?: React.ReactNode;
}) {
  return (
    <div style={{
      flex: 'none', height: 'var(--h-head)', display: 'flex', alignItems: 'center',
      gap: 'var(--sp-2)', padding: `0 ${pad}`,
      borderBottom: '1px solid var(--border)',
      fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)',
    }}>
      {cols ? cols.map((c, i) => <span key={i} style={cell(c, true)}>{c.head}</span>) : children}
    </div>
  );
}

export const picked = (on: boolean): React.CSSProperties => ({
  background: on ? 'color-mix(in srgb, var(--accent) 14%, transparent)' : 'transparent',
  boxShadow: on ? 'inset 2px 0 0 var(--accent)' : 'none',
});

export function TableRow({ cols, on, pad = 'var(--sp-3)', fs = 'var(--fs-5)', muted, onClick, title, innerRef, children }: {
  cols?: Col[];
  on?: boolean;
  pad?: string;
  fs?: string;
  muted?: boolean;
  onClick?: () => void;
  title?: string;
  innerRef?: React.Ref<HTMLButtonElement>;
  children: React.ReactNode;
}) {
  return (
    <button type="button" onClick={onClick} title={title} ref={innerRef}
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
      {cols
        ? React.Children.toArray(children).map((ch, i) => (
            <span key={i} style={cell(cols[i] ?? {})}>{ch}</span>
          ))
        : children}
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
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)', maxWidth: side ? 250 : undefined }}>
          <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{title}</div>
          {body && <div style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)', lineHeight: 1.8 }}>{body}</div>}
          {side && actions && <div style={{ display: 'flex', gap: 'var(--sp-2)', marginTop: 'var(--sp-2)' }}>{actions}</div>}
        </div>
        {!side && actions && <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>{actions}</div>}
      </div>
    </div>
  );
}

export function EmptyBoard({ size = 150 }: { size?: number }) {
  const cells: (0 | 1 | 2)[] = Array(64).fill(0);
  cells[3 * 8 + 3] = 2; cells[4 * 8 + 4] = 2;
  cells[4 * 8 + 3] = 1; cells[3 * 8 + 4] = 1;
  return (
    <div aria-hidden style={{
      width: size, flex: 'none', padding: 'var(--sp-1)',
      borderRadius: 'var(--r-4)', background: 'var(--panel)',
    }}>
      <div style={{ borderRadius: 'var(--r-1)', overflow: 'hidden' }}>
        <Board cells={cells} coords={false} disabled />
      </div>
    </div>
  );
}

const FOCUSABLE =
  'textarea:not(:disabled), input:not(:disabled), button:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])';
const FIRST_STOP = 'textarea:not(:disabled), input:not(:disabled), select:not(:disabled)';

export function Overlay({ onClose, children }: { onClose?: () => void; children: React.ReactNode }) {
  React.useEffect(() => {
    if (!onClose) return;
    const on = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
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
    <div
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
