import React, { useLayoutEffect, useRef, useState } from 'react';
import { ggsApi } from './api';
import type { GgsSnapshot, MatchView } from './types';
import {
  condToSrc, countDiscs, ggsMoveToIndex, gtypeLabel, parseCond, useClocks,
  type ClockSide, type ClockView, type Cond,
} from './ggs';
import { t, useLang } from './i18n';
import { Caption, EmptyBoard, EmptyState, ListRow, popoverStyle, Toolbar, useDismiss } from './components/layout';
import { Icon } from './components/Icons';
import { Button, Segmented, TextArea } from './components/primitives';
import { Confirm, SettingRow } from './Dialogs';
import { FormulaEditor, MatchRow, Tag, type Match, type NavId } from './components/ggs';
import { Board, type Cell, type EvalInfo } from './components/board';
import { EvalTrend, PlayerRow } from './components/data';
import { flipped, type Prefs } from './prefs';
import { askSheet } from './AskSheet';
import { Lobby, takeFocus } from './ggs/Lobby';
import { focusPlayer, Players } from './ggs/Players';
import { Results } from './ggs/Results';
import { Chat } from './ggs/Chat';
import { Tourney } from './ggs/Tourney';
import { Console } from './ggs/Console';
import { Login } from './ggs/Login';
import { act, condLine, ListHead, ListPane, Page, signed as signedNum, sumBoards, TitleBit, type TrendPoint } from './ggsui';


export function GgsScreen({ nav, snap, onNav, prefs, onKifu }: {
  nav: NavId; snap: GgsSnapshot | null; onNav: (id: NavId) => void; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  useLang();
  if (nav === 'ggs-login') return <Login conn={snap?.conn ?? 'disconnected'} server={snap?.server ?? ''} />;
  // Unconnected GGS screens are sent to the login screen (App's `reachable`), so no snapshot means
  // the first one has not arrived yet.
  if (!snap) return null;

  switch (nav) {
    case 'ggs-play': return <GgsPlay snap={snap} onNav={onNav} prefs={prefs} onKifu={onKifu} />;
    case 'ggs-lobby': return <Lobby snap={snap} onNav={onNav} />;
    case 'ggs-players': return <Players snap={snap} onNav={onNav} onKifu={onKifu} />;
    case 'ggs-results': return <Results snap={snap} onKifu={onKifu} />;
    case 'ggs-chat': return <Chat snap={snap} />;
    case 'ggs-tourney': return <Tourney snap={snap} onNav={onNav} />;
    case 'ggs-console': return <Console snap={snap} />;
    default: return null;
  }
}

/**
 * "観戦 3": who is watching this match, kept live from the server's notices. Pressing it opens
 * their names under it; a name opens that player. With nobody watching it cannot be pressed.
 */
function Watchers({ names, onPick }: { names: string[]; onPick: (name: string) => void }) {
  const [at, setAt] = useState<{ top: number; left: number } | null>(null);
  const btn = useRef<HTMLSpanElement>(null);
  const box = useRef<HTMLDivElement>(null);
  // The last one leaving closes the list for good (it must not reopen when someone comes later).
  if (!names.length && at) setAt(null);
  const open = names.length > 0 ? at : null;
  useDismiss(open !== null, () => setAt(null), btn, box);
  const toggle = () => {
    if (open) { setAt(null); return; }
    const r = btn.current?.getBoundingClientRect();
    if (!r) return;
    // Under the button, from its left edge; near the window's right edge, ending at its right edge.
    const w = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--w-popover')) || 300;
    setAt({ top: r.bottom + 4, left: r.left + w > window.innerWidth - 8 ? Math.max(8, r.right - w) : r.left });
  };
  return (
    <span ref={btn} style={{ display: 'flex' }}>
      <Button variant="ghost" disabled={!names.length} title={t('ggs.watchers.title')} onClick={toggle}>
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 'var(--sp-1)', color: names.length ? 'var(--text)' : undefined }}>
          <Icon name="watch" size={15} />
          {t('ggs.watchers.button')}
          <b style={{ fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>{names.length}</b>
        </span>
      </Button>
      {open && (
        // Fixed to the window: the toolbar around the button must not cut it off.
        <div ref={box} role="dialog" style={{ ...popoverStyle, position: 'fixed', top: open.top, left: open.left, zIndex: 20, width: 'var(--w-popover)' }}>
          <div style={{ flex: 'none', height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', padding: 'var(--sp-1) var(--sp-3) 0' }}>
            <Caption>{t('ggs.watchers.title')}</Caption>
            <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{t('ggs.watchers.count', { n: names.length })}</span>
          </div>
          <div className="k-scroll" style={{ maxHeight: 'calc(var(--h-field) * 7)', display: 'flex', flexDirection: 'column', padding: 'var(--sp-1) 0' }}>
            {names.map((name) => (
              <ListRow key={name} rule={false} title={t('ggs.watchers.open_player')}
                       onClick={() => { setAt(null); onPick(name); }}>
                <span className="k-sel" style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>{name}</span>
              </ListRow>
            ))}
          </div>
        </div>
      )}
    </span>
  );
}

export function GgsPlay({ snap, onNav, prefs, onKifu }: {
  snap: GgsSnapshot; onNav: (id: NavId) => void; prefs: Prefs;
  onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  const [sel, setSel] = useState(() => takeFocus());
  const [filter, setFilter] = useState<'all' | 'live' | 'over'>('all');
  const [ask, setAsk] = useState<'' | 'resign' | 'break'>('');
  const clock = useClocks(snap.matches);

  const groups = new Map<string, MatchView[]>();
  for (const m of snap.matches) groups.set(m.base, [...(groups.get(m.base) ?? []), m]);
  const fresh = (k: string) => Math.max(...groups.get(k)!.map((m) => m.order));
  const keys = [...groups.keys()].sort((a, b) => fresh(b) - fresh(a));
  const isLive = (k: string) => !groups.get(k)!.every((m) => m.over);
  const live = keys.filter(isLive);
  const over = keys.filter((k) => !isLive(k));
  const cur = groups.has(sel) ? sel : live[0] ?? over[0] ?? '';
  const pair = groups.get(cur);

  if (!groups.size) {
    // A request out or in is what happens next, so say so instead of "nothing here".
    const sent = snap.offers.filter((o) => !o.incoming && o.names[0] === snap.login);
    const got = snap.offers.filter((o) => o.incoming);
    const stored = snap.stored;
    const pending = sent.length + got.length + stored.length > 0;
    const lines = [
      ...stored.map((x) => `${t('ggs.state.adjourned')} · ${x.opp || '?'} · ${gtypeLabel(x.gtype)}`),
      ...got.map((o) => `${t('play.offer_from', { name: o.names[0] })} · ${condLine(o.gtype, o.time, o.rated)}`),
      ...sent.map((o) => `${o.names.slice(1).join(t('ggs.and_separator')) || t('ask.anyone_short')} · ${condLine(o.gtype, o.time, o.rated)}`),
    ];
    return (
      <>
        <Toolbar><TitleBit title={t('ggs.nav.games')} /></Toolbar>
        <EmptyState title={stored.length ? t('play.stored_title', { n: stored.length }) : got.length ? t('play.offers_in', { n: got.length })
                             : sent.length ? t('play.waiting_title') : t('play.empty_title')}
                    visual={<EmptyBoard />}
                    body={pending ? <>
                      {lines.map((l) => <span key={l} style={{ display: 'block', color: 'var(--text)' }}>{l}</span>)}
                      <span style={{ display: 'block' }}>
                        {stored.length ? t('play.stored_body') : got.length ? t('play.offers_in_body') : t('play.waiting_body')}
                      </span>
                    </> : t('play.empty_body')}
                    actions={pending ? <>
                      {stored.length === 1 && (
                        <Button variant="primary" onClick={() => act(ggsApi.resumeStored(stored[0].id))}>{t('ggs.lobby.resume')}</Button>
                      )}
                      <Button variant={stored.length === 1 ? undefined : 'primary'} onClick={() => onNav('ggs-lobby')}>{t('play.to_lobby_reply')}</Button>
                      {!stored.length && !got.length && sent.length === 1 && (
                        <Button onClick={() => act(ggsApi.raw(`tell /os cancel ${sent[0].id}`))}>{t('lobby.cancel')}</Button>
                      )}
                    </> : <>
                      <Button variant="primary" onClick={() => askSheet.open({})}>{t('ask.title')}</Button>
                      <Button onClick={() => onNav('ggs-lobby')}>{t('play.to_lobby')}</Button>
                    </>} />
      </>
    );
  }

  const first = pair?.[0];
  const mine = !!pair?.some((m) => m.my_color);
  const ended = !!pair?.every((m) => m.over);
  const tourney = snap.td.games.some((g) => g.id === cur || g.id === first?.id);
  // Own games put the login first; watched ones put the first board's black first.
  const blackOf = (m: MatchView) => m.players.find((p) => p.color === 'black')?.name ?? m.players[0]?.name ?? '?';
  const whiteOf = (m: MatchView) => m.players.find((p) => p.color === 'white')?.name ?? m.players[1]?.name ?? '?';
  const names: [string, string] = first
    ? (mine ? [snap.login, first.opp_name || '?'] : [blackOf(first), whiteOf(first)])
    : ['?', '?'];
  const rated = pair?.find((m) => m.rated != null)?.rated ?? null;
  const ending = pair?.map((m) => m.ended).find(Boolean) ?? '';
  const leftBy = pair?.map((m) => m.left_by).find(Boolean) ?? '';
  const sum = pair ? matchSum(pair, names[0]) : null;
  const send = (verb: 'resign' | 'break') => { setAsk(''); act(ggsApi.matchCmd(cur, verb)); };
  const showKifu = () => {
    if (!first) return;
    onKifu(t('ggs.vs', { a: names[0], b: names[1] }), first.ggf || first.moves.join(''), first.archive || undefined);
  };
  const shown = (k: string) => filter === 'all' || (filter === 'live' ? isLive(k) : !isLive(k));
  const rows = (list: string[]) => list.filter(shown).map((key) => (
    <MatchRow key={key} m={matchRowOf(groups.get(key)!, snap.login)}
              active={key === cur} onSelect={() => setSel(key)}
              onClose={() => {
                act(ggsApi.closeMatch(key));
                if (key === sel) setSel('');
              }} />
  ));

  return (
    <>
      <Toolbar end={<>
        {!ended && first && (
          <Watchers names={snap.watchers[first.base] ?? []}
                    onPick={(name) => { focusPlayer(name); onNav('ggs-players'); }} />
        )}
        {mine && !ended && !tourney && <Button onClick={() => setAsk('break')}>{t('ggs.adjourn')}</Button>}
        {mine && !ended && <Button variant="danger" onClick={() => setAsk('resign')}>{t('ggs.resign')}</Button>}
        {!mine && !ended && first && (
          <Button onClick={() => act(ggsApi.watch(cur, false))}>{t('ggs.stop_observing')}</Button>
        )}
        {ended && <Button onClick={showKifu}>{t('ggs.game_record')}</Button>}
      </>}>
        <TitleBit title={`${names[0]} ${t('ggs.vs_word')} ${names[1]}`}
                  sub={first ? [tourney ? t('play.tourney_game') : '', condLine(first.gtype, '', rated)].filter(Boolean).join(' · ') : ''} />
      </Toolbar>
      <Page row>
        <ListPane
          head={<div style={{ flex: 'none', padding: 'var(--sp-1) var(--sp-2)', borderBottom: '1px solid var(--border-weak)', display: 'flex' }}>
            <Segmented value={filter} onChange={setFilter} fill
                       options={[{ value: 'all', label: t('play.filter_all') },
                                 { value: 'live', label: t('play.filter_live') },
                                 { value: 'over', label: t('play.filter_over') }]} />
          </div>}
          foot={over.length > 0 && (
            <Button variant="ghost" size="field" className="k-wide"
                    onClick={() => { for (const k of over) act(ggsApi.closeMatch(k)); }}>
              {t('play.close_over', { n: over.length })}
            </Button>
          )}>
          {live.some(shown) && <ListHead aside={t('ggs.lobby.game_count', { n: live.length })}>{t('play.group_live')}</ListHead>}
          {rows(live)}
          {over.some(shown) && <ListHead aside={t('ggs.lobby.game_count', { n: over.length })}>{t('play.group_over')}</ListHead>}
          {rows(over)}
        </ListPane>

        <div style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
          {ended && pair && (
            <div style={{
              flex: 'none', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', height: 'var(--h-field)',
              padding: '0 var(--sp-4)', borderBottom: '1px solid var(--border)', background: 'var(--panel)',
              fontSize: 'var(--fs-5)', whiteSpace: 'nowrap', overflow: 'hidden',
            }}>
              <Tag tone={ending === 'adjourned' ? 'bad' : 'sub'}>
                {ending === 'adjourned' ? t('ggs.state.adjourned') : ending === 'aborted' ? t('ggs.state.aborted') : t('ggs.state.finished')}
              </Tag>
              <b style={{ fontWeight: 600 }}>
                {ending === 'adjourned' ? (leftBy ? t('play.left_adjourned', { who: leftBy }) : t('ggs.state.adjourned'))
                  : ending === 'aborted' ? t('ggs.state.aborted')
                  : sum != null ? pairOutcome(sum, names[0], names[1]) : `${names[0]} − ${names[1]}`}
              </b>
              <span style={{ color: 'var(--sub)', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                {[ending === 'adjourned' && rated ? t('play.resume_where') : '',
                  ending === 'finished' && pair.length === 2 ? t('play.seen_from', { name: names[0] }) + ' ' + pair.map((m, i) => `${t('ggs.play.board_n', { n: i + 1 })} ${boardDiff(m, names[0])}`).join(' · ') : '',
                  condLine(pair[0].gtype, '', rated)].filter(Boolean).join(' · ')}
              </span>
            </div>
          )}
          {pair && (
            <BoardsAndGraph pair={pair}
              board={(m, side, face) => (
                <MatchBoard key={m.id} snap={snap} m={m} clock={clock} prefs={prefs} face={face} side={side} lead={names[0]} />
              )}
              graph={(h) => (
                <EvalTrend points={trendOf(pair, names[0])} plies={Math.max(...pair.map(pliesOf))} height={h}
                           title={t('play.trend_title', { name: names[0] })}
                           split={pair.length === 2 ? splitAt(pair[0], pair[1]) : undefined}
                           names={{ mine: names[0], opp: names[1] }} />
              )} />
          )}
        </div>
      </Page>
      {ask === 'break' && (
        <Confirm title={t('ggs.play.adjourn_confirm')} ok={t('ggs.play.adjourn_ok')}
                 body={rated === true ? t('ggs.play.adjourn_body_rated')
                   : rated === false ? t('ggs.play.adjourn_body_unrated') : t('ggs.play.adjourn_body_unknown')}
                 onCancel={() => setAsk('')} onOk={() => send('break')} />
      )}
      {ask === 'resign' && (
        <Confirm title={t('ggs.play.resign_confirm')} ok={t('ggs.play.resign_ok')} danger
                 body={t('ggs.play.resign_body')}
                 onCancel={() => setAsk('')} onOk={() => send('resign')} />
      )}
    </>
  );
}

/** Whether `name` has black on this board (an own game knows its colour directly). */
const isBlack = (m: MatchView, name: string): boolean =>
  m.my_color ? m.my_color === 'black'
    : (m.players.find((p) => p.name === name)?.color ?? 'black') === 'black';

/** The final disc difference of one board from `name`'s side (GGS writes it from black's). */
function boardDiff(m: MatchView, name: string): string {
  const v = Number.parseFloat(m.result);
  if (!Number.isFinite(v)) return m.result || '—';
  const d = isBlack(m, name) ? v : -v;
  return d === 0 ? '±0' : signedNum(d);
}

/** A finished match told in words: who won it and by how much, or a draw. */
function pairOutcome(sum: number, a: string, b: string): string {
  if (sum === 0) return t('play.draw');
  return t('play.won_by', { name: sum > 0 ? a : b, d: Math.abs(sum) });
}

/** The match total from `name`'s side: one board, or the sum of a synchro pair. */
function matchSum(g: MatchView[], name: string): number | null {
  if (!g.every((x) => x.over && x.ended === 'finished')) return null;
  let total = 0;
  for (const m of g) {
    const v = Number.parseFloat(m.result);
    if (!Number.isFinite(v)) return null;
    total += isBlack(m, name) ? v : -v;
  }
  return total;
}

function matchRowOf(g: MatchView[], login: string): Match {
  const m = g[0];
  const mine = g.some((x) => x.my_color);
  // Own games name the login first; watched ones name the first board's black first.
  const a = mine ? login : m.players.find((p) => p.color === 'black')?.name ?? m.players[0]?.name ?? '?';
  const b = mine ? (m.opp_name || '?') : m.players.find((p) => p.name !== a)?.name ?? '?';
  const sum = matchSum(g, a);
  return {
    mine, live: !g.every((x) => x.over),
    opponent: m.opp_name,
    black: a, white: b,
    kind: gtypeLabel(m.gtype),
    result: sum != null ? pairOutcome(sum, a, b) : undefined,
    ended: g.map((x) => x.ended).find(Boolean) || undefined,
    leftBy: g.map((x) => x.left_by).find(Boolean) || undefined,
  };
}

/** The play column's side padding (`--sp-3`); a synchro board keeps it from the divider too. */
// Each board column keeps var(--sp-3) on both sides and var(--sp-2) above, as in the design.
const EDGE_PAD = 12;
const TOP_PAD = 8;
// The graph's chart: its usual height, the least it shrinks to in a short window, the most it grows to,
// and what its heading, legend and axis take around the chart.
const TREND_H = 132;
const TREND_MIN = 64;
const TREND_MAX = 260;
const TREND_CHROME = 28 + 16;
const BOARD_MIN = 120;
const BOARD_EASY = 300;

/**
 * The boards (one, or a synchro pair side by side) and the eval graph under them share one area.
 * The board takes the largest square that fits; when the window is wide and short the graph gives up
 * height first (down to TREND_MIN), and when the board is held back by the width the height left over
 * goes to the graph, so there is no empty band between them.
 */
function BoardsAndGraph({ pair, board, graph }: {
  pair: MatchView[];
  board: (m: MatchView, side: number, face?: number) => React.ReactNode;
  graph: (height: number) => React.ReactNode;
}) {
  const area = useRef<HTMLDivElement | null>(null);
  const [box, setBox] = useState({ w: 0, h: 0 });
  const [chrome, setChrome] = useState(116);
  const two = pair.length === 2;

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      setBox((b) => (Math.abs(b.w - r.width) < 1 && Math.abs(b.h - r.height) < 1 ? b : { w: r.width, h: r.height }));
    };
    measure();
    const o = new ResizeObserver(measure);
    o.observe(el);
    return () => o.disconnect();
  }, []);

  const perBoard = two ? (box.w - 1) / 2 - 2 * EDGE_PAD : box.w - 2 * EDGE_PAD;
  const tall = box.h - TOP_PAD - chrome - TREND_CHROME;      // height for the board and the chart together
  // The graph keeps its usual height unless that would leave the board under BOARD_EASY; then it shrinks.
  const room = Math.max(tall - TREND_H, Math.min(tall - TREND_MIN, BOARD_EASY));
  const side = Math.max(BOARD_MIN, Math.floor(Math.min(perBoard, room)));
  const chart = Math.max(TREND_MIN, Math.min(TREND_MAX, tall - side));
  const boardsH = box.h - chart - TREND_CHROME;

  // The rows above and below a board are measured once they are drawn.
  useLayoutEffect(() => {
    const col = area.current?.querySelector<HTMLElement>('[data-col]');
    const b = col?.querySelector<HTMLElement>('[data-board]');
    if (!col || !b) return;
    const c = col.offsetHeight - b.offsetHeight;
    if (Math.abs(c - chrome) >= 1) setChrome(c);
  }, [chrome, side, pair]);

  return (
    <div ref={area} style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>
      {box.w > 0 && <>
        <div style={{ height: boardsH, flex: 'none', display: 'flex', justifyContent: 'center', alignItems: 'flex-start', overflow: 'hidden' }}>
          <div style={column}>{board(pair[0], side, two ? 1 : undefined)}</div>
          {two && <>
            <div style={{ alignSelf: 'stretch', width: 1, flex: 'none', background: 'var(--border-weak)' }} />
            <div style={column}>{board(pair[1], side, 2)}</div>
          </>}
        </div>
        <div style={{ flex: 1, minHeight: 0, padding: 'var(--sp-2) var(--sp-4)', borderTop: '1px solid var(--border)' }}>
          {graph(chart)}
        </div>
      </>}
    </div>
  );
}

const column: React.CSSProperties = {
  flex: 1, minWidth: 0, display: 'flex', justifyContent: 'center', padding: `${TOP_PAD}px ${EDGE_PAD}px 0`,
};

// GGS numbers a pass like a move; only placed discs count as moves here.
const isPass = (mv: string) => /^(pa|pass)$/i.test(mv);
const placed = (m: MatchView) => m.moves.filter((mv) => !isPass(mv)).length;
// Both boards of a synchro pair start from one position; the first move that differs is where the
// two games split (counted in placed discs, like the graph).
function splitAt(a: MatchView, b: MatchView): number | undefined {
  const pa = a.moves.filter((mv) => !isPass(mv)).map((mv) => mv.toLowerCase());
  const pb = b.moves.filter((mv) => !isPass(mv)).map((mv) => mv.toLowerCase());
  const n = Math.min(pa.length, pb.length);
  for (let i = 0; i < n; i++) if (pa[i] !== pb[i]) return i + 1;
  return undefined;
}

const pliesOf = (m: MatchView) => m.cells.filter((c) => c === 0).length + placed(m);



// Each side's reported evals, turned to `name`'s side: an own game reports "mine" for the login,
// a watched one for black, so a board where `name` is white swaps the two lines.
function boardTrend(m: MatchView, name: string): TrendPoint[] {
  // For a watched game the series marks black's moves; flip that when `name` is white. Each eval
  // is the mover's own, so it is never flipped here: `name`'s moves give theirs, the other side's
  // are negated below.
  const swap = !m.my_color && !isBlack(m, name);
  return m.eval_series.map((p) => {
    const first = swap ? !p.mine : p.mine;
    const v = p.eval;
    return {
      x: p.n,
      mine: first ? v : null,
      opp: first || v == null ? null : -v,
      mySecs: first ? p.secs : null,
      oppSecs: first ? null : p.secs,
    };
  });
}

// A synchro pair starts both boards from one position with the colours swapped, so the two
// boards' evals at the same move number cancel out; what is left shows which board moved the
// match. Each board keeps its last reported value until it reports again.
function trendOf(pair: MatchView[], name: string): TrendPoint[] {
  return sumBoards(pair.map((m) => boardTrend(m, name)));
}

function MatchBoard({ snap, m, clock, prefs, face, side, lead }: {
  snap: GgsSnapshot; m: MatchView; clock: (id: string, side: ClockSide) => ClockView; prefs: Prefs;
  face?: number;
  side: number;
  /** The player named first; results under the board are from their side. */
  lead: string;
}) {
  const observer = !m.my_color;
  const { black, white } = countDiscs(m.cells);
  const last = [...m.moves].reverse().map(ggsMoveToIndex).find((x) => x !== null) ?? null;
  const myRate = snap.my_ranks.find((r) => r.gtype === (m.gtype.includes('r') ? '8r' : '8'))?.rating;

  // The last value each side reported, with its thinking time.
  const lastOf = (want: boolean): string | undefined => {
    for (let i = m.eval_series.length - 1; i >= 0; i--) {
      const p = m.eval_series[i];
      if (p.mine !== want || p.eval == null) continue;
      const v = p.eval;
      return t('play.eval', { v: (v > 0 ? '+' : '') + v.toFixed(1) })
        + (p.secs != null ? ` · ${p.secs.toFixed(0)}${t('unit.s')}` : '');
    }
    return undefined;
  };
  const myEval = m.last_eval != null
    ? t('play.eval', {
        v: (m.last_from_book ? t('ggs.play.book_mark') + ' ' : '') + (m.last_eval > 0 ? '+' : '')
          + (m.last_eval_exact ? m.last_eval.toFixed(0) : m.last_eval.toFixed(1))
          + (m.last_eval_exact ? ' ' + t('ggs.play.solve_mark') : ''),
      })
    : undefined;
  const oppEval = m.opp_eval != null
    ? t('play.eval', { v: (m.opp_eval > 0 ? '+' : '') + m.opp_eval.toFixed(1) })
      + (m.opp_secs_used != null ? ` · ${m.opp_secs_used.toFixed(0)}${t('unit.s')}` : '')
    : undefined;

  const busyEval: Record<number, EvalInfo> | undefined =
    m.busy && m.busy_best != null
      ? {
          [m.busy_best]: {
            score: m.busy_eval ?? 0,
            src: m.busy === 'solve' ? { exact: true } : { depth: m.busy_depth },
            best: true,
          },
        }
      : undefined;

  const top = observer && m.players.length >= 2
    ? { name: m.players[0].name, rate: m.players[0].rating, color: m.players[0].color, side: 'p0' as const,
        meta: lastOf(m.players[0].color === 'black') }
    : { name: m.opp_name, rate: m.opp_rating, color: m.my_color === 'black' ? 'white' as const : 'black' as const,
        side: 'opp' as const, meta: oppEval };
  const bottom = observer && m.players.length >= 2
    ? { name: m.players[1].name, rate: m.players[1].rating, color: m.players[1].color, side: 'p1' as const,
        meta: lastOf(m.players[1].color === 'black') }
    : { name: snap.login, rate: myRate != null ? myRate.toFixed(1) : '', color: m.my_color as 'black' | 'white',
        side: 'my' as const, meta: myEval };
  const toMove = m.turn ? m.players.find((p) => p.color === m.turn)?.name
    ?? (m.turn === m.my_color ? snap.login : m.opp_name) : '';

  // Under the board: whose turn it is, and what kuroobi is doing about it.
  let state: string;
  let busy = false;
  if (m.over) {
    state = m.ended === 'finished' ? t('play.board_over', { name: lead, d: boardDiff(m, lead) })
      : m.ended === 'adjourned' ? t('ggs.state.adjourned') : t('ggs.state.aborted');
  } else {
    const parts = [toMove ? t('play.to_move', { name: toMove }) : ''];
    if (m.busy === 'think') {
      // kuroobi searches its own moves (thinking) and other people's games (analysing).
      const work = observer ? t('play.analysing') : t('ggs.play.thinking');
      parts.push(m.busy_depth > 0 ? `${work} · ${t('play.depth', { n: m.busy_depth })}` : work);
      busy = true;
    } else if (m.busy === 'solve') {
      parts.push(t('ggs.play.solving'));
      busy = true;
    } else if (snap.thinking === m.id) {
      parts.push(t('ggs.play.thinking'));
      busy = true;
    } else if (observer && m.watch_eval != null) {
      parts.push(`${t('ggs.play.analysis')} ${m.watch_eval > 0 ? '+' : ''}${m.watch_eval.toFixed(1)}`
        + (m.watch_best ? ` (${m.watch_best})` : ''));
    }
    state = parts.filter(Boolean).join(' · ');
  }

  // A small board leaves no room beside the name for the rating and the last eval; the clock matters more.
  const compact = side < 340;
  return (
    <div data-col style={{ width: side, flex: 'none', display: 'flex', flexDirection: 'column' }}>
      <div style={{
        height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
        fontSize: 'var(--fs-7)', color: 'var(--sub)', whiteSpace: 'nowrap',
      }}>
        {face !== undefined && <span style={{ fontWeight: 600, color: 'var(--text)' }}>{t('ggs.play.board_n', { n: face })}</span>}
        <span style={{ marginLeft: 'auto' }}>{black} – {white} · {t('ggs.play.moves', { n: placed(m) })}</span>
      </div>
      <PlayerRow color={top.color === 'black' ? 'b' : 'w'} name={top.name || '?'}
                 rate={!compact && top.rate ? +top.rate : undefined}
                 meta={compact ? undefined : top.meta}
                 clock={clock(m.id, top.side).text} active={!m.over && clock(m.id, top.side).active} />
      <div data-board style={{ width: side, height: side }}>
        <Board cells={m.cells as Cell[]} last={last} disabled
               evals={busyEval}
               legal={[...new Set([...m.legal, ...(busyEval ? Object.keys(busyEval).map(Number) : [])])]}
               coords={prefs.coords} grain={prefs.grain}
               flip={flipped(prefs.facing, m.my_color)} />
      </div>
      <PlayerRow color={bottom.color === 'black' ? 'b' : 'w'} name={bottom.name || '?'}
                 rate={!compact && bottom.rate ? +bottom.rate : undefined}
                 meta={compact ? undefined : bottom.meta}
                 clock={clock(m.id, bottom.side).text} active={!m.over && clock(m.id, bottom.side).active} />
      <div style={{
        display: 'flex', alignItems: 'center', height: 'var(--h-row)', whiteSpace: 'nowrap', overflow: 'hidden',
        fontSize: 'var(--fs-6)', color: busy ? 'var(--accent)' : m.over ? 'var(--text)' : 'var(--sub)',
      }}>{state}</div>
    </div>
  );
}

export function FormulaField({ label, src, onSave, preset }: {
  label: string; src: string; onSave: (s: string) => void;
  /** A ready-made rule offered below the editor; it is only filled in, and sent by "Apply". */
  preset?: { src: string; label: string; note: string };
}) {
  const [cond, setCond] = useState<Cond | null>(() => parseCond(src));
  const [raw, setRaw] = useState<string | null>(null);
  // What GGS has, written the way the editor writes it, against what is on screen now.
  const sent = (() => { const c = parseCond(src); return c ? condToSrc(c) : src.trim(); })();
  const shown = raw !== null ? raw.trim() : cond ? condToSrc(cond) : '';
  const dirty = shown !== sent && !(raw !== null && raw.trim() === src.trim());
  // In the raw editor the column's own gap already spaces it.
  const presetRow = (spaced: boolean) => preset && (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', marginTop: spaced ? 'var(--sp-2)' : 0 }}>
      <Button disabled={shown === preset.src}
              onClick={() => { if (raw !== null) setRaw(preset.src); else setCond(parseCond(preset.src)); }}>
        {preset.label}
      </Button>
      <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.5 }}>{preset.note}</span>
    </div>
  );
  if (raw !== null) {
    return (
      <SettingRow label={label}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)', width: '100%', maxWidth: 'var(--w-control)' }}>
          <TextArea mono rows={4} value={raw} onChange={setRaw}
                    placeholder={t('ggs.settings.formula_placeholder')} />
          <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
            <Button onClick={() => { setRaw(null); setCond(parseCond(raw)); }}>
              {t('ggs.settings.back_to_tree')}
            </Button>
            <span style={{ flex: 1 }} />
            {dirty && <span style={{ alignSelf: 'center', fontSize: 'var(--fs-6)', color: 'var(--gold)' }}>{t('ggs.formula.unsent')}</span>}
            <Button variant="primary" disabled={!dirty} onClick={() => { onSave(raw); setRaw(null); setCond(parseCond(raw)); }}>
              {t('ggs.apply')}
            </Button>
          </div>
          {presetRow(false)}
        </div>
      </SettingRow>
    );
  }
  return (
    <SettingRow label={label}>
      <div style={{ width: '100%', maxWidth: 'var(--w-control)' }}>
        <FormulaEditor value={cond} onChange={setCond} onClear={() => setCond(null)}
                       onSave={onSave} onRaw={setRaw} dirty={dirty} />
        {presetRow(true)}
      </div>
    </SettingRow>
  );
}

