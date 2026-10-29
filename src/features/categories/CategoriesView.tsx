import { useQueryClient } from '@tanstack/react-query';
import { FolderOpen, Pencil, Plus, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { Badge } from '../../components/Badge';
import { Button } from '../../components/Button';
import { CategoryGlyph } from '../../components/CategoryGlyph';
import { confirmAction } from '../../components/confirmStore';
import { EmptyState } from '../../components/EmptyState';
import { Spinner } from '../../components/Spinner';
import type { Category } from '../../contract';
import { api } from '../../lib/api';
import { toApiError } from '../../lib/errors';
import { keys, useCategories } from '../../lib/queries';
import { CategoryEditor } from './CategoryEditor';

/** The categories and what each one catches (V1's CategoriesView). The rules are in docs/FILE_NAMING.md. */
export function CategoriesView() {
  const qc = useQueryClient();
  const { data: categories, isLoading, isError, error } = useCategories();
  const [editing, setEditing] = useState<Category | 'new' | null>(null);

  const remove = async (c: Category) => {
    const ok = await confirmAction({
      title: `Delete the “${c.name}” category?`,
      description: 'Its downloads are left without a category. No file is moved or deleted.',
      confirmLabel: 'Delete category',
      destructive: true,
    });
    if (!ok) return;
    try {
      await api.deleteCategory(c.id);
      toast.success('Category deleted');
    } catch (e) {
      toast.error(toApiError(e).message);
    }
    await qc.invalidateQueries({ queryKey: keys.categories });
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border bg-surface px-3">
        <h1 className="px-1 text-[13px] font-semibold">Categories</h1>
        <span className="truncate text-xs text-fg-subtle">New downloads are sorted by file extension while automatic categories are on.</span>
        <div className="flex-1" />
        <Button variant="primary" onClick={() => { setEditing('new'); }}>
          <Plus aria-hidden /> New category
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-4">
        {isLoading ? (
          <div className="flex items-center gap-2 text-fg-muted" role="status">
            <Spinner /> Loading…
          </div>
        ) : isError ? (
          <EmptyState icon={FolderOpen} title="The categories couldn't be loaded" description={toApiError(error).message} />
        ) : (
          <ul className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-3">
            {(categories ?? []).map((c) => (
              <li key={c.id} className="rounded-lg border border-border bg-surface p-4 shadow-xs">
                <div className="flex items-center gap-2.5">
                  <span className="flex size-8 items-center justify-center rounded-md" style={{ backgroundColor: `${c.color}1f` }}>
                    <CategoryGlyph icon={c.icon} color={c.color} />
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-[13px] font-medium">{c.name}</p>
                    <p className="truncate text-[11px] text-fg-subtle" title={c.directory ?? undefined}>
                      {c.directory ?? 'Default folder'}
                    </p>
                  </div>
                  {c.builtin ? <Badge>Built-in</Badge> : null}
                </div>
                <p className="mt-3 line-clamp-2 min-h-8 text-xs text-fg-muted">
                  {c.extensions.length ? c.extensions.map((e) => `.${e}`).join(' ') : 'No extensions: only downloads given this category'}
                </p>
                <div className="mt-3 flex gap-1">
                  <Button size="sm" variant="ghost" onClick={() => { setEditing(c); }}>
                    <Pencil aria-hidden /> Edit
                  </Button>
                  {!c.builtin ? (
                    <Button size="sm" variant="ghost" onClick={() => void remove(c)}>
                      <Trash2 aria-hidden /> Delete
                    </Button>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
      {editing ? <CategoryEditor category={editing === 'new' ? null : editing} onClose={() => { setEditing(null); }} /> : null}
    </div>
  );
}
