import { useState } from 'react';
import { LEVELS, SOLVE_MAX, clampLevels, type Levels } from '../state';
import { Select, Slider } from './primitives';

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
      {/* 3 つとも連続した数なので摘みで選ぶ (規則 68)。36 個の選択肢を
          Select で出すのは操作が重い。**制限は選択肢を減らさず min で表す** —
          読切の下限が深さに追随するので、触れる範囲そのものが答えになる */}
      {custom && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
          <Num label="深さ" value={value.depth} min={1} max={SOLVE_MAX}
               onChange={(n) => onChange(clampLevels({ ...value, depth: n }))} />
          {/* 読切は深さ以上でなければならない。深さのほうが大きいと、中盤探索が
              終局を跨いで読むだけの区間ができる */}
          <Num label="読切" value={value.solve} min={value.depth} max={SOLVE_MAX}
               onChange={(n) => onChange(clampLevels({ ...value, solve: n }))} />
          <Num label="選択読み" value={value.band} min={0} max={12} zero="なし"
               onChange={(n) => onChange({ ...value, band: n })} />
        </div>
      )}
    </div>
  );
}

/** 摘みで選ぶ 1 つの数。**現在値は摘みの脇に数字で出す** — 摘みの位置
 *  だけでは 22 なのか 23 なのかが読めない。 */
function Num({ label, value, min, max, zero, onChange }: {
  label: string; value: number; min: number; max: number;
  /** 0 のときに数字の代わりに出す言葉 (選択読みの「なし」)。 */
  zero?: string;
  onChange: (n: number) => void;
}) {
  return (
    <label style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-3)' }}>
      <span style={{ width: 'var(--w-label)', flex: 'none', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        {label}
      </span>
      <span style={{ flex: 1, minWidth: 0 }}>
        <Slider value={value} min={min} max={max} onChange={onChange} />
      </span>
      <span style={{
        width: 34, flex: 'none', textAlign: 'right', fontSize: 'var(--fs-5)',
        fontVariantNumeric: 'tabular-nums',
      }}>{zero && value === 0 ? zero : value}</span>
    </label>
  );
}
