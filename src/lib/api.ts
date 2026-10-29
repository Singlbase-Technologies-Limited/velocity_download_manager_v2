// The typed bridge to the native commands (#29, docs/CONTRACTS.md). Every answer is checked with the
// strict schemas in contract.ts before the UI uses it, and every failure becomes an ApiError.
import { invoke, type InvokeArgs } from '@tauri-apps/api/core';
import { z } from 'zod';
import {
  AddResultSchema,
  AppearanceSettingsSchema,
  BatchResultSchema,
  CapabilitiesSchema,
  CategorySchema,
  DestinationCheckSchema,
  DownloadsSnapshotSchema,
  parseAppInfo,
  DiagnosticReportSchema,
  EngineHealthSchema,
  ShellSettingsSchema,
  type AddRequest,
  type AppearanceSettings,
  type Category,
  type DownloadLogin,
  type MoveDirection,
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
  /** What adding `url` would do with these choices, each `null` for automatic (#31). */
  checkDestination: (url: string, directory: string | null, fileName: string | null, categoryId: string | null) =>
    call('check_destination', { url, directory, fileName, categoryId }, DestinationCheckSchema),
  /** Adds the Add dialog's links and `.torrent` files: a result for each, in order (#60). */
  addDownloads: (request: AddRequest) => call('add_downloads', { request }, z.array(AddResultSchema)),
  /** What the transfer engine can do (#57). */
  capabilities: () => call('get_capabilities', undefined, CapabilitiesSchema),
  /** The system's chooser for `.torrent` files; empty when the user cancels. */
  chooseTorrents: () => call('choose_torrents', undefined, z.array(z.string().min(1))),
  /** The system's chooser for an SSH private key; `null` when the user cancels. */
  chooseSshKey: () => call('choose_ssh_key', undefined, z.string().min(1).nullable()),
  /** Trusts the SFTP host key the user confirmed (#53); the download is then retried. */
  trustHostKey: (host: string, port: number, fingerprint: string) => call('trust_host_key', { host, port, fingerprint }, nothing),
  retryDownloads: (ids: string[]) => call('retry_downloads', { ids }, BatchResultSchema),
  // The list's commands (#58, #59). Those over several downloads say what each did.
  pauseDownloads: (ids: string[]) => call('pause_downloads', { ids }, BatchResultSchema),
  resumeDownloads: (ids: string[]) => call('resume_downloads', { ids }, BatchResultSchema),
  cancelDownloads: (ids: string[]) => call('cancel_downloads', { ids }, BatchResultSchema),
  removeDownloads: (ids: string[], deleteFiles: boolean) => call('remove_downloads', { ids, deleteFiles }, BatchResultSchema),
  retryFailed: () => call('retry_failed', undefined, nothing),
  pauseAll: () => call('pause_all', undefined, nothing),
  resumeAll: () => call('resume_all', undefined, nothing),
  /** How many completed downloads left the list. */
  clearCompleted: () => call('clear_completed', undefined, z.number().int().nonnegative()),
  moveDownload: (id: string, direction: MoveDirection) => call('move_download', { id, direction }, nothing),
  /** Stops a torrent seeding: its seeding policy becomes "never". */
  stopSeeding: (id: string) => call('set_torrent_seeding', { id, seeding: { mode: 'never' } }, nothing),
  // Diagnostics (#28).
  diagnostics: () => call('get_diagnostics', undefined, DiagnosticReportSchema),
  /** Saves the diagnostics to a file the user chooses; where, or `null` when they cancel. */
  exportDiagnostics: () => call('export_diagnostics', undefined, z.string().min(1).nullable()),
  /** "Restart transfer engine" and "Restart torrent helper": the health after asking. */
  restartEngine: () => call('restart_engine', undefined, EngineHealthSchema),
  restartTorrentHelper: () => call('restart_torrent_helper', undefined, EngineHealthSchema),
  /** Gives a download that came back without its login, headers or SSH key those again; then resume or retry it. */
  setDownloadLogin: (id: string, login: DownloadLogin) => call('set_download_login', { id, login }, nothing),
};
