import { useState } from 'react';
import { LEVELS, SOLVE_MAX, clampLevels, type Levels } from '../state';
import { t } from '../i18n';
import { Select } from './primitives';


const label = (l: typeof LEVELS[number]) =>
  (l.band
    ? t('ui.strength.preset_band', { name: l.name, depth: l.depth, solve: l.solve, band: l.band })
    : t('ui.strength.preset', { name: l.name, depth: l.depth, solve: l.solve }));

const presetOf = (v: Levels): number | 'custom' => {
  const i = LEVELS.findIndex((l) => l.depth === v.depth && l.solve === v.solve && l.band === v.band);
  return i >= 0 ? i : 'custom';
};



export function Strength({ value, onChange }: { value: Levels; onChange: (v: Levels) => void }) {
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
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 'var(--sp-2)' }}>
          <Pick label={t('ui.strength.depth')} value={value.depth} min={1} max={SOLVE_MAX}
                onChange={(n) => onChange(clampLevels({ ...value, depth: n }))} />
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

function Pick({ label, value, min, max, zero, plus, onChange }: {
  label: string; value: number; min: number; max: number;
  zero?: string;
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
      <Select size="ctrl" value={String(Math.max(min, Math.min(max, value)))}
              options={options} onChange={(v) => onChange(+v)} />
    </label>
  );
}
