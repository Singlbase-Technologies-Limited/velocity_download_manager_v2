import { useVirtualizer } from '@tanstack/react-virtual';
import { ArrowDown, ArrowUp } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { ContextMenu } from '../../components/Menu';
import { SelectBox } from '../../components/SelectBox';
import { COLUMN_MAX_WIDTH, COLUMN_MIN_WIDTH, type Category, type ColumnId, type Density, type DownloadRecord } from '../../contract';
import { canOpenFile, displayName, type SortKey, type SortState } from '../../downloads/list';
import { cn } from '../../lib/cn';
import { useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';
import { clampColumnWidth, type ColumnDef } from './columns';
import { buildMenu } from './menus';
import type { DownloadActions } from './useDownloadActions';

/** Grid gap (gap-3) and horizontal padding (px-4) of the header and rows. */
const COLUMN_GAP = 12;
const ROW_PADDING = 32;
/** The selection checkbox's column. */
const CHECK_WIDTH = 16;
/** One arrow-key press on a column's resizer. */
const RESIZE_STEP = 16;

type Widths = Partial<Record<ColumnId, number>>;

/** A checkbox without a visible label; `mixed` shows the indeterminate state of a partial selection. */
/**
 * A column edge's handle (V1's). A drag previews the width and saves it once, when it ends, so nothing is written per
 * pointer event; arrow keys change it by a step, and Home or a double-click restores its default.
 */
function ColumnResizer({ column, width, onPreview, onCommit }: { column: ColumnDef; width: number | undefined; onPreview: (px: number | null) => void; onCommit: (px: number | null) => void }) {
  const drag = useRef<{ x: number; start: number; last: number } | null>(null);
  // The last width sent by keyboard, so quick presses build on each other before the save comes back.
  const keyed = useRef<number | null>(null);
  useEffect(() => {
    keyed.current = null;
  }, [width]);
  const measure = (el: HTMLElement) => width ?? el.parentElement?.getBoundingClientRect().width ?? column.minWidth;
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={`Resize ${column.label} column`}
      aria-valuemin={Math.max(COLUMN_MIN_WIDTH, column.minWidth)}
      aria-valuemax={COLUMN_MAX_WIDTH}
      aria-valuenow={width}
      tabIndex={0}
      className="absolute -right-2 top-0 z-10 h-full w-3 cursor-col-resize touch-none after:absolute after:left-1/2 after:top-1/4 after:h-1/2 after:w-px after:bg-border hover:after:bg-accent focus-visible:outline-none focus-visible:after:w-0.5 focus-visible:after:bg-accent"
      onPointerDown={(e) => {
        e.preventDefault();
        e.stopPropagation();
        const start = clampColumnWidth(column, measure(e.currentTarget));
        drag.current = { x: e.clientX, start, last: start };
        e.currentTarget.setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        d.last = clampColumnWidth(column, d.start + e.clientX - d.x);
        onPreview(d.last);
      }}
      onPointerUp={(e) => {
        const d = drag.current;
        drag.current = null;
        e.currentTarget.releasePointerCapture(e.pointerId);
        if (d && d.last !== d.start) onCommit(d.last);
        else onPreview(null);
      }}
      onDoubleClick={() => { onCommit(null); }}
      onKeyDown={(e) => {
        const step = e.key === 'ArrowRight' ? RESIZE_STEP : e.key === 'ArrowLeft' ? -RESIZE_STEP : 0;
        if (step) {
          e.preventDefault();
          keyed.current = clampColumnWidth(column, (keyed.current ?? measure(e.currentTarget)) + step);
          onCommit(keyed.current);
        } else if (e.key === 'Home') {
          e.preventDefault();
          keyed.current = null;
          onCommit(null);
        }
      }}
    />
  );
}

interface Props {
  rows: DownloadRecord[];
  columns: ColumnDef[];
  /** The widths the user set, by column. */
  widths: Widths;
  /** Saves a column's width; `null` restores its default. */
  onResize: (id: ColumnId, px: number | null) => void;
  sort: SortState;
  onSort: (key: SortKey) => void;
  actions: DownloadActions;
  busy: boolean;
  density: Density;
  categories: readonly Category[];
  needsLogin: ReadonlySet<string>;
  now: number;
}

/** The virtualised, keyboard-operable download grid (V1's): only the rows in view are drawn. */
export function DownloadTable({ rows, columns, widths: saved, onResize, sort, onSort, actions, busy, density, categories, needsLogin, now }: Props) {
  const parentRef = useRef<HTMLDivElement>(null);
  const headerRef = useRef<HTMLDivElement>(null);
  // The width being dragged, shown live and saved when the drag ends.
  const [preview, setPreview] = useState<{ id: ColumnId; px: number } | null>(null);
  const widths: Widths = preview ? { ...saved, [preview.id]: preview.px } : saved;
  // Every chosen column is shown; narrower than this, the table scrolls sideways in its own area.
  const minWidth = CHECK_WIDTH + columns.reduce((a, c) => a + (widths[c.id] ?? c.minWidth), 0) + COLUMN_GAP * columns.length + ROW_PADDING;
  const selection = useDownloadsStore((s) => s.selection);
  const focus = useDownloadsStore((s) => s.focus);
  const select = useDownloadsStore((s) => s.select);
  const selectAll = useDownloadsStore((s) => s.selectAll);
  const clearSelection = useDownloadsStore((s) => s.clearSelection);
  const showDetails = useUiStore((s) => s.showDetails);
  const openChooseFiles = useUiStore((s) => s.openChooseFiles);
  const byId = useMemo(() => new Map(categories.map((c) => [c.id, c])), [categories]);
  const rowHeight = density === 'compact' ? 32 : 40;
  const template = [`${String(CHECK_WIDTH)}px`, ...columns.map((c) => { const w = widths[c.id]; return w ? `${String(w)}px` : c.width; })].join(' ');
  const ids = rows.map((r) => r.id);
  // Select-all covers the rows shown (list, search and category), nothing else.
  const shownSelected = rows.reduce((n, r) => n + (selection.has(r.id) ? 1 : 0), 0);
  const allState = shownSelected === 0 ? false : shownSelected === rows.length ? true : 'mixed';

  // TanStack Virtual returns new functions each render, which the React Compiler lint flags; this component isn't memoised.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 12,
    initialRect: { width: 1000, height: 600 },
  });

  const focusedIndex = focus ? ids.indexOf(focus) : -1;
  useEffect(() => {
    if (focusedIndex >= 0) virtualizer.scrollToIndex(focusedIndex, { align: 'auto' });
  }, [focusedIndex, virtualizer]);

  const selectedRows = () => rows.filter((r) => selection.has(r.id));

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (rows.length === 0) return;
    const move = (to: number) => {
      const target = rows[Math.max(0, Math.min(rows.length - 1, to))];
      if (target) select(target.id, e.shiftKey ? 'range' : 'single', ids);
    };
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        move(focusedIndex + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        move(focusedIndex <= 0 ? 0 : focusedIndex - 1);
        break;
      case 'Home':
        e.preventDefault();
        move(0);
        break;
      case 'End':
        e.preventDefault();
        move(rows.length - 1);
        break;
      case 'a':
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          selectAll(ids);
        }
        break;
      case 'Escape':
        clearSelection();
        break;
      case 'Enter':
        if (focus) showDetails(focus);
        break;
      case ' ': {
        e.preventDefault();
        if (busy) break;
        const sel = selectedRows();
        const toResume = sel.filter((d) => d.status === 'paused').map((d) => d.id);
        const toPause = sel.filter((d) => ['downloading', 'connecting', 'queued', 'retrying', 'fetchingMetadata', 'stalled', 'seeding'].includes(d.status)).map((d) => d.id);
        if (toResume.length) void actions.resume(toResume);
        if (toPause.length) void actions.pause(toPause);
        break;
      }
      case 'Delete': {
        const sel = selectedRows();
        if (sel.length && !busy) void (e.shiftKey ? actions.removeAndDelete(sel) : actions.remove(sel));
        break;
      }
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div ref={headerRef} className="shrink-0 overflow-hidden border-b border-border bg-surface-2">
        <div role="row" className="grid items-center gap-3 px-4 text-[11px] font-medium uppercase tracking-wide text-fg-subtle" style={{ gridTemplateColumns: template, height: 30, minWidth }}>
          <div role="columnheader" aria-label="Selected" className="flex items-center">
            <SelectBox checked={allState} label="Select all shown downloads" onChange={() => { if (allState === true) clearSelection(); else selectAll(ids); }} />
          </div>
          {columns.map((c) => {
            const active = c.sort !== undefined && sort.key === c.sort;
            const content = (
              <>
                {c.label}
                {active ? sort.dir === 'asc' ? <ArrowUp className="size-3" aria-hidden /> : <ArrowDown className="size-3" aria-hidden /> : null}
              </>
            );
            return (
              <div
                key={c.id}
                role="columnheader"
                aria-label={c.label}
                aria-sort={active ? (sort.dir === 'asc' ? 'ascending' : 'descending') : undefined}
                className={cn('relative flex h-full min-w-0 items-center', c.align === 'right' && 'justify-end')}
              >
                {c.sort ? (
                  <button type="button" className="flex min-w-0 items-center gap-1 truncate uppercase hover:text-fg" onClick={() => { if (c.sort) onSort(c.sort); }}>
                    {content}
                  </button>
                ) : (
                  <span className="truncate">{content}</span>
                )}
                <ColumnResizer
                  column={c}
                  width={widths[c.id]}
                  onPreview={(px) => { setPreview(px === null ? null : { id: c.id, px }); }}
                  onCommit={(px) => {
                    setPreview(null);
                    onResize(c.id, px);
                  }}
                />
              </div>
            );
          })}
        </div>
      </div>
      <div
        ref={parentRef}
        role="grid"
        aria-label="Downloads"
        aria-multiselectable="true"
        aria-rowcount={rows.length}
        aria-activedescendant={focus ? `row-${focus}` : undefined}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onScroll={(e) => {
          if (headerRef.current) headerRef.current.scrollLeft = e.currentTarget.scrollLeft;
        }}
        className="min-h-0 flex-1 overflow-auto focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/40"
      >
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative', minWidth }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const d = rows[vi.index];
            if (!d) return null;
            const selected = selection.has(d.id);
            const ctx = { category: d.categoryId ? byId.get(d.categoryId) : undefined, now, needsLogin: needsLogin.has(d.id) };
            return (
              <ContextMenu
                key={d.id}
                items={() => buildMenu(selection.has(d.id) ? selectedRows() : [d], actions, busy, { showDetails, chooseFiles: openChooseFiles })}
                onOpenChange={(open) => {
                  if (open && !selection.has(d.id)) select(d.id, 'single', ids);
                }}
              >
                <div
                  id={`row-${d.id}`}
                  role="row"
                  aria-selected={selected}
                  aria-rowindex={vi.index + 1}
                  data-status={d.status}
                  onClick={(e) => { select(d.id, e.shiftKey ? 'range' : e.ctrlKey || e.metaKey ? 'toggle' : 'single', ids); }}
                  // V1's: a finished download opens its file, a torrent in Choose files chooses them, any other shows its details.
                  onDoubleClick={() => {
                    if (canOpenFile(d)) void actions.openFile(d);
                    else if (d.status === 'choosingFiles') openChooseFiles(d.id);
                    else showDetails(d.id);
                  }}
                  className={cn(
                    'absolute left-0 top-0 grid w-full cursor-default items-center gap-3 border-b border-border/60 px-4 text-[12.5px] text-fg-muted',
                    selected ? 'bg-row-selected' : 'hover:bg-row-hover',
                    focus === d.id && 'shadow-[inset_2px_0_0_var(--accent)]',
                  )}
                  style={{ gridTemplateColumns: template, height: vi.size, transform: `translateY(${String(vi.start)}px)` }}
                >
                  {/* The checkbox toggles this row without changing the rest of the selection. */}
                  <div role="gridcell" className="flex items-center" onClick={(e) => { e.stopPropagation(); }} onDoubleClick={(e) => { e.stopPropagation(); }}>
                    <SelectBox checked={selected} label={`Select ${displayName(d)}`} onChange={() => { select(d.id, 'toggle', ids); }} />
                  </div>
                  {columns.map((c) => (
                    <div key={c.id} role="gridcell" data-column={c.id} className={cn('flex min-w-0 items-center overflow-hidden whitespace-nowrap tabular-nums', c.align === 'right' && 'justify-end')}>
                      {c.render(d, ctx)}
                    </div>
                  ))}
                </div>
              </ContextMenu>
            );
          })}
        </div>
      </div>
    </div>
  );
}
