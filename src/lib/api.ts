// The typed bridge to the native commands (#29, docs/CONTRACTS.md). Every answer is checked with the
// strict schemas in contract.ts before the UI uses it, and every failure becomes an ApiError.
import { invoke, type InvokeArgs } from '@tauri-apps/api/core';
import { z } from 'zod';
import {
  AddResultSchema,
  AppearanceSettingsSchema,
  BatchResultSchema,
  BrowserIntegrationSchema,
  CapabilitiesSchema,
  CategorySchema,
  DestinationCheckSchema,
  DownloadsSnapshotSchema,
  parseAppInfo,
  DiagnosticReportSchema,
  EngineHealthSchema,
  HistoryPageSchema,
  KeptFileSchema,
  PeerStatusSchema,
  NetworkStatusSchema,
  SettingsSchema,
  ShellSettingsSchema,
  StorageSettingsSchema,
  TorrentDetailsSchema,
  TorrentPreviewSchema,
  TrackerStatusSchema,
  type AddRequest,
  type Browser,
  type ConfirmKind,
  type AppearanceSettings,
  type Category,
  type DownloadLogin,
  type Settings,
  type HistoryQuery,
  type StorageSettings,
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
  // The Settings screen (#30).
  settings: () => call('get_settings', undefined, SettingsSchema),
  /** Checks, saves and applies every section; the settings as saved. */
  setSettings: (settings: Settings) => call('set_settings', { settings }, SettingsSchema),
  resetSettings: () => call('reset_settings', undefined, SettingsSchema),
  /** Whether the proxy has a password (#192); the password itself never comes back. */
  proxyPasswordSet: () => call('get_proxy_password_set', undefined, z.boolean()),
  /** Sets the proxy's password, or clears it with `null` (#192). */
  setProxyPassword: (password: string | null) => call('set_proxy_password', { password }, nothing),
  torrentNetwork: () => call('get_torrent_network', undefined, NetworkStatusSchema),
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
  // The browser integration (#65): where the native host is registered, for this user only.
  browserIntegration: () => call('get_browser_integration', undefined, BrowserIntegrationSchema),
  registerBrowser: (browser: Browser) => call('register_browser', { browser }, BrowserIntegrationSchema),
  unregisterBrowser: (browser: Browser) => call('unregister_browser', { browser }, BrowserIntegrationSchema),
  /** Allows what a download stopped at for the user's confirmation (#67), and retries it. */
  allowDownload: (id: string, kind: ConfirmKind) => call('allow_download', { id, kind }, nothing),
  /** Saves a download's login in the system keychain, at the user's request (#70). */
  rememberLogin: (id: string) => call('remember_login', { id }, nothing),
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
  // The history (#32).
  history: (query: HistoryQuery) => call('get_history', { query }, HistoryPageSchema),
  /** How many entries left the history. */
  clearHistory: () => call('clear_history', undefined, z.number().int().nonnegative()),
  deleteHistoryEntries: (ids: string[]) => call('delete_history_entries', { ids }, nothing),
  storageSettings: () => call('get_storage_settings', undefined, StorageSettingsSchema),
  setStorageSettings: (settings: StorageSettings) => call('set_storage_settings', { settings }, StorageSettingsSchema),
  // The details panel's (#61).
  /** Sets a download's own speed limit in KiB/s; 0 for none. */
  setSpeedLimit: (id: string, kib: number) => call('set_download_speed_limit', { id, kib }, nothing),
  /** Asks for `connections`; answers how many apply. */
  setConnections: (id: string, connections: number) => call('set_download_connections', { id, connections }, z.number().int().positive()),
  setCategory: (id: string, categoryId: string | null) => call('set_download_category', { id, categoryId }, nothing),
  /** Opens the finished file; `CONFIRMATION_REQUIRED` for a program until `allowProgram`. */
  openFile: (id: string, allowProgram: boolean) => call('open_download_file', { id, allowProgram }, nothing),
  /** Shows the file in its folder, or the folder when the file is gone. */
  openFolder: (id: string) => call('open_download_folder', { id }, nothing),
  // Torrents (#63).
  /** What adding the `.torrent` file at `path` would add. */
  previewTorrent: (path: string) => call('preview_torrent_file', { path }, TorrentPreviewSchema),
  torrentDetails: (id: string) => call('get_torrent_details', { id }, TorrentDetailsSchema),
  trackers: (id: string) => call('get_torrent_trackers', { id }, z.array(TrackerStatusSchema)),
  peers: (id: string) => call('get_torrent_peers', { id }, z.array(PeerStatusSchema)),
  reannounce: (id: string) => call('reannounce_torrent', { id }, nothing),
  /** KiB/s; 0 for no limit. */
  setUploadLimit: (id: string, kib: number) => call('set_torrent_upload_limit', { id, kib }, nothing),
  /** Downloads exactly `files`; with `deleteDropped`, deletes the torrent's own files no longer chosen. Answers the files kept. */
  chooseTorrentFiles: (id: string, files: number[], deleteDropped: boolean) => call('choose_torrent_files', { id, files, deleteDropped }, z.array(KeptFileSchema)),
  /** Gives a download that came back without its login, headers or SSH key those again; then resume or retry it. */
  setDownloadLogin: (id: string, login: DownloadLogin) => call('set_download_login', { id, login }, nothing),
};
