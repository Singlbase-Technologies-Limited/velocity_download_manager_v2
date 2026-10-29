import type { StatusTone } from '../downloads/list';
import { cn } from '../lib/cn';

const barTone: Record<StatusTone, string> = {
  active: 'bg-accent',
  neutral: 'bg-fg-subtle',
  success: 'bg-success',
  danger: 'bg-danger',
  warning: 'bg-warning',
  muted: 'bg-fg-subtle/60',
};

/** V1's progress bar. `value` `null` is an unknown size: an empty or moving bar, never a made-up percentage. */
export function ProgressBar({ value, tone, animated, label, className }: { value: number | null; tone: StatusTone; animated?: boolean; label: string; className?: string }) {
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value === null ? undefined : Math.round(value)}
      className={cn('relative h-1.5 w-full overflow-hidden rounded-full bg-surface-3', className)}
    >
      {value === null ? (
        animated ? <div className={cn('absolute inset-0 opacity-60 progress-active', barTone[tone])} /> : null
      ) : (
        <div className={cn('h-full rounded-full transition-[width] duration-500 motion-reduce:transition-none', barTone[tone], animated && 'progress-active')} style={{ width: `${String(Math.min(100, Math.max(0, value)))}%` }} />
      )}
    </div>
  );
}
