import { describe, expect, it } from 'vitest';
import { formatBytes, formatSpeed } from './format';

describe('formatBytes', () => {
  it('says bytes in binary units, as the tray does', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1023)).toBe('1023 B');
    expect(formatBytes(1024)).toBe('1.0 KiB');
    expect(formatBytes(1.5 * 1024 * 1024)).toBe('1.5 MiB');
    expect(formatBytes(5 * 1024 ** 3)).toBe('5.0 GiB');
    expect(formatBytes(2 * 1024 ** 5)).toBe('2048.0 TiB');
  });

  it('a speed is per second', () => {
    expect(formatSpeed(2 * 1024 * 1024)).toBe('2.0 MiB/s');
  });
});
