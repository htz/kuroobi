import { useEffect, useRef, useState } from 'react';
import { ggsApi, type ActivityView, type SavedServer } from './api';
import type { GgsSnapshot } from './types';
import { t, dateLocale } from './i18n';
import { Caption, popoverStyle, StatusBar, useDismiss } from './components/layout';
import { Dot } from './components/primitives';
import { askSheet } from './AskSheet';
import { serverName } from './SettingsScreen';
import { useNow } from './ggsui';
import { openNewLogin } from './ggs/Login';

/** One load gauge: a thin bar coloured by how close it is to its limit, then the number. */
function Load({ label, ratio, value, unit }: { label: string; ratio: number; value: string; unit: string }) {
  const tone = ratio >= 0.9 ? 'var(--bad)' : ratio >= 0.7 ? 'var(--gold)' : 'var(--ok)';
  return (
    <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-1)', whiteSpace: 'nowrap' }}>
      {label}
      <span aria-hidden style={{
        display: 'inline-block', width: 32, height: 4, borderRadius: 'var(--r-pill)',
        background: 'var(--track)', overflow: 'hidden',
      }}>
        <span style={{ display: 'block', height: '100%', width: `${Math.round(Math.min(1, ratio) * 100)}%`, background: tone }} />
      </span>
      <b style={{ color: ratio >= 0.7 ? tone : 'var(--text)', fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>{value}</b>
      {unit}
    </span>
  );
}

const Item = ({ children }: { children: React.ReactNode }) => (
  <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-1)', whiteSpace: 'nowrap' }}>{children}</span>
);
const B = ({ children }: { children: React.ReactNode }) => (
  <b style={{ color: 'var(--text)', fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>{children}</b>
);

/**
 * The bar shows the whole app, never one screen. Left: what is running now (it appears and goes away).
 * Right: what is always there — CPU, memory and the GGS connection.
 */
export function GlobalStatus({ snap, cpu, analyzing, thinking, thinkSecs, nodes, nps, servers }: {
  snap: GgsSnapshot | null; cpu: ActivityView | null; analyzing: { done: number; total: number } | null;
  thinking: boolean; thinkSecs: number; nodes: number; nps: number; servers: SavedServer[];
}) {
  const online = snap?.conn === 'online';
  const auto = online && snap!.standby.enabled;
  // A GGS game's search reads the same as a local one: seconds so far, then the last search's nodes and speed.
  const ggsMatch = online ? snap!.matches.find((m) => m.id === snap!.thinking) : undefined;
  const ggsThinking = !thinking && !!ggsMatch && ggsMatch.think_since_ms > 0;
  const now = useNow(ggsThinking, 200);
  const ggsSecs = ggsThinking ? Math.max(0, (now - ggsMatch!.think_since_ms) / 1000) : 0;
  const ggsLast = online && !thinking && cpu?.ggs_match ? snap!.last_search : null;
  const shownNodes = ggsLast ? ggsLast.nodes : nodes;
  const shownNps = ggsLast ? (ggsLast.secs > 0 ? ggsLast.nodes / ggsLast.secs : 0) : nps;
  return (
    <StatusBar
      left={<>
        {auto && (
          <button type="button" className="k-press" onClick={() => askSheet.open({ repeat: true })}
                  title={t('status.auto_title')}
                  style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-1)', height: 'var(--h-chip)',
                           padding: '0 var(--sp-1)', border: 0, borderRadius: 'var(--r-1)', background: 'transparent',
                           color: 'inherit', font: 'inherit' }}>
            <Dot tone="ok" />{t('status.auto')}
          </button>
        )}
        {analyzing && <Item><Dot tone="accent" />{t('app.status.analyzing')} <B>{analyzing.done}</B> / {analyzing.total}</Item>}
        {(thinking || ggsThinking) && (
          <Item><Dot tone="accent" />{t('app.status.thinking')} <B>{(thinking ? thinkSecs : ggsSecs).toFixed(1)}</B> {t('status.secs')}</Item>
        )}
        {cpu?.learn && (
          <Item><Dot tone="gold" />{t('status.learning')} <B>{cpu.learn[0].toLocaleString()}</B> / {cpu.learn[1].toLocaleString()}</Item>
        )}
        {/* Details last: they are the first to go when the bar is narrow. */}
        {shownNodes > 0 && <Item>{t('status.nodes')} <B>{fmtNodes(shownNodes)}</B></Item>}
        {shownNps > 0 && <Item>{t('status.speed')} <B>{(shownNps / 1e6).toFixed(1)}</B> {t('unit.mnps')}</Item>}
      </>}
      right={<>
        {cpu && <Load label="CPU" ratio={cpu.cores > 0 ? cpu.cpu / (cpu.cores * 100) : 0}
                      value={String(Math.round(cpu.cpu))} unit="%" />}
        {cpu && <Load label={t('app.meter.memory')} ratio={cpu.mem_total > 0 ? cpu.mem / cpu.mem_total : 0}
                      value={(cpu.mem / 1e9).toFixed(1)} unit={' ' + t('unit.gb')} />}
        <GgsConn snap={snap} servers={servers} />
      </>} />
  );
}

/** "GGS 接続中 (alice · ローカル)"; clicking it opens the account switcher above the bar. */
function GgsConn({ snap, servers }: { snap: GgsSnapshot | null; servers: SavedServer[] }) {
  const [open, setOpen] = useState(false);
  const [saved, setSaved] = useState<{ server: string; login: string }[]>([]);
  const box = useRef<HTMLSpanElement>(null);
  const conn = snap?.conn ?? 'disconnected';
  const online = conn === 'online';
  // A dropped connection coming back is a reconnect; a first login, or one after logging out, is not.
  const [wasOnline, setWasOnline] = useState(false);
  if (online && !wasOnline) setWasOnline(true);
  if (conn === 'disconnected' && wasOnline) setWasOnline(false);
  useEffect(() => {
    if (open) void ggsApi.accounts().then(setSaved).catch(() => {});
  }, [open]);
  useDismiss(open, () => setOpen(false), box);
  const state = online ? t('app.ggs.connected')
    : conn === 'disconnected' ? t('app.ggs.offline') : wasOnline ? t('status.reconnecting') : t('status.connecting');
  const who = online ? ` (${snap!.login} · ${serverName(servers, snap!.server)})` : '';
  const groups = servers
    .map((s) => ({ s, list: saved.filter((a) => a.server === s.addr) }))
    .filter((g) => g.list.length);
  return (
    <span ref={box} style={{ position: 'relative', display: 'flex' }}>
      <button type="button" className="k-press" aria-expanded={open} disabled={!online}
              onClick={() => setOpen(!open)}
              style={{ height: 'var(--h-chip)', padding: '0 var(--sp-2)', border: 0, borderRadius: 'var(--r-1)',
                       background: open ? 'var(--hover)' : 'transparent', color: 'var(--sub)', font: 'inherit',
                       whiteSpace: 'nowrap', cursor: online ? 'pointer' : 'default' }}>
        GGS <b style={{ color: online ? 'var(--text)' : conn === 'disconnected' ? 'var(--sub)' : 'var(--gold)', fontWeight: 600 }}>{state}</b>{who}
      </button>
      {open && online && (
        <div role="menu" style={{
          position: 'absolute', zIndex: 50, right: 0, bottom: 'calc(100% + var(--sp-1))', width: 'var(--w-popover)',
          ...popoverStyle, whiteSpace: 'nowrap',
        }}>
          <div style={{ height: 'var(--h-field)', display: 'flex', alignItems: 'center', padding: '0 var(--sp-3)' }}>
            <Caption>{t('app.ggs.switch_account')}</Caption>
          </div>
          <div className="k-scroll" style={{ maxHeight: 240 }}>
            {groups.map(({ s, list }) => (
              <div key={s.addr} style={{ display: 'flex', flexDirection: 'column' }}>
                <div style={{ height: 'var(--h-head)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                              padding: '0 var(--sp-3)', fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>
                  <b style={{ fontWeight: 600 }}>{s.name}</b><span style={{ fontFamily: 'var(--ff-mono)' }}>{s.addr}</span>
                </div>
                {list.map((a) => {
                  const cur = a.server === snap!.server && a.login === snap!.login;
                  return (
                    <button key={a.server + '|' + a.login} type="button" role="menuitem" className="k-row"
                            onClick={() => {
                              setOpen(false);
                              if (!cur) void ggsApi.switchAccount(a.server, a.login);
                            }}
                            style={{ height: 'var(--h-field)', display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                                     padding: '0 var(--sp-3)', border: 0, textAlign: 'left', background: 'transparent',
                                     color: 'var(--text)', fontSize: 'var(--fs-5)' }}>
                      <span style={{ width: 14, color: 'var(--accent)' }}>{cur ? '✓' : ''}</span>
                      <b style={{ fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis' }}>{a.login}</b>
                    </button>
                  );
                })}
              </div>
            ))}
          </div>
          <button type="button" role="menuitem" className="k-row"
                  onClick={() => { setOpen(false); openNewLogin(); void ggsApi.disconnect(); }}
                  style={{ height: 'var(--h-field)', padding: '0 var(--sp-3)', paddingLeft: 'calc(var(--sp-3) + 14px + var(--sp-2))',
                           border: 0, borderTop: '1px solid var(--border)', textAlign: 'left', background: 'transparent',
                           color: 'var(--text)', fontSize: 'var(--fs-5)' }}>
            {t('status.new_login')}
          </button>
          <button type="button" role="menuitem" className="k-row"
                  onClick={() => { setOpen(false); void ggsApi.disconnect(); }}
                  style={{ height: 'var(--h-field)', padding: '0 var(--sp-3)', paddingLeft: 'calc(var(--sp-3) + 14px + var(--sp-2))',
                           border: 0, textAlign: 'left', background: 'transparent',
                           color: 'var(--bad)', fontSize: 'var(--fs-5)' }}>
            {t('ggs.settings.logout')}
          </button>
        </div>
      )}
    </span>
  );
}

/** "5,870 万" in Japanese, "58.7M" in English. */
const fmtNodes = (n: number): string => {
  if (dateLocale() === 'ja-JP') {
    return n >= 1e8 ? (n / 1e8).toFixed(2) + ' 億' : n >= 1e4 ? Math.round(n / 1e4).toLocaleString() + ' 万' : n.toLocaleString();
  }
  return n >= 1e9 ? (n / 1e9).toFixed(1) + 'G' : n >= 1e6 ? (n / 1e6).toFixed(1) + 'M'
    : n >= 1e3 ? (n / 1e3).toFixed(0) + 'k' : String(n);
};

