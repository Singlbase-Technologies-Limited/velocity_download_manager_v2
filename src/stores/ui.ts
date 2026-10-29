import { create } from 'zustand';
import { DEFAULT_LAYOUT, type LayoutPrefs } from '../layout/layoutLogic';
import type { ListView } from '../downloads/views';

/** What the main area shows: a list of downloads, or a screen. */
export type View = ListView | 'categories';

/** What the Add dialog opens with (#60): pasted or dropped input, or what the browser sent (#64, #65). */
export interface AddPrefill {
  links?: string;
  fileName?: string;
  referrer?: string;
  torrentFiles?: string[];
  /** Sent from the browser: shown above the form. */
  fromBrowser?: { referrer: string | null; fileSize: number | null; mime: string | null };
}

/** Which download needs its login again, and whether it is then resumed or retried. */
export interface LoginRequest {
  id: string;
  then: 'resume' | 'retry';
}

// Presentation state only (V1's ui store); what the native side owns comes through queries and events.
interface UiState {
  view: View;
  /** The category the list is filtered by, if any. */
  categoryFilter: string | null;
  /** Panel sizes and visibility, loaded from and saved to the `appearance` settings. */
  layout: LayoutPrefs;
  /** The Add dialog, and what it opened with. */
  addDialog: { open: boolean; prefill: AddPrefill | null };
  /** A download that came back without its login, and what to do once it is given again (#59). */
  loginRequest: LoginRequest | null;
  setView: (view: View) => void;
  setCategoryFilter: (id: string | null) => void;
  setLayout: (patch: Partial<LayoutPrefs>) => void;
  /** Opens the Add dialog; what it opened with replaces an open one's. */
  openAdd: (prefill?: AddPrefill) => void;
  closeAdd: () => void;
  askLogin: (request: LoginRequest) => void;
  closeLogin: () => void;
}

export const useUiStore = create<UiState>((set) => ({
  view: 'all',
  categoryFilter: null,
  layout: { ...DEFAULT_LAYOUT },
  addDialog: { open: false, prefill: null },
  loginRequest: null,
  setView: (view) => { set({ view, categoryFilter: null }); },
  setCategoryFilter: (categoryFilter) => { set({ categoryFilter, view: 'all' }); },
  setLayout: (patch) => { set((s) => ({ layout: { ...s.layout, ...patch } })); },
  openAdd: (prefill) => { set({ addDialog: { open: true, prefill: prefill ?? null } }); },
  closeAdd: () => { set({ addDialog: { open: false, prefill: null } }); },
  askLogin: (loginRequest) => { set({ loginRequest }); },
  closeLogin: () => { set({ loginRequest: null }); },
}));
