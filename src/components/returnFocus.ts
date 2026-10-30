import { useState } from 'react';

const focused = () => (document.activeElement instanceof HTMLElement ? document.activeElement : null);

/**
 * Radix gives focus back to a dialog's trigger when it closes, but the app's dialogs open from app state and have
 * none, so focus would fall to the page. Spread the result on the dialog's content to give focus back to whatever had
 * it when the dialog opened, if that is still on the page (#66, docs/ACCESSIBILITY.md).
 *
 * The opener is noted while rendering the change to `open`: once React commits it, a field with `autoFocus` has
 * already taken focus, and Radix then skips its open event.
 */
export function useReturnFocus(open: boolean) {
  const [wasOpen, setWasOpen] = useState(open);
  const [opener, setOpener] = useState(() => (open ? focused() : null));
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setOpener(focused());
  }
  return {
    onCloseAutoFocus: (event: Event) => {
      if (opener?.isConnected) {
        event.preventDefault();
        opener.focus();
      }
    },
  };
}
