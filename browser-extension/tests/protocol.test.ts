import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { buildAdd, canSend, isMagnet, parseResponse } from '../src/shared/protocol';
import { sendNative } from '../src/shared/native';

const fixtures = JSON.parse(readFileSync(new URL('../../tests/fixtures/contract/browser-messages.json', import.meta.url), 'utf8')) as { valid: unknown[]; invalid: unknown[] };

describe('protocol', () => {
  it('builds add requests that drop unsafe optional fields', () => {
    const r = buildAdd({ url: 'https://x.test/a.zip', source: 'intercept', referrer: 'javascript:alert(1)', filename: 'C:\\tmp\\..\\evil.exe', fileSize: -1, mime: 'a\u0000b' });
    expect(r).toMatchObject({ v: 1, type: 'download.add', payload: { url: 'https://x.test/a.zip', source: 'intercept', filename: 'evil.exe' } });
    expect(r.type === 'download.add' && r.payload).not.toHaveProperty('referrer');
    expect(r.type === 'download.add' && r.payload).not.toHaveProperty('fileSize');
    expect(r.type === 'download.add' && r.payload).not.toHaveProperty('mime');
    expect(r.id).toMatch(/^[A-Za-z0-9-]{1,64}$/);
  });

  it('produces messages with the same shape as the shared contract fixture', () => {
    // The Rust protocol tests accept every fixture in `valid` and reject every one in `invalid`.
    const r = buildAdd({ url: 'https://example.com/file.zip', source: 'contextMenu', referrer: 'https://example.com/page', filename: 'file.zip', fileSize: 1024, mime: 'application/zip' });
    const sample = fixtures.valid.find((m) => (m as { type: string }).type === 'download.add') as Record<string, unknown>;
    expect(Object.keys(r).sort()).toEqual(Object.keys(sample).sort());
    expect(Object.keys((r as { payload: object }).payload).sort()).toEqual(Object.keys(sample.payload as object).sort());
  });

  it('only sends http(s) links', () => {
    expect(canSend('https://x.test/a')).toBe(true);
    expect(canSend('file:///etc/passwd')).toBe(false);
    expect(canSend('https://x.test/' + 'a'.repeat(9000))).toBe(false);
  });

  it('also sends magnet links with an info hash, bounded in length', () => {
    const m = 'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x';
    expect(canSend(m)).toBe(true);
    expect(isMagnet('MAGNET:?xt=urn:btih:abc')).toBe(true);
    expect(canSend('magnet:evil')).toBe(false);
    expect(canSend('magnet:?dn=no-hash')).toBe(false);
    expect(canSend(m + '&tr=' + 'a'.repeat(9000))).toBe(false);
    expect(isMagnet('https://x.test/magnet:?xt=urn:btih:abc')).toBe(false);
  });

  it('validates responses', () => {
    expect(parseResponse({ v: 1, id: 'a', ok: true, result: { x: 1 } }, 'a').ok).toBe(true);
    expect(parseResponse({ v: 2, id: 'a', ok: true, result: {} }, 'a')).toMatchObject({ ok: false, error: { code: 'INVALID_RESPONSE' } });
    expect(parseResponse('nope', 'a')).toMatchObject({ ok: false });
    expect(parseResponse({ v: 1, id: 'a', ok: false, error: { code: 'DISABLED', message: 'off' } }, 'a')).toMatchObject({ ok: false, error: { code: 'DISABLED' } });
  });

  it('maps a missing native host and timeouts to clear errors', async () => {
    const missing = await sendNative({ sendNativeMessage: () => Promise.reject(new Error('Specified native messaging host not found.')) }, { v: 1, id: 'p', type: 'ping' });
    expect(missing).toMatchObject({ ok: false, error: { code: 'HOST_NOT_INSTALLED' } });
    const slow = await sendNative({ sendNativeMessage: () => new Promise(() => undefined) }, { v: 1, id: 'p', type: 'ping' }, 20);
    expect(slow).toMatchObject({ ok: false, error: { code: 'TIMEOUT' } });
  });
});
