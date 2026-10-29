import { create } from 'zustand';
import type { DownloadsSnapshot } from '../contract';

// The latest snapshot of the downloads, from `get_snapshot` and the `downloads-snapshot` event.
interface DownloadsState {
  snapshot: DownloadsSnapshot | null;
  /** Why the snapshot couldn't be read, if it couldn't. */
  error: string | null;
  setSnapshot: (snapshot: DownloadsSnapshot) => void;
  setError: (error: string) => void;
}

export const useDownloadsStore = create<DownloadsState>((set) => ({
  snapshot: null,
  error: null,
  setSnapshot: (snapshot) => { set({ snapshot, error: null }); },
  setError: (error) => { set({ error }); },
}));
