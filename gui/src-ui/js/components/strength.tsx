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

const range = (from: number, to: number) =>
  Array.from({ length: to - from + 1 }, (_, i) => [String(from + i), String(from + i)] as [string, string]);

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
      {custom && (
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 'var(--sp-2)' }}>
          <Num label="深さ" value={value.depth} opts={range(1, SOLVE_MAX)}
               onChange={(n) => onChange(clampLevels({ ...value, depth: n }))} />
          {/* 読切は深さ以上でなければならない。深さのほうが大きいと、中盤探索が
              終局を跨いで読むだけの区間ができる */}
          <Num label="読切" value={value.solve} opts={range(value.depth, SOLVE_MAX)}
               onChange={(n) => onChange(clampLevels({ ...value, solve: n }))} />
          <Num label="選択読み" value={value.band} opts={range(0, 12)}
               onChange={(n) => onChange({ ...value, band: n })} />
        </div>
      )}
    </div>
  );
}

function Num({ label, value, opts, onChange }: {
  label: string; value: number; opts: [string, string][]; onChange: (n: number) => void;
}) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{label}</span>
      <Select size="ctrl" value={String(value)} options={opts} onChange={(s) => onChange(+s)} />
    </label>
  );
}
