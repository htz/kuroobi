import { useCallback, useEffect, useState, type CSSProperties } from 'react';
import { ggsApi } from '../api';
import type { GgsSnapshot, TourneyView } from '../types';
import { gtypeLabel, onlineRows } from '../ggs';
import { t } from '../i18n';
import { captionStyle, Combobox, ListRow, Toolbar, TwoLines } from '../components/layout';
import { Button, Dot, Progress, Segmented, TextField } from '../components/primitives';
import { Tag, type NavId } from '../components/ggs';
import { askSheet } from '../AskSheet';
import { focusGame } from './Lobby';
import { act, Band, Blank, Cell, HeadRow, Line, ListHead, ListNote, ListPane, Page, Rows, TitleBit, useNow } from '../ggsui';

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
          <TourneyDetail key={x.id} snap={snap} x={x} now={now} errText={errText} />
        )}
      </Page>
    </>
  );
}

function TourneyDetail({ snap, x, now, errText }: {
  snap: GgsSnapshot; x: TourneyView; now: number; errText: string;
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
  const [view, setView] = useState<'cross' | 'games'>('cross');
  // The schedule follows the current round until another one is picked.
  const [pinned, setPinned] = useState<number | null>(null);
  const round = pinned ?? cur;
  const [typed, setTyped] = useState<string | null>(null);
  const roundText = typed ?? String(round);
  // Two symmetric picks: one player's games, or the games between two.
  const [pick, setPick] = useState<[string, string]>(['', '']);
  const picked = pick.filter(Boolean);
  const focus = picked[0] ?? '';

  const go = (n: number) => {
    const v = Math.max(1, Math.min(all || 1, n));
    setPinned(v === cur ? null : v); setTyped(null);
  };
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT')) return;
      if (view !== 'games' || picked.length) return;
      if (e.key === 'ArrowLeft') { e.preventDefault(); go(round - 1); }
      if (e.key === 'ArrowRight') { e.preventDefault(); go(round + 1); }
    };
    window.addEventListener('keydown', on);
    return () => window.removeEventListener('keydown', on);
  });

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
  const done = (r: SchedRow) => isFinite(parseFloat(r.raw));
  const thisRound = sched.filter((r) => r.round === cur && !r.bye);
  const doneThis = thisRound.filter(done).length;
  const myNext = sched.find((r) => r.round >= cur && (r.a === me || r.b === me) && !done(r));
  const myRank = ranks.findIndex((r) => r.name === me);
  const entrants = (() => {
    const names = (info.get('players') ?? '').split(/\s+/).slice(1).map((p) => p.replace(/[()]/g, '')).filter((p) => p && p !== 'bye');
    return names.length ? names : x.players;
  })();
  const joining = info.get('joining');
  const shown = picked.length
    ? sched.filter((r) => !r.bye && picked.every((n) => r.a === n || r.b === n))
    : sched.filter((r) => r.round === round);
  const cross = crossOf(sched, cur);
  const players = [...ranks.map((r) => r.name), ...[...cross.keys()].filter((n) => !ranks.some((r) => r.name === n)).sort()];
  const pickOptions = (other: string): [string, string][] =>
    [['', t('tourney.all')], ...players.filter((n) => n !== other).map((n) => [n, n] as [string, string])];

  const tabs = (
    <Segmented value={view} onChange={setView}
               options={[{ value: 'cross' as const, label: t('ggs.tourney.crosstable') },
                         { value: 'games' as const, label: t('ggs.tourney.all_games') }]} />
  );
  const sub: CSSProperties = { fontSize: 'var(--fs-6)', color: 'var(--sub)' };
  const gamesBar: CSSProperties = {
    flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-field)', padding: '0 var(--sp-3)',
    borderTop: '1px solid var(--border-weak)', background: 'var(--panel)', whiteSpace: 'nowrap', fontSize: 'var(--fs-5)',
  };

  return (
    <div style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
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
              ? <span style={sub}>{t('ggs.tourney.next_round_in', { left })}</span>
              : thisRound.length > 0 && <span style={sub}>{t('tourney.round_done')} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{doneThis} / {thisRound.length}</b></span>}
            {myNext && <span style={sub}>· {t('tourney.next_of', { name: me })} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{t('ggs.tourney.round_n', { n: myNext.round })}</b></span>}
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
            <span style={sub}>{t('ggs.tourney.players', { n: entrants.length })}</span>
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
        <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
          <div style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-bar)', padding: '0 var(--sp-3)', whiteSpace: 'nowrap' }}>
            {tabs}
            {view === 'cross' && myRank >= 0 && (
              <span style={{ ...sub, marginLeft: 'auto' }}>{t('tourney.place_of', { name: me, n: myRank + 1, of: ranks.length })}</span>
            )}
            {view === 'games' && (
              <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
                <span style={sub}>{t('tourney.game_players')}</span>
                <Combobox value={pick[0]} onChange={(v) => setPick([v, pick[1]])} options={pickOptions(pick[1])} width={124} title={t('tourney.game_players')} />
                <span style={{ color: 'var(--sub)' }}>−</span>
                <Combobox value={pick[1]} onChange={(v) => setPick([pick[0], v])} options={pickOptions(pick[0])} width={124} title={t('tourney.game_players')} />
                {picked.length > 0 && <Button size="row" variant="ghost" onClick={() => setPick(['', ''])}>{t('tourney.clear')}</Button>}
              </span>
            )}
          </div>

          {view === 'cross' && (
            !ranks.length && !players.length ? <Blank title={t('ggs.loading')} /> : (
              <CrossTable players={players} ranks={ranks} cross={cross} me={me}
                          onPick={(a, b) => { setPick([a, b]); setView('games'); }} />
            )
          )}

          {view === 'games' && (
            <>
              {picked.length ? (
                <div style={gamesBar}>
                  <b style={{ fontWeight: 600 }}>{t('tourney.filter_who', { who: picked.join(' − ') })}</b>
                  <span style={{ color: 'var(--sub)' }}>{t('tourney.filter_count', { n: shown.length, done: shown.filter(done).length })}</span>
                </div>
              ) : (
                <div style={gamesBar}>
                  <Button square onClick={() => go(round - 1)} title={t('tourney.prev_round')}>‹</Button>
                  <span style={{ color: 'var(--sub)' }}>{t('tourney.round')}</span>
                  <TextField value={roundText} onChange={setTyped} numeric align="right" width={52}
                             onEnter={() => go(Number(roundText) || round)} />
                  <span style={{ color: 'var(--sub)' }}>/ {all}</span>
                  <Button square onClick={() => go(round + 1)} title={t('tourney.next_round')}>›</Button>
                  <span style={{ marginLeft: 'auto' }}>
                    <Button size="row" variant="ghost" disabled={round === cur} onClick={() => go(cur)}>{t('tourney.this_round')}</Button>
                  </span>
                </div>
              )}
              <HeadRow cols={[...(picked.length ? [['R', 40, 'right'] as [string, number, 'right']] : []),
                              [t('lobby.player'), undefined, 'right'], ['', 16], [t('lobby.player'), undefined], [t('tourney.result'), 72, 'right']]} />
              {!shown.length ? <Blank title={sched.length ? t('tourney.no_games') : t('ggs.loading')} /> : (
                <Rows>
                  {shown.map((r) => {
                    const mine = r.a === me || r.b === me;
                    // Filtered, the picked player sits on the left and the result is theirs.
                    const flip = focus !== '' && r.b === focus;
                    const v = parseFloat(r.raw) * (synchro ? 2 : 1) * (flip ? -1 : 1);
                    const tone = focus && done(r) ? (v > 0 ? 'var(--ok)' : v < 0 ? 'var(--bad)' : undefined) : undefined;
                    return (
                      <Line key={r.game} height="row" picked={mine}>
                        {picked.length > 0 && <Cell w={40} align="right" sub>{r.round}</Cell>}
                        <Cell align="right" sel sub={r.round > cur}>{flip ? r.b : r.a}</Cell>
                        <span style={{ width: 16, flex: 'none', textAlign: 'center', color: 'var(--sub)' }}>{r.bye ? '' : '−'}</span>
                        <Cell sel sub={r.round > cur}>{flip ? r.a : r.b}</Cell>
                        {/* Only a finished pairing is a result; the rest is a state, written quietly. */}
                        <Cell w={72} align="right" sub={!done(r)}>
                          <b style={{ fontWeight: done(r) ? 600 : 400, color: tone }}>
                            {r.round > cur && !done(r) ? '' : done(r) ? num(v) : resultOf(r, synchro)}
                          </b>
                        </Cell>
                      </Line>
                    );
                  })}
                </Rows>
              )}
            </>
          )}

        </div>
      )}
    </div>
  );
}

type CrossCell = { w: number; d: number; l: number; live: boolean };

/** Each pair's games from the row player's side; a pair meets many times in a GGS round robin. */
function crossOf(sched: SchedRow[], cur: number): Map<string, Map<string, CrossCell>> {
  const m = new Map<string, Map<string, CrossCell>>();
  const at = (a: string, b: string): CrossCell => {
    let row = m.get(a);
    if (!row) { row = new Map(); m.set(a, row); }
    let c = row.get(b);
    if (!c) { c = { w: 0, d: 0, l: 0, live: false }; row.set(b, c); }
    return c;
  };
  for (const r of sched) {
    if (r.bye || !r.b) continue;
    const x = at(r.a, r.b), y = at(r.b, r.a);
    const v = parseFloat(r.raw);
    if (isFinite(v)) {
      if (v > 0) { x.w++; y.l++; } else if (v < 0) { x.l++; y.w++; } else { x.d++; y.d++; }
    } else if (r.round === cur && r.raw === '?') {
      x.live = true; y.live = true;
    }
  }
  return m;
}

const CELL = 50;
// The five columns that stay put while the opponents scroll sideways.
const fixedPart = (bg: string, extra?: CSSProperties): CSSProperties => ({
  position: 'sticky', left: 0, zIndex: 1, alignSelf: 'stretch', display: 'flex', alignItems: 'center',
  gap: 'var(--sp-3)', padding: '0 var(--sp-3)', background: bg, borderRight: '1px solid var(--border)', ...extra,
});
const fmtPts = (p: number) => (Number.isInteger(p) ? String(p) : p.toFixed(1));

function CrossTable({ players, ranks, cross, me, onPick }: {
  players: string[]; ranks: RankRow[]; cross: Map<string, Map<string, CrossCell>>; me: string;
  onPick: (a: string, b: string) => void;
}) {
  const byName = new Map(ranks.map((r) => [r.name, r]));
  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0, overflow: 'auto', borderTop: '1px solid var(--border-weak)' }}>
      <div style={{ width: 'max-content', minWidth: '100%', display: 'flex', flexDirection: 'column' }}>
        <div style={{ position: 'sticky', top: 0, zIndex: 2, flex: 'none', display: 'flex', alignItems: 'center', height: 'var(--h-head)',
                      paddingRight: 'var(--sp-3)', borderBottom: '1px solid var(--border)', background: 'var(--bg)', ...captionStyle }}>
          <span style={fixedPart('var(--bg)')}>
            <span style={{ width: 32, textAlign: 'right' }}>{t('ggs.tourney.col.rank')}</span>
            <span style={{ width: 96 }}>{t('ggs.tourney.col.player')}</span>
            <span style={{ width: 40, textAlign: 'right' }}>{t('ggs.tourney.col.points')}</span>
            <span style={{ width: 60, textAlign: 'right' }}>{t('ggs.tourney.col.wdl')}</span>
            <span style={{ width: 48, textAlign: 'right' }}>{t('ggs.tourney.col.disc')}</span>
          </span>
          <span style={{ display: 'flex', gap: 2, paddingLeft: 'var(--sp-3)' }}>
            {players.map((n) => (
              <span key={n} title={n} style={{ width: CELL, textAlign: 'center', overflow: 'hidden', textOverflow: 'ellipsis', letterSpacing: 0,
                                               color: n === me ? 'var(--accent)' : undefined }}>{n}</span>
            ))}
          </span>
        </div>
        {players.map((a, i) => {
          const r = byName.get(a);
          const mine = a === me;
          const disc = r ? parseFloat(r.disc) : NaN;
          return (
            <div key={a} style={{ flex: 'none', display: 'flex', alignItems: 'center', height: 'var(--h-row2)', paddingRight: 'var(--sp-3)',
                                  borderBottom: '1px solid var(--border-weak)', fontSize: 'var(--fs-5)', whiteSpace: 'nowrap' }}>
              <span style={fixedPart(mine ? 'var(--card)' : 'var(--bg)', mine ? { boxShadow: 'inset 2px 0 0 var(--accent)' } : undefined)}>
                <span style={{ width: 32, textAlign: 'right', fontWeight: 700, color: i === 0 ? 'var(--gold)' : 'var(--sub)' }}>{r ? i + 1 : ''}</span>
                <span className="k-sel" style={{ width: 96, overflow: 'hidden', textOverflow: 'ellipsis', fontWeight: mine ? 600 : undefined }}>{a}</span>
                <span style={{ width: 40, textAlign: 'right', fontWeight: 700 }}>{r?.pts ?? ''}</span>
                <span style={{ width: 60, textAlign: 'right' }}>
                  {r && <><span style={{ color: 'var(--ok)' }}>{r.w}</span>-{r.d}-<span style={{ color: 'var(--bad)' }}>{r.l}</span></>}
                </span>
                <span style={{ width: 48, textAlign: 'right', color: disc > 0 ? 'var(--ok)' : disc < 0 ? 'var(--bad)' : 'var(--sub)' }}>{r ? num(r.disc) : ''}</span>
              </span>
              <span style={{ alignSelf: 'stretch', display: 'flex', gap: 2, padding: '3px 0 3px var(--sp-3)' }}>
                {players.map((b) => {
                  if (a === b) {
                    return <span key={b} style={{ width: CELL, borderRadius: 'var(--r-1)',
                                                  background: 'repeating-linear-gradient(45deg, transparent 0 5px, var(--border-weak) 5px 6px)' }} />;
                  }
                  const c = cross.get(a)?.get(b);
                  const g = c ? c.w + c.d + c.l : 0;
                  const p = c ? c.w + c.d / 2 : 0;
                  // Tinted by how far the pair is from even: up to 32% of ok / bad.
                  const pct = g ? Math.round(Math.min(Math.abs(p / g - 0.5) * 2, 1) * 32) : 0;
                  const bg = pct ? `color-mix(in srgb, var(${p / g > 0.5 ? '--ok' : '--bad'}) ${pct}%, transparent)` : 'transparent';
                  return (
                    <span key={b} className="k-press" title={t('tourney.cell_tip', { a, b })} onClick={() => onPick(a, b)}
                          style={{ width: CELL, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', lineHeight: 1.15,
                                   borderRadius: 'var(--r-1)', background: bg, cursor: 'pointer',
                                   boxShadow: c?.live ? 'inset 0 0 0 1px var(--accent)' : undefined }}>
                      <b style={{ fontWeight: 600, color: g ? undefined : 'var(--sub)' }}>{g ? fmtPts(p) : '–'}</b>
                      {g > 0 && <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{c!.w}-{c!.d}-{c!.l}</span>}
                    </span>
                  );
                })}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
