import * as DropdownPrimitive from '@radix-ui/react-dropdown-menu';
import { useQueryClient } from '@tanstack/react-query';
import { AlertTriangle, Check, Columns3, Download as DownloadIcon, Inbox, MoreHorizontal, PanelRight, Pause, Play, RotateCcw, Search, Square, Trash2, X } from 'lucide-react';
import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react';
import { toast } from 'sonner';
import { Button } from '../../components/Button';
import { EmptyState } from '../../components/EmptyState';
import { DropdownMenu, menuContent, menuItem } from '../../components/Menu';
import { Input } from '../../components/Input';
import { Spinner } from '../../components/Spinner';
import { Splitter } from '../../components/Splitter';
import { ColumnIdSchema, DEFAULT_COLUMNS, type AppearanceSettings, type ColumnId, type ShellSettings } from '../../contract';
import { actionsForSelection, visibleDownloads } from '../../downloads/list';
import type { ListView } from '../../downloads/views';
import { useShellLayout } from '../../layout/useLayout';
import { api } from '../../lib/api';
import { toApiError } from '../../lib/errors';
import { keys, useCategories, useShellSettings } from '../../lib/queries';
import { useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';
import { ALL_COLUMNS, columnsFor } from './columns';
import { DetailsPanel } from './DetailsPanel';
import { DownloadTable } from './DownloadTable';
import { LoginDialog } from './LoginDialog';
import { useDownloadActions } from './useDownloadActions';

const TITLES: Record<ListView, string> = {
  all: 'All Downloads',
  downloading: 'Downloading',
  queued: 'Queued',
  completed: 'Completed',
  seeding: 'Seeding',
  failed: 'Failed',
};

const EMPTY: Record<ListView, { title: string; description: string }> = {
  all: { title: 'No downloads yet', description: 'Add a link with Add download in the navigation, or press Ctrl+N.' },
  downloading: { title: 'Nothing is downloading', description: 'Active downloads appear here with the speed and progress the engine reports.' },
  queued: { title: 'The queue is empty', description: 'Queued, paused and retrying downloads appear here. Reorder them from the right-click menu.' },
  completed: { title: 'No completed downloads', description: 'Finished downloads appear here once the engine has checked their files.' },
  seeding: { title: 'Nothing is seeding', description: 'Torrents that share what they downloaded appear here.' },
  failed: { title: 'No failed downloads', description: 'Downloads that fail or are cancelled appear here, so you can retry them.' },
};

const everySecond = (tick: () => void) => {
  const timer = window.setInterval(tick, 1000);
  return () => { window.clearInterval(timer); };
};
const never = () => () => undefined;
/** The current second, so a snapshot of it is stable until the next one. */
const thisSecond = () => Math.floor(Date.now() / 1000) * 1000;

/** Now, in ms since the Unix epoch, to the second; it ticks only while something counts down. */
function useNow(ticking: boolean): number {
  return useSyncExternalStore(ticking ? everySecond : never, thisSecond);
}

/** Saves a change to the list's columns over the latest saved appearance, so a layout saved meanwhile is kept. */
function useSaveColumns() {
  const qc = useQueryClient();
  return (change: (a: AppearanceSettings) => Partial<AppearanceSettings>) => {
    const current = qc.getQueryData<ShellSettings>(keys.shellSettings);
    if (!current) return;
    api
      .setAppearance({ ...current.appearance, ...change(current.appearance) })
      .then((appearance) => { qc.setQueryData<ShellSettings>(keys.shellSettings, (s) => (s ? { ...s, appearance } : s)); })
      .catch((e: unknown) => toast.error(toApiError(e).message));
  };
}

/** The details panel's splitter (V1's): a drag previews the width, and its end saves it. */
function DetailsSplitter() {
  const shell = useShellLayout();
  const setLayout = useUiStore((s) => s.setLayout);
  if (!shell) return null;
  const d = shell.resolved.details;
  return (
    <Splitter
      label="Resize details panel"
      controls="details-panel"
      side="end"
      value={d.width}
      min={d.min}
      max={d.max}
      onPreview={shell.previewDetails}
      onCommit={(w) => {
        shell.previewDetails(d.width);
        setLayout({ detailsWidth: w });
      }}
      onReset={() => { setLayout({ detailsWidth: null }); }}
    />
  );
}

/** V1's downloads view (src/features/downloads/DownloadsView.tsx): the toolbar, the list, its empty states and the details panel. */
export function DownloadsView({ view }: { view: ListView }) {
  const snapshot = useDownloadsStore((s) => s.snapshot);
  const search = useDownloadsStore((s) => s.search);
  const setSearch = useDownloadsStore((s) => s.setSearch);
  const sort = useDownloadsStore((s) => s.sort);
  const toggleSort = useDownloadsStore((s) => s.toggleSort);
  const selection = useDownloadsStore((s) => s.selection);
  const clearSelection = useDownloadsStore((s) => s.clearSelection);
  const focus = useDownloadsStore((s) => s.focus);
  const categoryFilter = useUiStore((s) => s.categoryFilter);
  const detailsId = useUiStore((s) => s.detailsId);
  const detailsVisible = useUiStore((s) => s.layout.detailsVisible);
  const showDetails = useUiStore((s) => s.showDetails);
  const { data: categories } = useCategories();
  const { data: settings } = useShellSettings();
  const { actions, busy } = useDownloadActions();
  const saveColumns = useSaveColumns();
  const searchRef = useRef<HTMLInputElement>(null);

  const categoryName = useMemo(() => {
    const names = new Map((categories ?? []).map((c) => [c.id, c.name]));
    return (id: string | null) => (id ? (names.get(id) ?? '') : '');
  }, [categories]);
  const rows = useMemo(
    () => visibleDownloads(snapshot?.downloads ?? [], { view, search, category: categoryFilter, sort }, categoryName),
    [snapshot, view, search, categoryFilter, sort, categoryName],
  );
  const needsLogin = useMemo(() => new Set(snapshot?.needsLogin), [snapshot]);
  const selected = useMemo(() => rows.filter((r) => selection.has(r.id)), [rows, selection]);
  const can = actionsForSelection(selected);
  const ids = selected.map((d) => d.id);
  const appearance = settings?.appearance;
  const visible = appearance?.visibleColumns ?? DEFAULT_COLUMNS;
  const columns = columnsFor(visible);
  const now = useNow(rows.some((r) => r.status === 'retrying' && r.retryAtMs !== null));
  const title = categoryFilter ? (categories?.find((c) => c.id === categoryFilter)?.name ?? 'Category') : TITLES[view];
  const engine = snapshot?.engine;
  // An open panel follows the focused row (click or arrow keys); without one it shows the download it was opened for,
  // even while the search hides it, else the first row (V1's).
  const focusedRow = focus && rows.some((r) => r.id === focus) ? focus : null;
  const detailsTarget = detailsVisible ? (focusedRow ?? detailsId ?? rows[0]?.id ?? null) : null;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        searchRef.current?.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => { window.removeEventListener('keydown', onKey); };
  }, []);

  const resizeColumn = (id: ColumnId, px: number | null) => {
    saveColumns((a) => {
      const others = Object.entries(a.columnWidths).filter(([c]) => c !== id);
      return { columnWidths: Object.fromEntries(px === null ? others : [...others, [id, px]]) };
    });
  };
  const toggleColumn = (id: ColumnId, on: boolean) => {
    saveColumns((a) => {
      const chosen = new Set(a.visibleColumns);
      if (on) chosen.add(id);
      else chosen.delete(id);
      return { visibleColumns: ColumnIdSchema.options.filter((c) => chosen.has(c)) };
    });
  };
  const resetColumns = () => { saveColumns(() => ({ visibleColumns: [...DEFAULT_COLUMNS], columnWidths: {} })); };

  return (
    <div className="@container flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center gap-1.5 border-b border-border bg-surface px-3" role="toolbar" aria-label="Download actions">
        <Button variant="ghost" size="icon" aria-label="Resume selected" title="Resume selected" disabled={busy || !can.resume} onClick={() => void actions.resume(ids)}>
          <Play />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Pause selected" title="Pause selected" disabled={busy || !can.pause} onClick={() => void actions.pause(ids)}>
          <Pause />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Cancel selected" title="Cancel selected (keeps what was downloaded for a retry)" disabled={busy || !can.cancel} onClick={() => void actions.cancel(ids)}>
          <Square />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Retry selected" title="Retry selected" disabled={busy || !can.retry} onClick={() => void actions.retry(ids)}>
          <RotateCcw />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Remove selected from list" title="Remove selected from list" disabled={busy || !can.remove} onClick={() => void actions.remove(selected)}>
          <X />
        </Button>
        <div className="mx-1.5 h-5 w-px bg-border" />
        {/* Labels give way to icons when the list is narrow; the names stay for assistive technology. */}
        <Button variant="ghost" size="sm" disabled={busy} onClick={() => void actions.resumeAll()} title="Resume all">
          <Play /> <span className="@max-4xl:sr-only">Resume all</span>
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={() => void actions.pauseAll()} title="Pause all">
          <Pause /> <span className="@max-4xl:sr-only">Pause all</span>
        </Button>
        <DropdownMenu
          trigger={
            <Button variant="ghost" size="icon" aria-label="More actions" title="More actions">
              <MoreHorizontal />
            </Button>
          }
          items={[
            { label: 'Retry all failed', icon: <RotateCcw />, disabled: busy, onSelect: () => void actions.retryFailed() },
            { label: 'Clear completed from list', icon: <X />, disabled: busy, onSelect: () => void actions.clearCompleted() },
            'separator',
            { label: 'Delete files of selected…', icon: <Trash2 />, destructive: true, disabled: busy || !can.deleteFiles, onSelect: () => void actions.removeAndDelete(selected) },
          ]}
        />
        <div className="flex-1" />
        <div className="relative w-64 min-w-28 shrink">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-fg-subtle" aria-hidden />
          <Input ref={searchRef} type="search" value={search} onChange={(e) => { setSearch(e.target.value); }} placeholder="Search name, link or folder (Ctrl+F)" aria-label="Search downloads" className="pl-8" />
        </div>
        <DropdownPrimitive.Root>
          <DropdownPrimitive.Trigger asChild>
            <Button variant="ghost" size="icon" aria-label="Choose columns" title="Choose columns">
              <Columns3 />
            </Button>
          </DropdownPrimitive.Trigger>
          <DropdownPrimitive.Portal>
            <DropdownPrimitive.Content align="end" sideOffset={4} className={menuContent}>
              <DropdownPrimitive.Label className="px-2 py-1 text-[11px] font-semibold uppercase text-fg-subtle">Columns</DropdownPrimitive.Label>
              {ALL_COLUMNS.map((c) => (
                <DropdownPrimitive.CheckboxItem
                  key={c.id}
                  checked={c.id === 'name' || visible.includes(c.id)}
                  disabled={c.id === 'name'}
                  onSelect={(e) => { e.preventDefault(); }}
                  onCheckedChange={(on) => { toggleColumn(c.id, on); }}
                  className={menuItem}
                >
                  <span className="flex size-4 items-center justify-center">
                    <DropdownPrimitive.ItemIndicator>
                      <Check />
                    </DropdownPrimitive.ItemIndicator>
                  </span>
                  {c.label}
                </DropdownPrimitive.CheckboxItem>
              ))}
              <DropdownPrimitive.Separator className="my-1 h-px bg-border" />
              <DropdownPrimitive.Item className={menuItem} onSelect={resetColumns}>
                <RotateCcw /> Reset to default columns
              </DropdownPrimitive.Item>
            </DropdownPrimitive.Content>
          </DropdownPrimitive.Portal>
        </DropdownPrimitive.Root>
        <Button
          variant="ghost"
          size="icon"
          aria-label="Toggle details panel"
          title={detailsTarget ? 'Hide details' : 'Show details for the focused download'}
          aria-pressed={detailsTarget !== null}
          aria-controls={detailsTarget ? 'details-panel' : undefined}
          onClick={() => { showDetails(detailsTarget ? null : (selected[0]?.id ?? rows[0]?.id ?? null)); }}
        >
          <PanelRight />
        </Button>
      </div>

      <div className="flex h-9 shrink-0 items-center gap-3 border-b border-border px-4">
        <h1 className="text-[13px] font-semibold">{title}</h1>
        <span className="text-xs tabular-nums text-fg-subtle">{rows.length === 1 ? '1 item' : `${String(rows.length)} items`}</span>
        {selected.length > 0 ? (
          <span className="text-xs text-fg-muted">
            · {selected.length} selected{' '}
            <button type="button" className="text-accent hover:underline" onClick={clearSelection}>
              Clear
            </button>
          </span>
        ) : null}
      </div>

      {engine && engine.state !== 'running' ? (
        <div role="status" aria-label="Transfer engine" className="flex shrink-0 items-center gap-2 border-b border-border bg-warning-soft px-4 py-2 text-xs text-warning">
          <AlertTriangle className="size-3.5 shrink-0" aria-hidden />
          {engine.state === 'restarting'
            ? 'The transfer engine stopped and is starting again. Downloads continue once it runs.'
            : 'The transfer engine isn’t running, so direct downloads wait. No speed or time left is shown until it runs again.'}
        </div>
      ) : null}
      {snapshot && snapshot.unavailable.length > 0 ? (
        <p className="shrink-0 border-b border-border px-4 py-2 text-xs text-fg-muted">
          {snapshot.unavailable.length === 1 ? 'One saved download is' : `${String(snapshot.unavailable.length)} saved downloads are`} from a newer version of Velocity, so this one can’t show or use {snapshot.unavailable.length === 1 ? 'it' : 'them'}. Nothing was changed.
        </p>
      ) : null}

      <div className="flex min-h-0 min-w-0 flex-1">
      <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-surface">
        {!snapshot ? (
          <div className="flex flex-1 items-center justify-center gap-2 text-fg-muted">
            <Spinner /> Loading downloads…
          </div>
        ) : rows.length === 0 ? (
          search ? (
            <EmptyState icon={Search} title="No matches" description={`No downloads match “${search}”.`} />
          ) : (
            <EmptyState icon={view === 'all' ? Inbox : DownloadIcon} title={EMPTY[view].title} description={EMPTY[view].description} />
          )
        ) : (
          <DownloadTable
            rows={rows}
            columns={columns}
            widths={appearance?.columnWidths ?? {}}
            onResize={resizeColumn}
            sort={sort}
            onSort={toggleSort}
            actions={actions}
            busy={busy}
            density={appearance?.density ?? 'compact'}
            categories={categories ?? []}
            needsLogin={needsLogin}
            now={now}
          />
        )}
      </div>
        {detailsTarget ? (
          <>
            <DetailsSplitter />
            <DetailsPanel
              id={detailsTarget}
              hidden={!rows.some((r) => r.id === detailsTarget)}
              actions={actions}
              busy={busy}
              needsLogin={needsLogin.has(detailsTarget)}
              now={now}
              onClose={() => { showDetails(null); }}
            />
          </>
        ) : null}
      </div>
      <LoginDialog />
    </div>
  );
}
