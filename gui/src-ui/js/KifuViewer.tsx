import { useEffect, useEffectEvent, useState } from 'react';
import { api, ggsApi, type KifuFrame } from './api';
import { Board } from './components/board';
import { MoveScrub, StoneDot } from './components/data';
import { Caption, FlashNote, Modal, Overlay, useFlash } from './components/layout';
import { Button, Segmented } from './components/primitives';
import { sqName } from './adapt';
import { t, tErr } from './i18n';
import { gtypeLabel } from './ggs';
import { clockLabel, signed, useWindowHeight } from './ggsui';

interface KifuViewerProps {
  title: string;
  kifu: string;
  onClose: () => void;
  /** Resolves false when the person keeps the record they have (the viewer then stays open). */
  onStudy: (kifu: string) => Promise<boolean>;
  onRefetch?: () => void;
  parts?: string[];
}

export function KifuViewer({ title, kifu, onClose, onStudy, onRefetch, parts }: KifuViewerProps) {
  const [pick, setPick] = useState({ key: '', face: 0 });
  const key = parts?.[0] ?? '';
  const face = pick.key === key ? pick.face : 0;
  const setFace = (i: number) => setPick({ key, face: i });
  const shown = parts && parts.length > 1 ? (parts[face] ?? kifu) : kifu;
  const pb = tagOf(shown, 'PB');
  const pw = tagOf(shown, 'PW');
  const [got, setGot] = useState<{ kifu: string; frames?: KifuFrame[]; err?: string } | null>(null);
  const [at, setAt] = useState<number | null>(null);
  const [note, say] = useFlash();
  const [playing, setPlaying] = useState(false);

  // The latest onRefetch, without making a new one rerun the preview.
  const refetch = useEffectEvent((): boolean => {
    if (!onRefetch) return false;
    onRefetch();
    return true;
  });
  useEffect(() => {
    if (!shown.trim()) return;
    let alive = true;
    void api.previewKifu(shown)
      .then((frames) => { if (alive) { setGot({ kifu: shown, frames }); setAt(null); } })
      .catch((e) => {
        if (!alive) return;
        if (refetch()) return;
        setGot({ kifu: shown, err: tErr(e) }); setAt(null);
      });
    return () => { alive = false; };
  }, [shown]);

  const frames = got?.kifu === shown ? got.frames : undefined;
  const err = got?.kifu === shown ? got.err : undefined;
  const last = frames ? frames.length - 1 : 0;
  const cur = Math.min(at ?? last, last);
  const f = frames?.[cur];
  const end = frames?.[last];
  const moveList = frames ? frames.slice(1).map((k) => moveName(k.last)).join('') : '';

  const running = playing && cur < last;
  useEffect(() => {
    if (!running) return;
    const id = window.setInterval(() => {
      setAt((a) => Math.min((a ?? 0) + 1, last));
    }, 450);
    return () => { window.clearInterval(id); };
  }, [running, last]);

  // The dialog is at most 88% of the window; the board takes what the rows around it leave, up to 300.
  const boardPx = Math.max(160, Math.min(300, Math.floor(useWindowHeight() * 0.88) - 290));
  const seek = (n: number) => { setPlaying(false); setAt(Math.max(0, Math.min(n, last))); };

  // The line under the title: format, clock and the result from the first-named player's side.
  const faces = parts && parts.length > 1 ? parts : [shown];
  const res = faces.map((g, i) => {
    const v = parseFloat(tagOf(g, 'RE'));
    return isFinite(v) ? (i === 0 ? v : -v) : null;
  });
  const total = res.every((v) => v != null) ? res.reduce((x, v) => x + (v ?? 0), 0) : null;
  const [type, clock, black, white] = (['TY', 'TI', 'PB', 'PW'] as const).map((tag) => tagOf(faces[0], tag));
  const sub = [
    // A synchro game's boards say "8r14" on some servers and "s8r14" on others.
    type ? gtypeLabel((faces.length > 1 && !type.startsWith('s') ? 's' : '') + type) : '',
    clock ? clockLabel(clock) : '',
    // Told the way the game screen tells it: a draw, or who won and by how much.
    black && white
      ? (total == null ? `${black} − ${white}`
        : Math.round(total) === 0 ? t('play.draw')
        : t('play.won_by', { name: total > 0 ? black : white, d: Math.abs(Math.round(total)) }))
      : '',
  ].filter(Boolean).join(' · ');

  return (
    <Overlay onClose={onClose}>
      <Modal title={title} sub={sub || undefined} width="var(--w-modal-wide)" onClose={onClose} scroll
             band={parts && parts.length > 1 ? (
               <Segmented value={String(face)} onChange={(v) => setFace(Number(v))}
                          options={parts.map((_, i) => ({
                            value: String(i), label: t('kifu.board_tab', { n: i + 1 }),
                          }))} />
             ) : undefined}
             actions={<>
               <Button size="field" variant="ghost" onClick={onClose}>{t('kifu.close')}</Button>
               <span style={{ flex: 1, minWidth: 0, display: 'flex', paddingLeft: 'var(--sp-3)' }}><FlashNote note={note} /></span>
               <Button size="field" disabled={!shown} onClick={() => {
                 void navigator.clipboard.writeText(shown)
                   .then(() => say(t('kifu.copied')))
                   .catch(() => say(t('kifu.copy_failed'), true));
               }}>{t('kifu.copy')}</Button>
               <Button size="field" disabled={!shown}
                       onClick={() => void ggsApi.saveKifu(shown, 'kifu')
                         .catch((e) => say(t('kifu.save_failed', { err: tErr(e) }), true))}>
                 {t('kifu.save')}
               </Button>
               <Button size="field" variant="primary" disabled={!frames}
                       onClick={() => void onStudy(shown).then((done) => { if (done) onClose(); })}>{t('kifu.open_in_study')}</Button>
             </>}>
        {!f ? (
          // While the server sends the kifu, and when it cannot.
          <div style={{ minHeight: 240, display: 'grid', placeItems: 'center', padding: 'var(--sp-5)', textAlign: 'center',
                        fontSize: 'var(--fs-5)', color: err ? 'var(--bad)' : 'var(--sub)' }}>
            {err ? <span style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
              {t('kifu.failed')}
              <span className="k-sel" style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{err}</span>
            </span> : t('kifu.fetching')}
          </div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-4)' }}>
            <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'flex-start' }}>
              <div style={{ width: boardPx, height: boardPx, flex: 'none' }}>
                <Board cells={f.cells as (0 | 1 | 2)[]} last={f.last} coords disabled />
              </div>
              <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                  {([['b', pb, tagOf(shown, 'RB'), f.black], ['w', pw, tagOf(shown, 'RW'), f.white]] as const).map(([c, n, r, cnt]) => (
                    <div key={c} style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', height: 'var(--h-field)', whiteSpace: 'nowrap' }}>
                      <StoneDot color={c} />
                      <span className="k-sel" style={{ fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis' }}>{n || '?'}</span>
                      {r && <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{r}</span>}
                      <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-2)', fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>{cnt}</span>
                    </div>
                  ))}
                </div>
                <div style={{ display: 'flex', alignItems: 'baseline', gap: 'var(--sp-3)', paddingTop: 'var(--sp-2)',
                              borderTop: '1px solid var(--border-weak)', whiteSpace: 'nowrap' }}>
                  <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>{t('kifu.this_move')}</span>
                  <b style={{ fontSize: 'var(--fs-1)', fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>
                    {cur === 0 ? t('kifu.initial_position') : `${cur}. ${moveName(f.last)}`}
                  </b>
                  {end && (
                    <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
                      {t('kifu.final')} <b style={{ color: 'var(--text)', fontWeight: 600 }}>{signed(end.black - end.white)}</b>
                    </span>
                  )}
                </div>
                {moveList && (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
                    <Caption>{t('kifu.raw.moves')}</Caption>
                    <div className="k-scroll k-sel" style={{
                      maxHeight: 96, padding: 'var(--sp-1) var(--sp-2)', borderRadius: 'var(--r-2)', background: 'var(--card)',
                      fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', lineHeight: 1.6, wordBreak: 'break-all',
                    }}>{moveList}</div>
                  </div>
                )}
              </div>
            </div>

            <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', whiteSpace: 'nowrap' }}>
                <Button size="field" square title={t('data.scrub.first')} disabled={cur === 0} onClick={() => seek(0)}>|◀</Button>
                <Button size="field" square title={t('data.scrub.prev')} disabled={cur === 0} onClick={() => seek(cur - 1)}>◀</Button>
                <Button size="field" square title={t('data.scrub.next')} disabled={cur >= last} onClick={() => seek(cur + 1)}>▶</Button>
                <Button size="field" square title={t('data.scrub.last')} disabled={cur >= last} onClick={() => seek(last)}>▶|</Button>
                <Button size="field" square title={running ? t('kifu.nav.stop') : t('kifu.nav.autoplay')}
                        disabled={last === 0}
                        onClick={() => { if (running) { setPlaying(false); return; } if (cur >= last) setAt(0); setPlaying(true); }}>
                  {running ? '❚❚' : '▶▶'}
                </Button>
                <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-6)', color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                  <b style={{ fontSize: 'var(--fs-1)', fontWeight: 600, color: 'var(--text)' }}>{cur}</b>
                  {' '}{t('kifu.of_plies', { n: last })}
                </span>
              </div>
              <MoveScrub nav={false} plies={last} cursor={cur} onSeek={seek} />
            </div>
          </div>
        )}
      </Modal>
    </Overlay>
  );
}

const tagOf = (ggf: string, tag: string): string => new RegExp(tag + '\\[([^\\]]*)\\]').exec(ggf)?.[1] ?? '';

const moveName = (sq: number | null): string => (sq == null ? t('kifu.pass') : sqName(sq));
