import { useEffect } from 'react';
import { api } from '../lib/api';
import { toApiError } from '../lib/errors';
import { onSnapshot } from '../lib/events';
import { useDownloadsStore } from '../stores/downloads';

/** Keeps the downloads store in step with the native side: the snapshot now, then every one it sends. */
export function useSnapshotFeed(enabled: boolean) {
  const setSnapshot = useDownloadsStore((s) => s.setSnapshot);
  const setError = useDownloadsStore((s) => s.setError);
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onSnapshot((s) => {
      setSnapshot(s);
    }).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    api
      .snapshot()
      .then((s) => {
        if (!disposed) setSnapshot(s);
      })
      .catch((e: unknown) => {
        if (!disposed) setError(toApiError(e).message);
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [enabled, setSnapshot, setError]);
}
