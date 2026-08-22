import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { api, emitApp, onApp, type HashView, type KifuFrame, type ThreadsView } from './api';
import { TATAMI, type Prefs, type Theme } from './prefs';
import { Modal, Note, Overlay, Section } from './components/layout';
import { Button, Segmented, Select, TextField } from './components/primitives';
import { Icon } from './components/Icons';
import { Board } from './components/board';
import { StoneDot } from './components/data';
import { GgsSettings } from './GgsScreens';
import type { GgsSnapshot } from './types';

/* Confirmations and inputs, replacing browser confirm()/prompt()
 * (which ignore the design and render OS-styled in the WebView). */

export function Confirm({ title, body, ok = 'OK', danger, onOk, onCancel }: {
  title: string; body?: React.ReactNode; ok?: string; danger?: boolean;
  onOk: () => void; onCancel: () => void;
}) {
  return (
    <Overlay onClose={onCancel}>
      <Modal title={title} body={body} onClose={onCancel} actions={<>
        <span style={{ marginLeft: 'auto' }} />
        <Button size="field" onClick={onCancel}>やめる</Button>
        <Button size="field" variant={danger ? 'danger' : 'primary'} onClick={onOk}>{ok}</Button>
      </>} />
    </Overlay>
  );
}

/** Pick one from a list (chat's new-conversation picker). */
export function PickOne({ title, body, options, ok = '開く', onOk, onCancel }: {
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
               <Button size="field" onClick={onCancel}>やめる</Button>
               <Button size="field" variant="primary" disabled={!v} onClick={() => onOk(v)}>{ok}</Button>
             </>} />
    </Overlay>
  );
}

/** Paste-a-record loader; the file picker path shares the box. */
export function PasteKifu({ onLoad, onFile, onCancel }: {
  onLoad: (text: string) => void; onFile: () => void; onCancel: () => void;
}) {
  const [text, setText] = useState('');
  /* Preview parseability before loading: GGF, move lists and
   * board-prefixed forms are all accepted, so without a preview you
   * cannot tell until you press. The backend dry-runs it (game state
   * untouched), debounced. */
  const [peek, setPeek] = useState<{ frames: KifuFrame[]; err: string } | null>(null);
  useEffect(() => {
    const t = text.trim();
    let alive = true;
    if (!t) {
      // Clearing is debounced too; never set state directly in the effect.
      const clear = setTimeout(() => { if (alive) setPeek(null); }, 0);
      return () => { alive = false; clearTimeout(clear); };
    }
    const id = setTimeout(() => {
      void api.previewKifu(t)
        .then((frames) => { if (alive) setPeek({ frames, err: '' }); })
        .catch((e) => { if (alive) setPeek({ frames: [], err: '' + e }); });
    }, 250);
    return () => { alive = false; clearTimeout(id); };
  }, [text]);

  // frames[0] is the start; fewer than two means no move parsed.
  const ok = !!peek && !peek.err && peek.frames.length > 1;
  const last = ok ? peek!.frames[peek!.frames.length - 1] : null;
  return (
    <Overlay onClose={onCancel}>
      <Modal title="棋譜を読み込む" width="var(--w-modal-wide)" onClose={onCancel}
             sub="GGF・f5d6… 形式・盤面つきのいずれでも読めます。"
             actions={<>
               <Button size="field" onClick={onFile}>ファイルから…</Button>
               <span style={{ marginLeft: 'auto' }} />
               {/* The design caught up to this wording (2026-08-08). */}
               <Button size="field" onClick={onCancel}>やめる</Button>
               <Button size="field" variant="primary" disabled={!ok}
                       onClick={() => onLoad(text)}>読み込む</Button>
             </>}>
        <textarea value={text} onChange={(e) => setText(e.target.value)}
          className="k-input"
          style={{
            height: 110, resize: 'none', padding: 'var(--sp-3)', borderRadius: 'var(--r-3)',
            background: 'var(--bg)', border: '1px solid var(--border)',
            fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', lineHeight: 1.6,
          }} />
        {/* Preview result: the reason when unreadable, discs and move
            count when readable. Dimensions from the design capture. */}
        <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'center', minHeight: 96 }}>
          <div style={{
            width: 96, height: 96, flex: 'none',
            // The svg's own rx shrinks to 1.5px at 96px; clip with the
            // container to match the design's rounding.
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
            }}>下読み</span>
            {!peek && <span style={{ color: 'var(--sub)' }}>貼り付けると、ここに読み取り結果が出ます。</span>}
            {peek?.err && <span>{peek.err}</span>}
            {last && <>
              {/* Dots outside the numbers: this reads "black vs white"
                  (one game's result), unlike ScoreRow's comparison row. */}
              <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2h)' }}>
                <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
                  <StoneDot color="b" size={13} /><b style={{ fontWeight: 600 }}>{last.black}</b>
                </span>
                <span style={{ color: 'var(--sub)' }}>—</span>
                <span style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-0)' }}>
                  <b style={{ fontWeight: 600 }}>{last.white}</b><StoneDot color="w" size={13} />
                </span>
              </div>
              {/* Never call an unfinished board the final position —
                  truncated records load too, and then it is just the
                  last position. */}
              <span style={{ color: 'var(--sub)' }}>
                {peek!.frames.length - 1} 手 ・ {last.black + last.white === 64 ? '終局図' : '最後の局面'}
              </span>
            </>}
            {peek && !peek.err && !ok && <span style={{ color: 'var(--sub)' }}>手が 1 つも読み取れません。</span>}
          </div>
        </div>
      </Modal>
    </Overlay>
  );
}

/* ---------------- Settings (gear) ----------------
 * Engine files, local thread count, learning import. GGS engine
 * settings live in the GGS screens (separate engine, separate
 * settings). */

// Display names must match resource_status's — a mismatch loses the
// status lookup and the path display.
/* Order per the design (NNUE, linear, book) — what KUROOBI reads most
 * sits on top. The third field is the display name; the second is the
 * backend's key and must not drift. */
const KINDS: [string, string, string][] = [
  ['nnue', 'NNUE の重み', 'NNUE 重み'],
  ['weights', '線形評価の重み', '線形評価'],
  ['book', '定石', '定石'],
];

/** File size; MB capped at one decimal so digits stay put. */
function fmtSize(n: number): string {
  if (n <= 0) return '';
  if (n < 1024) return n + ' B';
  if (n < 1024 * 1024) return Math.round(n / 1024) + ' KB';
  return (n / 1024 / 1024).toFixed(1) + ' MB';
}

/* Missing files state the consequence, not just the absence —
 * "missing" alone does not say whether anything breaks. */
const NOT_FOUND: Record<string, string> = {
  weights: '見つかりません。KUROOBI は評価できません',
  nnue: '見つかりません。線形評価だけで指します',
  book: '見つかりません。実戦から育つ book_learn.txt だけを使います',
};

/* Settings once lived in a separate window; with no shared React
 * state, prefs sync via localStorage and backend-owned values send
 * change notifications. Learning import controls stay in the dock
 * (one setting, one place). */
export function Settings({ prefs, setPref, ggs, initialTab, onClose }: {
  prefs: Prefs; setPref: <K extends keyof Prefs>(k: K, v: Prefs[K]) => void;
  /** GGS snapshot for the GGS tab (null while disconnected). */
  ggs?: GgsSnapshot | null;
  /** Initial tab (screenshot entry, `KUROOBI_AUTOPLAY=settings:ggs`). */
  initialTab?: 'engine' | 'view' | 'ggs';
  /** Close (the overlay's footer button). */
  onClose?: () => void;
}) {
  const [tab, setTab] = useState<'engine' | 'view' | 'ggs'>(initialTab ?? 'engine');
  /* Screenshot entry: tabs are click-only, so automation needs this.
     A third segment (settings:view:light) actually switches the theme
     so the change can be captured without clicking. */
  useEffect(() => {
    void api.autoplay().then((v) => {
      const [, t, arg] = (v ?? '').split(':');
      if (t === 'engine' || t === 'view' || t === 'ggs') setTab(t);
      if (arg === 'light' || arg === 'dark' || arg === 'os') {
        // Delay so the capture can catch the before state.
        window.setTimeout(() => setPref('theme', arg), 5000);
      }
    }).catch(() => { /* Tauri 外では効かないだけ */ });
    // setPref is stable for the window's lifetime; run once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [reset, setReset] = useState(false);
  const [status, setStatus] = useState<[string, string, boolean, number, string][]>([]);
  const [th, setTh] = useState<ThreadsView | null>(null);
  /** Solve-speed measurement in progress (seconds; shown as disabled). */
  const [calib, setCalib] = useState(false);
  const [hash, setHash] = useState<HashView | null>(null);

  const load = useCallback(async () => {
    try { setStatus(await api.resourceStatus()); } catch { /* エンジン未初期化 */ }
  }, []);

  // Fetch the state as of opening; discard replies after close.
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [st, t, h] = await Promise.all([
          api.resourceStatus(), api.localThreads(), api.hashSizes(),
        ]);
        if (!alive) return;
        setStatus(st);
        setTh(t);
        setHash(h);
      } catch { /* エンジン未初期化 */ }
    })();
    return () => { alive = false; };
  }, []);

  /* Calibration runs in the background; opening this right after
     launch once froze the "unmeasured" state. Re-fetch on the
     completion notice. */
  useEffect(() => {
    let alive = true;
    const off = onApp('resources-changed', () => {
      void api.localThreads().then((t) => { if (alive) setTh(t); }).catch(() => {});
    });
    return () => { alive = false; void off.then((f) => f()); };
  }, []);

  const byName = new Map(status.map(([n, p, ok, size, kind]) => [n, { p, ok, size, kind }]));
  const change = async (kind: string, path: string | null) => {
    await api.setResource(kind, path);
    await load();
    // The main screen is another document and cannot notice itself.
    emitApp('resources-changed');
  };
  const setThreads = async (n: number | null) => {
    try {
      await api.setLocalThreads(n);
      setTh(await api.localThreads());
    } catch { /* 保存失敗はそのまま */ }
  };

  return (
    <Modal title="設定" width="560px" onClose={onClose} scroll
           band={<>
               {/* Tabs are not Segmented: the design draws bare labels
                   with only the selection filled. The strip container
                   (44px, --card, bottom keyline) belongs to Modal. */}
               <div style={{
                 flex: 1, display: 'flex', alignItems: 'center',
                 justifyContent: 'center', gap: 'var(--sp-1)',
               }}>
                 {TABS.map(([v, label]) => {
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

        {/* Display-only settings; localStorage, never the backend. */}
        {tab === 'view' && <ViewSettings prefs={prefs} setPref={setPref} />}

        {/* GGS settings live here per the design; the nav destination
            was dropped (two roads to one setting compete). */}
        {/* Snapshots exist even disconnected. Hiding the whole tab
            until connect was overkill: strength/clock/book/behavior
            are local settings the disconnected loop accepts fine; only
            the server-side sections fold inside GgsSettings. */}
        {tab === 'ggs' && (ggs
          ? <GgsSettings snap={ggs} />
          : (
            <Section title="GGS">
              <Note>
                GGS の設定を読み込めていません。申し込みの扱いなどは
                サーバー側に残る設定なので、繋いでから読み書きします。
              </Note>
            </Section>
          )
        )}


        {tab === 'engine' && <>
        <Section title="ファイル">
          {KINDS.map(([kind, title, label]) => {
            const info = byName.get(title);
            return (
              <div key={kind} style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                {/* The path lives in the input; pushed to the row edge
                    it separates "what is read" from "how to change it". */}
                <Row2 label={label}>
                  <TextField value={info?.p ?? ''} placeholder="未指定" invalid={!info?.ok} />
                  <Button size="field" onClick={async () => {
                    const p = await api.pickResource(kind);
                    if (p) await change(kind, p);
                  }}>選択…</Button>
                </Row2>
                {/* Status goes right under the field, aligned to its
                    column (the design leaves 96px too). */}
                <div style={{
                  marginLeft: 'calc(var(--w-label) + var(--sp-3))',
                  display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                  fontSize: 'var(--fs-6)', color: info?.ok ? 'var(--ok)' : 'var(--bad)',
                }}>
                  <Icon name={info?.ok ? 'check' : 'alert'} size={12} />
                  {info?.ok
                    ? ['読み込み済み', fmtSize(info.size), info.kind].filter(Boolean).join(' · ')
                    : NOT_FOUND[kind]}
                </div>
              </div>
            );
          })}
        </Section>

          <Section title="ローカル対局の持ち時間">
            <Row2 label="持ち時間">
              <Select width={140} value={String(prefs.clockSecs)}
                      onChange={(v) => setPref('clockSecs', +v)}
                      options={[['0', 'なし'], ['300', '5 分'], ['600', '10 分'],
                                ['900', '15 分'], ['1200', '20 分'], ['1800', '30 分']]} />
            </Row2>
            <Note>KUROOBI と自分の両方が持ちます。次の「新規対局」から効きます。切れた側の負けです。</Note>
          </Section>
        {th && (
          <Section title="ローカル探索のスレッド数">
            <Row2 label="スレッド">
              {/* Auto is not a separate item: it marks its number
                  inside the one column, and selecting it saves "unset"
                  — saving the number would freeze auto on a machine
                  with different cores. */}
              <Select width={140} value={String(th.set ?? th.auto)}
                      onChange={(v) => void setThreads(+v === th.auto ? null : +v)}
                      options={Array.from({ length: th.auto * 2 }, (_, i) => {
                        const n = i + 1;
                        return [String(n), n === th.auto ? `${n} (自動)` : String(n)] as [string, string];
                      })} />
            </Row2>
            {/* The description spans the section under the control;
                indented to the field column it reads as a field hint. */}
            <Note>ローカル対局・検討・学習・GGS 対局が使う並列数です。</Note>
          </Section>
        )}
        {/* Speed is per-thread-count, so it follows the threads
            section — but as its own section, or the heading lies. */}
        {th && (
          <Section title="読切の速度">
            <Row2 label="この機械">
              {/* Give the value field width so the buttons align with
                  the column above. */}
              <span style={{
                width: 200, flex: 'none',
                fontSize: 'var(--fs-5)', fontVariantNumeric: 'tabular-nums',
                color: th.nps != null ? 'var(--text)' : 'var(--sub)',
              }}>
                {th.nps == null ? '未測定' : `${(th.nps / 1e6).toFixed(1)}M ノード/秒`}
              </span>
              <Button size="field" disabled={calib}
                      onClick={() => void (async () => {
                        setCalib(true);
                        try { setTh(await api.calibrateNps()); } catch { /* 測れなければ据え置き */ }
                        setCalib(false);
                      })()}>{calib ? '測定中…' : '測り直す'}</Button>
            </Row2>
            <Note>持ち時間のある対局で読切に入る空きを決めるのに使います。起動時に自動で測ります。</Note>
          </Section>
        )}
        {hash && (
          <Section title="置換表の大きさ">
            {/* The endgame default (22) is small: billion-node solves
                overflow it, and 22 -> 26 measured -14-16% nodes /
                -23-31% time — exactly the region GGS games read. */}
            <Row2 label="中盤">
                <Select width={140} value={String(hash.mid)}
                        onChange={(v) => void (async () => {
                          try { setHash(await api.setHashSizes(+v, hash.end)); } catch { /* 保存失敗 */ }
                        })()}
                        options={Array.from({ length: hash.max - hash.min + 1 }, (_, i) => {
                          const b = hash.min + i;
                          return [String(b), `${fmtSize(2 ** (b + 4))}${b === 22 ? ' (既定)' : ''}`] as [string, string];
                        })} />
            </Row2>
            <Row2 label="終盤">
              <Select width={140} value={String(hash.end)}
                      onChange={(v) => void (async () => {
                        try { setHash(await api.setHashSizes(hash.mid, +v)); } catch { /* 保存失敗 */ }
                      })()}
                      options={Array.from({ length: hash.max - hash.min + 1 }, (_, i) => {
                        const b = hash.min + i;
                        return [String(b), `${fmtSize(2 ** b * 24)}${b === 24 ? ' (既定)' : ''}`] as [string, string];
                      })} />
            </Row2>
            <Note>次の起動から効きます。増やしたら一度起動して確かめてください。</Note>
          </Section>
        )}
        </>}

        {/* No OK button (changes apply immediately — said explicitly,
            or it feels unconfirmed). Shown only on tabs with content;
            a bare keyline over nothing reads as something missing. */}
        {tab === 'engine' && (
          <div style={{
            display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
            margin: '0 var(--sp-3)', paddingTop: 'var(--sp-4)',
            borderTop: '1px solid var(--border-weak)',
          }}>
            <Button size="field" onClick={() => setReset(true)}>既定に戻す</Button>
          </div>
        )}
      </div>
      {reset && (
        <Confirm title="ファイルの指定を既定に戻しますか？"
                 body="選んだ重みと定石の場所を忘れ、既定の探し方に戻します。ファイルそのものは消えません。"
                 ok="戻す"
                 onCancel={() => setReset(false)}
                 onOk={() => {
                   setReset(false);
                   void (async () => {
                     for (const [kind] of KINDS) await api.setResource(kind, null);
                     await setThreads(null);
                     await load();
                     emitApp('resources-changed');
                   })();
                 }} />
      )}
    </Modal>
  );
}

/* The design's fourth tab was dropped: no drawn content, no matching
 * spec — an empty tab sends people searching. */
const TABS: ['engine' | 'view' | 'ggs', string][] = [
  ['engine', 'エンジン'], ['view', '表示'], ['ggs', 'GGS'],
];

const THEMES: [Theme, string][] = [
  ['os', 'システムに合わせる'], ['dark', 'ダーク'], ['light', 'ライト'],
];

/** Theme swatches, ground color only (boards and text would crush at
 *  this size); "system" splits the two diagonally. */
function ThemeSwatch({ kind }: { kind: Theme }) {
  /* Literal colors, deliberately: swatches show the OTHER theme's
     ground, and var(--bg) would paint all three identically. Keep in
     sync with tokens.css. */
  const dark = '#16191d', light = '#faf8f3';
  return (
    <span style={{
      display: 'block', height: 38, borderRadius: 'var(--r-1)', width: '100%',
      // The dark swatch matches the ground; a 1px inner line keeps it
      // visible.
      boxShadow: kind === 'dark' ? 'inset 0 0 0 1px var(--border)' : undefined,
      background: kind === 'dark' ? dark : kind === 'light' ? light
        : `linear-gradient(135deg, ${dark} 50%, ${light} 50%)`,
    }} />
  );
}

/** One settings row with aligned label column; labels right-align so
 *  varying lengths keep a constant gap to the fields. */
function Row2({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', minHeight: 'var(--h-field)' }}>
      <span style={{ width: 'var(--w-label)', flex: 'none', textAlign: 'right',
                     fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>{label}</span>
      {children}
    </div>
  );
}

/** The Display tab: appearance only, localStorage only. Extracted
 *  from Settings (the tabs share nothing). */
function ViewSettings({ prefs, setPref }: {
  prefs: Prefs;
  setPref: <K extends keyof Prefs>(k: K, v: Prefs[K]) => void;
}) {
  return (
    <>
      <Section title="テーマ">
        {/* Three swatch cards per the design — colors read faster than
            words, including what "follow OS" resolves to. */}
        <div style={{ display: 'flex', gap: 'var(--sp-2h)' }}>
          {THEMES.map(([v, label]) => {
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
      <Section title="盤">
        <Row2 label="畳の色">
          {/* Four color swatches; words would require a round trip to
              the board to see the choice. */}
          <span style={{ display: 'flex', gap: 'var(--sp-2)' }}>
            {TATAMI.map((t, i) => {
              const on = prefs.tatami === i;
              return (
                <button key={t.label} type="button" className="k-press"
                        onClick={() => setPref('tatami', i as Prefs['tatami'])}
                        title={t.label} aria-label={t.label} aria-pressed={on}
                        style={{
                          width: 28, height: 28, borderRadius: 'var(--r-2)', padding: 0,
                          border: 0, background: t.board,
                          // Selection gets an outer 2px ring; an inner
                          // frame would thin the dark swatch colors.
                          boxShadow: on
                            ? '0 0 0 2px var(--accent), inset 0 0 0 1px var(--border)'
                            : 'inset 0 0 0 1px var(--border)',
                        }} />
              );
            })}
          </span>
        </Row2>
        <Row2 label="座標">
          <Segmented value={prefs.coords ? 'on' : 'off'} onChange={(v) => setPref('coords', v === 'on')}
                     options={[{ value: 'on', label: '出す' }, { value: 'off', label: '出さない' }]} />
        </Row2>
        <Row2 label="織り目">
          <Segmented value={prefs.grain ? 'on' : 'off'} onChange={(v) => setPref('grain', v === 'on')}
                     options={[{ value: 'on', label: '出す' }, { value: 'off', label: '出さない' }]} />
        </Row2>
        <Row2 label="石返し">
          <Segmented value={String(prefs.flipMs)} onChange={(v) => setPref('flipMs', +v as Prefs['flipMs'])}
                     options={[{ value: '0', label: '動かさない' },
                               { value: '120', label: '速い' },
                               { value: '240', label: 'ゆっくり' }]} />
        </Row2>
        <Row2 label="盤の向き">
          <Segmented value={prefs.facing} onChange={(v) => setPref('facing', v)} options={[
            { value: 'black', label: '黒が下' },
            { value: 'white', label: '白が下' },
            { value: 'auto', label: '自分が下' },
          ]} />
        </Row2>
      </Section>
      <Section title="数値">
        {/* No unit setting (discs are the only unit). Viewpoint is the
            eval sign, not board orientation, and switches from the
            study toolbar per the design. */}
        <Row2 label="小数">
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
