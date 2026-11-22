import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { api, emitApp, onApp, type ExportFormat, type HashView, type KifuPreview, type ThreadsView } from './api';
import { TATAMI, type Prefs, type Theme } from './prefs';
import { t, useLang, tErr } from './i18n';
import { Caption, Confirm, FlashNote, Modal, Overlay, Section, useFlash } from './components/layout';
import { Button, Segmented, Select, TextArea, TextField, Toggle } from './components/primitives';
import { Board } from './components/board';
import { markApplied } from './applied';
import { MoveScrub, StoneDot } from './components/data';
import { Tag } from './components/ggs';

const FORMAT_KEY: Record<KifuPreview['format'], string> = {
  moves: 'dialog.kifu.format.moves', board: 'dialog.kifu.format.board', board_moves: 'dialog.kifu.format.board_moves',
  ggf: 'dialog.kifu.format.ggf', quest: 'dialog.kifu.format.quest',
};

/** Pasted text or a file's text, previewed (format, board at a move picked with the slider) before it loads. */
export function LoadKifu({ onLoad, onCancel }: {
  onLoad: (text: string, ply: number) => void; onCancel: () => void;
}) {
  const [text, setText] = useState('');
  const [file, setFile] = useState('');
  const [peek, setPeek] = useState<{ prev: KifuPreview | null; err: string } | null>(null);
  const [at, setAt] = useState<number | null>(null);
  const [fileErr, setFileErr] = useState('');
  useEffect(() => {
    const src = text.trim();
    let alive = true;
    if (!src) {
      const clear = setTimeout(() => { if (alive) setPeek(null); }, 0);
      return () => { alive = false; clearTimeout(clear); };
    }
    const id = setTimeout(() => {
      void api.previewKifu(src)
        .then((prev) => { if (alive) { setPeek({ prev, err: '' }); setAt(null); } })
        .catch((e) => { if (alive) setPeek({ prev: null, err: tErr(e) }); });
    }, 250);
    return () => { alive = false; clearTimeout(id); };
  }, [text]);

  const pick = async () => {
    try {
      const got = await api.readKifuFile();
      if (!got) return;   // closed without choosing
      setText(got.text);
      setFile(got.name);
      setFileErr('');
    } catch (e) { setFileErr(tErr(e)); }
  };

  // A board alone (one frame) loads too: text with neither board nor moves comes back as an error.
  const frames = peek?.prev?.frames ?? [];
  const ok = !peek?.err && frames.length > 0;
  const plies = Math.max(0, frames.length - 1);
  const ply = at ?? plies;
  const cur = ok ? frames[ply] : null;
  const err = fileErr || peek?.err;
  const summary = !cur ? '' : frames.length === 1
    ? t('dialog.kifu.summary_board', { side: t(`app.color.${cur.turn}`) })
    : t('dialog.kifu.summary_at', { n: ply, m: plies }) + ' · ' + (ply === plies && cur.black + cur.white === 64
      ? t('dialog.kifu.game_over') : t('dialog.kifu.to_move', { side: t(`app.color.${cur.turn}`) }));
  return (
    <Overlay onClose={onCancel}>
      <Modal title={t('dialog.kifu.title')} width="var(--w-modal-wide)" onClose={onCancel}
             actions={<>
               <Button size="field" onClick={() => void pick()}>{t('dialog.kifu.from_file')}</Button>
               <span style={{ marginLeft: 'auto' }} />
               <Button size="field" onClick={onCancel}>{t('dialog.cancel')}</Button>
               <Button size="field" variant="primary" disabled={!ok}
                       onClick={() => onLoad(text, ply)}>{t('dialog.kifu.load')}</Button>
             </>}>
        <TextArea mono rows={5} value={text} placeholder={t('dialog.kifu.placeholder')}
                  onChange={(v) => { setText(v); setFile(''); setFileErr(''); }} />
        <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'center', minHeight: 160 }}>
          <div style={{
            width: 160, height: 160, flex: 'none',
            borderRadius: 'var(--r-2)', overflow: 'hidden',
            // Until something is pasted the slot shows as a slot, not as a hole in the layout.
            border: cur ? 'none' : '1px dashed var(--border)',
          }}>
            {cur && <Board cells={cur.cells as (0 | 1 | 2)[]} last={cur.last} coords={false} grain={false} />}
          </div>
          <div style={{
            flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)',
            fontSize: 'var(--fs-5)', color: err ? 'var(--bad)' : 'var(--text)',
          }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
              <Caption>{t('dialog.kifu.preview')}</Caption>
              {ok && peek?.prev && <Tag>{t(FORMAT_KEY[peek.prev.format])}</Tag>}
              {file && <span style={{
                minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                fontSize: 'var(--fs-6)', color: 'var(--sub)',
              }}>{file}</span>}
            </div>
            {!peek && !err && <span style={{ color: 'var(--sub)' }}>{t('dialog.kifu.preview_hint')}</span>}
            {err && <span>{err}</span>}
            {cur && <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2h)' }}>
                <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
                  <StoneDot color="b" size={13} /><b style={{ fontWeight: 600 }}>{cur.black}</b>
                </span>
                <span style={{ color: 'var(--sub)' }}>—</span>
                <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
                  <b style={{ fontWeight: 600 }}>{cur.white}</b><StoneDot color="w" size={13} />
                </span>
              </div>
              <span style={{ color: 'var(--sub)' }}>{summary}</span>
            </>}
          </div>
        </div>
        {ok && plies > 0 && (
          <div style={{ margin: '0 calc(-1 * var(--sp-4))' }}>
            <MoveScrub plies={plies} cursor={ply} onSeek={setAt} />
          </div>
        )}
      </Modal>
    </Overlay>
  );
}

/** The record as text in a chosen format, to copy or save to a file. */
export function ExportKifu({ format, onFormat, names, onClose }: {
  format: ExportFormat; onFormat: (f: ExportFormat) => void; names: [string, string]; onClose: () => void;
}) {
  const [text, setText] = useState('');
  const [note, say] = useFlash();
  const [black, white] = names;
  useEffect(() => {
    let alive = true;
    void api.exportKifu(format, black, white)
      .then((s) => { if (alive) setText(s); })
      .catch((e) => { if (alive) { setText(''); say(tErr(e), true); } });
    return () => { alive = false; };
  }, [format, black, white, say]);
  const opts: { value: ExportFormat; label: string }[] = [
    { value: 'moves', label: t('dialog.export.moves') },
    { value: 'ggf', label: t('dialog.export.ggf') },
    { value: 'board_moves', label: t('dialog.export.board_moves') },
    { value: 'board', label: t('dialog.export.board') },
  ];
  return (
    <Overlay onClose={onClose}>
      <Modal title={t('dialog.export.title')} width="var(--w-modal-wide)" onClose={onClose}
             actions={<>
               <Button size="field" disabled={!text} onClick={() => {
                 void api.saveKifuText(text, format === 'ggf')
                   .then((path) => { if (path) say(t('dialog.export.saved', { path })); })
                   .catch((e) => say(tErr(e), true));
               }}>{t('dialog.export.save')}</Button>
               <span style={{ flex: 1, minWidth: 0, display: 'flex', paddingLeft: 'var(--sp-3)' }}><FlashNote note={note} /></span>
               <Button size="field" onClick={onClose}>{t('dialog.close')}</Button>
               <Button size="field" variant="primary" disabled={!text} onClick={() => {
                 void navigator.clipboard.writeText(text)
                   .then(() => say(t('dialog.export.copied')))
                   .catch(() => say(t('dialog.export.copy_failed'), true));
               }}>{t('dialog.export.copy')}</Button>
             </>}>
        <Segmented value={format} options={opts} fill onChange={(v) => onFormat(v as ExportFormat)} />
        {/* Read-only: the text is what the format makes of the record, so typing into it is dropped. */}
        <TextArea mono rows={5} value={text} onChange={() => {}} />
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
  if (n < 1024) return n + ' ' + t('unit.b');
  if (n < 1024 * 1024) return Math.round(n / 1024) + ' ' + t('unit.kb');
  return (n / 1024 / 1024).toFixed(1) + ' ' + t('unit.mb');
}

const notFound = (kind: string): string =>
  kind === 'nnue' ? t('settings.file.nnue_missing')
  : kind === 'book' ? t('settings.file.book_missing')
  : t('settings.file.weights_missing');

/** One setting: the label on the left, the control and a one-line help under it on the right. */
export function SettingRow({ label, help, error, children }: {
  label: string; help?: ReactNode; error?: ReactNode; children: ReactNode;
}) {
  return (
    <div style={{
      display: 'flex', gap: 'var(--sp-4)', padding: 'var(--sp-3) 0',
      borderBottom: '1px solid var(--border-weak)',
    }}>
      <span style={{ width: 'var(--w-label)', flex: 'none', lineHeight: 'var(--h-ctrl)', fontSize: 'var(--fs-4)' }}>{label}</span>
      <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', alignItems: 'flex-start' }}>
        {children}
        {error
          ? <span style={{ fontSize: 'var(--fs-6)', color: 'var(--bad)' }}>{error}</span>
          : help && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{help}</span>}
      </div>
    </div>
  );
}

/** A switch in a setting row, on the line of the row's label. */
export function SettingToggle({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <span style={{ display: 'flex', alignItems: 'center', height: 'var(--h-ctrl)' }}>
      <Toggle checked={checked} onChange={onChange} />
    </span>
  );
}

/** A titled group of setting rows. */
export function SettingGroup({ title, aside, children }: { title: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <Section title={title} aside={aside}>
      <div style={{ display: 'flex', flexDirection: 'column' }}>{children}</div>
    </Section>
  );
}

/** Learning from finished games: the switch, how much of the book it wrote, and the way to its log. */
export interface LearnSetting {
  on: boolean; set: (on: boolean) => void;
  stored?: number; learned?: number; onLog: () => void;
}

export function EngineSettings({ learn }: { learn: LearnSetting }) {
  useLang();
  const [reset, setReset] = useState(false);
  const [status, setStatus] = useState<[string, string, boolean, number, string][]>([]);
  const [th, setTh] = useState<ThreadsView | null>(null);
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
    markApplied();
  };
  const setThreads = async (n: number | null) => {
    try {
      await api.setLocalThreads(n);
      setTh(await api.localThreads());
      markApplied();
    } catch { /* leave it as is when the save fails */ }
  };
  const saveHash = async (mid: number, end: number) => {
    try { setHash(await api.setHashSizes(mid, end)); markApplied(); } catch { /* the save failed */ }
  };

  return (
    <>
      <SettingGroup title={t('settings.files.title')}>
        {kinds().map(([kind, label]) => {
          const info = byName.get(kind);
          return (
            <SettingRow key={kind} label={label}
                        help={info?.ok
                          ? [t('settings.file.loaded'), fmtSize(info.size)].filter(Boolean).join(' · ')
                          : undefined}
                        error={info && !info.ok ? notFound(kind) : undefined}>
              <span style={{ display: 'flex', gap: 'var(--sp-2)', width: '100%', maxWidth: 'var(--w-control)' }}>
                <span style={{ flex: 1, minWidth: 0, display: 'flex' }}>
                  <TextField mono value={info?.p ?? ''} placeholder={t('settings.file.unset')} invalid={!!info && !info.ok} />
                </span>
                <Button onClick={async () => {
                  const p = await api.pickResource(kind);
                  if (p) await change(kind, p);
                }}>{t('settings.file.choose')}</Button>
              </span>
            </SettingRow>
          );
        })}
        <SettingRow label={t('settings.reset.label')} help={t('settings.reset.help')}>
          <Button onClick={() => setReset(true)}>{t('settings.reset.button')}</Button>
        </SettingRow>
      </SettingGroup>

      <SettingGroup title={t('settings.search.title')}>
        {th && (
          <SettingRow label={t('settings.threads.label')} help={t('settings.threads.note')}>
            <Select width="var(--w-select-sm)" value={String(th.set ?? th.auto)}
                    onChange={(v) => void setThreads(+v === th.auto ? null : +v)}
                    options={Array.from({ length: th.auto * 2 }, (_, i) => {
                      const n = i + 1;
                      return [String(n), n === th.auto ? t('settings.threads.auto', { n }) : String(n)] as [string, string];
                    })} />
          </SettingRow>
        )}
        {hash && <>
          <SettingRow label={t('settings.hash.mid_label')}>
            <HashSelect hash={hash} bits={hash.mid} standard={22} bytes={(b) => 2 ** b * hash.entry[0]}
                        onPick={(b) => void saveHash(b, hash.end)} />
          </SettingRow>
          <SettingRow label={t('settings.hash.end_label')} help={t('settings.hash.note')}>
            <HashSelect hash={hash} bits={hash.end} standard={24} bytes={(b) => 2 ** b * hash.entry[1]}
                        onPick={(b) => void saveHash(hash.mid, b)} />
          </SettingRow>
        </>}
      </SettingGroup>

      <SettingGroup title={t('settings.learn.title')}>
        <SettingRow label={t('settings.learn.toggle')} help={t('settings.learn.help')}>
          <Toggle checked={learn.on} onChange={learn.set} />
        </SettingRow>
        <SettingRow label={t('settings.learn.book')}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-4)', minHeight: 'var(--h-ctrl)',
                        fontSize: 'var(--fs-5)', color: 'var(--sub)', whiteSpace: 'nowrap' }}>
            {([[t('settings.learn.stored'), learn.stored], [t('settings.learn.learned'), learn.learned]] as const).map(([label, n]) => (
              <span key={label}>{label}{' '}
                <b style={{ color: 'var(--text)', fontWeight: 600, fontVariantNumeric: 'tabular-nums' }}>{n?.toLocaleString() ?? '—'}</b>
              </span>
            ))}
            <Button onClick={learn.onLog}>{t('settings.learn.open_log')}</Button>
          </div>
        </SettingRow>
      </SettingGroup>

      {reset && (
        <Confirm title={t('settings.reset.title')}
                 body={t('settings.reset.body')}
                 ok={t('settings.reset.ok')}
                 onCancel={() => setReset(false)}
                 onOk={() => {
                   setReset(false);
                   void (async () => {
                     // Only the files, as the question says; the threads stay as chosen.
                     for (const [kind] of kinds()) await api.setResource(kind, null);
                     await load();
                     emitApp('resources-changed');
                   })();
                 }} />
      )}
    </>
  );
}

function HashSelect({ hash, bits, standard, bytes, onPick }: {
  hash: HashView; bits: number; standard: number;
  bytes: (bits: number) => number; onPick: (bits: number) => void;
}) {
  return (
    <Select width="var(--w-select-sm)" value={String(bits)} onChange={(v) => onPick(+v)}
            options={Array.from({ length: hash.max - hash.min + 1 }, (_, i) => {
              const b = hash.min + i;
              const size = fmtSize(bytes(b));
              return [String(b), b === standard ? t('settings.hash.default', { size }) : size] as [string, string];
            })} />
  );
}

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
  const dark = 'var(--swatch-dark)', light = 'var(--swatch-light)';
  return (
    <span style={{
      display: 'block', height: 38, borderRadius: 'var(--r-1)', width: '100%',
      boxShadow: 'inset 0 0 0 1px var(--border)',
      background: kind === 'dark' ? dark : kind === 'light' ? light
        : `linear-gradient(135deg, ${dark} 50%, ${light} 50%)`,
    }} />
  );
}

export function ViewSettings({ prefs, setPref }: {
  prefs: Prefs;
  setPref: <K extends keyof Prefs>(k: K, v: Prefs[K]) => void;
}) {
  useLang();
  return (
    <>
      <SettingGroup title={t('settings.screen.title')}>
        <SettingRow label={t('settings.theme.title')}>
          <div style={{ display: 'flex', gap: 'var(--sp-2h)', width: '100%', maxWidth: 420 }}>
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
        </SettingRow>
        <SettingRow label={t('settings.language.label')}>
          <Select value={prefs.lang} onChange={(v) => setPref('lang', v as Prefs['lang'])}
                  options={languages()} width="var(--w-select)" />
        </SettingRow>
        <SettingRow label={t('settings.numbers.decimals')}>
          <Segmented value={String(prefs.decimals)}
                     onChange={(v) => setPref('decimals', +v as Prefs['decimals'])}
                     options={[{ value: '0', label: '0' },
                               { value: '1', label: '1' },
                               { value: '2', label: '2' }]} />
        </SettingRow>
      </SettingGroup>
      <SettingGroup title={t('settings.board.title')}>
        <SettingRow label={t('settings.board.coords')}>
          <SettingToggle checked={prefs.coords} onChange={(v) => setPref('coords', v)} />
        </SettingRow>
        <SettingRow label={t('settings.board.grain')}>
          <SettingToggle checked={prefs.grain} onChange={(v) => setPref('grain', v)} />
        </SettingRow>
        <SettingRow label={t('settings.board.flip')}>
          <Segmented value={String(prefs.flipMs)} onChange={(v) => setPref('flipMs', +v as Prefs['flipMs'])}
                     options={[{ value: '0', label: t('settings.flip.none') },
                               { value: '120', label: t('settings.flip.fast') },
                               { value: '240', label: t('settings.flip.slow') }]} />
        </SettingRow>
        <SettingRow label={t('settings.board.facing')} help={t('settings.view.facing_help')}>
          <Segmented value={prefs.facing} onChange={(v) => setPref('facing', v)} options={[
            { value: 'black', label: t('settings.facing.black') },
            { value: 'white', label: t('settings.facing.white') },
            { value: 'auto', label: t('settings.facing.auto') },
          ]} />
        </SettingRow>
        <SettingRow label={t('settings.board.tatami')}>
          <span style={{ display: 'flex', gap: 'var(--sp-2)' }}>
            {TATAMI.map((mat, i) => {
              const on = prefs.tatami === i;
              const label = t(mat.labelKey);
              return (
                <button key={mat.labelKey} type="button" className="k-press"
                        onClick={() => setPref('tatami', i as Prefs['tatami'])}
                        title={label} aria-label={label} aria-pressed={on}
                        style={{
                          width: 'var(--h-ctrl)', height: 'var(--h-ctrl)', borderRadius: 'var(--r-2)', padding: 0,
                          border: 0, background: mat.board,
                          boxShadow: on
                            ? '0 0 0 2px var(--accent), inset 0 0 0 1px var(--border)'
                            : 'inset 0 0 0 1px var(--border)',
                        }} />
              );
            })}
          </span>
        </SettingRow>
      </SettingGroup>
    </>
  );
}
