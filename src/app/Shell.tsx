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
import { SettingsView } from '../features/settings/SettingsView';
import { ChooseFilesDialog } from '../features/torrents/ChooseFilesDialog';
import { commitSidebarWidth, LAYOUT, resolveLayout } from '../layout/layoutLogic';
import { ShellLayoutContext, useLayoutPersistence, useViewportWidth, type ShellLayout } from '../layout/useLayout';
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
  // The details panel belongs to the download lists (#61).
  const detailsOpen = layout.detailsVisible && view !== 'categories' && view !== 'history' && view !== 'settings';
  const resolved = useMemo(() => resolveLayout(layout, viewport, detailsOpen), [layout, viewport, detailsOpen]);
  const rootRef = useRef<HTMLDivElement>(null);

  // The panels' widths live in CSS variables, so a drag updates them without a React render.
  const writeVar = useCallback((name: '--sidebar-w' | '--details-w', width: number) => rootRef.current?.style.setProperty(name, `${String(width)}px`), []);
  const writeWidth = useCallback((width: number) => writeVar('--sidebar-w', width), [writeVar]);
  useLayoutEffect(() => {
    writeWidth(resolved.sidebar.width);
    writeVar('--details-w', resolved.details.width);
  });
  const shellLayout = useMemo<ShellLayout>(() => ({ resolved, previewDetails: (w) => writeVar('--details-w', w) }), [resolved, writeVar]);

  const content = useMemo(() => {
    switch (view) {
      case 'categories':
        return <CategoriesView />;
      case 'history':
        return <HistoryView />;
      case 'settings':
        return <SettingsView />;
      default:
        return <DownloadsView view={view} />;
    }
  }, [view]);
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
          <ShellLayoutContext.Provider value={shellLayout}>{content}</ShellLayoutContext.Provider>
        </main>
      </div>
      <StatusBar />
      <AddDownloadDialog />
      <ChooseFilesDialog />
      <HostKeyPrompt />
    </div>
  );
}
