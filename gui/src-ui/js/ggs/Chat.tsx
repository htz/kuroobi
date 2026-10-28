import React, { useEffect, useRef, useState } from 'react';
import { ggsApi, jsLog } from '../api';
import type { ChatMsg, GgsSnapshot } from '../types';
import { hasJapanese, onlineRows, translate } from '../ggs';
import { t, dateLocale, useLang } from '../i18n';
import { Caption, Divider, ListRow, popoverStyle, Toolbar, TwoLines, useDismiss } from '../components/layout';
import { Badge, Button, TextField, Toggle } from '../components/primitives';
import { Bubble, DayMark, Tag } from '../components/ggs';
import { Blank, ListPane, Page, reportGgsFailure, ListNote } from '../ggsui';
import { clock, whenChat } from '../when';

let wanted = '';
/** Opens a conversation with `name` the next time the chat screen shows. */
export const openChat = (name: string) => { wanted = name; };

const trKey = (c: ChatMsg): string => c.from + '|' + c.text;
const threadOf = (c: ChatMsg): string => c.thread || c.chan || c.from;
/** A conversation's last message: the time today, 昨日 yesterday, the date before that. */

// What each conversation was read up to, kept per account on this machine.
const seenKey = (login: string) => `kuroobi.chat.seen.${login}`;
function readSeen(login: string): Record<string, number> {
  try { return JSON.parse(localStorage.getItem(seenKey(login)) ?? '{}') ?? {}; } catch { return {}; }
}
function writeSeen(login: string, v: Record<string, number>) {
  try { localStorage.setItem(seenKey(login), JSON.stringify(v)); } catch { /* storage off */ }
}

export function Chat({ snap }: { snap: GgsSnapshot }) {
  const login = snap.login;
  const [thread, setThread] = useState(() => { const w = wanted; wanted = ''; return w || '.chat'; });
  const [text, setText] = useState('');
  const [toEn, setToEn] = useState(false);
  // Translations asked for one message at a time, into the app's language: null while it is on its way.
  const lang = useLang();
  const [trs, setTrs] = useState<Record<string, { text: string | null; failed?: boolean; shown: boolean }>>({});
  const [picker, setPicker] = useState(false);
  const pickerBox = useRef<HTMLDivElement>(null);
  const pickerBtn = useRef<HTMLSpanElement>(null);
  useDismiss(picker, () => setPicker(false), pickerBox, pickerBtn);
  const [query, setQuery] = useState('');
  const [seen, setSeen] = useState<Record<string, number>>(() => readSeen(login));
  const box = useRef<HTMLDivElement>(null);

  const threads = new Map<string, { last: ChatMsg | null; unread: number }>();
  threads.set('.chat', { last: null, unread: 0 });
  if (thread !== '.chat') threads.set(thread, { last: null, unread: 0 });
  for (const c of snap.chat) {
    const key = threadOf(c);
    const prev = threads.get(key) ?? { last: null, unread: 0 };
    const base = seen[key] ?? snap.chat_seen;
    threads.set(key, { last: c, unread: prev.unread + (c.from !== login && c.at > base ? 1 : 0) });
  }
  const cur = threads.has(thread) ? thread : '.chat';
  const list = [...threads.entries()].sort((a, b) =>
    a[0] === '.chat' ? -1 : b[0] === '.chat' ? 1 : (b[1].last?.at ?? Infinity) - (a[1].last?.at ?? Infinity));
  const msgs = snap.chat.filter((c) => threadOf(c) === cur);

  // Leaving a conversation, or opening one, marks it read up to its last message.
  const lastAt = (key: string) => {
    const xs = snap.chat.filter((c) => threadOf(c) === key);
    return xs.length ? xs[xs.length - 1].at : 0;
  };
  const go = (key: string) => {
    const next = { ...seen, [cur]: Math.max(seen[cur] ?? 0, lastAt(cur)), [key]: Math.max(seen[key] ?? 0, lastAt(key)) };
    setSeen(next);
    writeSeen(login, next);
    setThread(key);
  };

  const count = msgs.length;
  useEffect(() => {
    const b = box.current;
    if (b) b.scrollTop = b.scrollHeight;
  }, [count, cur]);

  const toggleTr = (c: ChatMsg) => {
    const key = trKey(c) + '|' + lang;
    const had = trs[key];
    if (had && had.text !== null && !had.failed) {
      setTrs((prev) => ({ ...prev, [key]: { ...had, shown: !had.shown } }));
      return;
    }
    setTrs((prev) => ({ ...prev, [key]: { text: null, shown: true } }));
    translate(c.text, lang)
      .then((tr) => setTrs((prev) => ({ ...prev, [key]: { text: tr || c.text, shown: true } })))
      .catch((e) => {
        jsLog('translation failed: ' + e);
        setTrs((prev) => ({ ...prev, [key]: { text: t('ggs.chat.translate_failed'), failed: true, shown: true } }));
      });
  };
  const trOf = (c: ChatMsg) => trs[trKey(c) + '|' + lang];

  const send = async () => {
    let msg = text.trim();
    if (!msg) return;
    setText('');
    if (toEn && hasJapanese(msg)) {
      try { msg = (await translate(msg, 'en')) || msg; } catch (e) { jsLog('translation failed: ' + e); }
    }
    // A message that did not go out comes back to the box, and says why.
    ggsApi.chat(cur, msg).catch((e) => { setText(msg); reportGgsFailure(e); });
  };

  const rows: { c: ChatMsg; day: string; dayHead: boolean; head: boolean }[] = [];
  let lastFrom = '', lastDay = '';
  for (const c of msgs) {
    const day = c.at ? new Date(c.at * 1000).toLocaleDateString(dateLocale(),
      { month: 'long', day: 'numeric', weekday: 'short' }) : '';
    const dayHead = !!day && day !== lastDay;
    if (dayHead) { lastDay = day; lastFrom = ''; }
    const head = c.from !== lastFrom;
    lastFrom = c.from;
    rows.push({ c, day, dayHead, head });
  }
  const people = onlineRows(snap).map((u) => u.name)
    .filter((n) => n !== login && (!query || n.toLowerCase().includes(query.toLowerCase())));
  const title = (key: string) => (key === '.chat' ? t('ggs.chat.global') : key);

  return (
    <>
      <Toolbar>
        <b style={{ fontSize: 'var(--fs-3)', fontWeight: 600, whiteSpace: 'nowrap' }}>{title(cur)}</b>
        <Tag tone="sub">{cur === '.chat' ? t('ggs.chat.to_everyone') : t('ggs.chat.to_person')}</Tag>
        <Divider />
        <Toggle checked={toEn} onChange={setToEn} label={t('ggs.chat.translate_out')} />
      </Toolbar>
      <Page row>
        <ListPane head={
          <div style={{ flex: 'none', height: 'var(--h-field)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                        padding: '0 var(--sp-2) 0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)' }}>
            <Caption>{t('ggs.chat.threads')}</Caption>
            <span ref={pickerBtn} style={{ marginLeft: 'auto' }}>
              <Button size="row" onClick={() => { setPicker(!picker); setQuery(''); }}>{t('ggs.chat.new_thread')}</Button>
            </span>
          </div>
        }>
          {list.map(([key, info]) => {
            const on = key === cur;
            return (
              <ListRow key={key} height="row2" on={on} onClick={() => go(key)}>
                <TwoLines top={<>
                  <span style={{ fontWeight: info.unread ? 600 : 400, overflow: 'hidden', textOverflow: 'ellipsis' }}>{title(key)}</span>
                  {info.last && <span style={{ marginLeft: 'auto', flex: 'none', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{whenChat(info.last.at)}</span>}
                </>} bottom={info.last?.text || '—'} />
                {info.unread > 0 && !on && <Badge>{info.unread}</Badge>}
              </ListRow>
            );
          })}
          {picker && (
            <div ref={pickerBox} role="dialog" aria-label={t('ggs.chat.pick_title')} style={{
              position: 'absolute', zIndex: 2, top: 'calc(var(--h-field) + var(--sp-1))', left: 'var(--sp-2)',
              width: 'var(--w-popover)', ...popoverStyle,
            }}>
              <div style={{ padding: 'var(--sp-2)', borderBottom: '1px solid var(--border-weak)', display: 'flex' }}>
                <TextField value={query} onChange={setQuery} placeholder={t('chat.find_online')}
                           onEnter={() => { if (people[0]) { go(people[0]); setPicker(false); } }} />
              </div>
              <div className="k-scroll" style={{ maxHeight: 320, padding: 'var(--sp-1) 0' }}>
                {!people.length && <ListNote>{t('ggs.users.nobody')}</ListNote>}
                {people.map((n) => (
                  <ListRow key={n} height="row" rule={false} onClick={() => { go(n); setPicker(false); }}>{n}</ListRow>
                ))}
              </div>
            </div>
          )}
        </ListPane>
        <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
          {!rows.length
            ? <Blank title={cur === '.chat' ? t('ggs.chat.empty_global') : t('chat.empty_thread', { name: cur })}
                     body={cur === '.chat' ? undefined : t('chat.empty_help')} />
            : (
              <div className="k-scroll" ref={box} style={{ flex: 1, minHeight: 0, padding: 'var(--sp-4)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
                {rows.map(({ c, day, dayHead, head }, i) => {
                  const mine = c.from === login;
                  const tr = trOf(c);
                  const shown = tr?.shown && tr.text !== null;
                  return (
                    <React.Fragment key={i}>
                      {dayHead && <DayMark>{day}</DayMark>}
                      <Bubble showName={head} m={{
                        from: c.from, mine, at: clock(c.at), body: c.text,
                        tr: shown ? tr.text ?? undefined : undefined, trFailed: shown && tr.failed,
                      }} action={mine ? undefined
                        : tr && tr.text === null ? { label: t('ggs.chat.translating') }
                        : { label: shown && !tr.failed ? t('ggs.chat.hide_translation') : t('ggs.chat.translate'),
                            onClick: () => toggleTr(c) }} />
                    </React.Fragment>
                  );
                })}
              </div>
            )}
          <div style={{ flex: 'none', display: 'flex', gap: 'var(--sp-2)', alignItems: 'center',
                        padding: 'var(--sp-3) var(--sp-4)', borderTop: '1px solid var(--border)' }}>
            <span style={{ flex: 1, minWidth: 0, display: 'flex' }}>
              <TextField value={text} onChange={setText} onEnter={() => void send()} placeholder={t('ggs.chat.placeholder')} />
            </span>
            <Button size="field" variant="primary" onClick={() => void send()}>{t('ggs.send')}</Button>
          </div>
        </div>
      </Page>
    </>
  );
}
