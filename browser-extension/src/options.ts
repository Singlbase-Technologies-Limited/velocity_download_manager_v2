import { ext } from './shared/ext';
import { DEFAULT_SETTINGS, sanitizeSettings, type ExtensionSettings } from './shared/settings';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const lines = (s: string) => s.split(/[\s,]+/).map((x) => x.trim()).filter(Boolean);

function fill(s: ExtensionSettings) {
  $<HTMLSelectElement>('mode').value = s.mode;
  $<HTMLTextAreaElement>('fileTypes').value = s.fileTypes.join(' ');
  $<HTMLInputElement>('minSizeMib').value = String(s.minSizeMib);
  $<HTMLTextAreaElement>('allowDomains').value = s.allowDomains.join('\n');
  $<HTMLTextAreaElement>('denyDomains').value = s.denyDomains.join('\n');
  $<HTMLInputElement>('includePrivate').checked = s.includePrivate;
  $<HTMLInputElement>('contextMenu').checked = s.contextMenu;
  $<HTMLInputElement>('captureTorrents').checked = s.captureTorrents;
}

async function init() {
  fill(sanitizeSettings((await ext.storage.local.get('settings')).settings));
  $('form').addEventListener('submit', (e) => {
    e.preventDefault();
    const s = sanitizeSettings({
      mode: $<HTMLSelectElement>('mode').value,
      fileTypes: lines($<HTMLTextAreaElement>('fileTypes').value),
      minSizeMib: Number($<HTMLInputElement>('minSizeMib').value),
      allowDomains: lines($<HTMLTextAreaElement>('allowDomains').value),
      denyDomains: lines($<HTMLTextAreaElement>('denyDomains').value),
      includePrivate: $<HTMLInputElement>('includePrivate').checked,
      contextMenu: $<HTMLInputElement>('contextMenu').checked,
      captureTorrents: $<HTMLInputElement>('captureTorrents').checked,
    });
    // Catching magnet link clicks needs access to pages; ask only when the user turns it on.
    const access = s.captureTorrents ? ext.permissions.request({ origins: ['<all_urls>'] }).catch(() => false) : Promise.resolve(true);
    void access.then((granted) => {
      $('torrentAccess').textContent = granted ? '' : 'Without access to pages only .torrent downloads are sent; magnet link clicks stay with the browser.';
      return ext.storage.local.set({ settings: s });
    }).then(() => {
      fill(s);
      $('saved').textContent = 'Saved.';
      setTimeout(() => ($('saved').textContent = ''), 2000);
    });
  });
  $('reset').addEventListener('click', () => {
    fill(DEFAULT_SETTINGS);
    void ext.storage.local.set({ settings: DEFAULT_SETTINGS });
  });
}

void init();
