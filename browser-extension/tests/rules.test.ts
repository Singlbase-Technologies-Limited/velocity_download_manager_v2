import { describe, expect, it } from 'vitest';
import { candidateName, decide, extensionOf, hostMatches, RecentUrls } from '../src/shared/rules';
import { DEFAULT_SETTINGS, sanitizeSettings } from '../src/shared/settings';

const s = (over: Partial<typeof DEFAULT_SETTINGS> = {}) => ({ ...DEFAULT_SETTINGS, ...over });

describe('interception rules', () => {
  it('intercepts configured file types over http(s)', () => {
    expect(decide({ url: 'https://cdn.test/a/file.ZIP' }, s(), 'me')).toEqual({ intercept: true });
    expect(decide({ url: 'https://cdn.test/dl?id=1', filename: '/home/u/Downloads/setup.exe' }, s(), 'me').intercept).toBe(true);
  });

  it('intercepts large files by size when the type is unknown', () => {
    expect(decide({ url: 'https://cdn.test/blob', fileSize: 25 * 1024 * 1024 }, s(), 'me').intercept).toBe(true);
    expect(decide({ url: 'https://cdn.test/blob', fileSize: 5 * 1024 * 1024 }, s(), 'me').intercept).toBe(false);
    expect(decide({ url: 'https://cdn.test/blob', fileSize: 25 * 1024 * 1024 }, s({ minSizeMib: 0 }), 'me').intercept).toBe(false);
  });

  it('never intercepts browser-only schemes, private windows, or its own fallback downloads', () => {
    expect(decide({ url: 'blob:https://x.test/abc', filename: 'a.zip' }, s(), 'me').intercept).toBe(false);
    expect(decide({ url: 'data:application/zip;base64,AA', filename: 'a.zip' }, s(), 'me').intercept).toBe(false);
    expect(decide({ url: 'https://x.test/a.zip', incognito: true }, s(), 'me').intercept).toBe(false);
    expect(decide({ url: 'https://x.test/a.zip', incognito: true }, s({ includePrivate: true }), 'me').intercept).toBe(true);
    expect(decide({ url: 'https://x.test/a.zip', byExtensionId: 'me' }, s(), 'me').intercept).toBe(false);
    expect(decide({ url: 'https://x.test/a.zip' }, s({ mode: 'off' }), 'me').intercept).toBe(false);
  });

  it('applies allow and deny lists including subdomains', () => {
    expect(decide({ url: 'https://dl.bank.test/a.zip' }, s({ denyDomains: ['bank.test'] }), 'me').intercept).toBe(false);
    expect(decide({ url: 'https://notbank.test/a.zip' }, s({ denyDomains: ['bank.test'] }), 'me').intercept).toBe(true);
    expect(decide({ url: 'https://other.test/a.zip' }, s({ allowDomains: ['cdn.test'] }), 'me').intercept).toBe(false);
    expect(decide({ url: 'https://eu.cdn.test/a.zip' }, s({ allowDomains: ['cdn.test'] }), 'me').intercept).toBe(true);
    expect(hostMatches('EXAMPLE.test', ['example.test'])).toBe(true);
  });

  it('derives names and extensions', () => {
    expect(extensionOf('C:\\Users\\a\\file.tar.GZ')).toBe('gz');
    expect(extensionOf('.bashrc')).toBe('');
    expect(candidateName({ url: 'https://x.test/a%20b.iso?x=1' })).toBe('a b.iso');
  });

  it('deduplicates repeated URLs within the window', () => {
    const r = new RecentUrls(1000);
    expect(r.check('u', 0)).toBe(false);
    expect(r.check('u', 500)).toBe(true);
    expect(r.check('u', 2000)).toBe(false);
  });
});

describe('settings sanitization', () => {
  it('falls back to defaults for invalid values', () => {
    expect(sanitizeSettings(null)).toEqual(DEFAULT_SETTINGS);
    const out = sanitizeSettings({ mode: 'hack', minSizeMib: -5, fileTypes: ['.ZIP', 'bad ext', 3, 'zip'], denyDomains: ['*.Bank.test', 'not a domain', 'bad_domain!'], includePrivate: 'yes' });
    expect(out.mode).toBe('auto');
    expect(out.minSizeMib).toBe(0);
    expect(out.fileTypes).toEqual(['zip']);
    expect(out.denyDomains).toEqual(['bank.test']);
    expect(out.includePrivate).toBe(false);
  });

  it('leaves .torrent downloads alone unless torrent capture is on', () => {
    const t = { url: 'https://tracker.test/dl/linux.torrent', mime: 'application/x-bittorrent', fileSize: 30_000 };
    expect(decide(t, DEFAULT_SETTINGS, 'self')).toMatchObject({ intercept: false });
    expect(decide(t, { ...DEFAULT_SETTINGS, captureTorrents: true }, 'self')).toEqual({ intercept: true });
    expect(decide({ url: 'https://x.test/get?id=1', mime: 'application/x-bittorrent', fileSize: 100 }, { ...DEFAULT_SETTINGS, captureTorrents: true }, 'self')).toEqual({ intercept: true });
    expect(decide(t, { ...DEFAULT_SETTINGS, captureTorrents: true, mode: 'off' }, 'self')).toMatchObject({ intercept: false });
    expect(decide(t, { ...DEFAULT_SETTINGS, captureTorrents: true, denyDomains: ['tracker.test'] }, 'self')).toMatchObject({ intercept: false });
  });

  it('keeps torrent capture off by default and sanitizes it', () => {
    expect(DEFAULT_SETTINGS.captureTorrents).toBe(false);
    expect(sanitizeSettings({ captureTorrents: true }).captureTorrents).toBe(true);
    expect(sanitizeSettings({ captureTorrents: 'yes' }).captureTorrents).toBe(false);
  });
});
