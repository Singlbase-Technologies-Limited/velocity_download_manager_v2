import { ArrowDownToLine, ArrowUpToLine, ChevronDown, ChevronUp, Copy, FileSearch, FolderOpen, Info, ListChecks, Pause, Play, RotateCcw, Square, Trash2, UploadCloud, X } from 'lucide-react';
import type { MenuSpec } from '../../components/Menu';
import type { DownloadRecord } from '../../contract';
import { actionsForSelection, canOpenFile } from '../../downloads/list';
import type { DownloadActions } from './useDownloadActions';

/**
 * The right-click menu for a selection (V1's menus.tsx). An action the selection doesn't allow is shown disabled.
 */
export function buildMenu(selection: DownloadRecord[], a: DownloadActions, busy: boolean, ui: { showDetails: (id: string) => void; chooseFiles: (id: string) => void }): MenuSpec {
  const can = actionsForSelection(selection);
  const ids = selection.map((d) => d.id);
  const one = selection.length === 1 ? selection[0] : undefined;
  return [
    { label: 'Open file', icon: <FileSearch />, disabled: !one || !canOpenFile(one), onSelect: () => { if (one) void a.openFile(one); } },
    { label: 'Open folder', icon: <FolderOpen />, disabled: !one, onSelect: () => { if (one) void a.openFolder(one.id); } },
    ...(one?.status === 'choosingFiles' ? ([{ label: 'Choose files…', icon: <ListChecks />, onSelect: () => { ui.chooseFiles(one.id); } }] as MenuSpec) : []),
    'separator',
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
    { label: 'Details', icon: <Info />, disabled: !one, onSelect: () => { if (one) ui.showDetails(one.id); } },
    'separator',
    { label: 'Remove from list', icon: <X />, disabled: busy || !can.remove, shortcut: 'Del', onSelect: () => void a.remove(selection) },
    { label: 'Delete files…', icon: <Trash2 />, destructive: true, disabled: busy || !can.deleteFiles, shortcut: 'Shift+Del', onSelect: () => void a.removeAndDelete(selection) },
  ];
}
