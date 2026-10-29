import { useEffect } from 'react';
import { toast } from 'sonner';
import { onTorrentsDropped } from '../../lib/events';
import { useUiStore } from '../../stores/ui';

/**
 * Where the Add dialog opens from besides the navigation (#60): Ctrl+N (⌘N on macOS), as in V1, and
 * `.torrent` files dropped on the window. The browser's prefill (#64, #65) calls `openAdd` too.
 */
export function useAddEntryPoints() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault();
        // An open dialog keeps what the user typed.
        if (!useUiStore.getState().addDialog.open) useUiStore.getState().openAdd();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => { window.removeEventListener('keydown', onKey); };
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
