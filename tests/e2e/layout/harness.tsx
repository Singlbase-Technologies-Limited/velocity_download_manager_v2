/**
 * The browser harness for the layout checks (layout-e2e.mjs, V1's tests/e2e/layout): the real window in Chromium, with
 * Tauri's IPC mocked and made-up downloads, so splitters and column edges can be dragged with a real pointer and
 * screenshots taken without the desktop shell or an engine. Saved settings live in localStorage, so a reload stands in
 * for a restart. Served only by Vite's dev server; never part of a build.
 *
 * Query parameters: `theme=light|dark` for a fresh start, and `fresh=1` to forget what was saved.
 */
import { mockIPC } from '@tauri-apps/api/mocks';
import { createRoot } from 'react-dom/client';
import { App } from '../../../src/app/App';
import type { Category, DownloadRecord, DownloadsSnapshot, ShellSettings } from '../../../src/contract';
import { COMMAND_API_VERSION, DEFAULT_COLUMNS } from '../../../src/contract';
import '../../../src/index.css';
import capabilities from '../../fixtures/contract/capabilities.json';
import category from '../../fixtures/contract/category.json';
import sample from '../../fixtures/contract/download-record.json';

const KEY = 'velocity-layout-harness-settings';
const params = new URLSearchParams(location.search);
if (params.get('fresh') === '1') localStorage.removeItem(KEY);

const saved = localStorage.getItem(KEY);
const settings: ShellSettings = saved
  ? (JSON.parse(saved) as ShellSettings)
  : {
      appearance: {
        theme: params.get('theme') === 'dark' ? 'dark' : 'light',
        density: 'compact',
        layout: { sidebarWidth: null, sidebarCollapsed: false, sidebarVisible: true, detailsWidth: null, detailsVisible: false },
        visibleColumns: [...DEFAULT_COLUMNS],
        columnWidths: {},
      },
      onboardingCompleted: true,
      defaultDirectory: '/home/user/Downloads',
    };

const base = sample as DownloadRecord;
const at = (day: number) => Date.UTC(2026, 9, day, 10);
const make = (over: Partial<DownloadRecord>): DownloadRecord => ({ ...base, error: null, retryAtMs: null, ...over });
const LONG = 'Ubuntu-24.04.1-desktop-amd64-with-an-extremely-long-file-name-that-keeps-going-and-going-beyond-the-column-width.iso';
const progress = base.progress;
const downloads: DownloadRecord[] = [
  make({
    id: 'long',
    fileName: LONG,
    sourceUrl: `https://mirror.example.org/releases/24.04.1/very/deep/path/${LONG}`,
    directory: '/home/user/Downloads/Operating Systems/Linux/Ubuntu/Releases/24.04/with/a/very/long/folder/path',
    status: 'downloading',
    createdAtMs: at(5),
    progress: { ...progress, completedBytes: 1_200_000_000, totalBytes: 6_100_000_000, downloadSpeedBps: 12_500_000, connections: 4 },
  }),
  make({ id: 'zip', fileName: 'archive.zip', status: 'paused', createdAtMs: at(4) }),
  make({ id: 'retry', fileName: 'flaky-server-file.bin', status: 'retrying', retryAtMs: Date.now() + 90_000, createdAtMs: at(4) - 1 }),
  make({
    id: 'movie',
    fileName: 'holiday-movie-1080p.mp4',
    status: 'completed',
    createdAtMs: at(3),
    completedAtMs: at(3) + 600_000,
    progress: { ...progress, completedBytes: 4_200_000_000, totalBytes: 4_200_000_000, connections: 0, connectionDetail: null },
  }),
  make({
    id: 'failed',
    fileName: 'broken-download-with-a-long-name.tar.gz',
    status: 'failed',
    createdAtMs: at(2),
    error: { class: 'permanent', httpStatus: 404, message: 'The file was not found on the server (HTTP 404).', retryable: false, hostKey: null, confirm: null },
  }),
];
for (let i = 0; i < 30; i += 1) {
  downloads.push(make({ id: `bulk${String(i)}`, fileName: `dataset-part-${String(i).padStart(3, '0')}.bin`, status: i % 3 === 0 ? 'queued' : 'paused', createdAtMs: Date.UTC(2026, 8, (i % 28) + 1, 10) }));
}
const snapshot: DownloadsSnapshot = { downloads, unavailable: [], needsLogin: [], engine: { state: 'running' }, torrentEngine: { state: 'running' }, storageError: null };

/** Each `set_appearance`, counted, so a check can see a drag saves once. */
const win = window as unknown as { appearanceSaves: number };
win.appearanceSaves = 0;

mockIPC(
  (cmd, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case 'app_info':
        return { name: 'Velocity Download Manager (layout harness)', version: '0.1.0', commandApiVersion: COMMAND_API_VERSION };
      case 'get_shell_settings':
        return settings;
      case 'set_appearance':
        win.appearanceSaves += 1;
        settings.appearance = args.appearance as ShellSettings['appearance'];
        localStorage.setItem(KEY, JSON.stringify(settings));
        return settings.appearance;
      case 'get_snapshot':
        return snapshot;
      case 'list_categories':
        return [category as Category];
      case 'get_capabilities':
        return capabilities;
      case 'pause_downloads':
      case 'resume_downloads':
      case 'cancel_downloads':
      case 'retry_downloads':
      case 'remove_downloads':
        return { done: args.ids, failed: [], kept: [] };
      default:
        return null;
    }
  },
  { shouldMockEvents: true },
);

const root = document.getElementById('root');
if (root) createRoot(root).render(<App />);
