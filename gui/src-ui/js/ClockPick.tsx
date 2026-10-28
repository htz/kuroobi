import { useState } from 'react';
import { ClockField, Select } from './components/primitives';
import { t } from './i18n';

/** A clock picked from a few choices, or to the second through 「カスタム…」. */
export function ClockPick({ secs, onChange, choices, min, max }: {
  secs: number;
  onChange: (secs: number) => void;
  choices: [number, string][];
  min?: number;
  max?: number;
}) {
  const [custom, setCustom] = useState(!choices.some(([v]) => v === secs));
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--sp-2)' }}>
      <Select value={custom ? 'custom' : String(secs)}
              onChange={(v) => {
                setCustom(v === 'custom');
                if (v !== 'custom') onChange(Number(v));
              }}
              options={[...choices.map(([v, l]): [string, string] => [String(v), l]),
                        ['custom', t('ggs.clock_choice.custom')]]} />
      {custom && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 'var(--sp-2)', fontSize: 'var(--fs-5)',
                      color: 'var(--sub)', whiteSpace: 'nowrap' }}>
          <ClockField value={secs} onChange={onChange} min={min} max={max} label={t('ggs.clock_choice.unit')} />
          <span>{t('ggs.clock_choice.unit')}</span>
        </div>
      )}
    </div>
  );
}
