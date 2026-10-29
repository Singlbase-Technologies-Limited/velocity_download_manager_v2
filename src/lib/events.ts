import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DownloadsSnapshotSchema, QuitRequestSchema, type DownloadsSnapshot, type QuitRequest } from '../contract';

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
