import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  AppInfoSchema,
  BatchResultSchema,
  CapabilitiesSchema,
  CategoryIconSchema,
  CategorySchema,
  ConnectionLimitSchema,
  COMMAND_API_VERSION,
  DownloadRecordSchema,
  DownloadsSnapshotSchema,
  DownloadStatusSchema,
  EngineHealthSchema,
  ErrorCodeSchema,
  KeptReasonSchema,
  parseAppInfo,
  VerificationSchema,
} from './contract';

// Fixtures are generated from the Rust types (crates/velocity-core/src/domain.rs):
// VELOCITY_UPDATE_FIXTURES=1 cargo test -p velocity-core contract
const fixture = (name: string): unknown =>
  JSON.parse(readFileSync(join(__dirname, '..', 'tests', 'fixtures', 'contract', name), 'utf8'));

describe('contract fixtures generated from Rust', () => {
  it('statuses: exactly the values the schema knows, in order', () => {
    expect(fixture('statuses.json')).toEqual(DownloadStatusSchema.options);
  });

  it('connection limits: exactly the values the schema knows, in order', () => {
    expect(fixture('connection-limits.json')).toEqual(ConnectionLimitSchema.options);
  });

  it('verifications: exactly the values the schema knows, in order', () => {
    expect(fixture('verifications.json')).toEqual(VerificationSchema.options);
  });

  it('error codes: exactly the codes the schema knows, each with a user-facing message', () => {
    const codes = fixture('error-codes.json') as { code: string; message: string }[];
    expect(codes.map((c) => c.code)).toEqual(ErrorCodeSchema.options);
    for (const c of codes) expect(c.message, c.code).toMatch(/^[A-Z].{10,}\.$/);
  });

  it('download record', () => {
    expect(DownloadRecordSchema.parse(fixture('download-record.json'))).toEqual(fixture('download-record.json'));
  });

  it('capabilities', () => {
    expect(CapabilitiesSchema.parse(fixture('capabilities.json'))).toEqual(fixture('capabilities.json'));
  });

  it('downloads snapshot (#58)', () => {
    expect(DownloadsSnapshotSchema.parse(fixture('downloads-snapshot.json'))).toEqual(fixture('downloads-snapshot.json'));
  });

  it('engine health: running, restarting and unavailable (#58)', () => {
    const all = fixture('engine-health.json') as unknown[];
    expect(all.map((h) => EngineHealthSchema.parse(h))).toEqual(all);
  });

  it('batch result (#58)', () => {
    expect(BatchResultSchema.parse(fixture('batch-result.json'))).toEqual(fixture('batch-result.json'));
  });

  it('kept reasons: exactly the values the schema knows, in order (#58)', () => {
    expect(fixture('kept-reasons.json')).toEqual(KeptReasonSchema.options);
  });

  it('category (#31)', () => {
    expect(CategorySchema.parse(fixture('category.json'))).toEqual(fixture('category.json'));
  });

  it('category icons: exactly the values the schema knows, in order (#31)', () => {
    expect(fixture('category-icons.json')).toEqual(CategoryIconSchema.options);
  });

  it('app info carries the same contract version as this UI', () => {
    const info = AppInfoSchema.parse(fixture('app-info.json'));
    expect(info.commandApiVersion).toBe(COMMAND_API_VERSION);
  });
});

describe('schemas reject drift', () => {
  const record = () => structuredClone(fixture('download-record.json')) as Record<string, unknown>;

  it('a field the UI doesn’t know (Rust added a field)', () => {
    expect(() => DownloadRecordSchema.parse({ ...record(), engineGid: 'x' })).toThrow();
  });

  it('a missing field (Rust removed or renamed a field)', () => {
    const r = record();
    delete r.sourceUrl;
    expect(() => DownloadRecordSchema.parse(r)).toThrow();
  });

  it('an unknown status or error class', () => {
    expect(() => DownloadRecordSchema.parse({ ...record(), status: 'active' })).toThrow();
    expect(() =>
      DownloadRecordSchema.parse({ ...record(), error: { class: 'fatal', httpStatus: null, message: 'x', retryable: false } }),
    ).toThrow();
  });

  it('an engine state the UI doesn’t know', () => {
    expect(() => EngineHealthSchema.parse({ state: 'sleeping' })).toThrow();
    expect(() => EngineHealthSchema.parse({ state: 'restarting' })).toThrow();
  });

  it('a nested field with the wrong type', () => {
    const r = record();
    (r.progress as Record<string, unknown>).completedBytes = '10';
    expect(() => DownloadRecordSchema.parse(r)).toThrow();
  });
});

describe('parseAppInfo', () => {
  const ok = { name: 'Velocity Download Manager (dev)', version: '0.1.0', commandApiVersion: COMMAND_API_VERSION };

  it('accepts the native response for this contract version', () => {
    expect(parseAppInfo(ok)).toEqual(ok);
  });

  it('refuses a native side with another contract version, saying so', () => {
    expect(() => parseAppInfo({ ...ok, commandApiVersion: COMMAND_API_VERSION + 1 })).toThrow(
      /contract version 9.*expects 8/,
    );
  });

  it('refuses anything that isn’t app info', () => {
    expect(() => parseAppInfo(null)).toThrow('Unexpected app_info response');
    expect(() => parseAppInfo({ name: 'x', version: '0.1.0' })).toThrow('Unexpected app_info response');
  });
});
