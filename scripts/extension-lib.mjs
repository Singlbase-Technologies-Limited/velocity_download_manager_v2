// The browser extension's manifest and identity (#64), kept apart from the build so that tests can
// check them without building. See docs/BROWSER_EXTENSION.md.
import { createHash } from 'node:crypto';

/** The browsers the extension is built for: Chromium (Chrome, Edge, Brave) and Firefox. */
export const TARGETS = ['chrome', 'firefox'];

/** The extension's scripts, one bundle each. */
export const ENTRIES = ['background', 'popup', 'options', 'prompt', 'magnet'];

/** V2's icons, from the desktop app's own (`src-tauri/icons`): published name -> source name. */
export const ICONS = { 32: '32x32.png', 64: '64x64.png', 128: '128x128.png' };

/**
 * The permissions the extension asks for, the same as V1's. Access to pages is optional and asked
 * for only when the user turns on magnet link capture.
 */
export const PERMISSIONS = ['downloads', 'nativeMessaging', 'storage', 'contextMenus', 'scripting'];
export const OPTIONAL_HOST_PERMISSIONS = ['<all_urls>'];

/** The lowest browser versions: MV3 with `storage.session` and promise-based APIs. */
export const MINIMUM_CHROME_VERSION = '116';
export const MINIMUM_FIREFOX_VERSION = '115.0';

/**
 * The Chrome extension ID a manifest `key` pins: the first 128 bits of the SHA-256 of the public
 * key (DER), written with the letters a to p for the hex digits 0 to f.
 * @param {string} keyBase64
 */
export function chromeExtensionId(keyBase64) {
  const digest = createHash('sha256').update(Buffer.from(keyBase64, 'base64')).digest('hex');
  return [...digest.slice(0, 32)].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join('');
}

/**
 * The manifest for one browser.
 * @param {'chrome' | 'firefox'} target
 * @param {{ release: boolean, version: string, branding: Record<string, unknown> }} options
 */
export function manifest(target, { release, version, branding }) {
  const name = String(release ? branding.releaseExtensionName : branding.extensionName);
  const icons = Object.fromEntries(Object.keys(ICONS).map((size) => [size, `icons/icon-${size}.png`]));
  const m = {
    manifest_version: 3,
    name,
    short_name: String(branding.extensionShortName),
    version,
    description: `Send downloads from your browser to ${name}.`,
    icons,
    permissions: [...PERMISSIONS],
    optional_host_permissions: [...OPTIONAL_HOST_PERMISSIONS],
    action: { default_popup: 'popup.html', default_title: name, default_icon: icons },
    options_ui: { page: 'options.html', open_in_tab: true },
    content_security_policy: { extension_pages: "script-src 'self'; object-src 'none'; base-uri 'none'" },
  };
  if (target === 'chrome') {
    return {
      ...m,
      background: { service_worker: 'background.js' },
      // Pins the extension ID that the native host's manifest allows (allowed_origins).
      key: String(branding.chromeExtensionKey),
      minimum_chrome_version: MINIMUM_CHROME_VERSION,
    };
  }
  return {
    ...m,
    background: { scripts: ['background.js'] },
    browser_specific_settings: { gecko: { id: String(branding.firefoxExtensionId), strict_min_version: MINIMUM_FIREFOX_VERSION } },
  };
}
