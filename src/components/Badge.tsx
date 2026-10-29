import type { ReactNode } from 'react';
import type { StatusTone } from '../downloads/list';
import { cn } from '../lib/cn';

const toneClasses: Record<StatusTone, string> = {
  active: 'bg-accent-soft text-accent',
  neutral: 'bg-surface-3 text-fg-muted',
  success: 'bg-success-soft text-success',
  danger: 'bg-danger-soft text-danger',
  warning: 'bg-warning-soft text-warning',
  muted: 'bg-surface-3 text-fg-subtle',
};

export function Badge({ children, tone = 'neutral' }: { children: ReactNode; tone?: StatusTone }) {
  return <span className={cn('inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-medium', toneClasses[tone])}>{children}</span>;
}
