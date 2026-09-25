import React, { useEffect, useRef, useState } from 'react';
import { api, ggsApi, jsLog, onApp } from './api';
import type { ChatMsg, GameResult, GgsSnapshot, MatchView, UserRow } from './types';
import {
  clockChoices, gtypeChoices, clockOf, countDiscs, ggsMoveToIndex, gtypeLabel,
  fingerGroups, fingerValue, hasJapanese, normKey, parseCond, translate, useClocks,
  type ClockSide, type ClockView,
} from './ggs';
import { sqName } from './adapt';
import { t, useLang, tErr } from './i18n';
import { Col, Empty, EmptyBoard, EmptyState, List, Modal, Note, Overlay, Section, TableHead, TableRow, picked } from './components/layout';
import { Button, Segmented, Select, TextArea, TextField, Toggle } from './components/primitives';
import { Strength } from './components/strength';
import { Confirm, PickOne } from './Dialogs';
import { IconButton } from './components/Icons';
import {
  Bubble, ConsoleLog, DayMark, FormulaEditor, FormulaView, MatchRow, PlayerRow, RateRow, Tag,
  type Cond, type Match, type NavId,
} from './components/ggs';
import { Board, type Cell, type EvalInfo } from './components/board';
import { EvalTrend, RateChart, ResultHead, ResultRow, StoneDot } from './components/data';
import { flipped, type Prefs } from './prefs';
import { logLinesOf } from './adapt';


export function GgsScreen({ nav, snap, onNav, prefs, onKifu }: {
  nav: NavId; snap: GgsSnapshot | null; onNav: (id: NavId) => void; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  useLang();
  if (nav === 'ggs-login') return <GgsLogin />;
  if (!snap) return <EmptyState title={t('ggs.not_connected')} />;

  switch (nav) {
    case 'ggs-play': return <GgsPlay snap={snap} onNav={onNav} prefs={prefs} onKifu={onKifu} />;
    case 'ggs-lobby': return <GgsLobby snap={snap} onNav={onNav} />;
    case 'ggs-players': return <GgsUsers snap={snap} onNav={onNav} onKifu={onKifu} />;
    case 'ggs-results': return <GgsResults snap={snap} onKifu={onKifu} />;
    case 'ggs-chat': return <GgsChat snap={snap} />;
    case 'ggs-standby': return <GgsStandby snap={snap} />;
    case 'ggs-console': return <GgsConsole snap={snap} />;
    case 'ggs-settings': return <GgsSettings snap={snap} />;
    default: return null;
  }
}

function Field({ label, children, stretch }: { label: string; children: React.ReactNode; stretch?: boolean }) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
                    alignSelf: 'start',
                    maxWidth: '100%', minWidth: 0,
                    alignItems: stretch ? 'stretch' : 'flex-start' }}>
      <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{label}</span>
      {children}
    </label>
  );
}

function GgsLogin() {
  const [user, setUser] = useState('');
  const [pw, setPw] = useState('');
  const [status, setStatus] = useState('');

  const connect = async () => {
    setStatus(t('ggs.login.connecting'));
    try {
      await ggsApi.connect(user, pw);
      setStatus('');
    } catch (e) {
      setStatus(tErr(e));
    }
  };

  return (
    <div style={{ flex: 1, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)' }}>
      <div style={{
        width: 'var(--w-modal)', borderRadius: 'var(--r-4)', background: 'var(--panel)',
        border: '1px solid var(--border)', padding: 22,
        display: 'flex', flexDirection: 'column', gap: 'var(--sp-3h)',
      }}>
        <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{t('ggs.login.title')}</div>
        <Note>
          <span style={{ fontFamily: 'var(--ff-mono)' }}>skatgame.net:5000</span>
          {' '}{t('ggs.login.note')}
        </Note>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          <LoginField label={t('ggs.login.username')}>
            <TextField value={user} onChange={setUser} />
          </LoginField>
          <LoginField label={t('ggs.login.password')}>
            <TextField value={pw} password onChange={setPw} />
          </LoginField>
        </div>
        <Button size="field" variant="primary" className="k-wide"
                onClick={() => void connect()}>{t('ggs.login.submit')}</Button>
        <div style={{
          fontSize: 'var(--fs-6)', minHeight: 16,
          color: status === t('ggs.login.connecting') ? 'var(--sub)' : 'var(--bad)',
        }}>{status}</div>
      </div>
    </div>
  );
}

function LoginField({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 5, alignItems: 'stretch' }}>
      <span style={{
        fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)',
      }}>{label}</span>
      {children}
    </label>
  );
}

export function GgsConsole({ snap }: { snap: GgsSnapshot }) {
  const [cmd, setCmd] = useState('');
  const [dir, setDir] = useState<'all' | 'out' | 'in'>('all');
  const [from, setFrom] = useState(0);
  const [note, setNote] = useState('');
  const say = (msg: string) => { setNote(msg); window.setTimeout(() => setNote(''), 2500); };
  const send = () => {
    const c = cmd.trim();
    if (!c) return;
    void ggsApi.raw(c);
    setCmd('');
  };
  const all = logLinesOf(snap.log);
  const shown = all.slice(Math.min(from, all.length))
    .filter((l) => dir === 'all' || l.dir === dir);
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', padding: '0 var(--sp-4)' }}>
        <Section title={t('ggs.console.title')} aside={<>
          <Segmented value={dir} onChange={setDir} options={[
            { value: 'all', label: t('ggs.filter.all') },
            { value: 'out', label: t('ggs.console.dir_out') },
            { value: 'in', label: t('ggs.console.dir_in') },
          ]} />
          <Button onClick={() => setFrom(all.length)} disabled={!all.length}>{t('ggs.console.clear')}</Button>
          <Button disabled={!shown.length}
                  onClick={() => void ggsApi.saveLog(
                    shown.map((l) => (l.dir === 'out' ? '› ' : '') + l.text).join('\n') + '\n',
                  )
                    .catch((e) => say(t('ggs.console.save_failed', { error: tErr(e) })))}>
            {t('ggs.console.save')}
          </Button>
          {note && <span style={{
            fontSize: 'var(--fs-6)', letterSpacing: 0, color: 'var(--bad)',
          }}>{note}</span>}
        </>} />
        <ConsoleLog lines={shown} />
      </div>
      <div style={{
        flex: 'none', display: 'flex', gap: 'var(--sp-2)', alignItems: 'center',
        padding: 'var(--sp-3) var(--sp-4)', borderTop: '1px solid var(--border-weak)',
      }}>
        <TextField mono value={cmd} onChange={setCmd} onEnter={send}
                   placeholder={t('ggs.console.placeholder')} />
        <Button size="field" onClick={send}>{t('ggs.send')}</Button>
      </div>
    </div>
  );
}


const PROVISIONAL_DEV = 100;

const wantsTranslation = (c: ChatMsg, login: string): boolean =>
  c.from !== login && !hasJapanese(c.text) && /[a-zA-Z]{2,}/.test(c.text);

const trKey = (c: ChatMsg): string => c.from + '|' + c.text;

export function GgsChat({ snap }: { snap: GgsSnapshot }) {
  const [thread, setThread] = useState('.chat');
  const [text, setText] = useState('');
  const [autoJa, setAutoJa] = useState(true);
  const [pick, setPick] = useState(false);
  const [toEn, setToEn] = useState(false);
  const [trs, setTrs] = useState<Record<string, string>>({});
  const pending = useRef<Set<string>>(new Set());
  const box = useRef<HTMLDivElement>(null);

  const threads = new Map<string, { last: ChatMsg; n: number }>();
  threads.set('.chat', { last: { chan: '.chat', from: '', text: '', at: 0, thread: '.chat' }, n: 0 });
  for (const c of snap.chat) {
    const key = c.thread || c.chan || c.from;
    threads.set(key, { last: c, n: (threads.get(key)?.n ?? 0) + 1 });
  }
  const cur = threads.has(thread) ? thread : '.chat';
  const sorted = [...threads.entries()].sort((a, b) =>
    a[0] === '.chat' ? -1 : b[0] === '.chat' ? 1 : b[1].last.at - a[1].last.at);
  const msgs = snap.chat.filter((c) => (c.thread || c.chan || c.from) === cur);

  const login = snap.login;
  const count = msgs.length;
  useEffect(() => {
    const b = box.current;
    if (b) b.scrollTop = b.scrollHeight;
  }, [count, cur]);

  useEffect(() => {
    if (!autoJa) return;
    for (const c of msgs) {
      if (!wantsTranslation(c, login)) continue;
      const key = trKey(c);
      if (key in trs || pending.current.has(key)) continue;
      pending.current.add(key);
      translate(c.text, 'ja')
        .then((tr) => setTrs((prev) => ({ ...prev, [key]: tr && tr !== c.text ? tr : '' })))
        .catch(() => setTrs((prev) => ({ ...prev, [key]: '' })));
    }
  }, [msgs, autoJa, trs, login]);

  const send = async () => {
    let msg = text.trim();
    if (!msg) return;
    setText('');
    if (toEn && hasJapanese(msg)) {
      try { msg = (await translate(msg, 'en')) || msg; } catch (e) { jsLog('translation failed: ' + e); }
    }
    ggsApi.chat(cur, msg).catch((e) => jsLog(String(e)));
  };

  const rows: { c: ChatMsg; day: string; dayHead: boolean; head: boolean }[] = [];
  let lastFrom = '', lastDay = '';
  for (const c of msgs) {
    const day = c.at ? new Date(c.at * 1000).toLocaleDateString('ja-JP',
      { month: 'long', day: 'numeric', weekday: 'short' }) : '';
    const dayHead = !!day && day !== lastDay;
    if (dayHead) { lastDay = day; lastFrom = ''; }
    const head = c.from !== lastFrom;
    lastFrom = c.from;
    rows.push({ c, day, dayHead, head });
  }

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <ChatList sorted={sorted} cur={cur} onThread={setThread} onPick={setPick} />

      <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
        <div style={{
          flex: 'none', display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)',
          padding: 'var(--sp-3) var(--sp-4)', borderBottom: '1px solid var(--border-weak)',
        }}>
          <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>
            {cur === '.chat' ? t('ggs.chat.global') : cur}
          </span>
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
            {cur === '.chat' ? t('ggs.chat.to_everyone') : t('ggs.chat.to_person')}
          </span>
        </div>
        <div style={{
          flex: 'none', height: 'var(--h-field)', display: 'flex', alignItems: 'center',
          gap: 'var(--sp-4)', padding: '0 var(--sp-4)', borderBottom: '1px solid var(--border-weak)',
        }}>
          <Toggle checked={autoJa} onChange={setAutoJa} label={t('ggs.chat.translate_in')} />
          <Toggle checked={toEn} onChange={setToEn} label={t('ggs.chat.translate_out')} />
        </div>
        <div className="k-scroll" ref={box} style={{
          flex: 1, minHeight: 0, padding: 'var(--sp-4)',
          display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)',
        }}>
          {!rows.length && (
            <span style={{ margin: 'auto' }}>
              <Empty>{cur === '.chat' ? t('ggs.chat.empty_global') : t('ggs.chat.empty_thread')}</Empty>
            </span>
          )}
          {rows.map(({ c, day, dayHead, head }, i) => (
            <React.Fragment key={i}>
              {dayHead && <DayMark>{day}</DayMark>}
              <Bubble showName={head} m={{
                from: c.from, mine: c.from === login, at: clockOf(c.at), body: c.text,
                ja: autoJa && wantsTranslation(c, login) ? (trs[trKey(c)] || undefined) : undefined,
              }} />
            </React.Fragment>
          ))}
        </div>
        <div style={{
          flex: 'none', display: 'flex', gap: 'var(--sp-2)', alignItems: 'center',
          padding: 'var(--sp-3) var(--sp-4)', borderTop: '1px solid var(--border-weak)',
        }}>
          <TextField value={text} onChange={setText} onEnter={() => void send()}
                     placeholder={t('ggs.chat.placeholder')} />
          <Button size="field" variant="primary" onClick={() => void send()}>{t('ggs.send')}</Button>
        </div>
      </div>
      {pick && (
        <PickOne title={t('ggs.chat.pick_title')}
                 body={<span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                   {t('ggs.chat.pick_note')}
                 </span>}
                 options={snap.users.filter((u) => u.name !== login).map((u) => [u.name, u.name] as [string, string])}
                 onCancel={() => setPick(false)}
                 onOk={(who) => { setThread(who); setPick(false); }} />
      )}
    </div>
  );
}

function GgsLobby({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
  const [opp, setOpp] = useState('');
  const [gtype, setGtype] = useState('s8r16');
  const [time, setTime] = useState('00:15:00');
  const noRated = useNoRated();
  const calibrated = useCalibrated();
  const [rated, setRated] = useState(true);
  const [info, setInfo] = useState('');
  useEffect(() => { void ggsApi.listMatches().catch(() => {}); }, []);

  const games = snap.ongoing.filter((o) => !o.mine);
  const names = snap.users.filter((u) => u.name !== snap.login).map((u) => u.name);

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <div className="k-scroll" style={{ flex: 1, minWidth: 0, padding: 'var(--sp-4) var(--sp-2) 0' }}>
        <Section title={t('ggs.lobby.ongoing_title')}
                 aside={games.length ? t('ggs.lobby.game_count', { n: games.length }) : undefined}>
          {!games.length && <Empty>{t('ggs.lobby.no_ongoing')}</Empty>}
          <List>
          {games.map((o) => (
            <Row key={o.id}
                 title={t('ggs.vs', { a: o.names[0] || '?', b: o.names[1] || '?' })}
                 sub={gtypeLabel(o.gtype)}
                 actions={
                   <Button size="row" variant={o.watching ? 'danger' : 'primary'}
                           onClick={() => {
                             const on = !o.watching;
                             void ggsApi.watch(o.id, on);
                             if (on) { focusMatch(o.id); onNav('ggs-play'); }
                           }}>
                     {o.watching ? t('ggs.stop_observing') : t('ggs.observe')}
                   </Button>} />
          ))}
          </List>
        </Section>

        <Section title={t('ggs.match_requests')}>
          {!snap.offers.length && <Empty>{t('ggs.lobby.no_requests')}</Empty>}
          <List>
          {snap.offers.map((o) => {
            const who = o.names.filter((n) => n !== snap.login);
            return (
              <React.Fragment key={o.id}>
              <Row title={who.join(t('ggs.and_separator')) || '?'}
                   tag={o.incoming ? t('ggs.tag.to_me') : undefined}
                   tagTone={o.incoming ? 'bad' : undefined}
                   alert={o.incoming}
                   sub={`${gtypeLabel(o.gtype)} · ${o.time || '?'}${o.rated ? ' · ' + t('ggs.rated') : ''}`}
                   actions={<>
                     <Button size="row" onClick={() => setInfo(info === o.id ? '' : o.id)}>{t('ggs.lobby.raw_info')}</Button>
                     {o.incoming && <>
                       <Button size="row" variant="primary"
                               onClick={() => { focusMatch(o.id); void ggsApi.accept(o.id); }}>{t('ggs.lobby.accept')}</Button>
                       <Button size="row" variant="danger" onClick={() => void ggsApi.decline(o.id)}>{t('ggs.lobby.decline')}</Button>
                     </>}
                   </>} />
              {info === o.id && (
                <div style={{
                  padding: 'var(--sp-2) var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
                  fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-7)', color: 'var(--sub)',
                  lineHeight: 1.7, wordBreak: 'break-all',
                }}>{o.raw || t('ggs.lobby.no_raw')}</div>
              )}
              </React.Fragment>
            );
          })}
          </List>
        </Section>
      </div>

      <aside className="k-scroll" style={{
        width: 'var(--w-dock)', flex: 'none', borderLeft: '1px solid var(--border)',
        padding: 'var(--sp-4) var(--sp-2) 0', minHeight: 0,
      }}>
        <Section title={t('ggs.lobby.request_title')}>
          <Field label={t('ggs.field.opponent')}>
            <Select value={names.includes(opp) ? opp : ''} onChange={setOpp}
                    options={[['', t('ggs.opponent_any')], ...names.map((n) => [n, n] as [string, string])]} />
          </Field>
          <Field label={t('ggs.field.format')}><Select value={gtype} onChange={setGtype} options={gtypeChoices()} /></Field>
          <Field label={t('ggs.field.time_control')}><Select value={time} onChange={setTime} options={clockChoices()} /></Field>
          <Field label={t('ggs.rated')}>
            <Segmented value={rated && !noRated ? 'on' : 'off'} disabled={noRated}
                       onChange={(v) => setRated(v === 'on')}
                       options={[{ value: 'on', label: t('ggs.on') },
                                 { value: 'off', label: t('ggs.off') }]} />
            {noRated && <Note>{t('ggs.no_rated_note')}</Note>}
          </Field>
          <Button size="field" variant="primary" disabled={!calibrated}
                  onClick={() => void ggsApi.ask(gtype, time, opp, rated)}>
            {opp ? t('ggs.lobby.ask') : t('ggs.lobby.open_request')}
          </Button>
          {!calibrated && <Note>{calibNote()}</Note>}
          <Note>{t('ggs.lobby.synchro_note')}</Note>
        </Section>

        <Section title={t('ggs.adjourned_games')}
                 aside={<Button onClick={() => void ggsApi.listStored()}>{t('ggs.refresh')}</Button>}>
          {!snap.stored.length && <Empty>{t('ggs.lobby.no_adjourned')}</Empty>}
          <List>
          {snap.stored.map((x) => (
            <Row key={x.id} title={x.opp || '?'} sub={gtypeLabel(x.gtype)}
                 actions={<Button size="row" variant="primary"
                                  onClick={() => void ggsApi.resumeStored(x.id)}>{t('ggs.lobby.resume')}</Button>} />
          ))}
          </List>
        </Section>
      </aside>
    </div>
  );
}

function Row({ title, sub, tag, tagTone, alert, actions, onClick, title2 }: {
  title: string; sub?: string; tag?: string;
  tagTone?: 'sub' | 'accent' | 'ok' | 'bad';
  alert?: boolean;
  actions?: React.ReactNode;
  onClick?: () => void;
  title2?: string;
}) {
  const Tag_ = onClick && !actions ? 'button' : 'div';
  return (
    <Tag_ {...(onClick && !actions
      ? { type: 'button' as const, className: 'k-row', onClick, title: title2 }
      : {})} style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', width: '100%',
      height: 'var(--h-row2)', flex: 'none', padding: '0 var(--sp-4)',
      border: 0, borderRadius: 0,
      borderBottom: '1px solid var(--border-weak)',
      background: alert ? 'color-mix(in srgb, var(--bad) 8%, transparent)' : 'transparent',
      boxShadow: alert ? 'inset 2px 0 0 var(--bad)' : undefined,
      textAlign: 'left',
      color: 'var(--text)', cursor: onClick && !actions ? 'pointer' : undefined,
    }}>
      <span style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)' }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{title}</span>
          {tag && <Tag tone={tagTone ?? 'accent'}>{tag}</Tag>}
        </span>
        {sub && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)',
                               overflow: 'hidden', textOverflow: 'ellipsis',
                               whiteSpace: 'nowrap' }}>{sub}</span>}
      </span>
      {actions}
    </Tag_>
  );
}

function useNoRated(): boolean {
  const [no, setNo] = useState(false);
  useEffect(() => { ggsApi.noRated().then(setNo).catch(() => {}); }, []);
  return no;
}

function useCalibrated(): boolean {
  const [ok, setOk] = useState(true);
  useEffect(() => {
    let alive = true;
    const load = () => void api.localThreads()
      .then((t) => { if (alive) setOk(t.nps != null); })
      .catch(() => {});
    load();
    const off = onApp('resources-changed', load);
    return () => { alive = false; void off.then((f) => f()); };
  }, []);
  return ok;
}

const calibNote = (): string => t('ggs.calib_note');

function GgsStandby({ snap }: { snap: GgsSnapshot }) {
  const sb = snap.standby;
  const st = snap.standby_stats;
  const [opp, setOpp] = useState(sb.opponent);
  const [gtype, setGtype] = useState(sb.gtype || 's8r16');
  const [time, setTime] = useState(sb.time || '00:15:00');
  const [maxGames, setMaxGames] = useState(sb.max_games);
  const [interval, setInterval] = useState(sb.interval_secs || 20);
  const [autoAccept, setAutoAccept] = useState(sb.auto_accept);
  const noRated = useNoRated();
  const calibrated = useCalibrated();
  const [rated, setRated] = useState(sb.rated);

  const names = snap.users.filter((u) => u.name !== snap.login).map((u) => u.name);
  const playing = snap.matches.some((m) => !m.over);
  const state = sb.enabled
    ? (playing ? t('ggs.standby.state_playing') : t('ggs.standby.state_waiting'))
    : t('ggs.standby.state_off');

  const toggle = () => void ggsApi.setStandby({
    enabled: !sb.enabled, auto_accept: autoAccept, rated: rated && !noRated, opponent: opp.trim(),
    gtype, time, max_games: maxGames, interval_secs: interval,
  });

  useEffect(() => {
    if (snap.login) ggsApi.finger(snap.login).catch(() => {});
  }, [snap.login]);

  const form = (key: 'accept' | 'decline'): string =>
    (snap.fingers[snap.login]?.fields
      .find(([k]) => k.replace(/\s+/g, '').replace(/\(.*\)/, '') === key)?.[1] ?? '')
      .replace(/^\s*:\s*/, '').trim();

  const online = snap.conn === 'online';
  const login = snap.login;
  useEffect(() => { if (online && login) ggsApi.finger(login).catch(() => {}); }, [online, login]);
  const [formSaid, setFormSaid] = useState('');
  const saveForm = async (kind: 'aform' | 'dform', expr: string) => {
    try {
      await ggsApi.setFormula(kind, expr);
    } catch (e) {
      setFormSaid(t('ggs.settings.formula_failed', { error: tErr(e) }));
      window.setTimeout(() => setFormSaid(''), 2500);
      return;
    }
    if (login) ggsApi.finger(login).catch(() => {});
  };

  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <Section title={t('ggs.standby.title')}
               aside={<span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
                 <Tag tone={sb.enabled ? 'ok' : 'sub'}>{state}</Tag>
                 <Stat v={st.games} label={t('ggs.stat.games')} />
                 <Stat v={st.wins} label={t('ggs.stat.wins')} color="var(--ok)" />
                 <Stat v={st.losses} label={t('ggs.stat.losses')} color="var(--bad)" />
                 <Stat v={st.draws} label={t('ggs.stat.draws')} />
                 <Stat v={`${st.diff_sum > 0 ? '+' : ''}${st.diff_sum}`} label={t('ggs.stat.disc_diff')} />
                 <Button variant={sb.enabled ? 'danger' : 'primary'}
                         disabled={!sb.enabled && !calibrated} onClick={toggle}>
                   {sb.enabled ? t('ggs.standby.stop') : t('ggs.standby.start')}
                 </Button>
               </span>}>
        <div style={{
          display: 'grid', gap: 'var(--sp-4)',
          gridTemplateColumns: 'repeat(auto-fit, minmax(240px, 1fr))',
        }}>
          <Field stretch label={t('ggs.field.opponent')}>
            <Select value={names.includes(opp) ? opp : ''} onChange={setOpp}
                    options={[['', t('ggs.opponent_any')], ...names.map((n) => [n, n] as [string, string])]} />
          </Field>
          <Field stretch label={t('ggs.field.format')}><Select value={gtype} onChange={setGtype} options={gtypeChoices()} /></Field>
          <Field stretch label={t('ggs.field.time_control')}><Select value={time} onChange={setTime} options={clockChoices()} /></Field>
          <Field stretch label={t('ggs.standby.max_games')}>
            <TextField numeric align="right" value={String(maxGames)}
                       onChange={(x) => setMaxGames(+x || 0)} />
          </Field>
          <Field stretch label={t('ggs.standby.interval')}>
            <TextField numeric align="right" value={String(interval)}
                       onChange={(x) => setInterval(+x || 0)} />
          </Field>
          <Field label={t('ggs.rated')}>
            <Segmented value={rated && !noRated ? 'on' : 'off'} disabled={noRated}
                       onChange={(v) => setRated(v === 'on')}
                       options={[{ value: 'on', label: t('ggs.on') },
                                 { value: 'off', label: t('ggs.off') }]} />
            {noRated && <Note>{t('ggs.no_rated_note')}</Note>}
          </Field>
        </div>
        <span style={{ width: 300, display: 'block' }}>
          <Toggle checked={autoAccept} onChange={setAutoAccept} label={t('ggs.standby.auto_accept')} />
        </span>
        <Note>{t('ggs.standby.note')}</Note>
        {!sb.enabled && !calibrated && <Note>{calibNote()}</Note>}
      </Section>

      <Section title={t('ggs.standby.formula_title')}>
        <Note>{t('ggs.standby.formula_note')}</Note>
        {online ? <>
          <FormulaField key={'a:' + form('accept')}
                        label={t('ggs.finger.accept')} src={form('accept')}
                        onSave={(x) => void saveForm('aform', x)} />
          <FormulaField key={'d:' + form('decline')}
                        label={t('ggs.finger.decline')} src={form('decline')}
                        onSave={(x) => void saveForm('dform', x)} />
        </> : (
          <Note>{t('ggs.settings.offline_note')}</Note>
        )}
        {formSaid && <Note>{formSaid}</Note>}
      </Section>
    </div>
  );
}

function Stat({ v, label, color }: { v: number | string; label: string; color?: string }) {
  return (
    <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', letterSpacing: 0 }}>
      <b style={{ color: color ?? 'var(--text)', fontWeight: 600 }}>{v}</b>
      <span style={{ marginLeft: 2 }}>{label}</span>
    </span>
  );
}

let wantedMatch = '';
export function focusMatch(id: string) { wantedMatch = id; }

function GgsPlay({ snap, onNav, prefs, onKifu }: {
  snap: GgsSnapshot; onNav: (id: NavId) => void; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  const [sel, setSel] = useState(() => { const w = wantedMatch; wantedMatch = ''; return w; });
  const clock = useClocks(snap.matches);

  const groups = new Map<string, MatchView[]>();
  for (const m of snap.matches) groups.set(m.base, [...(groups.get(m.base) ?? []), m]);
  const fresh = (k: string) => Math.max(...groups.get(k)!.map((m) => m.order));
  const keys = [...groups.keys()].sort((a, b) => {
    const mine = (k: string) => (groups.get(k)!.some((m) => m.my_color) ? 0 : 1);
    return mine(a) - mine(b) || fresh(b) - fresh(a);
  });
  const cur = groups.has(sel) ? sel : keys[0] ?? '';
  const pair = groups.get(cur);

  if (!groups.size) {
    return (
      <EmptyState title={t('ggs.play.empty_title')}
                  visual={<EmptyBoard />}
                  body={t('ggs.play.empty_body')}
                  actions={<>
                    <Button variant="primary" onClick={() => onNav('ggs-lobby')}>{t('ggs.play.to_lobby')}</Button>
                    <Button onClick={() => onNav('ggs-standby')}>{t('ggs.play.to_standby')}</Button>
                    <Button onClick={() => void ggsApi.listMatches()}>{t('ggs.refresh')}</Button>
                  </>} />
    );
  }

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <aside className="k-scroll" style={{
        width: 'var(--w-list)', flex: 'none', borderRight: '1px solid var(--border)', minHeight: 0,
      }}>
        {keys.map((key) => (
          <MatchRow key={key} m={matchRowOf(groups.get(key)!, key)}
                    active={key === cur} onSelect={() => setSel(key)}
                    onClose={() => {
                      void ggsApi.closeMatch(key);
                      if (key === sel) setSel('');
                    }} />
        ))}
      </aside>

      <div className="k-scroll" style={{ flex: 1, minWidth: 0, minHeight: 0, padding: 'var(--sp-3)' }}>
        {pair && <MatchActions id={cur} pair={pair} />}
        <div style={{ display: 'flex', gap: 'var(--sp-3)', flexWrap: 'wrap', justifyContent: 'center' }}>
          {pair?.map((m, i) => (
            <MatchBoard key={m.id} snap={snap} m={m} clock={clock} prefs={prefs} onKifu={onKifu}
                        face={(pair?.length ?? 1) > 1 ? i + 1 : undefined} />
          ))}
        </div>
      </div>
    </div>
  );
}

function MatchActions({ id, pair }: { id: string; pair: MatchView[] }) {
  const [ask, setAsk] = useState<'' | 'resign' | 'break'>('');
  const mine = pair.some((m) => m.my_color);
  const live = !pair.every((m) => m.over);
  if (!mine || !live) return null;
  const send = (verb: 'resign' | 'break') => { setAsk(''); void ggsApi.matchCmd(id, verb); };
  const rated = pair.find((m) => m.rated != null)?.rated ?? null;
  const adjournBody = rated === true ? t('ggs.play.adjourn_body_rated')
    : rated === false ? t('ggs.play.adjourn_body_unrated')
    : t('ggs.play.adjourn_body_unknown');
  return (
    <div style={{
      display: 'flex', gap: 'var(--sp-2)', justifyContent: 'flex-end',
      paddingBottom: 'var(--sp-3)',
    }}>
      <Button title={t('ggs.play.adjourn_hint')}
              onClick={() => setAsk('break')}>{t('ggs.adjourn')}</Button>
      <Button variant="danger" title={t('ggs.play.resign_hint')}
              onClick={() => setAsk('resign')}>{t('ggs.resign')}</Button>
      {ask === 'break' && (
        <Confirm title={t('ggs.play.adjourn_confirm')} ok={t('ggs.play.adjourn_ok')}
                 body={<>{adjournBody}</>}
                 onCancel={() => setAsk('')} onOk={() => send('break')} />
      )}
      {ask === 'resign' && (
        <Confirm title={t('ggs.play.resign_confirm')} ok={t('ggs.play.resign_ok')} danger
                 body={<>{t('ggs.play.resign_body')}</>}
                 onCancel={() => setAsk('')} onOk={() => send('resign')} />
      )}
    </div>
  );
}

// GGS scores a synchro match as the mean of its boards, so the match has no
// result until every board is in -- and taking the first board that finished
// would show one half of it.
function matchResult(g: MatchView[]): string | undefined {
  if (!g.every((x) => x.over)) return undefined;
  const diffs = g.map((x) => Number.parseFloat(x.result));
  if (diffs.length !== g.length || diffs.some((v) => !Number.isFinite(v))) return undefined;
  const mean = diffs.reduce((a, b) => a + b, 0) / diffs.length;
  return (mean > 0 ? '+' : '') + mean.toFixed(2);
}

function matchRowOf(g: MatchView[], key: string): Match {
  const m = g[0];
  const mine = g.some((x) => x.my_color);
  return {
    id: key, mine, live: !g.every((x) => x.over),
    opponent: m.opp_name,
    black: m.players[0]?.name ?? '?', white: m.players[1]?.name ?? '?',
    kind: gtypeLabel(m.gtype),
    result: matchResult(g),
    ended: g.map((x) => x.ended).find(Boolean) || undefined,
    leftBy: g.map((x) => x.left_by).find(Boolean) || undefined,
  };
}

function MatchBoard({ snap, m, clock, prefs, onKifu, face }: {
  snap: GgsSnapshot; m: MatchView; clock: (id: string, side: ClockSide) => ClockView; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
  face?: number;
}) {
  const observer = !m.my_color;
  const { black, white } = countDiscs(m.cells);
  const last = [...m.moves].reverse().map(ggsMoveToIndex).find((x) => x !== null) ?? null;
  const myRate = snap.my_ranks.find((r) => r.gtype === (m.gtype.includes('r') ? '8r' : '8'))?.rating;
  const myEval = m.last_eval != null
    ? t('ggs.play.my_eval', {
        v: (m.last_from_book ? t('ggs.play.book_mark') + ' ' : '') + (m.last_eval > 0 ? '+' : '')
          + (m.last_eval_exact ? m.last_eval.toFixed(0) : m.last_eval.toFixed(1))
          + (m.last_eval_exact ? ' ' + t('ggs.play.solve_mark') : ''),
      })
    : undefined;

  const oppEval = !observer && m.opp_eval != null
    ? t('ggs.play.opp_eval', {
        v: (m.opp_eval > 0 ? '+' : '') + m.opp_eval.toFixed(1)
          + (m.opp_secs_used != null ? ` · ${m.opp_secs_used.toFixed(0)}s` : ''),
      })
    : undefined;

  const trend = m.eval_series.map((p) => ({
    x: p.n,
    mine: p.mine ? p.eval : null,
    opp: p.mine || p.eval == null ? null : -p.eval,
  }));

  const busyEval: Record<number, EvalInfo> | undefined =
    m.busy && m.busy !== 'ponder' && m.busy_best != null
      ? {
          [m.busy_best]: {
            score: m.busy_eval ?? 0,
            src: m.busy === 'solve' ? { exact: true }
              : m.busy === 'select' ? { select: true }
              : { depth: m.busy_depth },
            best: true,
          },
        }
      : undefined;

  const top = observer && m.players.length >= 2
    ? { name: m.players[0].name, rate: m.players[0].rating, color: m.players[0].color, side: 'p0' as const }
    : { name: m.opp_name, rate: m.opp_rating, color: m.my_color === 'black' ? 'white' as const : 'black' as const, side: 'opp' as const };
  const bottom = observer && m.players.length >= 2
    ? { name: m.players[1].name, rate: m.players[1].rating, color: m.players[1].color, side: 'p1' as const }
    : { name: snap.login, rate: myRate != null ? myRate.toFixed(1) : '', color: m.my_color as 'black' | 'white', side: 'my' as const };

  return (
    <div style={{
      flex: '1 1 300px', minWidth: 260, maxWidth: 'min(460px, calc(100vh - 280px))',
      display: 'flex', flexDirection: 'column',
      background: 'var(--panel)', borderRadius: 'var(--r-4)', padding: 'var(--sp-3)',
    }}>
      {face !== undefined && (
        <div style={{
          height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
          fontSize: 'var(--fs-7)', color: 'var(--sub)',
        }}>
          <span>{t('ggs.play.board_n', { n: face })}</span>
          {m.my_color && (
            <span>{m.my_color === 'black' ? t('ggs.play.my_color_black') : t('ggs.play.my_color_white')}</span>
          )}
        </div>
      )}
      <PlayerRow color={top.color === 'black' ? 'b' : 'w'} name={top.name || '?'}
                 rate={top.rate ? +top.rate : undefined}
                 meta={oppEval}
                 clock={clock(m.id, top.side).text} active={clock(m.id, top.side).cls === 'turn'} />
      <Board cells={m.cells as Cell[]} last={last} disabled
             evals={busyEval}
             next={m.busy === 'ponder' ? m.busy_predict : null}
             legal={busyEval ? Object.keys(busyEval).map(Number)
                    : m.busy === 'ponder' && m.busy_predict != null ? [m.busy_predict]
                    : []}
             coords={prefs.coords} grain={prefs.grain}
             flip={flipped(prefs.facing, m.my_color)} />
      <PlayerRow color={bottom.color === 'black' ? 'b' : 'w'} name={bottom.name || '?'}
                 rate={bottom.rate ? +bottom.rate : undefined}
                 meta={myEval}
                 clock={clock(m.id, bottom.side).text} active={clock(m.id, bottom.side).cls === 'turn'} />
      {!observer && <EvalTrend points={trend} />}
      <div style={{
        display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>
        <span style={{ color: 'var(--text)' }}>{black} – {white}</span>
        <span>{t('ggs.play.moves', { n: m.moves.length })}</span>
        {observer && m.watch_eval != null && (
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            {t('ggs.play.analysis')} <StoneDot color="b" />
            {m.watch_eval > 0 ? '+' : ''}{m.watch_eval.toFixed(1)}
            {m.watch_best ? ` (${m.watch_best})` : ''}
          </span>
        )}
        {m.busy === 'think' && (
          <span style={{ color: 'var(--accent)' }}>
            {m.busy_depth > 0
              ? t('ggs.play.thinking_depth', { n: m.busy_depth })
              : t('ggs.play.thinking')}
          </span>
        )}
        {m.busy === 'solve' && <span style={{ color: 'var(--accent)' }}>{t('ggs.play.solving')}</span>}
        {m.busy === 'select' && <span style={{ color: 'var(--accent)' }}>{t('ggs.play.selecting')}</span>}
        {m.busy === 'ponder' && (
          <span style={{ color: 'var(--sub)' }}>
            {m.busy_predict != null && m.busy_eval != null
              ? t('ggs.play.pondering_line', {
                  m: sqName(m.busy_predict),
                  v: (m.busy_eval > 0 ? '+' : '') + m.busy_eval.toFixed(1),
                })
              : m.busy_predict != null
                ? t('ggs.play.pondering_reply', { m: sqName(m.busy_predict) })
                : t('ggs.play.pondering')}
          </span>
        )}
        {!m.busy && snap.thinking === m.id && (
          <span style={{ color: 'var(--accent)' }}>{t('ggs.play.thinking')}</span>
        )}
        {m.ended === 'adjourned' && (
          <span style={{ color: 'var(--gold)' }}>
            {t('ggs.state.adjourned')}
            {m.left_by ? ' · ' + t('ggs.play.left_by', { who: m.left_by }) : ''}
          </span>
        )}
        {m.ended === 'aborted' && <span style={{ color: 'var(--sub)' }}>{t('ggs.state.aborted')}</span>}
        {m.ended === 'finished' && m.result && (
          <span style={{ color: 'var(--text)' }}>{t('ggs.play.finished_result', { result: m.result })}</span>
        )}
        <span style={{ marginLeft: 'auto' }} />
        <Button
                onClick={() => onKifu(m.opp_name
                                        ? t('ggs.play.game_with', { name: m.opp_name })
                                        : t('ggs.play.record_title'),
                                      m.ggf || m.moves.join(''), m.archive || undefined)}>
          {t('ggs.game_record')}
        </Button>
      </div>
    </div>
  );
}

function bothRates(u: UserRow | undefined): string {
  if (!u) return '';
  const m = /(\d+(?:\.\d+)?)@\s*(\d+(?:\.\d+)?)/.exec(u.raw || '');
  const r8 = m ? m[1] : (u.rating != null ? u.rating.toFixed(1) : '');
  const d8 = m ? Math.round(parseFloat(m[2])) : null;
  const parts: string[] = [];
  if (r8) parts.push(`${t('ggs.pool.normal')} ${r8}${d8 != null ? ` ±${d8}` : ''}`);
  if (u.rating_r != null) {
    parts.push(`${t('ggs.pool.random')} ${u.rating_r.toFixed(1)}`
      + `${u.dev_r != null ? ` ±${Math.round(u.dev_r)}` : ''}`);
  }
  return parts.join(' · ');
}

export function GgsSettings({ snap }: { snap: GgsSnapshot }) {
  const e = snap.engine;
  const calibrated = useCalibrated();
  const [saved, setSaved] = useState('');
  const say = (msg: string) => { setSaved(msg); window.setTimeout(() => setSaved(''), 2500); };
  const [levels, setLevels] = useState({ depth: e.depth, solve: e.solve, band: e.band });
  const threads = e.threads;
  const [ponder, setPonder] = useState(e.ponder);
  const [auto, setAuto] = useState(snap.auto_play);
  const [watch, setWatch] = useState(snap.watch_analysis);
  const [book, setBook] = useState(e.use_book);
  const [pace, setPace] = useState(e.pace === 'depth' ? 'depth' : 'fast');
  const byClock = pace !== 'depth';
  const [maxMove, setMaxMove] = useState(e.max_move_secs);
  const [reserve, setReserve] = useState(e.reserve_secs);
  const [budgetUse, setBudgetUse] = useState(e.budget_use);
  const [cores, setCores] = useState(0);
  useEffect(() => { api.activity().then((a) => setCores(a.cores)).catch(() => {}); }, []);

  const online = snap.conn === 'online';

  const apply = async () => {
    try {
      await ggsApi.setEngine(levels.depth, levels.solve, levels.band, ponder);
      await ggsApi.setPacing(pace, maxMove, reserve, budgetUse);
      await ggsApi.setAutoPlay(auto);
      await ggsApi.setWatchAnalysis(watch);
      await ggsApi.setUseBook(book);
    } catch (e) {
      say(t('ggs.settings.apply_failed', { error: tErr(e) }));
      return;
    }
    say(t('ggs.settings.applied'));
  };

  const first = useRef(true);
  useEffect(() => {
    if (first.current) { first.current = false; return; }
    if (!calibrated) return;
    const timer = setTimeout(() => void apply(), 400);
    return () => clearTimeout(timer);
  }, [levels.depth, levels.solve, levels.band, ponder, pace,
      maxMove, reserve, budgetUse, auto, watch, book]);


  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <div style={{ maxWidth: 720, display: 'flex', flexDirection: 'column' }}>
        <Section title={t('ggs.settings.strength')}>
          <Field label={t('ggs.settings.mode')}>
            <Segmented value={pace} onChange={setPace}
                       options={[{ value: 'fast', label: t('ggs.settings.by_clock') },
                                 { value: 'depth', label: t('ggs.settings.by_level') }]} />
          </Field>
          <Note>
            {byClock ? t('ggs.settings.by_clock_note') : t('ggs.settings.by_level_note')}
          </Note>
          {!byClock && (
            <span style={{ maxWidth: 340, display: 'block' }}>
              <Strength value={levels} onChange={setLevels} />
            </span>
          )}
          <Field label={t('ggs.settings.threads')}>
            <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>
              {threads === 0
                ? t('ggs.settings.threads_auto', { n: Math.max(1, Math.floor(cores / 2)) || '—' })
                : threads}
              {' · '}{t('ggs.settings.threads_hint')}
            </span>
          </Field>
          <Field label={t('ggs.settings.ponder')}>
            <Segmented value={ponder ? 'on' : 'off'}
                       onChange={(v) => setPonder(v === 'on')}
                       options={[{ value: 'on', label: t('ggs.on') },
                                 { value: 'off', label: t('ggs.off') }]} />
          </Field>
          <Note>{t('ggs.settings.ponder_note')}</Note>
        </Section>

        <Section title={t('ggs.settings.clock_use')}>
          {!byClock && (
            <Note>
              <b style={{ color: 'var(--text)' }}>{t('ggs.settings.level_mode_note')}</b>
            </Note>
          )}
          <Note>{t('ggs.settings.pacing_note')}</Note>
          {(
            <div style={{ display: 'flex', gap: 'var(--sp-4)' }}>
              <Field label={t('ggs.settings.max_move')}>
                <TextField numeric align="right" width={80} value={String(maxMove)}
                           onChange={(x) => setMaxMove(Math.max(0, +x || 0))} />
              </Field>
              <Field label={t('ggs.settings.reserve')}>
                <TextField numeric align="right" width={80} value={String(reserve)}
                           onChange={(x) => setReserve(Math.max(0, +x || 0))} />
              </Field>
              <Field label={t('ggs.settings.budget_use')}>
                <TextField numeric align="right" width={80} value={String(budgetUse)}
                           onChange={(x) => setBudgetUse(Math.max(0.1, +x || 2.5))} />
              </Field>
            </div>
          )}
          <Note>{t('ggs.settings.budget_note')}</Note>
        </Section>

        <Section title={t('ggs.book')}>
          {e.book_loaded
            ? <div><Segmented value={book ? 'on' : 'off'} onChange={(v) => setBook(v === 'on')}
                              options={[{ value: 'on', label: t('ggs.book_on') },
                                        { value: 'off', label: t('ggs.book_off') }]} /></div>
            : <Note>{t('ggs.settings.book_missing')}</Note>}
        </Section>

        <Section title={t('ggs.settings.behavior')}>
          <span style={{ width: 300, display: 'block' }}>
            <Toggle checked={auto} onChange={setAuto} label={t('ggs.settings.auto_play')} />
          </span>
          <span style={{ width: 300, display: 'block' }}>
            <Toggle checked={watch} onChange={setWatch} label={t('ggs.settings.watch_analysis')} />
          </span>
        </Section>

        <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', padding: '0 var(--sp-3) var(--sp-5)' }}>
          {saved && (
            <span style={{
              fontSize: 'var(--fs-6)',
              color: saved === t('ggs.settings.applied') ? 'var(--ok)' : 'var(--bad)',
            }}>{saved}</span>
          )}
          <span style={{ marginLeft: 'auto' }} />
          {!calibrated && <Note>{calibNote()}</Note>}
        </div>

        {online && (
          <Section title={t('ggs.settings.connection')}>
            <Note>{t('ggs.settings.logout_note')}</Note>
            <div><Button variant="danger" onClick={() => void ggsApi.disconnect()}>{t('ggs.settings.logout')}</Button></div>
          </Section>
        )}
      </div>
    </div>
  );
}

function FormulaField({ label, src, onSave }: { label: string; src: string; onSave: (s: string) => void }) {
  const [cond, setCond] = useState<Cond | null>(() => (src ? parseCond(src) : null));
  const [raw, setRaw] = useState<string | null>(null);
  if (raw !== null) {
    return (
      <Field label={label}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          <TextArea mono rows={4} value={raw} onChange={setRaw}
                    placeholder={t('ggs.settings.formula_placeholder')} />
          <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
            <Button onClick={() => { setRaw(null); setCond(raw ? parseCond(raw) : null); }}>
              {t('ggs.settings.back_to_tree')}
            </Button>
            <span style={{ flex: 1 }} />
            <Button variant="primary" onClick={() => { onSave(raw); setRaw(null); setCond(raw ? parseCond(raw) : null); }}>
              {t('ggs.apply')}
            </Button>
          </div>
        </div>
      </Field>
    );
  }
  return (
    <Field label={label}>
      <FormulaEditor value={cond} onChange={setCond} onClear={() => setCond(null)}
                     onSave={onSave} onRaw={(s) => setRaw(s)} />
    </Field>
  );
}

function GgsResults({ snap, onKifu }: {
  snap: GgsSnapshot;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  const [gtype, setGtype] = useState('');
  const login = snap.login;
  useEffect(() => { if (login) void ggsApi.history('').catch(() => {}); }, [login]);
  const kinds = [...new Set([
    ...snap.results.map((r) => poolOf(r.base, r.raw)),
    ...(snap.history[snap.login] ?? []).map((h) => poolOf(h.gtype)),
  ])].filter(Boolean);
  const known = new Set(snap.results.flatMap((r) => [r.archive, r.id].filter(Boolean)));
  const fromServer: GameResult[] = (snap.history[snap.login] ?? [])
    .filter((h) => !known.has(h.id))
    .map((h) => {
      const iAmBlack = h.black === snap.login;
      const diff = parseFloat(h.score);
      return {
        id: h.id, seq: 0, base: h.gtype, opp: iAmBlack ? h.white : h.black,
        my_diff: Number.isFinite(diff) ? (iAmBlack ? diff : -diff) : null,
        my_rating: parseFloat(iAmBlack ? h.black_rating : h.white_rating) || null,
        at: Date.parse(h.at) / 1000 || 0,
        kifu: '', ggf: '', archive: h.id,
      } as GameResult;
    });
  const all = [...snap.results, ...fromServer].sort((x, y) => (y.at ?? 0) - (x.at ?? 0));
  let most = '';
  let mostN = -1;
  for (const k of kinds) {
    const n = all.filter((r) => poolOf(r.base, r.raw) === k).length;
    if (n > mostN) { mostN = n; most = k; }
  }
  const cur = gtype || most || 'all';
  const rows = all.filter((r) => cur === 'all' || poolOf(r.base, r.raw) === cur);
  const rated = rows.filter((r) => r.my_rating != null).reverse();
  const rates = rated.map((r) => r.my_rating as number);
  const rateDates = rated.map((r) => fmtDay(r.at ?? 0));
  const rateLabels = rated.map((r) => {
    const d = r.my_diff;
    const sign = d == null ? '' : d > 0 ? `+${d}` : `${d}`;
    return `${fmtDay(r.at ?? 0)} · ${r.opp} · ${sign} · ${Math.round(r.my_rating as number)}`;
  });
  const [hover, setHover] = useState<number | null>(null);
  const hoverKey = hover != null && rated[hover] ? rowKey(rated[hover]) : '';

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column',
                  padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <Section title={t('ggs.results.rating_trend')}
               aside={kinds.length > 1 ? (
                 <Segmented value={cur} onChange={setGtype}
                            options={[...kinds.map((k) => ({ value: k, label: poolLabel(k) })),
                                      { value: 'all', label: t('ggs.filter.all') }]} />
               ) : undefined}>
        {cur === 'all' ? (
          <Note>{t('ggs.results.per_pool_note')}</Note>
        ) : (
          <RateChart points={rates} width={800} height={180} axes dates={rateDates}
                     labels={rateLabels} hover={hover} onHover={setHover} />
        )}
      </Section>

      <Section title={t('ggs.results.finished')} aside={<span>{rows.length}</span>} grow
               head={!!rows.length && (
                 <ResultHead note={cur === 'all'}
                             rating={rows.some((r) => r.my_rating != null)} when />
               )}>
        {!rows.length && <Empty>{t('ggs.no_records')}</Empty>}
        <List key={cur}>
        {rows.map((r) => (
          <ResultRow key={rowKey(r)} opponent={r.opp}
                     picked={!!hoverKey && rowKey(r) === hoverKey}
                     onHover={(on) => {
                       const i = rated.findIndex((x) => rowKey(x) === rowKey(r));
                       setHover(on ? (i < 0 ? null : i) : null);
                     }}
                     win={(r.my_diff ?? 0) > 0} draw={r.my_diff === 0}
                     adjourned={r.my_diff == null}
                     discs={r.my_diff ?? 0} when={fmtDay(r.at)}
                     note={cur === 'all' ? gtypeLabel(baseType(r.base, r.raw)) : undefined}
                     rating={r.my_rating}
                     onClick={() => onKifu(t('ggs.play.game_with', { name: r.opp }),
                                           r.ggf || r.kifu, r.archive)}
                     dim={!r.ggf && !r.kifu && !r.archive} />
        ))}
        </List>
      </Section>
    </div>
  );
}

// GGS hands out the same match number again and again, and the sequence number
// beside it restarts with the app, so id and seq together name 33 pairs of
// different games in the saved history. Hovering a row then lit up whichever
// older game answered to the same name -- a row from 8/21 moved the chart's
// marker to 8/14. The finish time settles it.
const rowKey = (r: GameResult) => `${r.id}#${r.seq}#${r.at ?? 0}`;

const poolOf = (base: string, raw?: string) => {
  const kind = baseType(base, raw);
  if (!kind) return '';
  return kind.includes('r') ? '8r' : '8';
};

const poolLabel = (p: string) =>
  (p === '8r' ? t('ggs.gtype.rand_opening') : p === '8' ? t('ggs.pool.normal') : p);

const baseType = (base: string, raw?: string) => {
  const fromRaw = raw?.split(/\s+/).find((x) => /^s?8(r\d+)?$/.test(x));
  if (fromRaw) return fromRaw;
  return base.split('.')[0] ?? '';
};

function fmtWhen(at: string): string {
  const ms = Date.parse(at);
  if (!Number.isFinite(ms)) return at;
  const d = new Date(ms);
  const p2 = (n: number) => String(n).padStart(2, '0');
  return `${d.getMonth() + 1}/${p2(d.getDate())} ${p2(d.getHours())}:${p2(d.getMinutes())}`;
}

function fmtDay(secs: number): string {
  if (!secs) return '';
  const d = new Date(secs * 1000);
  const now = new Date();
  const p2 = (n: number) => String(n).padStart(2, '0');
  const same = d.toDateString() === now.toDateString();
  return same ? `${p2(d.getHours())}:${p2(d.getMinutes())}` : `${d.getMonth() + 1}/${p2(d.getDate())}`;
}


function UserCard({ snap, name, onClose, onDetail, onAsk }: {
  snap: GgsSnapshot; name: string;
  onClose: () => void;
  onDetail: () => void;
  onAsk: () => void;
}) {
  useEffect(() => { ggsApi.finger(name).catch(() => {}); }, [name]);

  const u = snap.users.find((x) => x.name === name);
  const rates = bothRates(u);
  const fields = snap.fingers[name]?.fields ?? [];

  const facts = (fingerGroups(fields).find((g) => g.id === 'request')?.rows ?? [])
    .filter((r) => !['accept', 'decline', 'request'].includes(normKey(r.key).replace(/\(.*\)/, '')));

  return (
    <Overlay onClose={onClose}>
      <Modal title={name} onClose={onClose}
             sub={rates || undefined}
             scroll
             actions={<>
               <Button size="field" onClick={onDetail}>{t('ggs.users.details')}</Button>
               <span style={{ marginLeft: 'auto' }} />
               <Button size="field" variant="primary" onClick={onAsk}>{t('ggs.lobby.ask')}</Button>
             </>}>
        <Section title={t('ggs.match_requests')}>
            {!facts.length && <Empty>{t('ggs.loading')}</Empty>}
            {facts.map((r) => (
              <div key={r.key} style={{ display: 'flex', alignItems: 'center',
                                        gap: 'var(--sp-3)', fontSize: 'var(--fs-5)' }}>
                <span style={{ color: 'var(--sub)' }}>{r.label}</span>
                <span style={{ marginLeft: 'auto', overflow: 'hidden',
                               textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {fingerValue(r.key, r.value) || '—'}
                </span>
              </div>
            ))}
        </Section>
      </Modal>
    </Overlay>
  );
}

function GgsUsers({ snap, onNav, onKifu }: {
  snap: GgsSnapshot; onNav: (id: NavId) => void;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  const [card, setCard] = useState<string | null>(null);
  const [mode, setMode] = useState<'who' | 'top'>('who');
  const userCols: Col[] = [
    ...(mode === 'top' ? [{ head: '#', w: 26, right: true, num: true } as Col] : []),
    { head: t('ggs.users.col_name'), clip: true },
    { head: mode === 'who' ? t('ggs.pool.normal') : t('ggs.users.col_rating'),
      w: 96, right: true, num: true },
    ...(mode === 'who'
      ? [{ head: t('ggs.pool.random'), w: 104, right: true, num: true } as Col] : []),
    ...(mode === 'who' ? [{ head: t('ggs.users.col_open'), w: 96, right: true } as Col] : []),
    { head: t('ggs.users.col_status'), w: 64, right: true },
  ];
  useEffect(() => { void ggsApi.listMatches().catch(() => {}); }, []);
  useEffect(() => {
    void ggsApi.autoview().then((v) => {
      if (v === 'players:card') setCard(snap.login || '—');
      if (v === 'players:top') setMode('top');
    }).catch(() => {});
  }, [snap.login]);
  const [sel, setSel] = useState<string | null>(null);
  const [pool, setPool] = useState('8');
  const [page, setPage] = useState(0);
  const [perPage, setPerPage] = useState(25);
  const [tab, setTab] = useState('profile');

  if (sel) {
    return <UserDetail snap={snap} name={sel} tab={tab} onTab={setTab}
                       onBack={() => setSel(null)} onNav={onNav} onKifu={onKifu} />;
  }

  const rows = mode === 'who' ? snap.users : snap.ranking;
  const mine = snap.my_ranks;
  const pages = Math.max(1, Math.ceil(rows.length / perPage));
  const cur = Math.min(page, pages - 1);
  const slice = rows.slice(cur * perPage, (cur + 1) * perPage);

  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4) var(--sp-4) 0' }}>
      {card && (
        <UserCard snap={snap} name={card}
                  onClose={() => setCard(null)}
                  onDetail={() => { setSel(card); setCard(null); }}
                  onAsk={() => { setCard(null); onNav('ggs-lobby'); }} />
      )}
      <Section title={t('ggs.users.my_rating')}
               aside={<Button onClick={() => {
                 for (const pool of ['8', '8r']) void ggsApi.rank(pool, snap.login);
               }}>{t('ggs.refresh')}</Button>}>
        {!mine.length && <Empty>{t('ggs.no_records')}</Empty>}
        {mine.map((r) => (
          <RateRow key={r.gtype} label={gtypeLabel(r.gtype)}
                   rate={{ value: r.rating, dev: r.dev, rank: r.rank,
                           w: r.wins, l: r.losses, d: r.draws,
                           provisional: r.dev > PROVISIONAL_DEV }} />
        ))}
      </Section>

      <Section title={mode === 'who' ? t('ggs.users.online') : t('ggs.users.ranking')}
               aside={<>
                 {mode === 'top' && (
                   <Segmented value={pool} onChange={(p) => {
                     setPool(p);
                     void ggsApi.top(p, 100);
                   }} options={[{ value: '8', label: t('ggs.pool.normal') },
                                { value: '8r', label: t('ggs.gtype.rand_opening') }]} />
                 )}
                 <Segmented value={mode} onChange={(m) => {
                   setMode(m);
                   setPage(0);
                   if (m === 'who') void ggsApi.who(); else void ggsApi.top(pool, 100);
                 }} options={[{ value: 'who', label: t('ggs.users.online') },
                              { value: 'top', label: t('ggs.users.top') }]} />
               </>}>
        {!rows.length && <Empty>{t('ggs.users.nobody')}</Empty>}
        {!!rows.length && (
          <TableHead cols={userCols} pad="var(--sp-4)" />
        )}
        {!slice.length && (
          <Empty>{mode === 'who' ? t('ggs.users.none_online') : t('ggs.users.no_ranking')}</Empty>
        )}
        <List>
        {slice.map((u, i) => {
        const playing = snap.ongoing.some((o) => o.names.includes(u.name));
        return (
          <TableRow key={u.name} cols={userCols} pad="var(--sp-4)" onClick={() => setCard(u.name)}>
            {mode === 'top' && (
              <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>
                {cur * perPage + i + 1}
              </span>
            )}
            <span className="k-sel">{u.name}</span>
            <span>
              {u.rating != null && <>
                {u.rating.toFixed(1)}
                {u.dev != null && (
                  <span style={{ color: 'var(--sub)', marginLeft: 4 }}>±{Math.round(u.dev)}</span>
                )}
              </>}
            </span>
            {mode === 'who' && (
              <span>
                {u.rating_r != null && <>
                  {u.rating_r.toFixed(1)}
                  {u.dev_r != null && (
                    <span style={{ color: 'var(--sub)', marginLeft: 4 }}>±{Math.round(u.dev_r)}</span>
                  )}
                </>}
              </span>
            )}
            {mode === 'who' && (
              <span style={{
                fontSize: 'var(--fs-6)',
                color: u.open === '+' ? 'var(--accent)'
                  : u.open === 'x' ? 'var(--bad)' : 'var(--sub)',
              }}>
                {u.open === '+' ? t('ggs.users.open_yes')
                  : u.open === 'x' ? t('ggs.users.open_ghost')
                  : u.open ? t('ggs.users.open_no') : '—'}
              </span>
            )}
            <span style={{
              fontSize: 'var(--fs-6)', color: playing ? 'var(--ok)' : 'var(--sub)',
            }}>{playing ? t('ggs.state.playing') : t('ggs.state.idle')}</span>
          </TableRow>
        );})}
        </List>
        {rows.length > perPage && (
          <div style={{
            display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
            padding: 'var(--sp-2) 0', fontSize: 'var(--fs-6)', color: 'var(--sub)',
          }}>
            <Button disabled={cur === 0} onClick={() => setPage(cur - 1)}>{t('ggs.prev')}</Button>
            <span style={{ fontVariantNumeric: 'tabular-nums' }}>{cur + 1} / {pages}</span>
            <Button disabled={cur >= pages - 1} onClick={() => setPage(cur + 1)}>{t('ggs.next')}</Button>
            <span style={{ marginLeft: 'auto' }}>{t('ggs.users.per_page')}</span>
            <Select size="ctrl" value={String(perPage)}
                    options={[['25', '25'], ['50', '50'], ['100', '100']]}
                    onChange={(v) => { setPerPage(+v); setPage(0); }} />
          </div>
        )}
      </Section>
    </div>
  );
}

function UserDetail({ snap, name, tab, onTab, onBack, onNav, onKifu }: {
  snap: GgsSnapshot; name: string; tab: string;
  onTab: (t: string) => void; onBack: () => void; onNav: (id: NavId) => void;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  useEffect(() => { ggsApi.finger(name).catch(() => {}); }, [name]);
  useEffect(() => { ggsApi.history(name === snap.login ? '' : name).catch(() => {}); }, [name, snap.login]);

  const u = snap.users.find((x) => x.name === name);
  const rates = bothRates(u);
  const playing = snap.ongoing.some((o) => o.names.includes(name));
  const fields = snap.fingers[name]?.fields ?? [];
  const who = { me: name, them: t('ggs.formula.who.me') };
  const rows = snap.history[name] ?? [];
  const histCols: Col[] = [
    { head: t('ggs.users.col_when'), w: 132 },
    { head: t('ggs.field.format'), w: 104 },
    { head: t('ggs.field.opponent'), clip: true },
    { head: t('ggs.users.col_side'), w: 44 },
    { head: t('ggs.users.col_diff'), w: 64, right: true, num: true },
  ];

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div style={{
        flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        padding: 'var(--sp-3) var(--sp-4)', borderBottom: '1px solid var(--border)',
        background: 'var(--panel)',
      }}>
        <IconButton name="back" label={t('ggs.back_to_list')} onClick={onBack} />
        <span className="k-sel" style={{ fontSize: 'var(--fs-2)', fontWeight: 600 }}>{name}</span>
        {rates && (
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{rates}</span>
        )}
        {playing && <Tag tone="ok">{t('ggs.state.playing')}</Tag>}
        <span style={{ marginLeft: 'auto' }} />
        <Button variant="primary" onClick={() => onNav('ggs-lobby')}>{t('ggs.lobby.request_title')}</Button>
      </div>
      <div style={{ flex: 'none', padding: 'var(--sp-2) var(--sp-4)' }}>
        <Segmented value={tab} onChange={onTab}
                   options={[{ value: 'profile', label: t('ggs.users.tab_profile') },
                             { value: 'history', label: t('ggs.users.tab_history') }]} />
      </div>

      <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: '0 var(--sp-4) var(--sp-4)' }}>
        {tab === 'profile' ? (
          <>
            {!fields.length && <Empty>{t('ggs.loading')}</Empty>}
            {fingerGroups(fields).map((g) => (
              <Section key={g.title} title={g.title}>
                {g.rows.map((r) => {
                  const key = normKey(r.key).replace(/\(.*\)/, '');
                  const cond = ['accept', 'decline', 'request'].includes(key)
                    ? parseCond(r.value) : null;
                  return (
                    <div key={r.key} style={{
                      display: 'flex', gap: 'var(--sp-3)', alignItems: 'flex-start',
                      padding: 'var(--sp-2) 0', borderBottom: '1px solid var(--border-weak)',
                    }}>
                      <span style={{
                        width: 'var(--w-label)', flex: 'none', fontSize: 'var(--fs-6)', color: 'var(--sub)',
                      }}>{r.label}</span>
                      <span style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-5)' }}>
                        {cond ? <FormulaView node={cond} top who={who} />
                          : ['accept', 'decline', 'request'].includes(key) ? t('ggs.formula.unset')
                          : fingerValue(r.key, r.value)}
                      </span>
                    </div>
                  );
                })}
              </Section>
            ))}
          </>
        ) : (
          <>
            {!rows.length && <Empty>{t('ggs.users.no_history')}</Empty>}
            {!!rows.length && <TableHead cols={histCols} />}
            <List>
            {rows.map((h) => {
              const black = h.black === name;
              const d = parseFloat(h.score);
              const mine = Number.isFinite(d) ? (black ? d : -d) : null;
              return (
                <TableRow key={h.id} cols={histCols}
                          onClick={() => onKifu(t('ggs.vs', { a: h.black, b: h.white }), '', h.id)}>
                  <span style={{ color: 'var(--sub)' }}>{fmtWhen(h.at)}</span>
                  <span style={{ color: 'var(--sub)' }}>{gtypeLabel(h.gtype)}</span>
                  <span className="k-sel">{black ? h.white : h.black}</span>
                  <span style={{ color: 'var(--sub)' }}>
                    {black ? t('ggs.color.black') : t('ggs.color.white')}
                  </span>
                  <span style={{
                    color: mine == null ? 'var(--sub)'
                      : mine > 0 ? 'var(--ok)' : mine < 0 ? 'var(--bad)' : 'var(--text)',
                  }}>
                    {mine == null ? '—' : mine > 0 ? `+${mine}` : `${mine}`}
                  </span>
                </TableRow>
              );
            })}
            </List>
          </>
        )}
      </div>
    </div>
  );
}

function ChatList({ sorted, cur, onThread, onPick }: {
  sorted: [string, { last: ChatMsg; n: number }][];
  cur: string;
  onThread: (t: string) => void;
  onPick: (v: boolean) => void;
}) {
  return (
    <>
    <aside style={{
      width: 'var(--w-lobby)', flex: 'none', borderRight: '1px solid var(--border)',
      minHeight: 0, display: 'flex', flexDirection: 'column',
    }}>
      <div style={{
        flex: 'none', height: 'var(--h-field)', display: 'flex', alignItems: 'center',
        gap: 'var(--sp-2)', padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)',
      }}>
        <span style={{ fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)' }}>{t('ggs.chat.threads')}</span>
        <span style={{ marginLeft: 'auto' }} />
        <Button size="chip" onClick={() => onPick(true)}>{t('ggs.chat.new_thread')}</Button>
      </div>
      <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>
      {sorted.map(([key, info]) => (
        <button key={key} type="button" onClick={() => onThread(key)}
          aria-current={key === cur || undefined}
          className={'k-row' + (key === cur ? ' k-on' : '')}
          style={{
            width: '100%', border: 0, textAlign: 'left', display: 'flex', flexDirection: 'column',
            gap: 'var(--sp-1)', padding: 'var(--sp-2) var(--sp-3)',
            borderBottom: '1px solid var(--border-weak)',
            ...picked(key === cur),
          }}>
          <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)' }}>
            {key === '.chat' ? t('ggs.chat.global') : key}
            {info.last.at > 0 && (
              <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>
                {clockOf(info.last.at)}
              </span>
            )}
          </span>
          <span style={{
            fontSize: 'var(--fs-6)', color: 'var(--sub)',
            overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
          }}>{info.last.text || '—'}</span>
        </button>
      ))}
      </div>
    </aside>
    </>
  );
}
