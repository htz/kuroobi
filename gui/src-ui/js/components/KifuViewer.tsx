// 棋譜を盤面で確かめる画面。
//
// 棋譜そのもの (GGF や f5d6…) は人が読むものではないので、まず盤面を
// 並べて見せる。そこから納得した上で、コピー・保存・検討へ渡す。
import { useEffect, useState } from 'react';
import { api } from '../api';
import type { KifuFrame } from '../api';
import { Board } from './Board';
import { Icon } from './Icons';
import { Modal } from './Modal';

export interface KifuViewerProps {
  title: string;
  /** GGF か着手列。空なら「取得中」を出す。 */
  kifu: string;
  onClose: () => void;
  /** 検討画面へ送る。 */
  onOpenStudy: (kifu: string) => void;
  /** ファイルに保存する (呼ぶ側が保存先を持つ)。 */
  onSave?: (kifu: string) => void;
}

export function KifuViewer({ title, kifu, onClose, onOpenStudy, onSave }: KifuViewerProps) {
  // 取得結果はどの棋譜のものかを持たせ、表示は描画時に導く。
  // (棋譜が差し替わった瞬間に前の盤面が残らない)
  const [got, setGot] = useState<{ kifu: string; frames?: KifuFrame[]; err?: string } | null>(null);
  const [at, setAt] = useState<number | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!kifu.trim()) return;
    let alive = true;
    api.previewKifu(kifu)
      .then((frames) => { if (alive) { setGot({ kifu, frames }); setAt(null); } })
      .catch((e) => { if (alive) { setGot({ kifu, err: String(e) }); setAt(null); } });
    return () => { alive = false; };
  }, [kifu]);

  const frames = got?.kifu === kifu ? got.frames : undefined;
  const err = got?.kifu === kifu ? got.err : undefined;
  const last = frames ? frames.length - 1 : 0;
  // 既定は最終手 (終局の盤面をまず見たい)
  const cur = Math.min(at ?? last, last);
  const f = frames?.[cur];
  const go = (n: number) => setAt(Math.max(0, Math.min(last, n)));

  return (
    <Modal title={title} onClose={onClose} wide
           actions={<>
             <button className="btn ghost" onClick={onClose}>閉じる</button>
             <span className="spacer" />
             <button className="btn" disabled={!kifu} onClick={async () => {
               try { await navigator.clipboard.writeText(kifu); } catch { /* 権限なし */ }
               setCopied(true);
               window.setTimeout(() => setCopied(false), 1200);
             }}>{copied ? 'コピーしました' : '棋譜をコピー'}</button>
             {onSave && (
               <button className="btn" disabled={!kifu}
                       onClick={() => onSave(kifu)}>ファイルに保存</button>
             )}
             <button className="btn primary" disabled={!frames}
                     onClick={() => { onOpenStudy(kifu); onClose(); }}>検討で開く</button>
           </>}>
      {!kifu.trim() && <p className="hint">取得しています…</p>}
      {err && <p className="warn-line"><Icon name="alert" size={14} />{err}</p>}
      {f && (
        <div className="kifu-view">
          <div className="kv-board">
            <Board cells={f.cells} last={f.last ?? null} disabled />
          </div>
          <div className="kv-bar">
            <span className="kv-score">
              <i className="disc b" />{f.black}
              <span className="vs">—</span>
              {f.white}<i className="disc w" />
            </span>
            <span className="spacer" />
            <span className="kv-at">{cur} / {last} 手</span>
          </div>
          <div className="kv-nav">
            <button className="btn small" onClick={() => go(0)} disabled={cur === 0}>最初</button>
            <button className="btn small" onClick={() => go(cur - 1)} disabled={cur === 0}>前</button>
            <input type="range" min={0} max={last} value={cur}
                   onChange={(e) => go(+e.target.value)} aria-label="手数" />
            <button className="btn small" onClick={() => go(cur + 1)} disabled={cur === last}>次</button>
            <button className="btn small" onClick={() => go(last)} disabled={cur === last}>最後</button>
          </div>
        </div>
      )}
    </Modal>
  );
}
