/**
 * Background logic, written against a narrow interface of the WebExtension APIs so it
 * can run in Chrome (MV3 service worker), Edge and Firefox (MV3 background script) and be
 * unit tested with a fake.
 */
import { decide, RecentUrls, type DownloadCandidate } from './shared/rules';
import { browserCanDownload, buildAdd, canSend, isMagnet, newId, PROTOCOL_VERSION, type AddPayload, type Response } from './shared/protocol';
import { sendNative, type NativeApi } from './shared/native';
import { DEFAULT_SETTINGS, sanitizeSettings, type ExtensionSettings } from './shared/settings';
import { SHORT_NAME } from './shared/branding';

export interface DownloadItemLike {
  id: number;
  url: string;
  finalUrl?: string;
  referrer?: string;
  filename?: string;
  mime?: string;
  fileSize?: number;
  totalBytes?: number;
  incognito?: boolean;
  byExtensionId?: string;
  state?: string;
}

export interface PendingPrompt {
  downloadId: number;
  windowId?: number;
  payload: AddPayload;
}

export interface ExtApi extends NativeApi {
  selfId: string;
  getSettings(): Promise<unknown>;
  pauseDownload(id: number): Promise<void>;
  resumeDownload(id: number): Promise<void>;
  cancelDownload(id: number): Promise<void>;
  eraseDownload(id: number): Promise<void>;
  startBrowserDownload(url: string): Promise<void>;
  openPromptWindow(promptId: string): Promise<number | undefined>;
  closeWindow(windowId: number): Promise<void>;
  setBadge(text: string, title: string): Promise<void>;
  sessionGet(key: string): Promise<PendingPrompt | undefined>;
  sessionSet(key: string, value: PendingPrompt): Promise<void>;
  sessionRemove(key: string): Promise<void>;
  sessionAll(): Promise<Record<string, PendingPrompt>>;
}

/** Messages from the extension's own pages (popup, options, prompt). */
export type PageMessage =
  | { type: 'status' }
  | { type: 'add'; url: string }
  | { type: 'prompt.get'; id: string }
  | { type: 'prompt.decide'; id: string; choice: 'velocity' | 'browser' };

export function isPageMessage(m: unknown): m is PageMessage {
  if (typeof m !== 'object' || m === null) return false;
  const r = m as Record<string, unknown>;
  switch (r.type) {
    case 'status':
      return true;
    case 'add':
      return typeof r.url === 'string' && r.url.length <= 8192;
    case 'prompt.get':
      return typeof r.id === 'string';
    case 'prompt.decide':
      return typeof r.id === 'string' && (r.choice === 'velocity' || r.choice === 'browser');
    default:
      return false;
  }
}

/** Messages from the optional magnet-link content script. */
export interface ContentMessage {
  type: 'magnet';
  url: string;
}

export function isContentMessage(m: unknown): m is ContentMessage {
  if (typeof m !== 'object' || m === null) return false;
  const r = m as Record<string, unknown>;
  return r.type === 'magnet' && typeof r.url === 'string' && r.url.length <= 8192 && isMagnet(r.url);
}

const PROMPT_PREFIX = 'prompt:';

function candidateOf(item: DownloadItemLike): DownloadCandidate {
  const size = item.fileSize !== undefined && item.fileSize > 0 ? item.fileSize : item.totalBytes !== undefined && item.totalBytes > 0 ? item.totalBytes : undefined;
  return { url: item.url, finalUrl: item.finalUrl, filename: item.filename, mime: item.mime, fileSize: size, incognito: item.incognito, byExtensionId: item.byExtensionId };
}

function payloadOf(item: DownloadItemLike, source: AddPayload['source']): AddPayload {
  const c = candidateOf(item);
  const p: AddPayload = { url: item.finalUrl || item.url, source };
  if (item.referrer) p.referrer = item.referrer;
  if (item.filename) p.filename = item.filename;
  if (c.fileSize !== undefined) p.fileSize = c.fileSize;
  if (item.mime) p.mime = item.mime;
  return p;
}

export function createBackground(api: ExtApi) {
  const recent = new RecentUrls();

  async function settings(): Promise<ExtensionSettings> {
    try {
      return sanitizeSettings(await api.getSettings());
    } catch {
      return DEFAULT_SETTINGS;
    }
  }

  async function report(r: Response) {
    if (r.ok) await api.setBadge('', '');
    else await api.setBadge('!', `${SHORT_NAME}: ${r.error.message}`);
  }

  async function handOff(payload: AddPayload): Promise<Response> {
    if (!canSend(payload.url)) return { v: PROTOCOL_VERSION, id: '', ok: false, error: { code: 'UNSUPPORTED', message: `Only http, https and magnet links can be sent to ${SHORT_NAME}.` } };
    const r = await sendNative(api, buildAdd(payload));
    await report(r);
    return r;
  }

  /** Hand the download to Velocity; on any failure let the browser continue it. */
  async function transfer(downloadId: number, payload: AddPayload): Promise<Response> {
    const r = await handOff(payload);
    if (r.ok) {
      await api.cancelDownload(downloadId).catch(() => undefined);
      await api.eraseDownload(downloadId).catch(() => undefined);
    } else {
      await api.resumeDownload(downloadId).catch(() => undefined);
    }
    return r;
  }

  return {
    async onDownloadCreated(item: DownloadItemLike): Promise<'ignored' | 'handed-off' | 'fallback' | 'prompted' | 'duplicate'> {
      if (item.state !== undefined && item.state !== 'in_progress') return 'ignored';
      const s = await settings();
      const d = decide(candidateOf(item), s, api.selfId);
      if (!d.intercept) return 'ignored';
      const url = item.finalUrl || item.url;
      // Pause first so the browser does not keep downloading while Velocity takes over.
      await api.pauseDownload(item.id).catch(() => undefined);
      if (recent.check(url)) {
        // The same link was handed to Velocity moments ago (double click or redirect echo).
        await api.cancelDownload(item.id).catch(() => undefined);
        await api.eraseDownload(item.id).catch(() => undefined);
        return 'duplicate';
      }
      const payload = payloadOf(item, 'intercept');
      if (s.mode === 'ask') {
        const promptId = newId();
        const pending: PendingPrompt = { downloadId: item.id, payload };
        await api.sessionSet(PROMPT_PREFIX + promptId, pending);
        const windowId = await api.openPromptWindow(promptId).catch(() => undefined);
        if (windowId === undefined) {
          await api.sessionRemove(PROMPT_PREFIX + promptId);
          await api.resumeDownload(item.id).catch(() => undefined);
          return 'fallback';
        }
        await api.sessionSet(PROMPT_PREFIX + promptId, { ...pending, windowId });
        return 'prompted';
      }
      const r = await transfer(item.id, payload);
      return r.ok ? 'handed-off' : 'fallback';
    },

    /** A prompt window closed without a decision: let the browser download continue. */
    async onWindowRemoved(windowId: number) {
      const all = await api.sessionAll();
      for (const [key, p] of Object.entries(all)) {
        if (key.startsWith(PROMPT_PREFIX) && p.windowId === windowId) {
          await api.sessionRemove(key);
          await api.resumeDownload(p.downloadId).catch(() => undefined);
        }
      }
    },

    async onContextMenu(info: { linkUrl?: string; srcUrl?: string; pageUrl?: string }): Promise<Response> {
      const url = info.linkUrl ?? info.srcUrl ?? '';
      const payload: AddPayload = { url, source: 'contextMenu' };
      if (info.pageUrl) payload.referrer = info.pageUrl;
      const r = await handOff(payload);
      // Fall back to the browser's own download so the click is not lost.
      if (!r.ok && browserCanDownload(url) && r.error.code !== 'DISABLED') await api.startBrowserDownload(url).catch(() => undefined);
      return r;
    },

    /**
     * A magnet link click caught by the content script (only registered when torrent
     * capture is on). Velocity always asks before adding it.
     */
    async onCapturedMagnet(url: string, pageUrl: string | undefined): Promise<'ignored' | 'handed-off' | 'failed' | 'duplicate'> {
      const s = await settings();
      if (!s.captureTorrents || s.mode === 'off' || !isMagnet(url) || !canSend(url)) return 'ignored';
      if (recent.check(url)) return 'duplicate';
      const payload: AddPayload = { url, source: 'intercept' };
      if (pageUrl) payload.referrer = pageUrl;
      const r = await handOff(payload);
      return r.ok ? 'handed-off' : 'failed';
    },

    async onPageMessage(msg: PageMessage): Promise<unknown> {
      switch (msg.type) {
        case 'status': {
          const ping = await sendNative(api, { v: 1, id: newId(), type: 'ping' });
          if (!ping.ok) return ping;
          return sendNative(api, { v: 1, id: newId(), type: 'app.status' });
        }
        case 'add':
          return handOff({ url: msg.url.trim(), source: 'popup' });
        case 'prompt.get':
          return (await api.sessionGet(PROMPT_PREFIX + msg.id)) ?? null;
        case 'prompt.decide': {
          const key = PROMPT_PREFIX + msg.id;
          const p = await api.sessionGet(key);
          if (!p) return { ok: false, error: { code: 'EXPIRED', message: 'This download request is no longer pending.' } };
          if (msg.choice === 'browser') {
            await api.sessionRemove(key);
            await api.resumeDownload(p.downloadId).catch(() => undefined);
            if (p.windowId !== undefined) await api.closeWindow(p.windowId).catch(() => undefined);
            return { ok: true };
          }
          const r = await handOff({ ...p.payload });
          if (r.ok) {
            await api.sessionRemove(key);
            await api.cancelDownload(p.downloadId).catch(() => undefined);
            await api.eraseDownload(p.downloadId).catch(() => undefined);
            if (p.windowId !== undefined) await api.closeWindow(p.windowId).catch(() => undefined);
          }
          // On failure the prompt stays open and shows the error with a "Download in browser" choice.
          return r;
        }
      }
    },
  };
}
