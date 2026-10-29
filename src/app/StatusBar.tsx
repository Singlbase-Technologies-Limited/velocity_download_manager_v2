import { AlertTriangle, ArrowDown } from 'lucide-react';
import { useMemo } from 'react';
import type { EngineHealth } from '../contract';
import { countByView } from '../downloads/views';
import { cn } from '../lib/cn';
import { ERROR_MESSAGES } from '../lib/errors';
import { formatSpeed } from '../lib/format';
import { useDownloadsStore } from '../stores/downloads';

/** What an engine is doing, for the status bar: a label, a dot and, when unavailable, why. */
function engineState(name: string, health: EngineHealth | undefined) {
  if (!health) return { label: 'Starting', dot: 'bg-warning animate-pulse', reason: undefined };
  switch (health.state) {
    case 'running':
      return { label: 'Ready', dot: 'bg-success', reason: undefined };
    case 'restarting':
      return { label: `${name} restarting`, dot: 'bg-warning animate-pulse', reason: undefined };
    case 'unavailable':
      return { label: `${name} unavailable`, dot: 'bg-danger', reason: ERROR_MESSAGES[health.code] };
  }
}

/** The bottom bar: the engines, what is active and queued, the total speed (V1's StatusBar). */
export function StatusBar() {
  const snapshot = useDownloadsStore((s) => s.snapshot);
  const error = useDownloadsStore((s) => s.error);
  const counts = useMemo(() => countByView(snapshot?.downloads ?? []), [snapshot]);
  const speed = useMemo(() => (snapshot?.downloads ?? []).reduce((sum, d) => sum + d.progress.downloadSpeedBps, 0), [snapshot]);
  const engine = engineState('Transfer engine', snapshot?.engine);
  // The torrent helper is named only when it isn't running.
  const torrents = snapshot && snapshot.torrentEngine.state !== 'running' ? engineState('Torrent helper', snapshot.torrentEngine) : null;
  const problem = error ?? (snapshot?.storageError ? ERROR_MESSAGES[snapshot.storageError] : null);

  return (
    <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-border bg-surface px-3 text-[11px] text-fg-muted" aria-label="Status">
      {[engine, torrents].map((s) =>
        s ? (
          <span key={s.label} className="flex items-center gap-1.5 px-1" title={s.reason}>
            <span className={cn('size-2 rounded-full', s.dot)} aria-hidden />
            <span>{s.label}</span>
          </span>
        ) : null,
      )}
      <span className="tabular-nums" aria-live="polite">
        {snapshot ? `${String(counts.downloading)} active · ${String(counts.queued)} queued` : '—'}
      </span>
      {problem ? (
        <span className="flex min-w-0 items-center gap-1 text-danger" role="alert">
          <AlertTriangle className="size-3.5 shrink-0" aria-hidden />
          <span className="truncate">{problem}</span>
        </span>
      ) : null}
      <div className="flex-1" />
      <span className="flex min-w-24 items-center justify-end gap-1 font-medium tabular-nums text-fg" title="Total download speed">
        <ArrowDown className="size-3.5 text-accent" aria-hidden />
        {formatSpeed(speed)}
      </span>
    </footer>
  );
}
