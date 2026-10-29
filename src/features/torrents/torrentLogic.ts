// Pure torrent rules for the Add dialog, the file chooser and the details panel (V1's torrentLogic, #63).
import type { ContentFile, SeedPolicy, TorrentVersion, TrackerState, TrackerStatus } from '../../contract';
import { linkKind } from '../add/addLogic';

export interface FileTreeNode {
  name: string;
  /** Path of this node, `/`-separated. Unique within the tree. */
  path: string;
  isFile: boolean;
  size: number;
  /** The indexes of every file at or below this node. */
  fileIndexes: number[];
  children: FileTreeNode[];
}

/** Nests a torrent's flat file list into folders. Folders sort before files, then by name. */
export function buildFileTree(files: readonly ContentFile[]): FileTreeNode {
  const root: FileTreeNode = { name: '', path: '', isFile: false, size: 0, fileIndexes: [], children: [] };
  for (const f of files) {
    const parts = f.path.split('/').filter(Boolean);
    let node = root;
    node.size += f.length;
    node.fileIndexes.push(f.index);
    parts.forEach((part, i) => {
      const isFile = i === parts.length - 1;
      const path = parts.slice(0, i + 1).join('/');
      let child = node.children.find((c) => c.name === part && c.isFile === isFile);
      if (!child) {
        child = { name: part, path, isFile, size: 0, fileIndexes: [], children: [] };
        node.children.push(child);
      }
      child.size += f.length;
      child.fileIndexes.push(f.index);
      node = child;
    });
  }
  const sort = (n: FileTreeNode) => {
    n.children.sort((a, b) => (a.isFile === b.isFile ? a.name.localeCompare(b.name) : a.isFile ? 1 : -1));
    n.children.forEach(sort);
  };
  sort(root);
  return root;
}

export type FolderState = 'all' | 'some' | 'none';

export function folderState(node: FileTreeNode, selected: ReadonlySet<number>): FolderState {
  const n = node.fileIndexes.filter((i) => selected.has(i)).length;
  return n === 0 ? 'none' : n === node.fileIndexes.length ? 'all' : 'some';
}

/** Toggles a folder (or file): fully selected becomes cleared, anything else becomes fully selected. */
export function toggleFolder(selected: ReadonlySet<number>, node: FileTreeNode): Set<number> {
  const next = new Set(selected);
  const clear = folderState(node, selected) === 'all';
  for (const i of node.fileIndexes) {
    if (clear) next.delete(i);
    else next.add(i);
  }
  return next;
}

export function selectedSize(files: readonly ContentFile[], selected: ReadonlySet<number>): number {
  return files.reduce((sum, f) => (selected.has(f.index) ? sum + f.length : sum), 0);
}

/** The files to send, in order: `null` when every file is chosen; an error for none. */
export function fileChoice(files: readonly ContentFile[], selected: ReadonlySet<number>): { files: number[] | null; error: string | null } {
  const chosen = files.map((f) => f.index).filter((i) => selected.has(i)).sort((a, b) => a - b);
  if (chosen.length === 0) return { files: null, error: 'Choose at least one file.' };
  return { files: chosen.length === files.length ? null : chosen, error: null };
}

export function sameSelection(a: ReadonlySet<number>, b: ReadonlySet<number>): boolean {
  return a.size === b.size && [...a].every((i) => b.has(i));
}

/** Text pasted outside a text field opens the Add dialog when every part of it is a magnet or `.torrent` link. */
export function torrentLinksInPaste(text: string): string[] {
  const parts = text.split(/\s+/).filter(Boolean);
  if (parts.length === 0 || parts.length > 500) return [];
  const torrentLink = (p: string) => {
    const kind = linkKind(p);
    return kind === 'magnet' || kind === 'torrentLink';
  };
  if (!parts.every(torrentLink)) return [];
  return [...new Set(parts)];
}

function duration(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) return `${String(m)} min`;
  return m === 0 ? `${String(h)} h` : `${String(h)} h ${String(m)} min`;
}

export function seedingSummary(p: SeedPolicy): string {
  switch (p.mode) {
    case 'never':
      return 'Stops when the download finishes';
    case 'time':
      return `Seeds for ${duration(p.minutes)}`;
    case 'ratio':
      return `Seeds until the share ratio reaches ${String(p.ratio)}`;
    case 'timeOrRatio':
      return `Seeds for ${duration(p.minutes)} or until the share ratio reaches ${String(p.ratio)}`;
    case 'unlimited':
      return 'Seeds until you stop it';
  }
}

export const SEED_MODE_LABEL: Record<SeedPolicy['mode'], string> = {
  never: 'Don’t seed',
  time: 'For a time',
  ratio: 'Until a share ratio',
  timeOrRatio: 'Time or ratio, whichever first',
  unlimited: 'Until I stop it',
};

export const VERSION_LABEL: Record<TorrentVersion, string> = {
  v1: 'BitTorrent v1',
  v2: 'BitTorrent v2',
  hybrid: 'Hybrid (v1 and v2)',
};

export const TRACKER_STATE: Record<TrackerState, string> = {
  working: 'Working',
  updating: 'Contacting',
  error: 'Error',
  notContacted: 'Not contacted yet',
};

/** One line under a tracker: its message, scrape counts and next announce, only as reported. */
export function trackerSummary(t: TrackerStatus): string {
  const parts: string[] = [];
  if (t.message) parts.push(t.message);
  if (t.seeds !== null || t.peers !== null) parts.push(`${String(t.seeds ?? 0)} seeds, ${String(t.peers ?? 0)} peers`);
  if (t.state === 'error' && t.fails > 1) parts.push(`failed ${String(t.fails)} times`);
  if (t.nextAnnounceSecs !== null) {
    const s = t.nextAnnounceSecs;
    parts.push(`next announce in ${s < 90 ? `${String(s)} s` : `${String(Math.round(s / 60))} min`}`);
  }
  return parts.join(' · ');
}
