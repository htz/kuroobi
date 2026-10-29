import { useEffect, type ReactNode } from 'react';
import { ggsApi } from '../api';
import type { GgsSnapshot } from '../types';
import { gtypeLabel, onlineRows, ratingOf } from '../ggs';
import { t } from '../i18n';
import { Divider, Section, Toolbar } from '../components/layout';
import { Button, Toggle } from '../components/primitives';
import { Tag, poolLabel, type NavId } from '../components/ggs';
import { askSheet } from '../AskSheet';
import { Band, Cell, HeadRow, Line, ListNote, Page, Rows, Sections, Table, condLine, useNow, act } from '../ggsui';

const rate = (snap: GgsSnapshot, name: string, gtype: string): string => {
  const u = onlineRows(snap).find((x) => x.name === name);
  return u ? ratingOf(u, gtype.includes('r')) : '';
};
/** The rating a request line carries just before a name ("1724.5 regu"), for a player not in the list yet. */
const offerRate = (raw: string, name: string): string => {
  const w = raw.split(/\s+/);
  const i = w.indexOf(name);
  return i > 0 && /^\d+(\.\d+)?$/.test(w[i - 1]) ? w[i - 1] : '';
};

/** "Random 2051.2": in the to-do list, which rating the number is. */
const poolRate = (snap: GgsSnapshot, name: string, gtype: string, raw = ''): string => {
  const r = rate(snap, name, gtype) || offerRate(raw, name);
  return r ? `${poolLabel(gtype.includes('r') ? '8r' : '8')} ${r}` : '';
};

let wanted = '';
/** Remembers which game to show when the game screen opens next. */
export const focusGame = (id: string) => { wanted = id; };
export const takeFocus = (): string => { const w = wanted; wanted = ''; return w; };

/** Things waiting for me first (adjourned games, requests to me, my own requests), then games to watch and open requests. */
export function Lobby({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
  useEffect(() => {
    void ggsApi.listMatches().catch(() => {});
    void ggsApi.listStored().catch(() => {});
  }, []);
  const me = snap.login;
  const incoming = snap.offers.filter((o) => o.incoming);
  const mine = snap.offers.filter((o) => !o.incoming && o.names[0] === me);
  // Requests to anybody: only the asker is named.
  const open = snap.offers.filter((o) => !o.incoming && o.names[0] !== me && o.names.length === 1);
  const todo = snap.stored.length + incoming.length + mine.length;
  const games = snap.ongoing;
  const now = useNow(incoming.length + mine.length > 0, 1000);
  // GGS does not say when a request was made; the backend notes when it first heard of it.
  const age = (id: string) => {
    const since = snap.offers.find((o) => o.id === id)?.since_ms || now;
    return Math.max(0, Math.floor((now - since) / 1000));
  };
  const ago = (id: string) => { const m = Math.floor(age(id) / 60); return m < 1 ? t('lobby.just_now') : t('lobby.minutes_ago', { n: m }); };
  const waited = (id: string) => { const s = age(id); return t('lobby.waiting_for', { t: `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}` }); };

  return (
    <>
      <Toolbar end={<Button variant="ghost" onClick={() => {
        void ggsApi.listMatches().catch(() => {});
        void ggsApi.listStored().catch(() => {});
      }}>{t('ggs.refresh')}</Button>}>
        <Button variant="primary" onClick={() => askSheet.open({})}>{t('ask.title')}</Button>
        <Divider />
        <Toggle checked={snap.open} label={t('ggs.lobby.open_toggle')} onChange={(on) => act(ggsApi.setOpen(on))} />
      </Toolbar>
      <Page>
        <Sections>
          {!snap.open && <Band><b style={{ fontWeight: 600 }}>{t('lobby.closed')}</b></Band>}

          <Section title={t('lobby.todo')} aside={t('lobby.count', { n: todo })}>
            <div className="k-scroll" style={{ display: 'flex', flexDirection: 'column', maxHeight: 'calc(var(--h-row2) * 4)' }}>
              {todo === 0 && <ListNote>{t('lobby.nothing')}</ListNote>}
              {snap.stored.map((x) => (
                <Todo key={'s' + x.id} kind={t('ggs.state.adjourned')} tone="sub" name={x.opp || '?'}
                      rate={poolRate(snap, x.opp, x.gtype)} cond={gtypeLabel(x.gtype)}>
                  <Button variant="primary" onClick={() => act(ggsApi.resumeStored(x.id))}>{t('ggs.lobby.resume')}</Button>
                </Todo>
              ))}
              {incoming.map((o) => {
                const who = o.names.filter((n) => n !== me).join(t('ggs.and_separator')) || '?';
                return (
                  <Todo key={'i' + o.id} kind={t('lobby.kind_in')} tone="accent" mark name={who}
                        rate={poolRate(snap, who, o.gtype, o.raw)} cond={condLine(o.gtype, o.time, o.rated)} note={ago(o.id)}>
                    <Button variant="ghost" onClick={() => act(ggsApi.decline(o.id))}>{t('ggs.lobby.decline')}</Button>
                    <Button variant="primary" onClick={() => act(ggsApi.accept(o.id))}>{t('ggs.lobby.accept')}</Button>
                  </Todo>
                );
              })}
              {mine.map((o) => {
                const to = o.names.slice(1).join(t('ggs.and_separator')) || t('ask.anyone_short');
                return (
                  <Todo key={'o' + o.id} kind={t('lobby.kind_out')} tone="sub" name={to}
                        rate={poolRate(snap, to, o.gtype, o.raw)} cond={condLine(o.gtype, o.time, o.rated)}
                        note={waited(o.id)}>
                    <Button variant="ghost" onClick={() => act(ggsApi.raw(`tell /os cancel ${o.id}`))}>{t('lobby.cancel')}</Button>
                  </Todo>
                );
              })}
            </div>
          </Section>

          <div style={{ flex: 1, minHeight: 0, display: 'flex', gap: 'var(--sp-4)' }}>
            <div style={{ flex: 3, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
              <Section title={t('ggs.lobby.ongoing_title')} aside={t('ggs.lobby.game_count', { n: games.length })} grow>
                <Table>
                  <HeadRow cols={[[t('lobby.player'), undefined], [t('lobby.player'), undefined],
                                  [t('ggs.field.format'), 128], ['', 64]]} />
                  {!games.length && <ListNote>{t('lobby.no_games')}</ListNote>}
                  <Rows>
                    {games.map((o) => (
                      <Line key={o.id}>
                        <NameRate name={o.names[0] ?? '?'} rate={o.ratings[0] ?? ''} />
                        <NameRate name={o.names[1] ?? '?'} rate={o.ratings[1] ?? ''} />
                        <Cell w={128} sub>{gtypeLabel(o.gtype)}</Cell>
                        <span style={{ width: 64, flex: 'none', display: 'flex', justifyContent: 'flex-end' }}>
                          {/* A game being played is already on the game screen: the row opens it there. */}
                          {o.mine ? (
                            <Button size="row" variant="secondary"
                                    onClick={() => { focusGame(o.id); onNav('ggs-play'); }}>
                              {t('lobby.open_game')}
                            </Button>
                          ) : (
                            <Button size="row" variant={o.watching ? 'ghost' : 'secondary'}
                                    onClick={() => {
                                      const on = !o.watching;
                                      act(ggsApi.watch(o.id, on));
                                      if (on) { focusGame(o.id); onNav('ggs-play'); }
                                    }}>
                              {o.watching ? t('ggs.stop_observing') : t('ggs.observe')}
                            </Button>
                          )}
                        </span>
                      </Line>
                    ))}
                  </Rows>
                </Table>
              </Section>
            </div>
            <div style={{ flex: 2, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
              <Section title={t('lobby.open_requests')} aside={t('lobby.count', { n: open.length })} grow>
                <Table>
                  {!open.length && <ListNote>{t('lobby.no_open')}</ListNote>}
                  <Rows>
                    {open.map((o) => (
                      <Line key={o.id} height="row2">
                        <span style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', lineHeight: 1.3 }}>
                          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                            <b className="k-sel" style={{ fontWeight: 600 }}>{o.names[0]}</b>{' '}
                            <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{rate(snap, o.names[0], o.gtype)}</span>
                          </span>
                          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                            {condLine(o.gtype, o.time, o.rated)}
                          </span>
                        </span>
                        <Button size="row" onClick={() => act(ggsApi.accept(o.id))}>{t('ggs.lobby.accept')}</Button>
                      </Line>
                    ))}
                  </Rows>
                </Table>
              </Section>
            </div>
          </div>
        </Sections>
      </Page>
    </>
  );
}

/** A player in a table cell: the name gives way first, the rating stays whole. */
function NameRate({ name, rate: r }: { name: string; rate: string }) {
  return (
    <span style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'baseline', gap: 'var(--sp-1)', whiteSpace: 'nowrap' }}>
      <span className="k-sel" style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>{name}</span>
      {r && <span style={{ flex: 'none', color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{r}</span>}
    </span>
  );
}

function Todo({ kind, tone, mark, name, rate: r, cond, note, children }: {
  kind: string; tone: 'accent' | 'sub'; mark?: boolean; name: string; rate: string; cond: string; note?: string;
  children: ReactNode;
}) {
  return (
    <Line height="row2" mark={mark}>
      <span style={{ flex: 'none', width: 56, display: 'flex' }}><Tag tone={tone}>{kind}</Tag></span>
      <span style={{ flex: 'none', width: 180, minWidth: 0, display: 'flex', flexDirection: 'column', lineHeight: 1.3 }}>
        <b className="k-sel" style={{ fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis' }}>{name}</b>
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{r}</span>
      </span>
      <span style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', lineHeight: 1.3, overflow: 'hidden' }}>
        <span style={{ fontSize: 'var(--fs-5)', overflow: 'hidden', textOverflow: 'ellipsis' }}>{cond}</span>
        {note && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{note}</span>}
      </span>
      <span style={{ flex: 'none', display: 'flex', gap: 'var(--sp-2)' }}>{children}</span>
    </Line>
  );
}
