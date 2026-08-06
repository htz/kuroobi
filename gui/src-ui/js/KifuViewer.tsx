import { useEffect, useState } from 'react';
import { api, ggsApi, type KifuFrame } from './api';
import { Board } from './components/board';
import { MoveScrub } from './components/data';
import { Overlay } from './components/layout';
import { Button } from './components/primitives';

/* 棋譜を盤面で確かめる覆い (規則 71)。
 *
 * 棋譜そのもの (GGF や f5d6…) は人が読むものではないので、まず盤面を
 * 並べて見せる。そこから納得した上で、コピー・保存・検討へ渡す。
 *
 * **行き先ではなく Modal。** どの一覧からも開くので、見て閉じれば元の
 * 一覧に戻るのが自然。手を辿るのは検討と同じ「手数の帯」を使い回す
 * (規則 72 — 同じ動作に 2 つの部品を作らない)。
 */

export interface KifuViewerProps {
  title: string;
  /** GGF か着手列。空なら「取り出しています…」を出す。 */
  kifu: string;
  onClose: () => void;
  /** 検討で開く。 */
  onStudy: (kifu: string) => void;
}

export function KifuViewer({ title, kifu, onClose, onStudy }: KifuViewerProps) {
  // 取得結果はどの棋譜のものかを持たせる (棋譜が差し替わった瞬間に
  // 前の盤面が残らない)
  const [got, setGot] = useState<{ kifu: string; frames?: KifuFrame[]; err?: string } | null>(null);
  const [at, setAt] = useState<number | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!kifu.trim()) return;
    let alive = true;
    void api.previewKifu(kifu)
      .then((frames) => { if (alive) { setGot({ kifu, frames }); setAt(null); } })
      .catch((e) => { if (alive) { setGot({ kifu, err: String(e) }); setAt(null); } });
    return () => { alive = false; };
  }, [kifu]);

  const frames = got?.kifu === kifu ? got.frames : undefined;
  const err = got?.kifu === kifu ? got.err : undefined;
  const last = frames ? frames.length - 1 : 0;
  // 既定は最終手 (終局図をまず見たい)
  const cur = Math.min(at ?? last, last);
  const f = frames?.[cur];

  return (
    <Overlay onClose={onClose}>
      {/* 幅は 520px = 盤 248px + 情報列。wide は要らない */}
      <div role="dialog" aria-modal style={{
        width: 520, borderRadius: 'var(--r-4)', background: 'var(--card)',
        border: '1px solid var(--border)', boxShadow: 'var(--sh-2)',
        padding: 'var(--sp-5)', display: 'flex', flexDirection: 'column', gap: 'var(--sp-4)',
      }}>
        <div style={{ fontSize: 'var(--fs-3)', fontWeight: 600 }}>{title}</div>

        {!kifu.trim() && (
          <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>取り出しています…</span>
        )}
        {err && <span style={{ fontSize: 'var(--fs-5)', color: 'var(--bad)' }}>{err}</span>}

        {f && (
          <>
            <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'flex-start' }}>
              <div style={{ width: 248, flex: 'none' }}>
                <Board cells={f.cells as (0 | 1 | 2)[]} last={f.last} coords={false} disabled />
              </div>
              <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
                <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>この手</div>
                <div style={{ fontSize: 'var(--fs-1)', fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>
                  {cur === 0 ? '初期局面' : `${cur}. ${sqName(f.last)}`}
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)', fontSize: 'var(--fs-4)' }}>
                  <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                    <i style={{ width: 12, height: 12, borderRadius: '50%', background: 'var(--stone-black)',
                                border: '1px solid var(--stone-black-edge)' }} />
                    <b style={{ fontVariantNumeric: 'tabular-nums' }}>{f.black}</b>
                  </span>
                  <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                    <i style={{ width: 12, height: 12, borderRadius: '50%', background: '#f2f2f0' }} />
                    <b style={{ fontVariantNumeric: 'tabular-nums' }}>{f.white}</b>
                  </span>
                </div>
                <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                  {cur} / {last} 手
                </div>
              </div>
            </div>

            {/* 手を辿るのは検討と同じ帯。同じ動作に 2 つの部品を作らない */}
            <MoveScrub plies={last} cursor={cur} onSeek={setAt} />
          </>
        )}

        <div style={{ display: 'flex', gap: 'var(--sp-2)', alignItems: 'center' }}>
          <Button onClick={onClose}>閉じる</Button>
          <span style={{ flex: 1 }} />
          {/* コピーと保存を落とさない — 検討へ直行するだけだと、棋譜を人に
              渡す道が消える */}
          <Button disabled={!kifu} onClick={() => {
            void navigator.clipboard.writeText(kifu).catch(() => { /* 権限なし */ });
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1200);
          }}>{copied ? 'コピーしました' : '棋譜をコピー'}</Button>
          <Button disabled={!kifu}
                  onClick={() => void ggsApi.saveKifu(kifu, 'kifu').catch(() => {})}>
            ファイルに保存
          </Button>
          <Button variant="primary" disabled={!frames}
                  onClick={() => { onStudy(kifu); onClose(); }}>検討で開く</Button>
        </div>
      </div>
    </Overlay>
  );
}

/** 盤のマス番号 → a1 形式。パス (null) は「パス」。 */
const sqName = (sq: number | null): string =>
  sq == null ? 'パス' : 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);
