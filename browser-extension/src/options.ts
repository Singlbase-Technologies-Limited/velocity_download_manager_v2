import { ext } from './shared/ext';
import { DEFAULT_SETTINGS, sanitizeSettings, type ExtensionSettings } from './shared/settings';
import { $ } from './shared/dom';

const lines = (s: string) => s.split(/[\s,]+/).map((x) => x.trim()).filter(Boolean);

function fill(s: ExtensionSettings) {
  $('mode', HTMLSelectElement).value = s.mode;
  $('fileTypes', HTMLTextAreaElement).value = s.fileTypes.join(' ');
  $('minSizeMib', HTMLInputElement).value = String(s.minSizeMib);
  $('allowDomains', HTMLTextAreaElement).value = s.allowDomains.join('\n');
  $('denyDomains', HTMLTextAreaElement).value = s.denyDomains.join('\n');
  $('includePrivate', HTMLInputElement).checked = s.includePrivate;
  $('contextMenu', HTMLInputElement).checked = s.contextMenu;
  $('captureTorrents', HTMLInputElement).checked = s.captureTorrents;
}

async function init() {
  fill(sanitizeSettings((await ext.storage.local.get('settings')).settings));
  $('form', HTMLElement).addEventListener('submit', (e) => {
    e.preventDefault();
    const s = sanitizeSettings({
      mode: $('mode', HTMLSelectElement).value,
      fileTypes: lines($('fileTypes', HTMLTextAreaElement).value),
      minSizeMib: Number($('minSizeMib', HTMLInputElement).value),
      allowDomains: lines($('allowDomains', HTMLTextAreaElement).value),
      denyDomains: lines($('denyDomains', HTMLTextAreaElement).value),
      includePrivate: $('includePrivate', HTMLInputElement).checked,
      contextMenu: $('contextMenu', HTMLInputElement).checked,
      captureTorrents: $('captureTorrents', HTMLInputElement).checked,
    });
    // Catching magnet link clicks needs access to pages; ask only when the user turns it on.
    const access = s.captureTorrents ? ext.permissions.request({ origins: ['<all_urls>'] }).catch(() => false) : Promise.resolve(true);
    void access.then((granted) => {
      $('torrentAccess', HTMLElement).textContent = granted ? '' : 'Without access to pages only .torrent downloads are sent; magnet link clicks stay with the browser.';
      return ext.storage.local.set({ settings: s });
    }).then(() => {
      fill(s);
      $('saved', HTMLElement).textContent = 'Saved.';
      setTimeout(() => ($('saved', HTMLElement).textContent = ''), 2000);
    });
  });
  $('reset', HTMLElement).addEventListener('click', () => {
    fill(DEFAULT_SETTINGS);
    void ext.storage.local.set({ settings: DEFAULT_SETTINGS });
  });
}

void init();
