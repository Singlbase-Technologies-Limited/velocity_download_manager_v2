import * as AlertDialogPrimitive from '@radix-ui/react-alert-dialog';
import type { ReactNode } from 'react';
import { cn } from '../lib/cn';
import { Button } from './Button';
import { contentClass, overlayClass } from './dialogStyles';
import { useReturnFocus } from './returnFocus';

export interface ConfirmOptions {
  title: string;
  description: ReactNode;
  confirmLabel: string;
  /** The label of the button that changes nothing; "Cancel" unless given. */
  cancelLabel?: string;
  destructive?: boolean;
}

/** Asks before an action that can't be undone (V1's ConfirmDialog). */
export function ConfirmDialog({
  open,
  options,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  options: ConfirmOptions | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const returnFocus = useReturnFocus(open);
  return (
    <AlertDialogPrimitive.Root open={open} onOpenChange={(o) => {
        if (!o) onCancel();
      }}>
      <AlertDialogPrimitive.Portal>
        <AlertDialogPrimitive.Overlay className={overlayClass} />
        <AlertDialogPrimitive.Content className={cn(contentClass, 'max-w-md p-5')} {...returnFocus}>
          <AlertDialogPrimitive.Title className="text-[15px] font-semibold">{options?.title}</AlertDialogPrimitive.Title>
          <AlertDialogPrimitive.Description asChild>
            <div className="mt-2 text-[13px] leading-relaxed text-fg-muted">{options?.description}</div>
          </AlertDialogPrimitive.Description>
          <div className="mt-5 flex justify-end gap-2">
            <AlertDialogPrimitive.Cancel asChild>
              <Button variant="secondary">{options?.cancelLabel ?? 'Cancel'}</Button>
            </AlertDialogPrimitive.Cancel>
            <AlertDialogPrimitive.Action asChild>
              <Button variant={options?.destructive ? 'danger' : 'primary'} onClick={onConfirm}>
                {options?.confirmLabel}
              </Button>
            </AlertDialogPrimitive.Action>
          </div>
        </AlertDialogPrimitive.Content>
      </AlertDialogPrimitive.Portal>
    </AlertDialogPrimitive.Root>
  );
}
