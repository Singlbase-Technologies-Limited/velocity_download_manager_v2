import { Copy, FileSearch, FolderOpen, Gauge, RotateCcw, X } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { Badge } from '../../components/Badge';
import { Button } from '../../components/Button';
import { Input } from '../../components/Input';
import { ProgressBar } from '../../components/ProgressBar';
import { Select } from '../../components/Select';
import { ConnectionLine } from '../../ConnectionLine';
import type { Verification } from '../../contract';
import { actionsFor, canOpenFile, displayName, etaSeconds, percentOf, RUNNING, STATUS_TONE, statusLabel } from '../../downloads/list';
import { formatDuration, formatPercent, formatShortDate, formatSpeed, formatTransferred, relativeFromNow } from '../../lib/format';
import { useCategories } from '../../lib/queries';
import { useDownloadsStore } from '../../stores/downloads';
import { TorrentSection } from '../torrents/TorrentSection';
import type { DownloadActions } from './useDownloadActions';

const NO_CATEGORY = '__none';
/** Set by the shell from the saved width, fitted to the window; the splitter draws the border. */
const PANEL_WIDTH = { width: 'var(--details-w, 20rem)' };
const KIB = 1024;

const VERIFICATION: Record<Verification, string> = {
  verified: 'Verified: its checksum matched',
  sizeVerified: 'Every byte arrived and the size matched; no checksum was given',
  notVerified: 'Not verified: it resumed without a way to tell the server’s file hadn’t changed',
};

/** A titled group of rows, so the panel reads as sections (V1 #45). */
function DetailsGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mt-4 border-t border-border pt-3 first-of-type:border-t-0" aria-label={title}>
      <h3 className="text-[11px] font-semibold uppercase tracking-wide text-fg-subtle">{title}</h3>
      <dl className="mt-1">{children}</dl>
    </section>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[104px_1fr] gap-2 py-1 text-xs">
      <dt className="text-fg-subtle">{label}</dt>
      <dd className="min-w-0 break-words text-fg">{children}</dd>
    </div>
  );
}

function Shell({ children, onClose }: { children: ReactNode; onClose: () => void }) {
  return (
    <aside id="details-panel" aria-label="Download details" className="flex min-w-0 shrink-0 flex-col items-center justify-center gap-3 bg-surface p-6 text-center text-xs text-fg-muted" style={PANEL_WIDTH}>
      {children}
      <Button size="sm" onClick={onClose}>
        Close
      </Button>
    </aside>
  );
}

interface Props {
  id: string;
  /** The search or filter hides it from the list. */
  hidden: boolean;
  actions: DownloadActions;
  busy: boolean;
  needsLogin: boolean;
  now: number;
  onClose: () => void;
}

/**
 * The details of one download (V1's DetailsPanel, #61). Everything comes from the live snapshot, so switching rows shows
 * the new download at once and no late answer can show another; only what the engines report is shown.
 */
export function DetailsPanel({ id, hidden, actions, busy, needsLogin, now, onClose }: Props) {
  const d = useDownloadsStore((s) => s.snapshot?.downloads.find((x) => x.id === id));
  const { data: categories } = useCategories();
  // A typed value belongs to the download it was typed for, never to the next one shown.
  const [limit, setLimit] = useState<{ id: string; kib: string } | null>(null);
  const [connections, setConnections] = useState<{ id: string; n: string } | null>(null);

  if (!d) return <Shell onClose={onClose}>This download is no longer in the list.</Shell>;

  const torrent = d.kind === 'torrent' || d.kind === 'magnet';
  const finished = ['completed', 'failed', 'removed', 'unknown'].includes(d.status);
  const pct = d.status === 'completed' || d.status === 'seeding' ? 100 : percentOf(d);
  const savedKib = d.speedLimitBps === null ? 0 : Math.round(d.speedLimitBps / KIB);
  const typedKib = limit?.id === d.id ? limit.kib : String(savedKib);
  const typedConnections = connections?.id === d.id ? connections.n : String(d.requestedConnections ?? '');
  const can = actionsFor(d);

  const commitLimit = () => {
    if (limit?.id !== d.id) return;
    const kib = Number(limit.kib);
    setLimit(null);
    if (Number.isInteger(kib) && kib >= 0 && kib !== savedKib) void actions.setSpeedLimit(d.id, kib);
  };
  const wanted = Number(typedConnections);
  const connectionsChanged = Number.isInteger(wanted) && wanted >= 1 && wanted !== d.requestedConnections;

  return (
    <aside id="details-panel" aria-label="Download details" className="flex min-w-0 shrink-0 flex-col bg-surface" style={PANEL_WIDTH}>
      <div className="flex items-start gap-2 border-b border-border px-4 py-3">
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-[13px] font-semibold" title={displayName(d)}>
            {displayName(d)}
          </h2>
          <div className="mt-1.5 flex items-center gap-2">
            <Badge tone={STATUS_TONE[d.status]}>{statusLabel(d, now, { needsLogin })}</Badge>
          </div>
        </div>
        <Button variant="ghost" size="icon-sm" aria-label="Close details" onClick={onClose}>
          <X />
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {hidden ? (
          <p role="status" className="mb-3 rounded-md bg-surface-2 px-3 py-2 text-xs text-fg-muted">
            This download is hidden by the search or filter. It stays here until you choose another.
          </p>
        ) : null}
        <ProgressBar value={pct} tone={STATUS_TONE[d.status]} animated={d.status === 'downloading'} label="Progress" />
        <div className="mt-1.5 flex justify-between text-xs tabular-nums text-fg-muted">
          <span>{formatTransferred(d.progress.completedBytes, d.progress.totalBytes)}</span>
          <span>{formatPercent(pct)}</span>
        </div>

        {d.error ? (
          <div role="alert" className="mt-3 rounded-md border border-danger/30 bg-danger-soft px-3 py-2 text-xs text-danger">
            <p className="font-medium">{d.error.message}</p>
            <p className="mt-1 opacity-80">
              {d.error.httpStatus !== null ? `HTTP ${String(d.error.httpStatus)}. ` : ''}
              {d.error.retryable ? 'This kind of failure can usually be retried.' : 'Retrying is unlikely to help without changing the request.'}
            </p>
            {can.retry ? (
              <Button size="sm" variant="secondary" className="mt-2" disabled={busy} onClick={() => void actions.retry([d.id])}>
                <RotateCcw /> Retry
              </Button>
            ) : null}
          </div>
        ) : null}

        <DetailsGroup title="Transfer">
          <Row label="Speed">{d.status === 'downloading' ? formatSpeed(d.progress.downloadSpeedBps) : '—'}</Row>
          <Row label="Average speed">{formatSpeed(d.averageSpeedBps)}</Row>
          <Row label="Time left">{d.status === 'downloading' ? formatDuration(etaSeconds(d)) : '—'}</Row>
          {torrent ? null : (
            <Row label="Connections">
              {/* What held its connections back while it ran says nothing once it stopped. */}
              {RUNNING.includes(d.status) ? <ConnectionLine active={d.progress.connections} detail={d.progress.connectionDetail} /> : <ConnectionLine active={0} detail={null} />}
            </Row>
          )}
          {d.status === 'retrying' && d.retryAtMs !== null ? <Row label="Next try">{relativeFromNow(d.retryAtMs, now)}</Row> : null}
          {d.verification ? <Row label="Verification">{VERIFICATION[d.verification]}</Row> : null}
        </DetailsGroup>

        {torrent ? <TorrentSection d={d} actions={actions} busy={busy} /> : null}

        <DetailsGroup title="Source and destination">
          <Row label="Link">{d.sourceUrl}</Row>
          <Row label="Folder">{d.directory}</Row>
          <Row label="File">{d.fileName ?? 'Named by the server once it answers'}</Row>
        </DetailsGroup>

        <DetailsGroup title="Timeline">
          <Row label="Added">{formatShortDate(d.createdAtMs)}</Row>
          <Row label="Started">{formatShortDate(d.startedAtMs)}</Row>
          <Row label="Completed">{formatShortDate(d.completedAtMs)}</Row>
        </DetailsGroup>

        <section className="mt-4 space-y-3 border-t border-border pt-3" aria-label="Options for this download">
          <h3 className="text-[11px] font-semibold uppercase tracking-wide text-fg-subtle">Options</h3>
          <div className="flex items-center justify-between gap-2 text-xs">
            <label htmlFor="details-category" className="text-fg-subtle">
              Category
            </label>
            <Select id="details-category" value={d.categoryId ?? NO_CATEGORY} className="w-40" onChange={(e) => void actions.setCategory(d.id, e.target.value === NO_CATEGORY ? null : e.target.value)}>
              <option value={NO_CATEGORY}>None</option>
              {(categories ?? []).map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </Select>
          </div>
          {finished || d.status === 'seeding' ? null : (
            <div className="flex items-center justify-between gap-2 text-xs">
              <label htmlFor="details-limit" className="flex items-center gap-1.5 text-fg-subtle">
                <Gauge className="size-3.5" aria-hidden /> Speed limit (KiB/s, 0 for none)
              </label>
              <Input
                id="details-limit"
                type="number"
                min={0}
                inputMode="numeric"
                className="h-8 w-24"
                value={typedKib}
                onChange={(e) => { setLimit({ id: d.id, kib: e.target.value }); }}
                onBlur={commitLimit}
                onKeyDown={(e) => { if (e.key === 'Enter') commitLimit(); }}
              />
            </div>
          )}
          {torrent || finished ? null : (
            <div className="flex items-center justify-between gap-2 text-xs">
              <label htmlFor="details-connections" className="text-fg-subtle">
                Connections
              </label>
              <div className="flex items-center gap-1.5">
                <Input id="details-connections" type="number" min={1} inputMode="numeric" className="h-8 w-16" value={typedConnections} onChange={(e) => { setConnections({ id: d.id, n: e.target.value }); }} />
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={busy || !connectionsChanged}
                  onClick={() => {
                    setConnections(null);
                    void actions.setConnections(d.id, wanted);
                  }}
                >
                  Apply
                </Button>
              </div>
            </div>
          )}
        </section>
      </div>

      <div className="flex flex-wrap gap-2 border-t border-border px-4 py-3">
        {canOpenFile(d) ? (
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void actions.openFile(d)}>
            <FileSearch /> Open file
          </Button>
        ) : null}
        <Button size="sm" variant="secondary" disabled={busy} onClick={() => void actions.openFolder(d.id)}>
          <FolderOpen /> Open folder
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void actions.copyLink(d)}>
          <Copy /> Copy link
        </Button>
      </div>
    </aside>
  );
}
