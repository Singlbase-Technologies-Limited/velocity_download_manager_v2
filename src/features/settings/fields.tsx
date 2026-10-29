// The Settings screen's rows (V1's SettingRow, Switch and NumberInput), on V2's own components.
import { useId, useState, type ReactNode } from 'react';
import { Input } from '../../components/Input';
import { Select } from '../../components/Select';

/** A titled group of rows. */
export function Group({ title, description, children }: { title: string; description?: ReactNode; children: ReactNode }) {
  return (
    <section aria-label={title} className="rounded-lg border border-border bg-surface p-4">
      <h2 className="text-[13px] font-semibold">{title}</h2>
      {description ? <p className="mt-0.5 text-xs text-fg-muted">{description}</p> : null}
      <div className="mt-2 divide-y divide-border">{children}</div>
    </section>
  );
}

function Row({ id, title, description, children }: { id: string; title: string; description?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 py-2.5">
      <div className="min-w-0">
        <label htmlFor={id} className="text-[13px]">
          {title}
        </label>
        {description ? <p className="text-xs text-fg-muted">{description}</p> : null}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

/** A setting that is on or off. */
export function Toggle({ title, description, checked, disabled, onChange }: { title: string; description?: ReactNode; checked: boolean; disabled?: boolean; onChange: (on: boolean) => void }) {
  const id = useId();
  return (
    <Row id={id} title={title} description={description}>
      <input id={id} type="checkbox" className="size-4 accent-[var(--accent)]" checked={checked} disabled={disabled} onChange={(e) => { onChange(e.target.checked); }} />
    </Row>
  );
}

/**
 * A whole number, saved when the field is left or Enter is pressed (V1's), brought within `min` to `max`, so typing
 * doesn't save every keystroke.
 */
export function Num({ title, description, value, min, max, onChange }: { title: string; description?: ReactNode; value: number; min: number; max: number; onChange: (n: number) => void }) {
  const id = useId();
  const [typed, setTyped] = useState<string | null>(null);
  const commit = () => {
    if (typed === null) return;
    setTyped(null);
    const n = Number(typed);
    if (typed.trim() === '' || !Number.isFinite(n)) return;
    const clamped = Math.min(max, Math.max(min, Math.round(n)));
    if (clamped !== value) onChange(clamped);
  };
  return (
    <Row id={id} title={title} description={description}>
      <Input
        id={id}
        type="number"
        inputMode="numeric"
        min={min}
        max={max}
        className="h-8 w-28"
        value={typed ?? String(value)}
        onChange={(e) => { setTyped(e.target.value); }}
        onBlur={commit}
        onKeyDown={(e) => { if (e.key === 'Enter') commit(); }}
      />
    </Row>
  );
}

/** Text, saved when the field is left; empty is `null`. */
export function Text({ title, description, value, placeholder, onChange }: { title: string; description?: ReactNode; value: string | null; placeholder?: string; onChange: (text: string | null) => void }) {
  const id = useId();
  const [typed, setTyped] = useState<string | null>(null);
  const commit = () => {
    if (typed === null) return;
    setTyped(null);
    const next = typed.trim() === '' ? null : typed.trim();
    if (next !== value) onChange(next);
  };
  return (
    <Row id={id} title={title} description={description}>
      <Input id={id} className="h-8 w-72" placeholder={placeholder} value={typed ?? value ?? ''} onChange={(e) => { setTyped(e.target.value); }} onBlur={commit} onKeyDown={(e) => { if (e.key === 'Enter') commit(); }} />
    </Row>
  );
}

/** One of a few choices. */
export function Choice<T extends string>({ title, description, value, options, onChange }: { title: string; description?: ReactNode; value: T; options: { value: T; label: string }[]; onChange: (v: T) => void }) {
  const id = useId();
  return (
    <Row id={id} title={title} description={description}>
      <Select id={id} className="w-56" value={value} onChange={(e) => { onChange(e.target.value as T); }}>
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </Select>
    </Row>
  );
}
