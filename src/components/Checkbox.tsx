import type { ReactNode } from 'react';
import { cn } from '../lib/cn';

/** A checkbox with its label beside it; clicking the label ticks it. */
export function Checkbox({
  checked,
  onChange,
  children,
  disabled,
  className,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  children: ReactNode;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <label className={cn('flex items-center gap-2 text-xs', disabled && 'opacity-50', className)}>
      <input
        type="checkbox"
        className="size-3.5 accent-[var(--accent)]"
        checked={checked}
        disabled={disabled}
        onChange={(e) => { onChange(e.target.checked); }}
      />
      {children}
    </label>
  );
}
