import { create } from 'zustand';
import type { DownloadsSnapshot } from '../contract';
import type { SortKey, SortState } from '../downloads/list';

/** How the list starts: nothing selected, no search, newest first (V1's). */
export const INITIAL_LIST = {
  selection: new Set<string>() as ReadonlySet<string>,
  anchor: null as string | null,
  focus: null as string | null,
  search: '',
  sort: { key: 'added', dir: 'desc' } as SortState,
};

// The latest snapshot of the downloads, from `get_snapshot` and the `downloads-snapshot` event, and the list's own
// presentation state (V1's downloads store).
interface DownloadsState {
  snapshot: DownloadsSnapshot | null;
  /** Why the snapshot couldn't be read, if it couldn't. */
  error: string | null;
  selection: ReadonlySet<string>;
  /** Where a Shift range starts: the last download clicked. */
  anchor: string | null;
  /** The row the arrow keys move from. */
  focus: string | null;
  search: string;
  sort: SortState;
  setSnapshot: (snapshot: DownloadsSnapshot) => void;
  setError: (error: string) => void;
  setSearch: (search: string) => void;
  toggleSort: (key: SortKey) => void;
  /** A plain click, a Ctrl or Cmd click (`toggle`), or a Shift click (`range`, over the ids in the order shown). */
  select: (id: string, mode: 'single' | 'toggle' | 'range', orderedIds?: readonly string[]) => void;
  selectAll: (ids: readonly string[]) => void;
  clearSelection: () => void;
}

export const useDownloadsStore = create<DownloadsState>((set, get) => ({
  snapshot: null,
  error: null,
  ...INITIAL_LIST,
  setSnapshot: (snapshot) => {
    set((state) => {
      // A download that left the list leaves the selection, so no action targets it.
      const ids = new Set(snapshot.downloads.map((d) => d.id));
      const kept = [...state.selection].filter((id) => ids.has(id));
      return { snapshot, error: null, selection: kept.length === state.selection.size ? state.selection : new Set(kept) };
    });
  },
  setError: (error) => { set({ error }); },
  setSearch: (search) => { set({ search }); },
  toggleSort: (key) => {
    const current = get().sort;
    set({ sort: current.key === key ? { key, dir: current.dir === 'asc' ? 'desc' : 'asc' } : { key, dir: key === 'name' || key === 'queue' ? 'asc' : 'desc' } });
  },
  select: (id, mode, orderedIds = []) => {
    set((state) => {
      if (mode === 'toggle') {
        const next = new Set(state.selection);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return { selection: next, anchor: id, focus: id };
      }
      if (mode === 'range' && state.anchor) {
        const a = orderedIds.indexOf(state.anchor);
        const b = orderedIds.indexOf(id);
        if (a >= 0 && b >= 0) return { selection: new Set(orderedIds.slice(Math.min(a, b), Math.max(a, b) + 1)), focus: id };
      }
      return { selection: new Set([id]), anchor: id, focus: id };
    });
  },
  selectAll: (ids) => { set({ selection: new Set(ids), anchor: ids[0] ?? null, focus: ids[0] ?? null }); },
  clearSelection: () => { set({ selection: new Set(), anchor: null, focus: null }); },
}));
