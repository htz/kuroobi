import { useCallback, useEffect, useState } from 'react';
import { api } from './api';
import { sqName } from './adapt';
import type { BookNode } from './types';
import { Board, type EvalInfo } from './components/board';
import { PlayerRow } from './components/data';
import { Section } from './components/layout';
import { Button } from './components/primitives';

/* 定石を眺める。
 *
 * 対局・検討とは別の状態を持つ — 定石を辿っている最中に対局の盤が動くと、
 * どちらを見ているのか分からなくなる。手順はこちらだけが覚え、盤はその手順を
 * バックエンドで指し直したものを受け取る (JS 側に盤の規則を写さない)。
 */

export interface BookBrowse {
  /** 初期局面からの手順。 */
  line: number[];
  node: BookNode | null;
  err: string;
  push: (sq: number) => void;
  back: () => void;
  reset: () => void;
  /** 手順を文字列 ("f5d6") で指定して開く。動作確認用の入口。 */
  open: (kifu: string) => void;
}

export function useBookBrowse(on: boolean): BookBrowse {
  const [line, setLine] = useState<number[]>([]);
  const [node, setNode] = useState<BookNode | null>(null);
  const [err, setErr] = useState('');

  useEffect(() => {
    if (!on) return;
    let alive = true;
    void api.bookNode(line.map(sqName).join(''))
      .then((n) => { if (alive) { setNode(n); setErr(''); } })
      .catch((e) => { if (alive) setErr('' + e); });
    return () => { alive = false; };
  }, [on, line]);

  return {
    line, node, err,
    push: useCallback((sq: number) => setLine((l) => [...l, sq]), []),
    back: useCallback(() => setLine((l) => l.slice(0, -1)), []),
    reset: useCallback(() => setLine([]), []),
    open: useCallback((kifu: string) => setLine(parseLine(kifu)), []),
  };
}

/** 盤。定石にある手だけを打てる — 定石の外へ出る道はここでは要らない。 */
export function BookPane({ b, coords, grain, flip }: {
  b: BookBrowse; coords: boolean; grain: boolean; flip: boolean;
}) {
  const n = b.node;
  // 候補は評価値のマスとして出す。値の高いものが金の輪 (best)
  const evals: Record<number, EvalInfo> = {};
  n?.moves.forEach((m, i) => {
    evals[m.pos] = { score: m.value, src: { book: true }, best: i === 0 };
  });
  return (
    <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', padding: '0 var(--sp-4)' }}>
      <PlayerRow color="b" name="黒" discs={n?.black ?? 2} active={n?.player === 'black'} />
      <div style={{ flex: 1, minHeight: 0, display: 'grid', placeItems: 'center', padding: 'var(--sp-2) 0' }}>
        <div style={{ height: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
          <Board cells={(n?.cells ?? INITIAL) as (0 | 1 | 2)[]}
                 legal={n?.moves.map((m) => m.pos) ?? []}
                 last={b.line.length ? b.line[b.line.length - 1] : null}
                 evals={evals} coords={coords} grain={grain} flip={flip}
                 onPlay={(sq) => { if (n?.moves.some((m) => m.pos === sq)) b.push(sq); }} />
        </div>
      </div>
      <PlayerRow color="w" name="白" discs={n?.white ?? 2} active={n?.player === 'white'}
                 meta={b.err || (n && n.moves.length === 0 ? 'この局面から先は定石にありません' : undefined)} />
    </div>
  );
}

/** 候補手の一覧。盤の上のマスと同じ順 (値の高い順) で並べる。 */
export function BookDock({ b }: { b: BookBrowse }) {
  const n = b.node;
  return (
    <>
      <Section title="手順" aside={<span>{b.line.length} 手</span>}>
        <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.9, wordBreak: 'break-all' }}>
          {b.line.length ? b.line.map(sqName).join(' ') : '初期局面'}
        </div>
        <div style={{ display: 'flex', gap: 'var(--sp-2)' }}>
          <Button size="chip" disabled={!b.line.length} onClick={b.back}>戻る</Button>
          <Button size="chip" disabled={!b.line.length} onClick={b.reset}>最初へ</Button>
        </div>
        {n?.learned && (
          // 元の定石ファイルの値か、自分の対局から書き戻した値かで重みが違う。
          // 学習ぶんは数局しか根拠が無いこともあるので、その断りを出す
          // (棋譜の出所欄に出す「定石·学」と同じ意味)
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--gold)' }}>
            定石·学 — 実戦から学習した値を含みます
          </span>
        )}
        {n && n.learned_size > 0 && (
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
            学習で増えた局面 {n.learned_size.toLocaleString()}
          </span>
        )}
      </Section>

      <Section title="候補手" aside={n ? <span>{n.moves.length}</span> : undefined}>
        {n && n.moves.length === 0 && (
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
            この局面から先は定石にありません。
          </span>
        )}
        {n?.moves.map((m, i) => (
          <button key={m.pos} type="button" className="k-row" onClick={() => b.push(m.pos)}
                  style={{
                    display: 'flex', alignItems: 'center', gap: 'var(--sp-3)',
                    border: 0, background: 'transparent', cursor: 'pointer',
                    padding: 'var(--sp-2) var(--sp-2)', borderRadius: 'var(--r-2)',
                    fontSize: 'var(--fs-6)', color: 'var(--text)', textAlign: 'left',
                  }}>
            <span style={{ width: 26, color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>{i + 1}</span>
            <span style={{ width: 34, fontWeight: 600 }}>{sqName(m.pos)}</span>
            {/* 値は手番から見た石差。符号を落とすと、白番のときに読み違える */}
            <span style={{ width: 56, textAlign: 'right', fontVariantNumeric: 'tabular-nums',
                           color: i === 0 ? 'var(--gold)' : 'var(--text)' }}>
              {m.value > 0 ? '+' : ''}{m.value.toFixed(1)}
            </span>
            <span style={{ marginLeft: 'auto', color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
              {m.games} 局
            </span>
          </button>
        ))}
      </Section>
    </>
  );
}

const INITIAL = Array.from({ length: 64 }, (_, i) =>
  i === 3 * 8 + 4 || i === 4 * 8 + 3 ? 1 : i === 3 * 8 + 3 || i === 4 * 8 + 4 ? 2 : 0);

/** "f5d6" を マス番号の並びにする。読めない字は捨てる。 */
function parseLine(kifu: string): number[] {
  const out: number[] = [];
  const t = kifu.toLowerCase();
  for (let i = 0; i + 1 < t.length; i += 2) {
    const f = 'abcdefgh'.indexOf(t[i]);
    const r = +t[i + 1] - 1;
    if (f >= 0 && r >= 0 && r < 8) out.push(f * 8 + r);
  }
  return out;
}
