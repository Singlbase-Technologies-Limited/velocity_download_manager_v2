import { describe, expect, it } from 'vitest';
import { makeRecord } from '../test/records';
import { STATUS_LABEL, actionsFor, actionsForSelection, displayName, etaSeconds, matchesSearch, percentOf, sortDownloads, statusLabel, visibleDownloads } from './list';
import { countByView } from './views';

const progress = makeRecord().progress;

// V1's tests/unit/logic.test.ts, on V2's records. Not ported: the seeding queue (V2's snapshot doesn't carry it), opening
// files and the connection summary (the details panel, #61), and choosing a torrent's files (#63).
describe('download list logic', () => {
  const a = makeRecord({ id: 'a', fileName: 'Alpha.zip', status: 'downloading', createdAtMs: 1, progress: { ...progress, totalBytes: 10, downloadSpeedBps: 5 } });
  const b = makeRecord({ id: 'b', fileName: 'beta.iso', status: 'paused', categoryId: 'video', createdAtMs: 2, progress: { ...progress, totalBytes: 30, downloadSpeedBps: 0 } });
  const c = makeRecord({ id: 'c', fileName: 'gamma.mp4', status: 'completed', createdAtMs: 3, progress: { ...progress, totalBytes: null } });
  const d = makeRecord({ id: 'd', fileName: 'delta.bin', status: 'failed', sourceUrl: 'https://mirror.test/delta.bin', createdAtMs: 4 });
  const all = [a, b, c, d];

  it('counts downloads per view', () => {
    expect(countByView(all)).toEqual({ all: 4, downloading: 1, queued: 1, completed: 1, seeding: 0, failed: 1 });
  });

  it('filters by view, search and category', () => {
    const sort = { key: 'added', dir: 'asc' } as const;
    expect(visibleDownloads(all, { view: 'queued', search: '', category: null, sort }).map((x) => x.id)).toEqual(['b']);
    expect(visibleDownloads(all, { view: 'all', search: 'MIRROR', category: null, sort }).map((x) => x.id)).toEqual(['d']);
    expect(visibleDownloads(all, { view: 'all', search: '', category: 'video', sort }).map((x) => x.id)).toEqual(['b']);
    expect(matchesSearch(a, '   ')).toBe(true);
  });

  it('sorts by name, size and added date in both directions', () => {
    expect(sortDownloads(all, { key: 'name', dir: 'asc' }).map((x) => x.id)).toEqual(['a', 'b', 'd', 'c']);
    expect(sortDownloads(all, { key: 'size', dir: 'desc' }).map((x) => x.id)).toEqual(['d', 'b', 'a', 'c']);
    expect(sortDownloads(all, { key: 'added', dir: 'desc' }).map((x) => x.id)).toEqual(['d', 'c', 'b', 'a']);
  });

  it('sorts by queue position: the snapshot’s order, finished ones last', () => {
    const queue = [b, a, c, d];
    expect(sortDownloads(queue, { key: 'queue', dir: 'asc' }).map((x) => x.id)).toEqual(['b', 'a', 'c', 'd']);
  });

  it('offers only actions valid for each status', () => {
    expect(actionsFor(a)).toMatchObject({ pause: true, resume: false, retry: false });
    expect(actionsFor(b)).toMatchObject({ pause: false, resume: true, move: true });
    expect(actionsFor(c)).toMatchObject({ pause: false, cancel: false });
    expect(actionsFor(d)).toMatchObject({ retry: true, resume: false, cancel: false });
  });

  it('combines actions for multi-selection and restricts single-item actions', () => {
    const sel = actionsForSelection([a, d]);
    expect(sel.pause).toBe(true);
    expect(sel.retry).toBe(true);
    expect(sel.move).toBe(false);
    expect(actionsForSelection([b]).move).toBe(true);
    expect(actionsForSelection([]).remove).toBe(false);
  });

  it('offers nothing but removal for a download this build can’t act on', () => {
    expect(actionsFor(makeRecord({ status: 'unknown' }))).toEqual({
      pause: false, resume: false, cancel: false, retry: false, move: false, remove: true, deleteFiles: false, stopSeeding: false,
    });
  });

  describe('torrents', () => {
    const fetching = makeRecord({ id: 'm', kind: 'magnet', status: 'fetchingMetadata' });
    const stalled = makeRecord({ id: 's', kind: 'torrent', status: 'stalled' });
    const choosing = makeRecord({ id: 'f', kind: 'magnet', status: 'choosingFiles' });
    const seeding = makeRecord({ id: 'z', kind: 'torrent', status: 'seeding' });

    it('places torrent states in the right views', () => {
      expect(countByView([fetching, stalled, choosing, seeding])).toEqual({ all: 4, downloading: 2, queued: 1, completed: 1, seeding: 1, failed: 0 });
    });

    it('offers torrent actions for each state', () => {
      expect(actionsFor(fetching)).toMatchObject({ pause: true, cancel: true, stopSeeding: false });
      expect(actionsFor(stalled)).toMatchObject({ pause: true, cancel: true, retry: false });
      expect(actionsFor(choosing)).toMatchObject({ cancel: true, pause: false, resume: false });
      expect(actionsFor(seeding)).toMatchObject({ stopSeeding: true, pause: true, cancel: false, retry: false });
      expect(actionsForSelection([choosing, seeding]).stopSeeding).toBe(true);
    });

    it('labels every torrent state', () => {
      expect(STATUS_LABEL.fetchingMetadata).toBe('Getting metadata');
      expect(STATUS_LABEL.choosingFiles).toBe('Choose files');
      expect(STATUS_LABEL.stalled).toBe('Waiting for peers');
      expect(STATUS_LABEL.seeding).toBe('Seeding');
    });
  });

  it('says "waiting for peers" only for torrents', () => {
    const now = 0;
    expect(statusLabel(makeRecord({ kind: 'torrent', status: 'stalled' }), now)).toBe('Waiting for peers');
    expect(statusLabel(makeRecord({ kind: 'magnet', status: 'stalled' }), now)).toBe('Waiting for peers');
    expect(statusLabel(makeRecord({ kind: 'http', status: 'stalled' }), now)).toBe('No data received');
  });
});

describe('what the list shows (#59)', () => {
  const now = Date.parse('2026-10-29T12:00:00Z');

  it('counts down to the engine’s next attempt, and never invents one', () => {
    const retrying = (retryAtMs: number | null) => makeRecord({ status: 'retrying', retryAtMs });
    expect(statusLabel(retrying(now + 28_000), now)).toBe('Retrying in 28s');
    // The window's `now` is whole seconds, so up to a second over is still the whole second (#59's flaky row test).
    expect(statusLabel(retrying(now + 30_999), now)).toBe('Retrying in 30s');
    expect(statusLabel(retrying(now + 90_000), now)).toBe('Retrying in 1m 30s');
    expect(statusLabel(retrying(now - 500), now)).toBe('Retrying now');
    expect(statusLabel(retrying(null), now)).toBe('Waiting to retry');
  });

  it('names a verifying download and one waiting for its login', () => {
    expect(statusLabel(makeRecord({ status: 'verifying' }), now)).toBe('Verifying');
    expect(statusLabel(makeRecord({ status: 'paused' }), now, { needsLogin: true })).toBe('Needs its login');
  });

  it('shows the progress the bytes prove, and none for an unknown size', () => {
    expect(percentOf(makeRecord({ progress: { ...progress, completedBytes: 1, totalBytes: 4 } }))).toBe(25);
    expect(percentOf(makeRecord({ progress: { ...progress, completedBytes: 5, totalBytes: null } }))).toBeNull();
    expect(percentOf(makeRecord({ status: 'completed', progress: { ...progress, totalBytes: null } }))).toBe(100);
  });

  it('gives a time left only while bytes come, from the speed the engine reports', () => {
    const at = (status: 'downloading' | 'stalled', speed: number, total: number | null) =>
      makeRecord({ status, progress: { ...progress, completedBytes: 1000, totalBytes: total, downloadSpeedBps: speed } });
    expect(etaSeconds(at('downloading', 100, 3000))).toBe(20);
    expect(etaSeconds(at('downloading', 0, 3000))).toBeNull();
    expect(etaSeconds(at('downloading', 100, null))).toBeNull();
    expect(etaSeconds(at('stalled', 100, 3000))).toBeNull();
  });

  it('names a download by its file, or by its link until the server names it', () => {
    expect(displayName(makeRecord({ fileName: 'a.zip' }))).toBe('a.zip');
    expect(displayName(makeRecord({ fileName: null, sourceUrl: 'https://example.com/get?id=3' }))).toBe('https://example.com/get?id=3');
  });
});
