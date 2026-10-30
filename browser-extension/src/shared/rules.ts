/** Pure decision logic for which browser downloads are handed to Velocity. */
import type { ExtensionSettings } from './settings';

export interface DownloadCandidate {
  url: string;
  finalUrl?: string | undefined;
  filename?: string | undefined;
  mime?: string | undefined;
  fileSize?: number | undefined;
  incognito?: boolean | undefined;
  byExtensionId?: string | undefined;
}

export type Decision = { intercept: true } | { intercept: false; reason: string };

export function hostMatches(host: string, domains: readonly string[]): boolean {
  const h = host.toLowerCase();
  return domains.some((d) => h === d || h.endsWith(`.${d}`));
}

export function extensionOf(name: string): string {
  const base = name.split(/[\\/]/).pop() ?? '';
  const i = base.lastIndexOf('.');
  return i > 0 ? base.slice(i + 1).toLowerCase() : '';
}

/** File name the browser will use, or the last URL path segment. */
export function candidateName(c: DownloadCandidate): string {
  if (c.filename) return c.filename.split(/[\\/]/).pop() ?? c.filename;
  try {
    const seg = new URL(c.finalUrl ?? c.url).pathname.split('/').filter(Boolean).pop() ?? '';
    return decodeURIComponent(seg);
  } catch {
    return '';
  }
}

export function decide(c: DownloadCandidate, s: ExtensionSettings, selfId: string): Decision {
  if (s.mode === 'off') return { intercept: false, reason: 'interception is off' };
  if (c.byExtensionId && c.byExtensionId === selfId) return { intercept: false, reason: 'started by this extension (fallback)' };
  if (c.incognito && !s.includePrivate) return { intercept: false, reason: 'private window' };
  const raw = c.finalUrl || c.url;
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return { intercept: false, reason: 'invalid URL' };
  }
  // blob:, data:, file: and similar exist only inside the browser and cannot be handed off.
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return { intercept: false, reason: `unsupported scheme ${url.protocol}` };
  if (hostMatches(url.hostname, s.denyDomains)) return { intercept: false, reason: 'domain is excluded' };
  if (s.allowDomains.length > 0 && !hostMatches(url.hostname, s.allowDomains)) return { intercept: false, reason: 'domain is not in the allow list' };
  const ext = extensionOf(candidateName(c));
  if (s.captureTorrents && (ext === 'torrent' || c.mime?.toLowerCase() === 'application/x-bittorrent')) return { intercept: true };
  if (ext && s.fileTypes.includes(ext)) return { intercept: true };
  if (s.minSizeMib > 0 && typeof c.fileSize === 'number' && c.fileSize >= s.minSizeMib * 1024 * 1024) return { intercept: true };
  return { intercept: false, reason: 'file type and size do not match the rules' };
}

/** Remembers recently handled URLs so one click is not handed off twice. */
export class RecentUrls {
  private readonly seen = new Map<string, number>();
  constructor(private readonly ttlMs = 5000) {}
  /** Returns true if the URL was already seen within the window; records it otherwise. */
  check(url: string, now = Date.now()): boolean {
    for (const [u, t] of this.seen) if (now - t > this.ttlMs) this.seen.delete(u);
    if (this.seen.has(url)) return true;
    this.seen.set(url, now);
    return false;
  }
}
