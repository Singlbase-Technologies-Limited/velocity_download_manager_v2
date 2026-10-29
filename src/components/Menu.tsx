import * as ContextMenuPrimitive from '@radix-ui/react-context-menu';
import * as DropdownPrimitive from '@radix-ui/react-dropdown-menu';
import type { ReactNode } from 'react';
import { cn } from '../lib/cn';

// V1's menus (src/components/ui/menu.tsx): a right-click menu and a button's menu, with keyboard and screen-reader support.

export const menuContent = 'z-50 min-w-48 rounded-lg border border-border bg-surface p-1 text-[13px] shadow-panel';
export const menuItem =
  'flex cursor-default select-none items-center gap-2 rounded-md px-2 py-1.5 outline-none data-[disabled]:pointer-events-none data-[disabled]:opacity-40 data-[highlighted]:bg-surface-3 [&_svg]:size-4 [&_svg]:text-fg-muted';

export interface MenuEntry {
  label: string;
  icon?: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  destructive?: boolean;
  shortcut?: string;
}
export type MenuSpec = (MenuEntry | 'separator')[];

function renderItems(items: MenuSpec, Item: typeof DropdownPrimitive.Item | typeof ContextMenuPrimitive.Item, Separator: typeof DropdownPrimitive.Separator | typeof ContextMenuPrimitive.Separator) {
  return items.map((it, i) =>
    it === 'separator' ? (
      <Separator key={`s${String(i)}`} className="my-1 h-px bg-border" />
    ) : (
      <Item key={it.label} disabled={it.disabled} onSelect={it.onSelect} className={cn(menuItem, it.destructive && 'text-danger [&_svg]:text-danger')}>
        {it.icon}
        <span className="flex-1">{it.label}</span>
        {it.shortcut ? <span className="text-[11px] text-fg-subtle">{it.shortcut}</span> : null}
      </Item>
    ),
  );
}

export function DropdownMenu({ trigger, items, align = 'end' }: { trigger: ReactNode; items: MenuSpec; align?: 'start' | 'end' }) {
  return (
    <DropdownPrimitive.Root>
      <DropdownPrimitive.Trigger asChild>{trigger}</DropdownPrimitive.Trigger>
      <DropdownPrimitive.Portal>
        <DropdownPrimitive.Content align={align} sideOffset={4} className={menuContent}>
          {renderItems(items, DropdownPrimitive.Item, DropdownPrimitive.Separator)}
        </DropdownPrimitive.Content>
      </DropdownPrimitive.Portal>
    </DropdownPrimitive.Root>
  );
}

/** A right-click menu over `children`. `items` is read when it opens, so it reflects the selection then. */
export function ContextMenu({ children, items, onOpenChange }: { children: ReactNode; items: () => MenuSpec; onOpenChange?: (open: boolean) => void }) {
  return (
    <ContextMenuPrimitive.Root onOpenChange={onOpenChange}>
      <ContextMenuPrimitive.Trigger asChild>{children}</ContextMenuPrimitive.Trigger>
      <ContextMenuPrimitive.Portal>
        <ContextMenuPrimitive.Content className={menuContent}>{renderItems(items(), ContextMenuPrimitive.Item, ContextMenuPrimitive.Separator)}</ContextMenuPrimitive.Content>
      </ContextMenuPrimitive.Portal>
    </ContextMenuPrimitive.Root>
  );
}
