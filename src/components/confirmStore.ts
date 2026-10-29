import { create } from 'zustand';
import type { ConfirmOptions } from './ConfirmDialog';

// V1's confirm store: one question at a time, answered through ConfirmHost.
interface ConfirmState {
  options: ConfirmOptions | null;
  resolve: ((ok: boolean) => void) | null;
  ask: (options: ConfirmOptions) => Promise<boolean>;
  settle: (ok: boolean) => void;
}

export const useConfirmStore = create<ConfirmState>((set, get) => ({
  options: null,
  resolve: null,
  ask: (options) =>
    new Promise<boolean>((resolve) => {
      // A new question replaces one still open, which counts as cancelled.
      get().resolve?.(false);
      set({ options, resolve });
    }),
  settle: (ok) => {
    get().resolve?.(ok);
    set({ options: null, resolve: null });
  },
}));

/** Asks the user to confirm. Resolves to `true` only when they confirm. */
export function confirmAction(options: ConfirmOptions): Promise<boolean> {
  return useConfirmStore.getState().ask(options);
}
