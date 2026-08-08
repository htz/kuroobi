import { useEffect, useState } from 'react';
import { api, ggsApi, type KifuFrame } from './api';
import { Board } from './components/board';
import { MoveScrub, ScoreRow } from './components/data';
import { Modal, Overlay } from './components/layout';
import { Button } from './components/primitives';

/* 棋譜を盤面で確かめる覆い (規則 71)。設計 `適用画面 GGS 2` §9。
 *
 * 棋譜そのもの (GGF や f5d6…) は人が読むものではないので、まず盤面を
 * 並べて見せる。そこから納得した上で、コピー・保存・検討へ渡す。
 *
 * **行き先ではなく Modal。** どの一覧からも開くので、見て閉じれば元の
 * 一覧に戻るのが自然。手を辿るのは検討と同じ「手数の帯」を使い回す
 * (規則 72 — 同じ動作に 2 つの部品を作らない)。
 *
 * **段は 4 つ。ヘッダと足を罫で切る** (絵の実測 — ヘッダ 52 / 本体 290 /
 * 送りと帯 176 / 足 59。左右の余白 20 と 14 はトークンに無いので
 * `--sp-5` 24 と `--sp-4` 16 に寄せた。要 push)。
 *
 * **絵にあって作れないもの** (`notes/design-sync-from-impl.md`):
 * 「黒 ▼6.0 / −4.2」の評価 — ビューアは棋譜を再生するだけで、GGS から
 * 取った棋譜には評価値が付いてこない。副題の「— 1 面目」も、同期対局の
 * 面番号を呼び出し側が持っていない。
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
  /* 押した結果の一言。**成功も失敗も同じ場所に出す** (規則 34 —
     出すのは失敗と、押したのに進まない理由)。押していないのに何か言うことは
     しないので、トーストではなく足の中に置く */
  const [note, setNote] = useState('');
  const say = (t: string) => { setNote(t); window.setTimeout(() => setNote(''), 2000); };
  const [playing, setPlaying] = useState(false);

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
  const end = frames?.[last];

  /* 自動再生。**最後まで来たら勝手に止まる** — 止める条件を効果の
     依存に入れておくと、状態を書き戻さずに (React Compiler の
     set-state-in-effect を踏まずに) 時計を畳める */
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
             actions={<>
               <Button size="field" onClick={onClose}>閉じる</Button>
               <span style={{
                 flex: 1, minWidth: 0, paddingLeft: 'var(--sp-3)',
                 fontSize: 'var(--fs-6)', color: 'var(--bad)',
                 overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
               }}>{note}</span>
               {/* コピーと保存を落とさない — 検討へ直行するだけだと、
                   棋譜を人に渡す道が消える */}
               <Button size="field" disabled={!kifu} onClick={() => {
                 void navigator.clipboard.writeText(kifu)
                   .then(() => say('コピーしました'))
                   .catch(() => say('コピーできませんでした'));
               }}>棋譜をコピー</Button>
               {/* **成功したときは何も言わない** (規則 34)。保存の窓が閉じた
                   こと自体が報せになっている */}
               <Button size="field" disabled={!kifu}
                       onClick={() => void ggsApi.saveKifu(kifu, 'kifu')
                         .catch((e) => say('保存できませんでした (' + e + ')'))}>
                 ファイルに保存
               </Button>
               <Button size="field" variant="primary" disabled={!frames}
                       onClick={() => { onStudy(kifu); onClose(); }}>検討で開く</Button>
             </>}>
        {!kifu.trim() && (
            <span style={{ fontSize: 'var(--fs-5)', color: 'var(--sub)' }}>取り出しています…</span>
          )}
          {err && <span style={{ fontSize: 'var(--fs-5)', color: 'var(--bad)' }}>{err}</span>}

          {f && (
            <div style={{ display: 'flex', gap: 'var(--sp-4)', alignItems: 'flex-start' }}>
              {/* **縦も決める。** Board の svg は `height:100%` なので、
                  高さの決まっていない器に入れると基準が無く、260 のつもりが
                  185 で描かれていた (絵の実測は 260 角) */}
              <div style={{ width: 260, height: 260, flex: 'none' }}>
                <Board cells={f.cells as (0 | 1 | 2)[]} last={f.last} coords={false} disabled />
              </div>
              <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
                {/* 石数は対局・検討・定石と同じ 1 行の部品を使う。ここだけ
                    手で丸を描いていて、白石が `#f2f2f0` のリテラルだったため
                    ライトで地に沈んでいた (規則 44・50) */}
                <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
                  <ScoreRow black={f.black} white={f.white} />
                </div>
                {/* 終局の石差。**辿っている途中でも動かない** — この対局が
                    どう終わったかは、どの手を見ていても知りたい情報 */}
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
                {/* 棋譜の生の字。**「棋譜をコピー」で何が渡るのかを見せる** —
                    見えないまま押させると、GGF なのか着手列なのか分からない */}
                <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)', minHeight: 0 }}>
                  <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)', letterSpacing: '.08em' }}>
                    {kifu.startsWith('(;') ? 'GGF' : '着手列'}
                  </span>
                  <div className="k-scroll" style={{
                    maxHeight: 92, overflowY: 'auto', fontFamily: 'var(--ff-mono)',
                    fontSize: 'var(--fs-7)', color: 'var(--sub)', lineHeight: 1.6,
                    wordBreak: 'break-all',
                  }}>{kifu}</div>
                </div>
              </div>
            </div>
          )}

        {f && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
            {/* 送りの行。**帯だけだと 1 手ずつ動かせない** (掴んで引くしかない)。
                名前は検討のツールバーと同じにする — 同じ動作に別の呼び名を
                作らない (規則 49) */}
            <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
              <Button size="field" square title="最初へ" disabled={cur === 0} onClick={() => seek(0)}>⏮</Button>
              <Button size="field" square title="戻る" disabled={cur === 0} onClick={() => seek(cur - 1)}>◀</Button>
              <Button size="field" square title="進む" disabled={cur >= last} onClick={() => seek(cur + 1)}>▶</Button>
              <Button size="field" square title="最後へ" disabled={cur >= last} onClick={() => seek(last)}>⏭</Button>
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
            {/* 手を辿るのは検討と同じ帯。同じ動作に 2 つの部品を作らない */}
            <MoveScrub plies={last} cursor={cur} onSeek={seek} />
          </div>
        )}

      </Modal>
    </Overlay>
  );
}

/** 盤のマス番号 → a1 形式。パス (null) は「パス」。 */
const sqName = (sq: number | null): string =>
  sq == null ? 'パス' : 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);
