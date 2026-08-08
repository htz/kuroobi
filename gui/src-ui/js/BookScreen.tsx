import { useCallback, useEffect, useMemo, useState } from 'react';
import { api } from './api';
import { sqName } from './adapt';
import type { BookNode } from './types';
import { Board, type EvalInfo } from './components/board';
import { ScoreRow } from './components/data';
import { Empty, EmptyState, List, Section, TableHead } from './components/layout';
import { Button } from './components/primitives';

/* 定石を眺める。
 *
 * 対局・検討とは別の状態を持つ — 定石を辿っている最中に対局の盤が動くと、
 * どちらを見ているのか分からなくなる。手順はこちらだけが覚え、盤はその手順を
 * バックエンドで指し直したものを受け取る (JS 側に盤の規則を写さない)。
 *
 * 一覧は木にしてある。候補手を 1 段だけ出しても「その先どう分かれるか」が
 * 見えず、枝の広がりを見るには盤を進めては戻る往復が要る。
 */

/** 手順を「f5d6」の形にしたもの。木の節を指す鍵として使う。 */
const keyOf = (line: number[]) => line.map(sqName).join('');

export interface BookBrowse {
  /** 初期局面からの手順。 */
  line: number[];
  /** いま見ている局面。 */
  node: BookNode | null;
  /** 取ってきた節 (鍵は手順の文字列)。木を描くのに使う。 */
  nodes: Record<string, BookNode>;
  /** 開いている節。 */
  open: ReadonlySet<string>;
  toggle: (key: string) => void;
  err: string;
  push: (sq: number) => void;
  back: () => void;
  reset: () => void;
  /** 手順を丸ごと入れ替える。木の行から飛ぶときと、動作確認用の入口。 */
  goto: (kifu: string) => void;
}

export function useBookBrowse(on: boolean): BookBrowse {
  const [line, setLine] = useState<number[]>([]);
  const [nodes, setNodes] = useState<Record<string, BookNode>>({});
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set<string>());
  const [err, setErr] = useState('');

  const root = keyOf(line);

  /* 要るのは「いまの節」と「開いている節」、それに**画面に出ている行の子**。
   * 子まで取るのは出所 (定石ファイル / 実戦学習) の列を出すため — 節を
   * 取ってこないと分からないので、開くまで空欄にすると列の意味が無い。 */
  const want = useMemo(() => {
    const need = new Set<string>([root]);
    const addKids = (k: string) => {
      const n = nodes[k];
      if (!n) return;
      for (const m of n.moves) need.add(k + sqName(m.pos));
    };
    addKids(root);
    for (const k of open) {
      need.add(k);
      addKids(k);
    }
    return [...need].filter((k) => !(k in nodes));
  }, [root, open, nodes]);

  useEffect(() => {
    if (!on || want.length === 0) return;
    let alive = true;
    void Promise.all(want.map((k) => api.bookNode(k).then((n) => [k, n] as const)))
      .then((pairs) => {
        if (!alive) return;
        setNodes((prev) => ({ ...prev, ...Object.fromEntries(pairs) }));
        setErr('');
      })
      .catch((e) => { if (alive) setErr('' + e); });
    return () => { alive = false; };
  }, [on, want]);

  const toggle = useCallback((key: string) => {
    setOpen((prev) => {
      const next = new Set(prev);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }, []);

  return {
    line,
    node: nodes[root] ?? null,
    nodes,
    open,
    toggle,
    err,
    push: useCallback((sq: number) => setLine((l) => [...l, sq]), []),
    back: useCallback(() => setLine((l) => l.slice(0, -1)), []),
    reset: useCallback(() => setLine([]), []),
    goto: useCallback((kifu: string) => setLine(parseLine(kifu)), []),
  };
}

/** 盤。定石にある手だけを打てる — 定石の外へ出る道はここでは要らない。 */
export function BookPane({ b, coords, grain, flip, onSettings }: {
  b: BookBrowse; coords: boolean; grain: boolean; flip: boolean;
  /** 定石ファイルが無いときの直し方への入口。 */
  onSettings: () => void;
}) {
  const n = b.node;
  /* 定石そのものが無いときは、空の盤を出しても嘘になる。
   * 「この局面から先は定石にありません」と読めてしまい、局面のせいだと
   * 思わせる。押せない状態と直し方は同時に出す (デザイン規則 61)。 */
  if (n && n.size === 0) {
    return (
      <EmptyState title="定石がありません"
                  body="定石ファイルを読み込めていません。設定から場所を指定できます。"
                  actions={<Button size="field" variant="primary" onClick={onSettings}>設定を開く</Button>} />
    );
  }
  // 候補は評価値のマスとして出す。値の高いものが金の輪 (best)
  const evals: Record<number, EvalInfo> = {};
  n?.moves.forEach((m, i) => {
    evals[m.pos] = { score: m.value, src: { book: true }, best: i === 0 };
  });
  return (
    // 左右の余白は中の要素が持つ (石数の行の罫を端まで届かせる)。
    // minWidth:0 が要る — 横並びの中に置くと、既定の auto では中身 (盤の
    // 固有幅) より縮まず、隣の列を押し出す
    <div style={{ flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column' }}>
      {/* 行に minmax(0,1fr) を入れて**高さを確定させる**。既定の auto だと
              行の高さが中身で決まり、中の height:100% の基準が定まらない
              (基準が無いと auto 扱いになり、svg が固有の 880px で描かれて
              下へはみ出す)。はみ出しは全部下に出るので、石数の行と手数の帯に
              重なって初めて気付く (規則 77) */}
          <div style={{
            flex: 1, minHeight: 0, display: 'grid', placeItems: 'center',
            gridTemplateRows: 'minmax(0, 1fr)', gridTemplateColumns: 'minmax(0, 1fr)',
            padding: 'var(--sp-2) var(--sp-4)',
          }}>
        {/* maxHeight も要る。器が横長のときは maxWidth が効くが、**器が低く
                  なると grid の行が中身の大きさで決まり**、svg が固有の大きさ
                  (880px) で描かれて下へはみ出す。はみ出しは全部下に出るので
                  石数の行と手数の帯に重なって初めて気付く (規則 77) */}
              <div style={{ height: '100%', maxHeight: '100%', aspectRatio: '1 / 1', maxWidth: '100%' }}>
          <Board cells={(n?.cells ?? INITIAL) as (0 | 1 | 2)[]}
                 legal={n?.moves.map((m) => m.pos) ?? []}
                 last={b.line.length ? b.line[b.line.length - 1] : null}
                 evals={evals} coords={coords} grain={grain} flip={flip}
                 onPlay={(sq) => { if (n?.moves.some((m) => m.pos === sq)) b.push(sq); }} />
        </div>
      </div>
      {/* 対局・検討と同じ 1 行。画面ごとに石数の置き場所が変わると、
          同じものを探すのに毎回目が迷う */}
      <ScoreRow black={n?.black ?? 2} white={n?.white ?? 2}
                turn={n?.player === 'white' ? 'w' : 'b'}
                meta={b.err || (n && n.moves.length === 0 ? 'この局面から先は定石にありません' : undefined)} />
    </div>
  );
}

/** 木の 1 行ぶん。 */
interface Row {
  key: string;
  depth: number;
  name: string;
  value: number;
  games: number;
  /** その手を指した先の節。取れていなければ undefined */
  child?: BookNode;
}

/** いまの節から下を、開いている枝だけ展開して並べる。 */
function rowsOf(b: BookBrowse, key: string, depth: number, out: Row[]): void {
  const n = b.nodes[key];
  if (!n) return;
  for (const m of n.moves) {
    const k = key + sqName(m.pos);
    out.push({ key: k, depth, name: sqName(m.pos), value: m.value, games: m.games, child: b.nodes[k] });
    if (b.open.has(k)) rowsOf(b, k, depth + 1, out);
  }
}

/** 手順と、その先の木。 */
export function BookDock({ b, decimals = 1 }: { b: BookBrowse; decimals?: number }) {
  const n = b.node;

  return (
    <>
      <Section title="この局面" aside={<span>{b.line.length} 手</span>}>
        <div style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)', lineHeight: 1.9, wordBreak: 'break-all' }}>
          {b.line.length ? b.line.map(sqName).join(' ') : '初期局面'}
        </div>
        {n?.learned && (
          // 元の定石ファイルの値か、自分の対局から書き戻した値かで重みが違う。
          // 学習ぶんは数局しか根拠が無いこともあるので、その断りを出す
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--gold)' }}>
            定石·学 — 実戦から学習した値を含みます
          </span>
        )}
      </Section>

      {/* 木は左の列が持つ (設計 §7)。ここは**次の 1 手だけ**を表にする。
          同じ一覧を 2 か所に出すと、どちらが本物か分からなくなる */}
      <Section title="次の手" aside={n ? <span>{n.moves.length}</span> : undefined}>
        {n && n.moves.length === 0 ? (
          <span style={{ fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
            この局面から先は定石にありません。
          </span>
        ) : (
          <List>
            {n?.moves.map((m) => (
              <button key={m.pos} type="button" className="k-row" onClick={() => b.push(m.pos)}
                      title="押すとこの手へ進む"
                      style={{
                        display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                        height: 'var(--h-row)', border: 0, background: 'transparent',
                        borderBottom: '1px solid var(--border-weak)', borderRadius: 'var(--r-2)',
                        padding: '0 var(--sp-1)', fontSize: 'var(--fs-6)',
                        color: 'var(--text)', textAlign: 'left', cursor: 'pointer',
                      }}>
                <span style={{ flex: 1, fontWeight: 600 }}>{sqName(m.pos)}</span>
                <span style={{ width: 44, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {m.value > 0 ? '+' : ''}{m.value.toFixed(decimals)}
                </span>
                <span style={{ width: 58, textAlign: 'right', fontSize: 'var(--fs-7)',
                               color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
                  {m.games.toLocaleString()}
                </span>
              </button>
            ))}
          </List>
        )}
      </Section>
    </>
  );
}

/* 左の列 — 定石の木。設計 §7 は 269px の独立した列で、見出しに
 * 手順 / 評価 / 出現 の 3 つを置き、24px の行を字下げで重ねる。
 * ドックに入れていたときは「この局面」と同じ枠に積まれ、枝の広がりが
 * 読めなかった。
 *
 * **絵は左メニューの無い独立ウィンドウを前提にしている。** 主画面では
 * 左メニュー 208 + この列 269 + ドック 290 が乗るので、盤に残るのは
 * 473px (絵は 560px)。それでも枝が読めるほうが効くと判断した。 */
export function BookTree({ b, decimals = 1 }: { b: BookBrowse; decimals?: number }) {
  const root = keyOf(b.line);
  const rows: Row[] = [];
  rowsOf(b, root, 0, rows);

  return (
    <div style={{
      width: 'var(--w-book-tree)', flex: 'none', minHeight: 0,
      borderRight: '1px solid var(--border)', display: 'flex', flexDirection: 'column',
    }}>
      {/* 列の見出し。節の見出し (Section) と同じ 20px + 1px 罫だが、
          ここは列名なので右の 2 つを数字の幅に合わせて右揃えにする */}
      <TableHead>
        <span style={{ flex: 1 }}>手順</span>
        <span style={{ width: 44, textAlign: 'right' }}>評価</span>
        <span style={{ width: 58, textAlign: 'right' }}>出現</span>
      </TableHead>
      <div className="k-scroll" style={{ flex: 1, minHeight: 0, padding: '0 var(--sp-3)' }}>
        {rows.length === 0 && <Empty>この局面から先は定石にありません。</Empty>}
        {rows.map((r) => (
          <BookRow key={r.key} r={r} open={b.open.has(r.key)} decimals={decimals}
                   onToggle={() => b.toggle(r.key)}
                   onGo={() => b.goto(r.key)} />
        ))}
      </div>
    </div>
  );
}


function BookRow({ r, open, onToggle, onGo, decimals = 1 }: {
  r: Row; open: boolean; onToggle: () => void; onGo: () => void; decimals?: number;
}) {
  // 先があるかは節を取るまで分からない。取れていて 0 手なら三角を出さない
  const leaf = r.child && r.child.moves.length === 0;
  return (
    // 行の本体と三角は兄弟にする (button の中に button は置けない)
    // 字下げは設計の実測どおり 1 段 16px。12px だと 3 段目で親と子の頭が
    // 揃って見え、木として読めない。行の下罫も設計にある — 24px の行を
    // 罫なしで積むと、字下げの段差だけでは行の切れ目が分からない
    <div className="k-row" style={{
      display: 'flex', alignItems: 'center', height: 'var(--h-row)',
      paddingLeft: r.depth * 16, borderRadius: 'var(--r-2)',
      borderBottom: '1px solid var(--border-weak)',
    }}>
      {/* 三角は当たりを行の高さいっぱいに取る。16px 角に fs-7 の記号だと
          押す場所が分からないうえ、外しやすい。**記号自体も大きくする** —
          当たりが足りていても、絵が小さいと「押せる」と思われない */}
      {leaf ? (
        <span style={{ width: 22, flex: 'none' }} />
      ) : (
        <button type="button" className="k-press" onClick={onToggle}
                title={open ? '閉じる' : '先を開く'} aria-label={open ? '閉じる' : '先を開く'}
                style={{
                  width: 22, height: 'var(--h-row)', flex: 'none', border: 0, padding: 0,
                  background: 'transparent', color: 'var(--sub)',
                  fontSize: 'var(--fs-3)', lineHeight: 1, borderRadius: 'var(--r-1)',
                }}>{open ? '▾' : '▸'}</button>
      )}
      {/* **状態の層を付ける。**隣の三角は `k-press` で光るのに、名前のほうは
          何も付いておらず、押せるのに触れても変わらなかった */}
      <button type="button" onClick={onGo} className="k-row"
              title="押すとこの局面へ"
              style={{
                flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 'var(--sp-2)',
                border: 0, background: 'transparent', padding: '0 var(--sp-1)',
                fontSize: 'var(--fs-6)', color: 'var(--text)', textAlign: 'left',
              }}>
        <span style={{ width: 30, fontWeight: 600 }}>{r.name}</span>
        <span style={{ width: 44, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
          {r.value > 0 ? '+' : ''}{r.value.toFixed(decimals)}
        </span>
        {/* 桁区切りを入れる。5 桁を超える数を素で並べると桁が読めない
            (設計の絵も 12,480 の形。下の帯の局面数も同じ書き方) */}
        <span style={{ width: 58, textAlign: 'right', fontSize: 'var(--fs-7)',
                       color: 'var(--sub)', fontVariantNumeric: 'tabular-nums' }}>
          {r.games.toLocaleString()}
        </span>
        {/* 出所。実戦から書き戻した枝は色でも分かるようにする */}
        <span style={{ marginLeft: 'auto', fontSize: 'var(--fs-7)',
                       color: r.child?.learned ? 'var(--gold)' : 'var(--sub)' }}>
          {r.child ? (r.child.learned ? '定石·学' : '定石') : ''}
        </span>
      </button>
    </div>
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
