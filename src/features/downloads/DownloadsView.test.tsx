import { emit } from '@tauri-apps/api/event';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from '../../app/App';
import type { AppearanceSettings, DownloadRecord, DownloadsSnapshot } from '../../contract';
import { DEFAULT_LAYOUT } from '../../layout/layoutLogic';
import { createQueryClient } from '../../lib/queries';
import { INITIAL_LIST, useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';
import { callsOf, emptySnapshot, installFakeBackend, type FakeState } from '../../test/fakeBackend';
import { makeRecord } from '../../test/records';

// V1's download-list tests (tests/unit/app.test.tsx, "download list"), ported to V2's list, and the list's own (#59).

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
const button = (name: string) => screen.getByRole<HTMLButtonElement>('button', { name });
const appearanceSaves = (state: FakeState) => callsOf(state, 'set_appearance').map((c) => c.args.appearance as AppearanceSettings);

const progress = makeRecord().progress;
const archive = makeRecord({ id: 'a', fileName: 'archive.zip', status: 'downloading', createdAtMs: 1, progress: { ...progress, completedBytes: 250, totalBytes: 1000, downloadSpeedBps: 100 } });
const movie = makeRecord({ id: 'b', fileName: 'movie.mp4', status: 'paused', categoryId: 'video', createdAtMs: 2 });
const broken = makeRecord({
  id: 'c',
  fileName: 'broken.iso',
  status: 'failed',
  createdAtMs: 3,
  error: { class: 'permanent', httpStatus: 404, message: 'The file was not found on the server (HTTP 404).', retryable: false, hostKey: null },
});

function withDownloads(downloads: DownloadRecord[], over: Partial<DownloadsSnapshot> = {}): DownloadsSnapshot {
  return { ...emptySnapshot(), downloads, ...over };
}

beforeEach(() => {
  // jsdom has no matchMedia; the window follows a light system here.
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
  });
  useUiStore.setState({ view: 'all', categoryFilter: null, layout: { ...DEFAULT_LAYOUT }, addDialog: { open: false, prefill: null } });
  useDownloadsStore.setState({ snapshot: null, error: null, ...INITIAL_LIST });
});

describe('the download list (V1)', () => {
  let state: FakeState;
  beforeEach(() => {
    state = installFakeBackend({ snapshot: withDownloads([archive, movie, broken]) });
  });

  it('shows an empty state with no downloads', async () => {
    installFakeBackend();
    renderApp();
    expect(await screen.findByText('No downloads yet')).toBeTruthy();
  });

  it('renders status and real progress from the snapshot', async () => {
    renderApp();
    const row = await rowFor('archive.zip');
    expect(within(row).getByText('Downloading')).toBeTruthy();
    expect(within(row).getByText('25%')).toBeTruthy();
    expect(within(row).getByRole('progressbar').getAttribute('aria-valuenow')).toBe('25');
    expect(within(await rowFor('broken.iso')).getByText('Failed')).toBeTruthy();
  });

  it('updates rows when a new snapshot arrives', async () => {
    renderApp();
    await rowFor('archive.zip');
    const moved = { ...archive, progress: { ...archive.progress, completedBytes: 800 } };
    await act(() => emit('downloads-snapshot', withDownloads([moved, movie, broken])));
    expect(within(await rowFor('archive.zip')).getByText('80%')).toBeTruthy();
  });

  it('pauses and resumes the selected download', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(await rowFor('archive.zip'));
    await user.click(button('Pause selected'));
    await waitFor(() => { expect(callsOf(state, 'pause_downloads')[0]?.args).toEqual({ ids: ['a'] }); });

    await user.click(await rowFor('movie.mp4'));
    expect(button('Pause selected').disabled).toBe(true);
    await user.click(button('Resume selected'));
    await waitFor(() => { expect(callsOf(state, 'resume_downloads')[0]?.args).toEqual({ ids: ['b'] }); });
  });

  it('retries a failed download', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(await rowFor('broken.iso'));
    await user.click(button('Retry selected'));
    await waitFor(() => { expect(callsOf(state, 'retry_downloads')[0]?.args).toEqual({ ids: ['c'] }); });
  });

  it('asks before removing an active download and keeps files', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(await rowFor('archive.zip'));
    await user.click(button('Remove selected from list'));
    let dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('Files already on disk are kept');
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(callsOf(state, 'remove_downloads')).toHaveLength(0);

    await user.click(button('Remove selected from list'));
    dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: 'Stop and remove' }));
    await waitFor(() => { expect(callsOf(state, 'remove_downloads')[0]?.args).toEqual({ ids: ['a'], deleteFiles: false }); });
  });

  it('requires confirmation before deleting files', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(await rowFor('broken.iso'));
    await user.keyboard('{Shift>}{Delete}{/Shift}');
    const dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('cannot be undone');
    await user.click(within(dialog).getByRole('button', { name: 'Delete files' }));
    await waitFor(() => { expect(callsOf(state, 'remove_downloads')[0]?.args).toEqual({ ids: ['c'], deleteFiles: true }); });
  });

  it('filters by search text', async () => {
    const user = userEvent.setup();
    renderApp();
    await rowFor('archive.zip');
    await user.type(screen.getByRole('searchbox', { name: 'Search downloads' }), 'movie');
    const g = await grid();
    await waitFor(() => { expect(within(g).queryByText('archive.zip')).toBeNull(); });
    expect(within(g).getByText('movie.mp4')).toBeTruthy();
    await user.clear(screen.getByRole('searchbox', { name: 'Search downloads' }));
    await user.type(screen.getByRole('searchbox', { name: 'Search downloads' }), 'zzz');
    expect(await screen.findByText('No matches')).toBeTruthy();
  });

  it('supports keyboard navigation and space to pause', async () => {
    const user = userEvent.setup();
    renderApp();
    const g = await grid();
    g.focus();
    await user.keyboard('{Home}');
    // Newest first: broken.iso, movie.mp4, archive.zip.
    expect((await rowFor('broken.iso')).getAttribute('aria-selected')).toBe('true');
    await user.keyboard('{ArrowDown}{ArrowDown}');
    expect((await rowFor('archive.zip')).getAttribute('aria-selected')).toBe('true');
    await user.keyboard(' ');
    await waitFor(() => { expect(callsOf(state, 'pause_downloads')[0]?.args).toEqual({ ids: ['a'] }); });
    await user.keyboard('{Shift>}{ArrowUp}{/Shift}');
    expect((await rowFor('movie.mp4')).getAttribute('aria-selected')).toBe('true');
    expect((await rowFor('archive.zip')).getAttribute('aria-selected')).toBe('true');
  });
});

describe('the download list (#59)', () => {
  it('counts down to the engine’s next attempt', async () => {
    installFakeBackend({ snapshot: withDownloads([makeRecord({ fileName: 'flaky.bin', status: 'retrying', retryAtMs: Date.now() + 30_000 })]) });
    renderApp();
    expect(within(await rowFor('flaky.bin')).getByText(/^Retrying in (29|30)s$/)).toBeTruthy();
  });

  it('shows no speed or time left for a download that isn’t moving', async () => {
    const stalled = makeRecord({ fileName: 'stuck.bin', status: 'stalled', progress: { ...progress, completedBytes: 10, totalBytes: 100, downloadSpeedBps: 0 } });
    installFakeBackend({ snapshot: withDownloads([stalled]) });
    renderApp();
    const row = await rowFor('stuck.bin');
    expect(within(row).getByText('No data received')).toBeTruthy();
    expect(within(row).queryByText(/\/s$/)).toBeNull();
  });

  it('says what a command over several downloads couldn’t do', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({
      snapshot: withDownloads([archive, makeRecord({ id: 'd', fileName: 'second.zip', status: 'downloading', createdAtMs: 4 })]),
      batch: (_cmd, ids) => ({ done: ids.slice(1), failed: [{ id: ids[0] ?? '', code: 'INVALID_STATE' }], kept: [] }),
    });
    renderApp();
    await rowFor('archive.zip');
    await user.click(screen.getByRole('checkbox', { name: 'Select all shown downloads' }));
    await user.click(button('Pause selected'));
    await waitFor(() => { expect(callsOf(state, 'pause_downloads')).toHaveLength(1); });
    expect(await screen.findByText(/^Paused 1; 1 couldn’t be changed:/)).toBeTruthy();
  });

  it('can’t send an action twice while it is being sent', async () => {
    const user = userEvent.setup();
    let release: () => void = () => undefined;
    const state = installFakeBackend({ snapshot: withDownloads([archive]), hold: { pause_downloads: new Promise<void>((r) => { release = r; }) } });
    renderApp();
    await user.click(await rowFor('archive.zip'));
    await user.click(button('Pause selected'));
    await waitFor(() => { expect(button('Pause selected').disabled).toBe(true); });
    act(() => { release(); });
    await waitFor(() => { expect(button('Pause selected').disabled).toBe(false); });
    expect(callsOf(state, 'pause_downloads')).toHaveLength(1);
  });

  it('selects with checkboxes: one row, or every row shown', async () => {
    const user = userEvent.setup();
    installFakeBackend({ snapshot: withDownloads([archive, movie, broken]) });
    renderApp();
    await user.click(await within(await grid()).findByRole('checkbox', { name: 'Select movie.mp4' }));
    expect((await rowFor('movie.mp4')).getAttribute('aria-selected')).toBe('true');
    expect((await rowFor('archive.zip')).getAttribute('aria-selected')).toBe('false');
    const all = screen.getByRole<HTMLInputElement>('checkbox', { name: 'Select all shown downloads' });
    expect(all.indeterminate).toBe(true);
    await user.click(all);
    for (const name of ['archive.zip', 'movie.mp4', 'broken.iso']) expect((await rowFor(name)).getAttribute('aria-selected')).toBe('true');
    await user.click(all);
    expect((await rowFor('movie.mp4')).getAttribute('aria-selected')).toBe('false');
  });

  it('sorts by a column, and says so', async () => {
    const user = userEvent.setup();
    installFakeBackend({ snapshot: withDownloads([archive, movie, broken]) });
    renderApp();
    await rowFor('archive.zip');
    await user.click(screen.getByRole('button', { name: 'Name' }));
    expect(screen.getByRole('columnheader', { name: 'Name' }).getAttribute('aria-sort')).toBe('ascending');
    const names = within(await grid()).getAllByRole('row').map((r) => r.querySelector('[data-column="name"]')?.textContent);
    expect(names).toEqual(['archive.zip', 'broken.iso', 'movie.mp4']);
  });

  it('chooses its columns, and saves the choice', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({ snapshot: withDownloads([archive]) });
    renderApp();
    await rowFor('archive.zip');
    expect(screen.queryByRole('columnheader', { name: 'Destination' })).toBeNull();
    await user.click(button('Choose columns'));
    expect((await screen.findByRole('menuitemcheckbox', { name: 'Name' })).getAttribute('aria-disabled')).toBe('true');
    await user.click(screen.getByRole('menuitemcheckbox', { name: 'Destination' }));
    await waitFor(() => { expect(appearanceSaves(state).at(-1)?.visibleColumns).toContain('destination'); });
    // The menu stays open for more choices; the list shows the column once it closes.
    await user.keyboard('{Escape}');
    expect(await screen.findByRole('columnheader', { name: 'Destination' })).toBeTruthy();
  });

  it('saves a dragged column width once, when the drag ends', async () => {
    const state = installFakeBackend({ snapshot: withDownloads([archive]) });
    renderApp();
    await rowFor('archive.zip');
    const handle = screen.getByRole('separator', { name: 'Resize Size column' });
    fireEvent.pointerDown(handle, { clientX: 100, pointerId: 1 });
    for (const x of [120, 140, 160]) fireEvent.pointerMove(handle, { clientX: x, pointerId: 1 });
    expect(appearanceSaves(state)).toHaveLength(0);
    fireEvent.pointerUp(handle, { clientX: 160, pointerId: 1 });
    await waitFor(() => { expect(appearanceSaves(state)).toHaveLength(1); });
    expect(appearanceSaves(state)[0]?.columnWidths.size).toBeGreaterThanOrEqual(84);
  });

  it('acts on the selection from the right-click menu', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({ snapshot: withDownloads([archive, movie]) });
    renderApp();
    fireEvent.contextMenu(await rowFor('archive.zip'));
    await user.click(await screen.findByRole('menuitem', { name: 'Pause' }));
    await waitFor(() => { expect(callsOf(state, 'pause_downloads')[0]?.args).toEqual({ ids: ['a'] }); });
  });

  it('asks for the login again before resuming a download that came back without it', async () => {
    const user = userEvent.setup();
    const waiting = makeRecord({ id: 'l', fileName: 'private.zip', status: 'paused' });
    const state = installFakeBackend({ snapshot: withDownloads([waiting], { needsLogin: ['l'] }) });
    renderApp();
    const row = await rowFor('private.zip');
    expect(within(row).getByText('Needs its login')).toBeTruthy();
    await user.click(row);
    await user.click(button('Resume selected'));
    const dialog = await screen.findByRole('dialog', { name: 'Enter the login again' });
    await user.type(within(dialog).getByRole('textbox', { name: 'User name' }), 'alice');
    await user.type(within(dialog).getByLabelText('Password'), 'secret');
    await user.click(within(dialog).getByRole('button', { name: 'Resume' }));
    await waitFor(() => { expect(callsOf(state, 'resume_downloads')[0]?.args).toEqual({ ids: ['l'] }); });
    expect(callsOf(state, 'set_download_login')[0]?.args).toEqual({
      id: 'l',
      login: { credentials: { userName: 'alice', password: 'secret' }, sshKey: null, headers: [] },
    });
  });

  it('says when the transfer engine isn’t running, so waiting downloads aren’t mistaken for stuck ones', async () => {
    installFakeBackend({ snapshot: withDownloads([movie], { engine: { state: 'unavailable', code: 'ENGINE_MISSING' } }) });
    renderApp();
    await rowFor('movie.mp4');
    expect(screen.getByRole('status', { name: 'Transfer engine' }).textContent).toMatch(/isn’t running/);
  });
});
