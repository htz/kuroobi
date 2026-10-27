import { useCallback, useEffect, useRef, useState } from 'react';
import { ggsApi } from '../api';
import type { GgsSnapshot, TourneyView } from '../types';
import { gtypeLabel, onlineRows } from '../ggs';
import { t } from '../i18n';
import { ListRow, Toolbar, TwoLines } from '../components/layout';
import { Button, Dot, Progress, Segmented, TextField } from '../components/primitives';
import { Tag, type NavId } from '../components/ggs';
import { askSheet } from '../AskSheet';
import { focusGame } from './Lobby';
import { act, Band, Blank, Cell, HeadRow, Line, ListHead, ListNote, ListPane, Page, Rows, TitleBit, useNow, useWidth } from '../ggsui';

const td = (cmd: string) => act(ggsApi.raw(`tell /td ${cmd}`));


const mmss = (secs: number): string => {
  const s = Math.max(0, Math.round(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

const num = (v: string | number): string => {
  const n = typeof v === 'number' ? v : parseFloat(v);
  if (!isFinite(n)) return String(v);
  const s = Number.isInteger(n) ? String(n) : n.toFixed(1);
  return n > 0 ? '+' + s : s;
};

const tdErrors: [RegExp, string][] = [
  [/does not exist/, 'ggs.tourney.err.not_exist'],
  [/already in the tournament/, 'ggs.tourney.err.already'],
  [/after tournament starts|tournament is over/, 'ggs.tourney.err.started'],
  [/registered/, 'ggs.tourney.err.unregistered'],
];

const stateTone = (x: TourneyView): 'accent' | 'ok' | 'sub' =>
  x.state === 'running' ? 'accent' : x.state === 'break' || x.state === 'announced' ? 'ok' : 'sub';
const beforeStart = (x: TourneyView): boolean => x.state === 'announced' || x.state === '';
const finished = (x: TourneyView): boolean => x.state === 'over' || x.state === 'cancelled';

type RankRow = { pts: string; w: number; d: number; l: number; disc: string; name: string; tb: string };
function parseRankings(text: string): RankRow[] {
  return text.split('\n').flatMap((l) => {
    const m = /^\s*([\d.]+)\s*\(\s*(\d+)\s+(\d+)\s+(\d+)\)\s*\{\s*([-+\d.]+)\s*\}\s*(\S+)\s*\[([\d.]+)\]/.exec(l);
    return m ? [{ pts: m[1], w: +m[2], d: +m[3], l: +m[4], disc: m[5], name: m[6], tb: m[7] }] : [];
  });
}
function parseFinger(text: string): Map<string, string> {
  return new Map(text.split('\n').flatMap((l) => {
    const m = /^\s*(\w+)\s+:\s*(.*)$/.exec(l);
    return m ? [[m[1], m[2].trim()] as [string, string]] : [];
  }));
}
type SchedRow = { round: number; game: string; a: string; b: string; raw: string; result: string; bye?: boolean };
const tdResult = (r: string): string =>
  r === '?' ? t('ggs.tourney.result_pending')
    : r === 'unstarted' ? t('ggs.tourney.result_unstarted')
      : r === 'bye' ? t('ggs.tourney.bye') : r;
function parseSchedule(text: string): SchedRow[] {
  const rows: SchedRow[] = [];
  let round = 0;
  for (const l of text.split('\n')) {
    const r = /^\s*round\s+(\d+):/.exec(l);
    const bye = /^\s*(\d+\.\d+)\s+(\S+)\s+rests\s*$/.exec(l);
    const g = /^\s*(\d+\.\d+)\s+(\S+)\s+(\S+)\s+(\S+)\s*$/.exec(l);
    if (r) round = +r[1];
    else if (bye) rows.push({ round, game: bye[1], a: bye[2], b: '', raw: '', result: '', bye: true });
    else if (g) rows.push({ round, game: g[1], a: g[2], b: g[3], raw: g[4], result: tdResult(g[4]) });
  }
  return rows;
}

/** A finished pairing's result from the left player's side; a synchro pair is the sum of its two games. */
const resultOf = (r: SchedRow, synchro: boolean): string => {
  const v = parseFloat(r.raw);
  if (!isFinite(v)) return r.bye ? t('ggs.tourney.bye') : r.result;
  return num(v * (synchro ? 2 : 1));
};

export function Tourney({ snap, onNav }: { snap: GgsSnapshot; onNav: (id: NavId) => void }) {
  // Countdowns are in seconds.
  const now = useNow(true, 1000) / 1000;
  const [sel, setSel] = useState<number | null>(null);
  useEffect(() => { td('t'); }, []);
  const all = snap.td.tourneys;
  const live = all.filter((x) => !finished(x));
  const past = all.filter(finished);
  const x = all.find((y) => y.id === sel) ?? live[0] ?? past[0];
  const err = snap.td.reply?.err && now - snap.td.reply.at <= 60 ? snap.td.reply : null;
  const errText = err ? (() => { const k = tdErrors.find(([re]) => re.test(err.text))?.[1]; return k ? t(k) : t('ggs.tourney.err.other', { text: err.text }); })() : '';
  const myLive = x ? snap.td.games.find((g) => g.tid === x.id && g.diff == null) : undefined;

  const row = (y: TourneyView) => {
    const on = y.id === x?.id;
    const left = y.next_at != null ? mmss(y.next_at - now) : '';
    const sub = beforeStart(y) ? [left ? t('ggs.tourney.starts_in', { left }) : '', y.n_players ? t('ggs.tourney.players', { n: y.n_players }) : '']
      : finished(y) ? [y.winner ? t('ggs.tourney.winner', { name: y.winner }) : '', y.n_players ? t('ggs.tourney.players', { n: y.n_players }) : '']
      : [y.rounds ? t('ggs.tourney.rounds', { done: y.state === 'running' ? y.rounds_played + 1 : y.rounds_played, all: y.rounds }) : '',
         y.n_players ? t('ggs.tourney.players', { n: y.n_players }) : ''];
    return (
      <ListRow key={y.id} height="row2" on={on} onClick={() => setSel(y.id)}>
        <TwoLines top={<><b style={{ fontWeight: 600 }}>{t('ggs.tourney.name', { id: y.id })}</b>{y.joined && <Dot tone="gold" />}</>}
                  bottom={sub.filter(Boolean).join(' · ')} />
        {beforeStart(y) && !y.joined && (
          <span onClick={(e) => e.stopPropagation()}>
            <Button size="row" variant="primary" onClick={() => td(`join ${y.id}`)}>{t('ggs.tourney.join')}</Button>
          </span>
        )}
        {y.state && <Tag tone={stateTone(y)}>{t(`ggs.tourney.state.${y.state}`)}</Tag>}
      </ListRow>
    );
  };

  return (
    <>
      <Toolbar end={<>
        {myLive && <Button variant="primary" onClick={() => { focusGame(myLive.id); onNav('ggs-play'); }}>{t('ggs.tourney.open_game')}</Button>}
        {x && beforeStart(x) && (x.joined
          ? <Button onClick={() => td(`withdraw ${x.id}`)}>{t('ggs.tourney.withdraw')}</Button>
          : <Button variant="primary" onClick={() => td(`join ${x.id}`)}>{t('ggs.tourney.join')}</Button>)}
        <Button variant="ghost" onClick={() => { td('t'); if (x) { td(`f ${x.id}`); td(`r ${x.id}`); td(`st ${x.id}`); } }}>{t('ggs.refresh')}</Button>
      </>}>
        {x ? <>
          <TitleBit title={t('ggs.tourney.name', { id: x.id })} />
          {/* The same state word as the list on the left. */}
          {x.state && <Tag tone={stateTone(x)}>{t(`ggs.tourney.state.${x.state}`)}</Tag>}
          {x.joined && !finished(x) && <Tag tone="ok">{t('ggs.tourney.joined')}</Tag>}
        </> : <TitleBit title={t('ggs.nav.tourney')} />}
      </Toolbar>
      <Page row>
        <ListPane>
          <ListHead>{t('tourney.group_live')}</ListHead>
          {!live.length && <ListNote>{t('ggs.tourney.none')}</ListNote>}
          {live.map(row)}
          {past.length > 0 && <ListHead>{t('ggs.tourney.past')}</ListHead>}
          {past.map(row)}
        </ListPane>
        {!x ? <Blank title={t('tourney.none_title')} body={t('tourney.none_body')} /> : (
          <TourneyDetail key={x.id} snap={snap} x={x} now={now} onNav={onNav} errText={errText} />
        )}
      </Page>
    </>
  );
}

function TourneyDetail({ snap, x, now, onNav, errText }: {
  snap: GgsSnapshot; x: TourneyView; now: number; onNav: (id: NavId) => void; errText: string;
}) {
  const started = !beforeStart(x) && x.state !== 'cancelled';
  const refresh = useCallback(() => {
    td(`f ${x.id}`);
    if (started) { td(`r ${x.id}`); td(`st ${x.id}`); }
  }, [x.id, started]);
  useEffect(refresh, [refresh, x.rounds_played, x.state]);

  const me = snap.login;
  const ranks = parseRankings(x.rankings);
  const sched = parseSchedule(x.schedule);
  const info = parseFinger(x.info);
  const type = x.gtype || info.get('type') || '';
  const synchro = type.startsWith('s');
  const all = x.rounds || Math.max(0, ...sched.map((r) => r.round));
  const cur = x.state === 'running' ? x.rounds_played + 1 : Math.max(1, x.rounds_played);
  const [pane, setView] = useState<'rank' | 'mine' | 'sched'>('sched');
  // Too narrow for the ranking and the schedule side by side: one at a time, picked by tabs.
  const [boxRef, boxW] = useWidth();
  const narrow = boxW < 640;
  const view = !narrow && pane === 'rank' ? 'sched' : pane;
  // The schedule follows the current round until another one is picked.
  const [pinned, setPinned] = useState<number | null>(null);
  const round = pinned ?? cur;
  const [typed, setTyped] = useState<string | null>(null);
  const roundText = typed ?? String(round);
  const setRoundText = (v: string) => setTyped(v);
  const [who, setWho] = useState('');
  const rankBox = useRef<HTMLDivElement>(null);
  const mineBox = useRef<HTMLDivElement>(null);

  const go = (n: number) => {
    const v = Math.max(1, Math.min(all || 1, n));
    setPinned(v === cur ? null : v); setTyped(null);
  };
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA')) return;
      if (view !== 'sched' || who) return;
      if (e.key === 'ArrowLeft') { e.preventDefault(); go(round - 1); }
      if (e.key === 'ArrowRight') { e.preventDefault(); go(round + 1); }
    };
    window.addEventListener('keydown', on);
    return () => window.removeEventListener('keydown', on);
  });
  // "My games" opens with the current round in view.
  useEffect(() => {
    if (view !== 'mine') return;
    mineBox.current?.querySelector('[data-cur]')?.scrollIntoView({ block: 'center' });
  }, [view]);

  const clock = x.clock || info.get('clock') || '';
  const director = x.director || info.get('director') || '';
  const rules: [string, string][] = [
    [t('ggs.field.format'), type ? gtypeLabel(type) : '—'],
    [t('ggs.field.time_control'), clock ? clock.replace(/\/\//, ' +').replace(/\//g, ' ') : '—'],
    [t('tourney.pairing'), x.rounds ? t('ggs.tourney.rrobin', { n: x.rounds }) : '—'],
    ...(info.get('breaks') ? [[t('tourney.breaks'), info.get('breaks')!.trim()] as [string, string]] : []),
    [t('tourney.entrants'), t('ggs.tourney.players', { n: x.n_players || x.players.length })],
    ...(director ? [[t('tourney.director'), director] as [string, string]] : []),
  ];
  const left = x.next_at != null ? mmss(x.next_at - now) : '';
  const thisRound = sched.filter((r) => r.round === cur && !r.bye);
  const doneThis = thisRound.filter((r) => isFinite(parseFloat(r.raw))).length;
  const myNext = sched.find((r) => r.round >= cur && (r.a === me || r.b === me) && !isFinite(parseFloat(r.raw)));
  const myRank = ranks.findIndex((r) => r.name === me);
  const entrants = (() => {
    const names = (info.get('players') ?? '').split(/\s+/).slice(1).map((p) => p.replace(/[()]/g, '')).filter((p) => p && p !== 'bye');
    return names.length ? names : x.players;
  })();
  const joining = info.get('joining');
  const myGames = sched.filter((r) => r.a === me || r.b === me);
  const shown = who
    ? sched.filter((r) => r.a.toLowerCase().includes(who.toLowerCase()) || r.b.toLowerCase().includes(who.toLowerCase()))
    : sched.filter((r) => r.round === round);
  const openOf = (r: SchedRow) => snap.td.games.find((g) => g.tid === x.id && g.diff == null && (g.opp === r.a || g.opp === r.b));

  const tabs = (
    <Segmented value={view} onChange={setView}
               options={[...(narrow ? [{ value: 'rank' as const, label: t('ggs.tourney.rankings') }] : []),
                         { value: 'mine' as const, label: t('ggs.tourney.games_of', { name: me }) }, { value: 'sched' as const, label: t('ggs.tourney.schedule') }]} />
  );

  return (
    <div ref={boxRef} style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      {errText && <div style={{ paddingTop: 'var(--sp-2)' }}><Band tone="bad">{errText}</Band></div>}
      {x.joined && !finished(x) && snap.standby.enabled && (
        <div style={{ paddingTop: 'var(--sp-2)' }}>
          <Band>{t('ggs.tourney.standby_warn')}
            <span style={{ marginLeft: 'auto' }}><Button size="row" onClick={() => askSheet.open({ repeat: true })}>{t('tourney.change_auto')}</Button></span>
          </Band>
        </div>
      )}
      <div style={{ flex: 'none', display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)', padding: 'var(--sp-3) var(--sp-4)', borderBottom: '1px solid var(--border)' }}>
        <div style={{ display: 'flex', alignItems: 'center', flexWrap: 'wrap', gap: 'var(--sp-1) var(--sp-4)', fontSize: 'var(--fs-5)', whiteSpace: 'nowrap' }}>
          {rules.map(([k, v]) => (
            <span key={k}><span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>{k}</span> {v}</span>
          ))}
        </div>
        {started && !finished(x) ? (
          <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', whiteSpace: 'nowrap' }}>
            <span style={{ fontSize: 'var(--fs-5)' }}>{t('tourney.round')} <b style={{ fontSize: 'var(--fs-3)', fontWeight: 700 }}>{cur}</b> <span style={{ color: 'var(--sub)' }}>/ {all}</span></span>
            <span style={{ flex: 1, minWidth: 0 }}><Progress value={all ? (cur - 1 + (thisRound.length ? doneThis / thisRound.length : 0)) / all : 0} /></span>
            {x.state === 'break' && left
              ? <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('ggs.tourney.next_round_in', { left })}</span>
              : thisRound.length > 0 && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('tourney.round_done')} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{doneThis} / {thisRound.length}</b></span>}
            {myNext && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>· {t('tourney.next_of', { name: me })} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{t('ggs.tourney.round_n', { n: myNext.round })}</b></span>}
          </div>
        ) : (
          <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', fontSize: 'var(--fs-5)', whiteSpace: 'nowrap' }}>
            <b style={{ fontWeight: 600 }}>
              {beforeStart(x) ? (left ? t('ggs.tourney.starts_in', { left }) : t('tourney.open'))
                : x.state === 'cancelled' ? t('ggs.tourney.state.cancelled')
                : x.winner ? t('ggs.tourney.winner', { name: x.winner }) : t('ggs.tourney.state.over')}
            </b>
            <span style={{ color: 'var(--sub)' }}>
              {beforeStart(x) ? t('ggs.tourney.players', { n: entrants.length || x.n_players })
                : [all ? t('tourney.all_rounds', { n: all }) : '', myRank >= 0 ? t('tourney.place_of', { name: me, n: myRank + 1, of: ranks.length }) : ''].filter(Boolean).join(' · ')}
            </span>
          </div>
        )}
      </div>

      {!started ? (
        <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
          <div style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-bar)', padding: '0 var(--sp-3)' }}>
            <b style={{ fontWeight: 600 }}>{t('tourney.entrants')}</b>
            <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('ggs.tourney.players', { n: entrants.length })}</span>
            {joining && <span style={{ marginLeft: 'auto' }}><Tag tone={joining === 'open' ? 'ok' : 'sub'}>{joining === 'open' ? t('ggs.tourney.joining_open') : t('ggs.tourney.joining_closed')}</Tag></span>}
          </div>
          <HeadRow cols={[['#', 32, 'right'], [t('ggs.tourney.col.player'), undefined], [t('ggs.pool.random'), 96, 'right']]} />
          <Rows>
            {entrants.map((n, i) => {
              const r = onlineRows(snap).find((u) => u.name === n)?.rating_r;
              return (
                <Line key={n} height="row" picked={n === me}>
                  <Cell w={32} align="right" sub>{i + 1}</Cell>
                  <Cell sel bold={n === me}>{n}</Cell>
                  <Cell w={96} align="right">{r != null ? r.toFixed(1) : '—'}</Cell>
                </Line>
              );
            })}
          </Rows>
        </div>
      ) : (
        <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
          {(!narrow || view === 'rank') && (
          <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', overflow: 'hidden', borderRight: narrow ? 0 : '1px solid var(--border)' }}>
            <div style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-bar)', padding: '0 var(--sp-3)', whiteSpace: 'nowrap' }}>
              {narrow ? tabs : <b style={{ fontWeight: 600 }}>{t('ggs.tourney.rankings')}</b>}
              {myRank >= 0 && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('tourney.place_of', { name: me, n: myRank + 1, of: ranks.length })}</span>}
              {myRank >= 0 && (
                <span style={{ marginLeft: 'auto' }}>
                  <Button size="row" variant="ghost" onClick={() => rankBox.current?.querySelector('[data-me]')?.scrollIntoView({ block: 'center' })}>
                    {t('tourney.to_name', { name: me })}
                  </Button>
                </span>
              )}
            </div>
            <HeadRow cols={[[t('ggs.tourney.col.rank'), 32, 'right'], [t('ggs.tourney.col.player'), undefined],
                            [t('ggs.tourney.col.points'), 44, 'right'], [t('ggs.tourney.col.wdl'), 72, 'right'],
                            [t('ggs.tourney.col.disc'), 56, 'right']]} />
            {!ranks.length ? <Blank title={t('ggs.loading')} /> : (
              <div ref={rankBox} className="k-scroll" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
                {ranks.map((r, i) => (
                  <div key={r.name} data-me={r.name === me ? '' : undefined} style={{ display: 'contents' }}>
                    <Line height="row" picked={r.name === me}>
                      <Cell w={32} align="right"><b style={{ color: i === 0 ? 'var(--gold)' : 'var(--sub)' }}>{i + 1}</b></Cell>
                      <Cell sel bold={r.name === me}>{r.name}</Cell>
                      <Cell w={44} align="right"><b>{r.pts}</b></Cell>
                      <Cell w={72} align="right"><span style={{ color: 'var(--ok)' }}>{r.w}</span>-{r.d}-<span style={{ color: 'var(--bad)' }}>{r.l}</span></Cell>
                      <Cell w={56} align="right">
                        <span style={{ color: parseFloat(r.disc) > 0 ? 'var(--ok)' : parseFloat(r.disc) < 0 ? 'var(--bad)' : 'var(--sub)' }}>{num(r.disc)}</span>
                      </Cell>
                    </Line>
                  </div>
                ))}
              </div>
            )}
          </div>
          )}

          {(!narrow || view !== 'rank') && (
          <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>
            <div style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-bar)', padding: '0 var(--sp-3)', whiteSpace: 'nowrap' }}>
              {tabs}
              <span style={{ marginLeft: 'auto', flex: '0 1 160px', minWidth: 64, display: 'flex', justifyContent: 'flex-end' }}>
                {view === 'sched'
                  ? <TextField value={who} onChange={setWho} placeholder={t('tourney.filter')} />
                  : <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('tourney.mine_done', { done: myGames.filter((r) => isFinite(parseFloat(r.raw))).length, all: myGames.length })}</span>}
              </span>
            </div>

            {view === 'sched' && !who && (
              <div style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-field)', padding: '0 var(--sp-3)',
                            borderTop: '1px solid var(--border-weak)', background: 'var(--panel)', whiteSpace: 'nowrap', fontSize: 'var(--fs-5)' }}>
                <Button square onClick={() => go(round - 1)} title={t('tourney.prev_round')}>‹</Button>
                <span style={{ color: 'var(--sub)' }}>{t('tourney.round')}</span>
                <TextField value={roundText} onChange={setRoundText} numeric align="right" width={52}
                           onEnter={() => go(Number(roundText) || round)} />
                <span style={{ color: 'var(--sub)' }}>/ {all}</span>
                <Button square onClick={() => go(round + 1)} title={t('tourney.next_round')}>›</Button>
                <span style={{ marginLeft: 'auto' }}>
                  <Button size="row" variant="ghost" disabled={round === cur} onClick={() => go(cur)}>{t('tourney.this_round')}</Button>
                </span>
              </div>
            )}

            {view === 'sched' ? (
              <>
                <HeadRow cols={[...(who ? [['R', 40, 'right'] as [string, number, 'right']] : []),
                                [t('lobby.player'), undefined, 'right'], ['', 16], [t('lobby.player'), undefined], [t('tourney.result'), 72, 'right']]} />
                {!shown.length ? <Blank title={sched.length ? t('tourney.no_games') : t('ggs.loading')} /> : (
                  <Rows>
                    {shown.map((r) => {
                      const mine = r.a === me || r.b === me;
                      return (
                        <Line key={r.game} height="row" picked={mine}>
                          {who && <Cell w={40} align="right" sub>{r.round}</Cell>}
                          <Cell align="right" sel>{r.a}</Cell>
                          <span style={{ width: 16, flex: 'none', textAlign: 'center', color: 'var(--sub)' }}>{r.bye ? '' : '−'}</span>
                          <Cell sel>{r.b}</Cell>
                          {/* Only a finished pairing is a result; the rest is a state, written quietly. */}
                          <Cell w={72} align="right" sub={!isFinite(parseFloat(r.raw))}>
                            <b style={{ fontWeight: isFinite(parseFloat(r.raw)) ? 600 : 400 }}>{resultOf(r, synchro)}</b>
                          </Cell>
                        </Line>
                      );
                    })}
                  </Rows>
                )}
              </>
            ) : (
              <>
                <HeadRow cols={[['R', 40, 'right'], [t('ggs.field.opponent'), undefined], [t('tourney.result'), 72, 'right'], ['', 80]]} />
                <div ref={mineBox} className="k-scroll" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
                  {myGames.map((r) => {
                    const first = r.a === me;
                    const v = parseFloat(r.raw);
                    const done = isFinite(v);
                    const mine = done ? (first ? v : -v) * (synchro ? 2 : 1) : 0;
                    const live = r.round === cur && !done;
                    const g = live ? openOf(r) : undefined;
                    return (
                      <div key={r.game} data-cur={r.round === cur ? '' : undefined} style={{ display: 'contents' }}>
                        <Line height="row" picked={live}>
                          <Cell w={40} align="right" sub>{r.round}</Cell>
                          <Cell sel sub={r.round > cur}>{first ? r.b : r.a}</Cell>
                          <Cell w={72} align="right">
                            <b style={{ fontWeight: 600, color: !done ? 'var(--sub)' : mine > 0 ? 'var(--ok)' : mine < 0 ? 'var(--bad)' : 'var(--sub)' }}>
                              {done ? num(mine) : r.round === cur ? t('ggs.state.playing') : ''}
                            </b>
                          </Cell>
                          <span style={{ width: 80, flex: 'none', display: 'flex', justifyContent: 'flex-end' }}>
                            {g && <Button size="row" variant="primary" onClick={() => { focusGame(g.id); onNav('ggs-play'); }}>{t('ggs.tourney.open_game')}</Button>}
                          </span>
                        </Line>
                      </div>
                    );
                  })}
                </div>
              </>
            )}
          </div>
          )}
        </div>
      )}
    </div>
  );
}
