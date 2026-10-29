import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { DownloadRecord, DownloadStatus } from '../contract';
import { countByView } from './views';

const sample = JSON.parse(
  readFileSync(join(__dirname, '..', '..', 'tests', 'fixtures', 'contract', 'download-record.json'), 'utf8'),
) as DownloadRecord;
const records = (statuses: DownloadStatus[]) => statuses.map((status, i) => ({ ...sample, id: String(i), status }));

describe('countByView', () => {
  it('counts each download in every list it belongs to', () => {
    expect(countByView(records(['downloading', 'connecting', 'paused', 'retrying', 'seeding', 'completed', 'failed', 'removed']))).toEqual({
      all: 8,
      downloading: 2,
      queued: 2,
      completed: 2,
      seeding: 1,
      failed: 2,
    });
  });

  it('is all zeros with no downloads', () => {
    expect(countByView([])).toEqual({ all: 0, downloading: 0, queued: 0, completed: 0, seeding: 0, failed: 0 });
  });
});
