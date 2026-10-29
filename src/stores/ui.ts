import { create } from 'zustand';
import { DEFAULT_LAYOUT, type LayoutPrefs } from '../layout/layoutLogic';
import type { ListView } from '../downloads/views';

/** What the main area shows: a list of downloads, or a screen. */
export type View = ListView | 'categories';

// Presentation state only (V1's ui store); what the native side owns comes through queries and events.
interface UiState {
  view: View;
  /** The category the list is filtered by, if any. */
  categoryFilter: string | null;
  /** Panel sizes and visibility, loaded from and saved to the `appearance` settings. */
  layout: LayoutPrefs;
  setView: (view: View) => void;
  setCategoryFilter: (id: string | null) => void;
  setLayout: (patch: Partial<LayoutPrefs>) => void;
}

export const useUiStore = create<UiState>((set) => ({
  view: 'all',
  categoryFilter: null,
  layout: { ...DEFAULT_LAYOUT },
  setView: (view) => { set({ view, categoryFilter: null }); },
  setCategoryFilter: (categoryFilter) => { set({ categoryFilter, view: 'all' }); },
  setLayout: (patch) => { set((s) => ({ layout: { ...s.layout, ...patch } })); },
}));
