import { describe, expect, it } from 'vitest';
import {
  DEFAULT_LAYOUT,
  LAYOUT,
  commitSidebarWidth,
  defaultDetailsWidth,
  defaultSidebarWidth,
  resolveLayout,
  sanitizeLayout,
  type LayoutPrefs,
} from './layoutLogic';

const prefs = (over: Partial<LayoutPrefs> = {}): LayoutPrefs => ({ ...DEFAULT_LAYOUT, ...over });

describe('default panel widths', () => {
  it('keeps the navigation between 220 and 240 px and grows with the window', () => {
    expect(defaultSidebarWidth(960)).toBe(220);
    expect(defaultSidebarWidth(1366)).toBe(232);
    expect(defaultSidebarWidth(1920)).toBe(240);
    expect(defaultSidebarWidth(4000)).toBe(240);
  });

  it('keeps the details panel between 300 and 340 px', () => {
    expect(defaultDetailsWidth(960)).toBe(300);
    expect(defaultDetailsWidth(1366)).toBe(300);
    expect(defaultDetailsWidth(1600)).toBe(320);
    expect(defaultDetailsWidth(1920)).toBe(340);
  });

  it('starts with no saved widths, the navigation shown and the details panel closed', () => {
    expect(DEFAULT_LAYOUT).toEqual({ sidebarWidth: null, sidebarCollapsed: false, sidebarVisible: true, detailsWidth: null, detailsVisible: false });
  });

  it('uses the responsive defaults only when no width was saved', () => {
    const r = resolveLayout(prefs(), 1366, true);
    expect(r.sidebar.width).toBe(232);
    expect(r.details.width).toBe(300);
    const saved = resolveLayout(prefs({ sidebarWidth: 300, detailsWidth: 420 }), 1366, true);
    expect(saved.sidebar.width).toBe(300);
    expect(saved.details.width).toBe(420);
  });

  it('leaves the table at least its minimum width at the minimum window size', () => {
    const r = resolveLayout(prefs(), LAYOUT.WINDOW_MIN_WIDTH, true);
    expect(LAYOUT.WINDOW_MIN_WIDTH - r.sidebar.width - r.details.width).toBeGreaterThanOrEqual(LAYOUT.TABLE_MIN);
  });
});

describe('width bounds', () => {
  it('clamps the navigation to its bounds', () => {
    expect(resolveLayout(prefs({ sidebarWidth: 10 }), 1920, false).sidebar.width).toBe(LAYOUT.SIDEBAR_MIN);
    expect(resolveLayout(prefs({ sidebarWidth: 5000 }), 1920, false).sidebar.width).toBe(LAYOUT.SIDEBAR_MAX);
    const r = resolveLayout(prefs(), 1920, false);
    expect(r.sidebar.min).toBe(LAYOUT.SIDEBAR_MIN);
    expect(r.sidebar.max).toBe(LAYOUT.SIDEBAR_MAX);
  });

  it('clamps the details panel to its bounds', () => {
    expect(resolveLayout(prefs({ detailsWidth: 10 }), 1920, true).details.width).toBe(LAYOUT.DETAILS_MIN);
    expect(resolveLayout(prefs({ detailsWidth: 5000 }), 1920, true).details.width).toBe(LAYOUT.DETAILS_MAX);
    const r = resolveLayout(prefs(), 1920, true);
    expect(r.details.min).toBe(LAYOUT.DETAILS_MIN);
    expect(r.details.max).toBe(LAYOUT.DETAILS_MAX);
  });

  it('lowers each maximum so the table keeps its minimum width', () => {
    const r = resolveLayout(prefs({ sidebarWidth: 300 }), 1100, true);
    expect(r.details.max).toBe(1100 - 300 - LAYOUT.TABLE_MIN);
    expect(r.sidebar.max).toBe(1100 - r.details.width - LAYOUT.TABLE_MIN);
  });
});

describe('window resizing', () => {
  it('shrinks the details panel first, then the navigation, without changing the saved widths', () => {
    const p = prefs({ sidebarWidth: 340, detailsWidth: 560 });
    const wide = resolveLayout(p, 1920, true);
    expect([wide.sidebar.width, wide.details.width]).toEqual([340, 560]);
    const mid = resolveLayout(p, 1200, true);
    expect(mid.sidebar.width).toBe(340);
    expect(mid.details.width).toBe(1200 - 340 - LAYOUT.TABLE_MIN);
    const narrow = resolveLayout(p, 960, true);
    expect(narrow.details.width).toBe(LAYOUT.DETAILS_MIN);
    expect(narrow.sidebar.width).toBe(960 - LAYOUT.DETAILS_MIN - LAYOUT.TABLE_MIN);
    expect(p).toEqual(prefs({ sidebarWidth: 340, detailsWidth: 560 }));
  });

  it('switches the navigation to icons when even its minimum does not fit', () => {
    const r = resolveLayout(prefs(), 800, true);
    expect(r.sidebar.mode).toBe('rail');
    expect(r.sidebar.autoCompact).toBe(true);
    expect(r.sidebar.width).toBe(LAYOUT.SIDEBAR_RAIL);
  });

  it('gives the table the space of a closed details panel or hidden navigation', () => {
    expect(resolveLayout(prefs(), 1366, false).details.width).toBe(0);
    const hidden = resolveLayout(prefs({ sidebarVisible: false }), 1366, false);
    expect(hidden.sidebar.mode).toBe('hidden');
    expect(hidden.sidebar.width).toBe(0);
  });

  it('shows the icon rail when collapsed', () => {
    const r = resolveLayout(prefs({ sidebarCollapsed: true, sidebarWidth: 300 }), 1366, false);
    expect(r.sidebar.mode).toBe('rail');
    expect(r.sidebar.autoCompact).toBe(false);
    expect(r.sidebar.width).toBe(LAYOUT.SIDEBAR_RAIL);
  });
});

describe('saved layout validation', () => {
  it('fills defaults for missing or malformed values', () => {
    expect(sanitizeLayout(undefined)).toEqual(DEFAULT_LAYOUT);
    expect(sanitizeLayout('nonsense')).toEqual(DEFAULT_LAYOUT);
    expect(sanitizeLayout({ sidebarWidth: 'wide', detailsWidth: Number.NaN, sidebarCollapsed: 'yes', detailsVisible: 1 })).toEqual(DEFAULT_LAYOUT);
  });

  it('clamps out-of-range widths to safe bounds', () => {
    expect(sanitizeLayout({ sidebarWidth: -40, detailsWidth: 1e9 })).toMatchObject({ sidebarWidth: LAYOUT.SIDEBAR_MIN, detailsWidth: LAYOUT.DETAILS_MAX });
    expect(sanitizeLayout({ sidebarWidth: Number.POSITIVE_INFINITY })).toMatchObject({ sidebarWidth: null });
    expect(sanitizeLayout({ sidebarWidth: 250.6 })).toMatchObject({ sidebarWidth: 251 });
  });

  it('keeps valid values', () => {
    const p = { sidebarWidth: 250, sidebarCollapsed: true, sidebarVisible: false, detailsWidth: 400, detailsVisible: true };
    expect(sanitizeLayout(p)).toEqual(p);
  });
});

describe('navigation collapse by dragging', () => {
  it('collapses when dragged well below the minimum and keeps the saved width', () => {
    expect(commitSidebarWidth(prefs({ sidebarWidth: 260 }), LAYOUT.SIDEBAR_MIN - 60)).toEqual({ sidebarCollapsed: true });
  });

  it('expands from the rail when dragged wider', () => {
    expect(commitSidebarWidth(prefs({ sidebarCollapsed: true }), 150)).toEqual({ sidebarCollapsed: false, sidebarWidth: LAYOUT.SIDEBAR_MIN });
    expect(commitSidebarWidth(prefs({ sidebarCollapsed: true }), 70)).toEqual({});
  });

  it('saves a clamped width otherwise', () => {
    expect(commitSidebarWidth(prefs(), 275.4)).toEqual({ sidebarCollapsed: false, sidebarWidth: 275 });
    expect(commitSidebarWidth(prefs(), LAYOUT.SIDEBAR_MIN - 10)).toEqual({ sidebarCollapsed: false, sidebarWidth: LAYOUT.SIDEBAR_MIN });
  });
});
