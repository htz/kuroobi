import { useEffect, useState } from 'react';
import { ggsApi } from '../api';
import type { BoardRecord, GameResult, GgsSnapshot } from '../types';
import { fmtSecs, gtypeLabel } from '../ggs';
import { t } from '../i18n';
import { Divider, Drawer, Section, Toolbar } from '../components/layout';
import { Button, Segmented, Select } from '../components/primitives';
import { KeyValue } from '../components/layout';
import { EvalTrend, RateChart, ResultHead, ResultRow, StoneDot, type ResultBoard, type ResultEnding } from '../components/data';
import { askSheet } from '../AskSheet';
import { Blank, clockLabel, Page, RankRates, rateLabel, Rows, Sections, signed, sumBoards, Table, type TrendPoint, useWidth, useWindowHeight } from '../ggsui';
import { whenFull, whenShort } from '../when';

type Pool = '8r' | '8' | 'all';
type Period = 'all' | '7' | '30' | '90';

// GGS reuses match numbers and seq restarts with the app; the finish time tells them apart.
const rowKey = (r: GameResult) => `${r.id}#${r.seq}#${r.at ?? 0}`;

export const baseType = (base: string, raw?: string) => {
  const fromRaw = raw?.split(/\s+/).find((x) => /^s?8(r\d+)?$/.test(x));
  if (fromRaw) return fromRaw;
  return base.split('.')[0] ?? '';
};
const poolOf = (base: string, raw?: string) => {
  const kind = baseType(base, raw);
  if (!kind) return '';
  return kind.includes('r') ? '8r' : '8';
};


type Pt = TrendPoint;

/** A resignation or an adjournment decides the whole match (both boards of a synchro). */
function endingOf(r: GameResult, me: string): ResultEnding | null {
  if (r.ending === 'adjourn') return { text: t('data.result.stopped'), tone: 'sub' };
  if (r.ending !== 'resign') return null;
  return r.ending_by === me ? { text: t('data.result.resigned'), tone: 'bad' } : { text: t('data.result.opp_resigned'), tone: 'gold' };
}

/** Running out of time is per board: each board has its own clock. */
function boardOf(b: BoardRecord, me: string): ResultBoard {
  const lost = b.ending_by === me;
  return {
    diff: b.disc_diff ?? null,
    ...(b.ending === 'over_time' || b.ending === 'out_of_time'
      ? { tag: t(`data.result.${b.ending}_${lost ? 'loss' : 'win'}`), tone: lost ? 'bad' as const : 'gold' as const }
      : {}),
  };
}

/** The boards' cells of a row; a match decided as a whole leaves them empty. */
const boardsOf = (r: GameResult, me: string): ResultBoard[] => (r.ending ? [] : (r.boards ?? []).map((b) => boardOf(b, me)));

/** As the live clock reads: the main time left, or once it is gone the extension left. */
const clockLeft = (c: { main: number | null; ext: number | null }) =>
  c.main == null ? '\u2014'
    : c.main === 0 && c.ext != null ? t('ggs.clock.extension', { t: fmtSecs(c.ext) })
    : fmtSecs(c.main);

/** One board's reported values from a GGF that carries them (`B[F5/1.50/2.00]`), from `me`'s side. */
function ggfTrend(ggf: string, me: string): Pt[] {
  const black = (/PB\[([^\]]*)\]/.exec(ggf)?.[1] ?? '') === me;
  const out: Pt[] = [];
  const re = /([BW])\[([A-Ha-h][1-8]|PA|pa)(?:\/([-\d.]+))?(?:\/([\d.]+))?\]/g;
  let m: RegExpExecArray | null;
  let n = 0;
  while ((m = re.exec(ggf))) {
    if (/^pa$/i.test(m[2])) continue;
    n++;
    const mineMove = (m[1] === 'B') === black;
    const v = m[3] != null ? Number(m[3]) : null;
    const s = m[4] != null ? Number(m[4]) : null;
    out.push({ x: n, mine: mineMove ? v : null, opp: mineMove || v == null ? null : -v,
               mySecs: mineMove ? s : null, oppSecs: mineMove ? null : s });
  }
  return out;
}


/** My finished games: the rating trend or both ratings on top, the games below, the chosen one in a drawer. */
export function Results({ snap, onKifu }: {
  snap: GgsSnapshot; onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  const [pool, setPool] = useState<Pool>('8r');
  const [period, setPeriod] = useState<Period>('all');
  const [since, setSince] = useState(0);
  const [pick, setPick] = useState('');
  const [hover, setHover] = useState<number | null>(null);
  const [chartRef, chartW] = useWidth();
  // In a short window the chart gives its height to the list of games.
  const short = useWindowHeight() < 700;
  const login = snap.login;
  useEffect(() => { if (login) void ggsApi.history('').catch(() => {}); }, [login]);

  const known = new Set(snap.results.flatMap((r) => [r.archive, r.id].filter(Boolean)));
  const fromServer: GameResult[] = (snap.history[login] ?? [])
    .filter((h) => !known.has(h.id))
    .map((h) => {
      const iAmBlack = h.black === login;
      const diff = parseFloat(h.score);
      return {
        id: h.id, seq: 0, base: h.gtype, raw: h.gtype, opp: iAmBlack ? h.white : h.black,
        my_diff: Number.isFinite(diff) ? (iAmBlack ? diff : -diff) : null,
        my_rating: parseFloat(iAmBlack ? h.black_rating : h.white_rating) || null,
        at: Date.parse(h.at) / 1000 || 0,
        kifu: '', ggf: '', archive: h.id,
      } as GameResult;
    });
  const ever = snap.results.length + fromServer.length;
  const all = [...snap.results, ...fromServer]
    .filter((r) => (r.at ?? 0) >= since)
    .sort((x, y) => (y.at ?? 0) - (x.at ?? 0));
  const rows = all.filter((r) => pool === 'all' || poolOf(r.base, r.raw) === pool);
  const hasRating = rows.some((r) => r.my_rating != null);
  const rated = rows.filter((r) => r.my_rating != null).reverse();
  const sel = rows.find((r) => rowKey(r) === pick) ?? null;

  // The strip of numbers under the chart.
  const decided = rows.filter((r) => r.my_diff != null);
  const w = decided.filter((r) => (r.my_diff ?? 0) > 0).length;
  const l = decided.filter((r) => (r.my_diff ?? 0) < 0).length;
  const d = decided.length - w - l;
  const avg = decided.length ? decided.reduce((a, r) => a + (r.my_diff ?? 0), 0) / decided.length : null;
  const rates = rated.map((r) => r.my_rating as number);
  const delta = rates.length >= 2 ? rates[rates.length - 1] - rates[0] : null;
  const stats: [string, string][] = [
    [t('results.rating'), rates.length ? rates[rates.length - 1].toFixed(1) : '—'],
    [t('results.change'), delta != null ? (delta > 0 ? '+' : '') + delta.toFixed(1) : '—'],
    [t('results.wld'), `${w} / ${l} / ${d}`],
    [t('results.win_rate'), decided.length ? `${Math.round((w / decided.length) * 100)}%` : '—'],
    [t('results.avg_diff'), avg != null ? (avg > 0 ? '+' : '') + avg.toFixed(1) : '—'],
    [t('results.best'), rates.length ? Math.max(...rates).toFixed(1) : '—'],
  ];
  const mine = [...snap.my_ranks].sort((a, b) => (a.gtype === '8r' ? 0 : 1) - (b.gtype === '8r' ? 0 : 1));

  const trend = sel
    ? sumBoards((sel.boards?.length ? sel.boards.map((b) => b.ggf) : [sel.ggf]).filter(Boolean).map((g) => ggfTrend(g, login)))
    : [];
  const kind = sel ? baseType(sel.base, sel.raw) : '';
  const ggfTag = (tag: string) => (sel?.ggf ? new RegExp(tag + '\\[([^\\]]*)\\]').exec(sel.ggf)?.[1] ?? '' : '');
  const clock = ggfTag('TI') ? clockLabel(ggfTag('TI')) : sel?.raw ? /(\d+:\d\d)(?:\/\/|\s)/.exec(sel.raw)?.[1] : undefined;
  // From the record: the opponent's rating, which colour the player had, and how much the rating moved.
  const meBlack = ggfTag('PB') === login ? true : ggfTag('PW') === login ? false : null;
  // A synchro game is two boards with the colours swapped, so the player had both.
  const synchro = (sel?.boards?.length ?? 0) > 1 || /(^|\s)s8/.test(sel?.raw ?? '');
  const oppRate = meBlack == null ? '' : ggfTag(meBlack ? 'RW' : 'RB');
  const at = sel ? rated.findIndex((r) => rowKey(r) === rowKey(sel)) : -1;
  const before = at > 0 ? rated[at - 1].my_rating : null;
  const after = sel?.my_rating ?? null;
  const selBoards = (sel?.boards ?? []).filter((b) => b.my_color || b.clocks?.length);
  const colorName = (c?: string) => t(c === 'black' ? 'data.color.black' : 'data.color.white');
  const myColors = selBoards.length > 1
    ? selBoards.map((b, i) => `${t('data.result.head_board', { n: i + 1 })} ${colorName(b.my_color)}`).join(' · ')
    : selBoards.length === 1 && selBoards[0].my_color ? colorName(selBoards[0].my_color) : '';

  return (
    <>
      <Toolbar>
        <Segmented value={pool} onChange={(p) => { setPool(p); setHover(null); }}
                   options={[{ value: '8r', label: t('ggs.pool.random') }, { value: '8', label: t('ggs.pool.normal') },
                             { value: 'all', label: t('ggs.filter.all') }]} />
        <Divider />
        <Select value={period} width={128}
                onChange={(v) => { setPeriod(v as Period); setSince(v === 'all' ? 0 : Date.now() / 1000 - Number(v) * 86400); }}
                options={[['all', t('results.period_all')], ['7', t('results.period_days', { n: 7 })],
                          ['30', t('results.period_days', { n: 30 })], ['90', t('results.period_days', { n: 90 })]]} />
      </Toolbar>
      <Page row>
        {!ever ? <Blank title={t('ggs.no_records')} body={t('results.empty_help')} />
          : !all.length ? <Blank title={t('results.none_in_period')} /> : (
          <Sections>
            {pool === 'all' ? (
              <Section title={t('results.ratings')}>
                <RankRates ranks={mine} />
              </Section>
            ) : (
              <Section title={t('ggs.results.rating_trend')}>
                <div ref={chartRef} style={{ padding: '0 var(--sp-3)' }}>
                  <RateChart points={rates} width={chartW} height={short ? 96 : 168} dates={rated.map((r) => whenShort(r.at ?? 0))}
                             labels={rated.map((r) => rateLabel(whenFull(r.at ?? 0), r.opp, r.my_diff ?? null, r.my_rating as number))}
                             hover={hover} onHover={setHover} />
                </div>
                {/* Six numbers in a row; on a narrow window they wrap to a second row instead of being cut. */}
                <div style={{ display: 'flex', flexWrap: 'wrap', marginTop: 'var(--sp-2)', borderTop: '1px solid var(--border-weak)' }}>
                  {stats.map(([label, value]) => (
                    <div key={label} style={{ flex: '1 0 150px', minWidth: 0, height: 'var(--h-field)', display: 'flex', alignItems: 'center',
                                              padding: '0 var(--sp-3)', borderRight: '1px solid var(--border-weak)',
                                              borderBottom: '1px solid var(--border-weak)', whiteSpace: 'nowrap' }}>
                      <span style={{ flex: 1, minWidth: 0 }}><KeyValue label={label} value={value} /></span>
                    </div>
                  ))}
                </div>
              </Section>
            )}
            <Section title={t('ggs.results.finished')} aside={t('ggs.lobby.game_count', { n: rows.length })} grow>
              <Table>
                <ResultHead note rating={hasRating} boards />
                <Rows>
                  {rows.map((r) => (
                    <ResultRow key={rowKey(r)} opponent={r.opp}
                               picked={rowKey(r) === pick}
                               onHover={(on) => {
                                 const i = rated.findIndex((x) => rowKey(x) === rowKey(r));
                                 setHover(on ? (i < 0 ? null : i) : null);
                               }}
                               win={(r.my_diff ?? 0) > 0} draw={r.my_diff === 0} adjourned={r.my_diff == null}
                               discs={r.my_diff ?? 0} when={whenShort(r.at)}
                               note={gtypeLabel(baseType(r.base, r.raw))}
                               ending={endingOf(r, login)} boards={boardsOf(r, login)}
                               rating={hasRating ? r.my_rating : undefined}
                               onClick={() => setPick(rowKey(r))}
                               dim={!r.ggf && !r.kifu && !r.archive} />
                  ))}
                </Rows>
              </Table>
            </Section>
          </Sections>
        )}
        {sel && (
          <Drawer label={t('results.this_game')} onClose={() => setPick('')}
                  title={<span className="k-sel" style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{sel.opp}</span>}>
            <div className="k-scroll" style={{ flex: 1, minHeight: 0 }}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', padding: 'var(--sp-4) var(--sp-4) var(--sp-5)' }}>
              <ResultCard r={sel} me={login} />
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                {oppRate && <KeyValue label={t('results.opp_rating')} value={oppRate} />}
                {kind && <KeyValue label={t('ggs.field.format')} value={gtypeLabel(kind)} />}
                {clock && <KeyValue label={t('ggs.field.time_control')} value={clock} />}
                {myColors ? <KeyValue label={t('results.my_color')} value={myColors} />
                  : (synchro || meBlack != null) && <KeyValue label={t('results.my_color')} value={synchro ? t('ggs.results.both_colors') : meBlack ? t('data.color.black') : t('data.color.white')} />}
                <KeyValue label={t('results.when')} value={whenFull(sel.at ?? 0)} />
                {after != null && (
                  <KeyValue label={t('results.rating')}
                            value={after.toFixed(1) + (before != null ? ` (${after - before >= 0 ? '+' : ''}${(after - before).toFixed(1)})` : '')} />
                )}
              </div>
              {selBoards.length > 0 && <BoardTable boards={selBoards} />}
              {trend.some((p) => p.mine != null || p.opp != null) && (
                <EvalTrend points={trend} height={180} title={t('play.trend_title', { name: login })}
                           names={{ mine: login, opp: sel.opp }} />
              )}
              <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
                <Button variant="primary" size="field"
                        disabled={!sel.ggf && !sel.kifu && !sel.archive}
                        onClick={() => onKifu(`${t('ggs.play.game_with', { name: sel.opp })} · ${whenFull(sel.at ?? 0)}`, sel.ggf || sel.kifu, sel.archive)}>
                  {t('results.open_kifu')}
                </Button>
                <Button size="field" onClick={() => askSheet.open({ opponent: sel.opp, gtype: kind || undefined })}>{t('results.rematch')}</Button>
              </div>
            </div>
            </div>
          </Drawer>
        )}
      </Page>
    </>
  );
}

const HEAD: React.CSSProperties = {
  fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)',
  whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis',
};

/** Each board: its result on the board and a time mark, then one row per player with the clock it ended on. */
/** The result first and on its own: the outcome and the match's disc difference, then each board's. */
function ResultCard({ r, me }: { r: GameResult; me: string }) {
  const d = r.my_diff;
  const tone = d == null || d === 0 ? 'sub' : d > 0 ? 'ok' : 'bad';
  const word = d == null ? 'results.outcome_adjourned' : d > 0 ? 'results.outcome_win' : d < 0 ? 'results.outcome_loss' : 'results.outcome_draw';
  const ending = endingOf(r, me);
  const boards = boardsOf(r, me).filter((b) => b.diff != null);
  return (
    <div style={{
      display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)', padding: 'var(--sp-3) var(--sp-4)',
      borderRadius: 'var(--r-3)', borderLeft: `3px solid var(--${tone})`,
      background: `color-mix(in srgb, var(--${tone}) 12%, var(--card))`,
    }}>
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)', whiteSpace: 'nowrap' }}>
        <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700, color: `var(--${tone})` }}>{t(word)}</span>
        {d != null && <>
          <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>{signed(d)}</span>
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('data.result.head_discs')}</span>
        </>}
        {ending && <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-5)', color: `var(--${ending.tone})` }}>{ending.text}</span>}
      </div>
      {boards.length > 1 && (
        <div style={{ display: 'flex', gap: 'var(--sp-5)', fontSize: 'var(--fs-5)', whiteSpace: 'nowrap' }}>
          {boards.map((b, i) => (
            <span key={i} style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-2)' }}>
              <span style={{ color: 'var(--sub)' }}>{t('data.result.head_board', { n: i + 1 })}</span>
              <b style={{ fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>{signed(b.diff as number)}</b>
              {b.tag && <span style={{ color: `var(--${b.tone})`, fontSize: 'var(--fs-6)' }}>{b.tag}</span>}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function BoardTable({ boards }: { boards: BoardRecord[] }) {
  const row = (last: boolean): React.CSSProperties => ({
    height: 'var(--h-row)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)',
    borderBottom: last ? '1px solid var(--border-weak)' : undefined,
  });
  return (
    <div style={{ display: 'flex', flexDirection: 'column' }}>
      <div style={{ height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', borderBottom: '1px solid var(--border)' }}>
        <span style={{ ...HEAD, width: 48, flex: 'none' }}>{t('results.head_board')}</span>
        <span style={{ ...HEAD, flex: 1, minWidth: 0 }}>{t('results.head_player')}</span>
        <span style={{ ...HEAD, width: 112, flex: 'none', textAlign: 'right' }}>{t('results.head_left')}</span>
      </div>
      {boards.map((b, i) => {
        const players = b.clocks?.length ? b.clocks : [null];
        return players.map((c, j) => (
          <div key={b.id + j} style={row(j === players.length - 1)}>
            <span style={{ width: 48, flex: 'none', color: 'var(--sub)' }}>{j === 0 ? t('data.result.head_board', { n: i + 1 }) : ''}</span>
            <span style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-1)', whiteSpace: 'nowrap', overflow: 'hidden' }}>
              {c && <><StoneDot color={c.color === 'black' ? 'b' : 'w'} size={12} />
                <span className="k-sel" style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{c.name}</span></>}
            </span>
            <span style={{ width: 112, flex: 'none', textAlign: 'right', fontSize: 'var(--fs-6)', fontVariantNumeric: 'tabular-nums' }}>
              {c ? clockLeft(c) : ''}
            </span>
          </div>
        ));
      })}
    </div>
  );
}
