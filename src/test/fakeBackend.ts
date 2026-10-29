// A fake native side for the window's tests (#29): the commands of docs/CONTRACTS.md over Tauri's
// IPC mock, answering with the generated contract fixtures, and recording every call.
import { mockIPC } from '@tauri-apps/api/mocks';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
  COMMAND_API_VERSION,
  DEFAULT_COLUMNS,
  type AddRequest,
  type AddResult,
  type AppInfo,
  type Capabilities,
  type Category,
  type DestinationCheck,
  type DownloadsSnapshot,
  type ErrorCode,
  type ShellSettings,
} from '../contract';

export const fixture = (name: string): unknown =>
  JSON.parse(readFileSync(join(__dirname, '..', '..', 'tests', 'fixtures', 'contract', name), 'utf8'));

export interface FakeState {
  appInfo: AppInfo;
  settings: ShellSettings;
  snapshot: DownloadsSnapshot;
  categories: Category[];
  /** What the folder chooser returns; `null` is a cancel. */
  chosenDirectory: string | null;
  /** What the `.torrent` chooser returns; empty is a cancel. */
  chosenTorrents: string[];
  /** What the SSH key chooser returns; `null` is a cancel. */
  chosenKey: string | null;
  capabilities: Capabilities;
  /** What `check_destination` answers for its arguments. */
  destination: (args: { url: string; directory: string | null; fileName: string | null; categoryId: string | null }) => DestinationCheck;
  /** What `add_downloads` answers; by default, every link and file is added. */
  addResults: (request: AddRequest) => AddResult[];
  /** Commands that fail, with the code they fail with. */
  failures: Partial<Record<string, ErrorCode>>;
  calls: { cmd: string; args: Record<string, unknown> }[];
}

/** A first-run settings document: nothing saved yet. */
export function firstRun(): ShellSettings {
  return {
    appearance: {
      theme: 'system',
      density: 'compact',
      layout: { sidebarWidth: null, sidebarCollapsed: false, sidebarVisible: true, detailsWidth: null, detailsVisible: false },
      visibleColumns: [...DEFAULT_COLUMNS],
      columnWidths: {},
    },
    onboardingCompleted: false,
    defaultDirectory: '/home/user/Downloads',
  };
}

/** Settings after the first-run setup, with the default layout. */
export function setUp(): ShellSettings {
  return { ...firstRun(), onboardingCompleted: true };
}

export function emptySnapshot(): DownloadsSnapshot {
  return { downloads: [], unavailable: [], needsLogin: [], engine: { state: 'running' }, torrentEngine: { state: 'running' }, storageError: null };
}

export function installFakeBackend(over: Partial<FakeState> = {}): FakeState {
  const state: FakeState = {
    appInfo: { name: 'Velocity Download Manager (dev)', version: '0.1.0', commandApiVersion: COMMAND_API_VERSION },
    settings: setUp(),
    snapshot: emptySnapshot(),
    categories: [fixture('category.json') as Category],
    chosenDirectory: null,
    chosenTorrents: [],
    chosenKey: null,
    capabilities: withSftp(fixture('capabilities.json') as Capabilities),
    destination: ({ directory, fileName, categoryId }) => ({
      directory: directory ?? '/home/user/Downloads',
      directoryExists: true,
      automaticDirectory: directory === null,
      categoryId,
      fileName,
      fileExists: false,
      partialDownloadExists: false,
      suggestedName: null,
    }),
    addResults: (request) =>
      [...request.links, ...request.torrentFiles].map((source, i) => ({ source, outcome: { kind: 'added', id: `d-${String(100 + i)}` } })),
    failures: {},
    calls: [],
    ...over,
  };
  let next = 0;
  mockIPC(
    (cmd, payload) => {
      const args = (payload ?? {}) as Record<string, unknown>;
      if (!cmd.startsWith('plugin:')) state.calls.push({ cmd, args });
      const failure = state.failures[cmd];
      // Tauri rejects a failed command with its serialised error: the code, a string.
      // eslint-disable-next-line @typescript-eslint/only-throw-error
      if (failure) throw failure;
      switch (cmd) {
        case 'app_info':
          return state.appInfo;
        case 'get_shell_settings':
          return state.settings;
        case 'set_appearance':
          state.settings = { ...state.settings, appearance: args.appearance as ShellSettings['appearance'] };
          return state.settings.appearance;
        case 'complete_onboarding':
          state.settings = { ...state.settings, onboardingCompleted: true, defaultDirectory: (args.directory as string).trim() };
          return state.settings;
        case 'choose_directory':
          return state.chosenDirectory;
        case 'get_snapshot':
          return state.snapshot;
        case 'list_categories':
          return state.categories;
        case 'create_category': {
          next += 1;
          const created = { ...(args.input as Omit<Category, 'id' | 'builtin'>), id: `user-${String(next)}`, builtin: false };
          state.categories = [...state.categories, created];
          return created;
        }
        case 'update_category': {
          const updated = { ...(args.input as Omit<Category, 'id' | 'builtin'>), id: args.id as string, builtin: state.categories.find((c) => c.id === args.id)?.builtin ?? false };
          state.categories = state.categories.map((c) => (c.id === args.id ? updated : c));
          return updated;
        }
        case 'delete_category':
          state.categories = state.categories.filter((c) => c.id !== args.id);
          return null;
        case 'quit_app':
          return null;
        case 'check_destination':
          return state.destination(args as Parameters<FakeState['destination']>[0]);
        case 'add_downloads':
          return state.addResults(args.request as AddRequest);
        case 'get_capabilities':
          return state.capabilities;
        case 'choose_torrents':
          return state.chosenTorrents;
        case 'choose_ssh_key':
          return state.chosenKey;
        case 'trust_host_key':
          return null;
        case 'retry_downloads':
          return { done: args.ids, failed: [], kept: [] };
        default:
          throw new Error(`the fake backend has no command ${cmd}`);
      }
    },
    { shouldMockEvents: true },
  );
  return state;
}

/** The sample engine, which downloads over SFTP too. */
export function withSftp(capabilities: Capabilities): Capabilities {
  const http = capabilities.protocols[0];
  return http ? { ...capabilities, protocols: [...capabilities.protocols, { ...http, kind: 'sftp' }] } : capabilities;
}

/** The calls of one command, in order. */
export const callsOf = (state: FakeState, cmd: string) => state.calls.filter((c) => c.cmd === cmd);
