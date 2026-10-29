import { createBackground, isContentMessage, isPageMessage, type ExtApi, type PendingPrompt } from './background-core';
import { ext } from './shared/ext';
import { sanitizeSettings } from './shared/settings';
import { EXTENSION_NAME, SHORT_NAME } from './shared/branding';

const MENU_ID = 'velocity-v2-download-link';

// storage.session keeps pending prompts across service worker restarts; fall back to memory.
const memory = new Map<string, PendingPrompt>();
const session = ext.storage.session as chrome.storage.StorageArea | undefined;

const api: ExtApi = {
  selfId: ext.runtime.id,
  sendNativeMessage: (app, msg) => ext.runtime.sendNativeMessage(app, msg),
  getSettings: async () => (await ext.storage.local.get('settings')).settings,
  pauseDownload: (id) => ext.downloads.pause(id),
  resumeDownload: (id) => ext.downloads.resume(id),
  cancelDownload: (id) => ext.downloads.cancel(id),
  eraseDownload: async (id) => {
    await ext.downloads.erase({ id });
  },
  startBrowserDownload: async (url) => {
    await ext.downloads.download({ url });
  },
  openPromptWindow: async (promptId) => {
    const w = await ext.windows.create({ url: ext.runtime.getURL(`prompt.html?id=${encodeURIComponent(promptId)}`), type: 'popup', width: 460, height: 340, focused: true });
    return w?.id;
  },
  closeWindow: (id) => ext.windows.remove(id),
  setBadge: async (text, title) => {
    await ext.action.setBadgeText({ text });
    await ext.action.setBadgeBackgroundColor({ color: '#dc2626' });
    await ext.action.setTitle({ title: title || EXTENSION_NAME });
  },
  sessionGet: async (key) => (session ? ((await session.get(key))[key] as PendingPrompt | undefined) : memory.get(key)),
  sessionSet: async (key, value) => {
    if (session) await session.set({ [key]: value });
    else memory.set(key, value);
  },
  sessionRemove: async (key) => {
    if (session) await session.remove(key);
    else memory.delete(key);
  },
  sessionAll: async () => (session ? await session.get<Record<string, PendingPrompt>>(null) : Object.fromEntries(memory)),
};

const bg = createBackground(api);

async function syncContextMenu() {
  const s = sanitizeSettings((await ext.storage.local.get('settings')).settings);
  await ext.contextMenus.removeAll();
  if (s.contextMenu) {
    ext.contextMenus.create({ id: MENU_ID, title: `Download with ${SHORT_NAME}`, contexts: ['link', 'image', 'video', 'audio'] });
  }
}

const MAGNET_SCRIPT = 'velocity-v2-magnet-links';
const PAGES = { origins: ['<all_urls>'] };

/** Register the magnet click script only while capture is on and page access was granted. */
async function syncMagnetCapture() {
  const s = sanitizeSettings((await ext.storage.local.get('settings')).settings);
  const granted = await ext.permissions.contains(PAGES).catch(() => false);
  const registered = await ext.scripting.getRegisteredContentScripts({ ids: [MAGNET_SCRIPT] }).catch(() => []);
  if (s.captureTorrents && s.mode !== 'off' && granted) {
    if (registered.length === 0) {
      await ext.scripting
        .registerContentScripts([{ id: MAGNET_SCRIPT, js: ['magnet.js'], matches: ['<all_urls>'], runAt: 'document_start', allFrames: false, persistAcrossSessions: true }])
        .catch(() => undefined);
    }
  } else if (registered.length > 0) {
    await ext.scripting.unregisterContentScripts({ ids: [MAGNET_SCRIPT] }).catch(() => undefined);
  }
}

const syncAll = () => {
  void syncContextMenu();
  void syncMagnetCapture();
};
ext.runtime.onInstalled.addListener(syncAll);
ext.runtime.onStartup.addListener(syncAll);
ext.storage.onChanged.addListener((changes, area) => {
  if (area === 'local' && 'settings' in changes) syncAll();
});
ext.permissions.onRemoved.addListener(() => void syncMagnetCapture());
ext.permissions.onAdded.addListener(() => void syncMagnetCapture());

ext.downloads.onCreated.addListener((item) => {
  void bg.onDownloadCreated({
    id: item.id,
    url: item.url,
    finalUrl: item.finalUrl,
    referrer: item.referrer,
    filename: item.filename,
    mime: item.mime,
    fileSize: item.fileSize,
    totalBytes: item.totalBytes,
    incognito: item.incognito,
    byExtensionId: item.byExtensionId,
    state: item.state,
  });
});

ext.windows.onRemoved.addListener((id) => void bg.onWindowRemoved(id));

ext.contextMenus.onClicked.addListener((info) => {
  if (info.menuItemId === MENU_ID) {
    const i: { linkUrl?: string; srcUrl?: string; pageUrl?: string } = {};
    if (info.linkUrl) i.linkUrl = info.linkUrl;
    if (info.srcUrl) i.srcUrl = info.srcUrl;
    if (info.pageUrl) i.pageUrl = info.pageUrl;
    void bg.onContextMenu(i);
  }
});

ext.runtime.onMessage.addListener((msg: unknown, sender, sendResponse) => {
  // The magnet content script may only report a magnet link from a tab.
  if (sender.id === ext.runtime.id && sender.tab && isContentMessage(msg)) {
    void bg.onCapturedMagnet(msg.url, sender.tab.url);
    return false;
  }
  // Otherwise only the extension's own pages may talk to the background.
  if (sender.id !== ext.runtime.id || !sender.url?.startsWith(ext.runtime.getURL('')) || !isPageMessage(msg)) return false;
  bg.onPageMessage(msg).then(sendResponse, (e: unknown) => {
    sendResponse({ ok: false, error: { code: 'INTERNAL', message: String(e) } });
  });
  return true;
});
