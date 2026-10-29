// The typed bridge to the native commands (#29, docs/CONTRACTS.md). Every answer is checked with the
// strict schemas in contract.ts before the UI uses it, and every failure becomes an ApiError.
import { invoke, type InvokeArgs } from '@tauri-apps/api/core';
import { z } from 'zod';
import {
  AppearanceSettingsSchema,
  CategorySchema,
  DownloadsSnapshotSchema,
  parseAppInfo,
  ShellSettingsSchema,
  type AppearanceSettings,
  type Category,
} from '../contract';
import { ApiError, ERROR_MESSAGES, toApiError } from './errors';

/** The fields a category is created or edited with. */
export type CategoryInput = Omit<Category, 'id' | 'builtin'>;

async function call<S extends z.ZodType>(command: string, args: InvokeArgs | undefined, schema: S): Promise<z.infer<S>> {
  let value: unknown;
  try {
    value = await invoke(command, args);
  } catch (e) {
    throw toApiError(e);
  }
  const parsed = schema.safeParse(value);
  if (!parsed.success) {
    throw new ApiError('INTERNAL', ERROR_MESSAGES.INTERNAL, `${command} answered outside the contract: ${parsed.error.message}`);
  }
  return parsed.data;
}

const nothing = z.null();

/** Whether this window runs inside the desktop app, where the commands exist (Tauri's IPC is there). */
export const isDesktop = (): boolean => '__TAURI_INTERNALS__' in window;

export const api = {
  /** The app's name and version; refuses a native side built for another contract, saying so. */
  appInfo: async () => {
    let value: unknown;
    try {
      value = await invoke('app_info');
    } catch (e) {
      throw toApiError(e);
    }
    try {
      return parseAppInfo(value);
    } catch (e) {
      throw new ApiError('INTERNAL', e instanceof Error ? e.message : String(e));
    }
  },
  shellSettings: () => call('get_shell_settings', undefined, ShellSettingsSchema),
  setAppearance: (appearance: AppearanceSettings) => call('set_appearance', { appearance }, AppearanceSettingsSchema),
  completeOnboarding: (directory: string) => call('complete_onboarding', { directory }, ShellSettingsSchema),
  /** The system's folder chooser; `null` when the user cancels. */
  chooseDirectory: (initial: string | null) => call('choose_directory', { initial }, z.string().min(1).nullable()),
  quit: () => call('quit_app', undefined, nothing),
  snapshot: () => call('get_snapshot', undefined, DownloadsSnapshotSchema),
  categories: () => call('list_categories', undefined, z.array(CategorySchema)),
  createCategory: (input: CategoryInput) => call('create_category', { input }, CategorySchema),
  updateCategory: (id: string, input: CategoryInput) => call('update_category', { id, input }, CategorySchema),
  deleteCategory: (id: string) => call('delete_category', { id }, nothing),
};
