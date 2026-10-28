import React from 'react';
import { Icon, IconButton, type IconName } from './Icons';
import { Dot, Button, Select, TextField } from './primitives';
import { Caption, Note, picked } from './layout';
import { t } from '../i18n';
import logo from '../../assets/kuroobi.svg?raw';
import {
  colorChoices, boolOps, FORMULA_OPS, formulaVars, varOf, condToSrc, condLabel, groupChoices, groupLabel,
  numOk, poolChoices, isGroup, condValid, type Atom, type Cond, type FormulaOp, type Who,
} from '../ggs';

export type NavId =
  | 'play' | 'study' | 'book' | 'settings'
  | 'ggs-play' | 'ggs-lobby' | 'ggs-players' | 'ggs-results' | 'ggs-chat' | 'ggs-tourney' | 'ggs-console'
  | 'ggs-login';

export type NavItem = {
  id: NavId;
  label: string;
  icon: IconName;
  count?: number;
  alert?: boolean;   // addressed to me / unread; painted --bad
  dot?: 'ok' | 'gold' | 'bad';  // state badge (auto match on, my turn, ...)
  hint?: string;
};

type Conn = 'offline' | 'connecting' | 'logging-in' | 'online';

const CONN_TONE: Record<Conn, 'sub' | 'gold' | 'ok'> = {
  offline: 'sub', connecting: 'gold', 'logging-in': 'gold', online: 'ok',
};

export const navLocal = (): NavItem[] => [
  { id: 'play', label: t('ggs.nav.play'), icon: 'play' },
  { id: 'study', label: t('ggs.nav.study'), icon: 'study' },
  { id: 'book', label: t('ggs.nav.book'), icon: 'book', hint: '⌘B' },
];

export function ggsNav(conn: Conn, badges?: Partial<Record<NavId, Pick<NavItem, 'count' | 'alert' | 'dot'>>>): NavItem[] {
  if (conn !== 'online') return [{ id: 'ggs-login', label: t('ggs.nav.login'), icon: 'login' }];
  const base: NavItem[] = [
    { id: 'ggs-play', label: t('ggs.nav.games'), icon: 'ggs-play' },
    { id: 'ggs-lobby', label: t('ggs.nav.lobby'), icon: 'ggs-lobby' },
    { id: 'ggs-players', label: t('ggs.nav.players'), icon: 'ggs-users' },
    { id: 'ggs-results', label: t('ggs.nav.results'), icon: 'results' },
    { id: 'ggs-chat', label: t('ggs.nav.chat'), icon: 'ggs-chat' },
    { id: 'ggs-tourney', label: t('ggs.nav.tourney'), icon: 'ggs-tourney' },
    { id: 'ggs-console', label: t('ggs.nav.console'), icon: 'ggs-console' },
  ];
  return base.map(i => ({ ...i, ...(badges?.[i.id] ?? {}) }));
}

export function Nav({ items, ggsItems, conn, active, onSelect, footer }: {
  items: NavItem[]; ggsItems: NavItem[]; conn: Conn;
  active: NavId; onSelect?: (id: NavId) => void; footer?: React.ReactNode;
}) {
  return (
    <nav className="k-nav" style={{
      flex: 'none', background: 'var(--panel)',
      borderRight: '1px solid var(--border)', display: 'flex', flexDirection: 'column', minHeight: 0,
    }}>
      <div className="k-nav-logo" style={{ height: 'var(--h-bar)', flex: 'none' }}>
        <span aria-label="KUROOBI" dangerouslySetInnerHTML={{ __html: logo }} />
      </div>
      {/* The rows scroll when the window is too low for them; the logo and the footer stay put. */}
      <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>
      <div style={{ padding: 'var(--sp-1) var(--sp-2) 0', display: 'flex', flexDirection: 'column', gap: 1 }}>
        {items.map(i => <NavRow key={i.id} item={i} active={active === i.id} onSelect={onSelect} />)}
      </div>
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
      </div>
      {footer && <div className="k-nav-foot" style={{
        flex: 'none', display: 'flex', flexDirection: 'column',
        gap: 'var(--sp-3)', borderTop: '1px solid var(--border)',
      }}>{footer}</div>}
    </nav>
  );
}

export function NavRow({ item, active, onSelect }: { item: NavItem; active: boolean; onSelect?: (id: NavId) => void }) {
  return (
    <button type="button" onClick={() => onSelect?.(item.id)} aria-current={active || undefined}
      title={item.hint ? `${item.label} (${item.hint})` : item.label}
      className={'k-nav-row k-row' + (active ? ' k-on' : '')}
      style={{
        height: 'var(--h-field)', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        border: 0, borderRadius: 'var(--r-2)', fontSize: 'var(--fs-4)',
        background: active ? 'var(--accent-dim)' : 'transparent',
        color: active ? 'var(--on-accent)' : 'var(--text)', fontWeight: active ? 600 : 400,
      }}>
      <span style={{ flex: 'none', display: 'grid', placeItems: 'center' }}>
        <Icon name={item.icon} size={16} />
      </span>
      <span className="k-nav-label">{item.label}</span>
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





type Rate = { value: number; dev: number; rank?: number; w: number; l: number; d: number; provisional?: boolean };

export function RateRow({ label, rate }: { label: string; rate: Rate }) {
  return (
    <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)', fontVariantNumeric: 'tabular-nums' }}>
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

export type Match = {
  mine: boolean; live: boolean;
  opponent?: string;
  black: string; white: string;
  kind: string;
  result?: string;
  ended?: string;
  leftBy?: string;
};

// Both players by name, the same for one's own games and watched ones: only the actions differ.
const matchTitle = (m: Match) => t('ggs.vs', { a: m.black, b: m.white });

export function MatchRow({ m, active, onSelect, onClose }: {
  m: Match; active: boolean; onSelect: () => void; onClose: () => void;
}) {
  const closable = !m.live;
  return (
    // Only the button is the row: a k-row on the wrapper too laid the hover over twice.
    <div style={{
      position: 'relative', borderBottom: '1px solid var(--border-weak)',
      ...picked(active),
    }}>
      <button type="button" onClick={onSelect} className={'k-row' + (active ? ' k-on' : '')}
              aria-current={active || undefined} style={{
        width: '100%', border: 0, background: 'transparent', textAlign: 'left',
        padding: '9px var(--sp-3)', paddingRight: closable ? 40 : undefined,
        display: 'flex', flexDirection: 'column', gap: 5,
      }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)', minWidth: 0, whiteSpace: 'nowrap' }}>
          {/* Watched games carry a quiet mark; one's own games need none. */}
          {!m.mine && <Tag tone="sub">{t('ggs.tag.observing')}</Tag>}
          {/* Live games sit under the "in progress" heading already; only an ended game says how it ended. */}
          {!m.live && (
            <Tag tone={m.ended === 'adjourned' ? 'bad' : 'sub'}>
              {m.ended === 'adjourned' ? t('ggs.state.adjourned')
                : m.ended === 'aborted' ? t('ggs.state.aborted') : t('ggs.state.finished')}
            </Tag>
          )}
          <span style={{ fontSize: 'var(--fs-5)', color: m.live ? 'var(--text)' : 'var(--sub)', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>
            {matchTitle(m)}
          </span>
        </span>
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)',
                       overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {m.kind}
          {m.ended === 'adjourned'
            ? ' · ' + (m.leftBy ? t('ggs.play.left_by', { who: m.leftBy }) : t('ggs.play.opp_left'))
            : m.result && ` · ${m.result}`}
        </span>
      </button>
      {closable && (
        <span style={{ position: 'absolute', right: 4, top: 4 }}>
          <IconButton name="close" label={t('ggs.match.close')} onClick={onClose} size={14} />
        </span>
      )}
    </div>
  );
}

/**
 * A small label. It must read on every surface it lands on (the ground, a panel, a chosen row's
 * card), in both themes: a thin tint and an outline of its colour give the shape, and the text
 * is its colour pulled toward the body text, which keeps it above 4.5:1 on that tint.
 */
export function Tag({ tone = 'sub', children }: { tone?: 'sub' | 'accent' | 'ok' | 'bad'; children: React.ReactNode }) {
  const solid = tone === 'accent';
  const c = `var(--${tone})`;
  return <span style={{
    padding: '0 5px', borderRadius: 'var(--r-1)', fontSize: 'var(--fs-tag)', flex: 'none',
    fontWeight: solid ? 600 : 500,
    border: `1px solid ${solid ? 'var(--accent-dim)' : `color-mix(in srgb, ${c} 50%, transparent)`}`,
    background: solid ? 'var(--accent-dim)' : `color-mix(in srgb, ${c} 12%, transparent)`,
    color: solid ? 'var(--on-accent)' : `color-mix(in srgb, ${c} 70%, var(--text))`,
  }}>{children}</span>;
}


export function FormulaView({ node, top, who }: { node: Cond; top?: boolean; who?: Who }) {
  if (!isGroup(node)) return <CondChip c={node} who={who} />;
  return (
    <div style={{
      borderLeft: '2px solid var(--' + (top ? 'accent' : 'border') + ')',
      paddingLeft: 'var(--sp-2h)', display: 'flex', flexDirection: 'column', gap: 5,
    }}>
      <div style={{
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.06em',
        color: node.kind === 'any' ? 'var(--gold)' : 'var(--sub)',
      }}>{groupLabel(node)}</div>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 5, alignItems: 'flex-start' }}>
        {node.kids.map((n, i) => isGroup(n)
          ? <FormulaView key={i} node={n} who={who} />
          : <CondChip key={i} c={n} who={who} />)}
      </div>
    </div>
  );
}

function CondChip({ c, who }: { c: Cond; who?: Who }) {
  const known = isGroup(c) || (c.kind === 'atom' && !!varOf(c.name, who));
  return (
    <span style={{
      padding: '2px var(--sp-2)', borderRadius: 'var(--r-1)', background: 'var(--panel)',
      border: `1px ${known ? 'solid' : 'dashed'} var(--border)`, fontSize: 'var(--fs-6)',
      color: known ? 'var(--text)' : 'var(--sub)', fontFamily: 'var(--ff-mono)',
    }}>{condLabel(c, who)}</span>
  );
}

function FormulaWire({ text }: { text: string }) {
  return (
    <div style={{
      display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)', padding: '9px var(--sp-3)',
      borderRadius: 'var(--r-3)', background: 'var(--panel)', border: '1px solid var(--border)',
    }}>
      <Caption>{t('ggs.formula.wire')}</Caption>
      <code className="k-sel" style={{
        fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', color: 'var(--text)',
        minWidth: 0, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', lineHeight: 1.6,
      }}>{text}</code>
    </div>
  );
}

export function FormulaEditor({ value, onChange, onSave, onClear, onRaw, dirty = false }: {
  value: Cond | null;
  onChange: (c: Cond) => void;
  onSave: (src: string) => void;
  onClear: () => void;
  onRaw: (src: string) => void;
  /** The rule on screen differs from the one GGS has: it is sent only by "Apply". */
  dirty?: boolean;
}) {
  // Unlike the other settings this is sent only on "Apply", so an unsent change says so.
  const valid = !value || condValid(value);
  const apply = (src: string) => (
    <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
      {!valid
        ? <span style={{ fontSize: 'var(--fs-6)', color: 'var(--bad)' }}>{t('ggs.formula.invalid')}</span>
        : dirty && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--gold)' }}>{t('ggs.formula.unsent')}</span>}
      <Button variant="primary" disabled={!dirty || !valid} onClick={() => onSave(src)}>{t('ggs.apply')}</Button>
    </span>
  );
  if (!value) {
    return (
      <div style={{
        borderRadius: 'var(--r-3)', border: '1px solid var(--border)', background: 'var(--bg)',
        padding: 'var(--sp-4)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', alignItems: 'flex-start',
      }}>
        <div style={{ fontSize: 'var(--fs-5)', color: 'var(--text)' }}>{t('ggs.formula.unset')}</div>
        <Note>{t('ggs.formula.unset_note')}</Note>
        <div style={{ display: 'flex', gap: 'var(--sp-2)', alignSelf: 'stretch', alignItems: 'center' }}>
          <Button onClick={() => onChange(newGroup())}>{t('ggs.formula.add_condition')}</Button>
          <Button variant="ghost" onClick={() => onRaw('')}>{t('ggs.formula.write_raw')}</Button>
          {/* Clearing a rule GGS still has is a change to send too. */}
          {dirty && apply('')}
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
        <Button variant="ghost" onClick={() => onRaw(src)}>{t('ggs.formula.write_raw')}</Button>
        {apply(src)}
      </div>
    </div>
  );
}

const newAtom = (): Cond => ({ kind: 'atom', name: 'rated', op: '=', val: '', neg: false });
const newGroup = (op: 'all' | 'any' = 'all'): Cond => ({ kind: op, kids: [newAtom()] });

function CondNode({ node, top, onChange, onRemove }: {
  node: Cond; top?: boolean; onChange: (c: Cond) => void; onRemove?: () => void;
}) {
  if (!isGroup(node)) return <AtomRow c={node} onChange={onChange} onRemove={onRemove} />;

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
        <Select value={(node.neg ? '!' : '') + node.kind} width={160} size="ctrl" options={groupChoices()}
                onChange={v => onChange({ ...node, kind: v.replace('!', '') as 'all' | 'any', neg: v.startsWith('!') })} />
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

const EXPR = '=expr';

function AtomRow({ c, onChange, onRemove }: {
  c: Atom | { kind: 'expr'; src: string }; onChange: (c: Cond) => void; onRemove?: () => void;
}) {
  const v = c.kind === 'atom' ? varOf(c.name) : undefined;
  const type = v?.type ?? 'bool';
  const pick = (name: string) => {
    if (name === EXPR) { onChange({ kind: 'expr', src: condToSrc(c) }); return; }
    const nv = varOf(name);
    onChange({
      kind: 'atom', name, neg: false,
      op: nv?.type === 'num' ? '≥' : '=',
      val: nv?.type === 'num' ? String(nv.def ?? 0) : nv?.type === 'color' ? '?' : '',
      ...(nv?.arg ? { arg: nv.argDef } : {}),
    });
  };
  const remove = onRemove && (
    <span style={{ flex: 'none', height: 'var(--h-ctrl)', display: 'flex', alignItems: 'center' }}>
      <IconButton name="close" label={t('ggs.formula.remove_atom')} onClick={onRemove} size={13} />
    </span>
  );
  const numVars = formulaVars().filter(x => x.type === 'num' && !x.arg && !x.real === !v?.real);
  return (
    <div style={{ display: 'flex', alignItems: 'flex-start', gap: 'var(--sp-2)' }}>
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)', flexWrap: 'wrap', rowGap: 'var(--sp-1)', flex: 1, minWidth: 0 }}>
      <Select value={c.kind === 'expr' ? EXPR : c.name} width="var(--w-select-sm)" size="ctrl"
              options={[...formulaVars().map(x => [x.name, x.label] as [string, string]),
                        [EXPR, t('ggs.formula.as_expr')]]}
              onChange={pick} />
      {c.kind === 'expr' && (
        <span style={{ flex: 1, minWidth: 160, display: 'flex' }}>
          <TextField mono value={c.src} onChange={src => onChange({ kind: 'expr', src })}
                     placeholder={t('ggs.settings.formula_placeholder')} />
        </span>
      )}
      {c.kind === 'atom' && v?.arg === 'num' && <>
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', padding: '0 var(--sp-1)' }}>
          {c.name === 'rnd' ? t('ggs.formula.arg_below') : t('ggs.formula.arg_result')}
        </span>
        <TextField value={c.arg ?? ''} align="right" width={52} invalid={!numOk(c.arg ?? '', c.name !== 'rnd')}
                   onChange={arg => onChange({ ...c, arg })} />
      </>}
      {c.kind === 'atom' && v?.arg === 'pool' && (
        <Select value={c.arg ?? 'both'} width={96} size="ctrl" options={poolChoices()}
                onChange={arg => onChange({ ...c, arg })} />
      )}
      {c.kind === 'atom' && type === 'bool' && (
        <Select value={c.neg ? '1' : '0'} width={88} size="ctrl"
                options={boolOps().map(([neg, label]) => [neg ? '1' : '0', label] as [string, string])}
                onChange={s => onChange({ ...c, neg: s === '1' })} />
      )}
      {c.kind === 'atom' && type === 'color' && <>
        <Select value={c.op} width={88} size="ctrl"
                options={[['=', t('ggs.formula.bool_is')], ['≠', t('ggs.formula.bool_is_not')]]}
                onChange={op => onChange({ ...c, op: op as FormulaOp })} />
        <Select value={c.val} width={84} size="ctrl" options={colorChoices()}
                onChange={val => onChange({ ...c, val })} />
      </>}
      {c.kind === 'atom' && type === 'num' && <>
        <Select value={c.op} width={74} size="ctrl"
                options={FORMULA_OPS.map(o => [o, o] as [string, string])}
                onChange={op => onChange({ ...c, op: op as FormulaOp })} />
        {/* The other side is a number, or another variable such as the opponent's clock (mt1 != ot1). */}
        {!v?.arg && (
          <Select value={varOf(c.val) ? c.val : '#'} width={varOf(c.val) ? 'var(--w-select-sm)' : 80} size="ctrl"
                  options={[['#', t('ggs.formula.number')] as [string, string],
                            ...numVars.map(x => [x.name, x.label] as [string, string])]}
                  onChange={val => onChange({ ...c, val: val === '#' ? String(v?.def ?? 0) : val })} />
        )}
        {!varOf(c.val) && <>
          <TextField value={c.val} align="right" width={v?.real ? 66 : 58} invalid={!numOk(c.val, v?.real)}
                     onChange={val => onChange({ ...c, val })} />
          {v?.unit && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{v.unit}</span>}
        </>}
      </>}
    </div>
      {remove}
    </div>
  );
}


type Msg = { from: string; mine?: boolean; at: string; body: string; ja?: string };

export function Bubble({ m, showName }: { m: Msg; showName?: boolean }) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 3, alignItems: m.mine ? 'flex-end' : 'flex-start' }}>
      {showName && (
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 7 }}>
          <span className="k-sel" style={{ fontSize: 'var(--fs-6)', fontWeight: 600, color: m.mine ? 'var(--accent)' : 'var(--text)' }}>
            {m.from}
          </span>
          <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{m.at}</span>
        </div>
      )}
      <div style={{
        maxWidth: 'var(--w-bubble)', padding: '6px 10px', borderRadius: 'var(--r-bubble)', fontSize: 'var(--fs-5)', lineHeight: 1.5,
        background: m.mine ? 'var(--accent-dim)' : 'var(--panel)', color: m.mine ? 'var(--on-accent)' : 'var(--text)',
      }}>
        <span className="k-sel" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{m.body}</span>
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


export type LogLine = { seq: number; dir: 'out' | 'in' | 'app'; text: string };

export function ConsoleLog({ lines, follow = true }: { lines: LogLine[]; follow?: boolean }) {
  const box = React.useRef<HTMLDivElement>(null);
  const stick = React.useRef(true);
  const last = lines.at(-1)?.seq;
  React.useEffect(() => {
    const b = box.current;
    if (b && follow && stick.current) b.scrollTop = b.scrollHeight;
  }, [last, follow]);
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
      {lines.map((l) => (
        <div key={l.seq} style={{
          display: 'flex', gap: 'var(--sp-2)',
          color: l.dir === 'out' ? 'var(--accent)' : l.dir === 'app' ? 'var(--sub)' : 'var(--text)',
        }}>
          <span style={{ flex: 'none', width: 8 }}>{l.dir === 'out' ? '›' : ''}</span>
          <span className="k-sel">{l.text}</span>
        </div>
      ))}
    </div>
  );
}


type Toast = { id: number; tone: 'bad' | 'gold'; text: string };

export function Toasts({ items, onDismiss }: { items: Toast[]; onDismiss: (id: number) => void }) {
  return (
    // Above a drawer (26), below a dialog's scrim (30).
    <div data-k-toasts="" style={{
      position: 'absolute', right: 'var(--sp-4)', bottom: 'calc(var(--h-status) + var(--sp-3))',
      display: 'flex', flexDirection: 'column', gap: 'var(--sp-2h)', alignItems: 'flex-end', zIndex: 27,
    }}>
      {items.map(item => (
        <button key={item.id} type="button" onClick={() => onDismiss(item.id)} className="k-press" style={{
          maxWidth: 'var(--w-modal)', padding: 'var(--sp-3)', borderRadius: 'var(--r-4)', textAlign: 'left',
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


export const poolLabel = (pool: string): string =>
  pool === '8' ? t('ggs.pool.normal') : pool === '8r' ? t('ggs.pool.random') : pool;
