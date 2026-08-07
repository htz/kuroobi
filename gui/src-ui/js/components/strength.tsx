import { useState } from 'react';
import { LEVELS, SOLVE_MAX, clampLevels, type Levels } from '../state';
import { Button, Select, TextField } from './primitives';

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

/** 数を 1 つ決める。**数字の欄が主役**で、±1 は隣のボタン。
 *  範囲は打ち込んだ後に丸める (打っている途中で直すと、2 桁目が打てない)。 */
function Num({ label, value, min, max, zero, onChange }: {
  label: string; value: number; min: number; max: number;
  /** 0 の意味を添える言葉 (選択読みの「0 = なし」)。 */
  zero?: string;
  onChange: (n: number) => void;
}) {
  const clamp = (n: number) => Math.max(min, Math.min(max, n));
  return (
    <label style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)' }}>
      {/* --w-label (96px) は設定の見出し用で、ここには広すぎる。
          290px の枠に 欄 + ± + 補足 まで収めたいので、名前は 4 文字ぶん */}
      <span style={{ width: 64, flex: 'none', fontSize: 'var(--fs-6)', color: 'var(--sub)' }}>
        {label}
      </span>
      {/* 範囲は欄の脇に書かない — 290px の枠では折り返して 3 行になる。
          端に来たら ± が押せなくなるので、触れる範囲はそれで分かる */}
      <TextField numeric align="right" width={56} value={String(value)}
                 title={`${min}〜${max}`}
                 onChange={(t) => {
                   // 空にした途中の状態では動かさない (消してから打ち直せる)
                   if (t === '') return;
                   const n = parseInt(t, 10);
                   if (Number.isFinite(n)) onChange(clamp(n));
                 }} />
      {/* 欄と同じ 32px。既定の ctrl (28px) だと、同じ行で欄より 4px 低くなり
          底が揃わない。数を増減する釦は欄の一部なので、高さも欄に合わせる */}
      <Button size="field" disabled={value <= min} onClick={() => onChange(clamp(value - 1))}>−</Button>
      <Button size="field" disabled={value >= max} onClick={() => onChange(clamp(value + 1))}>＋</Button>
      {/* 0 に意味のある欄だけ、0 のときにその意味を添える */}
      {zero && value === 0 && (
        <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{zero}</span>
      )}
    </label>
  );
}
