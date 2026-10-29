import { useState } from 'react';
import { Field } from '../../components/Field';
import { Input } from '../../components/Input';
import { Select } from '../../components/Select';
import type { SeedPolicy } from '../../contract';
import { SEED_MODE_LABEL, seedingSummary } from './torrentLogic';

const MODES: SeedPolicy['mode'][] = ['never', 'time', 'ratio', 'timeOrRatio', 'unlimited'];
/** What a time or ratio starts at when its mode is chosen (V1's). */
const START = { minutes: 60, ratio: 1 };

/** When a torrent stops seeding (V1's SeedingFields, #63). The limits mirror `SeedPolicy::check`. */
export function SeedingFields({ value, onChange }: { value: SeedPolicy; onChange: (p: SeedPolicy) => void }) {
  // What was typed, kept while the policy keeps its mode, so a field can be cleared and retyped.
  const [minutes, setMinutes] = useState(String('minutes' in value ? value.minutes : START.minutes));
  const [ratio, setRatio] = useState(String('ratio' in value ? value.ratio : START.ratio));
  const usesTime = value.mode === 'time' || value.mode === 'timeOrRatio';
  const usesRatio = value.mode === 'ratio' || value.mode === 'timeOrRatio';

  const policy = (mode: SeedPolicy['mode'], m: string, r: string): SeedPolicy => {
    const min = Math.min(525_600, Math.max(1, Math.round(Number(m)) || START.minutes));
    const rat = Math.min(100, Math.max(0.1, Math.round((Number(r) || START.ratio) * 10) / 10));
    switch (mode) {
      case 'never':
      case 'unlimited':
        return { mode };
      case 'time':
        return { mode, minutes: min };
      case 'ratio':
        return { mode, ratio: rat };
      case 'timeOrRatio':
        return { mode, minutes: min, ratio: rat };
    }
  };

  return (
    <div className="flex flex-wrap items-start gap-3">
      <Field label="Seeding after download" hint={seedingSummary(value)}>
        {(p) => (
          <Select {...p} value={value.mode} className="w-60" onChange={(e) => { onChange(policy(e.target.value as SeedPolicy['mode'], minutes, ratio)); }}>
            {MODES.map((m) => (
              <option key={m} value={m}>
                {SEED_MODE_LABEL[m]}
              </option>
            ))}
          </Select>
        )}
      </Field>
      {usesTime ? (
        <Field label="Seed time (minutes)">
          {(p) => <Input {...p} type="number" min={1} max={525_600} className="w-28" value={minutes} onChange={(e) => { setMinutes(e.target.value); onChange(policy(value.mode, e.target.value, ratio)); }} />}
        </Field>
      ) : null}
      {usesRatio ? (
        <Field label="Share ratio">
          {(p) => <Input {...p} type="number" min={0.1} max={100} step={0.1} className="w-24" value={ratio} onChange={(e) => { setRatio(e.target.value); onChange(policy(value.mode, minutes, e.target.value)); }} />}
        </Field>
      ) : null}
    </div>
  );
}
