import { useEffect, useState } from 'react';
import { api, ggsApi, type KifuFrame } from './api';
import { Board } from './components/board';
import { MoveScrub, ScoreRow, StoneDot } from './components/data';
import { Modal, Overlay } from './components/layout';
import { Segmented } from './components/primitives';
import { Button } from './components/primitives';

/* Record viewer overlay. Raw records (GGF, f5d6...) are not for human
 * reading, so boards come first; copy/save/study follow once satisfied.
 *
 * A Modal, not a destination: opened from any list, closing returns
 * there. Move navigation reuses the study strip (one component per
 * interaction). Four bands with keylines at header and footer, per
 * the design captures. Items the design shows but data cannot provide
 * are listed in notes/design-sync-from-impl.md. */

export interface KifuViewerProps {
  title: string;
  /** GGF or a move list; empty shows the fetching state. */
  kifu: string;
  onClose: () => void;
  /** Open in study. */
  onStudy: (kifu: string) => void;
  /** Escape hatch when the local record is unreadable (re-fetch from
   *  the archive). */
  onRefetch?: () => void;
  /** All games under the archive id (synchro has two boards; showing
   *  one loses the other). The tab strip appears only with 2+. */
  parts?: string[];
  /** Our login; our color flips per board, and only the color says
   *  which board this is. */
  me?: string;
}

/** Extract player names from GGF; empty if absent. */
function playerOf(ggf: string, tag: 'PB' | 'PW'): string {
  return new RegExp(tag + '\\[([^\\]]*)\\]').exec(ggf)?.[1] ?? '';
}

export function KifuViewer({ title, kifu, onClose, onStudy, onRefetch, parts, me }: KifuViewerProps) {
  /* Which board is shown; a different game resets to board 1. Keyed
     by the record itself and decided during render (an effect would
     double-render). */
  const [pick, setPick] = useState({ key: '', face: 0 });
  const key = parts?.[0] ?? '';
  const face = pick.key === key ? pick.face : 0;
  const setFace = (i: number) => setPick({ key, face: i });
  const shown = parts && parts.length > 1 ? (parts[face] ?? kifu) : kifu;
  const pb = playerOf(shown, 'PB');
  const pw = playerOf(shown, 'PW');
  // Results carry which record they belong to (no stale boards when
  // the record swaps).
  const [got, setGot] = useState<{ kifu: string; frames?: KifuFrame[]; err?: string } | null>(null);
  const [at, setAt] = useState<number | null>(null);
  /* Action feedback, one place for both outcomes; lives in the footer
     rather than a toast since it only speaks when pressed. */
  const [note, setNote] = useState('');
  const say = (t: string) => { setNote(t); window.setTimeout(() => setNote(''), 2000); };
  const [playing, setPlaying] = useState(false);

  useEffect(() => {
    if (!shown.trim()) return;
    let alive = true;
    void api.previewKifu(shown)
      .then((frames) => { if (alive) { setGot({ kifu: shown, frames }); setAt(null); } })
      .catch((e) => {
        if (!alive) return;
        /* Unreadable records re-fetch from the server first: local
           records from the era of lost drawn-opening starts cannot
           replay, but the archive has the real thing. */
        if (onRefetch) { onRefetch(); return; }
        setGot({ kifu: shown, err: String(e) }); setAt(null);
      });
    return () => { alive = false; };
    // onRefetch is rebuilt every render; keep it out of the deps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shown]);

  const frames = got?.kifu === shown ? got.frames : undefined;
  const err = got?.kifu === shown ? got.err : undefined;
  const last = frames ? frames.length - 1 : 0;
  // Default to the last move (the final position comes first).
  const cur = Math.min(at ?? last, last);
  const f = frames?.[cur];
  const end = frames?.[last];
  /* The move list derives from the stored positions — GGF is never
     re-parsed in JS. Passes print as a word; this row is for reading,
     not re-pasting. */
  const moveList = frames ? frames.slice(1).map((k) => sqName(k.last)).join('') : '';

  /* Autoplay; stops itself at the end. The stop condition lives in
     the effect deps so the timer folds without a set-state-in-effect. */
  const running = playing && cur < last;
  useEffect(() => {
    if (!running) return;
    const id = window.setInterval(() => {
      setAt((a) => Math.min((a ?? 0) + 1, last));
    }, 450);
    return () => { window.clearInterval(id); };
  }, [running, last]);

  const seek = (n: number) => { setPlaying(false); setAt(Math.max(0, Math.min(n, last))); };

  return (
    <Overlay onClose={onClose}>
      <Modal title={title} width="var(--w-modal-wide)" onClose={onClose}
             /* Synchro: two boards per id. Tabs show numbers only; the
                player row below carries the colors. */
             band={parts && parts.length > 1 ? (
               <Segmented value={String(face)} onChange={(v) => setFace(Number(v))}
                          options={parts.map((_, i) => ({ value: String(i), label: `${i + 1} 面目` }))} />
             ) : undefined}
             actions={<>
               <Button size="field" onClick={onClose}>閉じる</Button>
               <span style={{
                 flex: 1, minWidth: 0, paddingLeft: 'var(--sp-3)',
                 fontSize: 'var(--fs-6)', color: 'var(--bad)',
                 overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
               }}>{note}</span>
               {/* Copy and save stay (study-only would remove the way
                   to hand records to people). Copy alone reports
                   success: clipboard writes change nothing visible,
                   unlike the save dialog closing. */}
               <Button size="field" disabled={!shown} onClick={() => {
                 void navigator.clipboard.writeText(shown)
                   .then(() => say('コピーしました'))
                   .catch(() => say('コピーできませんでした'));
               }}>棋譜をコピー</Button>
               {/* Silent by design: the closing dialog is the report —
                   the difference from copy above. */}
               <Button size="field" disabled={!shown}
                       onClick={() => void ggsApi.saveKifu(shown, 'kifu')
                         .catch((e) => say('保存できませんでした (' + e + ')'))}>
                 ファイルに保存
               </Button>
               <Button size="field" variant="primary" disabled={!frames}
                       onClick={() => { onStudy(shown); onClose(); }}>検討で開く</Button>
             </>}>
        {!shown.trim() && (
            <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>取り出しています…</span>
          )}
          {err && <span style={{ fontSize: 'var(--fs-5)', color: 'var(--bad)' }}>{err}</span>}

          {f && (
            <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'flex-start' }}>
              {/* Fix the height too: the board svg is height:100% and
                  an unsized container once rendered 260 as 185. */}
              <div style={{ width: 260, height: 260, flex: 'none' }}>
                <Board cells={f.cells as (0 | 1 | 2)[]} last={f.last} coords={false} disabled />
              </div>
              <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
                {/* Same disc-count component as everywhere else; a
                    hand-drawn copy with a literal white once sank into
                    the light background. */}
                {/* Discs and players on one line — separated, the eye
                    must join color, name and count. Bare move lists
                    show counts only. */}
                {pb || pw ? (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
                    {([['b', pb, f.black], ['w', pw, f.white]] as const).map(([c, n, cnt]) => (
                      <span key={c} style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
                        <StoneDot color={c} size={11} />
                        <span className="k-sel">{n || '?'}</span>
                        {me && n === me && (
                          <span style={{ color: 'var(--sub)', fontSize: 'var(--fs-6)' }}>自分</span>
                        )}
                        <span style={{
                          marginLeft: 'auto', fontSize: 'var(--fs-2)', fontWeight: 700,
                          fontVariantNumeric: 'tabular-nums',
                        }}>{cnt}</span>
                      </span>
                    ))}
                  </div>
                ) : (
                  <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
                    <ScoreRow black={f.black} white={f.white} />
                  </div>
                )}
                {/* Final result; static while scrubbing — how the game
                    ended matters at every move. */}
                {end && (
                  <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                    最終 <b style={{ color: 'var(--text)', fontWeight: 600 }}>
                      {end.black - end.white > 0 ? '+' : ''}{end.black - end.white}
                    </b>
                  </div>
                )}
                <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>この手</div>
                <div style={{ fontSize: 'var(--fs-1)', fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>
                  {cur === 0 ? '初期局面' : `${cur}. ${sqName(f.last)}`}
                </div>
                {/* Raw record text: shows what "copy" will hand over.
                    Both the move list and the GGF, readable one on top. */}
                <Raw label="着手列" tone="text" text={moveList} />
                {shown.startsWith('(;') && <Raw label="GGF" text={shown} scroll />}
              </div>
            </div>
          )}

        {f && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
            {/* Stepper row: the strip alone cannot step one move.
                Names match the study toolbar. */}
            <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
              <Button size="field" square title="最初へ" disabled={cur === 0} onClick={() => seek(0)}>|◀</Button>
              <Button size="field" square title="戻る" disabled={cur === 0} onClick={() => seek(cur - 1)}>◀</Button>
              <Button size="field" square title="進む" disabled={cur >= last} onClick={() => seek(cur + 1)}>▶</Button>
              <Button size="field" square title="最後へ" disabled={cur >= last} onClick={() => seek(last)}>▶|</Button>
              <Button size="field" square title={running ? '止める' : '自動再生'}
                      disabled={last === 0}
                      onClick={() => { if (running) { setPlaying(false); return; } if (cur >= last) setAt(0); setPlaying(true); }}>
                {running ? '❚❚' : '▶▶'}
              </Button>
              <span style={{ flex: 1 }} />
              <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                <b style={{ fontSize: 'var(--fs-1)', fontWeight: 600, color: 'var(--text)' }}>{cur}</b>
                {' '}/ {last} 手
              </span>
            </div>
            {/* The same strip as study, with its buttons suppressed —
                this overlay draws its own five above, and both would
                duplicate the controls (a MoveScrub side effect found
                on-device). */}
            <MoveScrub nav={false} plies={last} cursor={cur} onSeek={seek} />
          </div>
        )}

      </Modal>
    </Overlay>
  );
}

/** One raw-text band: a small heading over a monospace box; GGF is
 *  long, so it wraps. */
function Raw({ label, text, tone, scroll }: {
  label: string; text: string; tone?: 'text'; scroll?: boolean;
}) {
  if (!text) return null;
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', minHeight: 0 }}>
      <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)', letterSpacing: '.08em' }}>{label}</span>
      <div className={scroll ? 'k-scroll' : undefined} style={{
        maxHeight: scroll ? 60 : undefined,
        padding: '6px 10px', borderRadius: 'var(--r-2)', background: 'var(--card)',
        fontFamily: 'var(--ff-mono)', fontSize: 'var(--fs-6)', lineHeight: 1.6,
        color: tone === 'text' ? 'var(--text)' : 'var(--sub)', wordBreak: 'break-all',
      }}>{text}</div>
    </div>
  );
}

/** Square index -> a1 form; pass (null) renders as a word. */
const sqName = (sq: number | null): string =>
  sq == null ? 'パス' : 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);
