/** Extension settings stored in `storage.local`. Validated on every read. */

export type InterceptMode = 'auto' | 'ask' | 'off';

export interface ExtensionSettings {
  /** auto: hand matching downloads to Velocity; ask: ask in the browser first; off: never intercept. */
  mode: InterceptMode;
  /** File extensions (without dot) that are always handed off. */
  fileTypes: string[];
  /** Also hand off any download whose reported size is at least this many MiB (0 disables). */
  minSizeMib: number;
  /** When non-empty, only downloads from these domains (and subdomains) are handed off. */
  allowDomains: string[];
  /** Downloads from these domains (and subdomains) are never handed off. */
  denyDomains: string[];
  /** Intercept downloads started in private/incognito windows. Off by default. */
  includePrivate: boolean;
  /** Show the "Download with Velocity V2" context menu item. */
  contextMenu: boolean;
  /**
   * Send magnet link clicks and `.torrent` downloads to Velocity. Off by default;
   * magnet capture also needs the optional permission to read pages.
   */
  captureTorrents: boolean;
}

export const DEFAULT_SETTINGS: ExtensionSettings = {
  mode: 'auto',
  fileTypes: ['zip', '7z', 'rar', 'tar', 'gz', 'xz', 'bz2', 'iso', 'img', 'dmg', 'exe', 'msi', 'msix', 'deb', 'rpm', 'appimage', 'apk', 'mp4', 'mkv', 'avi', 'mov', 'webm', 'mp3', 'flac', 'wav', 'm4a', 'pdf', 'epub'],
  minSizeMib: 20,
  allowDomains: [],
  denyDomains: [],
  includePrivate: false,
  contextMenu: true,
  captureTorrents: false,
};

const EXT_RE = /^[a-z0-9]{1,16}$/;
const DOMAIN_RE = /^(?=.{1,253}$)([a-z0-9-]{1,63}\.)*[a-z0-9-]{1,63}$/;

export function normalizeDomain(d: string): string | null {
  const v = d.trim().toLowerCase().replace(/^\*\./, '').replace(/^\.+|\.+$/g, '');
  return DOMAIN_RE.test(v) ? v : null;
}

/** Coerce unknown storage contents into valid settings, falling back to defaults per field. */
export function sanitizeSettings(raw: unknown): ExtensionSettings {
  const r = (typeof raw === 'object' && raw !== null ? raw : {}) as Record<string, unknown>;
  const list = (v: unknown, norm: (s: string) => string | null, max: number): string[] | null =>
    Array.isArray(v) ? [...new Set(v.filter((x): x is string => typeof x === 'string').map(norm).filter((x): x is string => x !== null))].slice(0, max) : null;
  const mode = r.mode === 'auto' || r.mode === 'ask' || r.mode === 'off' ? r.mode : DEFAULT_SETTINGS.mode;
  const minSize = typeof r.minSizeMib === 'number' && Number.isFinite(r.minSizeMib) ? Math.min(100_000, Math.max(0, Math.round(r.minSizeMib))) : DEFAULT_SETTINGS.minSizeMib;
  return {
    mode,
    fileTypes: list(r.fileTypes, (s) => {
      const v = s.trim().toLowerCase().replace(/^\.+/, '');
      return EXT_RE.test(v) ? v : null;
    }, 200) ?? DEFAULT_SETTINGS.fileTypes,
    minSizeMib: minSize,
    allowDomains: list(r.allowDomains, normalizeDomain, 500) ?? [],
    denyDomains: list(r.denyDomains, normalizeDomain, 500) ?? [],
    includePrivate: typeof r.includePrivate === 'boolean' ? r.includePrivate : DEFAULT_SETTINGS.includePrivate,
    contextMenu: typeof r.contextMenu === 'boolean' ? r.contextMenu : DEFAULT_SETTINGS.contextMenu,
    captureTorrents: typeof r.captureTorrents === 'boolean' ? r.captureTorrents : DEFAULT_SETTINGS.captureTorrents,
  };
}
