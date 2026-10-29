import type { DownloadRecord, DownloadStatus } from '../contract';

/** The lists the sidebar offers (V1's list views). */
export type ListView = 'all' | 'downloading' | 'queued' | 'completed' | 'seeding' | 'failed';

// V1's rules (src/features/downloads/logic.ts), without its seeding-queue flag, which V2's snapshot doesn't carry.
const VIEW_STATUSES: Record<Exclude<ListView, 'all'>, readonly DownloadStatus[]> = {
  downloading: ['connecting', 'downloading', 'verifying', 'fetchingMetadata', 'stalled'],
  queued: ['queued', 'paused', 'retrying', 'choosingFiles'],
  // A seeding torrent has finished downloading, so it is listed as completed too.
  completed: ['completed', 'seeding'],
  seeding: ['seeding'],
  failed: ['failed', 'removed'],
};

export function matchesView(d: DownloadRecord, view: ListView): boolean {
  // A torrent waiting for a seeding slot is listed with the seeding ones too (V1's, #63).
  if (view === 'seeding' && d.torrent?.seedQueued) return true;
  return view === 'all' || VIEW_STATUSES[view].includes(d.status);
}

/** How many downloads each list has. */
export function countByView(downloads: readonly DownloadRecord[]): Record<ListView, number> {
  const counts: Record<ListView, number> = { all: downloads.length, downloading: 0, queued: 0, completed: 0, seeding: 0, failed: 0 };
  for (const d of downloads) {
    for (const view of ['downloading', 'queued', 'completed', 'seeding', 'failed'] as const) {
      if (matchesView(d, view)) counts[view] += 1;
    }
  }
  return counts;
}
