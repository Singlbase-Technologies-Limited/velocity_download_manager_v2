import { useRef } from 'react';

/**
 * Radix gives focus back to a dialog's trigger when it closes, but the app's dialogs open from app state and have
 * none, so focus would fall to the page. Spread these on the dialog's content to give focus back to whatever had it
 * when the dialog opened, if that is still on the page (#66, docs/ACCESSIBILITY.md).
 */
export function useReturnFocus() {
  const opener = useRef<HTMLElement | null>(null);
  return {
    onOpenAutoFocus: () => {
      opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    },
    onCloseAutoFocus: (event: Event) => {
      const element = opener.current;
      opener.current = null;
      if (element?.isConnected) {
        event.preventDefault();
        element.focus();
      }
    },
  };
}
