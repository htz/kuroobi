import React, { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { ggsApi, type UnfitView } from './api';
import type { GgsSnapshot, StandbyCfg, UserRow } from './types';
import { clockChoices, gtypeChoices, gtypeLabel, onlineRows, presence } from './ggs';
import { t, tErr } from './i18n';
import { marked, Modal, Overlay, popoverStyle, useDismiss } from './components/layout';
import { ListNote } from './ggsui';
import { Button, Segmented, Select, TextField, Toggle } from './components/primitives';

/** What the sheet opens with. Every field is optional: the rest comes from the last request. */
export interface AskInit {
  opponent?: string;
  gtype?: string;
  time?: string;
  rated?: boolean;
  repeat?: boolean;
}

// The sheet is opened from many screens (lobby, players, results, chat, the status bar), so it
// lives outside them: a tiny store that App renders from.
let current: AskInit | null = null;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());
export const askSheet = {
  open(init: AskInit = {}) { current = init; emit(); },
  close() { current = null; emit(); },
};
export function useAskSheet(): AskInit | null {
  return useSyncExternalStore(
    (f) => { subs.add(f); return () => subs.delete(f); },
    () => current,
  );
}

// The last request, so the sheet opens as it was left.
const LAST_KEY = 'kuroobi.ask.last';
type Last = { gtype: string; time: string; rated: boolean };
const readLast = (): Last => {
  try {
    const v = JSON.parse(localStorage.getItem(LAST_KEY) ?? '');
    if (v && typeof v.gtype === 'string') return v as Last;
  } catch { /* nothing saved yet */ }
  return { gtype: 's8r14', time: '00:05:00', rated: true };
};
const writeLast = (v: Last) => {
  try { localStorage.setItem(LAST_KEY, JSON.stringify(v)); } catch { /* storage off */ }
};

/** A registered account has a name and an email on the server; rated games need one. */
function registered(snap: GgsSnapshot): boolean {
  const f = snap.fingers[snap.login];
  if (!f) return true;
  return f.fields.some(([k, v]) => k.toLowerCase().startsWith('email') && v.trim() !== '' && v.trim() !== '-');
}

const ratingOf = (u: UserRow, random: boolean): string => {
  const r = random ? (u.rating_r ?? u.rating) : (u.rating ?? u.rating_r);
  return r != null ? r.toFixed(1) : '';
};

export function AskSheet({ snap, init }: { snap: GgsSnapshot; init: AskInit }) {
  const sb = snap.standby;
  const running = sb.enabled;
  const last = useMemo(() => readLast(), []);
  const [opp, setOpp] = useState(init.opponent ?? (init.repeat && running ? sb.opponent : ''));
  const [gtype, setGtype] = useState(init.gtype ?? (init.repeat && running ? sb.gtype : last.gtype));
  const [time, setTime] = useState(init.time ?? (init.repeat && running ? sb.time : last.time));
  const reg = registered(snap);
  const [rated, setRated] = useState(reg && (init.rated ?? last.rated));
  const [repeat, setRepeat] = useState(!!init.repeat);
  const [maxGames, setMaxGames] = useState(String(running ? sb.max_games : sb.max_games || 20));
  const [interval, setInterval] = useState(String(sb.interval_secs || 20));
  const [autoAccept, setAutoAccept] = useState(sb.auto_accept);
  const [query, setQuery] = useState('');
  const [err, setErr] = useState('');
  const close = () => askSheet.close();

  const random = gtype.includes('r');
  const people = onlineRows(snap)
    .filter((u) => u.name !== snap.login)
    .filter((u) => !query || u.name.toLowerCase().includes(query.toLowerCase()))
    // While searching, the name typed in full comes first, then names that start with it.
    .sort((a, b) => matchRank(a.name, query) - matchRank(b.name, query)
      || Number(ratingOf(b, random) || 0) - Number(ratingOf(a, random) || 0));
  const stored = opp ? snap.stored.find((x) => x.opp === opp) : undefined;
  const oppRow = opp ? onlineRows(snap).find((u) => u.name === opp) : undefined;
  const stateOf = (u: UserRow) => {
    const p = presence(snap, u.name);
    return p === 'playing' ? { label: t('ask.state_playing'), color: 'var(--ok)' }
      : u.open === '-' ? { label: t('ask.state_closed'), color: 'var(--sub)' }
      : { label: t('ask.state_open'), color: 'var(--accent)' };
  };
  const clocks = clockChoices();
  const pick = (name: string) => setOpp(name);

  // The opponent's decline formula, checked here so a request it would refuse is never sent.
  useEffect(() => { if (opp) ggsApi.finger(opp).catch(() => {}); }, [opp]);
  const decline = opp ? snap.fingers[opp]?.fields.find(([k]) => k === 'decline')?.[1] ?? '' : '';
  const key = [opp, gtype, time, rated, decline].join('|');
  const [unfit, setUnfit] = useState<{ key: string; view: UnfitView | null }>({ key: '', view: null });
  const [unfitOpen, setUnfitOpen] = useState(false);
  useEffect(() => {
    if (!opp) return;
    let alive = true;
    ggsApi.unfit(gtype, time, opp, rated)
      .then((view) => { if (alive) setUnfit({ key, view }); })
      .catch(() => {});
    return () => { alive = false; };
  }, [key, opp, gtype, time, rated]);
  const unfitView = opp && unfit.key === key ? unfit.view : null;
  // Folded, only the lines that hold show; a group that does not hold hides with its lines.
  const unfitRows: UnfitView['rows'] = [];
  let unfitHidden = 0;
  let below = -1;
  for (const r of unfitView?.rows ?? []) {
    if (below >= 0 && r.depth > below) { if (unfitOpen) unfitRows.push(r); continue; }
    below = -1;
    if (!r.hit) {
      unfitHidden++;
      if (r.group !== null) below = r.depth;
      if (!unfitOpen) continue;
    }
    unfitRows.push(r);
  }

  const go = async () => {
    setErr('');
    writeLast({ gtype, time, rated });
    try {
      if (repeat) {
        const cfg: StandbyCfg = {
          ...sb, enabled: true, opponent: opp, gtype, time, rated,
          max_games: Math.max(0, Math.floor(Number(maxGames) || 0)),
          interval_secs: Math.max(0, Math.floor(Number(interval) || 0)),
          auto_accept: autoAccept,
        };
        await ggsApi.setStandby(cfg);
      } else {
        await ggsApi.ask(gtype, time, opp, rated);
      }
      close();
    } catch (e) { setErr(tErr(e)); }
  };
  const stop = async () => {
    try { await ggsApi.setStandby({ ...sb, enabled: false }); close(); } catch (e) { setErr(tErr(e)); }
  };

  const label = (s: string) => (
    <span style={{ lineHeight: 'var(--h-ctrl)', fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>{s}</span>
  );
  const summary = [
    opp || (repeat ? t('ask.pick_opponent') : t('ask.anyone_short')),
    gtypeLabel(gtype),
    clocks.find(([v]) => v === time)?.[1] ?? time,
    rated ? t('ggs.rated') : t('ask.unrated'),
  ].join(' · ');

  return (
    <Overlay onClose={close}>
      <Modal title={t('ask.title')} width="var(--w-modal-wide)" onClose={close} scroll
             actions={<>
               <span style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-6)', lineHeight: 1.35, color: err ? 'var(--bad)' : 'var(--sub)' }}>
                 {err || summary}
               </span>
               {running && repeat && (
                 <Button size="field" variant="danger" onClick={() => void stop()}>{t('ask.stop')}</Button>
               )}
               <Button size="field" variant="ghost" onClick={close}>{t('dialog.cancel')}</Button>
               <Button size="field" variant="primary" disabled={(repeat && !opp) || unfitView !== null} onClick={() => void go()}>
                 {repeat ? (running ? t('ask.apply_repeat') : t('ask.start_repeat')) : t('ask.go')}
               </Button>
             </>}>
        <div style={{ display: 'grid', gridTemplateColumns: 'var(--w-label-sm) minmax(0, 1fr)', columnGap: 'var(--sp-3)',
                      rowGap: 'var(--sp-3)', alignItems: 'start' }}>
          {label(t('ask.opponent'))}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)', minWidth: 0 }}>
            <OppPicker label={opp || (repeat ? t('ask.pick_opponent') : t('ask.anyone'))} rate={oppRow ? ratingOf(oppRow, random) : ''}
                       state={oppRow ? stateOf(oppRow).label : ''} stateColor={oppRow ? stateOf(oppRow).color : undefined}
                       query={query} onQuery={setQuery}
                       empty={!people.length && query ? t('ggs.users.nobody') : ''}
                       items={[
                         ...(repeat ? [] : [{ key: '', name: t('ask.anyone'), rate: '', state: '', on: opp === '', pick: () => pick('') }]),
                         ...people.map((u) => ({ key: u.name, name: u.name, rate: ratingOf(u, random), state: stateOf(u).label,
                                                  stateColor: stateOf(u).color, on: opp === u.name, pick: () => pick(u.name) })),
                       ]} />
            {repeat && !opp && (
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--gold)' }}>{t('ask.need_opponent')}</span>
            )}
            {stored && (
              <div style={{ display: 'flex', flexDirection: 'column', lineHeight: 1.35, padding: 'var(--sp-2) var(--sp-3)',
                            borderRadius: 'var(--r-1)', background: 'var(--card)', boxShadow: 'inset 2px 0 0 var(--gold)' }}>
                <span style={{ fontSize: 'var(--fs-5)', fontWeight: 600 }}>{t('ask.stored', { name: opp })}</span>
                <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                  {gtypeLabel(stored.gtype)} · {t('ask.stored_where')}
                </span>
              </div>
            )}
            {unfitView && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', lineHeight: 1.35, padding: 'var(--sp-2) var(--sp-3)',
                            borderRadius: 'var(--r-1)', background: 'var(--card)', boxShadow: 'inset 2px 0 0 var(--bad)' }}>
                <span style={{ fontSize: 'var(--fs-5)', fontWeight: 600 }}>{t('ask.unfit', { name: opp })}</span>
                <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t(`ask.unfit_lead_${unfitView.top}`)}</span>
                {/* Indent stops at four levels, long words wrap, and a long list scrolls inside the card. */}
                <div className="k-scroll" style={{ maxHeight: 240, display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                  {unfitRows.map((r, i) => (
                    <span key={i} style={{ flex: 'none', display: 'flex', alignItems: 'baseline', gap: 'var(--sp-2)', paddingLeft: Math.min(r.depth, 4) * 16,
                                           fontSize: 'var(--fs-6)', color: r.hit ? 'var(--text)' : 'var(--sub)', fontWeight: r.group !== null ? 600 : 400 }}>
                      <span aria-hidden="true" style={{ width: 10, flex: 'none', textAlign: 'center', color: r.hit ? 'var(--bad)' : 'var(--sub)' }}>
                        {r.hit ? '✕' : '·'}
                      </span>
                      <span style={{ minWidth: 0, overflowWrap: 'anywhere' }}>{r.group === null ? r.text : t(r.group ? 'ask.unfit_any' : 'ask.unfit_all')}</span>
                    </span>
                  ))}
                </div>
                {unfitHidden > 0 && (
                  <button type="button" className="k-press" onClick={() => setUnfitOpen(!unfitOpen)}
                          style={{ alignSelf: 'flex-start', display: 'flex', alignItems: 'center', gap: 'var(--sp-1)', height: 'var(--h-chip)',
                                   padding: 0, border: 0, background: 'transparent', font: 'inherit', fontSize: 'var(--fs-6)', color: 'var(--accent)' }}>
                    {unfitOpen ? `▾ ${t('ask.unfit_fold')}` : `▸ ${t('ask.unfit_more', { n: unfitHidden })}`}
                  </button>
                )}
              </div>
            )}
          </div>

          {label(t('ggs.field.format'))}
          <Select value={gtype} onChange={setGtype} options={gtypeChoices()} />

          {label(t('ggs.field.time_control'))}
          {/* Seven choices do not fit side by side in every language: a list, as for a local game. */}
          <Select value={time} onChange={setTime} options={clocks} />

          {label(t('ggs.rated'))}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
            <span style={{ display: 'flex', alignItems: 'center', height: 'var(--h-ctrl)', opacity: reg ? 1 : 0.4 }}>
              <Toggle checked={rated} onChange={(v) => reg && setRated(v)} />
            </span>
            {!reg && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('ask.unregistered')}</span>}
          </div>

          {label(t('ask.times'))}
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
            <div style={{ display: 'flex' }}>
              <Segmented value={repeat ? 'n' : '1'} onChange={(v) => setRepeat(v === 'n')}
                         options={[{ value: '1', label: t('ask.once') }, { value: 'n', label: t('ask.repeat') }]} />
            </div>
            {repeat && <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)',
                            color: 'var(--sub)', whiteSpace: 'nowrap' }}>
                <TextField value={maxGames} onChange={setMaxGames} numeric align="right" width={56} />
                <span>{t('ask.games_until')}</span>
                <TextField value={interval} onChange={setInterval} numeric align="right" width={48} />
                <span>{t('ask.interval')}</span>
              </div>
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('ask.zero_unlimited')}</span>
              <span style={{ display: 'flex', alignItems: 'center', height: 'var(--h-ctrl)' }}>
                <Toggle checked={autoAccept} onChange={setAutoAccept} label={t('ask.auto_accept')} />
              </span>
            </>}
          </div>
        </div>
      </Modal>
    </Overlay>
  );
}

type Opt = { key: string; name: string; rate: string; state: string; stateColor?: string; on: boolean; pick: () => void };

function OppRow({ name, rate, state, stateColor, on, act, onPick, onHover }: {
  name: string; rate: string; state: string; stateColor?: string; on: boolean; act: boolean;
  onPick: () => void; onHover: () => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (act) ref.current?.scrollIntoView({ block: 'nearest' }); }, [act]);
  return (
    <button ref={ref} type="button" tabIndex={-1} onClick={onPick} onMouseMove={onHover} role="option" aria-selected={act}
            className={'k-row' + (on ? ' k-on' : '')}
            style={{
              width: '100%', height: 'var(--h-field)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
              padding: '0 var(--sp-3)', border: 0, textAlign: 'left', fontSize: 'var(--fs-5)', whiteSpace: 'nowrap',
              background: act ? 'var(--card)' : 'transparent', color: 'var(--text)',
              ...marked(on), fontWeight: on ? 600 : 400,
            }}>
      <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>{name}</span>
      <span style={{ width: 64, textAlign: 'right', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{rate}</span>
      <span style={{ width: 56, textAlign: 'right', fontSize: 'var(--fs-6)', color: stateColor ?? 'var(--sub)' }}>{state}</span>
    </button>
  );
}

function matchRank(name: string, query: string): number {
  if (!query) return 0;
  const n = name.toLowerCase(), q = query.toLowerCase();
  return n === q ? 0 : n.startsWith(q) ? 1 : 2;
}

/**
 * The opponent field: shut, it shows who is picked; open, a search box and the list drop below it
 * (above it when there is no room). It sits inside the sheet's scrolling body, so the list is
 * placed on the window and closes when anything around it scrolls.
 */
function OppPicker({ label, rate, state, stateColor, query, onQuery, items, empty }: {
  label: string; rate: string; state: string; stateColor?: string;
  query: string; onQuery: (v: string) => void;
  items: Opt[]; empty: string;
}) {
  const [box, setBox] = useState<{ left: number; width: number; top?: number; bottom?: number } | null>(null);
  // The row Enter would pick: while searching, the best-matching player; otherwise who is picked now.
  const start = query ? Math.max(0, items.findIndex((o) => o.key !== '')) : Math.max(0, items.findIndex((o) => o.on));
  const [moved, setMoved] = useState<{ query: string; at: number } | null>(null);
  const act = moved && moved.query === query ? Math.min(moved.at, items.length - 1) : start;
  const choose = (o: Opt | undefined) => { if (!o) return; o.pick(); shut(); field.current?.focus(); };
  const keys = (e: React.KeyboardEvent) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const n = items.length;
      if (n) setMoved({ query, at: (act + (e.key === 'ArrowDown' ? 1 : n - 1)) % n });
    } else if (e.key === 'Enter') {
      e.preventDefault();
      choose(items[act]);
    }
  };
  const field = useRef<HTMLButtonElement>(null);
  const pop = useRef<HTMLDivElement>(null);
  const shut = () => { setBox(null); setMoved(null); onQuery(''); };
  const open = () => {
    const r = field.current?.getBoundingClientRect();
    if (!r) return;
    const below = window.innerHeight - r.bottom;
    setBox(below >= 300 || below >= r.top
      ? { left: r.left, width: r.width, top: r.bottom + 4 }
      : { left: r.left, width: r.width, bottom: window.innerHeight - r.top + 4 });
  };
  // A press outside or Escape closes it like every other popover; being placed on the window, it
  // also closes when anything around it scrolls or the window is resized.
  useDismiss(!!box, shut, pop, field);
  useEffect(() => {
    if (!box) return;
    pop.current?.querySelector('input')?.focus();
    const moved = (e: Event) => { if (!pop.current?.contains(e.target as Node)) shut(); };
    window.addEventListener('scroll', moved, true);
    window.addEventListener('resize', shut);
    return () => {
      window.removeEventListener('scroll', moved, true);
      window.removeEventListener('resize', shut);
    };
  });
  return (
    <>
      <button ref={field} type="button" className="k-press k-input" aria-haspopup="listbox" aria-expanded={!!box}
              onClick={() => (box ? shut() : open())}
              onKeyDown={(e) => { if (!box && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) { e.preventDefault(); open(); } }}
              style={{
                width: '100%', height: 'var(--h-field)', padding: '0 var(--sp-3)', borderRadius: 'var(--r-3)',
                background: 'var(--bg)', border: '1px solid ' + (box ? 'var(--accent)' : 'var(--border)'),
                display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)',
                color: 'var(--text)', textAlign: 'left', whiteSpace: 'nowrap',
              }}>
        <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', fontWeight: 600 }}>{label}</span>
        {rate && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{rate}</span>}
        {state && <span style={{ fontSize: 'var(--fs-6)', color: stateColor ?? 'var(--sub)' }}>{state}</span>}
        <span style={{ color: 'var(--sub)', flex: 'none' }}>▾</span>
      </button>
      {box && (
        <div ref={pop} role="listbox" onKeyDown={keys} style={{
          position: 'fixed', zIndex: 60, left: box.left, width: box.width, top: box.top, bottom: box.bottom,
          ...popoverStyle,
        }}>
          <div style={{ padding: 'var(--sp-1)', borderBottom: '1px solid var(--border-weak)', display: 'flex' }}>
            <TextField value={query} onChange={onQuery} placeholder={t('ask.search')} />
          </div>
          <div className="k-scroll" style={{ maxHeight: 'calc(var(--h-field) * 6)' }}>
            {items.map((o, i) => (
              <OppRow key={o.key || '*'} name={o.name} rate={o.rate} state={o.state} stateColor={o.stateColor} on={o.on}
                      act={i === act} onPick={() => choose(o)} onHover={() => i !== act && setMoved({ query, at: i })} />
            ))}
            {empty && (
              <ListNote>
                {empty}
              </ListNote>
            )}
          </div>
        </div>
      )}
    </>
  );
}
