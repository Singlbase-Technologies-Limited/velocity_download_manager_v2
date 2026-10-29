import { describe, expect, it } from 'vitest';
import { formatBytes, formatDuration, formatPercent, formatShortDate, formatSpeed, formatTransferred, formatTransferredBytes, relativeFromNow } from './format';

describe('formatBytes', () => {
  it('says bytes in binary units, as the tray does', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1023)).toBe('1023 B');
    expect(formatBytes(1024)).toBe('1.0 KiB');
    expect(formatBytes(1.5 * 1024 * 1024)).toBe('1.5 MiB');
    expect(formatBytes(5 * 1024 ** 3)).toBe('5.0 GiB');
    expect(formatBytes(2 * 1024 ** 5)).toBe('2048.0 TiB');
  });

  // V1's tests/unit/format.test.ts, with V2's unit names (KiB, as the tray says them).
  it('never invents a value for an unknown size', () => {
    expect(formatBytes(null)).toBe('—');
    expect(formatBytes(-1)).toBe('—');
    expect(formatBytes(Number.NaN)).toBe('—');
  });

  it('a speed is per second', () => {
    expect(formatSpeed(2 * 1024 * 1024)).toBe('2.0 MiB/s');
    expect(formatSpeed(null)).toBe('—');
  });
});

describe('formatting (V1)', () => {
  it('formats durations and percentages', () => {
    expect(formatDuration(59)).toBe('59s');
    expect(formatDuration(61)).toBe('1m 01s');
    expect(formatDuration(3 * 3600 + 120)).toBe('3h 02m');
    expect(formatDuration(50 * 3600)).toBe('2d 2h');
    expect(formatDuration(null)).toBe('—');
    expect(formatDuration(-1)).toBe('—');
    expect(formatPercent(5.25)).toBe('5.3%');
    expect(formatPercent(42.4)).toBe('42%');
    expect(formatPercent(100)).toBe('100%');
    expect(formatPercent(null)).toBe('—');
  });

  it('describes relative times', () => {
    const now = Date.parse('2026-01-01T00:00:00Z');
    expect(relativeFromNow(Date.parse('2026-01-01T00:01:00Z'), now)).toBe('in 1m 00s');
    expect(relativeFromNow(Date.parse('2025-12-31T23:59:30Z'), now)).toBe('30s ago');
  });
});

describe('formatShortDate (V1)', () => {
  it('drops the date today and the year this year', () => {
    const now = new Date(2026, 9, 18, 15, 0);
    const today = formatShortDate(new Date(2026, 9, 18, 12, 35).getTime(), now);
    const thisYear = formatShortDate(new Date(2026, 2, 3, 9, 5).getTime(), now);
    const lastYear = formatShortDate(new Date(2025, 2, 3, 9, 5).getTime(), now);
    expect(today).not.toMatch(/2026/);
    expect(today).toMatch(/35/);
    expect(thisYear).not.toMatch(/2026/);
    expect(lastYear).toMatch(/2025/);
    expect(formatShortDate(null, now)).toBe('—');
  });
});

describe('transferred sizes (V1)', () => {
  const GiB = 1024 ** 3;
  it('shows large transfers precisely enough to see them move', () => {
    expect(formatTransferredBytes(3.27 * GiB)).toBe('3.27 GiB');
    expect(formatTransferredBytes(3.28 * GiB)).toBe('3.28 GiB');
    expect(formatTransferredBytes(250 * 1024 ** 2)).toBe('250.0 MiB');
    expect(formatTransferredBytes(1536)).toBe('1.5 KiB');
    expect(formatTransferredBytes(null)).toBe('—');
  });

  it('uses the unit of the total for both sides', () => {
    expect(formatTransferred(512 * 1024 ** 2, 3.8 * GiB)).toBe('0.50 GiB of 3.80 GiB');
    expect(formatTransferred(3.8 * GiB, 3.8 * GiB)).toBe('3.80 GiB of 3.80 GiB');
    expect(formatTransferred(2048, null)).toBe('2.0 KiB of unknown size');
  });
});
