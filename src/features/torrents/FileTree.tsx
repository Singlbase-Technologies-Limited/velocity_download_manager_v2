import { ChevronDown, ChevronRight, File, Folder } from 'lucide-react';
import { useMemo, useState } from 'react';
import { SelectBox } from '../../components/SelectBox';
import type { ContentFile } from '../../contract';
import { formatBytes } from '../../lib/format';
import { buildFileTree, folderState, toggleFolder, type FileTreeNode } from './torrentLogic';

/**
 * A torrent's files as a tree of checkboxes (V1's FileTree, #63). Names come from untrusted torrent metadata, so they
 * are shown as plain text only. Large torrents start with the deeper folders closed.
 */
export function FileTree({ files, selected, onChange, disabled }: { files: readonly ContentFile[]; selected: ReadonlySet<number>; onChange: (next: Set<number>) => void; disabled?: boolean }) {
  const root = useMemo(() => buildFileTree(files), [files]);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => (files.length > 200 ? collectFolders(root, 2) : new Set()));
  const toggleOpen = (path: string) => {
    setCollapsed((c) => {
      const n = new Set(c);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });
  };

  const rows: { node: FileTreeNode; depth: number }[] = [];
  const walk = (n: FileTreeNode, depth: number) => {
    for (const c of n.children) {
      rows.push({ node: c, depth });
      if (!c.isFile && !collapsed.has(c.path)) walk(c, depth + 1);
    }
  };
  walk(root, 0);

  return (
    <div role="tree" aria-label="Files in this torrent" className="max-h-64 overflow-y-auto rounded-md border border-border text-xs">
      {rows.map(({ node, depth }) => {
        const state = folderState(node, selected);
        return (
          <div
            key={`${node.isFile ? 'f' : 'd'}:${node.path}`}
            role="treeitem"
            aria-level={depth + 1}
            aria-selected={state === 'all'}
            aria-expanded={node.isFile ? undefined : !collapsed.has(node.path)}
            className="flex h-7 items-center gap-1.5 border-b border-border/60 pr-3 last:border-b-0 hover:bg-surface-2"
            style={{ paddingLeft: 8 + depth * 16 }}
          >
            {node.isFile ? (
              <span className="size-3.5" />
            ) : (
              <button type="button" aria-label={collapsed.has(node.path) ? `Expand ${node.name}` : `Collapse ${node.name}`} onClick={() => { toggleOpen(node.path); }} className="text-fg-subtle hover:text-fg">
                {collapsed.has(node.path) ? <ChevronRight className="size-3.5" /> : <ChevronDown className="size-3.5" />}
              </button>
            )}
            <SelectBox checked={state === 'all' ? true : state === 'some' ? 'mixed' : false} label={`Download ${node.name}`} disabled={disabled} onChange={() => { onChange(toggleFolder(selected, node)); }} />
            {node.isFile ? <File className="size-3.5 shrink-0 text-fg-subtle" aria-hidden /> : <Folder className="size-3.5 shrink-0 text-fg-subtle" aria-hidden />}
            <span className="min-w-0 flex-1 truncate" title={node.path}>
              {node.name}
            </span>
            <span className="shrink-0 tabular-nums text-fg-muted">{formatBytes(node.size)}</span>
          </div>
        );
      })}
    </div>
  );
}

function collectFolders(n: FileTreeNode, fromDepth: number, depth = 0, out = new Set<string>()): Set<string> {
  for (const c of n.children) {
    if (!c.isFile) {
      if (depth + 1 >= fromDepth) out.add(c.path);
      collectFolders(c, fromDepth, depth + 1, out);
    }
  }
  return out;
}
