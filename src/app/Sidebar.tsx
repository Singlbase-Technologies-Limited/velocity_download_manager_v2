import { AlertCircle, ArrowDownToLine, ArrowUpFromLine, CheckCircle2, Clock3, FolderCog, History, Layers, PanelLeftClose, PanelLeftOpen, Plus, type LucideIcon } from 'lucide-react';
import { useMemo, type ReactNode } from 'react';
import { BrandIcon } from '../components/BrandIcon';
import { CategoryGlyph } from '../components/CategoryGlyph';
import type { AppInfo } from '../contract';
import { countByView, type ListView } from '../downloads/views';
import { cn } from '../lib/cn';
import { useCategories } from '../lib/queries';
import { useDownloadsStore } from '../stores/downloads';
import { useUiStore } from '../stores/ui';

const LIST_ITEMS: { view: ListView; label: string; icon: LucideIcon }[] = [
  { view: 'all', label: 'All Downloads', icon: Layers },
  { view: 'downloading', label: 'Downloading', icon: ArrowDownToLine },
  { view: 'queued', label: 'Queued', icon: Clock3 },
  { view: 'completed', label: 'Completed', icon: CheckCircle2 },
  { view: 'seeding', label: 'Seeding', icon: ArrowUpFromLine },
  { view: 'failed', label: 'Failed', icon: AlertCircle },
];

function NavButton({
  active,
  icon: Icon,
  label,
  count,
  onClick,
  compact,
  children,
}: {
  active: boolean;
  icon?: LucideIcon;
  label: string;
  count?: number;
  onClick: () => void;
  compact: boolean;
  children?: ReactNode;
}) {
  const counted = count !== undefined && count > 0;
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? 'page' : undefined}
      // Icons only: the label stays the accessible name and shows as a tooltip.
      title={compact ? label : undefined}
      className={cn(
        'group flex h-8 w-full items-center gap-2.5 rounded-md text-left text-[13px] transition-colors',
        compact ? 'relative justify-center px-0' : 'px-2.5',
        active ? 'bg-surface font-medium text-fg shadow-xs ring-1 ring-border' : 'text-fg-muted hover:bg-surface-3 hover:text-fg',
      )}
    >
      {Icon ? <Icon className={cn('size-4 shrink-0', active ? 'text-accent' : 'text-fg-subtle group-hover:text-fg-muted')} aria-hidden /> : children}
      <span className={compact ? 'sr-only' : 'min-w-0 flex-1 truncate'}>{label}</span>
      {counted && !compact ? <span className="text-[11px] tabular-nums text-fg-subtle">{count}</span> : null}
      {counted && compact ? <span className="sr-only">, {count}</span> : null}
    </button>
  );
}

const groupLabel = 'px-2.5 pb-1 text-[10px] font-semibold uppercase tracking-wider text-fg-subtle';

/**
 * The navigation (V1's Sidebar). Its width comes from the shell (`--sidebar-w`); `compact` shows icons
 * only, because the user collapsed it or the window is too narrow. Settings come with #30.
 */
export function Sidebar({ app, compact, autoCompact }: { app: AppInfo; compact: boolean; autoCompact: boolean }) {
  const view = useUiStore((s) => s.view);
  const categoryFilter = useUiStore((s) => s.categoryFilter);
  const setView = useUiStore((s) => s.setView);
  const setCategoryFilter = useUiStore((s) => s.setCategoryFilter);
  const collapsed = useUiStore((s) => s.layout.sidebarCollapsed);
  const setLayout = useUiStore((s) => s.setLayout);
  const openAdd = useUiStore((s) => s.openAdd);
  const downloads = useDownloadsStore((s) => s.snapshot?.downloads);
  const { data: categories } = useCategories();
  const counts = useMemo(() => countByView(downloads ?? []), [downloads]);
  const perCategory = useMemo(() => {
    const m = new Map<string, number>();
    for (const d of downloads ?? []) if (d.categoryId) m.set(d.categoryId, (m.get(d.categoryId) ?? 0) + 1);
    return m;
  }, [downloads]);

  return (
    <nav id="app-sidebar" aria-label="Primary" data-mode={compact ? 'rail' : 'full'} className="flex shrink-0 flex-col overflow-hidden bg-sidebar" style={{ width: 'var(--sidebar-w)' }}>
      <div className={cn('flex h-12 shrink-0 items-center gap-2.5', compact ? 'justify-center' : 'px-4')}>
        <span className="flex size-7 shrink-0 drop-shadow-sm" title={compact ? app.name : undefined}>
          <BrandIcon className="size-7" />
        </span>
        <div className={compact ? 'sr-only' : 'min-w-0'}>
          <p className="truncate text-[13px] font-semibold leading-tight">Velocity</p>
          <p className="truncate text-[10px] leading-tight text-fg-subtle">Download Manager</p>
        </div>
      </div>
      <div className={cn('flex-1 space-y-5 overflow-y-auto overflow-x-hidden py-2', compact ? 'px-2' : 'px-2.5')}>
        <button
          type="button"
          onClick={() => { openAdd(); }}
          title={compact ? 'Add download (Ctrl+N)' : undefined}
          aria-keyshortcuts="Control+N"
          className={cn(
            'flex h-8 w-full items-center gap-2 rounded-md bg-accent text-[13px] font-medium text-accent-fg shadow-sm hover:bg-accent-strong',
            compact ? 'justify-center' : 'px-2.5',
          )}
        >
          <Plus className="size-4 shrink-0" aria-hidden />
          <span className={compact ? 'sr-only' : 'min-w-0 flex-1 truncate text-left'}>Add download</span>
        </button>
        <div className="space-y-0.5">
          {LIST_ITEMS.map((it) => (
            <NavButton key={it.view} compact={compact} active={view === it.view && categoryFilter === null} icon={it.icon} label={it.label} count={counts[it.view]} onClick={() => { setView(it.view); }} />
          ))}
        </div>
        {categories && categories.length > 0 ? (
          <div className={cn(compact && 'border-t border-border pt-3')}>
            <p className={compact ? 'sr-only' : groupLabel}>Categories</p>
            <div className="space-y-0.5">
              {categories.map((c) => (
                <NavButton key={c.id} compact={compact} active={categoryFilter === c.id} label={c.name} count={perCategory.get(c.id)} onClick={() => { setCategoryFilter(c.id); }}>
                  <CategoryGlyph icon={c.icon} color={c.color} className="size-4 shrink-0" />
                </NavButton>
              ))}
            </div>
          </div>
        ) : null}
        <div className={cn('space-y-0.5', compact && 'border-t border-border pt-3')}>
          <p className={compact ? 'sr-only' : groupLabel}>Library</p>
          <NavButton compact={compact} active={view === 'history'} icon={History} label="History" onClick={() => { setView('history'); }} />
          <NavButton compact={compact} active={view === 'categories'} icon={FolderCog} label="Categories" onClick={() => { setView('categories'); }} />
        </div>
      </div>
      <div className={cn('flex shrink-0 items-center gap-2 border-t border-border py-1.5 text-[10px] text-fg-subtle', compact ? 'justify-center' : 'pl-4 pr-1.5')}>
        {compact ? null : <span className="min-w-0 flex-1 truncate">v{app.version}</span>}
        {autoCompact ? null : (
          <button
            type="button"
            onClick={() => { setLayout({ sidebarCollapsed: !collapsed }); }}
            aria-label={collapsed ? 'Expand navigation' : 'Collapse navigation'}
            title={collapsed ? 'Expand navigation' : 'Collapse navigation'}
            className="flex size-7 shrink-0 items-center justify-center rounded-md text-fg-subtle hover:bg-surface-3 hover:text-fg"
          >
            {collapsed ? <PanelLeftOpen className="size-4" aria-hidden /> : <PanelLeftClose className="size-4" aria-hidden />}
          </button>
        )}
      </div>
    </nav>
  );
}
