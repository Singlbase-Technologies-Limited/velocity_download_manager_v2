import { beforeEach, describe, expect, it } from 'vitest';
import { createBackground, isContentMessage, isPageMessage, type ExtApi, type PendingPrompt } from '../src/background-core';
import { DEFAULT_SETTINGS } from '../src/shared/settings';

type Reply = (msg: Record<string, unknown>) => unknown;

function fakeApi(reply: Reply, settings: unknown = DEFAULT_SETTINGS) {
  const log: string[] = [];
  const sent: Record<string, unknown>[] = [];
  const session = new Map<string, PendingPrompt>();
  let nextWindow = 100;
  const api: ExtApi = {
    selfId: 'self',
    sendNativeMessage: (_app, msg) => {
      sent.push(msg as Record<string, unknown>);
      return Promise.resolve(reply(msg as Record<string, unknown>));
    },
    getSettings: () => Promise.resolve(settings),
    pauseDownload: (id) => (log.push(`pause:${String(id)}`), Promise.resolve()),
    resumeDownload: (id) => (log.push(`resume:${String(id)}`), Promise.resolve()),
    cancelDownload: (id) => (log.push(`cancel:${String(id)}`), Promise.resolve()),
    eraseDownload: (id) => (log.push(`erase:${String(id)}`), Promise.resolve()),
    startBrowserDownload: (url) => (log.push(`browser:${url}`), Promise.resolve()),
    openPromptWindow: () => Promise.resolve(nextWindow++),
    closeWindow: (id) => (log.push(`close:${String(id)}`), Promise.resolve()),
    setBadge: (text) => (log.push(`badge:${text}`), Promise.resolve()),
    sessionGet: (k) => Promise.resolve(session.get(k)),
    sessionSet: (k, v) => (session.set(k, v), Promise.resolve()),
    sessionRemove: (k) => (session.delete(k), Promise.resolve()),
    sessionAll: () => Promise.resolve(Object.fromEntries(session)),
  };
  return { api, log, sent, session };
}

const ok: Reply = (m) => ({ v: 1, id: m.id, ok: true, result: { action: 'prompted' } });
const notRunning: Reply = (m) => ({ v: 1, id: m.id, ok: false, error: { code: 'DESKTOP_NOT_RUNNING', message: 'Velocity V2 is not running.' } });
const item = (over: Record<string, unknown> = {}) => ({ id: 7, url: 'https://cdn.test/big.iso', referrer: 'https://site.test/', filename: '', mime: 'application/octet-stream', fileSize: 1 << 30, incognito: false, state: 'in_progress', ...over });

describe('background interception', () => {
  let f: ReturnType<typeof fakeApi>;
  beforeEach(() => {
    f = fakeApi(ok);
  });

  it('hands matching downloads to Velocity and removes the browser copy', async () => {
    const bg = createBackground(f.api);
    expect(await bg.onDownloadCreated(item())).toBe('handed-off');
    expect(f.log).toEqual(['pause:7', 'badge:', 'cancel:7', 'erase:7']);
    expect(f.sent[0]).toMatchObject({ v: 1, type: 'download.add', payload: { url: 'https://cdn.test/big.iso', referrer: 'https://site.test/', source: 'intercept', fileSize: 1 << 30 } });
  });

  it('lets the browser continue when the desktop app is unavailable', async () => {
    f = fakeApi(notRunning);
    const bg = createBackground(f.api);
    expect(await bg.onDownloadCreated(item())).toBe('fallback');
    expect(f.log).toEqual(['pause:7', 'badge:!', 'resume:7']);
  });

  it('falls back when the native host throws', async () => {
    f.api.sendNativeMessage = () => Promise.reject(new Error('Specified native messaging host not found.'));
    const bg = createBackground(f.api);
    expect(await bg.onDownloadCreated(item())).toBe('fallback');
    expect(f.log).toContain('resume:7');
  });

  it('ignores downloads that do not match the rules', async () => {
    const bg = createBackground(f.api);
    expect(await bg.onDownloadCreated(item({ url: 'https://cdn.test/pic.png', fileSize: 1000 }))).toBe('ignored');
    expect(await bg.onDownloadCreated(item({ url: 'blob:https://cdn.test/1' }))).toBe('ignored');
    expect(await bg.onDownloadCreated(item({ state: 'complete' }))).toBe('ignored');
    expect(f.log).toEqual([]);
    expect(f.sent).toEqual([]);
  });

  it('cancels an immediate duplicate of a link already handed off', async () => {
    const bg = createBackground(f.api);
    await bg.onDownloadCreated(item({ id: 1 }));
    expect(await bg.onDownloadCreated(item({ id: 2 }))).toBe('duplicate');
    expect(f.sent).toHaveLength(1);
    expect(f.log.slice(-3)).toEqual(['pause:2', 'cancel:2', 'erase:2']);
  });

  it('asks first in ask mode and resumes the browser download if the prompt is closed', async () => {
    f = fakeApi(ok, { ...DEFAULT_SETTINGS, mode: 'ask' });
    const bg = createBackground(f.api);
    expect(await bg.onDownloadCreated(item())).toBe('prompted');
    expect(f.sent).toHaveLength(0);
    const [key, pending] = [...f.session.entries()][0] ?? [];
    expect(key).toMatch(/^prompt:/);
    expect(pending?.windowId).toBe(100);
    await bg.onWindowRemoved(100);
    expect(f.log).toEqual(['pause:7', 'resume:7']);
    expect(f.session.size).toBe(0);
  });

  it('completes an ask-mode prompt with Velocity', async () => {
    f = fakeApi(ok, { ...DEFAULT_SETTINGS, mode: 'ask' });
    const bg = createBackground(f.api);
    await bg.onDownloadCreated(item());
    const id = ([...f.session.keys()][0] ?? '').slice('prompt:'.length);
    expect(await bg.onPageMessage({ type: 'prompt.get', id })).toMatchObject({ downloadId: 7 });
    expect(await bg.onPageMessage({ type: 'prompt.decide', id, choice: 'velocity' })).toMatchObject({ ok: true });
    expect(f.log).toEqual(['pause:7', 'badge:', 'cancel:7', 'erase:7', 'close:100']);
    expect(await bg.onPageMessage({ type: 'prompt.decide', id, choice: 'velocity' })).toMatchObject({ ok: false, error: { code: 'EXPIRED' } });
  });

  it('keeps the prompt open on failure so the user can choose the browser', async () => {
    f = fakeApi(notRunning, { ...DEFAULT_SETTINGS, mode: 'ask' });
    const bg = createBackground(f.api);
    await bg.onDownloadCreated(item());
    const id = ([...f.session.keys()][0] ?? '').slice('prompt:'.length);
    expect(await bg.onPageMessage({ type: 'prompt.decide', id, choice: 'velocity' })).toMatchObject({ ok: false });
    expect(f.session.size).toBe(1);
    await bg.onPageMessage({ type: 'prompt.decide', id, choice: 'browser' });
    expect(f.log.slice(-2)).toEqual(['resume:7', 'close:100']);
  });

  it('context menu sends the link and falls back to a browser download on failure', async () => {
    let bg = createBackground(f.api);
    expect(await bg.onContextMenu({ linkUrl: 'https://cdn.test/a.zip', pageUrl: 'https://site.test/p' })).toMatchObject({ ok: true });
    expect(f.sent[0]).toMatchObject({ payload: { url: 'https://cdn.test/a.zip', referrer: 'https://site.test/p', source: 'contextMenu' } });
    f = fakeApi(notRunning);
    bg = createBackground(f.api);
    await bg.onContextMenu({ linkUrl: 'https://cdn.test/a.zip' });
    expect(f.log).toContain('browser:https://cdn.test/a.zip');
  });

  it('refuses to send non-http links from the popup', async () => {
    const bg = createBackground(f.api);
    expect(await bg.onPageMessage({ type: 'add', url: 'file:///etc/shadow' })).toMatchObject({ ok: false });
    expect(f.sent).toHaveLength(0);
  });

  it('validates page messages', () => {
    expect(isPageMessage({ type: 'status' })).toBe(true);
    expect(isPageMessage({ type: 'add', url: 1 })).toBe(false);
    expect(isPageMessage({ type: 'prompt.decide', id: 'a', choice: 'delete' })).toBe(false);
    expect(isPageMessage({ type: 'exec' })).toBe(false);
    expect(isPageMessage(null)).toBe(false);
  });
});

describe('magnet links', () => {
  const magnet = 'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=album';

  it('sends magnet links from the context menu without a browser fallback', async () => {
    const f = fakeApi(notRunning);
    const bg = createBackground(f.api);
    const r = await bg.onContextMenu({ linkUrl: magnet, pageUrl: 'https://site.test/page' });
    expect(r.ok).toBe(false);
    expect(f.sent[0]).toMatchObject({ type: 'download.add', payload: { url: magnet, source: 'contextMenu', referrer: 'https://site.test/page' } });
    // Browsers cannot download magnet links themselves.
    expect(f.log.some((l) => l.startsWith('browser:'))).toBe(false);
  });

  it('ignores captured magnet clicks unless torrent capture is on', async () => {
    const off = fakeApi(ok);
    expect(await createBackground(off.api).onCapturedMagnet(magnet, 'https://site.test/')).toBe('ignored');
    expect(off.sent).toHaveLength(0);
    const on = fakeApi(ok, { ...DEFAULT_SETTINGS, captureTorrents: true });
    const bg = createBackground(on.api);
    expect(await bg.onCapturedMagnet(magnet, 'https://site.test/')).toBe('handed-off');
    expect(on.sent[0]).toMatchObject({ type: 'download.add', payload: { url: magnet, source: 'intercept', referrer: 'https://site.test/' } });
    expect(await bg.onCapturedMagnet(magnet, 'https://site.test/')).toBe('duplicate');
    expect(await bg.onCapturedMagnet('magnet:?dn=nohash', 'https://site.test/')).toBe('ignored');
    expect(await createBackground(fakeApi(ok, { ...DEFAULT_SETTINGS, captureTorrents: true, mode: 'off' }).api).onCapturedMagnet(magnet, 'https://site.test/')).toBe('ignored');
  });

  it('accepts only well-formed messages from the magnet content script', () => {
    expect(isContentMessage({ type: 'magnet', url: magnet })).toBe(true);
    expect(isContentMessage({ type: 'magnet', url: 'https://x.test/a.zip' })).toBe(false);
    expect(isContentMessage({ type: 'magnet', url: magnet + 'a'.repeat(9000) })).toBe(false);
    expect(isContentMessage({ type: 'add', url: magnet })).toBe(false);
    expect(isContentMessage(null)).toBe(false);
  });
});
