import { useEffect, useState } from 'react';
import { ggsApi } from '../api';
import type { GameResult, GgsSnapshot } from '../types';
import { gtypeLabel } from '../ggs';
import { t } from '../i18n';
import { Divider, Dock, Section, Toolbar } from '../components/layout';
import { Button, Segmented, Select } from '../components/primitives';
import { Tag } from '../components/ggs';
import { KeyValue } from '../components/layout';
import { EvalTrend, RateChart, ResultHead, ResultRow } from '../components/data';
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


/** My finished games: the rating trend or both ratings on top, the games below, the chosen one in the dock. */
export function Results({ snap, onKifu }: {
  snap: GgsSnapshot; onKifu: (title: string, kifu: string, archive?: string) => void;
}) {
  const [pool, setPool] = useState<Pool>('8r');
  const [period, setPeriod] = useState<Period>('all');
  const [since, setSince] = useState(0);
  const [pick, setPick] = useState('');
  const [hover, setHover] = useState<number | null>(null);
  const [dock, setDock] = useState(false);
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

  return (
    <>
      <Toolbar dock={{ open: dock, onToggle: () => setDock((o) => !o) }}>
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
        {!all.length ? <Blank title={t('ggs.no_records')} body={t('results.empty_help')} /> : (
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
                <ResultHead note={pool === 'all'} rating={hasRating} />
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
                               note={pool === 'all' ? gtypeLabel(baseType(r.base, r.raw)) : undefined}
                               rating={hasRating ? r.my_rating : undefined}
                               onClick={() => { setPick(rowKey(r)); setDock(true); }}
                               dim={!r.ggf && !r.kifu && !r.archive} />
                  ))}
                </Rows>
              </Table>
            </Section>
          </Sections>
        )}
        <Dock push tabs={[{ id: 'game', label: t('results.this_game') }]} active="game" open={dock}>
          {!sel ? <Blank title={t('results.pick')} /> : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)', padding: 'var(--sp-4) var(--sp-4) var(--sp-5)' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', minWidth: 0, whiteSpace: 'nowrap' }}>
                <Tag tone={sel.my_diff == null ? 'sub' : sel.my_diff > 0 ? 'ok' : sel.my_diff < 0 ? 'bad' : 'sub'}>
                  {sel.my_diff == null ? t('results.tag_adjourned') : sel.my_diff > 0 ? t('ggs.stat.wins') : sel.my_diff < 0 ? t('ggs.stat.losses') : t('ggs.stat.draws')}
                </Tag>
                <span className="k-sel" style={{ fontSize: 'var(--fs-2)', fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis' }}>{sel.opp}</span>
                {sel.my_diff != null && (
                  <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-1)', fontWeight: 600,
                                 color: sel.my_diff > 0 ? 'var(--ok)' : sel.my_diff < 0 ? 'var(--bad)' : 'var(--text)' }}>
                    {signed(sel.my_diff)}
                  </span>
                )}
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                {oppRate && <KeyValue label={t('results.opp_rating')} value={oppRate} />}
                {kind && <KeyValue label={t('ggs.field.format')} value={gtypeLabel(kind)} />}
                {clock && <KeyValue label={t('ggs.field.time_control')} value={clock} />}
                {(synchro || meBlack != null) && <KeyValue label={t('ggs.field.turn')} value={synchro ? t('ggs.results.both_colors') : meBlack ? t('data.color.black') : t('data.color.white')} />}
                <KeyValue label={t('results.when')} value={whenFull(sel.at ?? 0)} />
                {after != null && (
                  <KeyValue label={t('results.rating')}
                            value={after.toFixed(1) + (before != null ? ` (${after - before >= 0 ? '+' : ''}${(after - before).toFixed(1)})` : '')} />
                )}
              </div>
              {trend.some((p) => p.mine != null || p.opp != null) && (
                <EvalTrend points={trend} height={150} title={t('play.trend_title', { name: login })}
                           names={{ mine: login, opp: sel.opp }} />
              )}
              <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
                <span style={{ flex: 1, minWidth: 0, display: 'flex' }}>
                  <Button variant="primary" size="field" className="k-wide"
                          disabled={!sel.ggf && !sel.kifu && !sel.archive}
                          onClick={() => onKifu(`${t('ggs.play.game_with', { name: sel.opp })} · ${whenFull(sel.at ?? 0)}`, sel.ggf || sel.kifu, sel.archive)}>
                    {t('results.open_kifu')}
                  </Button>
                </span>
                <Button size="field" onClick={() => askSheet.open({ opponent: sel.opp, gtype: kind || undefined })}>{t('results.rematch')}</Button>
              </div>
            </div>
          )}
        </Dock>
      </Page>
    </>
  );
}
