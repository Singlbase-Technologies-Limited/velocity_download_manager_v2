import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';

export function EmptyState({ icon: Icon, title, description, action }: { icon: LucideIcon; title: string; description?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 p-10 text-center">
      <div className="flex size-12 items-center justify-center rounded-full bg-surface-3 text-fg-subtle">
        <Icon className="size-6" aria-hidden />
      </div>
      <div>
        <p className="text-sm font-semibold text-fg">{title}</p>
        {description ? <p className="mt-1 max-w-sm text-xs text-fg-muted">{description}</p> : null}
      </div>
      {action}
    </div>
  );
}
