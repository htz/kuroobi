import React from 'react';
/* IconButton comes from Icons.tsx (the primitives copy was removed).
 * props are {name, label, onClick, size} — label fills both title and
 * aria-label; onClick takes no arguments. */
import { Icon, IconButton, type IconName } from './Icons';
import type { GgsSnapshot } from '../types';
import { Badge, Dot, Button, Select, TextField } from './primitives';
import { Note, picked } from './layout';
import { t } from '../i18n';
import logo from '../../assets/kuroobi.svg?raw';
// Single source of truth; a copy in the design-side state.ts drifted
// (level table, formula variables and colors all diverged once).
import {
  colorChoices, boolOps, FORMULA_OPS, formulaVars, varOf, condToSrc, condLabel,
  isGroup, type Cond as SharedCond, type FormulaOp,
} from '../ggs';

/* GGS-specific components: left nav, resource meters, match list,
 * ratings, formula tree, translated chat bubbles, protocol log,
 * toasts. */

/* ============ Left nav ============ */

export type NavId =
  | 'play' | 'study' | 'book'
  | 'ggs-play' | 'ggs-lobby' | 'ggs-players' | 'ggs-results' | 'ggs-chat' | 'ggs-standby' | 'ggs-console' | 'ggs-settings'
  | 'ggs-login';

export type NavItem = {
  id: NavId;
  label: string;
  icon: IconName;     // IconName from Icons.tsx
  count?: number;    // count badge
  alert?: boolean;   // addressed to me / unread; painted --bad
  dot?: 'ok' | 'gold' | 'bad';  // state badge (waiting mode on, my turn, ...)
  /** Shortcut key, surfaced via title (like the play button's
   *  `title="⌘N"`) — invisible otherwise, and worth more in the
   *  collapsed 48px rail where labels disappear. */
  hint?: string;
};

export type Conn = 'offline' | 'connecting' | 'logging-in' | 'online';

const CONN_TONE: Record<Conn, 'sub' | 'gold' | 'ok'> = {
  offline: 'sub', connecting: 'gold', 'logging-in': 'gold', online: 'ok',
};

export const navLocal = (): NavItem[] => [
  { id: 'play', label: t('ggs.nav.play'), icon: 'play' },
  { id: 'study', label: t('ggs.nav.study'), icon: 'study' },
  { id: 'book', label: t('ggs.book'), icon: 'book', hint: '⌘B' },
];

/* While disconnected, show a single login row instead of all seven. */
export function ggsNav(conn: Conn, badges?: Partial<Record<NavId, Pick<NavItem, 'count' | 'alert' | 'dot'>>>): NavItem[] {
  if (conn !== 'online') return [{ id: 'ggs-login', label: t('ggs.nav.login'), icon: 'login' }];
  const base: NavItem[] = [
    { id: 'ggs-play', label: t('ggs.nav.games'), icon: 'ggs-play' },
    { id: 'ggs-lobby', label: t('ggs.nav.lobby'), icon: 'ggs-lobby' },
    { id: 'ggs-players', label: t('ggs.nav.players'), icon: 'ggs-users' },
    { id: 'ggs-results', label: t('ggs.nav.results'), icon: 'results' },
    { id: 'ggs-chat', label: t('ggs.nav.chat'), icon: 'ggs-chat' },
    { id: 'ggs-standby', label: t('ggs.waiting_mode'), icon: 'ggs-standby' },
    { id: 'ggs-console', label: t('ggs.nav.console'), icon: 'ggs-console' },
    // GGS settings moved to the settings window's GGS tab; two
    // destinations compete for authority (rule 58).
  ];
  return base.map(i => ({ ...i, ...(badges?.[i.id] ?? {}) }));
}

export function Nav({ items, ggsItems, conn, active, onSelect, footer }: {
  items: NavItem[]; ggsItems: NavItem[]; conn: Conn;
  active: NavId; onSelect?: (id: NavId) => void; footer?: React.ReactNode;
}) {
  return (
    // Width lives in base.css's .k-nav (collapses at 1040px; inline
    // styles are out of the media query's reach).
    <nav className="k-nav" style={{
      flex: 'none', background: 'var(--panel)',
      borderRight: '1px solid var(--border)', display: 'flex', flexDirection: 'column', minHeight: 0,
    }}>
      {/* Logo strip. The traffic-light 78px belongs to the window
          band; dropped entirely in the collapsed rail. */}
      <div className="k-nav-logo" style={{ height: 'var(--h-bar)', flex: 'none' }}>
        <span aria-label="KUROOBI" dangerouslySetInnerHTML={{ __html: logo }} />
      </div>
      <div style={{ padding: 'var(--sp-1) var(--sp-2) 0', display: 'flex', flexDirection: 'column', gap: 1 }}>
        {items.map(i => <NavRow key={i.id} item={i} active={active === i.id} onSelect={onSelect} />)}
      </div>
      {/* display lives in .k-nav-section (hidden at 1040px). */}
      {/* Keep the separator when collapsed: labels go, but the 1px
          rule keeps the two destination groups distinct (.k-nav-rule). */}
      <div className="k-nav-rule" />
      <div className="k-nav-section" style={{
        margin: 'var(--sp-4) 0 6px', padding: '0 18px', alignItems: 'center', gap: 'var(--sp-2)',
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.1em', color: 'var(--sub)',
      }}>
        <Dot tone={CONN_TONE[conn]} />GGS
      </div>
      <div style={{ padding: '0 var(--sp-2)', display: 'flex', flexDirection: 'column', gap: 1 }}>
        {ggsItems.map(i => <NavRow key={i.id} item={i} active={active === i.id} onSelect={onSelect} />)}
      </div>
      {/* Padding lives in .k-nav-foot; 12px sides in a 48px rail leave
          24px for a 32px hit target. */}
      {footer && <div className="k-nav-foot" style={{
        marginTop: 'auto', display: 'flex', flexDirection: 'column',
        gap: 'var(--sp-3)', borderTop: '1px solid var(--border)',
      }}>{footer}</div>}
    </nav>
  );
}

function NavRow({ item, active, onSelect }: { item: NavItem; active: boolean; onSelect?: (id: NavId) => void }) {
  return (
    <button type="button" onClick={() => onSelect?.(item.id)} aria-current={active || undefined}
      title={item.hint ? `${item.label} (${item.hint})` : item.label}
      className={'k-nav-row k-row' + (active ? ' k-on' : '')}
      // padding/justify-content live in .k-nav-row (centered at 1040px).
      style={{
        height: 'var(--h-field)', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        border: 0, borderRadius: 'var(--r-2)', fontSize: 'var(--fs-4)',
        background: active ? 'var(--accent-dim)' : 'transparent',
        color: active ? 'var(--on-accent)' : 'var(--text)', fontWeight: active ? 600 : 400,
      }}>
      {/* Never let the icon shrink: 16 + 12 + 18 = 46 overflows the
          32px target and flex crushed the icon to zero, leaving an
          unidentifiable badge. */}
      <span style={{ flex: 'none', display: 'grid', placeItems: 'center' }}>
        <Icon name={item.icon} size={16} />
      </span>
      <span className="k-nav-label">{item.label}</span>
      {/* Position and size live in base.css (the collapsed rail moves
          and shrinks it); only color and weight belong here. */}
      {item.count != null && (
        <span className="k-nav-n" style={{
          fontWeight: item.alert ? 700 : 600, display: 'grid', placeItems: 'center',
          background: item.alert ? 'var(--bad)'
            : active ? 'color-mix(in srgb, var(--on-accent) 22%, transparent)' : 'var(--border)',
          color: item.alert ? 'var(--on-bad)' : active ? 'var(--on-accent)' : 'var(--text)',
        }}>{item.count}</span>
      )}
      {item.dot && <span className="k-nav-dot" style={{
        width: 7, height: 7, borderRadius: '50%', background: 'var(--' + item.dot + ')',
      }} />}
    </button>
  );
}

/* ============ Resource meters ============
 * Local search and GGS games run on separate thread pools; you cannot
 * triage without seeing what is running. */

export function Meter({ icon, label, value, unit, ratio, note }: {
  icon: IconName; label: string; value: React.ReactNode; unit?: string; ratio: number; note?: string;
}) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
      {/* The rail keeps only the icon and the 4px bar; text drops at
          1040px. The icon sits outside that wrapper or it vanishes
          with the text, leaving two anonymous bars. */}
      <div className="k-meter-head" style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        <Icon name={icon} size={13} />
        <span className="k-meter-text">{label}</span>
        <span className="k-meter-text" style={{ marginLeft: 'auto', color: 'var(--text)' }}>
          <b style={{ fontWeight: 600 }}>{value}</b>{unit && <span style={{ color: 'var(--sub)' }}>{unit}</span>}
        </span>
      </div>
      {/* The bar drops in the rail (.k-meter-bar); at 4px it only says
          more-or-less, and the number matters more. */}
      <div className="k-meter-bar" style={{ height: 4, borderRadius: 'var(--r-0)', background: 'var(--track)', overflow: 'hidden' }}>
        <span style={{
          display: 'block', width: Math.min(100, Math.round(ratio * 100)) + '%', height: '100%',
          background: ratio > 0.75 ? 'var(--gold)' : 'var(--accent)',
        }} />
      </div>
      {/* Rail-only view: one number under the icon. */}
      <div className="k-meter-mini" style={{
        fontSize: 'var(--fs-7)', color: 'var(--text)', textAlign: 'center',
        fontVariantNumeric: 'tabular-nums',
      }}>{value}{typeof unit === 'string' ? unit.trim() : unit}</div>
      {note && <div style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{note}</div>}
    </div>
  );
}

/* Running jobs, including "yielding" (learning paused for a GGS game). */
export function JobList({ jobs }: { jobs: { label: string; threads?: number; yielded?: boolean }[] }) {
  // Text-only content; dropped entirely in the rail (.k-nav-jobs).
  if (!jobs.length) {
    return <div className="k-nav-jobs" style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>
      {t('ggs.jobs.idle')}
    </div>;
  }
  // display lives in .k-nav-jobs (hidden at 1040px).
  return (
    <div className="k-nav-jobs" style={{ flexDirection: 'column', gap: 3, fontSize: 'var(--fs-7)', color: 'var(--text)' }}>
      {jobs.map((j, i) => (
        <div key={i}>{j.label}{j.threads ? ' ' + t('ggs.jobs.threads', { n: j.threads }) : ''}
          {j.yielded && <span style={{ color: 'var(--sub)' }}>{' ' + t('ggs.jobs.yielding')}</span>}
        </div>
      ))}
    </div>
  );
}

/* ============ Ratings ============ */

export type Rate = { value: number; dev: number; rank?: number; w: number; l: number; d: number; provisional?: boolean };

/* Always show the ±n deviation; a GGS rating means nothing without it. */
export function RateRow({ label, rate }: { label: string; rate: Rate }) {
  return (
    <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)', fontVariantNumeric: 'tabular-nums' }}>
      {/* Format names outgrow --w-label and would wrap, breaking row
          heights. */}
      <span style={{
        width: 'var(--w-gtype)', flex: 'none', whiteSpace: 'nowrap',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>{label}</span>
      <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700 }}>{rate.value.toFixed(1)}</span>
      <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        ±{rate.dev}{rate.provisional && (
          <span style={{ color: 'var(--gold)', marginLeft: 5 }}>{t('ggs.rate.provisional')}</span>
        )}
      </span>
      {rate.rank != null && (
        <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
          {t('ggs.rate.rank', { n: rate.rank })}
        </span>
      )}
      <span style={{ fontSize: 'var(--fs-6)' }}>
        <span style={{ color: 'var(--ok)' }}>{rate.w}{t('ggs.stat.wins')}</span>{' '}
        <span style={{ color: 'var(--bad)' }}>{rate.l}{t('ggs.stat.losses')}</span>
        {' '}{rate.d}{t('ggs.stat.draws')}
      </span>
    </div>
  );
}

/* PlayerRow / StoneDot live in data.tsx. Duplicating them here caused
 * wrong imports and a 1|2 vs 'b'|'w' color-type split; UI colors are
 * 'b' | 'w'. */
export { PlayerRow, StoneDot, toStoneColor, type StoneColor } from './data';

/* ============ Match list ============
 * Synchro games come in pairs; a pair is one row. */

export type Match = {
  id: string; mine: boolean; live: boolean;
  /** Opponent name for own games; black/white for observed ones. */
  opponent?: string;
  black: string; white: string;
  kind: string;      // "Synchro, random 16" / "Standard"
  boards: number;    // 2 for synchro
  ply: number;
  myTurn?: boolean;
  result?: string;   // "+8" and the like, once finished
  /** How it ended: 'finished' / 'adjourned' / 'aborted'. */
  ended?: string;
  /** Who left, for adjourned games. */
  leftBy?: string;
};

const matchTitle = (m: Match) =>
  m.mine
    ? t('ggs.match.me_vs', { name: m.opponent ?? '?' })
    : t('ggs.vs', { a: m.black, b: m.white });

export function MatchRow({ m, active, onSelect, onClose }: {
  m: Match; active?: boolean; onSelect?: () => void; onClose?: () => void;
}) {
  const closable = !m.live && !!onClose;
  // Row body and close button are siblings: nested buttons are invalid
  // HTML and would force reimplementing IconButton's a11y. Hover lives
  // on the outer .k-row; no overlap, so no stopPropagation.
  return (
    <div className={'k-row' + (active ? ' k-on' : '')} style={{
      position: 'relative', borderBottom: '1px solid var(--border-weak)',
      ...picked(!!active),
    }}>
      {/* Reserve 36px for the close button or long names slide under
          it. */}
      {/* k-row makes it read as clickable; without it the selection
          color shows but hover gives nothing. */}
      <button type="button" onClick={onSelect} className="k-row"
              aria-current={active || undefined} style={{
        width: '100%', border: 0, background: 'transparent', textAlign: 'left',
        padding: '9px var(--sp-3)', paddingRight: closable ? 40 : undefined,
        display: 'flex', flexDirection: 'column', gap: 5,
      }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
          <Tag tone={m.mine ? 'accent' : 'sub'}>
            {m.mine ? t('ggs.tag.mine') : t('ggs.tag.observing')}
          </Tag>
          {/* Three endings; labeling an adjournment "finished" implies
              a decided result with no margin. */}
          <Tag tone={m.live ? 'ok' : m.ended === 'adjourned' ? 'bad' : 'sub'}>
            {m.live ? t('ggs.state.playing')
              : m.ended === 'adjourned' ? t('ggs.state.adjourned')
              : m.ended === 'aborted' ? t('ggs.state.aborted') : t('ggs.state.finished')}
          </Tag>
          <span style={{ fontSize: 'var(--fs-5)', color: m.live ? 'var(--text)' : 'var(--sub)' }}>
            {matchTitle(m)}
          </span>
          {m.myTurn && <Dot />}
        </span>
        {/* Clip, never wrap: the row is a fixed two lines, and English
            details run longer than the Japanese ones it was sized for. */}
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)',
                       overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {m.kind}{m.boards > 1 && ' · ' + t('ggs.lobby.game_count', { n: m.boards })}
          {' · '}{t('ggs.match.ply', { n: m.ply })}
          {m.ended === 'adjourned'
            ? ' · ' + (m.leftBy ? t('ggs.play.left_by', { who: m.leftBy }) : t('ggs.play.opp_left'))
            : m.result && ` · ${m.result}`}
        </span>
      </button>
      {closable && (
        <span style={{ position: 'absolute', right: 4, top: 4 }}>
          <IconButton name="close" label={t('ggs.match.close')} onClick={onClose!} size={14} />
        </span>
      )}
    </div>
  );
}

export function Tag({ tone = 'sub', children }: { tone?: 'sub' | 'accent' | 'ok' | 'bad'; children: React.ReactNode }) {
  const solid = tone === 'accent';
  return <span style={{
    padding: '1px 6px', borderRadius: 'var(--r-1)', fontSize: 'var(--fs-tag)', flex: 'none',
    fontWeight: solid ? 600 : 400,
    background: solid ? 'var(--accent-dim)' : tone === 'sub' ? 'var(--card)' : `color-mix(in srgb, var(--${tone}) 18%, transparent)`,
    color: solid ? 'var(--on-accent)' : tone === 'sub' ? 'var(--sub)' : `var(--${tone})`,
  }}>{children}</span>;
}

/* ============ Formula tree ============
 * GGS /os formulas are nested logic where structure is meaning; build
 * them as a tree, not as text. Read-only places use FormulaView. */

export type Cond = SharedCond;
export { isGroup };

/* Read-only tree. Groups (all-of / any-of) are shown by a 2px vertical
 * rule plus indent; --accent outermost, --border inside. */
export function FormulaView({ node, top }: { node: Cond; top?: boolean }) {
  if (node.kind === 'atom') return <CondChip c={node} />;
  return (
    <div style={{
      borderLeft: '2px solid var(--' + (top ? 'accent' : 'border') + ')',
      paddingLeft: 'var(--sp-2h)', display: 'flex', flexDirection: 'column', gap: 5,
    }}>
      <div style={{
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.06em',
        color: node.kind === 'any' ? 'var(--gold)' : 'var(--sub)',
      }}>{node.kind === 'all' ? t('ggs.formula.all_of') : t('ggs.formula.any_of')}</div>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 5, alignItems: 'flex-start' }}>
        {node.kids.map((n, i) => isGroup(n)
          ? <FormulaView key={i} node={n} />
          : <CondChip key={i} c={n} />)}
      </div>
    </div>
  );
}

function CondChip({ c }: { c: Cond }) {
  return (
    <span style={{
      padding: '2px var(--sp-2)', borderRadius: 'var(--r-1)', background: 'var(--panel)',
      border: '1px solid var(--border)', fontSize: 'var(--fs-6)', color: 'var(--text)', fontFamily: 'var(--ff-mono)',
    }}>{condLabel(c)}</span>
  );
}

/* Never hide what goes to the server; raw formula input stays as the
 * escape hatch. */
function FormulaWire({ text }: { text: string }) {
  return (
    <div style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', padding: '9px var(--sp-3)',
      borderRadius: 'var(--r-3)', background: 'var(--panel)', border: '1px solid var(--border)',
    }}>
      <span style={{ fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)', flex: 'none' }}>{t('ggs.formula.wire')}</span>
      <code style={{ fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', color: 'var(--text)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{text}</code>
    </div>
  );
}

/* The builder. The third slot depends on the variable's type: boolean
 * (is / is-not), color (black / white / either), numeric (six
 * comparators + value + unit). Edits keep the tree; condToSrc
 * serializes only on save.
 *
 * value can be null — the no-condition state. This component renders
 * the "unset" and "add condition" UI itself (no branching in parents). */
export function FormulaEditor({ value, onChange, onSave, onClear, onRaw }: {
  value: Cond | null;
  onChange: (c: Cond) => void;
  onSave?: (src: string) => void;
  onClear?: () => void;
  /** Escape hatch: raw GGS formula (expressible beyond the tree). */
  onRaw?: (src: string) => void;
}) {
  if (!value) {
    return (
      <div style={{
        borderRadius: 'var(--r-3)', border: '1px solid var(--border)', background: 'var(--bg)',
        padding: 'var(--sp-4)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', alignItems: 'flex-start',
      }}>
        <div style={{ fontSize: 'var(--fs-5)', color: 'var(--text)' }}>{t('ggs.formula.unset')}</div>
        <Note>{t('ggs.formula.unset_note')}</Note>
        <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
          <Button onClick={() => onChange(newGroup())}>{t('ggs.formula.add_condition')}</Button>
          {onRaw && <Button variant="ghost" onClick={() => onRaw('')}>{t('ggs.formula.write_raw')}</Button>}
        </div>
      </div>
    );
  }
  const src = condToSrc(value);
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
      <CondNode node={value} top onChange={onChange} />
      <FormulaWire text={src} />
      <div style={{ display: 'flex', gap: 'var(--sp-2)', alignItems: 'center' }}>
        <Button variant="secondary" onClick={onClear}>{t('ggs.formula.clear')}</Button>
        {onRaw && <Button variant="ghost" onClick={() => onRaw(src)}>{t('ggs.formula.write_raw')}</Button>}
        <span style={{ marginLeft: 'auto' }}>
          <Button variant="primary" onClick={() => onSave?.(src)}>{t('ggs.apply')}</Button>
        </span>
      </div>
    </div>
  );
}

const newAtom = (): Cond => ({ kind: 'atom', name: 'rated', op: '=', val: '', neg: false });
/* New groups start with one condition inside; an empty frame gives no
 * next action. */
const newGroup = (op: 'all' | 'any' = 'all'): Cond => ({ kind: op, kids: [newAtom()] });

function CondNode({ node, top, onChange, onRemove }: {
  node: Cond; top?: boolean; onChange: (c: Cond) => void; onRemove?: () => void;
}) {
  if (node.kind === 'atom') return <AtomRow c={node} onChange={onChange} onRemove={onRemove} />;

  const setKid = (i: number, c: Cond | null) => {
    const kids = c ? node.kids.map((k, j) => (j === i ? c : k)) : node.kids.filter((_, j) => j !== i);
    onChange({ ...node, kids });
  };
  const addAtom = () => onChange({ ...node, kids: [...node.kids, newAtom()] });
  const addGroup = () => onChange({ ...node, kids: [...node.kids, newGroup('any')] });

  return (
    <div style={{
      borderRadius: 'var(--r-3)', border: '1px solid var(--border)', background: 'var(--bg)',
      padding: 'var(--sp-3)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
    }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
        <Select value={node.kind} width={132} size="ctrl"
                options={[['all', t('ggs.formula.all_of')], ['any', t('ggs.formula.any_of')]]}
                onChange={v => onChange({ ...node, kind: v as 'all' | 'any' })} />
        {onRemove && (
          <span style={{ marginLeft: 'auto' }}>
            <IconButton name="close" label={t('ggs.formula.remove_group')} onClick={onRemove} size={13} />
          </span>
        )}
      </div>
      <div style={{
        paddingLeft: 12, borderLeft: '2px solid var(--' + (top ? 'accent' : 'border') + ')',
        display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
      }}>
        {node.kids.map((k, i) => (
          <CondNode key={i} node={k} onChange={c => setKid(i, c)} onRemove={() => setKid(i, null)} />
        ))}
        <div style={{ display: 'flex', gap: 'var(--sp-2)', paddingTop: 2 }}>
          <Button onClick={addAtom}>{t('ggs.formula.add_atom')}</Button>
          <Button onClick={addGroup}>{t('ggs.formula.add_group')}</Button>
        </div>
      </div>
    </div>
  );
}

function AtomRow({ c, onChange, onRemove }: {
  c: Extract<Cond, { kind: 'atom' }>; onChange: (c: Cond) => void; onRemove?: () => void;
}) {
  const v = varOf(c.name);
  const type = v?.type ?? 'bool';
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
      <Select value={c.name} width={140} size="ctrl"
              options={formulaVars().map(x => [x.name, x.label] as [string, string])}
              onChange={name => {
                const nv = varOf(name);
                onChange({
                  ...c, name,
                  // A type change rebuilds comparator and value (the
                  // variable carries the numeric default).
                  op: nv?.type === 'num' ? '≥' : '=',
                  val: nv?.type === 'num' ? String(nv.def ?? 0) : nv?.type === 'color' ? '?' : '',
                });
              }} />
      {type === 'bool' && (
        <Select value={c.neg ? '1' : '0'} width={88} size="ctrl"
                options={boolOps().map(([neg, label]) => [neg ? '1' : '0', label] as [string, string])}
                onChange={s => onChange({ ...c, neg: s === '1' })} />
      )}
      {type === 'color' && <>
        {/* Same wording as the boolean case; the bare particles don't
            read on their own. */}
        <Select value={c.op} width={88} size="ctrl"
                options={[['=', t('ggs.formula.bool_is')], ['≠', t('ggs.formula.bool_is_not')]]}
                onChange={op => onChange({ ...c, op: op as FormulaOp })} />
        <Select value={c.val} width={84} size="ctrl" options={colorChoices()}
                onChange={val => onChange({ ...c, val })} />
      </>}
      {type === 'num' && <>
        <Select value={c.op} width={74} size="ctrl"
                options={FORMULA_OPS.map(o => [o, o] as [string, string])}
                onChange={op => onChange({ ...c, op: op as FormulaOp })} />
        <TextField value={c.val} numeric align="right" width={58}
                   onChange={val => onChange({ ...c, val })} />
        {v?.unit && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{v.unit}</span>}
      </>}
      {onRemove && (
        <span style={{ marginLeft: 'auto' }}>
          <IconButton name="close" label={t('ggs.formula.remove_atom')} onClick={onRemove} size={13} />
        </span>
      )}
    </div>
  );
}

/* ============ Chat ============ */

export type Msg = { from: string; mine?: boolean; at: string; body: string; ja?: string };

/* English messages carry a Japanese translation (small, under a 1px
 * rule). */
export function Bubble({ m, showName }: { m: Msg; showName?: boolean }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3, alignItems: m.mine ? 'flex-end' : 'flex-start' }}>
      {showName && (
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 7 }}>
          <span className="k-sel" style={{ fontSize: 'var(--fs-6)', fontWeight: 600, color: m.mine ? 'var(--accent)' : 'var(--text)' }}>
            {m.mine ? t('ggs.chat.me') : m.from}
          </span>
          <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{m.at}</span>
        </div>
      )}
      <div style={{
        maxWidth: 320, padding: '6px 10px', borderRadius: 'var(--r-bubble)', fontSize: 'var(--fs-5)', lineHeight: 1.5,
        background: m.mine ? 'var(--accent-dim)' : 'var(--panel)', color: m.mine ? 'var(--on-accent)' : 'var(--text)',
      }}>
        {/* Selectable (k-sel) for quoting; the translation too. */}
        <span className="k-sel">{m.body}</span>
        {m.ja && <div className="k-sel" style={{
          marginTop: 4, paddingTop: 4, borderTop: '1px solid var(--border)',
          fontSize: 'var(--fs-6)', color: 'var(--sub)',
        }}>{m.ja}</div>}
      </div>
    </div>
  );
}

export function DayMark({ children }: { children: React.ReactNode }) {
  return <div style={{
    alignSelf: 'center', fontSize: 'var(--fs-7)', color: 'var(--sub)',
    padding: '2px 10px', borderRadius: 'var(--r-pill)', background: 'var(--panel)',
  }}>{children}</div>;
}

/* ============ Protocol log ============
 * Sent lines get › and --accent; received lines align the indent to
 * keep monospace columns. */

export type LogLine = { dir: 'out' | 'in' | 'app'; text: string };

export function ConsoleLog({ lines }: { lines: LogLine[] }) {
  // Follow the tail only while viewing near it; never yank while the
  // user scrolls back.
  const box = React.useRef<HTMLDivElement>(null);
  const stick = React.useRef(true);
  React.useEffect(() => {
    const b = box.current;
    if (b && stick.current) b.scrollTop = b.scrollHeight;
  }, [lines.length]);
  return (
    <div className="k-scroll" ref={box}
      onScroll={() => {
        const b = box.current;
        if (b) stick.current = b.scrollTop + b.clientHeight >= b.scrollHeight - 30;
      }}
      style={{
      flex: 1, minHeight: 0, padding: '10px var(--sp-3)', display: 'flex', flexDirection: 'column', gap: 3,
      fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', lineHeight: 1.7,
    }}>
      {lines.map((l, i) => (
        <div key={i} style={{
          display: 'flex', gap: 'var(--sp-2)',
          color: l.dir === 'out' ? 'var(--accent)' : l.dir === 'app' ? 'var(--sub)' : 'var(--text)',
        }}>
          <span style={{ flex: 'none', width: 8 }}>{l.dir === 'out' ? '›' : ''}</span>
          {/* Selectable (k-sel) for pasting into bug reports. */}
          <span className="k-sel">{l.text}</span>
        </div>
      ))}
    </div>
  );
}

/* ============ Toasts ============
 * Only failures and why-nothing-happened messages. No progress
 * notices, no restating what the user can see, no engine-internal
 * codes ("stopped" etc.). */

export type Toast = { id: string; tone: 'bad' | 'gold'; text: string };

export function Toasts({ items, onDismiss }: { items: Toast[]; onDismiss?: (id: string) => void }) {
  return (
    <div style={{
      position: 'absolute', right: 'var(--sp-4)', bottom: 'calc(var(--h-status) + var(--sp-3))',
      display: 'flex', flexDirection: 'column', gap: 'var(--sp-2h)', alignItems: 'flex-end', zIndex: 20,
    }}>
      {items.map(item => (
        <button key={item.id} type="button" onClick={() => onDismiss?.(item.id)} className="k-press" style={{
          // Floating element: --r-4 radius and shadow (rule 13); no
          // bare px (rule 1).
          maxWidth: 340, padding: 'var(--sp-3)', borderRadius: 'var(--r-4)', textAlign: 'left',
          background: 'var(--card)', border: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
          fontSize: 'var(--fs-5)', lineHeight: 1.6, color: 'var(--text)',
          display: 'flex', gap: 'var(--sp-2)', alignItems: 'flex-start',
        }}>
            <span style={{ color: 'var(--' + item.tone + ')', flex: 'none', marginTop: 1 }}><Icon name="alert" size={15} /></span>
          {item.text}
        </button>
      ))}
    </div>
  );
}

/* ============ Status-bar right edge ============
 * Chat / console appear only during a GGS game. */

export function StatusChip({ label, unread, active, onClick }: {
  label: string; unread?: number; active?: boolean; onClick?: () => void;
}) {
  return (
    <button type="button" onClick={onClick} className={'k-press' + (active ? ' k-on' : '')} style={{
      height: 'var(--h-chip)', padding: '0 10px', borderRadius: 'var(--r-1)', background: 'transparent',
      border: '1px solid var(--' + (active ? 'accent' : 'border') + ')',
      color: 'var(--' + (active ? 'accent' : 'sub') + ')',
      fontSize: 'var(--fs-7)', display: 'flex', alignItems: 'center', gap: 'var(--sp-0)',
    }}>
      {label}
      {unread ? <Badge tone="bad">{unread}</Badge> : null}
    </button>
  );
}

/* ============ GGS status band ============
 * The design's GGS screen tops with name, both pool ratings, current
 * strength and waiting mode — the four things you want while playing,
 * previously scattered across screens. Lives in the toolbar's aux, so
 * it drops below 940px; it is display-only, nothing lost (rule 8). */
export function GgsStatus({ snap, showStrength = true }: {
  snap: GgsSnapshot;
  /** Whether to show strength. Hidden on the GGS settings screen —
   *  the control that changes it sits right below (rule 58). */
  showStrength?: boolean;
}) {
  const e = snap.engine;
  return (
    <>
      {snap.my_ranks.map((r) => (
        <span key={r.gtype} style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
          {poolLabel(r.gtype)}{' '}
          <b style={{ color: 'var(--text)', fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>
            {r.rating.toFixed(0)}
          </b>
          {/* Ratings always carry the deviation (rule 29). */}
          <span style={{ opacity: .7, marginLeft: 3 }}>±{Math.round(r.dev)}</span>
        </span>
      ))}
      {showStrength && (
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
          {t('ggs.status.strength')} <b style={{ color: 'var(--text)', fontWeight: 600 }}>
            {t('ggs.status.depth', { n: e.depth })} / {t('ggs.status.solve', { n: e.solve })}
            {e.band > 0 ? ' / ' + t('ggs.status.band', { n: e.band }) : ''}
          </b>
        </span>
      )}
      {snap.standby.enabled && <Tag tone="ok">{t('ggs.waiting_mode')}</Tag>}
    </>
  );
}

/** Short pool names; GGS has only 8 and 8r. */
const poolLabel = (pool: string): string =>
  pool === '8' ? t('ggs.pool.normal') : pool === '8r' ? t('ggs.pool.random') : pool;
