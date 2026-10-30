import { useEffect } from 'react';
import { toast } from 'sonner';
import { onBrowserRequest, onTorrentsDropped } from '../../lib/events';
import { torrentLinksInPaste } from '../torrents/torrentLogic';
import { useUiStore } from '../../stores/ui';

/**
 * Where the Add dialog opens from besides the navigation (#60): Ctrl+N (⌘N on macOS), as in V1, `.torrent` files
 * dropped on the window, magnet or `.torrent` links pasted outside a text field (#63), and links the browser sends for
 * the user to check (#65).
 */
export function useAddEntryPoints() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Ctrl+, (⌘, on macOS) opens Settings, as in V1 (#30).
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key === ',') {
        e.preventDefault();
        useUiStore.getState().setView('settings');
        return;
      }
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        // An open dialog keeps what the user typed.
        if (!useUiStore.getState().addDialog.open) useUiStore.getState().openAdd();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => { window.removeEventListener('keydown', onKey); };
  }, []);

  // Magnet and `.torrent` links pasted outside a text field open the dialog with them (V1's, #63).
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target?.closest('input, textarea, [contenteditable="true"]') || useUiStore.getState().addDialog.open) return;
      const links = torrentLinksInPaste(e.clipboardData?.getData('text/plain') ?? '');
      if (links.length === 0) return;
      e.preventDefault();
      useUiStore.getState().openAdd({ links: links.join('\n') });
    };
    document.addEventListener('paste', onPaste);
    return () => { document.removeEventListener('paste', onPaste); };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onBrowserRequest((r) => {
      useUiStore.getState().openAdd({
        links: r.url,
        ...(r.fileName ? { fileName: r.fileName } : {}),
        ...(r.referrer ? { referrer: r.referrer } : {}),
        fromBrowser: { referrer: r.referrer, fileSize: r.fileSize, mime: r.mime },
      });
    }).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onTorrentsDropped(({ paths, ignored }) => {
      if (paths.length > 0) useUiStore.getState().openAdd({ torrentFiles: paths });
      else if (ignored > 0) toast.message('Only .torrent files can be dropped on the window.');
    }).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}
