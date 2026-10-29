import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Download, RefreshCw, RotateCw } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { toast } from 'sonner';
import { Badge } from '../../components/Badge';
import { Button } from '../../components/Button';
import { confirmAction } from '../../components/confirmStore';
import { Spinner } from '../../components/Spinner';
import type { EngineHealth } from '../../contract';
import { api } from '../../lib/api';
import { ERROR_MESSAGES, toApiError } from '../../lib/errors';
import { formatShortDate } from '../../lib/format';
import { useDownloadsStore } from '../../stores/downloads';

const KEY = ['diagnostics'] as const;

function Section({ title, description, actions, children }: { title: string; description?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <section aria-label={title} className="rounded-lg border border-border bg-surface p-4">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-[13px] font-semibold">{title}</h2>
          {description ? <p className="mt-0.5 text-xs text-fg-muted">{description}</p> : null}
        </div>
        {actions ? <div className="flex shrink-0 gap-1.5">{actions}</div> : null}
      </div>
      <div className="mt-3">{children}</div>
    </section>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[180px_1fr] gap-3 border-b border-border py-2 text-xs last:border-b-0">
      <dt className="text-fg-muted">{label}</dt>
      <dd className="min-w-0 break-words">{children}</dd>
    </div>
  );
}

function Health({ health }: { health: EngineHealth }) {
  switch (health.state) {
    case 'running':
      return <Badge tone="success">Running</Badge>;
    case 'restarting':
      return <Badge tone="warning">{`Starting again (attempt ${String(health.attempt)})`}</Badge>;
    case 'unavailable':
      return (
        <span>
          <Badge tone="danger">Unavailable</Badge> {ERROR_MESSAGES[health.code]}
        </span>
      );
  }
}

/**
 * What the app and its engines are, and their recent logs (#28, V1's DiagnosticsPanel). #30 places it on the Settings
 * screen. The export is the only way the report leaves the app, to a file the user chooses, redacted again as a whole;
 * nothing is sent anywhere.
 */
export function DiagnosticsPanel() {
  const qc = useQueryClient();
  const report = useQuery({ queryKey: KEY, queryFn: api.diagnostics, retry: false });
  // The engines' health follows the live snapshot when there is one.
  const live = useDownloadsStore((s) => s.snapshot);
  const [busy, setBusy] = useState<string | null>(null);

  const run = async (what: string, fn: () => Promise<void>) => {
    setBusy(what);
    try {
      await fn();
    } catch (e) {
      toast.error(toApiError(e).message);
    } finally {
      setBusy(null);
    }
  };
  const restart = (which: 'engine' | 'helper') =>
    run(which, async () => {
      const ok = await confirmAction({
        title: which === 'engine' ? 'Restart the transfer engine?' : 'Restart the torrent helper?',
        description: 'Running downloads stop for a moment and resume where they were.',
        confirmLabel: which === 'engine' ? 'Restart engine' : 'Restart helper',
      });
      if (!ok) return;
      const health = await (which === 'engine' ? api.restartEngine() : api.restartTorrentHelper());
      if (health.state === 'running') toast.success(which === 'engine' ? 'The transfer engine is running' : 'The torrent helper is running');
      else toast.warning(health.state === 'unavailable' ? ERROR_MESSAGES[health.code] : 'Still starting');
      await qc.invalidateQueries({ queryKey: KEY });
    });
  const exportReport = () =>
    run('export', async () => {
      const path = await api.exportDiagnostics();
      if (path) toast.success(`Saved to ${path}`);
    });

  if (report.isPending) {
    return (
      <div className="flex items-center gap-2 text-xs text-fg-muted">
        <Spinner /> Reading the diagnostics…
      </div>
    );
  }
  if (report.isError) {
    return (
      <p role="alert" className="text-xs text-danger">
        {toApiError(report.error).message}
      </p>
    );
  }
  const d = report.data;
  const engine = live?.engine ?? d.transferEngine;
  const helper = live?.torrentEngine ?? d.torrentEngine;

  return (
    <div className="space-y-4">
      <Section title={d.app.name} description={`Version ${d.app.version} · ${d.os} ${d.arch} · command contract ${String(d.app.commandApiVersion)}`}>
        <p className="text-xs leading-relaxed text-fg-muted">Velocity collects no analytics or telemetry, and sends nothing about you or your downloads anywhere. Diagnostics leave this computer only as a file you export and choose to share.</p>
      </Section>

      <Section title="Engines">
        <dl>
          <Row label="Transfer engine">
            <Health health={engine} />
          </Row>
          <Row label="Runs on">{d.capabilities.libcurl ? `libcurl ${d.capabilities.libcurl.version}, TLS by ${d.capabilities.libcurl.tls}` : 'Not reported'}</Row>
          <Row label="Protocols">{d.capabilities.protocols.map((p) => p.kind.toUpperCase()).join(', ') || '—'}</Row>
          <Row label="Torrent helper">
            <Health health={helper} />
          </Row>
          <Row label="Runs on">{d.libtorrent ? `libtorrent ${d.libtorrent}` : 'Not reported'}</Row>
          <Row label="Downloads">{d.downloads.map((c) => `${String(c.count)} ${c.status}`).join(', ') || 'None'}</Row>
          {d.storageError ? <Row label="Saving downloads">{ERROR_MESSAGES[d.storageError]}</Row> : null}
        </dl>
        <div className="mt-3 flex gap-2">
          <Button size="sm" variant="secondary" busy={busy === 'engine'} disabled={busy !== null} onClick={() => void restart('engine')}>
            <RotateCw /> Restart transfer engine
          </Button>
          <Button size="sm" variant="secondary" busy={busy === 'helper'} disabled={busy !== null} onClick={() => void restart('helper')}>
            <RotateCw /> Restart torrent helper
          </Button>
        </div>
      </Section>

      <Section
        title="Diagnostics"
        description="Logins, cookies, keys, tokens, magnet links and tracker passkeys are removed from everything here."
        actions={
          <>
            <Button size="sm" variant="ghost" aria-label="Read the diagnostics again" title="Read again" onClick={() => void report.refetch()}>
              <RefreshCw />
            </Button>
            <Button size="sm" variant="secondary" busy={busy === 'export'} disabled={busy !== null} onClick={() => void exportReport()}>
              <Download /> Export diagnostics…
            </Button>
          </>
        }
      >
        {d.appLog.length === 0 ? (
          <p className="text-xs text-fg-muted">Nothing logged yet.</p>
        ) : (
          <ul aria-label="Recent log" className="max-h-64 space-y-1 overflow-y-auto font-mono text-[11px]">
            {d.appLog.map((l, i) => (
              <li key={`${String(l.atMs)}-${String(i)}`} className={l.level === 'error' ? 'text-danger' : l.level === 'warn' ? 'text-warning' : 'text-fg-muted'}>
                {`${formatShortDate(l.atMs)} ${l.level.toUpperCase()} ${l.message}`}
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}
