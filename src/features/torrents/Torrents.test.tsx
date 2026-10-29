import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from '../../app/App';
import type { AddRequest, DownloadRecord, PeerStatus, TorrentDetails, TorrentPreview, TrackerStatus } from '../../contract';
import { DEFAULT_LAYOUT } from '../../layout/layoutLogic';
import { createQueryClient } from '../../lib/queries';
import { INITIAL_LIST, useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';
import { callsOf, emptySnapshot, fixture, installFakeBackend, type FakeState } from '../../test/fakeBackend';
import { makeRecord } from '../../test/records';

// V1's torrent tests (tests/unit/torrent.test.tsx and torrentControls.test.tsx), ported to V2 (#63). Those for aria2
// builds have no V2 counterpart: V2 always runs libtorrent (ADR-0014). The BitTorrent settings' are #30's (PO
// decision on #63), and duplicates, conflicts and the magnet's "Choose files" box were #60's.

function renderApp() {
  return render(<App client={createQueryClient()} />);
}

const sample = fixture('torrent.json') as { details: TorrentDetails; preview: TorrentPreview; trackers: TrackerStatus[]; peers: PeerStatus[] };
const HASH = '0123456789abcdef0123456789abcdef01234567';
const preview = (over: Partial<TorrentPreview> = {}): TorrentPreview => ({ ...sample.preview, duplicate: null, ...over });
const details = (over: Partial<TorrentDetails> = {}): TorrentDetails => ({ ...sample.details, chosen: null, seeding: { mode: 'never' }, uploadLimitBps: 0, ...over });
const torrent = (over: Partial<DownloadRecord>): DownloadRecord =>
  makeRecord({ kind: 'torrent', fileName: 'album', requestedConnections: null, speedLimitBps: null, torrent: { peers: 2, seeds: 1, uploadedBytes: 0, seedingSecs: 0, seedQueued: false }, ...over });
const requests = (state: FakeState) => callsOf(state, 'add_downloads').map((c) => c.args.request as AddRequest);
const panel = () => screen.findByRole('complementary', { name: 'Download details' });

/** Opens the details of `id` directly: a double-click on a finished download opens its file. */
async function openDetails(id: string) {
  renderApp();
  await screen.findByRole('grid', { name: 'Downloads' });
  act(() => { useUiStore.getState().showDetails(id); });
  return panel();
}

beforeEach(() => {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
  });
  useUiStore.setState({ view: 'all', categoryFilter: null, layout: { ...DEFAULT_LAYOUT }, addDialog: { open: false, prefill: null }, detailsId: null, chooseFilesId: null });
  useDownloadsStore.setState({ snapshot: null, error: null, ...INITIAL_LIST });
});

describe('adding torrents (V1)', () => {
  let state: FakeState;
  beforeEach(() => {
    state = installFakeBackend({ preview: () => preview() });
  });

  it('previews a .torrent file and adds only the chosen files', async () => {
    const user = userEvent.setup();
    renderApp();
    await screen.findByText('No downloads yet');
    act(() => { useUiStore.getState().openAdd({ torrentFiles: ['/t/album.torrent'] }); });
    const tree = await screen.findByRole('tree', { name: 'Files in this torrent' });
    expect(within(tree).getByText('a.bin')).toBeTruthy();
    expect(screen.getByText(/3 of 3 files/)).toBeTruthy();
    await user.click(within(tree).getByRole('checkbox', { name: 'Download sub' }));
    expect(screen.getByText(/2 of 3 files/)).toBeTruthy();
    await user.click(screen.getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({ torrentFiles: ['/t/album.torrent'], links: [], fileChoices: [{ torrentFile: '/t/album.torrent', files: [1, 3] }], seeding: { mode: 'never' } });
  });

  it('says which BitTorrent versions a torrent has, and its hashes', async () => {
    renderApp();
    await screen.findByText('No downloads yet');
    act(() => { useUiStore.getState().openAdd({ torrentFiles: ['/t/album.torrent'] }); });
    expect(await screen.findByText('Hybrid (v1 and v2)')).toBeTruthy();
    expect(screen.getByText(HASH)).toBeTruthy();
  });

  it('refuses to start with no files selected', async () => {
    const user = userEvent.setup();
    renderApp();
    await screen.findByText('No downloads yet');
    act(() => { useUiStore.getState().openAdd({ torrentFiles: ['/t/album.torrent'] }); });
    const tree = await screen.findByRole('tree', { name: 'Files in this torrent' });
    await user.click(within(tree).getByRole('checkbox', { name: 'Download album' }));
    await user.click(screen.getByRole('button', { name: 'Start download' }));
    expect(await screen.findByText('Choose at least one file.')).toBeTruthy();
    expect(requests(state)).toHaveLength(0);
  });

  it('adds several .torrent files, each with every file', async () => {
    const user = userEvent.setup();
    renderApp();
    await screen.findByText('No downloads yet');
    act(() => { useUiStore.getState().openAdd({ torrentFiles: ['/t/one.torrent', '/t/two.torrent'] }); });
    expect(await screen.findByText('one.torrent')).toBeTruthy();
    expect(screen.queryByRole('tree')).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({ torrentFiles: ['/t/one.torrent', '/t/two.torrent'], fileChoices: [] });
  });

  it('warns before adding a torrent already in the list or the history', async () => {
    state.preview = () => preview({ duplicate: { existingId: 'old', name: 'album', status: 'completed', inList: false, canAddAnyway: true } });
    renderApp();
    await screen.findByText('No downloads yet');
    act(() => { useUiStore.getState().openAdd({ torrentFiles: ['/t/album.torrent'] }); });
    expect(await screen.findByText(/already in your downloads or history/)).toBeTruthy();
  });

  it('adds .torrent files chosen from the Add dialog, with seeding and an upload limit', async () => {
    state.chosenTorrents = ['/t/album.torrent'];
    const user = userEvent.setup();
    renderApp();
    await screen.findByText('No downloads yet');
    act(() => { useUiStore.getState().openAdd(); });
    await user.click(await screen.findByRole('button', { name: /Choose .torrent files/ }));
    expect(await screen.findByText('3 of 3 files · 3.1 MiB')).toBeTruthy();
    await user.selectOptions(screen.getByLabelText('Seeding after download'), 'Until a share ratio');
    const ratio = screen.getByLabelText<HTMLInputElement>('Share ratio');
    await user.clear(ratio);
    await user.type(ratio, '2');
    await user.type(screen.getByLabelText('Upload limit (KiB/s)'), '64');
    await user.click(screen.getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    // Every file chosen is no choice: the torrent gets every file.
    expect(requests(state)[0]).toMatchObject({ torrentFiles: ['/t/album.torrent'], links: [], fileChoices: [], seeding: { mode: 'ratio', ratio: 2 }, uploadLimitKib: 64 });
  });

  it('sends no torrent options for ordinary links', async () => {
    const user = userEvent.setup();
    renderApp();
    await screen.findByText('No downloads yet');
    act(() => { useUiStore.getState().openAdd(); });
    await user.type(await screen.findByLabelText('Links'), 'https://example.com/a.zip');
    expect(screen.queryByText('Seeding after download')).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({ fileChoices: [], seeding: null, uploadLimitKib: 0 });
  });

  it('opens the Add dialog for magnet links pasted outside a text field', async () => {
    renderApp();
    await screen.findByText('No downloads yet');
    const paste = new Event('paste', { bubbles: true }) as Event & { clipboardData: { getData: () => string } };
    paste.clipboardData = { getData: () => `magnet:?xt=urn:btih:${HASH}` };
    act(() => { document.body.dispatchEvent(paste); });
    expect((await screen.findByLabelText<HTMLTextAreaElement>('Links')).value).toBe(`magnet:?xt=urn:btih:${HASH}`);
  });
});

describe('torrents in the list (V1)', () => {
  it('lets the user choose files once metadata arrives', async () => {
    const d = torrent({ id: 'm', kind: 'magnet', status: 'choosingFiles' });
    const state = installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [d] }, torrentDetails: () => details() });
    const user = userEvent.setup();
    renderApp();
    const row = (await within(await screen.findByRole('grid', { name: 'Downloads' })).findByText('album')).closest<HTMLElement>('[role="row"]');
    if (!row) throw new Error('no row');
    expect(within(row).getByText('Choose files')).toBeTruthy();
    await user.dblClick(row);
    const dialog = await screen.findByRole('dialog', { name: 'Choose files to download' });
    const tree = await within(dialog).findByRole('tree');
    await user.click(within(tree).getByRole('checkbox', { name: 'Download a.bin' }));
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(callsOf(state, 'choose_torrent_files')[0]?.args).toEqual({ id: 'm', files: [2, 3], deleteDropped: false }); });
  });

  it('shows both info hashes and the version in the details', async () => {
    installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'h', status: 'downloading' })] }, torrentDetails: () => details() });
    const p = await openDetails('h');
    expect(await within(p).findByText('ab'.repeat(32))).toBeTruthy();
    expect(within(p).getByText('Info hash (v1)')).toBeTruthy();
    expect(within(p).getByText('Info hash (v2)')).toBeTruthy();
    expect(within(p).getByText('Hybrid (v1 and v2)')).toBeTruthy();
  });

  it('shows seeding torrents in their own view and stops seeding', async () => {
    const state = installFakeBackend({
      snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 's', fileName: 'shared', status: 'seeding' }), makeRecord({ id: 'p', fileName: 'plain.zip', status: 'completed' })] },
      torrentDetails: () => details(),
    });
    const user = userEvent.setup();
    renderApp();
    await user.click(await screen.findByRole('button', { name: /^Seeding/ }));
    const g = await screen.findByRole('grid', { name: 'Downloads' });
    await waitFor(() => { expect(within(g).queryByText('plain.zip')).toBeNull(); });
    act(() => { useUiStore.getState().showDetails('s'); });
    await user.click(within(await panel()).getByRole('button', { name: 'Stop seeding' }));
    await waitFor(() => { expect(callsOf(state, 'set_torrent_seeding')[0]?.args).toEqual({ id: 's', seeding: { mode: 'never' } }); });
  });

  it('doesn’t flash “Waiting for peers” when the user pauses a torrent', async () => {
    // The manager keeps a pause against a late status (#63's core test); the list shows what the snapshot says.
    installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'w', status: 'paused' })] }, torrentDetails: () => details() });
    renderApp();
    const row = (await within(await screen.findByRole('grid', { name: 'Downloads' })).findByText('album')).closest<HTMLElement>('[role="row"]');
    if (!row) throw new Error('no row');
    expect(within(row).queryByText('Waiting for peers')).toBeNull();
    expect(within(row).getByText('Paused')).toBeTruthy();
  });
});

describe('trackers, peers and limits in the details (V1)', () => {
  it('shows each tracker’s state and re-announces', async () => {
    const state = installFakeBackend({
      snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'l', status: 'stalled' })] },
      torrentDetails: () => details(),
      trackers: () => [
        { url: 'http://127.0.0.1:9/announce', tier: 0, state: 'error', message: 'torrent not registered', fails: 3, nextAnnounceSecs: 40, seeds: null, peers: null },
        { url: 'udp://tracker.test:6969', tier: 1, state: 'working', message: null, fails: 0, nextAnnounceSecs: 1700, seeds: 4, peers: 9 },
      ],
    });
    const user = userEvent.setup();
    const p = await openDetails('l');
    const list = await within(p).findByRole('list', { name: 'Trackers' });
    const items = within(list).getAllByRole('listitem');
    expect(items).toHaveLength(2);
    expect(items[0]?.textContent).toContain('Error');
    expect(items[0]?.textContent).toContain('torrent not registered');
    expect(items[1]?.textContent).toContain('Working');
    expect(items[1]?.textContent).toContain('4 seeds, 9 peers');
    await user.click(within(p).getByRole('button', { name: 'Re-announce' }));
    await waitFor(() => { expect(callsOf(state, 'reannounce_torrent')[0]?.args).toEqual({ id: 'l' }); });
  });

  it('lists the connected peers, and asks for them only while the details are open', async () => {
    const state = installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'l', status: 'downloading' })] }, torrentDetails: () => details(), peers: () => sample.peers });
    renderApp();
    await screen.findByRole('grid', { name: 'Downloads' });
    await new Promise((r) => setTimeout(r, 50));
    expect(callsOf(state, 'get_torrent_peers')).toHaveLength(0);
    act(() => { useUiStore.getState().showDetails('l'); });
    const peers = await within(await panel()).findByRole('list', { name: 'Peers' });
    expect(peers.textContent).toContain('203.0.113.7:51413');
    expect(peers.textContent).toContain('Transmission 4.0');
  });

  it('sets the upload limit of the torrent shown', async () => {
    const state = installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'u', status: 'downloading' })] }, torrentDetails: () => details() });
    const user = userEvent.setup();
    const p = await openDetails('u');
    const limit = await within(p).findByLabelText<HTMLInputElement>(/Upload limit/);
    await user.clear(limit);
    await user.type(limit, '128');
    await user.tab();
    await waitFor(() => { expect(callsOf(state, 'set_torrent_upload_limit').at(-1)?.args).toEqual({ id: 'u', kib: 128 }); });
  });

  it('says why the trackers can’t be shown while the helper doesn’t have the torrent', async () => {
    installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'q', status: 'queued' })] }, torrentDetails: () => details(), failures: { get_torrent_trackers: 'INVALID_STATE' } });
    const p = await openDetails('q');
    expect(await within(p).findByText(/Trackers and peers show once the torrent runs/)).toBeTruthy();
  });
});

describe('changing files of a running torrent (V1)', () => {
  const chosen = details({ chosen: [1, 2] });

  it('applies a new selection and keeps removed files unless asked', async () => {
    const state = installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'c', status: 'downloading' })] }, torrentDetails: () => chosen });
    const user = userEvent.setup();
    const p = await openDetails('c');
    const tree = await within(p).findByRole('tree');
    expect(within(p).queryByRole('button', { name: 'Apply file changes' })).toBeNull();
    await user.click(within(tree).getByRole('checkbox', { name: 'Download b.bin' }));
    await user.click(within(tree).getByRole('checkbox', { name: 'Download c.txt' }));
    expect(within(p).getByRole<HTMLInputElement>('checkbox', { name: 'Delete files I removed' }).checked).toBe(false);
    await user.click(within(p).getByRole('button', { name: 'Apply file changes' }));
    await waitFor(() => { expect(callsOf(state, 'choose_torrent_files')[0]?.args).toEqual({ id: 'c', files: [1, 3], deleteDropped: false }); });
  });

  it('can delete removed files and refuses an empty selection', async () => {
    const state = installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'c', status: 'downloading' })] }, torrentDetails: () => chosen });
    const user = userEvent.setup();
    const p = await openDetails('c');
    const tree = await within(p).findByRole('tree');
    // Partly selected, then all, then none.
    await user.click(within(tree).getByRole('checkbox', { name: 'Download album' }));
    await user.click(within(tree).getByRole('checkbox', { name: 'Download album' }));
    expect(within(p).getByRole<HTMLButtonElement>('button', { name: 'Apply file changes' }).disabled).toBe(true);
    await user.click(within(tree).getByRole('checkbox', { name: 'Download a.bin' }));
    await user.click(within(p).getByRole('checkbox', { name: 'Delete files I removed' }));
    await user.click(within(p).getByRole('button', { name: 'Apply file changes' }));
    await waitFor(() => { expect(callsOf(state, 'choose_torrent_files')[0]?.args).toEqual({ id: 'c', files: [1], deleteDropped: true }); });
  });

  it('keeps the file list read-only once a torrent has finished', async () => {
    installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [torrent({ id: 'f', status: 'completed' })] }, torrentDetails: () => chosen });
    const p = await openDetails('f');
    const tree = await within(p).findByRole('tree');
    expect(within(tree).getByRole<HTMLInputElement>('checkbox', { name: 'Download b.bin' }).disabled).toBe(true);
  });
});

describe('the seeding queue (V1)', () => {
  it('shows a queued torrent as waiting to seed and can take it out of the queue', async () => {
    const d = torrent({ id: 'q', status: 'completed', torrent: { peers: 0, seeds: 0, uploadedBytes: 0, seedingSecs: 0, seedQueued: true } });
    const state = installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [d] }, torrentDetails: () => details({ seeding: { mode: 'unlimited' } }) });
    const user = userEvent.setup();
    const p = await openDetails('q');
    const row = (await within(screen.getByRole('grid', { name: 'Downloads' })).findByText('album')).closest<HTMLElement>('[role="row"]');
    if (!row) throw new Error('no row');
    expect(within(row).getByText('Waiting to seed')).toBeTruthy();
    expect(await within(p).findByText(/Waiting for a free seeding slot/)).toBeTruthy();
    await user.click(within(p).getByRole('button', { name: 'Leave seeding queue' }));
    await waitFor(() => { expect(callsOf(state, 'set_torrent_seeding')[0]?.args).toEqual({ id: 'q', seeding: { mode: 'never' } }); });
  });
});
