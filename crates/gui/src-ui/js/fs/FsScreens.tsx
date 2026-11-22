import React, { useEffect, useMemo, useState } from 'react';
import { fsApi } from '../api';
import { t, tErr } from '../i18n';
import type { FsBotRow, FsLeg, FsMatch, FsResult, FsSnapshot, GgsSnapshot } from '../types';
import type { Prefs } from '../prefs';
import type { NavId } from '../components/ggs';
import { BotMatchRow, Tag } from '../components/ggs';
import { Board } from '../components/board';
import { Icon } from '../components/Icons';
import { Button, Dot, Segmented, Select, TextField, Toggle } from '../components/primitives';
import { Combobox, Divider, Drawer, EmptyBoard, Field, KeyValue, Modal, Overlay, Section, Toolbar } from '../components/layout';
import { BotPhoto, EvalTrend, PlayerRow, RateChart, ResultHead, ResultRow } from '../components/data';
import { Band, Blank, ListHead, ListPane, Page, Rows, Sections, Table, TitleBit, useKept, useNow, useWidth, useWindowHeight } from '../ggsui';
import { whenFull, whenShort } from '../when';
import { boardCells, boardIndex, clockText, endBytes, legDiff, liveConn, matchesOf, midBytes, myRating, pliesOf, sizeText, splitOf, tablesOf, threadsOf, trendOf } from '../fs';
import { flipped } from '../prefs';
import { Strength, presetOf } from '../components/strength';
import { LEVELS } from '../state';

type Ask = (a: { title: string; body: string; ok: string; danger?: boolean }) => Promise<boolean>;

export function FsScreen({ nav, fs, ggs, onNav, prefs, onKifu, confirm, say }: {
  nav: NavId; fs: FsSnapshot | null; ggs: GgsSnapshot | null; onNav: (id: NavId) => void; prefs: Prefs;
  onKifu: (title: string, kifu: string, parts?: string[]) => void; confirm: Ask; say: (s: string) => void;
}) {
  const snap = fs ?? { cores: 1, bots: [], adding: null, token_change: null, connected: false, playing: false,
                      tables: { app: [22, 24], min: 16, max: 26, entry: [16, 32, 32], memory: 0 } };
  // A bot that is not connected has its games on the site read again when its screens open.
  const idle = snap.bots.filter((b) => !liveConn(b.conn)).map((b) => b.id).join(',');
  useEffect(() => {
    if (nav === 'fs-mine') return;
    for (const id of idle.split(',').filter(Boolean)) void fsApi.sync(Number(id));
  }, [nav, idle]);
  if (nav === 'fs-play') return <FsPlay fs={snap} onNav={onNav} prefs={prefs} onKifu={onKifu} />;
  if (nav === 'fs-bots') return <FsBots fs={snap} say={say} />;
  if (nav === 'fs-results') return <FsResults fs={snap} onKifu={onKifu} />;
  return <FsMine fs={snap} ggs={ggs} confirm={confirm} say={say} />;
}

/** A bot's name, or its number when the site gave none. */
const nm = (b: FsBotRow): string => b.name || t('fs.bot_n', { n: b.id });
const opp = (m: FsMatch): string => m.opponent_name || t('fs.bot_n', { n: m.opponent });

/** Every bot's picture by its number: this machine's bots, then all the bots a connected one sees. */
const photosOf = (fs: FsSnapshot): Map<number, string> => {
  const m = new Map<number, string>();
  for (const b of fs.bots) {
    for (const o of b.view?.bots ?? []) if (o.photo) m.set(o.id, o.photo);
    if (b.photo) m.set(b.id, b.photo);
  }
  return m;
};

const act = (say: (s: string) => void) => (p: Promise<unknown>) => { p.catch((e) => say(tErr(e))); };

const pausedOf = (b: FsBotRow): boolean => !!b.view?.bots.find((o) => o.id === b.id)?.paused;

/** What a bot is doing, in the bot list's words and tone. */
function stateOf(b: FsBotRow): { word: string; tone: 'ok' | 'accent' | 'gold' | 'sub'; color: string } {
  if (b.conn === 'online') {
    return b.view && b.view.matches.length
      ? { word: t('fs.state.playing'), tone: 'ok', color: 'var(--ok)' }
      : pausedOf(b) ? { word: t('fs.state.paused'), tone: 'gold', color: 'var(--gold)' }
      : { word: t('fs.state.waiting'), tone: 'accent', color: 'var(--accent)' };
  }
  if (b.conn === 'draining') return { word: t('fs.state.draining'), tone: 'gold', color: 'var(--gold)' };
  if (b.conn === 'connecting') return { word: t('fs.state.connecting'), tone: 'gold', color: 'var(--gold)' };
  if (b.conn === 'retrying') return { word: t('fs.state.retrying'), tone: 'gold', color: 'var(--gold)' };
  if (b.conn === 'refused') return { word: t('fs.state.refused'), tone: 'sub', color: 'var(--bad)' };
  return { word: t('fs.state.off'), tone: 'sub', color: 'var(--sub)' };
}

function useResults(ids: number[], rev: string): Map<number, FsResult[]> {
  const [all, setAll] = useState(new Map<number, FsResult[]>());
  const key = ids.join(',');
  useEffect(() => {
    let alive = true;
    void Promise.all(key ? key.split(',').map(async (id) => [Number(id), await fsApi.results(Number(id))] as const) : [])
      .then((rows) => { if (alive) setAll(new Map(rows)); })
      .catch(() => { /* no records yet */ });
    return () => { alive = false; };
  }, [key, rev]);
  return all;
}

// ---------------------------------------------------------------- My bots

function ThreadBar({ fs }: { fs: FsSnapshot }) {
  const colors = ['var(--accent)', 'var(--ok)', 'var(--gold)', 'var(--sub)'];
  const cells: string[] = [];
  fs.bots.forEach((b, i) => {
    const c = colors[i % colors.length];
    const faint = !liveConn(b.conn);
    for (let k = 0; k < threadsOf(b); k++) {
      cells.push(faint ? `color-mix(in srgb, ${c} 35%, var(--track))` : c);
    }
  });
  const used = cells.length;
  while (cells.length < fs.cores) cells.push('var(--track)');
  return (
    <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-6)', color: 'var(--sub)', whiteSpace: 'nowrap' }}>
      <span>{t('fs.threads')}</span>
      <span aria-hidden style={{ display: 'flex', gap: 2, width: 160, height: 6 }}>
        {cells.map((c, i) => <span key={i} style={{ flex: 1, borderRadius: 1, background: c }} />)}
      </span>
      <b style={{ fontWeight: 600, color: 'var(--text)' }}>{used} / {fs.cores}</b>
    </span>
  );
}

/** Every bot's tables (one game's × 2 games × matches at once) against this machine's memory; shown, never refused. */
function TableBar({ fs }: { fs: FsSnapshot }) {
  const colors = ['var(--accent)', 'var(--ok)', 'var(--gold)', 'var(--sub)'];
  const tb = fs.tables;
  const total = tb.memory || 1;
  const bytesOf = (b: FsBotRow) => tablesOf(tb, b) * 2 * matchesOf(b);
  const used = fs.bots.reduce((n, b) => n + bytesOf(b), 0);
  const parts = fs.bots.map((b, i) => {
    const bytes = bytesOf(b);
    const c = colors[i % colors.length];
    return { w: Math.min(160, (160 * bytes) / total), c: liveConn(b.conn) ? c : `color-mix(in srgb, ${c} 35%, var(--track))` };
  });
  return (
    <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-6)', color: 'var(--sub)', whiteSpace: 'nowrap' }}>
      <span>{t('fs.tables')}</span>
      <span aria-hidden style={{ display: 'flex', gap: 2, width: 160, height: 6, overflow: 'hidden' }}>
        {parts.map((p, i) => <span key={i} style={{ flex: 'none', width: p.w, borderRadius: 1, background: p.c }} />)}
        <span style={{ flex: 1, borderRadius: 1, background: 'var(--track)' }} />
      </span>
      <b style={{ fontWeight: 600, color: 'var(--text)' }}>
        {t('fs.tables_use', { used: +(used / 2 ** 30).toFixed(1), total: Math.round(tb.memory / 2 ** 30) })}
      </b>
    </span>
  );
}

/** The choices of one table's size: the app's setting, then each size from the largest. */
function hashOpts(fs: FsSnapshot, bytes: (bits: number) => number, app: number | null): [string, string][] {
  const tb = fs.tables;
  const out: [string, string][] = [['0', app == null ? t('fs.mine.hash_app_plain') : t('fs.mine.hash_app', { size: sizeText(bytes(app)) })]];
  for (let b = tb.max; b >= tb.min; b--) out.push([String(b), sizeText(bytes(b))]);
  return out;
}

const row: React.CSSProperties = {
  display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', minHeight: 'var(--h-row2)', padding: 'var(--sp-2) 0',
  borderBottom: '1px solid var(--border-weak)', fontSize: 'var(--fs-5)',
};
const rowLabel: React.CSSProperties = { width: 160, flex: 'none', color: 'var(--sub)' };
const rowTop: React.CSSProperties = { ...row, alignItems: 'flex-start' };
const rowTopLabel: React.CSSProperties = { ...rowLabel, lineHeight: 'var(--h-field)' };

type Level = [number, number] | null;
const LEVEL0: [number, number] = [LEVELS[6].depth, LEVELS[6].solve];

const levelName = (l: [number, number]): string => {
  const i = presetOf({ depth: l[0], solve: l[1] });
  return i === 'custom' ? t('app.level.custom') : t('app.level.preset', { n: i + 1 });
};

/** Main time (the search set by the clock) or a level, as in Play. */
function StrengthPick({ value, onChange }: { value: Level; onChange: (v: Level) => void }) {
  return (
    <div style={{ width: 320, display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
      <Segmented fill value={value ? 'level' : 'clock'} onChange={(v) => onChange(v === 'clock' ? null : LEVEL0)}
                 options={[{ value: 'level', label: t('dock.strength.by_level') }, { value: 'clock', label: t('dock.strength.by_clock') }]} />
      {value && <Strength value={{ depth: value[0], solve: value[1] }} onChange={(l) => onChange([l.depth, l.solve])} />}
    </div>
  );
}

/** FlipSight's limit on one engine's threads. */
const MAX_THREADS = 4;

const readAt = new Map<number, number>();

/** The site's matches at once of a disconnected bot, read again when its choices are about to open. */
function reread(b: FsBotRow) {
  const now = performance.now();
  if (liveConn(b.conn) || now - (readAt.get(b.id) ?? -Infinity) < 10_000) return;
  readAt.set(b.id, now);
  void fsApi.refresh(b.id);
}

function FsMine({ fs, ggs, confirm, say }: { fs: FsSnapshot; ggs: GgsSnapshot | null; confirm: Ask; say: (s: string) => void }) {
  const run = act(say);
  const [pick, setPick] = useState<number | 'new'>(fs.bots[0]?.id ?? 'new');
  const [token, setToken] = useState('');
  const [details, setDetails] = useState(false);
  const [newCap, setNewCap] = useState('0');
  const [newThreads, setNewThreads] = useState('0');
  const [newLevel, setNewLevel] = useState<Level>(null);
  const [newHash, setNewHash] = useState<[string, string]>(['0', '0']);
  const [tokenFor, setTokenFor] = useState<number | null>(null);
  const [newToken, setNewToken] = useState('');
  const sel = pick === 'new' ? undefined : fs.bots.find((b) => b.id === pick);
  const adding = !sel;
  const ggsOn = !!ggs && ggs.conn !== 'disconnected';
  const ggsPlaying = !!ggs && ggs.matches.some((m) => !!m.my_color && !m.over);

  // A bot just added is selected; the outcome of the form or the dialog is then forgotten.
  const done = fs.adding?.done ?? null;
  const tokenDone = fs.token_change?.done ?? null;
  const [seen, setSeen] = useState<[number | null, number | null]>([null, null]);
  if (done !== seen[0] || tokenDone !== seen[1]) {
    setSeen([done, tokenDone]);
    if (done != null) { setPick(done); setToken(''); }
    if (tokenDone != null) { setTokenFor(null); setNewToken(''); }
  }
  useEffect(() => {
    if (done != null || tokenDone != null) void fsApi.clearPending();
  }, [done, tokenDone]);
  if (pick !== 'new' && !sel && fs.bots.length) setPick(fs.bots[0].id);

  const blocked = ggsOn && (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', padding: 'var(--sp-3)', background: 'var(--card)',
                  boxShadow: 'inset 2px 0 0 var(--gold)', fontSize: 'var(--fs-5)' }}>
      {t('fs.mine.ggs_on', { name: ggs!.login })}
    </div>
  );

  let detail: React.ReactNode;
  if (sel) {
    const st = stateOf(sel);
    const m = matchesOf(sel);
    const site = sel.max_games;
    const capOpts: [string, string][] = [['0', site != null ? t('fs.mine.cap_site', { n: site }) : t('fs.mine.cap_site_plain')]];
    for (let i = 1; i < (site ?? 4); i++) capOpts.push([String(i), String(i)]);
    const threadOpts: [string, string][] = [];
    for (let i = 1; i <= Math.max(MAX_THREADS, sel.threads); i++) threadOpts.push([String(i), t('fs.mine.threads_n', { n: i })]);
    const on = liveConn(sel.conn);
    const refused = sel.conn === 'refused';
    const msg = refused ? [t('fs.mine.refused'), 'var(--bad)']
      : sel.conn === 'retrying' ? [t('fs.mine.retrying'), 'var(--gold)']
      : ggsOn && ggsPlaying && !on ? [t('fs.mine.ggs_playing'), 'var(--sub)'] : ['', 'var(--sub)'];
    detail = (
      <>
        {blocked}
        <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
        <BotPhoto src={sel.photo} name={nm(sel)} size={48} />
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
          <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
            <span className="k-sel" style={{ fontSize: 'var(--fs-1)', fontWeight: 700 }}>{nm(sel)}</span>
            <Tag tone={refused ? 'bad' : st.tone === 'gold' ? 'sub' : st.tone}>{st.word}</Tag>
          </span>
          {sel.view?.rating != null && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('fs.rating')} {sel.view.rating.toFixed(1)}</span>}
        </div>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', marginTop: 'var(--sp-2)', borderTop: '1px solid var(--border-weak)' }}>
          <div style={row}><span style={rowLabel}>{t('fs.mine.cap')}</span>
            <span onPointerEnter={() => reread(sel)} onFocus={() => reread(sel)}>
              <Select value={String(sel.cap ?? 0)} options={capOpts} onChange={(v) => run(fsApi.set(sel.id, { cap: Number(v) }))} />
            </span></div>
          <div style={row}><span style={rowLabel}>{t('fs.mine.accept')}</span>
            <Toggle checked={sel.accept} onChange={(v) => run(fsApi.set(sel.id, { accept: v }))} /></div>
          <div style={row}><span style={rowLabel}>{t('fs.mine.threads')}</span>
            <Select value={String(sel.threads)} options={threadOpts} onChange={(v) => run(fsApi.set(sel.id, { threads: Number(v) }))} />
            <span style={{ color: 'var(--sub)', whiteSpace: 'nowrap', fontVariantNumeric: 'tabular-nums' }}>
              {t('fs.mine.threads_sum', { m, n: threadsOf(sel) })}
            </span></div>
          <div style={row}><span style={rowLabel}>{t('fs.mine.hash_mid')}</span>
            <Select value={String(sel.hash[0] ?? 0)} options={hashOpts(fs, (b) => midBytes(fs.tables, b, !!sel.level), fs.tables.app[0])}
                    onChange={(v) => run(fsApi.set(sel.id, { hashMid: Number(v) }))} /></div>
          <div style={row}><span style={rowLabel}>{t('fs.mine.hash_end')}</span>
            <Select value={String(sel.hash[1] ?? 0)} options={hashOpts(fs, (b) => endBytes(fs.tables, b), fs.tables.app[1])}
                    onChange={(v) => run(fsApi.set(sel.id, { hashEnd: Number(v) }))} />
            <span style={{ color: 'var(--sub)', whiteSpace: 'nowrap', fontVariantNumeric: 'tabular-nums' }}>
              {t('fs.mine.hash_sum', { one: sizeText(tablesOf(fs.tables, sel)), m, all: sizeText(tablesOf(fs.tables, sel) * 2 * m) })}
            </span></div>
          <div style={rowTop}><span style={rowTopLabel}>{t('fs.mine.strength')}</span>
            <StrengthPick value={sel.level} onChange={(v) => run(fsApi.set(sel.id, { level: v ?? [0, 0] }))} /></div>
          <div style={row}><span style={rowLabel}>{t('fs.mine.ponder')}</span>
            <Toggle checked={sel.ponder && !sel.level} disabled={!!sel.level}
                    onChange={(v) => run(fsApi.set(sel.id, { ponder: v }))} /></div>
          <div style={row}><span style={rowLabel}>{t('fs.mine.book')}</span>
            <Toggle checked={sel.book} onChange={(v) => run(fsApi.set(sel.id, { book: v }))} /></div>
        </div>
        <div style={{ display: 'flex', gap: 'var(--sp-2)', marginTop: 'var(--sp-3)' }}>
          {!refused && (sel.conn === 'draining'
            ? <Button onClick={() => run(fsApi.connect(sel.id))}>{t('fs.mine.undo_disconnect')}</Button>
            : on
            ? <Button variant="danger" onClick={() => run(fsApi.disconnect(sel.id))}>{t('fs.mine.disconnect')}</Button>
            : <Button variant="primary" disabled={ggsOn && ggsPlaying} onClick={() => run(fsApi.connect(sel.id))}>
                {ggsOn ? t('fs.mine.logout_connect') : t('fs.mine.connect')}
              </Button>)}
          <Button variant={refused ? 'primary' : 'secondary'} onClick={() => { setNewToken(''); setTokenFor(sel.id); }}>
            {t('fs.mine.change_token')}
          </Button>
          <span style={{ marginLeft: 'auto' }}>
            <Button variant="ghost" onClick={() => void (async () => {
              if (await confirm({ title: t('fs.mine.remove_title', { name: nm(sel) }), body: t('fs.mine.remove_body'),
                                  ok: t('fs.mine.remove'), danger: true })) run(fsApi.remove(sel.id));
            })()}>{t('fs.mine.remove_bot')}</Button>
          </span>
        </div>
        <span style={{ fontSize: 'var(--fs-6)', color: msg[1], minHeight: '1.5em' }}>{msg[0]}</span>
      </>
    );
  } else {
    const busy = !!fs.adding?.busy;
    const capOpts: [string, string][] = [['0', t('fs.mine.cap_site_plain')], ...[1, 2, 3, 4].map((n): [string, string] => [String(n), String(n)])];
    const threadOpts: [string, string][] = [['0', t('fs.mine.threads_auto')]];
    for (let i = 1; i <= MAX_THREADS; i++) threadOpts.push([String(i), t('fs.mine.threads_n', { n: i })]);
    const submit = () => run(fsApi.add(token, Number(newCap) || null, Number(newThreads) || null, newLevel,
                                       Number(newHash[0]) || null, Number(newHash[1]) || null));
    detail = (
      <>
        {blocked}
        <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{t('fs.mine.add_title')}</span>
        <Field label={t('fs.mine.token')}>
          <TextField value={token} onChange={setToken} password invalid={!!fs.adding?.error && !busy}
                     placeholder="fsbm_…" onEnter={submit} />
        </Field>
        <button type="button" className="k-row" onClick={() => setDetails(!details)}
                style={{ alignSelf: 'flex-start', border: 0, padding: 0, background: 'transparent', color: 'var(--accent)',
                         font: 'inherit', fontSize: 'var(--fs-6)' }}>
          {details ? '▾ ' : '▸ '}{t('fs.mine.details')}
        </button>
        {details && (
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 'var(--sp-3)' }}>
            <Field label={t('fs.mine.cap')}><Select value={newCap} options={capOpts} onChange={setNewCap} /></Field>
            <Field label={t('fs.mine.threads')}><Select value={newThreads} options={threadOpts} onChange={setNewThreads} /></Field>
            <Field label={t('fs.mine.hash_mid')}>
              <Select value={newHash[0]} options={hashOpts(fs, (b) => midBytes(fs.tables, b, !!newLevel), null)}
                      onChange={(v) => setNewHash([v, newHash[1]])} /></Field>
            <Field label={t('fs.mine.hash_end')}>
              <Select value={newHash[1]} options={hashOpts(fs, (b) => endBytes(fs.tables, b), null)}
                      onChange={(v) => setNewHash([newHash[0], v])} /></Field>
          </div>
        )}
        {details && <Field label={t('fs.mine.strength')}><StrengthPick value={newLevel} onChange={setNewLevel} /></Field>}
        <Button variant="primary" size="field" disabled={busy || !token.trim()} onClick={submit}>
          {busy ? t('fs.mine.reading') : t('fs.mine.add')}
        </Button>
        <span style={{ fontSize: 'var(--fs-6)', color: 'var(--bad)', minHeight: '1.5em' }}>
          {fs.adding?.error && !busy ? tErr(fs.adding.error) : ''}
        </span>
      </>
    );
  }

  const tokenBot = fs.bots.find((b) => b.id === tokenFor);
  return (
    <>
      <Toolbar end={<span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-4)' }}><ThreadBar fs={fs} /><TableBar fs={fs} /></span>}>
        <TitleBit title={t('fs.mine.title')} />
      </Toolbar>
      <Page row>
        {fs.bots.length > 0 && (
          <ListPane head={<ListHead aside={t('fs.mine.count', { n: fs.bots.length })}>{t('fs.mine.title')}</ListHead>}
                    footRow foot={<button type="button" className="k-row" onClick={() => setPick('new')}
                                     style={{ flex: 1, height: 'var(--h-field)', padding: '0 var(--sp-3)', border: 0, textAlign: 'left',
                                              color: 'var(--text)', font: 'inherit', fontSize: 'var(--fs-5)',
                                              background: adding ? 'var(--card)' : 'transparent',
                                              boxShadow: adding ? 'inset 2px 0 0 var(--accent)' : 'none' }}>
                               ＋ {t('fs.mine.add_row')}</button>}>
            {fs.bots.map((b) => {
              const st = stateOf(b);
              const on = sel?.id === b.id;
              return (
                <button key={b.id} type="button" className="k-row" onClick={() => setPick(b.id)}
                        style={{ flex: 'none', width: '100%', height: 'var(--h-row2)', padding: '0 var(--sp-3)', border: 0,
                                 borderBottom: '1px solid var(--border-weak)', display: 'flex', alignItems: 'center',
                                 gap: 'var(--sp-2)', lineHeight: 1.3, textAlign: 'left', color: 'var(--text)',
                                 font: 'inherit', background: on ? 'var(--card)' : 'transparent',
                                 boxShadow: on ? 'inset 2px 0 0 var(--accent)' : 'none', whiteSpace: 'nowrap' }}>
                  <BotPhoto src={b.photo} name={nm(b)} size={32} />
                  <span style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', justifyContent: 'center', gap: 2 }}>
                  <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', overflow: 'hidden' }}>
                    <b style={{ fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis' }}>{nm(b)}</b>
                    <span style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: 'var(--sp-1)', fontSize: 'var(--fs-7)', color: st.color }}>
                      <Dot tone={st.tone} />{st.word}
                    </span>
                  </span>
                  <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{t('fs.mine.use', { t: b.threads, m: matchesOf(b) })}{b.level && ` · ${levelName(b.level)}`}</span>
                  </span>
                </button>
              );
            })}
          </ListPane>
        )}
        <div className="k-scroll" style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)' }}>
          <div style={{ width: adding ? 'var(--w-modal)' : 'var(--w-modal-wide)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
            {detail}
          </div>
        </div>
      </Page>
      {tokenBot && (
        <Overlay onClose={() => setTokenFor(null)}>
          <Modal title={t('fs.mine.token_title', { name: nm(tokenBot) })} onClose={() => setTokenFor(null)}
                 actions={<>
                   <span style={{ marginLeft: 'auto' }} />
                   <Button size="field" variant="ghost" onClick={() => setTokenFor(null)}>{t('dialog.cancel')}</Button>
                   <Button size="field" variant="primary" disabled={!newToken.trim() || !!fs.token_change?.busy}
                           onClick={() => run(fsApi.changeToken(tokenBot.id, newToken))}>
                     {fs.token_change?.busy ? t('fs.mine.reading') : t('fs.mine.token_change')}
                   </Button>
                 </>}>
            <Field label={t('fs.mine.token_new')} error={fs.token_change?.error ? tErr(fs.token_change.error) : undefined}>
              <TextField value={newToken} onChange={setNewToken} password invalid={!!fs.token_change?.error} placeholder="fsbm_…" />
            </Field>
          </Modal>
        </Overlay>
      )}
    </>
  );
}

// ---------------------------------------------------------------- Matches

type Live = { bot: FsBotRow; m: FsMatch };
type Over = { bot: FsBotRow; r: FsResult };

const resultWord = (r: FsResult, bot: string, opp: string): string =>
  r.diff === 0 ? t('fs.result.draw') : t('fs.result.win', { name: r.diff > 0 ? bot : opp, d: Math.abs(r.diff) });

const oppName = (r: FsResult): string => r.opponent_name || t('fs.bot_n', { n: r.opponent });

function FsPlay({ fs, onNav, prefs, onKifu }: { fs: FsSnapshot; onNav: (id: NavId) => void; prefs: Prefs; onKifu: (title: string, kifu: string, parts?: string[]) => void }) {
  const live: Live[] = fs.bots.flatMap((bot) => (bot.view?.matches ?? []).map((m) => ({ bot, m })));
  const rev = fs.bots.map((b) => `${b.id}:${b.view?.finished ?? 0}`).join(',');
  const results = useResults(fs.bots.map((b) => b.id), rev);
  const [closed, setClosed] = useState(new Set<string>());
  const over: Over[] = fs.bots
    .flatMap((bot) => (results.get(bot.id) ?? []).map((r) => ({ bot, r })))
    .filter((o) => !closed.has(`${o.bot.id}:${o.r.match}`))
    .sort((a, b) => b.r.t - a.r.t)
    .slice(0, 100);
  const [filter, setFilter] = useState<'all' | 'live' | 'over'>('all');
  const [pick, setPick] = useState('');
  const photos = photosOf(fs);
  const overPhoto = (r: FsResult) => r.opponent_photo ?? photos.get(r.opponent);
  // Two of our own bots playing each other show the match once for each, with the same number.
  const liveKey = (x: Live) => `live:${x.bot.id}:${x.m.id}`;
  const pickedLive = live.find((x) => liveKey(x) === pick) ?? (pick === '' ? live[0] : undefined);
  // A match that just ended stays picked as finished; with nothing picked, the latest finished one shows.
  const pickedOver = pickedLive ? undefined
    : over.find((x) => `over:${x.bot.id}:${x.r.match}` === pick || `live:${x.bot.id}:${x.r.match}` === pick) ?? (pick === '' || pick.startsWith('live:') ? over[0] : undefined);

  if (!live.length && !over.length) {
    const lines = fs.bots.map((b) => {
      if (!liveConn(b.conn)) return `${nm(b)} · ${t('fs.state.off')}`;
      const o = b.view?.outstanding;
      if (o) return t('fs.play.asking', { bot: nm(b), name: o.name || t('fs.bot_n', { n: o.target }) });
      return `${nm(b)} · ${t(pausedOf(b) ? 'fs.state.paused' : 'fs.play.waiting_short')}`;
    });
    return (
      <>
        <Toolbar><span /></Toolbar>
        <Page>
          <div style={{ flex: 1, minHeight: 0, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-5)' }}>
              <div style={{ flex: 'none', width: 160, opacity: 0.6 }}><EmptyBoard /></div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)', maxWidth: 320 }}>
                {fs.connected && <span style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{t('fs.play.waiting')}</span>}
                {lines.map((l) => <span key={l} style={{ fontSize: 'var(--fs-5)', lineHeight: 1.8 }}>{l}</span>)}
                <div style={{ display: 'flex', gap: 'var(--sp-2)', marginTop: 'var(--sp-2)' }}>
                  <Button variant="primary" onClick={() => onNav('fs-bots')}>{t('fs.play.see_bots')}</Button>
                </div>
              </div>
            </div>
          </div>
        </Page>
      </>
    );
  }

  const ownSide = (b: FsBotRow) => ({ own: nm(b), ownPhoto: b.photo, kind: t('fs.kind') });
  const title = pickedLive ? t('fs.vs', { a: nm(pickedLive.bot), b: opp(pickedLive.m) })
    : pickedOver ? t('fs.vs', { a: nm(pickedOver.bot), b: oppName(pickedOver.r) }) : '';
  const kifuParts = pickedOver ? pickedOver.r.legs.map((l) => l.moves ?? '').filter(Boolean) : [];
  return (
    <>
      <Toolbar end={pickedOver && kifuParts.length > 0 && (
        <Button onClick={() => onKifu(`${title} · ${whenFull(pickedOver.r.t / 1000)}`, kifuParts[0], kifuParts)}>{t('fs.play.kifu')}</Button>
      )}>
        <TitleBit title={title} sub={t('fs.kind')} />
      </Toolbar>
      <Page row>
        <ListPane head={<div style={{ flex: 'none', display: 'flex', alignItems: 'center', height: 'var(--h-field)', padding: '0 var(--sp-2)',
                                      borderBottom: '1px solid var(--border-weak)' }}>
                    <Segmented fill value={filter} onChange={setFilter}
                               options={[{ value: 'all', label: t('play.filter_all') }, { value: 'live', label: t('play.filter_live') },
                                         { value: 'over', label: t('play.filter_over') }]} /></div>}
                  foot={over.length > 0 && filter !== 'live' ? (
                    <Button variant="ghost" size="field" className="k-wide"
                            onClick={() => setClosed(new Set([...closed, ...over.map((o) => `${o.bot.id}:${o.r.match}`)]))}>
                      {t('fs.play.close_over', { n: over.length })}</Button>) : undefined}>
          {filter !== 'over' && live.length > 0 && <>
            <ListHead aside={t('fs.count', { n: live.length })}>{t('play.group_live')}</ListHead>
            {live.map((x) => (
              <BotMatchRow key={liveKey(x)} name={opp(x.m)} photo={photos.get(x.m.opponent)}
                           {...ownSide(x.bot)} active={pickedLive === x} onSelect={() => setPick(liveKey(x))} />
            ))}
          </>}
          {filter !== 'live' && over.length > 0 && <>
            <ListHead aside={t('fs.count', { n: over.length })}>{t('play.group_over')}</ListHead>
            {over.map((x) => {
              const k = `over:${x.bot.id}:${x.r.match}`;
              return <BotMatchRow key={k} name={oppName(x.r)} photo={overPhoto(x.r)}
                                  {...ownSide(x.bot)} diff={x.r.result === 'aborted' ? null : x.r.diff}
                                  active={pickedOver === x} onSelect={() => setPick(k)}
                                  onClose={() => setClosed(new Set([...closed, `${x.bot.id}:${x.r.match}`]))} />;
            })}
          </>}
        </ListPane>
        <div style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
          {pickedLive && <Boards legs={pickedLive.m.legs} bot={pickedLive.bot} them={opp(pickedLive.m)} themPhoto={photos.get(pickedLive.m.opponent)}
                                 themRate={pickedLive.m.opponent_rating} meRate={pickedLive.bot.view?.rating} prefs={prefs} />}
          {pickedOver && <OverStrip x={pickedOver} themPhoto={overPhoto(pickedOver.r)} prefs={prefs} />}
        </div>
      </Page>
    </>
  );
}

/** A finished leg drawn as a live one that has stopped, without clocks. */
const overLegs = (r: FsResult): FsLeg[] | null =>
  r.legs.every((l) => l.cells?.length === 64)
    ? r.legs.map((l) => ({
        cells: l.cells!, turn: 0, my_color: l.black ? 1 : 2, clocks_ms: [], discs: l.result_discs ?? l.discs,
        last: l.last ?? null, ply: (l.moves ?? '').length / 2, active: false, thinking: false,
        evals: l.evals ?? [], opp_secs: l.opp_secs ?? [], opp_evals: l.opp_evals ?? [], moves: l.moves ?? '',
      }))
    : null;

function OverStrip({ x, themPhoto, prefs }: { x: Over; themPhoto?: string; prefs: Prefs }) {
  const r = x.r;
  const pts = trendOf(r.legs);
  const legs = overLegs(r);
  return (
    <>
      <div style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)', padding: '0 var(--sp-4)',
                    borderBottom: '1px solid var(--border)', background: 'var(--panel)', fontSize: 'var(--fs-5)', whiteSpace: 'nowrap' }}>
        <Tag tone="sub">{t('ggs.state.finished')}</Tag>
        <b style={{ fontWeight: 600 }}>{resultWord(r, nm(x.bot), oppName(r))}</b>
        <span style={{ color: 'var(--sub)', overflow: 'hidden', textOverflow: 'ellipsis' }}>
          {r.legs.map((l, i) => `${t('fs.leg_n', { n: i + 1 })} ${signed(legDiff(l))}`).join(' · ')}
          {myRating(r) != null && ` · ${t('fs.rating')} ${r.my_rating_before?.toFixed(1) ?? '?'} → ${myRating(r)!.toFixed(1)}`}
        </span>
      </div>
      {legs && <Boards legs={legs} bot={x.bot} them={oppName(r)} themPhoto={themPhoto} themRate={r.opponent_rating} meRate={myRating(r)} prefs={prefs} />}
      {!legs && pts.length > 0 && (
        <div style={{ padding: 'var(--sp-2) var(--sp-4)' }}>
          <EvalTrend points={pts} plies={pliesOf(r.legs)} height={180} title={t('play.trend_title', { name: nm(x.bot) })} split={splitOf(r.legs)}
                     names={{ mine: nm(x.bot), opp: oppName(r) }} />
        </div>
      )}
    </>
  );
}

const signed = (d: number | null) => (d == null ? '—' : d > 0 ? `+${d}` : d < 0 ? `−${-d}` : '0');

type Sides = { bot: FsBotRow; them: string; themPhoto?: string | null; themRate?: number | null; meRate?: number | null };

function Boards({ legs, prefs, ...who }: Sides & { legs: FsLeg[]; prefs: Prefs }) {
  const [ref, width] = useWidth();
  const winH = useWindowHeight();
  const per = legs.length > 1 ? (width - 1) / 2 - 24 : width - 24;
  const side = Math.max(160, Math.floor(Math.min(per, winH - 380)));
  const now = useNow(true, 250);
  const [seen] = useState(() => ({ at: Date.now() }));
  void seen;
  return (
    <div ref={ref} style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      <div style={{ flex: 'none', display: 'flex', justifyContent: 'center' }}>
        {legs.map((l, i) => (
          <React.Fragment key={i}>
            {i > 0 && <div style={{ alignSelf: 'stretch', width: 1, background: 'var(--border-weak)' }} />}
            <div style={{ flex: 1, minWidth: 0, display: 'flex', justifyContent: 'center', padding: '8px 12px 0' }}>
              <LegBoard leg={l} n={legs.length > 1 ? i + 1 : undefined} {...who} side={side} prefs={prefs} now={now} />
            </div>
          </React.Fragment>
        ))}
      </div>
      <div style={{ flex: 1, minHeight: 0, padding: 'var(--sp-2) var(--sp-4)', borderTop: '1px solid var(--border)' }}>
        <EvalTrend points={trendOf(legs)} plies={pliesOf(legs)} height={132} title={t('play.trend_title', { name: nm(who.bot) })} split={splitOf(legs)}
                   names={{ mine: nm(who.bot), opp: who.them }} />
      </div>
    </div>
  );
}

function LegBoard({ leg, n, bot, them, themPhoto, themRate, meRate, side, prefs, now }: Sides & {
  leg: FsLeg; n?: number; side: number; prefs: Prefs; now: number;
}) {
  // The clock of the side to move runs on between polls.
  const [base, setBase] = useState(() => ({ at: now, leg }));
  if (base.leg !== leg) setBase({ at: now, leg });
  const ticking = (c: number) => (leg.active && leg.turn === c ? Math.max(0, (leg.clocks_ms[c - 1] ?? 0) - (now - base.at)) : leg.clocks_ms[c - 1] ?? 0);
  const mine = leg.my_color;
  const theirs = mine === 1 ? 2 : 1;
  const eval0 = leg.evals.length ? leg.evals[leg.evals.length - 1][1] : null;
  const theirEval = leg.opp_evals?.length ? leg.opp_evals[leg.opp_evals.length - 1][1] : null;
  const evalText = (v: number | null) => (v != null ? t('play.eval', { v: (v > 0 ? '+' : '') + v.toFixed(1) }) : undefined);
  const toMove = leg.turn === mine ? nm(bot) : them;
  const state = !leg.active ? t('fs.play.leg_over')
    : leg.thinking ? `${t('play.to_move', { name: toMove })} · ${t('ggs.play.thinking')}`
    : t('play.to_move', { name: toMove });
  const color = (c: number) => (c === 1 ? 'b' : 'w') as 'b' | 'w';
  const clock = (c: number) => (leg.clocks_ms.length ? clockText(ticking(c)) : undefined);
  return (
    <div style={{ width: side, flex: 'none', display: 'flex', flexDirection: 'column' }}>
      <div style={{ height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', fontSize: 'var(--fs-7)', color: 'var(--sub)', whiteSpace: 'nowrap' }}>
        {n !== undefined && <span style={{ fontWeight: 600, color: 'var(--text)' }}>{t('fs.leg_n', { n })}</span>}
        <span style={{ marginLeft: 'auto' }}>{leg.discs[0] ?? 0} – {leg.discs[1] ?? 0} · {t('ggs.play.moves', { n: leg.ply })}</span>
      </div>
      <PlayerRow color={color(theirs)} name={them} rate={themRate ?? undefined} photo={themPhoto ?? null}
                 meta={evalText(theirEval)}
                 clock={clock(theirs)} active={leg.active && leg.turn === theirs} />
      <div style={{ width: side, height: side }}>
        <Board cells={boardCells(leg.cells)} last={leg.last != null ? boardIndex(leg.last) : null} disabled
               turn={leg.turn === 2 ? 'w' : 'b'} coords={prefs.coords} grain={prefs.grain}
               flip={flipped(prefs.facing, mine === 1 ? 'black' : 'white')} />
      </div>
      <PlayerRow color={color(mine)} name={nm(bot)} rate={meRate ?? undefined} photo={bot.photo}
                 meta={evalText(eval0)}
                 clock={clock(mine)} active={leg.active && leg.turn === mine} />
      <div style={{ display: 'flex', alignItems: 'center', height: 'var(--h-row)', whiteSpace: 'nowrap', overflow: 'hidden', fontSize: 'var(--fs-6)',
                    color: leg.thinking ? 'var(--accent)' : 'var(--sub)' }}>{state}</div>
    </div>
  );
}

// ---------------------------------------------------------------- Bots

function useBotPick(fs: FsSnapshot): [FsBotRow | undefined, (id: number) => void, [string, string][]] {
  const mine = fs.bots;
  const [pick, setPick] = useState<number | null>(null);
  const sel = mine.find((b) => b.id === pick) ?? mine.find((b) => liveConn(b.conn)) ?? mine[0];
  return [sel, setPick, mine.map((b): [string, string] => [String(b.id), nm(b)])];
}

function FsBots({ fs, say }: { fs: FsSnapshot; say: (s: string) => void }) {
  const run = act(say);
  const [sel, setPick, opts] = useBotPick(fs);
  const rev = sel ? `${sel.id}:${sel.view?.finished ?? 0}` : '';
  const results = useResults(sel ? [sel.id] : [], rev).get(sel?.id ?? -1) ?? [];
  const now = useNow(!!sel?.view?.outstanding, 1000);
  const [hideBuiltin, setHideBuiltin] = useKept('fs.bots.hide_builtin', false);
  const [onlineOnly, setOnlineOnly] = useKept('fs.bots.online_only', false);
  if (!sel) return <><Toolbar><span /></Toolbar><Page><Blank title={t('fs.bots.none')} /></Page></>;
  const v = sel.view;
  const bots = v?.bots ?? [];
  const rank = bots.findIndex((b) => b.id === sel.id);
  // Ranks are among all bots, whatever the list is narrowed to.
  const shown = bots.map((b, i) => ({ b, i })).filter(({ b }) => (!hideBuiltin || !b.builtin) && (!onlineOnly || b.online !== false || b.playing));
  const w = results.filter((r) => r.diff > 0).length;
  const l = results.filter((r) => r.diff < 0).length;
  const d = results.filter((r) => r.diff === 0 && r.result !== 'aborted').length;
  const rated = results.map(myRating).filter((x): x is number => x != null);
  const recent = rated.length > 12 ? rated[rated.length - 1] - rated[rated.length - 13] : null;
  const out = v?.outstanding;
  const since = out ? clockText(now - out.since_ms) : '';
  const cooling = new Set(v?.cooling ?? []);
  // A challenge taken would start a match, so none is sent while the matches at once are full.
  const full = (v?.matches.length ?? 0) >= matchesOf(sel);
  return (
    <>
      <Toolbar>
        <Select value={String(sel.id)} options={opts} width={132} title={t('fs.bots.mine')} onChange={(x) => setPick(Number(x))} />
        <Divider />
        <Toggle checked={hideBuiltin} onChange={setHideBuiltin} label={t('fs.bots.hide_builtin')} />
        <Toggle checked={onlineOnly} onChange={setOnlineOnly} label={t('fs.bots.online_only')} />
      </Toolbar>
      <Page>
        {pausedOf(sel) && <Band>{t('fs.bots.paused')}</Band>}
        {!sel.accept && <Band>{t('fs.bots.not_accepting')}</Band>}
        <Sections>
          <Section title={t('fs.bots.rating_of', { name: nm(sel) })}>
            <div style={{ display: 'flex', flexWrap: 'wrap', borderTop: '1px solid var(--border-weak)' }}>
              {([
                [t('fs.rating'), v?.rating != null ? v.rating.toFixed(1) : '—'],
                [t('fs.bots.rank'), rank >= 0 ? `#${rank + 1}` : '—'],
                [t('results.wld'), `${w} / ${l} / ${d}`],
                [t('fs.bots.recent'), recent != null ? (recent > 0 ? '+' : '') + recent.toFixed(1) : '—'],
              ] as [string, string][]).map(([label, value]) => (
                <div key={label} style={{ flex: '1 0 auto', minWidth: 150, height: 'var(--h-field)', display: 'flex', alignItems: 'center',
                                          padding: '0 var(--sp-3)', borderRight: '1px solid var(--border-weak)', borderBottom: '1px solid var(--border-weak)' }}>
                  <span style={{ flex: 1, minWidth: 0 }}><KeyValue label={label} value={value} /></span>
                </div>
              ))}
            </div>
          </Section>
          <Section title={t('fs.bots.title')} aside={t('fs.bots.count', { n: shown.length })} grow>
            <Table>
              <div style={{ flex: 'none', height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', padding: '0 var(--sp-3)',
                            borderBottom: '1px solid var(--border)', fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)' }}>
                <span style={{ width: 36, flex: 'none' }}>{t('fs.bots.head_rank')}</span>
                <span style={{ flex: 1, minWidth: 0 }}>{t('fs.bots.head_name')}</span>
                <span style={{ width: 72, flex: 'none', textAlign: 'right' }}>{t('fs.rating')}</span>
                <span style={{ width: 96, flex: 'none' }}>{t('fs.bots.head_state')}</span>
                <span style={{ width: 120, flex: 'none' }} />
              </div>
              <Rows>
                {shown.map(({ b, i }) => {
                  const me = b.id === sel.id;
                  const asked = out?.target === b.id;
                  const st = cooling.has(b.id) ? [t('fs.bots.declined'), 'var(--sub)']
                    : b.playing ? [t('fs.state.playing'), 'var(--ok)']
                    : b.paused ? [t('fs.state.paused'), 'var(--gold)']
                    : b.online == null ? ['', 'var(--sub)'] : b.online ? [t('fs.bots.online'), 'var(--accent)'] : [t('fs.bots.offline'), 'var(--sub)'];
                  return (
                    <div key={b.id} style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-field)',
                                             padding: '0 var(--sp-3)', borderBottom: '1px solid var(--border-weak)', fontSize: 'var(--fs-5)',
                                             whiteSpace: 'nowrap', fontWeight: me ? 600 : 400 }}>
                      <span style={{ width: 36, flex: 'none', color: i === 0 ? 'var(--gold)' : 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>#{i + 1}</span>
                      <span style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', overflow: 'hidden' }}>
                        <BotPhoto src={b.photo} name={b.name} size={20} />
                        <span className="k-sel" style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{b.name}</span>
                        {b.builtin && (
                          <span role="img" aria-label={t('fs.bots.builtin')} title={t('fs.bots.builtin')}
                                style={{ flex: 'none', display: 'grid', placeItems: 'center', color: 'var(--accent)' }}>
                            <Icon name="cpu" size={15} />
                          </span>
                        )}
                      </span>
                      <span style={{ width: 72, flex: 'none', textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>{b.rating != null ? b.rating.toFixed(1) : '—'}</span>
                      <span style={{ width: 96, flex: 'none', color: st[1], fontSize: 'var(--fs-6)', fontWeight: 400 }}>{st[0]}</span>
                      <span style={{ width: 120, flex: 'none', display: 'flex', justifyContent: 'flex-end', fontWeight: 400 }}>
                        {asked ? <span style={{ fontSize: 'var(--fs-6)', color: 'var(--accent)' }}>{t('fs.bots.asking', { s: since })}</span>
                          : !me && <Button size="row" disabled={b.online === false || b.playing || !!out || full || !liveConn(sel.conn)}
                                           onClick={() => run(fsApi.challenge(sel.id, b.id))}>{t('fs.bots.ask')}</Button>}
                      </span>
                    </div>
                  );
                })}
              </Rows>
            </Table>
          </Section>
        </Sections>
      </Page>
    </>
  );
}

// ---------------------------------------------------------------- Results

type Period = 'all' | '7' | '30' | '90';

function FsResults({ fs, onKifu }: { fs: FsSnapshot; onKifu: (title: string, kifu: string, parts?: string[]) => void }) {
  const [sel, setPick, opts] = useBotPick(fs);
  const rev = sel ? `${sel.id}:${sel.view?.finished ?? 0}` : '';
  const got = useResults(sel ? [sel.id] : [], rev);
  const photos = photosOf(fs);
  const all = useMemo(() => got.get(sel?.id ?? -1) ?? [], [got, sel?.id]);
  const [chart, setChart] = useKept('fs.results.chart', true);
  const [period, setPeriod] = useState<Period>('all');
  const [since, setSince] = useState(0);
  const [opp, setOpp] = useState('');
  const [page, setPage] = useState(0);
  const [perPage, setPerPage] = useState(50);
  const [hover, setHover] = useState<number | null>(null);
  const [pickRow, setPickRow] = useState<number | null>(null);
  const [chartRef, chartW] = useWidth();
  const inPeriod = useMemo(() => all.filter((r) => r.t >= since && r.result !== 'aborted'), [all, since]);
  const rows = useMemo(() => inPeriod.filter((r) => !opp || oppName(r) === opp), [inPeriod, opp]);
  if (!sel) return <><Toolbar><span /></Toolbar><Page><Blank title={t('fs.bots.none')} /></Page></>;
  const newest = [...rows].reverse();
  const pages = Math.max(1, Math.ceil(newest.length / perPage));
  const cur = Math.min(page, pages - 1);
  const shown = newest.slice(cur * perPage, (cur + 1) * perPage);
  const rated = rows.filter((r) => myRating(r) != null);
  const rates = rated.map((r) => myRating(r)!);
  const w = rows.filter((r) => r.diff > 0).length;
  const l = rows.filter((r) => r.diff < 0).length;
  const d = rows.filter((r) => r.diff === 0).length;
  const avg = rows.length ? rows.reduce((a, r) => a + r.diff, 0) / rows.length : null;
  const delta = rates.length >= 2 ? rates[rates.length - 1] - rates[0] : null;
  const opps = [...new Set(inPeriod.map(oppName))].map((n): [string, string] => [n, `${n} · ${t('fs.count', { n: inPeriod.filter((r) => oppName(r) === n).length })}`]);
  const picked = shown.find((r) => r.match === pickRow) ?? rows.find((r) => r.match === pickRow);
  const parts = picked ? picked.legs.map((x) => x.moves ?? '').filter(Boolean) : [];
  const ending = (r: FsResult) => (r.finish_reason && r.finish_reason.includes('timeout')
    ? { text: r.diff > 0 ? t('fs.result.time_win') : t('fs.result.time_loss'), tone: (r.diff > 0 ? 'gold' : 'bad') as 'gold' | 'bad' } : null);
  const stats: [string, string][] = [
    [t('fs.rating'), rates.length ? rates[rates.length - 1].toFixed(1) : '—'],
    [t('results.change'), delta != null ? (delta > 0 ? '+' : '') + delta.toFixed(1) : '—'],
    [t('results.wld'), `${w} / ${l} / ${d}`],
    [t('results.win_rate'), rows.length ? `${Math.round((w / rows.length) * 100)}%` : '—'],
    [t('results.avg_diff'), avg != null ? (avg > 0 ? '+' : '') + avg.toFixed(1) : '—'],
    [t('results.best'), rates.length ? Math.max(...rates).toFixed(1) : '—'],
  ];
  return (
    <>
      <Toolbar end={<>
        <Button disabled={cur === 0} onClick={() => setPage(cur - 1)}>{t('ggs.prev')}</Button>
        <span style={{ fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>{rows.length ? `${cur + 1} / ${pages}` : '0 / 0'}</span>
        <Button disabled={cur >= pages - 1} onClick={() => setPage(cur + 1)}>{t('ggs.next')}</Button>
        <Select size="ctrl" value={String(perPage)} title={t('ggs.users.per_page')}
                options={[50, 100, 200].map((n): [string, string] => [String(n), t('players.per', { n })])}
                onChange={(v) => { setPerPage(+v); setPage(0); }} />
      </>}>
        <Select value={String(sel.id)} options={opts} width={132} title={t('fs.bots.mine')} onChange={(x) => { setPick(Number(x)); setPage(0); setOpp(''); }} />
        <Divider />
        <Select value={period} width={128}
                onChange={(v) => { setPeriod(v as Period); setSince(v === 'all' ? 0 : Date.now() - Number(v) * 86400e3); setPage(0); }}
                options={[['all', t('results.period_all')], ['7', t('results.period_days', { n: 7 })],
                          ['30', t('results.period_days', { n: 30 })], ['90', t('results.period_days', { n: 90 })]]} />
        <Combobox value={opp} options={[['', t('results.opp_all')], ...opps]} width={160} title={t('results.opp')}
                  onChange={(v) => { setOpp(v); setPage(0); }} />
      </Toolbar>
      <Page row>
        {!all.length ? <Blank title={t('ggs.no_records')} />
          : !rows.length ? <Blank title={t('results.none_matching')} /> : (
          <Sections>
            <Section title={t('fs.results.trend')}
                     fold={{ open: chart, onToggle: () => setChart(!chart), summary: stats.slice(0, 3) }}>
              <div ref={chartRef} style={{ padding: '0 var(--sp-3)' }}>
                <RateChart points={rates} width={chartW} height={168} dates={rated.map((r) => whenShort(r.t / 1000))}
                           labels={rated.map((r) => `${whenFull(r.t / 1000)} · ${oppName(r)} · ${signed(r.diff)} · ${myRating(r)!.toFixed(1)}`)}
                           hover={hover} onHover={setHover} />
              </div>
              <div style={{ display: 'flex', flexWrap: 'wrap', marginTop: 'var(--sp-2)', borderTop: '1px solid var(--border-weak)' }}>
                {stats.map(([label, value]) => (
                  <div key={label} style={{ flex: '1 0 auto', minWidth: 150, height: 'var(--h-field)', display: 'flex', alignItems: 'center',
                                            padding: '0 var(--sp-3)', borderRight: '1px solid var(--border-weak)', borderBottom: '1px solid var(--border-weak)' }}>
                    <span style={{ flex: 1, minWidth: 0 }}><KeyValue label={label} value={value} /></span>
                  </div>
                ))}
              </div>
            </Section>
            <Section title={t('ggs.results.finished')} aside={t('fs.count', { n: rows.length })} grow>
              <Table>
                <ResultHead noOutcome rating delta boards boardLabel={(n) => t('fs.leg_n', { n })} ratingLabel={t('fs.rating')} />
                <Rows key={`${sel.id}|${period}|${opp}|${perPage}|${cur}`}>
                  {shown.map((r) => (
                    <ResultRow key={r.match} noOutcome opponent={oppName(r)} picked={r.match === pickRow}
                               photo={r.opponent_photo ?? photos.get(r.opponent) ?? null}
                               onHover={(on) => { const i = rated.indexOf(r); setHover(on && i >= 0 ? i : null); }}
                               win={r.diff > 0} draw={r.diff === 0} discs={r.diff} when={whenShort(r.t / 1000)}
                               ending={ending(r)} boards={r.legs.map((x) => ({ diff: legDiff(x) }))}
                               rating={myRating(r)} delta={myRating(r) != null && r.my_rating_before != null ? myRating(r)! - r.my_rating_before : null}
                               onClick={() => setPickRow(r.match)} dim={!r.legs.some((x) => x.moves)} />
                  ))}
                </Rows>
              </Table>
            </Section>
          </Sections>
        )}
        {picked && (
          <Drawer label={t('results.this_game')} onClose={() => setPickRow(null)}
                  title={<span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', minWidth: 0 }}>
                    <BotPhoto src={picked.opponent_photo ?? photos.get(picked.opponent)} name={oppName(picked)} size={24} />
                    <span className="k-sel" style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{oppName(picked)}</span></span>}>
            <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', padding: 'var(--sp-4) var(--sp-4) var(--sp-5)' }}>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                  <KeyValue label={t('results.when')} value={whenFull(picked.t / 1000)} />
                  {picked.opponent_rating != null && <KeyValue label={t('fs.results.opp_rating')} value={picked.opponent_rating.toFixed(1)} />}
                  <KeyValue label={t('data.result.head_discs')} value={`${signed(picked.diff)} (${picked.legs.map((x, i) => `${t('fs.leg_n', { n: i + 1 })} ${signed(legDiff(x))}`).join(' · ')})`} />
                  <KeyValue label={t('results.my_color')} value={picked.legs.map((x, i) => `${t('fs.leg_n', { n: i + 1 })} ${t(x.black ? 'data.color.black' : 'data.color.white')}`).join(' · ')} />
                  {myRating(picked) != null && (
                    <KeyValue label={t('fs.rating')} value={myRating(picked)!.toFixed(1)
                      + (picked.my_rating_before != null ? ` (${signed(Math.round((myRating(picked)! - picked.my_rating_before) * 10) / 10)})` : '')} />
                  )}
                </div>
                {trendOf(picked.legs).length > 0 && (
                  <EvalTrend points={trendOf(picked.legs)} plies={pliesOf(picked.legs)} height={180} title={t('play.trend_title', { name: nm(sel) })} split={splitOf(picked.legs)}
                             names={{ mine: nm(sel), opp: oppName(picked) }} />
                )}
                <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
                  <Button variant="primary" size="field" disabled={!parts.length}
                          onClick={() => onKifu(`${t('fs.vs', { a: nm(sel), b: oppName(picked) })} · ${whenFull(picked.t / 1000)}`, parts[0], parts)}>
                    {t('results.open_kifu')}
                  </Button>
                </div>
              </div>
            </div>
          </Drawer>
        )}
      </Page>
    </>
  );
}
