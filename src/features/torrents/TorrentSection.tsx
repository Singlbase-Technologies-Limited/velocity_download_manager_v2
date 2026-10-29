import { useQueryClient } from '@tanstack/react-query';
import { ArrowUpFromLine, ListChecks, Radio, UploadCloud } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { Button } from '../../components/Button';
import { Checkbox } from '../../components/Checkbox';
import { Input } from '../../components/Input';
import type { DownloadRecord } from '../../contract';
import { ApiError, toApiError } from '../../lib/errors';
import { formatBytes, formatDuration, formatSpeed } from '../../lib/format';
import { keys, useSwarm, useTorrentDetails } from '../../lib/queries';
import { useUiStore } from '../../stores/ui';
import type { DownloadActions } from '../downloads/useDownloadActions';
import { FileTree } from './FileTree';
import { sameSelection, seedingSummary, TRACKER_STATE, trackerSummary, VERSION_LABEL } from './torrentLogic';

const PEERS_SHOWN = 20;
const KIB = 1024;
/** Its files can change while the helper still downloads it (V1's). */
const FILES_EDITABLE: readonly DownloadRecord['status'][] = ['downloading', 'stalled', 'connecting', 'paused', 'queued'];
/** The helper has nothing to say about a torrent in these. */
const FINISHED: readonly DownloadRecord['status'][] = ['completed', 'failed', 'removed', 'unknown'];

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[104px_1fr] gap-2 py-1 text-xs">
      <dt className="text-fg-subtle">{label}</dt>
      <dd className="min-w-0 break-words text-fg">{children}</dd>
    </div>
  );
}

/**
 * The torrent part of the details panel (V1's TorrentSection; #61, #63): what the snapshot carries, the torrent's
 * metadata, seeding and files from `get_torrent_details`, and its trackers and peers, asked for every second only
 * while it is shown (PO decision on #63).
 */
export function TorrentSection({ d, actions, busy }: { d: DownloadRecord; actions: DownloadActions; busy: boolean }) {
  const qc = useQueryClient();
  const openChooseFiles = useUiStore((s) => s.openChooseFiles);
  const details = useTorrentDetails(d.id);
  const live = !FINISHED.includes(d.status);
  const { trackers, peers } = useSwarm(d.id, live);
  // Typed values belong to the torrent they were typed for, never to the next one shown.
  const [upload, setUpload] = useState<{ id: string; kib: string } | null>(null);
  const [draft, setDraft] = useState<{ id: string; files: Set<number> } | null>(null);
  const [deleteDropped, setDeleteDropped] = useState(false);

  const t = details.data;
  const contents = t?.contents ?? null;
  const files = contents?.files ?? [];
  const chosen = new Set(t?.chosen ?? files.map((f) => f.index));
  const editable = FILES_EDITABLE.includes(d.status) && contents !== null;
  const shown = editable && draft?.id === d.id ? draft.files : chosen;
  const changed = draft?.id === d.id && !sameSelection(draft.files, chosen);
  const savedKib = t ? Math.round(t.uploadLimitBps / KIB) : 0;
  const typedKib = upload?.id === d.id ? upload.kib : String(savedKib);
  const queued = d.torrent?.seedQueued === true;
  const notRunning = trackers.error instanceof ApiError && trackers.error.code === 'INVALID_STATE';

  const commitUpload = () => {
    if (upload?.id !== d.id) return;
    const kib = Number(upload.kib);
    setUpload(null);
    if (Number.isInteger(kib) && kib >= 0 && kib !== savedKib) {
      void actions.setUploadLimit(d.id, kib).then(() => qc.invalidateQueries({ queryKey: keys.torrent(d.id) }));
    }
  };
  const applyFiles = async () => {
    if (draft?.id !== d.id) return;
    const ok = await actions.changeFiles(d.id, [...draft.files].sort((a, b) => a - b), deleteDropped);
    if (ok) {
      setDraft(null);
      setDeleteDropped(false);
      void qc.invalidateQueries({ queryKey: keys.torrent(d.id) });
    }
  };

  return (
    <section className="mt-4 border-t border-border pt-3" aria-label="Torrent">
      <h3 className="text-[11px] font-semibold uppercase tracking-wide text-fg-subtle">Torrent</h3>
      <dl className="mt-1">
        {contents?.infoHashV1 ? (
          <Row label={contents.infoHashV2 ? 'Info hash (v1)' : 'Info hash'}>
            <span className="font-mono text-[11px]">{contents.infoHashV1}</span>
          </Row>
        ) : null}
        {contents?.infoHashV2 ? (
          <Row label={contents.infoHashV1 ? 'Info hash (v2)' : 'Info hash'}>
            <span className="break-all font-mono text-[11px]">{contents.infoHashV2}</span>
          </Row>
        ) : null}
        {contents ? <Row label="Version">{VERSION_LABEL[contents.version]}</Row> : null}
        {contents?.private ? <Row label="Private">Yes. DHT and peer exchange aren’t used for this torrent.</Row> : null}
        {d.status === 'fetchingMetadata' ? <Row label="Metadata">Waiting for peers to send the torrent’s metadata</Row> : null}
        {d.torrent ? (
          <>
            <Row label="Peers">{`${String(d.torrent.peers)} connected, ${String(d.torrent.seeds)} with the whole torrent`}</Row>
            <Row label="Uploaded">{formatBytes(d.torrent.uploadedBytes)}</Row>
            <Row label="Upload speed">{formatSpeed(d.progress.uploadSpeedBps)}</Row>
            <Row label="Time seeding">{formatDuration(d.torrent.seedingSecs)}</Row>
          </>
        ) : (
          <Row label="Peers">Not reported yet: the torrent helper hasn’t sent them since Velocity started.</Row>
        )}
        {t ? <Row label="Seeding">{seedingSummary(t.seeding)}</Row> : null}
        {queued ? <Row label="Seeding queue">Waiting for a free seeding slot: only so many torrents seed at once.</Row> : null}
      </dl>

      {!live ? null : notRunning ? (
        <p className="mt-2 text-xs text-fg-muted">Trackers and peers show once the torrent runs.</p>
      ) : trackers.isError ? (
        <p className="mt-2 text-xs text-fg-muted">{toApiError(trackers.error).message}</p>
      ) : trackers.data && trackers.data.length > 0 ? (
        <div className="mt-3">
          <p className="mb-1.5 text-xs text-fg-subtle">Trackers</p>
          <ul aria-label="Trackers" className="space-y-1.5 text-xs">
            {trackers.data.map((tr) => (
              <li key={`${String(tr.tier)}:${tr.url}`} className="min-w-0">
                <div className="flex justify-between gap-2">
                  <span className="truncate font-mono text-[11px] text-fg-muted">{tr.url}</span>
                  <span className={tr.state === 'error' ? 'shrink-0 text-danger' : tr.state === 'working' ? 'shrink-0 text-success' : 'shrink-0 text-fg-subtle'}>{TRACKER_STATE[tr.state]}</span>
                </div>
                {trackerSummary(tr) ? <p className="text-fg-subtle">{trackerSummary(tr)}</p> : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {live && peers.data && peers.data.length > 0 ? (
        <div className="mt-3">
          <p className="mb-1.5 text-xs text-fg-subtle">Connected peers</p>
          <ul aria-label="Peers" className="space-y-1 text-xs">
            {peers.data.slice(0, PEERS_SHOWN).map((p) => (
              <li key={`${p.ip}:${String(p.port)}`} className="flex justify-between gap-2 tabular-nums">
                <span className="truncate font-mono text-[11px] text-fg-muted" title={p.client}>
                  {p.ip}:{p.port} · {p.client || 'Unknown client'}
                  {p.seed ? ' · seed' : ` · ${String(Math.round(p.progressPermille / 10))}%`}
                </span>
                <span className="shrink-0">
                  ↓ {formatSpeed(p.downloadSpeedBps)} ↑ {formatSpeed(p.uploadSpeedBps)}
                </span>
              </li>
            ))}
          </ul>
          {peers.data.length > PEERS_SHOWN ? <p className="mt-1 text-xs text-fg-subtle">and {peers.data.length - PEERS_SHOWN} more</p> : null}
        </div>
      ) : null}

      {live && d.status !== 'fetchingMetadata' ? (
        <div className="mt-3 flex items-center justify-between gap-2 text-xs">
          <label htmlFor="details-upload-limit" className="flex items-center gap-1.5 text-fg-subtle">
            <ArrowUpFromLine className="size-3.5" aria-hidden /> Upload limit (KiB/s, 0 for none)
          </label>
          <Input
            id="details-upload-limit"
            type="number"
            min={0}
            inputMode="numeric"
            className="h-8 w-24"
            value={typedKib}
            onChange={(e) => { setUpload({ id: d.id, kib: e.target.value }); }}
            onBlur={commitUpload}
            onKeyDown={(e) => { if (e.key === 'Enter') commitUpload(); }}
          />
        </div>
      ) : null}

      <div className="mt-3 flex flex-wrap gap-2">
        {d.status === 'choosingFiles' ? (
          <Button size="sm" variant="primary" onClick={() => { openChooseFiles(d.id); }}>
            <ListChecks /> Choose files…
          </Button>
        ) : null}
        {d.status === 'seeding' ? (
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void actions.stopSeeding([d.id])}>
            <UploadCloud /> Stop seeding
          </Button>
        ) : null}
        {queued ? (
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void actions.stopSeeding([d.id])}>
            <UploadCloud /> Leave seeding queue
          </Button>
        ) : null}
        {trackers.isSuccess && d.status !== 'paused' ? (
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void actions.reannounce(d.id)}>
            <Radio /> Re-announce
          </Button>
        ) : null}
      </div>

      {files.length > 0 && d.status !== 'choosingFiles' ? (
        <div className="mt-3">
          <p className="mb-1.5 text-xs text-fg-subtle">
            Files ({shown.size} of {files.length} chosen)
          </p>
          <FileTree files={files} selected={shown} onChange={(next) => { setDraft({ id: d.id, files: next }); }} disabled={!editable || busy} />
          {editable && changed ? (
            <div className="mt-2 space-y-2 text-xs">
              <Checkbox checked={deleteDropped} onChange={setDeleteDropped}>
                Delete files I removed
              </Checkbox>
              <p className="text-fg-subtle">{deleteDropped ? 'Only files this torrent wrote are deleted.' : 'Files you remove stay on disk; the torrent stops downloading them.'}</p>
              <div className="flex gap-2">
                <Button size="sm" variant="primary" disabled={shown.size === 0 || busy} onClick={() => void applyFiles()}>
                  Apply file changes
                </Button>
                <Button size="sm" variant="ghost" disabled={busy} onClick={() => { setDraft(null); setDeleteDropped(false); }}>
                  Undo
                </Button>
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
