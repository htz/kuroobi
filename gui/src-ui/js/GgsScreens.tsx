import React, { useEffect, useRef, useState } from 'react';
import { api, ggsApi, jsLog, onApp } from './api';
import type { ChatMsg, GameResult, GgsSnapshot, MatchView, UserRow } from './types';
import {
  clockChoices, gtypeChoices, clockOf, countDiscs, ggsMoveToIndex, gtypeLabel,
  fingerGroups, fingerValue, hasJapanese, normKey, parseCond, translate, useClocks,
  type ClockSide, type ClockView,
} from './ggs';
import { t, useLang, tErr } from './i18n';
import { Col, Empty, EmptyBoard, EmptyState, List, Modal, Note, Overlay, Section, TableHead, TableRow, picked } from './components/layout';
import { Button, Segmented, Select, TextField, Toggle } from './components/primitives';
import { Strength } from './components/strength';
import { Confirm, PickOne } from './Dialogs';
import { IconButton } from './components/Icons';
import {
  Bubble, ConsoleLog, DayMark, FormulaEditor, FormulaView, MatchRow, PlayerRow, RateRow, Tag,
  type Cond, type Match, type NavId,
} from './components/ggs';
import { Board, type Cell, type EvalInfo } from './components/board';
import { EvalTrend, RateChart, ResultRow, StoneDot } from './components/data';
import { flipped, type Prefs } from './prefs';
import { logLinesOf } from './adapt';

/* GGS screens, one per nav destination.
 *
 * Disconnected, the nav shows only the login row, so login is the
 * only reachable screen; the rest swap in as they are built. */

export function GgsScreen({ nav, snap, onNav, prefs, onKifu }: {
  nav: NavId; snap: GgsSnapshot | null; onNav: (id: NavId) => void; prefs: Prefs;
  /** Show a record in the overlay, fetching from `archive` when there
   *  is no local copy. */
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  // Screen root: subscribe so a language switch re-renders the tree.
  useLang();
  if (nav === 'ggs-login') return <GgsLogin />;
  if (!snap) return <EmptyState title={t('ggs.not_connected')} />;

  switch (nav) {
    case 'ggs-play': return <GgsPlay snap={snap} onNav={onNav} prefs={prefs} onKifu={onKifu} />;
    case 'ggs-lobby': return <GgsLobby snap={snap} onNav={onNav} />;
    case 'ggs-players': return <GgsUsers snap={snap} onNav={onNav} onKifu={onKifu} />;
    case 'ggs-results': return <GgsResults snap={snap} onKifu={onKifu} />;
    case 'ggs-chat': return <GgsChat snap={snap} />;
    case 'ggs-standby': return <GgsStandby snap={snap} onNav={onNav} />;
    case 'ggs-console': return <GgsConsole snap={snap} />;
    case 'ggs-settings': return <GgsSettings snap={snap} />;
    default: return null;
  }
}

/* Input label inside a floating box. Sections (1px-ruled headings)
 * belong to the content area; in a 340px box the rule crosses the box
 * and reads as a heading. */
/* In a column, alignItems: flex-start shrinks children to their own
 * width (TextField's flex:1 becomes a height matter). Fine for
 * content-sized choices, but text inputs stretch to the box — a
 * half-width input does not look like a place to type. */
function Field({ label, children, stretch }: { label: string; children: React.ReactNode; stretch?: boolean }) {
  return (
    /* alignSelf: 'start' is required: grid children stretch to the
       row height by default, so a tall sibling (the rated toggle with
       its description) stretched this too and flexGrow made the input
       absorb the surplus — two fields once rendered 3x tall. */
    <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)',
                    alignSelf: 'start',
                    alignItems: stretch ? 'stretch' : 'flex-start' }}>
      <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{label}</span>
      {children}
    </label>
  );
}

/* ---------------- Login ----------------
 *
 * Saved credentials feed the startup auto-login, so this screen shows
 * only when nothing is saved, auto-login failed, or after logout.
 *
 * Rounded boxes are banned for content, except here — the screen's
 * single entry point, with nothing beside it, reads better floated. */
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
      {/* Measured from the design: 340px box, --r-4, --panel ground,
          22px padding, 14px gaps. Fields are 32px on --bg. The button
          spans the box with the status line below, so fields and
          button read as one column. Headings match section type. */}
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
        {/* Reserve the line even when empty so nothing shifts. */}
        <div style={{
          fontSize: 'var(--fs-6)', minHeight: 16,
          color: status === t('ggs.login.connecting') ? 'var(--sub)' : 'var(--bad)',
        }}>{status}</div>
      </div>
    </div>
  );
}

/** Login field; heading in section type (the design's .fld). */
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

/* ---------------- Console ----------------
 *
 * Protocol log and raw commands. GGS has many features the UI never
 * surfaces; a direct-typing escape hatch always stays. */
export function GgsConsole({ snap }: { snap: GgsSnapshot }) {
  const [cmd, setCmd] = useState('');
  /* Filter and clear, per §6's heading. Clear is display-only — it
     marks "show from here", like a terminal clear, without touching
     the server traffic. Remembered as a count, so growth is safe. */
  const [dir, setDir] = useState<'all' | 'out' | 'in'>('all');
  const [from, setFrom] = useState(0);
  /* One-line result (rule 34 — only failures and why-nothing-moved). */
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
    // Sent/received are ours and theirs; app notes are neither and
    // drop when filtered.
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
                    // Silence on success (rule 34).
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
        {/* Enter sends here too — a raw-command loop shouldn't need
            the button each time. */}
        <TextField mono value={cmd} onChange={setCmd} onEnter={send}
                   placeholder={t('ggs.console.placeholder')} />
        <Button size="field" onClick={send}>{t('ggs.send')}</Button>
      </div>
    </div>
  );
}

/* ---------------- Chat ----------------
 *
 * Conversation list left (global + per correspondent), the selected
 * one right. English messages get an automatic Japanese translation;
 * Japanese messages can be sent translated to English. */

/** Ratings with deviation above this are still moving = provisional.
 *
 * GGS never says "provisional" directly: the raw rank row
 * (2184.2@180.8=) has a marker-like character, but two known
 * provisionals (6 and 5 games) both showed '=', so deviation is the
 * only usable signal.
 *
 * Why 100 — the design (§5) draws 1795.1±112 as provisional, and
 * live data splits the same way: newcomers ±350 (initial), few-game
 * players ±125-216, regulars ±44/±75/±91. TO CONFIRM (based on one
 * design example; the threshold deserves a discussion). */
const PROVISIONAL_DEV = 100;

/** Whether to attach a translation (others' English messages only). */
const wantsTranslation = (c: ChatMsg, login: string): boolean =>
  c.from !== login && !hasJapanese(c.text) && /[a-zA-Z]{2,}/.test(c.text);

const trKey = (c: ChatMsg): string => c.from + '|' + c.text;

export function GgsChat({ snap }: { snap: GgsSnapshot }) {
  const [thread, setThread] = useState('.chat');
  const [text, setText] = useState('');
  const [autoJa, setAutoJa] = useState(true);
  const [pick, setPick] = useState(false);
  const [toEn, setToEn] = useState(false);
  // Translation ('' = none: same as original, or fetch failed).
  const [trs, setTrs] = useState<Record<string, string>>({});
  const pending = useRef<Set<string>>(new Set());
  const box = useRef<HTMLDivElement>(null);

  // Conversation list: global chat pinned first, then newest first.
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

  // Attach translations to English messages (fetched in the
  // background, re-render on arrival).
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
    // Recipient is the open conversation (global or a name).
    ggsApi.chat(cur, msg).catch((e) => jsLog(String(e)));
  };

  // Precompute date headers and same-speaker name elision.
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
        {/* Header says which conversation and WHO receives — the
            list's selected row alone gives no last-second check. */}
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
        {/* Translation toggles get their own strip above the send row
            (as designed): in the send row they crowd the input, and
            both are per-conversation view settings, not per-message
            ones. */}
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
          {/* Empty must say so — global chat is often quiet, and blank
              reads as broken. */}
          {/* Messages stack from the bottom; center only when empty. */}
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

/* ---------------- Lobby ----------------
 *
 * Running games (observable), match requests, the request form, and
 * adjourned games. Lists left, "start something" right.
 *
 * The right column is --w-dock (290px). The design's --w-lobby
 * (174px) is unused — the format select wraps at 174px, and
 * per-screen widths make the body jump when switching. */
function GgsLobby({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
  const [opp, setOpp] = useState('');
  const [gtype, setGtype] = useState('s8r16');
  const [time, setTime] = useState('00:15:00');
  const noRated = useNoRated();
  const calibrated = useCalibrated();
  const [rated, setRated] = useState(true);
  /** The one request id whose details are expanded. */
  const [info, setInfo] = useState('');
  /* The "in progress" section also derives from the running list,
     which arrives at login and every 60s — so re-ask on open (same
     reason as the player list). */
  useEffect(() => { void ggsApi.listMatches().catch(() => {}); }, []);

  const games = snap.ongoing.filter((o) => !o.mine);
  const names = snap.users.filter((u) => u.name !== snap.login).map((u) => u.name);

  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
      <div className="k-scroll" style={{ flex: 1, minWidth: 0, padding: 'var(--sp-4) var(--sp-2) 0' }}>
        <Section title={t('ggs.lobby.ongoing_title')}
                 aside={games.length ? t('ggs.lobby.game_count', { n: games.length }) : undefined}>
          {!games.length && <Empty>{t('ggs.lobby.no_ongoing')}</Empty>}
          {/* Rows stay tight; the section gap (12px) between rows
              reads as bullet points (hit before in book and log). */}
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
                             // Observing means wanting the board: go to
                             // the game screen and show THAT game.
                             if (on) { focusMatch(o.id); onNav('ggs-play'); }
                           }}>
                     {o.watching ? t('ggs.stop_observing') : t('ggs.observe')}
                   </Button>} />
          ))}
          </List>
        </Section>

        <Section title={t('ggs.match_requests')}>
          {!snap.offers.length && <Empty>{t('ggs.lobby.no_requests')}</Empty>}
          {/* Wrapped in List here too — the 12px section gap trap, hit
              for the fifth time. */}
          <List>
          {snap.offers.map((o) => {
            const who = o.names.filter((n) => n !== snap.login);
            return (
              <React.Fragment key={o.id}>
              <Row title={who.join(t('ggs.and_separator')) || '?'}
                   // Rule 27 — only addressed-to-me and unread get
                   // --bad; accent would blend with clickable blue.
                   tag={o.incoming ? t('ggs.tag.to_me') : undefined}
                   tagTone={o.incoming ? 'bad' : undefined}
                   alert={o.incoming}
                   sub={`${gtypeLabel(o.gtype)} · ${o.time || '?'}${o.rated ? ' · ' + t('ggs.rated') : ''}`}
                   actions={<>
                     {/* Details exist on others' requests too: the
                         one-line summary drops color/komi/random-ply,
                         and you want the raw row before accepting. */}
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
        // The right column matches the dock width; per-screen widths
        // make the body jump.
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
          {/* Rated-ness is account-level in /os; the backend sends it
              before each request. This only holds the choice for this
              request. */}
          {/* Noun heading, verb chips — matching the other settings.
              "Rated" is the GGS term, so it heads the row as-is. */}
          <Field label={t('ggs.rated')}>
            {/* When banned: disabled with the reason in place (rule
                61). The send path blocks too; this is presentation. */}
            <Segmented value={rated && !noRated ? 'on' : 'off'} disabled={noRated}
                       onChange={(v) => setRated(v === 'on')}
                       options={[{ value: 'on', label: t('ggs.on') },
                                 { value: 'off', label: t('ggs.off') }]} />
            {noRated && <Note>{t('ggs.no_rated_note')}</Note>}
          </Field>
          {/* An opponent-less request is an open invitation; /os ask
              supports that, so don't block it. */}
          {/* Same 32px as the fields above; a shorter button breaks
              the fill-then-press sequence (as in login). */}
          <Button size="field" variant="primary" disabled={!calibrated}
                  onClick={() => void ggsApi.ask(gtype, time, opp, rated)}>
            {opp ? t('ggs.lobby.ask') : t('ggs.lobby.open_request')}
          </Button>
          {!calibrated && <Note>{calibNote()}</Note>}
          <Note>{t('ggs.lobby.synchro_note')}</Note>
        </Section>

        {/* Adjourned games arrive once at login; later adjournments
            require asking again. */}
        <Section title={t('ggs.adjourned_games')}
                 aside={<Button onClick={() => void ggsApi.listStored()}>{t('ggs.refresh')}</Button>}>
          {!snap.stored.length && <Empty>{t('ggs.lobby.no_adjourned')}</Empty>}
          {/* Rows stay tight (the 12px section-gap trap). */}
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

/** A list row: name and detail left, actions right; the row itself is
 *  not clickable. */
function Row({ title, sub, tag, tagTone, alert, actions, onClick, title2 }: {
  title: string; sub?: string; tag?: string;
  /** Dot color; default accent (rule 27 — only addressed/unread get
   *  --bad). */
  tagTone?: 'sub' | 'accent' | 'ok' | 'bad';
  /** Addressed to me; --bad left bar (design §4). */
  alert?: boolean;
  actions?: React.ReactNode;
  /** Make the row clickable; never with other clickables inside
   *  (rule 46). */
  onClick?: () => void;
  /** Clickable-row hint (title attribute). */
  title2?: string;
}) {
  /* Clickable means a clickable element (rule 41): div + onClick
     skips the Tab ring and ignores Enter/Space. */
  const Tag_ = onClick && !actions ? 'button' : 'div';
  return (
    <Tag_ {...(onClick && !actions
      ? { type: 'button' as const, className: 'k-row', onClick, title: title2 }
      : {})} style={{
      display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', width: '100%',
      /* Fixed height (measured 40): content-sized rows vary with and
         without details. A different type from the 24px rows. */
      height: 'var(--h-row2)', flex: 'none', padding: '0 var(--sp-4)',
      // Shorthand border first — written after, it erases
      // borderBottom, and the loss is invisible without a GGS
      // connection.
      border: 0, borderRadius: 0,
      borderBottom: '1px solid var(--border-weak)',
      /* Addressed marker (§4): inset shadow + tint, same shape as
         picked — a border would shift the content 3px. Color per
         rule 27. */
      background: alert ? 'color-mix(in srgb, var(--bad) 8%, transparent)' : 'transparent',
      boxShadow: alert ? 'inset 2px 0 0 var(--bad)' : undefined,
      textAlign: 'left',
      color: 'var(--text)', cursor: onClick && !actions ? 'pointer' : undefined,
    }}>
      {/* 2px between the two lines; sp-1 (4) overflows 40px. */}
      <span style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
        <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)' }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{title}</span>
          {tag && <Tag tone={tagTone ?? 'accent'}>{tag}</Tag>}
        </span>
        {sub && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{sub}</span>}
      </span>
      {actions}
    </Tag_>
  );
}

/** Whether rated play is banned — the lid for automated runs
 *  (KUROOBI_NO_RATED=1). The send path blocks too; this value only
 *  shows the unavailability. */
function useNoRated(): boolean {
  const [no, setNo] = useState(false);
  useEffect(() => { ggsApi.noRated().then(setNo).catch(() => {}); }, []);
  return no;
}

/* Whether solve speed is calibrated.
 *
 * Uncalibrated, time management falls back to a fixed ladder —
 * entering games blind to the machine's speed, where timeouts hit
 * the rating directly. So every time-touching action (requests,
 * waiting mode, clock settings) stops here. The backend enforces the
 * same check — UI-only gates leak through stale screens.
 *
 * Defaults to "calibrated" to avoid a flash of disabled while
 * fetching (the backend still blocks). */
function useCalibrated(): boolean {
  const [ok, setOk] = useState(true);
  useEffect(() => {
    let alive = true;
    const load = () => void api.localThreads()
      .then((t) => { if (alive) setOk(t.nps != null); })
      .catch(() => {});
    load();
    // Startup calibration runs in the background; a notice follows.
    const off = onApp('resources-changed', load);
    return () => { alive = false; void off.then((f) => f()); };
  }, []);
  return ok;
}

/** Uncalibrated message — includes the remedy (rule 34). Read per
 *  call so a language switch re-renders it. */
const calibNote = (): string => t('ggs.calib_note');

/* ---------------- Waiting mode ----------------
 *
 * Game end -> interval -> auto-request, repeated. The server-side
 * request formula is view-only here; editing lives in GGS settings —
 * two editable copies compete for authority. */
function GgsStandby({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
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
  /* Finished games don't count: they stay listed for their records,
     and counting them would leave "playing" stuck forever after one
     game (the backend's auto-accept uses the same test). */
  const playing = snap.matches.some((m) => !m.over);
  const state = sb.enabled
    ? (playing ? t('ggs.standby.state_playing') : t('ggs.standby.state_waiting'))
    : t('ggs.standby.state_off');

  const toggle = () => void ggsApi.setStandby({
    // Banned means false no matter what; stale screens don't pass.
    enabled: !sb.enabled, auto_accept: autoAccept, rated: rated && !noRated, opponent: opp.trim(),
    gtype, time, max_games: maxGames, interval_secs: interval,
  });

  // Re-fetch own settings (the server holds the formula).
  useEffect(() => {
    if (snap.login) ggsApi.finger(snap.login).catch(() => {});
  }, [snap.login]);

  const form = (key: 'accept' | 'decline'): string =>
    (snap.fingers[snap.login]?.fields
      .find(([k]) => k.replace(/\s+/g, '').replace(/\(.*\)/, '') === key)?.[1] ?? '')
      .replace(/^\s*:\s*/, '').trim();

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
               </span>}>
        {/* Three-column grid with no per-field widths — those cramp
            the format select and shrink number fields unevenly. Split
            the container in thirds. */}
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
          {/* Rated-ness for outgoing requests (account-level; sent
              before each). Incoming requests' rated-ness is the
              asker's choice. */}
          <Field label={t('ggs.rated')}>
            {/* When banned: disabled with the reason in place (rule
                61). The send path blocks too; this is presentation. */}
            <Segmented value={rated && !noRated ? 'on' : 'off'} disabled={noRated}
                       onChange={(v) => setRated(v === 'on')}
                       options={[{ value: 'on', label: t('ggs.on') },
                                 { value: 'off', label: t('ggs.off') }]} />
            {noRated && <Note>{t('ggs.no_rated_note')}</Note>}
          </Field>
        </div>
        {/* Toggle pushes its knob right; unconstrained it drifts to
            the screen edge. */}
        <span style={{ width: 300, display: 'block' }}>
          <Toggle checked={autoAccept} onChange={setAutoAccept} label={t('ggs.standby.auto_accept')} />
        </span>
        <div>
          {/* Stopping works even uncalibrated — never trap the user. */}
          <Button variant={sb.enabled ? 'danger' : 'primary'}
                  disabled={!sb.enabled && !calibrated} onClick={toggle}>
            {sb.enabled ? t('ggs.standby.stop') : t('ggs.standby.start')}
          </Button>
          {!sb.enabled && !calibrated && <Note>{calibNote()}</Note>}
        </div>
        <Note>{t('ggs.standby.note')}</Note>
      </Section>

      {/* Section name and description per §7. The old "two editable
          copies compete" note was builder's reasoning — rule 64 keeps
          that out of the UI. What the reader needs: it persists
          across restarts. */}
      <Section title={t('ggs.standby.formula_title')}
               aside={<Button onClick={() => onNav('ggs-settings')}>{t('ggs.standby.edit_conditions')}</Button>}>
        <Note>{t('ggs.standby.formula_note')}</Note>
        <FormulaRow label={t('ggs.finger.accept')} src={form('accept')} />
        <FormulaRow label={t('ggs.finger.decline')} src={form('decline')} />
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

function FormulaRow({ label, src }: { label: string; src: string }) {
  const cond = src ? parseCond(src) : null;
  return (
    <div style={{ display: 'flex', gap: 'var(--sp-3)', alignItems: 'flex-start' }}>
      <span style={{ width: 'var(--w-label)', flex: 'none', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{label}</span>
      {cond ? <FormulaView node={cond} top />
            : <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>{t('ggs.formula.unset')}</span>}
    </div>
  );
}

/* Show the explicitly opened game first.
 *
 * Defaulting to the list head kept showing some other game after
 * starting an observation or landing a match. Record the id at the
 * click and pick it up when the game screen mounts.
 *
 * It crosses screens, but React state would mean lifting to App; as
 * a one-shot handoff, a slot that is read-then-cleared suffices. */
let wantedMatch = '';
export function focusMatch(id: string) { wantedMatch = id; }

/* ---------------- Play / observe ----------------
 *
 * Synchro games are pairs; a pair is one row with boards on the
 * right. No undo/abort buttons — both need the opponent's consent,
 * and GGS opponents are mostly programs. Only resignation is one's
 * own decision. */
function GgsPlay({ snap, onNav, prefs, onKifu }: {
  snap: GgsSnapshot; onNav: (id: NavId) => void; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  /* Show the clicked-through game, then clear — kept around, it
     reverts later reselections. */
  const [sel, setSel] = useState(() => { const w = wantedMatch; wantedMatch = ''; return w; });
  const clock = useClocks(snap.matches);

  // Group into matches: own games first, then observed.
  const groups = new Map<string, MatchView[]>();
  for (const m of snap.matches) groups.set(m.base, [...(groups.get(m.base) ?? []), m]);
  /* Newest first — by arrival order, not id: GGS reuses ids, so a
     new game with a small id landed mid-list (reported live). */
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
                    {/* The design's second button: with no games,
                        auto-requesting is faster. */}
                    <Button onClick={() => onNav('ggs-standby')}>{t('ggs.play.to_standby')}</Button>
                    {/* The list can be stale right after reconnect;
                        keep a re-ask path. */}
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
                    // Finished games used to linger; results keep a
                    // copy, so closing here loses nothing.
                    onClose={() => {
                      void ggsApi.closeMatch(key);
                      if (key === sel) setSel('');
                    }} />
        ))}
      </aside>

      <div className="k-scroll" style={{ flex: 1, minWidth: 0, minHeight: 0, padding: 'var(--sp-3)' }}>
        {/* Synchro pairs sit side by side, wrapping when narrow. */}
        <div style={{ display: 'flex', gap: 'var(--sp-3)', flexWrap: 'wrap', justifyContent: 'center' }}>
          {pair?.map((m, i) => (
            <MatchBoard key={m.id} snap={snap} m={m} clock={clock} prefs={prefs} onKifu={onKifu}
                        // A synchro pair is unreadable without "which
                        // board am I which color on" (design §2).
                        face={(pair?.length ?? 1) > 1 ? i + 1 : undefined} />
          ))}
        </div>
      </div>
    </div>
  );
}

/** One match as a list row; finished matches stay listed. */
function matchRowOf(g: MatchView[], key: string): Match {
  const m = g[0];
  const mine = g.some((x) => x.my_color);
  return {
    id: key, mine, live: !g.every((x) => x.over),
    opponent: m.opp_name,
    black: m.players[0]?.name ?? '?', white: m.players[1]?.name ?? '?',
    kind: gtypeLabel(m.gtype), boards: g.length,
    ply: Math.max(...g.map((x) => x.moves.length)),
    result: g.map((x) => x.result).find(Boolean) || undefined,
    // Adjournment is not "finished, no result"; say who left.
    ended: g.map((x) => x.ended).find(Boolean) || undefined,
    leftBy: g.map((x) => x.left_by).find(Boolean) || undefined,
  };
}

function MatchBoard({ snap, m, clock, prefs, onKifu, face }: {
  snap: GgsSnapshot; m: MatchView; clock: (id: string, side: ClockSide) => ClockView; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
  /** Board index in a synchro pair (1-based); omit for single games. */
  face?: number;
}) {
  const [resign, setResign] = useState(false);
  const observer = !m.my_color;
  const { black, white } = countDiscs(m.cells);
  // Mark the last placed stone (passes place none; skip them).
  const last = [...m.moves].reverse().map(ggsMoveToIndex).find((x) => x !== null) ?? null;
  // Show own rating too; what you read mid-game is the gap.
  const myRate = snap.my_ranks.find((r) => r.gtype === (m.gtype.includes('r') ? '8r' : '8'))?.rating;
  /* Say whose viewpoint: the engine returns mover-side (own) disc
     diff, but a bare number reads as black's — worse in synchro pairs
     where own color flips per board. */
  const myEval = m.last_eval != null
    ? t('ggs.play.my_eval', {
        v: (m.last_from_book ? t('ggs.play.book_mark') + ' ' : '') + (m.last_eval > 0 ? '+' : '')
          + (m.last_eval_exact ? m.last_eval.toFixed(0) : m.last_eval.toFixed(1))
          + (m.last_eval_exact ? ' ' + t('ggs.play.solve_mark') : ''),
      })
    : undefined;

  /* Show the opponent's reported eval too. GGS move rows arrive as
     `3: C2/20.00/122.16` (move/eval/time), so opponents who report
     are readable. The sign stays opponent-side — opposite to ours,
     so one line compares who misreads by how much. Absent when the
     opponent doesn't report. */
  const oppEval = !observer && m.opp_eval != null
    ? t('ggs.play.opp_eval', {
        v: (m.opp_eval > 0 ? '+' : '') + m.opp_eval.toFixed(1)
          + (m.opp_secs_used != null ? ` · ${m.opp_secs_used.toFixed(0)}s` : ''),
      })
    : undefined;

  /* Reported-eval trend. Move numbers as X leave holes (silent
     opponents gap one side), so use sequence indices for a shared
     scale; opponent values negate to own view. */
  const trend = (() => {
    const by = new Map(m.eval_series.map((p) => [p.n, p]));
    const ns = [...by.keys()].sort((a, b) => a - b);
    return ns.map((n, i) => {
      const p = by.get(n)!;
      return {
        x: i,
        mine: p.mine ? p.eval : null,
        opp: p.mine ? null : -p.eval,
      };
    });
  })();

  /* Search progress; it moves only at iteration boundaries, so the
     marker steps with depth. Solve and selective phases show no depth
     (no iterations).

     Ponder values show too. They used to be squashed to 0, which the
     board rendered as "even". Ponder reads the opponent's position
     and is recorded sign-flipped (Progress::flip), so it displays
     directly as own-view disc diff. */
  const busyEval: Record<number, EvalInfo> | undefined =
    m.busy && m.busy_best != null
      ? {
          [m.busy_best]: {
            score: m.busy_eval ?? 0,
            src: m.busy === 'solve' ? { exact: true }
              : m.busy === 'select' ? { exact: true }
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
    // Synchro boards sit side by side; fixed widths always wrap at
    // two, so they share the container and shrink (460px cap for one).
    //
    // A width-only cap overflows short windows: boards are square, so
    // width becomes height, and at 860x560 row 8 and the clocks fell
    // off screen. Cap by window height too (280px is the non-board
    // share — toolbar 44 + status 28 + strips/rows/buttons ~130 +
    // padding).
    <div style={{
      flex: '1 1 300px', minWidth: 260, maxWidth: 'min(460px, calc(100vh - 280px))',
      display: 'flex', flexDirection: 'column',
      // The design frames each board (--panel, radius 11, padding 12);
      // unframed, the two boards read as one.
      background: 'var(--panel)', borderRadius: 'var(--r-4)', padding: 'var(--sp-3)',
    }}>
      {/* Synchro only; own color flips per board. */}
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
      {/* "Me at the bottom" uses own color; observing has no my_color,
          so black sits at the bottom. */}
      {/* Thinking/pondering shows on the board, styled like eval
          display: the single move currently believed best (the
          predicted reply while pondering). In-game αβ shares windows,
          so all-square values cannot exist — only this one move is
          actually known. */}
      <Board cells={m.cells as Cell[]} last={last} disabled
             evals={busyEval}
             /* The marked square must also enter `legal`: the board
                draws only on squares passed as legal, and the game
                screen normally passes none — so pass just this one. */
             legal={busyEval ? Object.keys(busyEval).map(Number) : []}
             coords={prefs.coords} grain={prefs.grain}
             flip={flipped(prefs.facing, m.my_color)} />
      <PlayerRow color={bottom.color === 'black' ? 'b' : 'w'} name={bottom.name || '?'}
                 rate={bottom.rate ? +bottom.rate : undefined}
                 meta={myEval}
                 clock={clock(m.id, bottom.side).text} active={clock(m.id, bottom.side).cls === 'turn'} />
      {/* Reported-eval trend: what both sides thought they were
          winning by, from move one. Raw reports are mover-side, so
          the opponent's negate to own view — unnormalized, the lines
          mirror and disagreements vanish. */}
      {!observer && <EvalTrend points={trend} />}
      <div style={{
        display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)',
        fontSize: 'var(--fs-6)', color: 'var(--sub)',
      }}>
        <span style={{ color: 'var(--text)' }}>{black} – {white}</span>
        <span>{t('ggs.play.moves', { n: m.moves.length })}</span>
        {/* Observation analysis is stored black-view (ggs.rs); say
            whose view, or the sign is unmoored from the board. */}
        {observer && m.watch_eval != null && (
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            {t('ggs.play.analysis')} <StoneDot color="b" />
            {m.watch_eval > 0 ? '+' : ''}{m.watch_eval.toFixed(1)}
            {m.watch_best ? ` (${m.watch_best})` : ''}
          </span>
        )}
        {/* Say the activity in words too; the board mark alone cannot
            show whether depth is moving. */}
        {m.busy === 'think' && (
          <span style={{ color: 'var(--accent)' }}>
            {m.busy_depth > 0
              ? t('ggs.play.thinking_depth', { n: m.busy_depth })
              : t('ggs.play.thinking')}
          </span>
        )}
        {m.busy === 'solve' && <span style={{ color: 'var(--accent)' }}>{t('ggs.play.solving')}</span>}
        {m.busy === 'select' && <span style={{ color: 'var(--accent)' }}>{t('ggs.play.selecting')}</span>}
        {m.busy === 'ponder' && <span style={{ color: 'var(--sub)' }}>{t('ggs.play.pondering')}</span>}
        {!m.busy && snap.thinking === m.id && (
          <span style={{ color: 'var(--accent)' }}>{t('ggs.play.thinking')}</span>
        )}
        {/* Adjournment is not a finish: no margin, so no result — say
            who left. Aborts (mutual) end without a result too. */}
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
        {/* Finished games stay listed and their records are
            fetchable; restored from the old GUI (rule 71). */}
        {/* Pass the archive id too: unreadable local records can be
            re-fetched (synchro brings both boards plus evals).
            Without it this dead-ended at "cannot read record". */}
        <Button
                onClick={() => onKifu(m.opp_name
                                        ? t('ggs.play.game_with', { name: m.opp_name })
                                        : t('ggs.play.record_title'),
                                      m.ggf || m.moves.join(''), m.archive || undefined)}>
          {t('ggs.game_record')}
        </Button>
        {!observer && !m.over && (
          <Button variant="danger"
                  title={t('ggs.play.resign_hint')}
                  onClick={() => setResign(true)}>{t('ggs.resign')}</Button>
        )}
      </div>
      {resign && (
        <Confirm title={t('ggs.play.resign_confirm')} ok={t('ggs.play.resign_ok')} danger
                 body={<>{t('ggs.play.resign_body')}</>}
                 onCancel={() => setResign(false)}
                 onOk={() => { setResign(false); void ggsApi.matchCmd(m.id, 'resign'); }} />
      )}
    </div>
  );
}

/* ---------------- GGS settings ----------------
 *
 * Strength and behavior for GGS play, separate from local games. The
 * request formula lives on the server, so it works with the app
 * closed. */
/** Both pool ratings (8 and 8r) side by side; pool-less screens
    (cards, details, who-list) always show both. */
function bothRates(u: UserRow | undefined): string {
  if (!u) return '';
  // Pull "rating@deviation" from raw finger rows (more precise than
  // the list).
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
  /* One-line result; success and failure share the spot. */
  const [saved, setSaved] = useState('');
  const say = (msg: string) => { setSaved(msg); window.setTimeout(() => setSaved(''), 2500); };
  const [levels, setLevels] = useState({ depth: e.depth, solve: e.solve, band: e.band });
  // Values from global settings (view-only here).
  const threads = e.threads;
  const [ponder, setPonder] = useState(e.ponder);
  const [auto, setAuto] = useState(snap.auto_play);
  const [watch, setWatch] = useState(snap.watch_analysis);
  const [book, setBook] = useState(e.use_book);
  /* Strength mode is a 2-way choice: derive per move from the clock,
     or read at the fixed level. Pacing (slow/even/fast) is a separate
     axis, measured and collapsed to fast — folding this choice in
     with it was a mistake. */
  const [pace, setPace] = useState(e.pace === 'depth' ? 'depth' : 'fast');
  const byClock = pace !== 'depth';
  const [maxMove, setMaxMove] = useState(e.max_move_secs);
  const [reserve, setReserve] = useState(e.reserve_secs);
  const [budgetUse, setBudgetUse] = useState(e.budget_use);
  const [cores, setCores] = useState(0);
  useEffect(() => { api.activity().then((a) => setCores(a.cores)).catch(() => {}); }, []);

  const online = snap.conn === 'online';
  // The server holds the request formula; re-fetch on open.
  const login = snap.login;
  useEffect(() => { if (login) ggsApi.finger(login).catch(() => {}); }, [login]);
  const myForm = (key: 'accept' | 'decline'): string =>
    snap.fingers[login]?.fields
      .find(([k]) => k.replace(/\s+/g, '').replace(/\(.*\)/, '') === key)?.[1] ?? '';
  const saveForm = async (kind: 'aform' | 'dform', expr: string) => {
    try {
      await ggsApi.setFormula(kind, expr);
    } catch (e) {
      say(t('ggs.settings.formula_failed', { error: tErr(e) }));
      return;
    }
    // Don't trust the send; re-fetch the server's value.
    if (login) ggsApi.finger(login).catch(() => {});
  };

  /* Never swallow failures and claim success (rule 34): all five
     calls used to .catch(() => {}) and report success even
     disconnected. */
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

  /* Apply on touch, no apply button. Everything else applies
     immediately; this alone required a press, and a forgotten press
     once sent a game out with ponder still on. Debounced so number
     fields don't send per keystroke. */
  const first = useRef(true);
  useEffect(() => {
    if (first.current) { first.current = false; return; }
    if (!calibrated) return;
    const timer = setTimeout(() => void apply(), 400);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
          {/* Clock-derived mode hides the dials — depth, solve and
              band all derive from time, so the dials would be inert. */}
          {!byClock && (
            <span style={{ maxWidth: 340, display: 'block' }}>
              <Strength value={levels} onChange={setLevels} />
            </span>
          )}
          {/* Thread count is NOT here: GGS uses the global engine
              setting. Separate values would need two calibrations,
              and the uncalibrated one would drop time management to
              the fixed ladder. */}
          <Field label={t('ggs.settings.threads')}>
            <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>
              {threads === 0
                ? t('ggs.settings.threads_auto', { n: Math.max(1, Math.floor(cores / 2)) || '—' })
                : threads}
              {' · '}{t('ggs.settings.threads_hint')}
            </span>
          </Field>
          {/* Ponder works in fixed-depth mode too — it turns "deeper"
              into "faster" (same depth in 1/3 the time, measured). */}
          <Field label={t('ggs.settings.ponder')}>
            <Segmented value={ponder ? 'on' : 'off'}
                       onChange={(v) => setPonder(v === 'on')}
                       options={[{ value: 'on', label: t('ggs.on') },
                                 { value: 'off', label: t('ggs.off') }]} />
          </Field>
          <Note>{t('ggs.settings.ponder_note')}</Note>
        </Section>

        {/* Pacing is not a choice: measured, "slow" scored 0.0% in
            3s/8s games (−34 discs) and "fast" never lost to "even" —
            the menu was one right answer among traps, so it
            collapsed. The mode above is a different axis. */}
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

        {/* Only this section lives on the server and needs a
            connection; strength/clock/book/behavior above are local
            and the disconnected loop accepts them. */}
        <Section title={t('ggs.settings.requests_title')}>
          <Note>{t('ggs.settings.requests_note')}</Note>
          {online ? <>
            <FormulaField label={t('ggs.finger.accept')} src={myForm('accept')}
                          onSave={(x) => void saveForm('aform', x)} />
            <FormulaField label={t('ggs.finger.decline')} src={myForm('decline')}
                          onSave={(x) => void saveForm('dform', x)} />
          </> : (
            <Note>{t('ggs.settings.offline_note')}</Note>
          )}
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

        {/* Connected only; a disabled logout is pointless. */}
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

/* Prose wraps at --w-text (720px) (rule 73); only prose — tables,
 * lists and boards may fill the window. */
/** One formula, edited as a tree, serialized only on save. */
function FormulaField({ label, src, onSave }: { label: string; src: string; onSave: (s: string) => void }) {
  const [cond, setCond] = useState<Cond | null>(() => (src ? parseCond(src) : null));
  /* Keep the escape hatch (rule 30): formulas beyond the tree exist,
   * and without onRaw the component hides the raw editor. */
  const [raw, setRaw] = useState<string | null>(null);
  if (raw !== null) {
    return (
      <Field label={label}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          <TextField mono value={raw} onChange={setRaw}
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

/* ---------------- Results ----------------
 *
 * The backend keeps 200 finished games across restarts; until now
 * they accumulated with nowhere to be read.
 *
 * Rating history on top, game list below. Rows open in study — this
 * screen exists to reread lost games, so the list means little
 * without that path. */
function GgsResults({ snap, onKifu }: {
  snap: GgsSnapshot;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  /* Never default to "all": ratings live in per-pool spaces with no
     combined value, and a mixed line jumps 150 points at each 8/8r
     switch. Default to the most-played format. */
  const [gtype, setGtype] = useState('');
  /* Import server history too: local records only cover games ended
   * in this app, so other machines' games were wholly missing (blank
   * screen). GGS returns everything via /os history. */
  const login = snap.login;
  // Request with '' (= self); the stored key is the login — the
  // backend shares the map with others' histories.
  useEffect(() => { if (login) void ggsApi.history('').catch(() => {}); }, [login]);
  /* Split by rating pool: GGS has exactly two (8 and 8r) — 16- and
     14-ply randoms share 8r (finger's Type table has two rows). A
     per-format split would break one rating line in two. */
  const kinds = [...new Set([
    ...snap.results.map((r) => poolOf(r.base, r.raw)),
    ...(snap.history[snap.login] ?? []).map((h) => poolOf(h.gtype)),
  ])].filter(Boolean);
  /* Fill gaps from server history, matching on ARCHIVE id: local id
     is the game number (.64), history returns archive numbers
     (.84058) — matching those never hit and every game listed twice,
     skewing format and win counts. */
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
        // No local record, but fetchable by id (the rule-71 overlay
        // handles it).
        kifu: '', ggf: '', archive: h.id,
      } as GameResult;
    });
  /* Re-sort by time: concatenating local + server keeps each sorted
     but rewinds at the seam (the graph's x ran 8/16 -> 8/15).
     Timeless entries go last. */
  const all = [...snap.results, ...fromServer].sort((x, y) => (y.at ?? 0) - (x.at ?? 0));
  /* Default (unselected) is the most-played format; never "all" —
     no combined rating exists. */
  let most = '';
  let mostN = -1;
  for (const k of kinds) {
    const n = all.filter((r) => poolOf(r.base, r.raw) === k).length;
    if (n > mostN) { mostN = n; most = k; }
  }
  const cur = gtype || most || 'all';
  const rows = all.filter((r) => cur === 'all' || poolOf(r.base, r.raw) === cur);
  // The graph runs oldest-first; results stack newest-first.
  const rated = rows.filter((r) => r.my_rating != null).reverse();
  const rates = rated.map((r) => r.my_rating as number);
  // X labels; only ends and center render, so pass all.
  const rateDates = rated.map((r) => fmtDay(r.at ?? 0));
  /* Hover text: "which game is this rating" was asked — date,
     opponent and margin identify the table row. */
  const rateLabels = rated.map((r) => {
    const d = r.my_diff;
    const sign = d == null ? '' : d > 0 ? `+${d}` : `${d}`;
    return `${fmtDay(r.at ?? 0)} · ${r.opp} · ${sign} · ${Math.round(r.my_rating as number)}`;
  });
  /* Graph-table linking matches on the game itself, not the point
     index (table newest-first, graph oldest-first). */
  const [hover, setHover] = useState<number | null>(null);
  const hoverKey = hover != null && rated[hover] ? rowKey(rated[hover]) : '';

  return (
    /* Only the list scrolls; whole-screen scrolling floats the graph
       away while you trace the correspondence. */
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column',
                  padding: 'var(--sp-4) var(--sp-4) 0' }}>
      <Section title={t('ggs.results.rating_trend')}
               aside={kinds.length > 1 ? (
                 <Segmented value={cur} onChange={setGtype}
                            options={[...kinds.map((k) => ({ value: k, label: poolLabel(k) })),
                                      { value: 'all', label: t('ggs.filter.all') }]} />
               ) : undefined}>
        {/* Full-width, so the viewBox matches (at 300 the strokes
            stretch). This is the screen's star, so axes show —
            "when was I at what" is the point (settled 2026-08-10;
            dock charts stay axis-less). */}
        {cur === 'all' ? (
          /* No combined rating exists — a line here would step at
             every pool switch. The list keeps its "all" option. */
          <Note>{t('ggs.results.per_pool_note')}</Note>
        ) : (
          <RateChart points={rates} width={800} height={180} axes dates={rateDates}
                     labels={rateLabels} hover={hover} onHover={setHover} />
        )}
      </Section>

      <Section title={t('ggs.results.finished')} aside={<span>{rows.length}</span>} grow>
        {!rows.length && <Empty>{t('ggs.no_records')}</Empty>}
        {/* Rows stay tight. Rebuild on format change: reusing the
            container left stale rows (14 drawn for 11 items, randoms
            squatting in the normal list). */}
        <List key={cur}>
        {rows.map((r) => (
          <ResultRow key={rowKey(r)} opponent={r.opp}
                     // Light the game hovered in the graph.
                     picked={!!hoverKey && rowKey(r) === hoverKey}
                     onHover={(on) => {
                       const i = rated.findIndex((x) => rowKey(x) === rowKey(r));
                       setHover(on ? (i < 0 ? null : i) : null);
                     }}
                     win={(r.my_diff ?? 0) > 0} draw={r.my_diff === 0}
                     discs={r.my_diff ?? 0} when={fmtDay(r.at)}
                     note={cur === 'all' ? gtypeLabel(baseType(r.base, r.raw)) : undefined}
                     rating={r.my_rating}
                     // GGF includes the start position (random-opening
                     // games restore); with neither, fetch by id.
                     onClick={() => onKifu(t('ggs.play.game_with', { name: r.opp }),
                                           r.ggf || r.kifu, r.archive)}
                     dim={!r.ggf && !r.kifu && !r.archive} />
        ))}
        </List>
      </Section>
    </div>
  );
}

/** List row key. Plain concatenation collides: '.6'+'86' and
 * '.68'+'6' both make '.686', and duplicate keys leave stale DOM rows
 * when the filter changes (counts vs rendered rows diverged). */
const rowKey = (r: GameResult) => `${r.id}#${r.seq}`;

/** Rating pool — GGS has exactly two: normal (8) and random (8r).
 * Formats vary (s8r16 / s8r14 / 8r16 ...) but ratings split only on
 * the r; this is the unit for history. */
const poolOf = (base: string, raw?: string) => {
  const kind = baseType(base, raw);
  if (!kind) return '';
  return kind.includes('r') ? '8r' : '8';
};

const poolLabel = (p: string) =>
  (p === '8r' ? t('ggs.gtype.rand_opening') : p === '8' ? t('ggs.pool.normal') : p);

/** Game format. History results carry base like "s8r16.2024...", so
 * take the head. Results built from game-over notices have base like
 * ".11" (a game id) which yields nothing ('?' appeared on screen);
 * their raw row carries the format instead. */
const baseType = (base: string, raw?: string) => {
  /* Prefer the raw row's format: id-shaped bases fall through anyway,
     and even non-id bases can lie — the raw row is right in both
     cases. */
  const fromRaw = raw?.split(/\s+/).find((x) => /^s?8(r\d+)?$/.test(x));
  if (fromRaw) return fromRaw;
  return base.split('.')[0] ?? '';
};

/** History timestamp (`30 Jul 2026 17:36:36`) to `7/30 17:36`.
 * Unparseable input returns as-is — raw text beats a blank when the
 * server's format changes. */
function fmtWhen(at: string): string {
  const ms = Date.parse(at);
  if (!Number.isFinite(ms)) return at;
  const d = new Date(ms);
  const p2 = (n: number) => String(n).padStart(2, '0');
  return `${d.getMonth() + 1}/${p2(d.getDate())} ${p2(d.getHours())}:${p2(d.getMinutes())}`;
}

/** End date; time only if today. */
function fmtDay(secs: number): string {
  if (!secs) return '';
  const d = new Date(secs * 1000);
  const now = new Date();
  const p2 = (n: number) => String(n).padStart(2, '0');
  const same = d.toDateString() === now.toDateString();
  return same ? `${p2(d.getHours())}:${p2(d.getMinutes())}` : `${d.getMonth() + 1}/${p2(d.getDate())}`;
}


/* Player card (design §6's floating one).
 *
 * A preview before the full-screen detail, not a replacement —
 * "details" hands over. Its value is checking an opponent without
 * leaving the board.
 *
 * The design once drew presence / last game / recent 10 / rating
 * history / head-to-head / observe; all dropped once told neither
 * engine nor server exposes them. What remains is finger's four
 * request rows.
 *
 * No match history here: three items per row overflow 340px, and a
 * table has no room. History lives in "details". */
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

  /* Only the four non-formula request rows; formulas render as trees
     and don't fit a 340px card (details holds them). */
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
                {/* Values read out too: the server returns 1 / + / 0,
                    unreadable raw. Details always went through
                    fingerValue; the card alone showed raw. */}
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

/* ---------------- Players ----------------
 *
 * Who-list plus a selected player's detail. The detail header carries
 * rating and game status, not just the name — losing what the list
 * showed forces a round trip before requesting. */
function GgsUsers({ snap, onNav, onKifu }: {
  snap: GgsSnapshot; onNav: (id: NavId) => void;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  /* Three tiers: list -> card -> full detail (§6). Never jump list to
     full — losing the list for a quick look is heavy. card = card,
     sel = full screen. */
  const [card, setCard] = useState<string | null>(null);
  const [mode, setMode] = useState<'who' | 'top'>('who');
  /* Player-list columns; head and rows read the SAME array. '#' is
     rank, present only in top mode — with the condition in two
     places, one fix drifts the columns. */
  const userCols: Col[] = [
    ...(mode === 'top' ? [{ head: '#', w: 26, right: true, num: true } as Col] : []),
    { head: t('ggs.users.col_name'), clip: true },
    // The who-list is pool-less; show both 8 and 8r.
    { head: mode === 'who' ? t('ggs.pool.normal') : t('ggs.users.col_rating'),
      w: 96, right: true, num: true },
    ...(mode === 'who'
      ? [{ head: t('ggs.pool.random'), w: 104, right: true, num: true } as Col] : []),
    // Accepting = "would a request land"; playing players may still
    // accept (open > games in progress), so it is a separate column.
    ...(mode === 'who' ? [{ head: t('ggs.users.col_open'), w: 56, right: true } as Col] : []),
    { head: t('ggs.users.col_status'), w: 52, right: true },
  ];
  /* The status column derives from snap.ongoing, which arrives only
     at login and every 60s — freshly connected, everyone shows blank.
     Re-ask on open; `tell /os match` is a pure query (rule 61: never
     make people wait for what asking solves). */
  useEffect(() => { void ggsApi.listMatches().catch(() => {}); }, []);
  /* Capture entry (KUROOBI_GGS_AUTOVIEW=users:card): the card frame
     without a connection — empty content is fine; the 340px box,
     three tiers and two footer buttons are what's checked. */
  useEffect(() => {
    void ggsApi.autoview().then((v) => {
      /* The first segment must spell the destination id: after the
         ggs-users -> ggs-players rename this stayed stale, users:card
         missed, and the body went blank on a navless destination. */
      if (v === 'players:card') setCard(snap.login || '—');
      if (v === 'players:top') setMode('top');
    }).catch(() => {});
  }, [snap.login]);
  const [sel, setSel] = useState<string | null>(null);
  // Rankings are per-pool (mixed ranks mean nothing); the pool is
  // selectable, and own rating shows both.
  const [pool, setPool] = useState('8');
  // Paged, not scrolled (rule 74) — scrolling loses "what rank am I
  // looking at".
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
                 {/* Pool select only in top mode; the who-list is the
                     same people regardless, so showing it would lie. */}
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
        {/* §5 draws this as a TABLE — headers (#/name/rating/status)
            and 24px rows (rule 5). It used to be headerless 32px rows
            with drifting columns. '#' only in top mode. */}
        {!!rows.length && (
          <TableHead cols={userCols} pad="var(--sp-4)" />
        )}
        {/* Never bare headers; right after connect the list hasn't
            arrived. */}
        {!slice.length && (
          <Empty>{mode === 'who' ? t('ggs.users.none_online') : t('ggs.users.no_ranking')}</Empty>
        )}
        {/* Rows stay tight (the section-gap trap). */}
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
            {/* Deviation always accompanies ratings (rule 29). /os t
                returns it, /os who does not — the who-list shows bare
                numbers. Who-list shows both pools; rankings use the
                selected one. */}
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
            {/* Accepting status: who marks names with + / - / x
                (ghost). Idle players may still refuse, and not
                knowing wastes a request. */}
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
            {/* Status as colored text (badges change row height).
                Non-playing players say "idle" — blanks read as a
                broken column (§5 draws the two values too). */}
            <span style={{
              fontSize: 'var(--fs-6)', color: playing ? 'var(--ok)' : 'var(--sub)',
            }}>{playing ? t('ggs.state.playing') : t('ggs.state.idle')}</span>
          </TableRow>
        );})}
        </List>
        {/* Paging sits outside the list — flush below, it reads as a
            row. */}
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
  // Own history sits under the login key like everyone's (only the
  // request uses '').
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
      {/* Header band: name, rating, status and request in one row. */}
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
            {/* 24 uniform rows bury what matters pre-request; grouping
                and order come from ggs.ts's FINGER_GROUPS. */}
            {fingerGroups(fields).map((g) => (
              <Section key={g.title} title={g.title}>
                {g.rows.map((r) => {
                  const key = normKey(r.key).replace(/\(.*\)/, '');
                  // Formulas render as trees, never flattened — the
                  // structure is the meaning.
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
                        {cond ? <FormulaView node={cond} top />
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
            {/* As a table: dot-joined two-liners scatter date, format
                and margin per row. Clicking opens the record overlay
                (fetched by id — nothing local). */}
            {!!rows.length && <TableHead cols={histCols} />}
            <List>
            {rows.map((h) => {
              const black = h.black === name;
              const d = parseFloat(h.score);
              // Margin from this player's view; black-side records
              // arrive black-view.
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

/** Chat's left column — the conversation list, at --w-lobby (174px),
 *  never borrowing the match list's --w-list (rule 6). Extracted from
 *  the 187-line GgsChat; four props suffice. */
function ChatList({ sorted, cur, onThread, onPick }: {
  sorted: [string, { last: ChatMsg; n: number }][];
  cur: string;
  onThread: (t: string) => void;
  onPick: (v: boolean) => void;
}) {
  return (
    <>
    {/* --w-lobby (174px), not the match list's --w-list (rule 6).
        §6 draws this column at 173px — the token's "GGS left column"
        description meant here (the lobby has no 174px column). Names
        clip with ellipsis. */}
    <aside style={{
      width: 'var(--w-lobby)', flex: 'none', borderRight: '1px solid var(--border)',
      minHeight: 0, display: 'flex', flexDirection: 'column',
    }}>
      {/* Heading button is chip (20px) — a 28px one fights the title
          in 174px. The design's 21px band sinks buttons into the
          rule, so this band alone is 32px. Needs a push. */}
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
