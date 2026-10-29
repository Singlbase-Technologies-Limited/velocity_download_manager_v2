import { AlertTriangle } from 'lucide-react';
import { Button } from '../components/Button';
import type { ApiError } from '../lib/errors';

/** The window can't go on: what went wrong, its code, and a retry where one can help (V1's FatalError). */
export function FatalError({ title, error, onRetry }: { title: string; error: ApiError; onRetry?: () => void }) {
  return (
    <div className="flex h-full items-center justify-center p-8">
      <div role="alert" className="max-w-lg rounded-xl border border-border bg-surface p-6 shadow-panel">
        <div className="flex items-center gap-3">
          <div className="flex size-10 items-center justify-center rounded-full bg-danger-soft text-danger">
            <AlertTriangle className="size-5" aria-hidden />
          </div>
          <h1 className="text-base font-semibold">{title}</h1>
        </div>
        <p className="mt-4 text-[13px] text-fg">{error.message}</p>
        {error.detail ? (
          <pre className="mt-3 max-h-40 overflow-auto whitespace-pre-wrap rounded-md bg-surface-3 p-3 font-mono text-[11px] text-fg-muted" data-selectable>
            {error.detail}
          </pre>
        ) : null}
        <p className="mt-3 text-xs text-fg-muted">Error code: {error.code}</p>
        {onRetry ? (
          <Button className="mt-5" variant="primary" onClick={onRetry}>
            Try again
          </Button>
        ) : null}
      </div>
    </div>
  );
}
