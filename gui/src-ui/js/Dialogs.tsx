import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { api, emitApp, onApp, type HashView, type KifuFrame, type ThreadsView } from './api';
import { TATAMI, type Prefs, type Theme } from './prefs';
import { t, useLang, tErr } from './i18n';
import { Modal, Note, Overlay, Section } from './components/layout';
import { Button, Segmented, Select, TextField } from './components/primitives';
import { Icon } from './components/Icons';
import { Board } from './components/board';
import { StoneDot } from './components/data';
import { GgsSettings } from './GgsScreens';
import type { GgsSnapshot } from './types';


export function Confirm({ title, body, ok = 'OK', danger, onOk, onCancel }: {
  title: string; body?: React.ReactNode; ok?: string; danger?: boolean;
  onOk: () => void; onCancel: () => void;
}) {
  return (
    <Overlay onClose={onCancel}>
      <Modal title={title} body={body} onClose={onCancel} actions={<>
        <span style={{ marginLeft: 'auto' }} />
        <Button size="field" onClick={onCancel}>{t('dialog.cancel')}</Button>
        <Button size="field" variant={danger ? 'danger' : 'primary'} onClick={onOk}>{ok}</Button>
      </>} />
    </Overlay>
  );
}

export function PickOne({ title, body, options, ok = t('dialog.open'), onOk, onCancel }: {
  title: string; body?: React.ReactNode; options: [string, string][];
  ok?: string; onOk: (v: string) => void; onCancel: () => void;
}) {
  const [v, setV] = useState(options[0]?.[0] ?? '');
  return (
    <Overlay onClose={onCancel}>
      <Modal title={title} onClose={onCancel}
             body={<div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
               {body}
               <Select value={v} options={options} onChange={setV} />
             </div>}
             actions={<>
               <span style={{ marginLeft: 'auto' }} />
               <Button size="field" onClick={onCancel}>{t('dialog.cancel')}</Button>
               <Button size="field" variant="primary" disabled={!v} onClick={() => onOk(v)}>{ok}</Button>
             </>} />
    </Overlay>
  );
}

export function PasteKifu({ onLoad, onFile, onCancel }: {
  onLoad: (text: string) => void; onFile: () => void; onCancel: () => void;
}) {
  const [text, setText] = useState('');
  const [peek, setPeek] = useState<{ frames: KifuFrame[]; err: string } | null>(null);
  useEffect(() => {
    const src = text.trim();
    let alive = true;
    if (!src) {
      const clear = setTimeout(() => { if (alive) setPeek(null); }, 0);
      return () => { alive = false; clearTimeout(clear); };
    }
    const id = setTimeout(() => {
      void api.previewKifu(src)
        .then((frames) => { if (alive) setPeek({ frames, err: '' }); })
        .catch((e) => { if (alive) setPeek({ frames: [], err: tErr(e) }); });
    }, 250);
    return () => { alive = false; clearTimeout(id); };
  }, [text]);

  const ok = !!peek && !peek.err && peek.frames.length > 1;
  const last = ok ? peek!.frames[peek!.frames.length - 1] : null;
  return (
    <Overlay onClose={onCancel}>
      <Modal title={t('dialog.kifu.title')} width="var(--w-modal-wide)" onClose={onCancel}
             sub={t('dialog.kifu.sub')}
             actions={<>
               <Button size="field" onClick={onFile}>{t('dialog.kifu.from_file')}</Button>
               <span style={{ marginLeft: 'auto' }} />
               <Button size="field" onClick={onCancel}>{t('dialog.cancel')}</Button>
               <Button size="field" variant="primary" disabled={!ok}
                       onClick={() => onLoad(text)}>{t('dialog.kifu.load')}</Button>
             </>}>
        <textarea value={text} onChange={(e) => setText(e.target.value)}
          className="k-input"
          style={{
            height: 110, resize: 'none', padding: 'var(--sp-3)', borderRadius: 'var(--r-3)',
            background: 'var(--bg)', border: '1px solid var(--border)',
            fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', lineHeight: 1.6,
          }} />
        <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'center', minHeight: 96 }}>
          <div style={{
            width: 96, height: 96, flex: 'none',
            borderRadius: 'var(--r-2)', overflow: 'hidden',
          }}>
            {last && <Board cells={last.cells as (0 | 1 | 2)[]} last={last.last} coords={false} grain={false} />}
          </div>
          <div style={{
            display: 'flex', flexDirection: 'column', gap: 'var(--sp-0)',
            fontSize: 'var(--fs-5)', color: peek?.err ? 'var(--bad)' : 'var(--text)',
          }}>
            <span style={{
              fontSize: 'var(--fs-7)', fontWeight: 600, letterSpacing: '.08em', color: 'var(--sub)',
            }}>{t('dialog.kifu.preview')}</span>
            {!peek && <span style={{ color: 'var(--sub)' }}>{t('dialog.kifu.preview_hint')}</span>}
            {peek?.err && <span>{peek.err}</span>}
            {last && <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2h)' }}>
                <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
                  <StoneDot color="b" size={13} /><b style={{ fontWeight: 600 }}>{last.black}</b>
                </span>
                <span style={{ color: 'var(--sub)' }}>—</span>
                <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
                  <b style={{ fontWeight: 600 }}>{last.white}</b><StoneDot color="w" size={13} />
                </span>
              </div>
              <span style={{ color: 'var(--sub)' }}>
                {t(last.black + last.white === 64
                  ? 'dialog.kifu.summary_final' : 'dialog.kifu.summary_last',
                   { n: peek!.frames.length - 1 })}
              </span>
            </>}
            {peek && !peek.err && !ok && <span style={{ color: 'var(--sub)' }}>{t('dialog.kifu.no_moves')}</span>}
          </div>
        </div>
      </Modal>
    </Overlay>
  );
}


const kinds = (): [string, string][] => [
  ['nnue', t('settings.file.nnue')],
  ['weights', t('settings.file.weights')],
  ['book', t('settings.file.book')],
];

function fmtSize(n: number): string {
  if (n <= 0) return '';
  if (n < 1024) return n + ' B';
  if (n < 1024 * 1024) return Math.round(n / 1024) + ' KB';
  return (n / 1024 / 1024).toFixed(1) + ' MB';
}

const notFound = (kind: string): string =>
  kind === 'nnue' ? t('settings.file.nnue_missing')
  : kind === 'book' ? t('settings.file.book_missing')
  : t('settings.file.weights_missing');

export function Settings({ prefs, setPref, ggs, initialTab, onClose }: {
  prefs: Prefs; setPref: <K extends keyof Prefs>(k: K, v: Prefs[K]) => void;
  ggs?: GgsSnapshot | null;
  initialTab?: 'engine' | 'view' | 'ggs';
  onClose?: () => void;
}) {
  useLang();
  const [tab, setTab] = useState<'engine' | 'view' | 'ggs'>(initialTab ?? 'engine');
  useEffect(() => {
    void api.autoplay().then((v) => {
      const [, want, arg] = (v ?? '').split(':');
      if (want === 'engine' || want === 'view' || want === 'ggs') setTab(want);
      if (arg === 'light' || arg === 'dark' || arg === 'os') {
        window.setTimeout(() => setPref('theme', arg), 5000);
      }
    }).catch(() => { /* outside Tauri it simply does nothing */ });
  }, []);
  const [reset, setReset] = useState(false);
  const [status, setStatus] = useState<[string, string, boolean, number, string][]>([]);
  const [th, setTh] = useState<ThreadsView | null>(null);
  const [calib, setCalib] = useState(false);
  const [hash, setHash] = useState<HashView | null>(null);

  const load = useCallback(async () => {
    try { setStatus(await api.resourceStatus()); } catch { /* engine not initialized yet */ }
  }, []);

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [st, threads, h] = await Promise.all([
          api.resourceStatus(), api.localThreads(), api.hashSizes(),
        ]);
        if (!alive) return;
        setStatus(st);
        setTh(threads);
        setHash(h);
      } catch { /* engine not initialized yet */ }
    })();
    return () => { alive = false; };
  }, []);

  useEffect(() => {
    let alive = true;
    const off = onApp('resources-changed', () => {
      void api.localThreads().then((v) => { if (alive) setTh(v); }).catch(() => {});
    });
    return () => { alive = false; void off.then((f) => f()); };
  }, []);

  const byName = new Map(status.map(([n, p, ok, size, kind]) => [n, { p, ok, size, kind }]));
  const change = async (kind: string, path: string | null) => {
    await api.setResource(kind, path);
    await load();
    emitApp('resources-changed');
  };
  const setThreads = async (n: number | null) => {
    try {
      await api.setLocalThreads(n);
      setTh(await api.localThreads());
    } catch { /* leave it as is when the save fails */ }
  };

  return (
    <Modal title={t('settings.title')} width="560px" onClose={onClose} scroll
           band={<>
               <div style={{
                 flex: 1, display: 'flex', alignItems: 'center',
                 justifyContent: 'center', gap: 'var(--sp-1)',
               }}>
                 {tabs().map(([v, label]) => {
                   const on = tab === v;
                   return (
                     <button key={v} type="button" className={'k-press' + (on ? ' k-on' : '')}
                             onClick={() => setTab(v)} aria-pressed={on}
                             style={{
                               height: 'var(--h-ctrl)', padding: '0 14px', border: 0,
                               borderRadius: 'var(--r-2)', fontSize: 'var(--fs-5)',
                               background: on ? 'var(--accent-dim)' : 'transparent',
                               color: on ? 'var(--on-accent)' : 'var(--sub)',
                               fontWeight: on ? 600 : 400,
                             }}>{label}</button>
                   );
                 })}
               </div>
           </>}>
      <div className="k-settings" style={{ display: 'flex', flexDirection: 'column' }}>

        {tab === 'view' && <ViewSettings prefs={prefs} setPref={setPref} />}

        {tab === 'ggs' && (ggs
          ? <GgsSettings snap={ggs} />
          : (
            <Section title="GGS">
              <Note>{t('settings.ggs.unloaded')}</Note>
            </Section>
          )
        )}


        {tab === 'engine' && <>
        <Section title={t('settings.files.title')}>
          {kinds().map(([kind, label]) => {
            const info = byName.get(kind);
            return (
              <div key={kind} style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                <Row2 label={label}>
                  <TextField value={info?.p ?? ''} placeholder={t('settings.file.unset')} invalid={!info?.ok} />
                  <Button size="field" onClick={async () => {
                    const p = await api.pickResource(kind);
                    if (p) await change(kind, p);
                  }}>{t('settings.file.choose')}</Button>
                </Row2>
                <div style={{
                  marginLeft: 'calc(var(--w-label) + var(--sp-3))',
                  display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                  fontSize: 'var(--fs-6)', color: info?.ok ? 'var(--ok)' : 'var(--bad)',
                }}>
                  <Icon name={info?.ok ? 'check' : 'alert'} size={12} />
                  {info?.ok
                    ? [t('settings.file.loaded'), fmtSize(info.size), info.kind].filter(Boolean).join(' · ')
                    : notFound(kind)}
                </div>
              </div>
            );
          })}
        </Section>

          <Section title={t('settings.clock.title')}>
            <Row2 label={t('settings.clock.label')}>
              <Select width={140} value={String(prefs.clockSecs)}
                      onChange={(v) => setPref('clockSecs', +v)}
                      options={[['0', t('settings.clock.none')],
                                ...[5, 10, 15, 20, 30].map((m) =>
                                  [String(m * 60), t('settings.clock.minutes', { n: m })] as [string, string])]} />
            </Row2>
            <Note>{t('settings.clock.note')}</Note>
          </Section>
        {th && (
          <Section title={t('settings.threads.title')}>
            <Row2 label={t('settings.threads.label')}>
              <Select width={140} value={String(th.set ?? th.auto)}
                      onChange={(v) => void setThreads(+v === th.auto ? null : +v)}
                      options={Array.from({ length: th.auto * 2 }, (_, i) => {
                        const n = i + 1;
                        return [String(n), n === th.auto ? t('settings.threads.auto', { n }) : String(n)] as [string, string];
                      })} />
            </Row2>
            <Note>{t('settings.threads.note')}</Note>
          </Section>
        )}
        {th && (
          <Section title={t('settings.nps.title')}>
            <Row2 label={t('settings.nps.label')}>
              <span style={{
                width: 200, flex: 'none',
                fontSize: 'var(--fs-5)', fontVariantNumeric: 'tabular-nums',
                color: th.nps != null ? 'var(--text)' : 'var(--sub)',
              }}>
                {th.nps == null
                  ? t('settings.nps.unmeasured')
                  : t('settings.nps.value', { n: (th.nps / 1e6).toFixed(1) })}
              </span>
              <Button size="field" disabled={calib}
                      onClick={() => void (async () => {
                        setCalib(true);
                        try { setTh(await api.calibrateNps()); } catch { /* keep the old value when it cannot be measured */ }
                        setCalib(false);
                      })()}>{calib ? t('settings.nps.measuring') : t('settings.nps.remeasure')}</Button>
            </Row2>
            <Note>{t('settings.nps.note')}</Note>
          </Section>
        )}
        {hash && (
          <Section title={t('settings.hash.title')}>
            <Row2 label={t('settings.hash.mid')}>
                <Select width={140} value={String(hash.mid)}
                        onChange={(v) => void (async () => {
                          try { setHash(await api.setHashSizes(+v, hash.end)); } catch { /* the save failed */ }
                        })()}
                        options={Array.from({ length: hash.max - hash.min + 1 }, (_, i) => {
                          const b = hash.min + i;
                          const size = fmtSize(2 ** (b + 4));
                          return [String(b), b === 22 ? t('settings.hash.default', { size }) : size] as [string, string];
                        })} />
            </Row2>
            <Row2 label={t('settings.hash.end')}>
              <Select width={140} value={String(hash.end)}
                      onChange={(v) => void (async () => {
                        try { setHash(await api.setHashSizes(hash.mid, +v)); } catch { /* the save failed */ }
                      })()}
                      options={Array.from({ length: hash.max - hash.min + 1 }, (_, i) => {
                        const b = hash.min + i;
                        const size = fmtSize(2 ** b * 24);
                        return [String(b), b === 24 ? t('settings.hash.default', { size }) : size] as [string, string];
                      })} />
            </Row2>
            <Note>{t('settings.hash.note')}</Note>
          </Section>
        )}
        </>}

        {tab === 'engine' && (
          <div style={{
            display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
            margin: '0 var(--sp-3)', paddingTop: 'var(--sp-4)',
            borderTop: '1px solid var(--border-weak)',
          }}>
            <Button size="field" onClick={() => setReset(true)}>{t('settings.reset.button')}</Button>
          </div>
        )}
      </div>
      {reset && (
        <Confirm title={t('settings.reset.title')}
                 body={t('settings.reset.body')}
                 ok={t('settings.reset.ok')}
                 onCancel={() => setReset(false)}
                 onOk={() => {
                   setReset(false);
                   void (async () => {
                     for (const [kind] of kinds()) await api.setResource(kind, null);
                     await setThreads(null);
                     await load();
                     emitApp('resources-changed');
                   })();
                 }} />
      )}
    </Modal>
  );
}

const tabs = (): ['engine' | 'view' | 'ggs', string][] => [
  ['engine', t('settings.tab.engine')],
  ['view', t('settings.tab.view')],
  ['ggs', t('settings.tab.ggs')],
];

const themes = (): [Theme, string][] => [
  ['os', t('settings.theme.os')],
  ['dark', t('settings.theme.dark')],
  ['light', t('settings.theme.light')],
];

const languages = (): [Prefs['lang'], string][] => [
  ['auto', t('settings.language.auto')],
  ['en', t('settings.language.en')],
  ['ja', t('settings.language.ja')],
];

function ThemeSwatch({ kind }: { kind: Theme }) {
  const dark = '#16191d', light = '#faf8f3';
  return (
    <span style={{
      display: 'block', height: 38, borderRadius: 'var(--r-1)', width: '100%',
      boxShadow: kind === 'dark' ? 'inset 0 0 0 1px var(--border)' : undefined,
      background: kind === 'dark' ? dark : kind === 'light' ? light
        : `linear-gradient(135deg, ${dark} 50%, ${light} 50%)`,
    }} />
  );
}

function Row2({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', minHeight: 'var(--h-field)' }}>
      <span style={{ width: 'var(--w-label)', flex: 'none', textAlign: 'right',
                     fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>{label}</span>
      {children}
    </div>
  );
}

function ViewSettings({ prefs, setPref }: {
  prefs: Prefs;
  setPref: <K extends keyof Prefs>(k: K, v: Prefs[K]) => void;
}) {
  return (
    <>
      <Section title={t('settings.language.label')}>
        <span style={{ alignSelf: 'flex-start' }}>
          <Select value={prefs.lang} onChange={(v) => setPref('lang', v as Prefs['lang'])}
                  options={languages()} width={200} />
        </span>
      </Section>
      <Section title={t('settings.theme.title')}>
        <div style={{ display: 'flex', gap: 'var(--sp-2h)' }}>
          {themes().map(([v, label]) => {
            const on = prefs.theme === v;
            return (
              <button key={v} type="button" className="k-press" onClick={() => setPref('theme', v)}
                aria-pressed={on}
                style={{
                  flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column',
                  gap: 'var(--sp-2)', padding: 'var(--sp-2)', borderRadius: 'var(--r-3)',
                  background: on ? 'var(--panel)' : 'transparent', fontSize: 'var(--fs-6)',
                  border: '1px solid ' + (on ? 'var(--accent)' : 'var(--border)'),
                  color: on ? 'var(--text)' : 'var(--sub)', fontWeight: 400,
                }}>
                <ThemeSwatch kind={v} />
                {label}
              </button>
            );
          })}
        </div>
      </Section>
      <Section title={t('settings.board.title')}>
        <Row2 label={t('settings.board.tatami')}>
          <span style={{ display: 'flex', gap: 'var(--sp-2)' }}>
            {TATAMI.map((mat, i) => {
              const on = prefs.tatami === i;
              const label = t(mat.labelKey);
              return (
                <button key={mat.labelKey} type="button" className="k-press"
                        onClick={() => setPref('tatami', i as Prefs['tatami'])}
                        title={label} aria-label={label} aria-pressed={on}
                        style={{
                          width: 28, height: 28, borderRadius: 'var(--r-2)', padding: 0,
                          border: 0, background: mat.board,
                          boxShadow: on
                            ? '0 0 0 2px var(--accent), inset 0 0 0 1px var(--border)'
                            : 'inset 0 0 0 1px var(--border)',
                        }} />
              );
            })}
          </span>
        </Row2>
        <Row2 label={t('settings.board.coords')}>
          <Segmented value={prefs.coords ? 'on' : 'off'} onChange={(v) => setPref('coords', v === 'on')}
                     options={[{ value: 'on', label: t('settings.show') },
                               { value: 'off', label: t('settings.hide') }]} />
        </Row2>
        <Row2 label={t('settings.board.grain')}>
          <Segmented value={prefs.grain ? 'on' : 'off'} onChange={(v) => setPref('grain', v === 'on')}
                     options={[{ value: 'on', label: t('settings.show') },
                               { value: 'off', label: t('settings.hide') }]} />
        </Row2>
        <Row2 label={t('settings.board.flip')}>
          <Segmented value={String(prefs.flipMs)} onChange={(v) => setPref('flipMs', +v as Prefs['flipMs'])}
                     options={[{ value: '0', label: t('settings.flip.none') },
                               { value: '120', label: t('settings.flip.fast') },
                               { value: '240', label: t('settings.flip.slow') }]} />
        </Row2>
        <Row2 label={t('settings.board.facing')}>
          <Segmented value={prefs.facing} onChange={(v) => setPref('facing', v)} options={[
            { value: 'black', label: t('settings.facing.black') },
            { value: 'white', label: t('settings.facing.white') },
            { value: 'auto', label: t('settings.facing.auto') },
          ]} />
        </Row2>
      </Section>
      <Section title={t('settings.numbers.title')}>
        <Row2 label={t('settings.numbers.decimals')}>
          <Segmented value={String(prefs.decimals)}
                     onChange={(v) => setPref('decimals', +v as Prefs['decimals'])}
                     options={[{ value: '0', label: '0' },
                               { value: '1', label: '1' },
                               { value: '2', label: '2' }]} />
        </Row2>
      </Section>
    </>
  );
}
