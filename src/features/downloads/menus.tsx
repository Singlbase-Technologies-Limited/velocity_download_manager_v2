import { ArrowDownToLine, ArrowUpToLine, ChevronDown, ChevronUp, Copy, Pause, Play, RotateCcw, Square, Trash2, UploadCloud, X } from 'lucide-react';
import type { MenuSpec } from '../../components/Menu';
import type { DownloadRecord } from '../../contract';
import { actionsForSelection } from '../../downloads/list';
import type { DownloadActions } from './useDownloadActions';

/**
 * The right-click menu for a selection (V1's menus.tsx). An action the selection doesn't allow is shown disabled.
 * Opening a file or its folder and the details come with the details panel (#61); choosing a torrent's files with #63.
 */
export function buildMenu(selection: DownloadRecord[], a: DownloadActions, busy: boolean): MenuSpec {
  const can = actionsForSelection(selection);
  const ids = selection.map((d) => d.id);
  const one = selection.length === 1 ? selection[0] : undefined;
  return [
    { label: 'Resume', icon: <Play />, disabled: busy || !can.resume, onSelect: () => void a.resume(ids) },
    { label: 'Pause', icon: <Pause />, disabled: busy || !can.pause, onSelect: () => void a.pause(ids) },
    { label: 'Cancel', icon: <Square />, disabled: busy || !can.cancel, onSelect: () => void a.cancel(ids) },
    { label: 'Retry', icon: <RotateCcw />, disabled: busy || !can.retry, onSelect: () => void a.retry(ids) },
    ...(can.stopSeeding ? (['separator', { label: 'Stop seeding', icon: <UploadCloud />, disabled: busy, onSelect: () => void a.stopSeeding(ids) }] as MenuSpec) : []),
    'separator',
    { label: 'Move to top', icon: <ArrowUpToLine />, disabled: busy || !can.move, onSelect: () => { if (one) void a.move(one.id, 'top'); } },
    { label: 'Move up', icon: <ChevronUp />, disabled: busy || !can.move, onSelect: () => { if (one) void a.move(one.id, 'up'); } },
    { label: 'Move down', icon: <ChevronDown />, disabled: busy || !can.move, onSelect: () => { if (one) void a.move(one.id, 'down'); } },
    { label: 'Move to bottom', icon: <ArrowDownToLine />, disabled: busy || !can.move, onSelect: () => { if (one) void a.move(one.id, 'bottom'); } },
    'separator',
    { label: 'Copy link', icon: <Copy />, disabled: !one, onSelect: () => { if (one) void a.copyLink(one); } },
    { label: 'Copy name', icon: <Copy />, disabled: !one, onSelect: () => { if (one) void a.copyName(one); } },
    'separator',
    { label: 'Remove from list', icon: <X />, disabled: busy || !can.remove, shortcut: 'Del', onSelect: () => void a.remove(selection) },
    { label: 'Delete files…', icon: <Trash2 />, destructive: true, disabled: busy || !can.deleteFiles, shortcut: 'Shift+Del', onSelect: () => void a.removeAndDelete(selection) },
  ];
}
