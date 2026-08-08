import { useState } from 'react';
import { LEVELS, SOLVE_MAX, clampLevels, type Levels } from '../state';
import { Select } from './primitives';

/* 強さの選び方。対局・検討・GGS で同じ形にするための共通部品。
 *
 * 別々に作っていたときは、対局がプリセットの一覧、GGS が数値入力と
 * 選び方そのものが違っていた。同じ 3 つの値を決めるのに操作が違うと、
 * 片方で覚えたことがもう片方で通じない。
 *
 * **プリセットを選んでいる間は 3 枠を出さない。** 中身は名前に書いてあるし、
 * 同じ数字を 2 か所に出すと、下の枠が触れる見た目なのに触ると別の状態
 * (カスタム) へ移ってしまう。
 */

const label = (l: typeof LEVELS[number]) =>
  `${l.name} — 深さ${l.depth} / 読切${l.solve}` + (l.band ? ` / 選択読み+${l.band}` : '');

const presetOf = (v: Levels): number | 'custom' => {
  const i = LEVELS.findIndex((l) => l.depth === v.depth && l.solve === v.solve && l.band === v.band);
  return i >= 0 ? i : 'custom';
};



export function Strength({ value, onChange }: { value: Levels; onChange: (v: Levels) => void }) {
  // 「カスタム」を選んだことを覚えておく。値だけから導くと、プリセットと
  // 同じ値のまま開けない — カスタムを選んでも値は変わらないので、次の描画で
  // またそのプリセットだと判定されて閉じてしまう
  const [picked, setPicked] = useState(false);
  const custom = picked || presetOf(value) === 'custom';

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
      <Select value={custom ? 'custom' : String(presetOf(value))}
              options={[...LEVELS.map((l, i) => [String(i), label(l)] as [string, string]),
                        ['custom', 'カスタム…']]}
              onChange={(s) => {
                if (s === 'custom') { setPicked(true); return; }   // 値は変えない
                setPicked(false);
                const l = LEVELS[+s];
                onChange({ depth: l.depth, solve: l.solve, band: l.band });
              }} />
      {/* **数字そのものを触らせる。** 36 段を摘みに載せると 1 段が数 px しか
          なく狙った値に止められないし、数字が脇の小さな文字に降格する。
          ここで決めるのは「いくつか」なので、数字が主役でなければならない。
          ±1 は隣のボタンで、飛ばすときは打ち込む (選択肢 36 個も出さない)。 */}
      {custom && (
        /* 設計 §4 は 深さ / 読切 / 選択読み を **3 列の格子**に並べ、
           それぞれ 10px のラベルの下に 28px の `Select` を置く。
           数値欄 + ステッパーに変えていた (規則 68 の撤回) が、ユーザーが
           絵のほうを正と判断したので戻した。3 つは同じ強さを決める 1 組
           なので、横に並ぶほうが「1 組」に見える。 */
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 'var(--sp-2)' }}>
          <Pick label="深さ" value={value.depth} min={1} max={SOLVE_MAX}
                onChange={(n) => onChange(clampLevels({ ...value, depth: n }))} />
          {/* 読切は深さ以上でなければならない。深さのほうが大きいと、中盤探索が
              終局を跨いで読むだけの区間ができる */}
          <Pick label="読切" value={value.solve} min={value.depth} max={SOLVE_MAX}
                onChange={(n) => onChange(clampLevels({ ...value, solve: n }))} />
          <Pick label="選択読み" value={value.band} min={0} max={12} zero="なし"
                onChange={(n) => onChange({ ...value, band: n })} />
        </div>
      )}
    </div>
  );
}

/** 数を 1 つ選ぶ。**選べる値だけを並べる** — 範囲外を打ち込ませて後から
 *  丸める形だと、読切が深さを下回る途中の状態が一瞬できる。
 *  0 に意味のある欄 (選択読み) は 0 の表示だけ言葉に替える。 */
function Pick({ label, value, min, max, zero, onChange }: {
  label: string; value: number; min: number; max: number;
  zero?: string; onChange: (n: number) => void;
}) {
  const options: [string, string][] = [];
  for (let n = min; n <= max; n++) options.push([String(n), zero && n === 0 ? zero : String(n)]);
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
      <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{label}</span>
      {/* 絵は 28px。欄の高さ (32px) ではなく、並ぶ 3 つで 1 組に見える大きさ */}
      <Select size="ctrl" value={String(Math.max(min, Math.min(max, value)))}
              options={options} onChange={(v) => onChange(+v)} />
    </label>
  );
}
