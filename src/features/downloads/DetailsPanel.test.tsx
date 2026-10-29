import { emit } from '@tauri-apps/api/event';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from '../../app/App';
import type { DownloadRecord, DownloadsSnapshot } from '../../contract';
import { DEFAULT_LAYOUT } from '../../layout/layoutLogic';
import { createQueryClient } from '../../lib/queries';
import { INITIAL_LIST, useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';
import { callsOf, emptySnapshot, installFakeBackend, type FakeState } from '../../test/fakeBackend';
import { makeRecord } from '../../test/records';

// V1's details-panel tests (tests/unit/app.test.tsx, "download list"), ported to V2's panel, and the panel's own (#61).

function renderApp() {
  return render(<App client={createQueryClient()} />);
}

const grid = () => screen.findByRole('grid', { name: 'Downloads' });
const rowFor = async (name: string) => {
  const cell = await within(await grid()).findByText(name);
  const row = cell.closest<HTMLElement>('[role="row"]');
  if (!row) throw new Error(`no row for ${name}`);
  return row;
};
const panel = () => screen.findByRole('complementary', { name: 'Download details' });
const heading = (p: HTMLElement) => within(p).getByRole('heading', { level: 2 }).textContent;

const progress = makeRecord().progress;
const archive = makeRecord({
  id: 'a',
  fileName: 'archive.zip',
  sourceUrl: 'https://example.com/a.zip',
  status: 'downloading',
  createdAtMs: 1,
  speedLimitBps: null,
  requestedConnections: 8,
  progress: { ...progress, completedBytes: 250, totalBytes: 1000, downloadSpeedBps: 100, connections: 3, connectionDetail: { requested: 8, allowed: 4, limitedBy: 'serverThrottling' } },
});
const movie = makeRecord({ id: 'b', fileName: 'movie.mp4', status: 'paused', createdAtMs: 2, speedLimitBps: 512 * 1024, requestedConnections: 4 });
const broken = makeRecord({
  id: 'c',
  fileName: 'broken.iso',
  status: 'failed',
  createdAtMs: 3,
  error: { class: 'permanent', httpStatus: 404, message: 'The file was not found on the server (HTTP 404).', retryable: false, hostKey: null, confirm: null },
});
const done = makeRecord({ id: 'd', fileName: 'setup.exe', status: 'completed', createdAtMs: 4, completedAtMs: 5, verification: 'notVerified' });
const album = makeRecord({
  id: 'e',
  kind: 'magnet',
  fileName: 'album',
  status: 'seeding',
  createdAtMs: 6,
  requestedConnections: null,
  torrent: { peers: 7, seeds: 2, uploadedBytes: 8 * 1024 * 1024, seedingSecs: 600, seedQueued: false },
});

function withDownloads(downloads: DownloadRecord[], over: Partial<DownloadsSnapshot> = {}): DownloadsSnapshot {
  return { ...emptySnapshot(), downloads, ...over };
}

beforeEach(() => {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
  });
  useUiStore.setState({ view: 'all', categoryFilter: null, layout: { ...DEFAULT_LAYOUT }, addDialog: { open: false, prefill: null }, detailsId: null });
  useDownloadsStore.setState({ snapshot: null, error: null, ...INITIAL_LIST });
});

describe('the details panel (V1)', () => {
  beforeEach(() => {
    installFakeBackend({ snapshot: withDownloads([archive, movie, broken, done, album]) });
  });

  it('opens on a double-click and groups the details into sections', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('archive.zip'));
    const p = await panel();
    for (const name of ['Transfer', 'Source and destination', 'Timeline', 'Options for this download']) {
      expect(within(p).getByRole('region', { name })).toBeTruthy();
    }
    const source = within(p).getByRole('region', { name: 'Source and destination' });
    expect(within(source).getByText('Folder')).toBeTruthy();
    expect(within(source).getByText('https://example.com/a.zip')).toBeTruthy();
  });

  it('shows errors in the details panel', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('broken.iso'));
    expect(within(await panel()).getByRole('alert').textContent).toContain('HTTP 404');
  });

  it('keeps the open details panel on the selected download', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('broken.iso'));
    const p = await panel();
    expect(heading(p)).toBe('broken.iso');

    await user.click(await rowFor('archive.zip'));
    expect(heading(p)).toBe('archive.zip');
    expect(within(p).getByText('https://example.com/a.zip')).toBeTruthy();

    await user.keyboard('{ArrowUp}');
    expect((await rowFor('movie.mp4')).getAttribute('aria-selected')).toBe('true');
    expect(heading(p)).toBe('movie.mp4');
  });

  it('does not carry a typed speed limit over to another download', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('archive.zip'));
    const p = await panel();
    const limit = within(p).getByLabelText<HTMLInputElement>(/Speed limit/);
    await user.clear(limit);
    await user.type(limit, '500');
    await user.click(await rowFor('movie.mp4'));
    expect(heading(p)).toBe('movie.mp4');
    expect(within(p).getByLabelText<HTMLInputElement>(/Speed limit/).value).toBe('512');
  });
});

describe('the details panel (#61)', () => {
  let state: FakeState;
  beforeEach(() => {
    state = installFakeBackend({ snapshot: withDownloads([archive, movie, broken, done, album]) });
  });

  it('opens with Enter and the toolbar button, closes, and remembers whether it is open', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(await rowFor('movie.mp4'));
    await user.keyboard('{Enter}');
    expect(heading(await panel())).toBe('movie.mp4');
    await user.click(screen.getByRole('button', { name: 'Close details' }));
    await waitFor(() => { expect(screen.queryByRole('complementary', { name: 'Download details' })).toBeNull(); });
    const toggle = screen.getByRole('button', { name: 'Toggle details panel' });
    expect(toggle.getAttribute('aria-pressed')).toBe('false');
    await user.click(toggle);
    expect(heading(await panel())).toBe('movie.mp4');
    await waitFor(() => { expect(callsOf(state, 'set_appearance').at(-1)?.args).toMatchObject({ appearance: { layout: { detailsVisible: true } } }); });
  });

  it('shows the download it was opened for while the search hides it, and says so', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('broken.iso'));
    const p = await panel();
    await user.type(screen.getByRole('searchbox', { name: 'Search downloads' }), 'movie');
    await waitFor(() => { expect(within(screen.getByRole('grid', { name: 'Downloads' })).queryByText('broken.iso')).toBeNull(); });
    expect(heading(p)).toBe('broken.iso');
    expect(within(p).getByRole('status').textContent).toContain('hidden by the search or filter');
  });

  it('says so when its download leaves the list, without showing another one', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('broken.iso'));
    await panel();
    await act(() => emit('downloads-snapshot', withDownloads([archive, movie, done, album])));
    await waitFor(async () => { expect((await panel()).textContent).toContain('This download is no longer in the list.'); });
    expect(within(await panel()).queryByRole('heading', { level: 2 })).toBeNull();
  });

  it('says how many connections are in use and what holds them back', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('archive.zip'));
    const transfer = within(await panel()).getByRole('region', { name: 'Transfer' });
    expect(transfer.textContent).toContain('Using 3 connections, up to 4 of the 8 asked for: the server asked Velocity to slow down.');
  });

  it('says a download that isn’t running isn’t connected, without the reason it had while it ran', async () => {
    const user = userEvent.setup();
    state.snapshot = withDownloads([{ ...movie, progress: { ...movie.progress, connections: 0, connectionDetail: { requested: 8, allowed: 4, limitedBy: 'serverThrottling' } } }]);
    renderApp();
    await user.dblClick(await rowFor('movie.mp4'));
    const transfer = within(await panel()).getByRole('region', { name: 'Transfer' });
    expect(transfer.textContent).toContain('Not connected.');
    expect(transfer.textContent).not.toContain('slow down');
  });

  it('changes the connections of a running download at once, with no restart to confirm', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('archive.zip'));
    const p = await panel();
    const input = within(p).getByLabelText<HTMLInputElement>('Connections');
    expect(input.value).toBe('8');
    await user.clear(input);
    await user.type(input, '12');
    await user.click(within(p).getByRole('button', { name: 'Apply' }));
    await waitFor(() => { expect(callsOf(state, 'set_download_connections')[0]?.args).toEqual({ id: 'a', connections: 12 }); });
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(await screen.findByText('Connections set to 12')).toBeTruthy();
  });

  it('sets the speed limit and the category of the download shown', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('movie.mp4'));
    const p = await panel();
    const limit = within(p).getByLabelText<HTMLInputElement>(/Speed limit/);
    await user.clear(limit);
    await user.type(limit, '256');
    await user.tab();
    await waitFor(() => { expect(callsOf(state, 'set_download_speed_limit').at(-1)?.args).toEqual({ id: 'b', kib: 256 }); });
    await user.selectOptions(within(p).getByLabelText('Category'), 'None');
    await waitFor(() => { expect(callsOf(state, 'set_download_category')[0]?.args).toEqual({ id: 'b', categoryId: null }); });
  });

  it('shows the verification state of a completed download', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('broken.iso'));
    await user.click(await rowFor('setup.exe'));
    const transfer = within(await panel()).getByRole('region', { name: 'Transfer' });
    expect(transfer.textContent).toContain('Not verified');
  });

  it('opens a finished file, asking first when it is a program', async () => {
    const user = userEvent.setup();
    state.programs = ['d'];
    renderApp();
    await user.click(await rowFor('setup.exe'));
    await user.keyboard('{Enter}');
    await user.click(within(await panel()).getByRole('button', { name: 'Open file' }));
    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('can run code on your computer');
    await user.click(within(dialog).getByRole('button', { name: 'Open anyway' }));
    await waitFor(() => {
      expect(callsOf(state, 'open_download_file').map((c) => c.args)).toEqual([
        { id: 'd', allowProgram: false },
        { id: 'd', allowProgram: true },
      ]);
    });
  });

  it('opens nothing when the user declines to open a program', async () => {
    const user = userEvent.setup();
    state.programs = ['d'];
    renderApp();
    await user.click(await rowFor('setup.exe'));
    await user.keyboard('{Enter}');
    await user.click(within(await panel()).getByRole('button', { name: 'Open file' }));
    await user.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Cancel' }));
    expect(callsOf(state, 'open_download_file')).toHaveLength(1);
  });

  it('opens a finished file on a double-click, and the folder from the panel', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.dblClick(await rowFor('setup.exe'));
    await waitFor(() => { expect(callsOf(state, 'open_download_file')[0]?.args).toEqual({ id: 'd', allowProgram: false }); });
    await user.dblClick(await rowFor('movie.mp4'));
    expect(within(await panel()).queryByRole('button', { name: 'Open file' })).toBeNull();
    await user.click(within(await panel()).getByRole('button', { name: 'Open folder' }));
    await waitFor(() => { expect(callsOf(state, 'open_download_folder')[0]?.args).toEqual({ id: 'b' }); });
  });

  it('says why a file could not be opened', async () => {
    const user = userEvent.setup();
    state.failures.open_download_file = 'NOT_FOUND';
    renderApp();
    await user.dblClick(await rowFor('setup.exe'));
    expect(await screen.findByText(/no longer there/)).toBeTruthy();
  });

  it('offers opening and the details from the right-click menu', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.pointer({ keys: '[MouseRight]', target: await rowFor('setup.exe') });
    const menu = await screen.findByRole('menu');
    await user.click(within(menu).getByRole('menuitem', { name: 'Open folder' }));
    await waitFor(() => { expect(callsOf(state, 'open_download_folder')[0]?.args).toEqual({ id: 'd' }); });
    await user.pointer({ keys: '[MouseRight]', target: await rowFor('movie.mp4') });
    const again = await screen.findByRole('menu');
    expect(within(again).getByRole('menuitem', { name: 'Open file' }).getAttribute('aria-disabled')).toBe('true');
    await user.click(within(again).getByRole('menuitem', { name: 'Details' }));
    expect(heading(await panel())).toBe('movie.mp4');
  });

  it('shows a torrent’s peers and seeding, and no connections to set', async () => {
    const user = userEvent.setup();
    renderApp();
    // A double-click would open a seeding torrent's files.
    await user.click(await rowFor('album'));
    await user.keyboard('{Enter}');
    const p = await panel();
    const torrent = within(p).getByRole('region', { name: 'Torrent' });
    expect(torrent.textContent).toContain('7 connected, 2 with the whole torrent');
    expect(torrent.textContent).toContain('8.0 MiB');
    expect(torrent.textContent).toContain('10m 00s');
    expect(within(p).queryByLabelText('Connections')).toBeNull();
  });

  it('says a torrent’s peers are unknown until the helper reports them', async () => {
    const user = userEvent.setup();
    state.snapshot = withDownloads([{ ...album, torrent: null }]);
    renderApp();
    await user.click(await rowFor('album'));
    await user.keyboard('{Enter}');
    const torrent = within(await panel()).getByRole('region', { name: 'Torrent' });
    expect(torrent.textContent).toContain('Not reported yet');
  });
});
