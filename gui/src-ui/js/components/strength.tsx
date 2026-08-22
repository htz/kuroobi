import { useState } from 'react';
import { LEVELS, SOLVE_MAX, clampLevels, type Levels } from '../state';
import { t } from '../i18n';
import { Select } from './primitives';

/* Strength picker, shared by play/study/GGS. They used to differ
 * (preset list vs numeric inputs) for the same three values. While a
 * preset is selected the three fields stay hidden: the name already
 * says the numbers, and touchable-looking duplicates would silently
 * switch to custom. */

/* One key per whole label; the band variant is its own sentence so no
   language has to build it by gluing fragments together. */
const label = (l: typeof LEVELS[number]) =>
  (l.band
    ? t('ui.strength.preset_band', { name: l.name, depth: l.depth, solve: l.solve, band: l.band })
    : t('ui.strength.preset', { name: l.name, depth: l.depth, solve: l.solve }));

const presetOf = (v: Levels): number | 'custom' => {
  const i = LEVELS.findIndex((l) => l.depth === v.depth && l.solve === v.solve && l.band === v.band);
  return i >= 0 ? i : 'custom';
};



export function Strength({ value, onChange }: { value: Levels; onChange: (v: Levels) => void }) {
  // Remember that custom was chosen: deriving from values alone
  // closes it again whenever they match a preset.
  const [picked, setPicked] = useState(false);
  const custom = picked || presetOf(value) === 'custom';

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-3)' }}>
      <Select value={custom ? 'custom' : String(presetOf(value))}
              options={[...LEVELS.map((l, i) => [String(i), label(l)] as [string, string]),
                        ['custom', t('ui.strength.custom')]]}
              onChange={(s) => {
                if (s === 'custom') { setPicked(true); return; }   // keep the values
                setPicked(false);
                const l = LEVELS[+s];
                onChange({ depth: l.depth, solve: l.solve, band: l.band });
              }} />
      {custom && (
        /* The design lays depth/solve/band as a three-column grid
           (10px labels over 28px Selects). A stepper variant was
           reverted per the user's call; side by side reads as the one
           unit they are. */
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 'var(--sp-2)' }}>
          <Pick label={t('ui.strength.depth')} value={value.depth} min={1} max={SOLVE_MAX}
                onChange={(n) => onChange(clampLevels({ ...value, depth: n }))} />
          {/* The solve must be >= depth, or a span exists where the
              midgame reads past the game end without solving. */}
          <Pick label={t('ui.strength.solve')} value={value.solve} min={value.depth} max={SOLVE_MAX}
                onChange={(n) => onChange(clampLevels({ ...value, solve: n }))} />
          <Pick label={t('ui.strength.band')} value={value.band} min={0} max={12}
                zero={t('ui.strength.band_none')} plus
                onChange={(n) => onChange({ ...value, band: n })} />
        </div>
      )}
    </div>
  );
}

/** Pick one number from valid values only — free input with clamping
 *  would transiently let the solve drop below depth. Fields where 0
 *  is meaningful render 0 as a word. */
function Pick({ label, value, min, max, zero, plus, onChange }: {
  label: string; value: number; min: number; max: number;
  zero?: string;
  /** Prefix `+` on additive fields (the band) to match the preset
   *  labels; absolute fields (depth, solve) go without. */
  plus?: boolean;
  onChange: (n: number) => void;
}) {
  const options: [string, string][] = [];
  for (let n = min; n <= max; n++) {
    options.push([String(n), zero && n === 0 ? zero : (plus ? '+' : '') + n]);
  }
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-1)' }}>
      <span style={{ fontSize: 'var(--fs-7)', color: 'var(--sub)' }}>{label}</span>
      {/* 28px per the design — sized so the three read as one unit. */}
      <Select size="ctrl" value={String(Math.max(min, Math.min(max, value)))}
              options={options} onChange={(v) => onChange(+v)} />
    </label>
  );
}
