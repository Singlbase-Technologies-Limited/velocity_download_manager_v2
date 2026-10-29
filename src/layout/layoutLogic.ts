// Ported from V1 (src/features/layout/layoutLogic.ts) for #29. The bounds mirror
// crates/velocity-core/src/app_settings.rs, which checks what is saved.

/**
 * Pure layout rules for the resizable navigation sidebar and details panel: defaults,
 * bounds, validation of saved values and fitting both panels into the window. All
 * widths are CSS pixels, so they do not depend on the display's scale factor.
 */

export const LAYOUT = {
  SIDEBAR_MIN: 184,
  SIDEBAR_MAX: 360,
  /** Width of the icon-only navigation. */
  SIDEBAR_RAIL: 56,
  DETAILS_MIN: 280,
  DETAILS_MAX: 600,
  /** Space always left for the download table, including the 1 px panel dividers. */
  TABLE_MIN: 440,
  /** The desktop window's minimum width (src-tauri/tauri.conf.json). */
  WINDOW_MIN_WIDTH: 800,
  /** Dragging the navigation this far below its minimum collapses it to icons. */
  COLLAPSE_SNAP: 48,
  /** Dragging the icon rail this far wider expands it again. */
  EXPAND_SNAP: 40,
} as const;

/** Saved layout preferences (the `appearance` settings' `layout`). A null width means "use the responsive default". */
export interface LayoutPrefs {
  sidebarWidth: number | null;
  sidebarCollapsed: boolean;
  sidebarVisible: boolean;
  detailsWidth: number | null;
  detailsVisible: boolean;
}

export const DEFAULT_LAYOUT: LayoutPrefs = {
  sidebarWidth: null,
  sidebarCollapsed: false,
  sidebarVisible: true,
  detailsWidth: null,
  detailsVisible: false,
};

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** About 17% of the window, between 220 and 240 px. */
export const defaultSidebarWidth = (viewport: number) => clamp(Math.round(viewport * 0.17), 220, 240);

/** A fifth of the window, between 300 and 340 px, so the table gets most of the space. */
export const defaultDetailsWidth = (viewport: number) => clamp(Math.round(viewport * 0.2), 300, 340);

function width(v: unknown, lo: number, hi: number): number | null {
  return typeof v === 'number' && Number.isFinite(v) ? clamp(Math.round(v), lo, hi) : null;
}

const bool = (v: unknown, fallback: boolean) => (typeof v === 'boolean' ? v : fallback);

/** Accept anything that was saved and return safe preferences: bad values become defaults, widths are clamped. */
export function sanitizeLayout(raw: unknown): LayoutPrefs {
  if (typeof raw !== 'object' || raw === null) return { ...DEFAULT_LAYOUT };
  const r = raw as Record<string, unknown>;
  return {
    sidebarWidth: width(r.sidebarWidth, LAYOUT.SIDEBAR_MIN, LAYOUT.SIDEBAR_MAX),
    sidebarCollapsed: bool(r.sidebarCollapsed, DEFAULT_LAYOUT.sidebarCollapsed),
    sidebarVisible: bool(r.sidebarVisible, DEFAULT_LAYOUT.sidebarVisible),
    detailsWidth: width(r.detailsWidth, LAYOUT.DETAILS_MIN, LAYOUT.DETAILS_MAX),
    detailsVisible: bool(r.detailsVisible, DEFAULT_LAYOUT.detailsVisible),
  };
}

export type SidebarMode = 'hidden' | 'rail' | 'full';

export interface ResolvedLayout {
  sidebar: { mode: SidebarMode; width: number; min: number; max: number; /** Icons only because the window is too narrow. */ autoCompact: boolean };
  details: { open: boolean; width: number; min: number; max: number };
}

/**
 * Effective widths for a window width. Saved preferences are never changed here: when
 * the window is too narrow the details panel shrinks first, then the navigation, then
 * the navigation falls back to icons, so the table keeps at least TABLE_MIN.
 */
export function resolveLayout(prefs: LayoutPrefs, viewport: number, detailsOpen: boolean): ResolvedLayout {
  let mode: SidebarMode = !prefs.sidebarVisible ? 'hidden' : prefs.sidebarCollapsed ? 'rail' : 'full';
  let sidebar = mode === 'hidden' ? 0 : mode === 'rail' ? LAYOUT.SIDEBAR_RAIL : clamp(prefs.sidebarWidth ?? defaultSidebarWidth(viewport), LAYOUT.SIDEBAR_MIN, LAYOUT.SIDEBAR_MAX);
  let details = detailsOpen ? clamp(prefs.detailsWidth ?? defaultDetailsWidth(viewport), LAYOUT.DETAILS_MIN, LAYOUT.DETAILS_MAX) : 0;
  const budget = viewport - LAYOUT.TABLE_MIN;
  let autoCompact = false;

  let over = sidebar + details - budget;
  if (over > 0 && detailsOpen) {
    const cut = Math.min(over, details - LAYOUT.DETAILS_MIN);
    details -= cut;
    over -= cut;
  }
  if (over > 0 && mode === 'full') {
    const cut = Math.min(over, sidebar - LAYOUT.SIDEBAR_MIN);
    sidebar -= cut;
    over -= cut;
    if (over > 0) {
      mode = 'rail';
      sidebar = LAYOUT.SIDEBAR_RAIL;
      autoCompact = true;
    }
  }

  return {
    sidebar: { mode, width: sidebar, min: LAYOUT.SIDEBAR_MIN, max: Math.max(LAYOUT.SIDEBAR_MIN, Math.min(LAYOUT.SIDEBAR_MAX, budget - details)), autoCompact },
    details: { open: detailsOpen, width: details, min: LAYOUT.DETAILS_MIN, max: Math.max(LAYOUT.DETAILS_MIN, Math.min(LAYOUT.DETAILS_MAX, budget - sidebar)) },
  };
}

/** What a finished drag (or key press) of the navigation splitter changes in the saved preferences. */
export function commitSidebarWidth(prefs: LayoutPrefs, raw: number): Partial<LayoutPrefs> {
  if (prefs.sidebarCollapsed) {
    return raw >= LAYOUT.SIDEBAR_RAIL + LAYOUT.EXPAND_SNAP ? { sidebarCollapsed: false, sidebarWidth: clamp(Math.round(raw), LAYOUT.SIDEBAR_MIN, LAYOUT.SIDEBAR_MAX) } : {};
  }
  if (raw < LAYOUT.SIDEBAR_MIN - LAYOUT.COLLAPSE_SNAP) return { sidebarCollapsed: true };
  return { sidebarCollapsed: false, sidebarWidth: clamp(Math.round(raw), LAYOUT.SIDEBAR_MIN, LAYOUT.SIDEBAR_MAX) };
}
