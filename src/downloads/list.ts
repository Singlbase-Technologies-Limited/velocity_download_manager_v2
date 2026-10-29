import type { DownloadRecord, DownloadStatus } from '../contract';
import { formatDuration } from '../lib/format';
import { matchesView, type ListView } from './views';

// The download list's rules (V1's src/features/downloads/logic.ts): searching, sorting, the actions each status allows
// and how a status is named. Pure, so the table, its menus and the toolbar agree.

export type SortKey = 'name' | 'size' | 'progress' | 'speed' | 'eta' | 'status' | 'added' | 'completed' | 'category' | 'queue';
export interface SortState {
  key: SortKey;
  dir: 'asc' | 'desc';
}

/** The file's name, or the link until the server names it: never a name made up for it. */
export function displayName(d: DownloadRecord): string {
  return d.fileName ?? d.sourceUrl;
}

/** The share of the file the accepted bytes prove; `null` while the size is unknown. */
export function percentOf(d: DownloadRecord): number | null {
  if (d.status === 'completed' || d.status === 'seeding') return 100;
  const total = d.progress.totalBytes;
  return total ? (d.progress.completedBytes / total) * 100 : null;
}

/** Seconds left at the speed the engine reports, while bytes come; `null` otherwise. */
export function etaSeconds(d: DownloadRecord): number | null {
  const { totalBytes, completedBytes, downloadSpeedBps } = d.progress;
  if (d.status !== 'downloading' || !totalBytes || downloadSpeedBps <= 0) return null;
  return Math.max(0, totalBytes - completedBytes) / downloadSpeedBps;
}

export function matchesSearch(d: DownloadRecord, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [displayName(d), d.sourceUrl, d.directory].some((s) => s.toLowerCase().includes(q));
}

const STATUS_ORDER: Record<DownloadStatus, number> = {
  downloading: 0,
  fetchingMetadata: 1,
  connecting: 2,
  stalled: 3,
  verifying: 4,
  retrying: 5,
  choosingFiles: 6,
  queued: 7,
  paused: 8,
  failed: 9,
  removed: 10,
  seeding: 11,
  completed: 12,
  unknown: 13,
};

const TERMINAL: readonly DownloadStatus[] = ['completed', 'failed', 'removed'];

function sortValue(d: DownloadRecord, key: SortKey, queue: ReadonlyMap<string, number>, categoryName: (id: string | null) => string): string | number {
  switch (key) {
    case 'name':
      return displayName(d).toLowerCase();
    case 'size':
      return d.progress.totalBytes ?? -1;
    case 'progress':
      return percentOf(d) ?? -1;
    case 'speed':
      return d.progress.downloadSpeedBps;
    case 'eta':
      return etaSeconds(d) ?? Number.MAX_SAFE_INTEGER;
    case 'status':
      return STATUS_ORDER[d.status];
    case 'added':
      return d.createdAtMs;
    case 'completed':
      return d.completedAtMs ?? -1;
    case 'category':
      return categoryName(d.categoryId).toLowerCase();
    case 'queue':
      return queue.get(d.id) ?? Number.MAX_SAFE_INTEGER;
  }
}

/** Sorted by `sort`, then by when they were added. The snapshot lists unfinished downloads in queue order, so that is the queue's order. */
export function sortDownloads(list: readonly DownloadRecord[], sort: SortState, categoryName: (id: string | null) => string = () => ''): DownloadRecord[] {
  const queue = new Map(list.filter((d) => !TERMINAL.includes(d.status)).map((d, i) => [d.id, i]));
  const dir = sort.dir === 'asc' ? 1 : -1;
  return [...list].sort((a, b) => {
    const va = sortValue(a, sort.key, queue, categoryName);
    const vb = sortValue(b, sort.key, queue, categoryName);
    if (va < vb) return -dir;
    if (va > vb) return dir;
    return a.createdAtMs - b.createdAtMs;
  });
}

export function visibleDownloads(
  all: readonly DownloadRecord[],
  opts: { view: ListView; search: string; category: string | null; sort: SortState },
  categoryName?: (id: string | null) => string,
): DownloadRecord[] {
  const filtered = all.filter((d) => matchesView(d, opts.view) && matchesSearch(d, opts.search) && (opts.category === null || d.categoryId === opts.category));
  return sortDownloads(filtered, opts.sort, categoryName);
}

/** What the user may do to a download in its status. Opening files (#61) and choosing a torrent's files (#63) come later. */
export interface ActionAvailability {
  pause: boolean;
  resume: boolean;
  cancel: boolean;
  retry: boolean;
  move: boolean;
  remove: boolean;
  deleteFiles: boolean;
  stopSeeding: boolean;
}

const RUNNING: readonly DownloadStatus[] = ['connecting', 'downloading', 'verifying', 'fetchingMetadata', 'stalled'];

export function actionsFor(d: DownloadRecord): ActionAvailability {
  const s = d.status;
  const running = RUNNING.includes(s);
  return {
    pause: running || s === 'queued' || s === 'retrying' || s === 'seeding',
    resume: s === 'paused',
    cancel: running || s === 'queued' || s === 'paused' || s === 'retrying' || s === 'choosingFiles',
    retry: s === 'failed' || s === 'removed',
    move: s === 'queued' || s === 'paused',
    remove: true,
    deleteFiles: s !== 'unknown',
    stopSeeding: s === 'seeding',
  };
}

const NONE: ActionAvailability = { pause: false, resume: false, cancel: false, retry: false, move: false, remove: false, deleteFiles: false, stopSeeding: false };

/** An action is offered when any selected download allows it; moving takes exactly one. */
export function actionsForSelection(list: readonly DownloadRecord[]): ActionAvailability {
  return list.reduce<ActionAvailability>((acc, d) => {
    const a = actionsFor(d);
    return {
      pause: acc.pause || a.pause,
      resume: acc.resume || a.resume,
      cancel: acc.cancel || a.cancel,
      retry: acc.retry || a.retry,
      move: list.length === 1 && a.move,
      remove: acc.remove || a.remove,
      deleteFiles: acc.deleteFiles || a.deleteFiles,
      stopSeeding: acc.stopSeeding || a.stopSeeding,
    };
  }, NONE);
}

export const STATUS_LABEL: Record<DownloadStatus, string> = {
  queued: 'Queued',
  connecting: 'Connecting',
  downloading: 'Downloading',
  paused: 'Paused',
  verifying: 'Verifying',
  retrying: 'Waiting to retry',
  fetchingMetadata: 'Getting metadata',
  choosingFiles: 'Choose files',
  stalled: 'Waiting for peers',
  seeding: 'Seeding',
  completed: 'Completed',
  failed: 'Failed',
  removed: 'Cancelled',
  unknown: 'Unavailable',
};

/** The status as the list says it, at `now` (ms since the Unix epoch): Retrying counts down to the engine's next attempt. */
export function statusLabel(d: DownloadRecord, now: number, opts: { needsLogin?: boolean } = {}): string {
  if (opts.needsLogin) return 'Needs its login';
  if (d.status === 'retrying' && d.retryAtMs !== null) {
    const left = Math.round((d.retryAtMs - now) / 1000);
    return left > 0 ? `Retrying in ${formatDuration(left)}` : 'Retrying now';
  }
  // Only torrents have peers; a direct download that stalls is waiting on its server.
  if (d.status === 'stalled' && d.kind !== 'torrent' && d.kind !== 'magnet') return 'No data received';
  return STATUS_LABEL[d.status];
}

export type StatusTone = 'active' | 'neutral' | 'success' | 'danger' | 'warning' | 'muted';

export const STATUS_TONE: Record<DownloadStatus, StatusTone> = {
  queued: 'neutral',
  connecting: 'active',
  downloading: 'active',
  paused: 'warning',
  verifying: 'active',
  retrying: 'warning',
  fetchingMetadata: 'active',
  choosingFiles: 'warning',
  stalled: 'warning',
  seeding: 'success',
  completed: 'success',
  failed: 'danger',
  removed: 'muted',
  unknown: 'muted',
};
