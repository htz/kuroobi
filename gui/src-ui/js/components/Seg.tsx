// 2〜4 択の切り替え。画面の設定はすべてこの形に揃えてある。
//
// **手書きしないこと。** かつては同じマークアップが 6 か所に散っていて、
// 見た目を変えるたびに取りこぼしが出ていた。

export interface SegProps<T extends string> {
  value: T;
  /** [値, 表示名] の並び。左から順に出す。 */
  options: readonly (readonly [T, string])[];
  onChange: (v: T) => void;
  /** 選べない状態 (定石ファイルが無い、など)。押せないことを見た目でも示す。 */
  disabled?: boolean;
}

export function Seg<T extends string>({ value, options, onChange, disabled }: SegProps<T>) {
  return (
    <div className={'seg' + (disabled ? ' disabled' : '')}>
      {options.map(([v, label]) => (
        <button key={v} className={value === v ? 'active' : ''}
                disabled={disabled}
                onClick={() => onChange(v)}>{label}</button>
      ))}
    </div>
  );
}
