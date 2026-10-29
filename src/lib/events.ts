import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
  BrowserRequestSchema,
  DownloadsSnapshotSchema,
  DroppedTorrentsSchema,
  QuitRequestSchema,
  type BrowserRequest,
  type DownloadsSnapshot,
  type DroppedTorrents,
  type QuitRequest,
} from '../contract';

// The native side's events, each checked against the contract like a command's answer. An event
// that doesn't match is dropped: the next one, or `get_snapshot`, brings the state again.

export function onSnapshot(handler: (snapshot: DownloadsSnapshot) => void): Promise<UnlistenFn> {
  return listen('downloads-snapshot', (e) => {
    const parsed = DownloadsSnapshotSchema.safeParse(e.payload);
    if (parsed.success) handler(parsed.data);
  });
}

/** Quitting would stop active downloads: the window asks first (#29). */
export function onQuitRequested(handler: (request: QuitRequest) => void): Promise<UnlistenFn> {
  return listen('quit-requested', (e) => {
    const parsed = QuitRequestSchema.safeParse(e.payload);
    if (parsed.success) handler(parsed.data);
  });
}

/** The browser sent a link for the user to check: the window opens the Add dialog with it (#65). */
export function onBrowserRequest(handler: (request: BrowserRequest) => void): Promise<UnlistenFn> {
  return listen('browser-request', (e) => {
    const parsed = BrowserRequestSchema.safeParse(e.payload);
    if (parsed.success) handler(parsed.data);
  });
}

/** `.torrent` files were dropped on the window: the window opens the Add dialog with them (#60). */
export function onTorrentsDropped(handler: (dropped: DroppedTorrents) => void): Promise<UnlistenFn> {
  return listen('torrents-dropped', (e) => {
    const parsed = DroppedTorrentsSchema.safeParse(e.payload);
    if (parsed.success) handler(parsed.data);
  });
}
