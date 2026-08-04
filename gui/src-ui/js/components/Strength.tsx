// 強さの選び方。対局と GGS で同じ形にするための共通部品。
//
// 別々に作っていたときは、対局がプリセットの一覧、GGS が数値入力と
// 選び方そのものが違っていた。同じ 3 つの値を決めるのに操作が違うと、
// 片方で覚えたことがもう片方で通じない。

import { useEffect, useState } from 'react';
import { LEVELS } from '../state';
import type { Levels } from '../state';

/** 読切 (空きマス数) の上限。これ以上は現実的な時間で解けない。 */
const SOLVE_MAX = 36;

/// 読切は深さ以上でなければならない。深さのほうが大きいと、中盤探索が
/// 終局を跨いで読むだけの区間ができる — 読み切れる局面なのに読み切りに
/// 入らず、MPC で枝を刈った不正確な値のまま深さだけを費やす。
/// 深さの上限も読切に合わせる (それより深くしても選べる読切が無い)。
export function clampLevels(v: Levels): Levels {
  const depth = Math.max(1, Math.min(SOLVE_MAX, v.depth));
  return { depth, solve: Math.max(depth, Math.min(SOLVE_MAX, v.solve)), band: v.band };
}

/** いまの値がプリセットのどれかなら、その番号。違えばカスタム。 */
function presetOf(v: Levels): number | 'custom' {
  const i = LEVELS.findIndex(
    (lv) => lv.depth === v.depth && lv.solve === v.solve && lv.band === v.band);
  return i >= 0 ? i : 'custom';
}

export interface StrengthProps {
  value: Levels;
  onChange: (v: Levels) => void;
}

export function Strength({ value, onChange }: StrengthProps) {
  // 「カスタム」を選んだことを覚えておく。値だけから導くと、プリセットと
  // 同じ値のまま開けない — カスタムを選んでも値は変わらないので、次の描画で
  // またそのプリセットだと判定されて閉じてしまう
  const [picked, setPicked] = useState(false);
  // プリセットを選んでいる間は 3 つを出さない。中身は名前に書いてあるし、
  // 触れない数字を並べても場所を取るだけ
  const custom = picked || presetOf(value) === 'custom';
  const preset = custom ? 'custom' : presetOf(value);

  // 前の画面では読切 < 深さ を作れたので、保存済みの設定が条件を満たして
  // いないことがある。そのままだと選択肢に無い値を選んでいる状態になり、
  // 何が入っているのか読めなくなるので、開いた時点で直す
  useEffect(() => {
    const c = clampLevels(value);
    if (c.depth !== value.depth || c.solve !== value.solve) onChange(c);
  }, [value, onChange]);

  const pick = (s: string) => {
    if (s === 'custom') {
      // 値は変えない。いまの強さを引き継いで、そこから触れるようにするだけ
      setPicked(true);
      return;
    }
    setPicked(false);
    const lv = LEVELS[+s];
    onChange({ depth: lv.depth, solve: lv.solve, band: lv.band });
  };

  const range = (from: number, to: number) =>
    Array.from({ length: to - from + 1 }, (_, i) => from + i);

  return (
    <>
      <div>
        <label className="field">強さ</label>
        <div className="selwrap">
          <select value={String(preset)} onChange={(e) => pick(e.target.value)}>
            {LEVELS.map((lv, i) => (
              <option key={i} value={i}>
                {`${lv.name} — 深さ${lv.depth} / 読切${lv.solve}${
                  lv.band ? ` / 選択読み+${lv.band}` : ''}`}
              </option>
            ))}
            <option value="custom">カスタム…</option>
          </select>
        </div>
      </div>
      {custom && (
        <div className="row">
          <div>
            <label className="field">深さ</label>
            <div className="selwrap">
              <select value={value.depth}
                      onChange={(e) => onChange(clampLevels({ ...value, depth: +e.target.value }))}>
                {range(1, SOLVE_MAX).map((n) => <option key={n} value={n}>{n}</option>)}
              </select>
            </div>
          </div>
          <div>
            <label className="field">読切</label>
            <div className="selwrap">
              {/* 深さ未満は出さない (上のコメントの理由) */}
              <select value={value.solve}
                      onChange={(e) => onChange({ ...value, solve: +e.target.value })}>
                {range(value.depth, SOLVE_MAX).map((n) => <option key={n} value={n}>{n}</option>)}
              </select>
            </div>
          </div>
          <div>
            <label className="field">選択読み</label>
            <div className="selwrap">
              <select value={value.band}
                      onChange={(e) => onChange({ ...value, band: +e.target.value })}>
                {range(0, 12).map((n) => (
                  <option key={n} value={n}>{n === 0 ? 'なし' : `+${n}`}</option>))}
              </select>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
