import { emit } from '@tauri-apps/api/event';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from '../../app/App';
import type { AddRequest, AddResult, Capabilities, Category, DownloadRecord } from '../../contract';
import { DEFAULT_LAYOUT } from '../../layout/layoutLogic';
import { ERROR_MESSAGES } from '../../lib/errors';
import { createQueryClient } from '../../lib/queries';
import { useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';
import { callsOf, emptySnapshot, fixture, installFakeBackend, type FakeState } from '../../test/fakeBackend';

// V1's add-dialog tests (tests/unit/app.test.tsx and torrent.test.tsx), ported to V2's dialog, with
// the SFTP host-key prompt, protocols this build lacks, and taken destinations (#60).

const category = fixture('category.json') as Category;
const record = (over: Partial<DownloadRecord>): DownloadRecord => ({ ...(fixture('download-record.json') as DownloadRecord), ...over });

function renderApp() {
  return render(<App client={createQueryClient()} />);
}

/** Opens the dialog from the navigation, as a user does. */
async function openDialog(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole('button', { name: 'Add download' }));
  return screen.findByRole('dialog', { name: 'Add download' });
}

const requests = (state: FakeState) => callsOf(state, 'add_downloads').map((c) => c.args.request as AddRequest);

beforeEach(() => {
  // jsdom has no matchMedia; the window follows a light system here.
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
  });
  useUiStore.setState({ view: 'all', categoryFilter: null, layout: { ...DEFAULT_LAYOUT }, addDialog: { open: false, prefill: null } });
  useDownloadsStore.setState({ snapshot: null, error: null });
});

describe('the Add download dialog', () => {
  it('opens from the navigation and with Ctrl+N, and Cancel adds nothing', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend();
    renderApp();
    let dialog = await openDialog(user);
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => { expect(screen.queryByRole('dialog')).toBeNull(); });
    await user.keyboard('{Control>}n{/Control}');
    dialog = await screen.findByRole('dialog', { name: 'Add download' });
    expect(within(dialog).getByRole('textbox', { name: 'Links' })).toBeTruthy();
    expect(requests(state)).toEqual([]);
  });

  it('flags invalid links and submits valid ones', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend();
    renderApp();
    const dialog = await openDialog(user);
    const links = within(dialog).getByRole('textbox', { name: 'Links' });
    await user.type(links, 'not-a-link');
    expect((await within(dialog).findByText(/This is not a complete link/)).textContent).toContain('not-a-link');
    expect(within(dialog).getByRole<HTMLButtonElement>('button', { name: 'Start download' }).disabled).toBe(true);
    await user.clear(links);
    await user.type(links, 'https://example.com/a.zip{enter}https://example.com/b.zip');
    expect(within(dialog).getByRole<HTMLInputElement>('textbox', { name: 'File name' }).disabled).toBe(true);
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    const request = requests(state)[0];
    expect(request?.links).toEqual(['https://example.com/a.zip', 'https://example.com/b.zip']);
    expect(request?.startPaused).toBe(false);
    expect(request?.onConflict).toBe('ask');
    expect(request?.fileName).toBeNull();
    expect(request?.allowDuplicate).toBe(false);
    await waitFor(() => { expect(screen.queryByRole('dialog')).toBeNull(); });
    expect(await screen.findByText('2 downloads added')).toBeTruthy();
  });

  it('shows the saved name with the link’s file type and leaves extensionless links to the server', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend();
    renderApp();
    const dialog = await openDialog(user);
    const links = within(dialog).getByRole('textbox', { name: 'Links' });
    const name = () => within(dialog).getByRole<HTMLInputElement>('textbox', { name: 'File name' });
    await user.type(links, 'https://example.com/download.php?id=3');
    expect(name().placeholder).toBe('Named by the server');
    await user.clear(links);
    await user.type(links, 'https://example.com/docs/report.pdf');
    expect(name().placeholder).toBe('report.pdf');
    await user.type(name(), 'Annual report');
    expect(await within(dialog).findByText('Saved as “Annual report.pdf”')).toBeTruthy();
    await waitFor(() => {
      expect(callsOf(state, 'check_destination').some((c) => c.args.fileName === 'Annual report' && c.args.url === 'https://example.com/docs/report.pdf')).toBe(true);
    });
    await user.click(within(dialog).getByRole('button', { name: 'Add paused' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({ fileName: 'Annual report', startPaused: true });
  });

  it('asks about a taken name and renames, never replaces', async () => {
    const user = userEvent.setup();
    const check = { ...(fixture('destination-check.json') as object), fileName: 'a.zip', suggestedName: 'a (1).zip' };
    const state = installFakeBackend({
      addResults: (request) =>
        request.links.map((source) =>
          request.onConflict === 'ask'
            ? ({ source, outcome: { kind: 'conflict', check } } as AddResult)
            : { source, outcome: { kind: 'added', id: 'd-0009' } },
        ),
    });
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'https://example.com/a.zip');
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    expect(await within(dialog).findByText('A file with this name already exists')).toBeTruthy();
    expect(within(dialog).getByText(/Keep both saves it as “a \(1\)\.zip”/)).toBeTruthy();
    expect(within(dialog).queryByRole('button', { name: /Replace/ })).toBeNull();
    await user.click(within(dialog).getByRole('button', { name: 'Keep both (rename)' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(2); });
    expect(requests(state)[1]).toMatchObject({ links: ['https://example.com/a.zip'], onConflict: 'rename' });
    await waitFor(() => { expect(screen.queryByRole('dialog')).toBeNull(); });
  });

  it('warns about a taken name before anything is added', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend();
    state.destination = ({ directory, fileName, categoryId }) => ({
      directory: directory ?? '/home/user/Downloads',
      directoryExists: true,
      automaticDirectory: directory === null,
      categoryId,
      fileName: fileName ?? 'a.zip',
      fileExists: true,
      partialDownloadExists: false,
      suggestedName: 'a (1).zip',
    });
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'https://example.com/a.zip');
    expect(await within(dialog).findByText('“a.zip” is already in this folder. You’ll be asked what to do when you start.')).toBeTruthy();
  });

  it('shows why each link the native side refused wasn’t added', async () => {
    const user = userEvent.setup();
    installFakeBackend({
      addResults: (request) => [
        { source: request.links[0] ?? '', outcome: { kind: 'failed', code: 'UNSUPPORTED_SCHEME' } },
        {
          source: request.links[1] ?? '',
          outcome: { kind: 'notFetched', error: { class: 'permanent', httpStatus: 404, message: 'Not found', retryable: false, hostKey: null, confirm: null } },
        },
      ],
    });
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'https://example.com/a.zip{enter}https://example.com/album.torrent');
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    expect(await within(dialog).findByText('2 links couldn’t be added')).toBeTruthy();
    expect(within(dialog).getByText(ERROR_MESSAGES.UNSUPPORTED_SCHEME)).toBeTruthy();
    expect(within(dialog).getByText('Not found')).toBeTruthy();
  });

  it('shows the category and folder a download will get, and creates a missing chosen folder only when asked', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({ categories: [category] });
    state.destination = ({ url, directory, fileName, categoryId }) => {
      // As Rust does: with no typed name, the link's own names the file.
      const name = fileName ?? url.split('/').pop() ?? '';
      const chosen = categoryId ?? (name.endsWith('.mp4') ? category.id : null);
      return {
        directory: directory ?? (chosen ? (category.directory ?? '') : '/home/user/Downloads'),
        directoryExists: directory === null,
        automaticDirectory: directory === null,
        categoryId: chosen,
        fileName,
        fileExists: false,
        partialDownloadExists: false,
        suggestedName: null,
      };
    };
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'https://example.com/lesson.mp4');
    expect(await within(dialog).findByRole('option', { name: `Automatic: ${category.name}` })).toBeTruthy();
    expect(await within(dialog).findByText('Saves to /home/user/Courses')).toBeTruthy();
    // An automatic folder is the settings' or the category's: not an error here.
    expect(within(dialog).queryByText(/This folder doesn’t exist/)).toBeNull();
    expect(within(dialog).queryByRole('checkbox', { name: 'Create this folder' })).toBeNull();
    await user.type(within(dialog).getByRole('textbox', { name: 'Save to' }), '/elsewhere');
    expect(await within(dialog).findByText(/This folder doesn’t exist/)).toBeTruthy();
    expect(within(dialog).getByRole<HTMLButtonElement>('button', { name: 'Start download' }).disabled).toBe(true);
    await user.click(within(dialog).getByRole('checkbox', { name: 'Create this folder' }));
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({ directory: '/elsewhere', createDirectory: true, categoryId: null });
  });

  it('chooses the folder with the system’s chooser', async () => {
    const user = userEvent.setup();
    installFakeBackend({ chosenDirectory: '/home/user/Picked' });
    renderApp();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole('button', { name: 'Browse' }));
    await waitFor(() => { expect(within(dialog).getByRole<HTMLInputElement>('textbox', { name: 'Save to' }).value).toBe('/home/user/Picked'); });
  });

  it('prefills what the browser sent', async () => {
    const state = installFakeBackend();
    renderApp();
    await screen.findByRole('button', { name: 'Add download' });
    act(() => {
      useUiStore.getState().openAdd({
        links: 'https://cdn.test/file.iso',
        fileName: 'file.iso',
        referrer: 'https://site.test/page',
        fromBrowser: { referrer: 'https://site.test/page', fileSize: 2048, mime: 'application/x-iso9660-image' },
      });
    });
    const dialog = await screen.findByRole('dialog', { name: 'Add download' });
    expect(within(dialog).getByText('From https://site.test/page')).toBeTruthy();
    expect(within(dialog).getByRole<HTMLTextAreaElement>('textbox', { name: 'Links' }).value).toBe('https://cdn.test/file.iso');
    expect(within(dialog).getByRole<HTMLInputElement>('textbox', { name: 'File name' }).value).toBe('file.iso');
    await userEvent.setup().click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({ referrer: 'https://site.test/page', fileName: 'file.iso' });
  });

  it('refuses a protocol this build can’t download, saying why, before sending anything', async () => {
    const user = userEvent.setup();
    // The sample engine lists no SFTP.
    const state = installFakeBackend({ capabilities: fixture('capabilities.json') as Capabilities });
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'sftp://files.example/a.tar');
    expect(await within(dialog).findByText(/This version of Velocity can’t download over SFTP yet\./)).toBeTruthy();
    expect(within(dialog).getByRole<HTMLButtonElement>('button', { name: 'Start download' }).disabled).toBe(true);
    expect(requests(state)).toEqual([]);
  });

  it('adds an SFTP link with a key login', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({ chosenKey: '/home/user/.ssh/id_ed25519' });
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'sftp://files.example/a.tar');
    await user.click(within(dialog).getByRole('button', { name: 'Advanced options' }));
    await user.type(within(dialog).getByRole('textbox', { name: 'User name' }), 'me');
    await user.click(within(dialog).getByRole('button', { name: 'Choose a key' }));
    await waitFor(() => { expect(within(dialog).getByRole<HTMLInputElement>('textbox', { name: 'SSH private key' }).value).toBe('/home/user/.ssh/id_ed25519'); });
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({
      credentials: { userName: 'me', password: '' },
      sshKey: { path: '/home/user/.ssh/id_ed25519', passphrase: null },
    });
  });

  it('warns that a plain FTP link isn’t encrypted, and can insist on encryption', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend();
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'ftp://ftp.example.org/pub/a.iso');
    expect(within(dialog).getByText(/This FTP download isn’t encrypted/)).toBeTruthy();
    await user.click(within(dialog).getByRole('checkbox', { name: 'Require encryption (FTPS)' }));
    expect(within(dialog).queryByText(/This FTP download isn’t encrypted/)).toBeNull();
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]?.requireEncryption).toBe(true);
  });

  it('sends the advanced options, and refuses headers it can’t read', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend();
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'https://example.com/a.zip');
    await user.click(within(dialog).getByRole('button', { name: 'Advanced options' }));
    await user.type(within(dialog).getByRole('spinbutton', { name: 'Connections' }), '3');
    await user.type(within(dialog).getByRole('spinbutton', { name: 'Speed limit (KiB/s)' }), '256');
    await user.type(within(dialog).getByRole('textbox', { name: 'Referrer' }), 'https://example.com/page');
    await user.click(within(dialog).getByRole('textbox', { name: 'Checksum' }));
    await user.paste(`sha-256=${'ab'.repeat(32)}`);
    await user.type(within(dialog).getByRole('textbox', { name: 'User name' }), 'me');
    await user.type(within(dialog).getByLabelText('Password'), 'pw');
    const headers = within(dialog).getByRole('textbox', { name: 'Extra headers' });
    await user.type(headers, 'Cookie a=b');
    expect(within(dialog).getByText('Line 1 must look like "Name: value".')).toBeTruthy();
    expect(within(dialog).getByRole<HTMLButtonElement>('button', { name: 'Start download' }).disabled).toBe(true);
    await user.clear(headers);
    await user.type(headers, 'Cookie: a=b');
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({
      connections: 3,
      speedLimitKib: 256,
      referrer: 'https://example.com/page',
      headers: [{ name: 'Cookie', value: 'a=b' }],
      checksum: { algorithm: 'sha256', hex: 'ab'.repeat(32) },
      credentials: { userName: 'me', password: 'pw' },
      sshKey: null,
    });
  });
});

describe('duplicates', () => {
  it('warns about a link added before, and can add it anyway', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({
      addResults: (request) =>
        request.links.map((source) =>
          request.allowDuplicate
            ? { source, outcome: { kind: 'added', id: 'd-0009' } }
            : { source, outcome: { kind: 'duplicate', duplicate: { existingId: 'd-0001', name: 'a.zip', status: 'completed', inList: false, canAddAnyway: true } } },
        ),
    });
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'https://example.com/a.zip');
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(await within(dialog).findByText('This was added before')).toBeTruthy();
    expect(within(dialog).getByText(/a\.zip · In your history/)).toBeTruthy();
    await user.click(within(dialog).getByRole('button', { name: 'Add anyway' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(2); });
    expect(requests(state)[1]).toMatchObject({ links: ['https://example.com/a.zip'], allowDuplicate: true });
    await waitFor(() => { expect(screen.queryByRole('dialog')).toBeNull(); });
  });

  it('explains a torrent still in the list, with no Add anyway', async () => {
    const user = userEvent.setup();
    installFakeBackend({
      addResults: (request) =>
        request.links.map((source) => ({
          source,
          outcome: { kind: 'duplicate', duplicate: { existingId: 'd-0001', name: 'album', status: 'downloading', inList: true, canAddAnyway: false } },
        })),
    });
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa');
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    expect(await within(dialog).findByText('This torrent is already in your list')).toBeTruthy();
    expect(within(dialog).getByText(/can’t run two copies of the same torrent/)).toBeTruthy();
    expect(within(dialog).queryByRole('button', { name: 'Add anyway' })).toBeNull();
    expect(within(dialog).getByRole('button', { name: 'Done' })).toBeTruthy();
  });

  it('shows the download already in the list (#59)', async () => {
    const user = userEvent.setup();
    useUiStore.setState({ view: 'completed' });
    useDownloadsStore.setState({ search: 'something else' });
    installFakeBackend({
      snapshot: { ...emptySnapshot(), downloads: [record({ id: 'd-0001', fileName: 'album', status: 'downloading' }), record({ id: 'd-0002', fileName: 'other' })] },
      addResults: (request) =>
        request.links.map((source) => ({
          source,
          outcome: { kind: 'duplicate', duplicate: { existingId: 'd-0001', name: 'album', status: 'downloading', inList: true, canAddAnyway: false } },
        })),
    });
    renderApp();
    const dialog = await openDialog(user);
    await user.type(within(dialog).getByRole('textbox', { name: 'Links' }), 'magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa');
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await user.click(await within(dialog).findByRole('button', { name: 'Show download' }));
    await waitFor(() => { expect(screen.queryByRole('dialog')).toBeNull(); });
    // The whole list, unfiltered, with that download selected.
    const grid = await screen.findByRole('grid', { name: 'Downloads' });
    const row = (await within(grid).findByText('album')).closest('[role="row"]');
    expect(row?.getAttribute('aria-selected')).toBe('true');
    expect(within(grid).getByText('other')).toBeTruthy();
  });
});

describe('torrents', () => {
  it('adds .torrent files chosen in the dialog, waiting for a file choice when asked', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({ chosenTorrents: ['/t/album.torrent', '/t/other.torrent'] });
    renderApp();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole('button', { name: 'Choose .torrent files…' }));
    const files = await within(dialog).findByRole('list', { name: 'Torrent files' });
    expect(within(files).getByText('album.torrent')).toBeTruthy();
    expect(within(files).getByText('other.torrent')).toBeTruthy();
    expect(within(dialog).getByRole<HTMLInputElement>('textbox', { name: 'File name' }).disabled).toBe(true);
    await user.click(within(dialog).getByRole('checkbox', { name: 'Choose files before downloading' }));
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]).toMatchObject({ links: [], torrentFiles: ['/t/album.torrent', '/t/other.torrent'], chooseFiles: true });
  });

  it('accepts magnet links and .torrent links, and sends no torrent choice for ordinary links', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend();
    renderApp();
    const dialog = await openDialog(user);
    const links = within(dialog).getByRole('textbox', { name: 'Links' });
    await user.type(links, 'https://example.com/a.zip');
    expect(within(dialog).queryByRole('checkbox', { name: 'Choose files before downloading' })).toBeNull();
    await user.type(links, '{enter}https://example.com/album.torrent');
    expect(within(dialog).getByText(/fetches the \.torrent file first/)).toBeTruthy();
    await user.type(links, '{enter}magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa');
    expect(within(dialog).queryByText(/can’t be added/)).toBeNull();
    expect(within(dialog).getByRole('checkbox', { name: 'Choose files before downloading' })).toBeTruthy();
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    await waitFor(() => { expect(requests(state)).toHaveLength(1); });
    expect(requests(state)[0]?.links).toHaveLength(3);
    expect(requests(state)[0]?.chooseFiles).toBe(false);
  });

  it('explains a torrent whose name is taken, and offers no rename', async () => {
    const user = userEvent.setup();
    const check = { ...(fixture('destination-check.json') as object), fileName: 'album', suggestedName: null };
    installFakeBackend({
      chosenTorrents: ['/t/album.torrent'],
      addResults: (request) => request.torrentFiles.map((source) => ({ source, outcome: { kind: 'conflict', check } }) as AddResult),
    });
    renderApp();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole('button', { name: 'Choose .torrent files…' }));
    await within(dialog).findByRole('list', { name: 'Torrent files' });
    await user.click(within(dialog).getByRole('button', { name: 'Start download' }));
    expect(await within(dialog).findByText(/A torrent keeps the names in its metadata/)).toBeTruthy();
    expect(within(dialog).queryByRole('button', { name: 'Keep both (rename)' })).toBeNull();
    expect(within(dialog).getByRole('button', { name: 'Skip' })).toBeTruthy();
  });

  it('opens with .torrent files dropped on the window', async () => {
    installFakeBackend();
    renderApp();
    await screen.findByRole('button', { name: 'Add download' });
    await act(() => emit('torrents-dropped', { paths: ['/t/dropped.torrent'], ignored: 1 }));
    const dialog = await screen.findByRole('dialog', { name: 'Add download' });
    expect(within(within(dialog).getByRole('list', { name: 'Torrent files' })).getByText('dropped.torrent')).toBeTruthy();
  });

  it('says so when nothing dropped was a .torrent file', async () => {
    installFakeBackend();
    renderApp();
    await screen.findByRole('button', { name: 'Add download' });
    await act(() => emit('torrents-dropped', { paths: [], ignored: 2 }));
    expect(await screen.findByText('Only .torrent files can be dropped on the window.')).toBeTruthy();
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('the SFTP host key', () => {
  const hostKey = (pinned: string | null) =>
    record({
      id: 'd-0007',
      kind: 'sftp',
      sourceUrl: 'sftp://files.example:2222/a.tar',
      status: 'failed',
      error: {
        class: 'permanent',
        httpStatus: null,
        message: 'The server’s host key isn’t trusted yet.',
        retryable: false,
        hostKey: { host: 'files.example', port: 2222, fingerprint: 'SHA256:abc123', pinned },
        confirm: null,
      },
    });

  it('asks when a download stops at an unknown key, and trusting it retries the download', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [hostKey(null)] } });
    renderApp();
    const ask = await screen.findByRole('dialog', { name: 'Trust this server?' });
    expect(within(ask).getByText('files.example:2222')).toBeTruthy();
    expect(within(ask).getByText('SHA256:abc123')).toBeTruthy();
    await user.click(within(ask).getByRole('button', { name: 'Trust and continue' }));
    await waitFor(() => { expect(callsOf(state, 'retry_downloads')).toHaveLength(1); });
    expect(callsOf(state, 'trust_host_key')[0]?.args).toEqual({ host: 'files.example', port: 2222, fingerprint: 'SHA256:abc123' });
    expect(callsOf(state, 'retry_downloads')[0]?.args).toEqual({ ids: ['d-0007'] });
    await waitFor(() => { expect(screen.queryByRole('dialog')).toBeNull(); });
  });

  it('Cancel trusts nothing and doesn’t ask again for that download', async () => {
    const user = userEvent.setup();
    const state = installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [hostKey(null)] } });
    renderApp();
    const ask = await screen.findByRole('dialog', { name: 'Trust this server?' });
    await user.click(within(ask).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => { expect(screen.queryByRole('dialog')).toBeNull(); });
    await act(() => emit('downloads-snapshot', { ...emptySnapshot(), downloads: [hostKey(null)] }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(callsOf(state, 'trust_host_key')).toEqual([]);
  });

  it('a changed key is explained, never offered for trust', async () => {
    installFakeBackend({ snapshot: { ...emptySnapshot(), downloads: [hostKey('SHA256:old')] } });
    renderApp();
    const ask = await screen.findByRole('dialog', { name: 'The server’s key has changed' });
    expect(within(ask).getByText('SHA256:abc123')).toBeTruthy();
    expect(within(ask).queryByRole('button', { name: 'Trust and continue' })).toBeNull();
    expect(within(ask).getByRole('button', { name: 'Done' })).toBeTruthy();
  });
});
