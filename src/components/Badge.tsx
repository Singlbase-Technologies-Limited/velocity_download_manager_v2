import type { ReactNode } from 'react';

export function Badge({ children }: { children: ReactNode }) {
  return <span className="inline-flex items-center gap-1 rounded bg-surface-3 px-1.5 py-0.5 text-[11px] font-medium text-fg-muted">{children}</span>;
}
