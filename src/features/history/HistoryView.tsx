import { keepPreviousData, useQuery, useQueryClient } from '@tanstack/react-query';
import { History as HistoryIcon, RotateCcw, Search, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { Badge } from '../../components/Badge';
import { Button } from '../../components/Button';
import { confirmAction } from '../../components/confirmStore';
import { EmptyState } from '../../components/EmptyState';
import { Input } from '../../components/Input';
import { Select } from '../../components/Select';
import { SelectBox } from '../../components/SelectBox';
import { Spinner } from '../../components/Spinner';
import type { HistoryEntry, HistoryQuery, HistoryStatus } from '../../contract';
import { STATUS_LABEL, STATUS_TONE } from '../../downloads/list';
import { api } from '../../lib/api';
import { toApiError } from '../../lib/errors';
import { formatBytes, formatShortDate } from '../../lib/format';
import { useUiStore } from '../../stores/ui';

const PAGE = 100;
const ANY = '';
const entries = (n: number) => (n === 1 ? '1 entry' : `${String(n)} entries`);
const nameOf = (e: HistoryEntry) => e.fileName ?? e.sourceUrl.split('/').filter(Boolean).pop() ?? e.sourceUrl;

/**
 * The history (#32, V1's HistoryView): every finished download, searched, filtered by how it ended, and paged. Taking
 * entries out of it never deletes a file, and a download still in the list stays there.
 */
export function HistoryView() {
  const qc = useQueryClient();
  const openAdd = useUiStore((s) => s.openAdd);
  const [search, setSearch] = useState('');
  const [debounced, setDebounced] = useState('');
  const [status, setStatus] = useState<HistoryStatus | typeof ANY>(ANY);
  const [page, setPage] = useState(0);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const t = setTimeout(() => {
      setDebounced(search.trim());
      setPage(0);
    }, 250);
    return () => { clearTimeout(t); };
  }, [search]);

  const query: HistoryQuery = { search: debounced || null, status: status === ANY ? null : status, limit: PAGE, offset: page * PAGE };
  const history = useQuery({ queryKey: ['history', query], queryFn: () => api.history(query), placeholderData: keepPreviousData, retry: false });
  const refresh = () => qc.invalidateQueries({ queryKey: ['history'] });

  const send = async (fn: () => Promise<string>) => {
    setBusy(true);
    try {
      toast.success(await fn());
      setSelected(new Set());
      void refresh();
    } catch (e) {
      toast.error(toApiError(e).message);
    } finally {
      setBusy(false);
    }
  };
  const remove = () =>
    send(async () => {
      const ids = [...selected];
      await api.deleteHistoryEntries(ids);
      return `Removed ${entries(ids.length)} from the history. Files on disk were not touched.`;
    });
  const clearAll = async () => {
    const ok = await confirmAction({
      title: 'Clear the history?',
      description: 'Every entry leaves the history. Downloads still in the list stay there. Downloaded files are not deleted.',
      confirmLabel: 'Clear history',
      destructive: true,
    });
    if (ok) await send(async () => `Cleared ${entries(await api.clearHistory())} from the history`);
  };

  const shown = history.data?.entries ?? [];
  const total = history.data?.total ?? 0;
  const allSelected = shown.length > 0 && shown.every((e) => selected.has(e.id));
  const someSelected = shown.some((e) => selected.has(e.id));
  const toggle = (id: string) => {
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border bg-surface px-3" role="toolbar" aria-label="History actions">
        <h1 className="px-1 text-[13px] font-semibold">History</h1>
        <span className="text-xs tabular-nums text-fg-subtle">{entries(total)}</span>
        <div className="flex-1" />
        <Select
          aria-label="Filter by status"
          className="w-36"
          value={status}
          onChange={(e) => {
            setStatus(e.target.value as HistoryStatus | typeof ANY);
            setPage(0);
          }}
        >
          <option value={ANY}>Any status</option>
          <option value="completed">Completed</option>
          <option value="failed">Failed</option>
          <option value="cancelled">Cancelled</option>
        </Select>
        <div className="relative w-64 min-w-28 shrink">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-fg-subtle" aria-hidden />
          <Input type="search" value={search} onChange={(e) => { setSearch(e.target.value); }} placeholder="Search name, link or info hash" aria-label="Search history" className="pl-8" />
        </div>
        <Button variant="ghost" size="sm" disabled={selected.size === 0 || busy} onClick={() => void remove()}>
          <Trash2 /> Remove selected
        </Button>
        <Button variant="ghost" size="sm" disabled={total === 0 || busy} onClick={() => void clearAll()}>
          Clear history
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto bg-surface">
        {history.isPending ? (
          <div className="flex h-full items-center justify-center gap-2 text-fg-muted">
            <Spinner /> Loading the history…
          </div>
        ) : history.isError ? (
          <EmptyState icon={HistoryIcon} title="The history couldn’t be read" description={toApiError(history.error).message} action={<Button size="sm" onClick={() => void history.refetch()}>Try again</Button>} />
        ) : shown.length === 0 ? (
          <EmptyState
            icon={debounced ? Search : HistoryIcon}
            title={debounced ? 'No matches' : 'No history yet'}
            description={debounced ? `Nothing in the history matches “${debounced}”.` : 'Completed, failed and cancelled downloads are kept here, even after they leave the list.'}
          />
        ) : (
          <table aria-label="History" className="w-full table-fixed text-[12.5px]">
            <thead className="sticky top-0 bg-surface-2 text-left text-[11px] font-medium uppercase tracking-wide text-fg-subtle">
              <tr className="h-[30px]">
                <th className="w-10 px-4">
                  <SelectBox checked={allSelected ? true : someSelected ? 'mixed' : false} label="Select every entry shown" onChange={() => { setSelected(allSelected ? new Set() : new Set(shown.map((e) => e.id))); }} />
                </th>
                <th className="px-2">Name</th>
                <th className="w-24 px-2 text-right">Size</th>
                <th className="w-40 px-2">Status</th>
                <th className="w-32 px-2">Finished</th>
                <th className="w-24 px-2">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {shown.map((e) => (
                <tr key={e.id} className="h-10 border-b border-border/60 text-fg-muted hover:bg-row-hover">
                  <td className="px-4">
                    <SelectBox checked={selected.has(e.id)} label={`Select ${nameOf(e)}`} onChange={() => { toggle(e.id); }} />
                  </td>
                  <td className="px-2">
                    <p className="truncate font-medium text-fg" title={nameOf(e)}>
                      {nameOf(e)}
                    </p>
                    <p className="truncate text-[11px] text-fg-subtle" title={e.sourceUrl}>
                      {e.errorMessage ?? e.sourceUrl}
                    </p>
                  </td>
                  <td className="px-2 text-right tabular-nums">{formatBytes(e.totalBytes)}</td>
                  <td className="px-2">
                    <Badge tone={STATUS_TONE[e.status]}>{STATUS_LABEL[e.status]}</Badge>
                    {e.inList ? <span className="ml-1.5 text-[11px] text-fg-subtle">In the list</span> : null}
                  </td>
                  <td className="px-2">{formatShortDate(e.completedAtMs ?? e.createdAtMs)}</td>
                  <td className="px-2 text-right">
                    <Button size="sm" variant="ghost" aria-label={`Download ${nameOf(e)} again`} title="Download again" onClick={() => { openAdd({ links: e.sourceUrl }); }}>
                      <RotateCcw /> Again
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {total > PAGE ? (
        <div className="flex h-10 shrink-0 items-center justify-end gap-2 border-t border-border px-4 text-xs text-fg-muted">
          <span>{`${String(page * PAGE + 1)}–${String(Math.min(total, (page + 1) * PAGE))} of ${String(total)}`}</span>
          <Button size="sm" variant="ghost" aria-label="Previous page" disabled={page === 0} onClick={() => { setPage((p) => p - 1); }}>
            Previous
          </Button>
          <Button size="sm" variant="ghost" aria-label="Next page" disabled={(page + 1) * PAGE >= total} onClick={() => { setPage((p) => p + 1); }}>
            Next
          </Button>
        </div>
      ) : null}
    </div>
  );
}
