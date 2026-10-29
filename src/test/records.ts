import type { DownloadRecord } from '../contract';
import { fixture } from './fakeBackend';

/** A download record for tests: the contract fixture's, queued with nothing measured, with `over` on top. */
export function makeRecord(over: Partial<DownloadRecord> = {}): DownloadRecord {
  const sample = fixture('download-record.json') as DownloadRecord;
  return {
    ...sample,
    status: 'queued',
    error: null,
    retryAtMs: null,
    startedAtMs: null,
    averageSpeedBps: null,
    progress: { ...sample.progress, connectionDetail: null },
    ...over,
  };
}
