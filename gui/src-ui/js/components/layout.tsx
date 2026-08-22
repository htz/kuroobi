import React from 'react';
import { Board, BoardDefs } from './board';
import { Button, Dot, Segmented } from './primitives';
import { IconButton } from './Icons';
import { t } from '../i18n';
// Assets are the single source (assets.d.ts policy): read contents
// instead of <img src> so no screen-side copy can drift.
import icon from '../../assets/icon.svg?raw';

/* KUROOBI layout
 * The screen is the left nav (ggs.tsx's Nav) plus three fixed-height
 * rows (Toolbar / body / StatusBar); growth never moves the frame.
 * No TitleBar — destinations live in the nav, and a second tab row
 * would split navigation. Traffic lights and the drag region belong
 * to the nav's top edge (Tauri, titleBarStyle: "Overlay").
 *
 * Pick-one-of-few rows all use one Segmented (Dock and BottomPanel
 * alike). Selection floats on --card; the --accent-dim fill is
 * reserved for the nav's current location, and a second blue patch
 * would dilute "where am I".
 */

/* Toasts anchor bottom-right against this element; dropping the
 * relative sends them to the top-left.
 *
 * The tatami <pattern> is drawn here too: the document needs exactly
 * one, and there is exactly one AppFrame. A missing pattern fails
 * silently — url(#kb-tatami) resolves to nothing and the boards turn
 * flat green — so it lives where it cannot be forgotten. */
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

/* Window band, full width, 28px, with nothing clickable — that is
 * the layer boundary (rule 75); clickables live in the screen
 * (Toolbar / Nav / body).
 *
 * Traffic-light margin and logo on the left, then a passive "what am
 * I looking at" (macOS title role) — legible in Mission Control and
 * screenshots. Tauri's title is blanked; we draw it here. */
export function WindowBar({ title, sub, right }: {
  title: string;
  sub?: React.ReactNode;
  /** Passive badge at the right edge, never clickable (rule 75).
   *  Currently only the env-var debug tag uses it. */
  right?: React.ReactNode;
}) {
  return (
    /* Equal traffic-light margins on both sides keep the title at the
       window's center; left-aligned it reads as a screen heading. The
       logo stays out — the nav's top edge has more room than a 28px
       band. */
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
      {/* The right-edge badge overlays absolutely so the title stays
          centered; in flow it would push the title left. */}
      {right && (
        <span style={{
          position: 'absolute', right: 'var(--sp-3)', top: 0,
          height: 'var(--h-window)', display: 'flex', alignItems: 'center', gap: 'var(--sp-1)',
        }}>{right}</span>
      )}
    </div>
  );
}

/* Middle band between WindowBar and StatusBar: Nav / screen / Dock in
 * a row. The overlaid Dock (k-open) anchors here so it never slides
 * under the full-width bands. */
export function Body({ children }: { children: React.ReactNode }) {
  return <div style={{ position: 'relative', flex: 1, display: 'flex', minHeight: 0 }}>{children}</div>;
}

/* Right of the nav: Toolbar then body, stacked — the screen itself. */
export function Main({ children, inset }: {
  children: React.ReactNode;
  /** Shrink the body by the overlaid Dock's width (collapsed tier only). */
  inset?: boolean;
}) {
  return (
    // k-main had no rules (no .k-main in base.css); dead classes trip
    // every audit.
    <div className={inset ? 'k-dock-inset' : undefined}
         style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>
      {children}
    </div>
  );
}

/* The band above the board: the mode's actions plus the minimum
 * game-setup choices. Numbers go to the StatusBar.
 *
 * aux disappears below 940px (base.css k-toolbar-aux) — put nothing
 * there that must survive; anything that must stay editable in a
 * narrow window goes in children. The split is typed because
 * per-screen manual classes always get forgotten.
 * dock toggles the Dock in the collapsed tier (<=1120px); the Dock
 * sits right of the body, so its entry point lives up here (mirror of
 * BottomPanel's onClose). graph likewise toggles the eval graph in
 * the lowest tier (<=620px), whose heading row collapses away. */
export function Toolbar({ children, aux, dock, graph }: {
  children: React.ReactNode;
  aux?: React.ReactNode;
  dock?: { open: boolean; onToggle: () => void; label?: string };
  graph?: { open: boolean; onToggle: () => void };
}) {
  return (
    /* Also a window-drag strip: the nav's top edge alone gives only
       208px, and not over the logo (the attribute only works on the
       element itself). Only the band's ground is draggable — child
       buttons lack the attribute, so clicks never move the window. */
    /* The screen's band, right of the nav, above the body — this
       screen's actions only. No traffic-light margin, no logo; the
       WindowBar owns those (rule 75). */
    <div style={{
      height: 'var(--h-bar)', flex: 'none', borderBottom: '1px solid var(--border-weak)',
      display: 'flex', alignItems: 'center', padding: '0 var(--sp-4)', gap: 'var(--sp-2)',
      /* Contain the band: without this the controls kept their width in
         a narrow window and painted over the dock beside them. Clipped
         at the edge they are at least cut off inside the band, and the
         groups below shrink before that happens. */
      overflow: 'hidden', minWidth: 0,
    }}>
      {/* The action group shrinks (and clips) rather than pushing the
          rest out of the band. */}
      <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                     minWidth: 0, overflow: 'hidden', flexShrink: 1 }}>{children}</span>
      {/* This spacer owns the push; putting marginLeft:auto on the
          disappearing aux makes everything jump left when it goes. */}
      <span style={{ flex: 1 }} />
      {/* display lives in .k-toolbar-aux (hidden at 940px). */}
      {aux && <span className="k-toolbar-aux" style={{ alignItems: 'center', gap: 'var(--sp-3)' }}>{aux}</span>}
      {graph && (
        /* Text button: next to the dock toggle, two icon squares would
           be indistinguishable. */
        <span className="k-graph-toggle">
          <Button size="chip" variant={graph.open ? 'secondary' : 'ghost'}
                  title={t('ui.toolbar.graph_title')} onClick={graph.onToggle}>{t('ui.toolbar.graph')}</Button>
        </span>
      )}
      {dock && (
        <span className="k-dock-toggle">
          {/* panel = vertically split rectangle; reusing ggs-console's
              icon would give one glyph two meanings. */}
          <IconButton name="panel" label={dock.label ?? t(dock.open ? 'ui.panel.close' : 'ui.panel.open')}
                      onClick={dock.onToggle} />
        </span>
      )}
    </div>
  );
}

export function Dock({ tabs, active, onTab, children, open, scroll = true }: {
  tabs: string[]; active: string; onTab?: (t: string) => void; children: React.ReactNode;
  /** Toggled in the collapsed tier (<=1120px), ignored when wide.
   *  The opener lives in Toolbar's dock={{...}}. */
  open?: boolean;
  /** Scroll the whole content. false for content pinning its own
   *  header (the move table scrolls rows only). */
  scroll?: boolean;
}) {
  return (
    // width/display live in base.css's .k-dock (hidden at 1120px —
    // inline styles escape the media query). k-open overlays it.
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

/* Section: no rounded boxes, just a heading and a 1px rule. Content
 * is optional — the section itself marks "new topic", so scrolling
 * containers (protocol log) may hold just the heading. */
export function Section({ title, aside, grow, children }: {
  title: string; aside?: React.ReactNode; children?: React.ReactNode;
  /** Take the remaining height and scroll only the content. */
  grow?: boolean;
}) {
  return (
    <section style={{
      display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
      padding: '0 var(--sp-3) var(--sp-4)',
      ...(grow ? { flex: 1, minHeight: 0, overflow: 'hidden' } : { flex: 'none' }),
    }}>
      {/* Taller band when actions sit on it; at --h-head (20px) the
          controls sink into the rule with a text-sized hit area. */}
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

/* Right-edge content varies by mode; chat/console only during a GGS
 * game. */
export function StatusBar({ left, right }: { left?: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div style={{
      height: 'var(--h-status)', flex: 'none', background: 'var(--bg)', borderTop: '1px solid var(--border)',
      display: 'flex', alignItems: 'center', gap: 'var(--sp-4)', padding: '0 var(--sp-4)',
      fontSize: 'var(--fs-6)', color: 'var(--sub)', whiteSpace: 'nowrap',
      // Tabular digits for the whole band: think time / nodes / nps
      // change every frame, and shifting digits shake the strip.
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

/* GGS-only bottom panel; chat and console share one sheet.
 *
 * Height is fixed (a constant within 180-420). A grip that looked
 * draggable but wasn't was worse than none, so it went; to add
 * resizing, restore onResize and the k-grip handle.
 *
 * Tabs carry unread badges so they are not Segmented, but selection
 * styling matches (--card). */
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
      {/* Tab strip is --h-field (32px); 36 is not on rule 5's scale
          and the design uses 32. Chips are 20px, leaving 6px each way. */}
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


/* Floating things. Only Modal / Toast / the board frame get --r-4
 * radius and a shadow (Popover was unused and removed — rule 70).
 *
 * There is exactly one modal shape. Four dialogs used to have four
 * builds, so the same "float and decide" thing looked different per
 * screen.
 *
 * Three tiers with three ground colors (measured from the settings
 * window — title #1f2328 / tabs #2c3138 / body #17191d):
 *   head - --h-bar (44px) of --panel; centered title, close at right
 *   body - --bg (darkest); padding 16/24; scrolls when long
 *   foot - same --panel + top rule; only when actions are passed
 *
 * Never brighten the body: brighter than the bands, the bands sink
 * into box edges and the modal reads as a plate, not a sheet.
 *
 * Width is --w-modal (340) or --w-modal-wide (520). Height follows
 * content, capped at 88vh with only the body scrolling. */
export function Modal({ title, sub, body, actions, width = 'var(--w-modal)', onClose, scroll, band, children }: {
  title: string;
  /** One line under the title (e.g. the record loader's format hint). */
  sub?: React.ReactNode;
  /** Body; same as children, but reads better for short text. */
  body?: React.ReactNode;
  actions?: React.ReactNode;
  /** Default --w-modal (340px); content-heavy modals use wide (rule 71). */
  width?: string;
  /** Close; when passed, a button appears at the head's right edge. */
  onClose?: () => void;
  /** Make the body scrollable (long content like settings). */
  scroll?: boolean;
  /** A strip pinned under the head (settings tabs) — never scrolled
   *  with the body, or the tabs drift away. */
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
        {/* Match the close button's width on the left to center the
            title. */}
        <span style={{ width: 32, flex: 'none' }} />
        <div style={{ flex: 1, minWidth: 0, textAlign: 'center' }}>
          {/* Titles can carry names (player cards); keep selectable. */}
          <div className="k-sel" style={{
            fontSize: 'var(--fs-4)', fontWeight: 600, color: 'var(--text)',
            overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{title}</div>
          {/* Subtitle inside the head, under the title (design §9); in
              the body it blends with the first content line. */}
          {sub && <div style={{
            fontSize: 'var(--fs-7)', color: 'var(--sub)',
            overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{sub}</div>}
        </div>
        <span style={{ width: 32, flex: 'none', display: 'grid', placeItems: 'center' }}>
          {onClose && <IconButton name="close" label={t('ui.close')} onClick={onClose} />}
        </span>
      </div>

      {/* The strip's container is owned here — height, ground and rule
          are modal law, and content-side copies drift. */}
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

/* Lists inside a section — never put rows directly under Section.
 *
 * Section stacks content with a 12px gap; fine for prose and fields,
 * but 12px between list rows reads as bullet points. The same slip
 * happened four times, hence a named container: always wrap rows in
 * this. */
export function List({ children }: { children: React.ReactNode }) {
  return <div style={{ display: 'flex', flexDirection: 'column' }}>{children}</div>;
}

/** Machine-at-work indicator: Dot + one word, --accent. Four places
 *  hand-rolled the same shape. Pairs with rule 34 (no progress
 *  toasts): progress is shown quietly in place. */
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

/** Label-left value-right row — the name: value shape inside a
 *  section. Merged from App.tsx's LearnStat and LearnLog's Fact
 *  (rule 50). big highlights a number; default is body-sized with
 *  ellipsis for long values. */
export function KeyValue({ label, value, big }: {
  label: string;
  /** Numbers get digit grouping (70,622); undefined renders as em
   *  dash. */
  value: React.ReactNode;
  /** Bold --fs-3 tabular value; only for number-reading rows. */
  big?: boolean;
}) {
  // Format here: caller-side toLocaleString() gets forgotten and the
  // book's position count exceeds five digits.
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

/** Toolbar vertical rule separating actions from setup choices —
 *  without it "new game" and "black" read as one row. Merged from
 *  four identical copies in App.tsx. */
export function Divider() {
  return <span style={{
    width: 1, height: 20, flex: 'none',
    margin: '0 var(--sp-1)', background: 'var(--border)',
  }} />;
}

/** Explanatory prose for sections and fields. Rule 73: prose wraps
 *  at --w-text (720px) — a 1500px line loses the eye on the way back.
 *  Seven places hand-rolled this. */
export function Note({ children }: { children: React.ReactNode }) {
  return <p style={{
    margin: 0, maxWidth: 'var(--w-text)',
    fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.8,
  }}>{children}</p>;
}

/* Table column spec: width and alignment come from ONE array shared
 * by head and rows. Hand-written widths in two places drifted. Cell
 * content stays as children — the array owns the container, the
 * caller owns the content. */
export interface Col {
  /** Header label. */
  head?: React.ReactNode;
  /** Width in px; columns without one share the remainder. */
  w?: number;
  /** Right-align, for numeric columns. */
  right?: boolean;
  /** Tabular digits (columns compared vertically). Rows only. */
  num?: boolean;
  /** Ellipsis overflow (name columns). Rows only. */
  clip?: boolean;
}

/** Column container; head mode drops the row-only bits (num/clip). */
function cell(c: Col, head?: boolean): React.CSSProperties {
  if (!head && c.clip) {
    // Clipping columns leave flex; textOverflow fails on flex children.
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
    // Rows are a fixed --h-row tall, so a wrap does not grow the row —
    // it spills over the neighbours. A column too narrow for its
    // longest translation must be widened, never wrapped.
    whiteSpace: 'nowrap',
    ...(!head && c.num ? { fontVariantNumeric: 'tabular-nums' } : null),
  };
}

/* Table header row — four screens hand-rolled it identically. Height
 * --h-head (20px), type matches section headings. Columns come from
 * cols; passing the same array to rows keeps widths written once. */
export function TableHead({ cols, pad = 'var(--sp-3)', children }: {
  /** Column spec — the SAME array passed to the rows. */
  cols?: Col[];
  /** Side padding, matching the rows (default --sp-3). */
  pad?: string;
  /** Only for tables without a columnar header. */
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

/** Selected-row treatment: 14% accent fill + 2px left bar. Lists come
 *  in three shapes but selection has exactly one look; hand-written
 *  values drifted once (LearnLog used --card + radius). */
export const picked = (on: boolean): React.CSSProperties => ({
  background: on ? 'color-mix(in srgb, var(--accent) 14%, transparent)' : 'transparent',
  boxShadow: on ? 'inset 2px 0 0 var(--accent)' : 'none',
});

/* Table row — hand-rolled in three places before. Height --h-row
 * (24px), 1px bottom rule, selection = 14% accent + 2px left bar (a
 * look that kept drifting until it got a named container).
 *
 * Columns come from cols (same array as TableHead); children follow
 * the column order — the array owns widths, children own content. */
export function TableRow({ cols, on, pad = 'var(--sp-3)', fs = 'var(--fs-5)', muted, onClick, title, innerRef, children }: {
  /** Column spec — the SAME array passed to the head. */
  cols?: Col[];
  /** Selected row. */
  on?: boolean;
  /** Side padding, matching the header. */
  pad?: string;
  /** Font size; --fs-6 for narrow columns. */
  fs?: string;
  /** Dim rows with no value yet (unplayed moves). */
  muted?: boolean;
  onClick?: () => void;
  title?: string;
  /** Ref for following the current row (used by the move table). */
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
        // Table rows always carry numeric columns; digits must align.
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

/* One-liner for an empty list — the small variant, placed inside a
 * section or table. A fully empty screen uses EmptyState (the big one
 * with art, title and a destination). Use only these two: raw spans
 * drifted into four different looks. */
export function Empty({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ padding: 'var(--sp-3) 0', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
      {children}
    </div>
  );
}

/* Empty state that avoids showing a dead board (e.g. GGS before any
 * game). With visual it goes horizontal (design's GGS empty state):
 * visual left, text and button right. Without it, the vertical
 * logo-on-top stack. Centered only when vertical — short horizontal
 * lines left-align, or the button floats mid-sentence. */
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
          {/* Horizontal keeps the button in the text column; vertical
              centers it outside. */}
          {side && actions && <div style={{ display: 'flex', gap: 'var(--sp-2)', marginTop: 'var(--sp-2)' }}>{actions}</div>}
        </div>
        {!side && actions && <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>{actions}</div>}
      </div>
    </div>
  );
}

/** Decorative board for empty states: initial position, not
 *  interactive. Reuses Board — a separate board would double-manage
 *  the tatami and stone rendering. */
export function EmptyBoard({ size = 150 }: { size?: number }) {
  // sq = file*8 + rank; d4/e5 white, e4/d5 black.
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

/* The scrim that centers floating things; Modals go inside. Its job
 * is making the rest unclickable, so clicking it closes — as does
 * Esc, which browser confirm() gave for free and we must write. */
/** Focusable targets (members of the Tab ring). */
const FOCUSABLE =
  'textarea:not(:disabled), input:not(:disabled), button:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])';
/** Preferred initial focus: inputs before buttons — querySelectorAll
 *  returns document order, which puts the head's close button first
 *  (the record loader focused "close" instead of the paste field). */
const FIRST_STOP = 'textarea:not(:disabled), input:not(:disabled), select:not(:disabled)';

export function Overlay({ onClose, children }: { onClose?: () => void; children: React.ReactNode }) {
  React.useEffect(() => {
    if (!onClose) return;
    const on = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', on);
    return () => window.removeEventListener('keydown', on);
  }, [onClose]);

  /* Move focus in, trap it, restore on close. We claimed aria-modal
     while Tab still walked the unclickable things under the scrim.
     Initial target is the first focusable (the paste field for the
     record loader), else the container. */
  const box = React.useRef<HTMLDivElement>(null);
  React.useEffect(() => {
    const el = box.current;
    const back = document.activeElement as HTMLElement | null;
    /** Only currently clickable ones; skip offsetParent-less
     *  (collapsed) elements. */
    const items = () => [...(el?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])]
      .filter((x) => x.offsetParent !== null);
    (el?.querySelector<HTMLElement>(FIRST_STOP) ?? items()[0] ?? el)?.focus();
    // Wrap at the ends; without the ring, Tab escapes under the scrim.
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

  return (
    <div onClick={onClose} style={{
      position: 'absolute', inset: 0, zIndex: 30,
      background: 'var(--scrim)', display: 'grid', placeItems: 'center',
      padding: 'var(--sp-5)',
    }}>
      {/* Clicks on the content do not close. */}
      <div ref={box} tabIndex={-1} onClick={(e) => e.stopPropagation()}>{children}</div>
    </div>
  );
}
