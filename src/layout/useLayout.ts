import { useQueryClient } from '@tanstack/react-query';
import { useEffect, useLayoutEffect, useSyncExternalStore } from 'react';
import { toast } from 'sonner';
import type { AppearanceSettings, ShellSettings } from '../contract';
import { api } from '../lib/api';
import { toApiError } from '../lib/errors';
import { keys } from '../lib/queries';
import { useUiStore } from '../stores/ui';
import { sanitizeLayout, type LayoutPrefs } from './layoutLogic';

/** Coalesces bursts (key repeats, several toggles) into one save (V1's delay). */
const SAVE_DELAY_MS = 250;

const sameLayout = (a: LayoutPrefs, b: LayoutPrefs) =>
  a.sidebarWidth === b.sidebarWidth &&
  a.sidebarCollapsed === b.sidebarCollapsed &&
  a.sidebarVisible === b.sidebarVisible &&
  a.detailsWidth === b.detailsWidth &&
  a.detailsVisible === b.detailsVisible;

/**
 * Keeps the store's layout in step with the saved `appearance` settings (V1's useLayoutPersistence):
 * it takes the saved layout before the first paint, and saves a change shortly after it stops, through
 * `set_appearance`. A drag changes the store only when it ends (Splitter), so a drag is one save.
 */
export function useLayoutPersistence(saved: AppearanceSettings) {
  const qc = useQueryClient();

  useLayoutEffect(() => {
    const next = sanitizeLayout(saved.layout);
    if (!sameLayout(next, useUiStore.getState().layout)) useUiStore.getState().setLayout(next);
  }, [saved.layout]);

  useEffect(() => {
    let timer: number | undefined;
    const flush = () => {
      window.clearTimeout(timer);
      timer = undefined;
      const current = qc.getQueryData<ShellSettings>(keys.shellSettings);
      const layout = useUiStore.getState().layout;
      if (!current || sameLayout(current.appearance.layout, layout)) return;
      api
        .setAppearance({ ...current.appearance, layout: { ...layout } })
        .then((appearance) => {
          qc.setQueryData<ShellSettings>(keys.shellSettings, (s) => (s ? { ...s, appearance } : s));
        })
        .catch((e: unknown) => toast.error(toApiError(e).message));
    };
    const unsubscribe = useUiStore.subscribe((s, prev) => {
      if (s.layout === prev.layout) return;
      window.clearTimeout(timer);
      timer = window.setTimeout(flush, SAVE_DELAY_MS);
    });
    return () => {
      unsubscribe();
      if (timer !== undefined) flush();
    };
  }, [qc]);
}

const subscribeResize = (cb: () => void) => {
  window.addEventListener('resize', cb);
  return () => { window.removeEventListener('resize', cb); };
};

/** The window's width in CSS pixels; browsers send resize events at most once a frame. */
export function useViewportWidth(): number {
  return useSyncExternalStore(subscribeResize, () => window.innerWidth);
}
