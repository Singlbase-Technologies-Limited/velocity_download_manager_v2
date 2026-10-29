import { useCallback, useEffect, useLayoutEffect, useMemo, useRef } from 'react';
import { Splitter } from '../components/Splitter';
import { confirmAction } from '../components/confirmStore';
import type { AppearanceSettings, AppInfo } from '../contract';
import { CategoriesView } from '../features/categories/CategoriesView';
import { AddDownloadDialog } from '../features/add/AddDownloadDialog';
import { HostKeyPrompt } from '../features/add/HostKeyPrompt';
import { useAddEntryPoints } from '../features/add/useAddEntryPoints';
import { DownloadsView } from '../features/downloads/DownloadsView';
import { HistoryView } from '../features/history/HistoryView';
import { commitSidebarWidth, LAYOUT, resolveLayout } from '../layout/layoutLogic';
import { useLayoutPersistence, useViewportWidth } from '../layout/useLayout';
import { api } from '../lib/api';
import { onQuitRequested } from '../lib/events';
import { useUiStore } from '../stores/ui';
import { quitQuestion } from './quitQuestion';
import { Sidebar } from './Sidebar';
import { StatusBar } from './StatusBar';

/** The native side asks before quitting with downloads active; the window answers. */
function useQuitRequests() {
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onQuitRequested(({ active }) => {
      void confirmAction(quitQuestion(active)).then((quit) => {
        // Quitting can't fail in a way the window could show: the app is gone.
        if (quit) void api.quit();
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
}

/** The window once set up: navigation, the current view and the status bar (V1's Shell). */
export function Shell({ app, appearance }: { app: AppInfo; appearance: AppearanceSettings }) {
  useLayoutPersistence(appearance);
  useQuitRequests();
  useAddEntryPoints();
  const view = useUiStore((s) => s.view);
  const layout = useUiStore((s) => s.layout);
  const setLayout = useUiStore((s) => s.setLayout);
  const viewport = useViewportWidth();
  // The details panel comes with the download list (#59).
  const resolved = useMemo(() => resolveLayout(layout, viewport, false), [layout, viewport]);
  const rootRef = useRef<HTMLDivElement>(null);

  // The navigation's width lives in a CSS variable, so a drag updates it without a React render.
  const writeWidth = useCallback((width: number) => rootRef.current?.style.setProperty('--sidebar-w', `${String(width)}px`), []);
  useLayoutEffect(() => {
    writeWidth(resolved.sidebar.width);
  });

  const content = useMemo(() => (view === 'categories' ? <CategoriesView /> : view === 'history' ? <HistoryView /> : <DownloadsView view={view} />), [view]);
  const sidebar = resolved.sidebar;
  const rail = sidebar.mode === 'rail';

  return (
    <div className="flex h-full flex-col">
      <div ref={rootRef} className="flex min-h-0 flex-1">
        {sidebar.mode !== 'hidden' ? (
          <>
            <Sidebar app={app} compact={rail} autoCompact={sidebar.autoCompact} />
            <Splitter
              label="Resize navigation"
              controls="app-sidebar"
              side="start"
              value={sidebar.width}
              min={rail ? LAYOUT.SIDEBAR_RAIL : sidebar.min}
              max={sidebar.max}
              onPreview={writeWidth}
              onCommit={(_width, raw) => {
                writeWidth(sidebar.width);
                setLayout(commitSidebarWidth(layout, raw));
              }}
              onToggle={() => { setLayout({ sidebarCollapsed: !layout.sidebarCollapsed }); }}
              onReset={() => { setLayout({ sidebarWidth: null, sidebarCollapsed: false }); }}
            />
          </>
        ) : null}
        <main className="flex min-w-0 flex-1 flex-col overflow-hidden bg-bg" aria-label="Main content">
          {content}
        </main>
      </div>
      <StatusBar />
      <AddDownloadDialog />
      <HostKeyPrompt />
    </div>
  );
}
