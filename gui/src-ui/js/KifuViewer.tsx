import { useEffect, useState } from 'react';
import { api, ggsApi, type KifuFrame } from './api';
import { Board } from './components/board';
import { MoveScrub, ScoreRow, StoneDot } from './components/data';
import { Modal, Overlay } from './components/layout';
import { Segmented } from './components/primitives';
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
  /** 手元の棋譜が読めなかったときの逃げ道 (サーバーの書庫から取り直す)。 */
  onRefetch?: () => void;
  /** 同じ番号に入っていた全局。**同期対局は 2 面**あり、片面だけ見せると
   *  「もう 1 局はどこへ行った」になる。2 つ以上あるときだけ帯を出す。 */
  parts?: string[];
  /** 自分のログイン名。**面ごとに自分の色が逆**になるので、どちらを見て
   *  いるのかは色でしか分からない。 */
  me?: string;
}

/** GGF から対局者名を拾う。無ければ空。 */
function playerOf(ggf: string, tag: 'PB' | 'PW'): string {
  return new RegExp(tag + '\\[([^\\]]*)\\]').exec(ggf)?.[1] ?? '';
}

export function KifuViewer({ title, kifu, onClose, onStudy, onRefetch, parts, me }: KifuViewerProps) {
  /* どの面を見ているか。**別の対局に変わったら 1 面目に戻す。**
     effect で戻すと描画が二度走るので、持ち主 (棋譜そのもの) を鍵にして
     描画中に判ずる。 */
  const [pick, setPick] = useState({ key: '', face: 0 });
  const key = parts?.[0] ?? '';
  const face = pick.key === key ? pick.face : 0;
  const setFace = (i: number) => setPick({ key, face: i });
  const shown = parts && parts.length > 1 ? (parts[face] ?? kifu) : kifu;
  const pb = playerOf(shown, 'PB');
  const pw = playerOf(shown, 'PW');
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
    if (!shown.trim()) return;
    let alive = true;
    void api.previewKifu(shown)
      .then((frames) => { if (alive) { setGot({ kifu: shown, frames }); setAt(null); } })
      .catch((e) => {
        if (!alive) return;
        /* **読めない棋譜は、まずサーバーへ聞き直す。** 手元の記録は
           抽選オープニングの開始局面を取り落としていた時期があり、その
           ぶんは再生できない。書庫には正しい棋譜が残っている。 */
        if (onRefetch) { onRefetch(); return; }
        setGot({ kifu: shown, err: String(e) }); setAt(null);
      });
    return () => { alive = false; };
    // onRefetch は毎描画で作り直されるので依存に入れない
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shown]);

  const frames = got?.kifu === shown ? got.frames : undefined;
  const err = got?.kifu === shown ? got.err : undefined;
  const last = frames ? frames.length - 1 : 0;
  // 既定は最終手 (終局図をまず見たい)
  const cur = Math.min(at ?? last, last);
  const f = frames?.[cur];
  const end = frames?.[last];
  /* 着手列は控えの局面から作る。GGF を画面側で解き直さない (盤の規則を
     JS に写さない、という他の画面と同じ考え方)。パスは座標を持たないので
     「パス」と書く — 貼り直せる字ではないが、ここは読むための行 */
  const moveList = frames ? frames.slice(1).map((k) => sqName(k.last)).join('') : '';

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
             /* 同期対局は 1 つの番号に 2 面。**帯は番号だけ。** どちらの色
                だったかは下の対局者の行が持つ (自分の対局と他人の対局で
                見た目を変えない) */
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
               {/* コピーと保存を落とさない — 検討へ直行するだけだと、
                   棋譜を人に渡す道が消える。
                   **コピーだけは成功も言う。** 規則 34 は「成功したときは
                   何も言わない」だが、それが成り立つのは*画面か OS に
                   変化が出る*操作 (下の保存は窓が閉じる)。クリップボードは
                   どこにも変化が出ないので、黙ると押せたのかどうかが
                   分からない。**この線引きは規則に無いので投げてある** */}
               <Button size="field" disabled={!shown} onClick={() => {
                 void navigator.clipboard.writeText(shown)
                   .then(() => say('コピーしました'))
                   .catch(() => say('コピーできませんでした'));
               }}>棋譜をコピー</Button>
               {/* こちらは何も言わない (規則 34)。保存の窓が閉じたこと自体が
                   報せになっている — 上のコピーとの違いはそこ */}
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
                {/* **石数と対局者は 1 行にまとめる。** 別々に置くと、色と
                    名前と数の 3 つを目で突き合わせることになる。名前が
                    無い棋譜 (着手列だけ) では石数だけを出す */}
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
                    見えないまま押させると、GGF なのか着手列なのか分からない。
                    絵 (§9) は**着手列と GGF を両方**出す。GGF は人が読むもの
                    ではないので、読める形の着手列を上に置く。着手列は控えの
                    局面から作る (GGF を自分で解き直さない) */}
                <Raw label="着手列" tone="text" text={moveList} />
                {shown.startsWith('(;') && <Raw label="GGF" text={shown} scroll />}
              </div>
            </div>
          )}

        {f && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
            {/* 送りの行。**帯だけだと 1 手ずつ動かせない** (掴んで引くしかない)。
                名前は検討のツールバーと同じにする — 同じ動作に別の呼び名を
                作らない (規則 49) */}
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
            {/* 手を辿るのは検討と同じ帯。同じ動作に 2 つの部品を作らない。
                **釦は出させない** — この覆いは絵 (§9) のとおり帯の上に
                自前の 5 つを並べており、帯にも出すと同じ操作が 2 列になる
                (`7279c1f` で MoveScrub に釦を足した副作用。実機で見つけた) */}
            <MoveScrub nav={false} plies={last} cursor={cur} onSeek={seek} />
          </div>
        )}

      </Modal>
    </Overlay>
  );
}

/** 棋譜の生の字を 1 段。絵は 見出し (fs-7 / --sub) の下に等幅の枠
 *  (角丸 --r-2 / 地 --card / 余白 6・10)。GGF は長いので巻けるようにする。 */
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

/** 盤のマス番号 → a1 形式。パス (null) は「パス」。 */
const sqName = (sq: number | null): string =>
  sq == null ? 'パス' : 'abcdefgh'[Math.floor(sq / 8)] + (sq % 8 + 1);
