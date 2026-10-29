import { useEffect, useState } from 'react';
import { ggsApi } from '../api';
import type { GgsSnapshot, UserRow } from '../types';
import { discDiff, fingerGroups, fingerKey, fingerValue, gtypeLabel, onlineRows, parseCond, presence } from '../ggs';
import { t } from '../i18n';
import { Caption, Drawer, Section, Toolbar } from '../components/layout';
import { Button, Segmented, Select, TextField } from '../components/primitives';
import { FormulaView, Tag, type NavId } from '../components/ggs';
import { RateChart, ResultHead, ResultRow } from '../components/data';
import { askSheet } from '../AskSheet';
import { openChat } from './Chat';
import { act, Blank, Cell, HeadRow, Line, ListHead, ListNote, Page, RankRates, rateLabel, Rows, Sections, Table } from '../ggsui';
import { secsOfText, whenFull, whenShort } from '../when';

const fmt = (r: number | null, d: number | null) =>
  r == null ? '—' : `${r.toFixed(1)}${d != null ? ` ±${Math.round(d)}` : ''}`;

/** Whether a player takes requests (`open` from `who`: + yes, x ghost, - no). */
const openOf = (open: string | null): { label: string; color: string } =>
  open === '+' ? { label: t('ggs.users.open_yes'), color: 'var(--accent)' }
    : open === 'x' ? { label: t('ggs.users.open_ghost'), color: 'var(--bad)' }
    : { label: open ? t('ggs.users.open_no') : '—', color: 'var(--sub)' };
let wantedPlayer = '';
/** Opens a player's details the next time the players screen opens (from the observers list). */
export const focusPlayer = (name: string) => { wantedPlayer = name; };

/** Online players or the ranking on the left; the chosen one's summary, history and profile in the dock. */
export function Players({ snap, onNav, onKifu }: {
  snap: GgsSnapshot; onNav: (id: NavId) => void;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  const [mode, setMode] = useState<'who' | 'top'>('who');
  const [pool, setPool] = useState<'8r' | '8'>('8r');
  const [query, setQuery] = useState('');
  const [page, setPage] = useState(0);
  const [perPage, setPerPage] = useState(25);
  // The details open only while someone is picked.
  const [sel, setSel] = useState<string | null>(() => { const w = wantedPlayer || null; wantedPlayer = ''; return w; });
  const [tab, setTab] = useState('summary');
  useEffect(() => { void ggsApi.listMatches().catch(() => {}); }, []);
  useEffect(() => {
    void ggsApi.autoview().then((v) => { if (v === 'players:top') setMode('top'); }).catch(() => {});
  }, [snap.login]);

  const all = mode === 'who'
    ? [...onlineRows(snap)].sort((a, b) => (b.rating_r ?? -1) - (a.rating_r ?? -1))
    : snap.ranking;
  const rows = all.filter((u) => !query || u.name.toLowerCase().includes(query.toLowerCase()));
  const pages = Math.max(1, Math.ceil(rows.length / perPage));
  const cur = Math.min(page, pages - 1);
  const slice = rows.slice(cur * perPage, (cur + 1) * perPage);

  return (
    <>
      <Toolbar end={<>
                 <Button disabled={cur === 0} onClick={() => setPage(cur - 1)}>{t('ggs.prev')}</Button>
                 <span style={{ fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>
                   {rows.length ? `${cur + 1} / ${pages}` : '0 / 0'}
                 </span>
                 <Button disabled={cur >= pages - 1} onClick={() => setPage(cur + 1)}>{t('ggs.next')}</Button>
                 <Select size="ctrl" value={String(perPage)} title={t('ggs.users.per_page')}
                         options={[['25', t('players.per', { n: 25 })], ['50', t('players.per', { n: 50 })], ['100', t('players.per', { n: 100 })]]}
                         onChange={(v) => { setPerPage(+v); setPage(0); }} />
               </>}>
        <TextField value={query} onChange={(v) => { setQuery(v); setPage(0); }} placeholder={t('players.search')} width={180} />
        <Segmented value={mode} onChange={(m) => {
          setMode(m); setPage(0);
          if (m === 'who') act(ggsApi.who()); else act(ggsApi.top(pool, 100));
        }} options={[{ value: 'who', label: t('ggs.users.online') }, { value: 'top', label: t('ggs.users.top') }]} />
        {mode === 'top' && (
          <Segmented value={pool} onChange={(p) => { setPool(p); act(ggsApi.top(p, 100)); }}
                     options={[{ value: '8r', label: t('ggs.pool.random') }, { value: '8', label: t('ggs.pool.normal') }]} />
        )}
        <Button variant="ghost" onClick={() => {
          if (mode === 'who') act(ggsApi.who()); else act(ggsApi.top(pool, 100));
          for (const p of ['8r', '8']) act(ggsApi.rank(p, snap.login));
        }}>{t('ggs.refresh')}</Button>
      </Toolbar>
      <Page row>
        <Sections>
          <Section title={t('ggs.users.rating_of', { name: snap.login })}>
            {!snap.my_ranks.length && <ListNote>{t('ggs.no_records')}</ListNote>}
            <RankRates ranks={snap.my_ranks} />
          </Section>
          <Section title={mode === 'who' ? t('ggs.users.online') : t('players.top_title', { pool: pool === '8r' ? t('ggs.pool.random') : t('ggs.pool.normal') })}
                   aside={t('players.count', { n: rows.length })} grow>
            <Table>
              {mode === 'who'
                ? <HeadRow cols={[[t('ggs.users.col_name'), undefined], [t('ggs.pool.random'), 112, 'right'],
                                  [t('ggs.pool.normal'), 112, 'right'], [t('ggs.users.col_open'), 72, 'right'],
                                  [t('ggs.users.col_status'), 80, 'right']]} />
                : <HeadRow cols={[[t('players.rank'), 40, 'right'], [t('ggs.users.col_name'), undefined],
                                  [t('ggs.users.col_rating'), 112, 'right'], [t('ggs.users.col_status'), 80, 'right']]} />}
              {!slice.length
                ? <Blank title={query ? t('players.no_hit', { q: query }) : mode === 'who' ? t('ggs.users.none_online') : t('ggs.users.no_ranking')}
                         body={query && mode === 'who' ? t('players.no_hit_help') : undefined} />
                : <Rows>
                    {slice.map((u, i) => (
                      <PlayerLine key={u.name} snap={snap} u={u} mode={mode} rank={cur * perPage + i + 1}
                                  picked={u.name === sel} onPick={() => setSel(u.name)} />
                    ))}
                  </Rows>}
            </Table>
          </Section>
        </Sections>
        {sel && (
          <Drawer label={t('players.detail')} onClose={() => setSel(null)}
                  title={<PlayerTitle snap={snap} name={sel} />}
                  tools={<Segmented value={tab} onChange={setTab}
                                    options={[{ value: 'summary', label: t('players.tab_summary') },
                                              { value: 'history', label: t('ggs.users.tab_history') },
                                              { value: 'profile', label: t('ggs.users.tab_profile') }]} />}>
            <PlayerDetail snap={snap} name={sel} tab={tab} onNav={onNav} onKifu={onKifu} />
          </Drawer>
        )}
      </Page>
    </>
  );
}

/** The drawer's heading: the name, whether they are playing, and whether they take requests. */
function PlayerTitle({ snap, name }: { snap: GgsSnapshot; name: string }) {
  const u = onlineRows(snap).find((x) => x.name === name) ?? snap.ranking.find((x) => x.name === name);
  return (
    <>
      <span className="k-sel" style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>{name}</span>
      {presence(snap, name) === 'playing' && <Tag tone="ok">{t('ggs.state.playing')}</Tag>}
      {u?.open && (
        <span style={{ flex: 'none', fontSize: 'var(--fs-6)', fontWeight: 400, color: openOf(u.open).color }}>
          {openOf(u.open).label}
        </span>
      )}
    </>
  );
}

function PlayerLine({ snap, u, mode, rank, picked, onPick }: {
  snap: GgsSnapshot; u: UserRow; mode: 'who' | 'top'; rank: number; picked: boolean; onPick: () => void;
}) {
  const state = presence(snap, u.name);
  const stateCell = (
    <Cell w={80} align="right">
      <span style={{ fontSize: 'var(--fs-6)', color: state === 'playing' ? 'var(--ok)' : state === 'online' ? 'var(--accent)' : 'var(--sub)' }}>
        {state === 'playing' ? t('ggs.state.playing') : state === 'online' ? t('ggs.users.presence_online') : t('ggs.users.presence_offline')}
      </span>
    </Cell>
  );
  const me = u.name === snap.login;
  if (mode === 'top') {
    return (
      <Line onClick={onPick} picked={picked}>
        <Cell w={40} align="right"><b style={{ color: rank === 1 ? 'var(--gold)' : 'var(--sub)' }}>{rank}</b></Cell>
        <Cell bold={me} sel>{u.name}</Cell>
        <Cell w={112} align="right">{fmt(u.rating, u.dev)}</Cell>
        {stateCell}
      </Line>
    );
  }
  return (
    <Line onClick={onPick} picked={picked}>
      <Cell bold={me} sel>{u.name}</Cell>
      <Cell w={112} align="right">{fmt(u.rating_r, u.dev_r)}</Cell>
      <Cell w={112} align="right" sub>{fmt(u.rating, u.dev)}</Cell>
      <Cell w={72} align="right">
        <span style={{ fontSize: 'var(--fs-6)', color: openOf(u.open).color }}>{openOf(u.open).label}</span>
      </Cell>
      {stateCell}
    </Line>
  );
}

function PlayerDetail({ snap, name, tab, onNav, onKifu }: {
  snap: GgsSnapshot; name: string; tab: string; onNav: (id: NavId) => void;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  useEffect(() => { ggsApi.finger(name).catch(() => {}); }, [name]);
  useEffect(() => { ggsApi.history(name === snap.login ? '' : name).catch(() => {}); }, [name, snap.login]);
  const [pool, setPool] = useState<'8r' | '8'>('8r');
  const [hover, setHover] = useState<number | null>(null);
  const u = onlineRows(snap).find((x) => x.name === name) ?? snap.ranking.find((x) => x.name === name);
  // Not there yet means still asking the server; an empty list means no games.
  const loaded = snap.history[name] !== undefined;
  const hist = snap.history[name] ?? [];
  const isRandom = (g: string) => g.includes('r');

  if (tab === 'history') {
    const rows = hist.filter((h) => (pool === '8r') === isRandom(h.gtype));
    return (
      <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
        <div style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-field)', padding: '0 var(--sp-3)' }}>
          <Segmented value={pool} onChange={setPool}
                     options={[{ value: '8r', label: t('ggs.pool.random') }, { value: '8', label: t('ggs.pool.normal') }]} />
          <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('players.games', { n: rows.length })}</span>
        </div>
        {/* The same table as the results screen, from this player's side. */}
        <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', padding: '0 var(--sp-2) var(--sp-2)' }}>
          <Table>
            <ResultHead note />
            {!rows.length ? <Blank title={loaded ? t('ggs.users.no_history') : t('ggs.loading')} /> : (
              <Rows>
                {rows.map((h) => {
                  const black = h.black === name;
                  const v = discDiff(h.score, black);
                  return (
                    <ResultRow key={h.id} opponent={black ? h.white : h.black}
                               win={(v ?? 0) > 0} draw={v === 0} adjourned={v == null} discs={v ?? 0}
                               when={whenShort(secsOfText(h.at)) || h.at} note={gtypeLabel(h.gtype)}
                               onClick={() => onKifu(t('ggs.vs', { a: h.black, b: h.white }), '', h.id)} />
                  );
                })}
              </Rows>
            )}
          </Table>
        </div>
      </div>
    );
  }

  if (tab === 'profile') {
    const fields = snap.fingers[name]?.fields ?? [];
    // The rules are this player's: "me" is them by name, "them" is whoever sends the request.
    const who = { me: t('ggs.formula.who.named', { name }), them: t('ggs.formula.who.them') };
    return (
      <div className="k-scroll" style={{ flex: 1, minHeight: 0, paddingBottom: 'var(--sp-4)' }}>
        {/* Until the profile arrives there is nothing to show, not even the request rules' defaults. */}
        {!fields.length ? <Blank title={t('ggs.loading')} /> : fingerGroups(fields).map((g) => (
          <div key={g.title} style={{ display: 'flex', flexDirection: 'column' }}>
            <ListHead>{g.title}</ListHead>
            {g.rows.map((r) => {
              const key = fingerKey(r.key);
              const cond = ['accept', 'decline', 'request'].includes(key) ? parseCond(r.value) : null;
              return (
                <div key={r.key} style={{ display: 'grid', gridTemplateColumns: 'var(--w-label-sm) minmax(0, 1fr)', columnGap: 'var(--sp-2)',
                                          alignItems: 'baseline', padding: 'var(--sp-1) var(--sp-3)',
                                          borderBottom: '1px solid var(--border-weak)', fontSize: 'var(--fs-6)', lineHeight: 1.5 }}>
                  <span style={{ color: 'var(--sub)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{r.label}</span>
                  <span className="k-sel" style={{ minWidth: 0, overflowWrap: 'anywhere' }}>
                    {cond ? <FormulaView node={cond} top who={who} />
                      : ['accept', 'decline', 'request'].includes(key) ? t('ggs.formula.unset')
                      : fingerValue(r.key, r.value)}
                  </span>
                </div>
              );
            })}
          </div>
        ))}
      </div>
    );
  }

  // Summary: who they are, two ratings, the actions and their rating over their recent games.
  const pts = hist
    .filter((h) => (pool === '8r') === isRandom(h.gtype))
    .map((h) => {
      const black = h.black === name;
      return { at: h.at, r: Number(black ? h.black_rating : h.white_rating), opp: black ? h.white : h.black,
               discs: discDiff(h.score, black) };
    })
    .filter((p) => Number.isFinite(p.r) && p.r > 0)
    .reverse();
  // GGS lists the rating a game started with. The chart, like the results screen, shows the one it
  // ended with: the next game's starting rating, and the current rating for the last game.
  const now = pool === '8r' ? u?.rating_r : u?.rating;
  const after = pts
    .map((p, i) => ({ ...p, r: i + 1 < pts.length ? pts[i + 1].r : now ?? NaN }))
    .filter((p) => Number.isFinite(p.r));
  return (
    <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', padding: 'var(--sp-4) var(--sp-4) var(--sp-5)' }}>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 'var(--sp-3)' }}>
          {([[t('ggs.pool.random'), u?.rating_r ?? null, u?.dev_r ?? null], [t('ggs.pool.normal'), u?.rating ?? null, u?.dev ?? null]] as const)
            .map(([label, r, d]) => (
              <div key={label} style={{ display: 'flex', flexDirection: 'column', lineHeight: 1.3, whiteSpace: 'nowrap' }}>
                <Caption>{label}</Caption>
                <span><b style={{ fontSize: 'var(--fs-3)', fontWeight: 700 }}>{r != null ? r.toFixed(1) : '—'}</b>{' '}
                  <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{d != null ? `±${Math.round(d)}` : ''}</span></span>
              </div>
            ))}
        </div>
        {name !== snap.login && (
          <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
            <Button variant="primary" onClick={() => askSheet.open({ opponent: name })}>{t('ask.title')}</Button>
            <Button onClick={() => { openChat(name); onNav('ggs-chat'); }}>{t('ggs.nav.chat')}</Button>
          </div>
        )}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', borderTop: '1px solid var(--border-weak)', paddingTop: 'var(--sp-3)' }}>
          <div style={{ display: 'flex', alignItems: 'center', height: 'var(--h-ctrl)', whiteSpace: 'nowrap' }}>
            <Caption>{t('players.rate_trend')}</Caption>
            <span style={{ marginLeft: 'auto' }}>
              <Segmented value={pool} onChange={setPool}
                         options={[{ value: '8r', label: t('ggs.pool.random') }, { value: '8', label: t('ggs.pool.normal') }]} />
            </span>
          </div>
          {after.length >= 2
            ? <RateChart points={after.map((p) => p.r)} width={448} height={160} dates={after.map((p) => whenShort(secsOfText(p.at)))}
                         labels={after.map((p) => rateLabel(whenFull(secsOfText(p.at)), p.opp, p.discs, p.r))} hover={hover} onHover={setHover} />
            : <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{loaded ? t('ggs.no_records') : t('ggs.loading')}</span>}
        </div>
      </div>
    </div>
  );
}
