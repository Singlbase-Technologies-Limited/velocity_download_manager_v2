import { emit } from '@tauri-apps/api/event';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import type { Category, DownloadRecord, ShellSettings } from '../contract';
import { DEFAULT_LAYOUT, LAYOUT } from '../layout/layoutLogic';
import { ERROR_MESSAGES } from '../lib/errors';
import { createQueryClient } from '../lib/queries';
import { useDownloadsStore } from '../stores/downloads';
import { useUiStore } from '../stores/ui';
import { callsOf, emptySnapshot, firstRun, fixture, installFakeBackend, setUp } from '../test/fakeBackend';
import { App } from './App';

function renderApp() {
  return render(<App client={createQueryClient()} />);
}

/** The OS colour scheme, as `matchMedia` reports it, with its change listeners. */
function osScheme(dark: boolean) {
  const listeners = new Set<(e: { matches: boolean }) => void>();
  const query = { matches: dark };
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: () => ({
      get matches() {
        return query.matches;
      },
      addEventListener: (_: string, l: (e: { matches: boolean }) => void) => listeners.add(l),
      removeEventListener: (_: string, l: (e: { matches: boolean }) => void) => listeners.delete(l),
    }),
  });
  return {
    set(next: boolean) {
      query.matches = next;
      act(() => {
        for (const l of listeners) l({ matches: next });
      });
    },
  };
}

function setViewport(width: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: width });
}

const record = (over: Partial<DownloadRecord>): DownloadRecord => ({ ...(fixture('download-record.json') as DownloadRecord), ...over });
const isDark = () => document.documentElement.classList.contains('dark');
/** The main area: category names show in the sidebar too. */
const main = () => within(screen.getByRole('main', { name: 'Main content' }));

beforeEach(() => {
  // The stores are module singletons: each test starts as a fresh process would.
  useUiStore.setState({ view: 'all', categoryFilter: null, layout: { ...DEFAULT_LAYOUT }, detailsId: null });
  useDownloadsStore.setState({ snapshot: null, error: null });
  document.documentElement.classList.remove('dark');
  osScheme(false);
  setViewport(1366);
});

describe('starting', () => {
  it('refuses a native side built for another contract, saying so', async () => {
    installFakeBackend({ appInfo: { name: 'Velocity', version: '0.1.0', commandApiVersion: 1 } });
    renderApp();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(/contract version 1, but this window expects/);
  });

  it('says why when the settings can’t be read', async () => {
    installFakeBackend({ failures: { get_shell_settings: 'DATABASE' } });
    renderApp();
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Error code: DATABASE');
  });
});

describe('the first-run setup', () => {
  it('offers the default folder, and finishing it saves the chosen one and opens the window', async () => {
    const user = userEvent.setup();
    const backend = installFakeBackend({ settings: firstRun(), chosenDirectory: '/data/Velocity' });
    renderApp();
    const folder = await screen.findByRole('textbox', { name: 'Download folder' });
    expect((folder as HTMLInputElement).value).toBe('/home/user/Downloads');
    await user.click(screen.getByRole('button', { name: 'Browse' }));
    await waitFor(() => { expect((folder as HTMLInputElement).value).toBe('/data/Velocity'); });
    expect(callsOf(backend, 'choose_directory')[0]?.args).toEqual({ initial: '/home/user/Downloads' });
    await user.click(screen.getByRole('button', { name: 'Get started' }));
    expect(await screen.findByRole('navigation', { name: 'Primary' })).toBeTruthy();
    expect(callsOf(backend, 'complete_onboarding').map((c) => c.args)).toEqual([{ directory: '/data/Velocity' }]);
  });

  it('points to the Browser settings for the extension, as an optional step (#65)', async () => {
    installFakeBackend({ settings: firstRun() });
    renderApp();
    expect(await screen.findByRole('heading', { name: '3. Optional: the browser extension' })).toBeTruthy();
    expect(screen.getByText(/register Chrome, Edge, Brave or Firefox in Settings, Browser/)).toBeTruthy();
  });

  it('shows why the folder was refused and stays open', async () => {
    const user = userEvent.setup();
    installFakeBackend({ settings: firstRun(), failures: { complete_onboarding: 'DESTINATION_MISSING' } });
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Get started' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toBe(ERROR_MESSAGES.DESTINATION_MISSING);
    expect(screen.queryByRole('navigation', { name: 'Primary' })).toBeNull();
  });

  it('can’t be finished without a folder', async () => {
    const user = userEvent.setup();
    installFakeBackend({ settings: { ...firstRun(), defaultDirectory: null } });
    renderApp();
    const start = await screen.findByRole('button', { name: 'Get started' });
    expect((start as HTMLButtonElement).disabled).toBe(true);
    await user.type(screen.getByRole('textbox', { name: 'Download folder' }), '/srv/files');
    expect((start as HTMLButtonElement).disabled).toBe(false);
  });

  it('shows whether the transfer engine is running', async () => {
    installFakeBackend({ settings: firstRun(), snapshot: { ...emptySnapshot(), engine: { state: 'unavailable', code: 'ENGINE_MISSING' } } });
    renderApp();
    const status = await screen.findByRole('status', { name: 'Transfer engine' });
    await waitFor(() => { expect(status.textContent).toContain(ERROR_MESSAGES.ENGINE_MISSING); });
  });
});

describe('the theme', () => {
  const withTheme = (theme: ShellSettings['appearance']['theme']): ShellSettings => ({ ...setUp(), appearance: { ...setUp().appearance, theme } });

  it('is dark or light as saved, whatever the system uses', async () => {
    const os = osScheme(true);
    installFakeBackend({ settings: withTheme('light') });
    const { unmount } = renderApp();
    await screen.findByRole('navigation', { name: 'Primary' });
    expect(isDark()).toBe(false);
    unmount();
    os.set(false);
    installFakeBackend({ settings: withTheme('dark') });
    renderApp();
    await screen.findByRole('navigation', { name: 'Primary' });
    expect(isDark()).toBe(true);
  });

  it('follows the system, and switches when the system does', async () => {
    const os = osScheme(false);
    installFakeBackend({ settings: withTheme('system') });
    renderApp();
    await screen.findByRole('navigation', { name: 'Primary' });
    expect(isDark()).toBe(false);
    os.set(true);
    expect(isDark()).toBe(true);
    os.set(false);
    expect(isDark()).toBe(false);
  });
});

describe('the panels', () => {
  it('a drag of the navigation is saved once, when it ends', async () => {
    const backend = installFakeBackend();
    renderApp();
    const handle = await screen.findByRole('separator', { name: 'Resize navigation' });
    expect(handle.getAttribute('aria-valuenow')).toBe('232');
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 232 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 260 });
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 280 });
    await new Promise((r) => setTimeout(r, 400));
    expect(callsOf(backend, 'set_appearance')).toEqual([]);
    fireEvent.pointerUp(handle, { pointerId: 1, clientX: 290 });
    await waitFor(() => { expect(callsOf(backend, 'set_appearance')).toHaveLength(1); });
    expect(callsOf(backend, 'set_appearance')[0]?.args).toEqual({
      appearance: { ...setUp().appearance, layout: { ...DEFAULT_LAYOUT, sidebarWidth: 290 } },
    });
    expect(handle.getAttribute('aria-valuenow')).toBe('290');
  });

  it('starts with the saved layout', async () => {
    installFakeBackend({ settings: { ...setUp(), appearance: { ...setUp().appearance, layout: { ...DEFAULT_LAYOUT, sidebarWidth: 300 } } } });
    renderApp();
    const handle = await screen.findByRole('separator', { name: 'Resize navigation' });
    await waitFor(() => { expect(handle.getAttribute('aria-valuenow')).toBe('300'); });
  });

  it('loading the saved layout doesn’t save it again', async () => {
    const backend = installFakeBackend({ settings: { ...setUp(), appearance: { ...setUp().appearance, layout: { ...DEFAULT_LAYOUT, sidebarWidth: 300 } } } });
    renderApp();
    const handle = await screen.findByRole('separator', { name: 'Resize navigation' });
    await waitFor(() => { expect(handle.getAttribute('aria-valuenow')).toBe('300'); });
    await new Promise((r) => setTimeout(r, 400));
    expect(callsOf(backend, 'set_appearance')).toEqual([]);
  });

  it('a change undone before it is saved isn’t saved', async () => {
    const backend = installFakeBackend();
    renderApp();
    fireEvent.click(await screen.findByRole('button', { name: 'Collapse navigation' }));
    fireEvent.click(screen.getByRole('button', { name: 'Expand navigation' }));
    await new Promise((r) => setTimeout(r, 400));
    expect(callsOf(backend, 'set_appearance')).toEqual([]);
  });

  it('collapsing the navigation to icons is saved too', async () => {
    const user = userEvent.setup();
    const backend = installFakeBackend();
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Collapse navigation' }));
    expect(screen.getByRole('navigation', { name: 'Primary' }).getAttribute('data-mode')).toBe('rail');
    await waitFor(() => { expect(callsOf(backend, 'set_appearance').at(-1)?.args).toMatchObject({ appearance: { layout: { sidebarCollapsed: true } } }); });
  });

  it('keeps the list its room in a narrow window by showing the navigation as icons', async () => {
    setViewport(LAYOUT.TABLE_MIN + LAYOUT.SIDEBAR_MIN - 1);
    installFakeBackend();
    renderApp();
    const nav = await screen.findByRole('navigation', { name: 'Primary' });
    expect(nav.getAttribute('data-mode')).toBe('rail');
  });

  // V1's details-panel layout tests (tests/unit/layout.test.tsx), with the panel (#61).
  const withLayout = (layout: Partial<typeof DEFAULT_LAYOUT>) =>
    installFakeBackend({
      settings: { ...setUp(), appearance: { ...setUp().appearance, layout: { ...DEFAULT_LAYOUT, ...layout } } },
      snapshot: { ...emptySnapshot(), downloads: [record({ id: 'a', fileName: 'archive.zip', status: 'paused' })] },
    });
  const detailsSeparator = () => screen.findByRole('separator', { name: 'Resize details panel' });
  const valueOf = (el: HTMLElement) => Number(el.getAttribute('aria-valuenow'));

  it('keeps the details panel closed at first, and opens it at the responsive default', async () => {
    const user = userEvent.setup();
    withLayout({});
    renderApp();
    await within(await screen.findByRole('grid', { name: 'Downloads' })).findByText('archive.zip');
    expect(screen.queryByRole('complementary', { name: 'Download details' })).toBeNull();
    expect(screen.queryByRole('separator', { name: 'Resize details panel' })).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Toggle details panel' }));
    expect(valueOf(await detailsSeparator())).toBe(300);
  });

  it('scales the details panel with a wide window', async () => {
    setViewport(1920);
    withLayout({ detailsVisible: true });
    renderApp();
    expect(valueOf(await detailsSeparator())).toBe(340);
  });

  it('keeps the details panel between its minimum and maximum, and moves it 16 px a key', async () => {
    withLayout({ detailsVisible: true });
    renderApp();
    const sep = await detailsSeparator();
    expect(sep.getAttribute('aria-valuemin')).toBe(String(LAYOUT.DETAILS_MIN));
    expect(sep.getAttribute('aria-valuemax')).toBe(String(LAYOUT.DETAILS_MAX));
    // The panel is on the right, so moving the separator left widens it.
    fireEvent.keyDown(sep, { key: 'ArrowLeft' });
    expect(valueOf(sep)).toBe(316);
    fireEvent.keyDown(sep, { key: 'Home' });
    expect(valueOf(sep)).toBe(LAYOUT.DETAILS_MIN);
    fireEvent.keyDown(sep, { key: 'End' });
    expect(valueOf(sep)).toBe(LAYOUT.DETAILS_MAX);
  });

  it('saves the details panel’s width and restores it, open, after a restart', async () => {
    const backend = withLayout({ detailsVisible: true });
    const first = renderApp();
    fireEvent.keyDown(await detailsSeparator(), { key: 'ArrowLeft', shiftKey: true });
    await waitFor(() => { expect(callsOf(backend, 'set_appearance').at(-1)?.args).toMatchObject({ appearance: { layout: { detailsWidth: 364, detailsVisible: true } } }); });
    first.unmount();
    useUiStore.setState({ layout: { ...DEFAULT_LAYOUT }, detailsId: null });
    useDownloadsStore.setState({ snapshot: null, error: null });
    renderApp();
    expect(valueOf(await detailsSeparator())).toBe(364);
    expect(await screen.findByRole('complementary', { name: 'Download details' })).toBeTruthy();
  });

  it('has no details panel on the Categories screen, and gives it back on the list', async () => {
    const user = userEvent.setup();
    withLayout({ detailsVisible: true });
    renderApp();
    await screen.findByRole('complementary', { name: 'Download details' });
    await user.click(within(screen.getByRole('navigation', { name: 'Primary' })).getByRole('button', { name: /Categories/ }));
    await waitFor(() => { expect(screen.queryByRole('complementary', { name: 'Download details' })).toBeNull(); });
    await user.click(within(screen.getByRole('navigation', { name: 'Primary' })).getByRole('button', { name: /All Downloads/ }));
    expect(await screen.findByRole('complementary', { name: 'Download details' })).toBeTruthy();
  });

  it('keeps a layout change made while the last one is being saved', async () => {
    let release: () => void = () => undefined;
    const backend = installFakeBackend({ hold: { set_appearance: new Promise<void>((r) => { release = r; }) } });
    renderApp();
    fireEvent.click(await screen.findByRole('button', { name: 'Collapse navigation' }));
    await waitFor(() => { expect(callsOf(backend, 'set_appearance')).toHaveLength(1); });
    // The collapse is being saved; the user expands the navigation again before it is.
    fireEvent.click(screen.getByRole('button', { name: 'Expand navigation' }));
    await act(async () => { release(); await new Promise((r) => setTimeout(r, 50)); });
    expect(screen.getByRole('navigation', { name: 'Primary' }).getAttribute('data-mode')).toBe('full');
    await waitFor(() => { expect(callsOf(backend, 'set_appearance').at(-1)?.args).toMatchObject({ appearance: { layout: { sidebarCollapsed: false } } }); });
  });

  it('says so when the layout can’t be saved', async () => {
    const user = userEvent.setup();
    installFakeBackend({ failures: { set_appearance: 'DATABASE' } });
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Collapse navigation' }));
    expect(await screen.findByText(ERROR_MESSAGES.DATABASE, {}, { timeout: 2000 })).toBeTruthy();
  });
});

describe('quitting with downloads active', () => {
  it('asks first, and quits only when the user confirms', async () => {
    const user = userEvent.setup();
    const backend = installFakeBackend();
    renderApp();
    await screen.findByRole('navigation', { name: 'Primary' });
    await act(() => emit('quit-requested', { active: 2 }));
    const dialog = await screen.findByRole('alertdialog', { name: 'Quit Velocity?' });
    expect(dialog.textContent).toContain('2 downloads are still running');
    await user.click(within(dialog).getByRole('button', { name: 'Keep downloading' }));
    expect(callsOf(backend, 'quit_app')).toEqual([]);
    await act(() => emit('quit-requested', { active: 1 }));
    const again = await screen.findByRole('alertdialog', { name: 'Quit Velocity?' });
    expect(again.textContent).toContain('1 download is still running');
    await user.click(within(again).getByRole('button', { name: 'Quit' }));
    await waitFor(() => { expect(callsOf(backend, 'quit_app')).toHaveLength(1); });
  });
});

describe('the sidebar and status bar', () => {
  it('count the downloads in each list and say what the engines are doing', async () => {
    installFakeBackend({
      snapshot: {
        ...emptySnapshot(),
        downloads: [
          record({ id: 'a', status: 'downloading', progress: { ...(fixture('download-record.json') as DownloadRecord).progress, downloadSpeedBps: 2 * 1024 * 1024 } }),
          record({ id: 'b', status: 'paused' }),
          record({ id: 'c', status: 'failed' }),
        ],
      },
    });
    renderApp();
    const nav = await screen.findByRole('navigation', { name: 'Primary' });
    await waitFor(() => { expect(within(nav).getByRole('button', { name: /All Downloads/ }).textContent).toContain('3'); });
    expect(within(nav).getByRole('button', { name: /Failed/ }).textContent).toContain('1');
    const status = screen.getByRole('contentinfo', { name: 'Status' });
    expect(status.textContent).toContain('1 active · 1 queued');
    expect(status.textContent).toContain('2.0 MiB/s');
    expect(status.textContent).toContain('Ready');
  });

  it('follow the snapshots the native side sends', async () => {
    installFakeBackend();
    renderApp();
    const status = await screen.findByRole('contentinfo', { name: 'Status' });
    await waitFor(() => { expect(status.textContent).toContain('0 active · 0 queued'); });
    await act(() => emit('downloads-snapshot', { ...emptySnapshot(), engine: { state: 'restarting', attempt: 1 } }));
    await waitFor(() => { expect(status.textContent).toContain('Transfer engine restarting'); });
    await act(() => emit('downloads-snapshot', { ...emptySnapshot(), engine: { state: 'unavailable', code: 'ENGINE_MISSING' } }));
    await waitFor(() => { expect(status.textContent).toContain('Transfer engine unavailable'); });
  });

  it('each list view says what it would hold when it is empty (#59)', async () => {
    useUiStore.setState({ view: 'failed' });
    installFakeBackend();
    renderApp();
    expect(await screen.findByText('No failed downloads')).toBeTruthy();
  });
});

describe('the Categories screen', () => {
  it('lists the categories, and creates and deletes one', async () => {
    const user = userEvent.setup();
    const backend = installFakeBackend();
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Categories' }));
    const builtIn = (fixture('category.json') as { name: string }).name;
    expect(await main().findByText(builtIn)).toBeTruthy();

    await user.click(screen.getByRole('button', { name: 'New category' }));
    const dialog = await screen.findByRole('dialog', { name: 'New category' });
    await user.type(within(dialog).getByRole('textbox', { name: 'Name' }), 'Podcasts');
    await user.type(within(dialog).getByRole('textbox', { name: 'File extensions' }), '.MP3, ogg mp3');
    await user.click(within(dialog).getByRole('button', { name: 'Save' }));
    expect(await main().findByText('Podcasts')).toBeTruthy();
    expect(callsOf(backend, 'create_category')[0]?.args).toEqual({
      input: { name: 'Podcasts', color: '#2563eb', icon: 'folder', extensions: ['mp3', 'ogg'], directory: null },
    });

    const card = main().getByText('Podcasts').closest('li') as HTMLElement;
    await user.click(within(card).getByRole('button', { name: 'Delete' }));
    const ask = await screen.findByRole('alertdialog', { name: 'Delete the “Podcasts” category?' });
    await user.click(within(ask).getByRole('button', { name: 'Delete category' }));
    await waitFor(() => { expect(main().queryByText('Podcasts')).toBeNull(); });
    expect(callsOf(backend, 'delete_category').map((c) => c.args)).toEqual([{ id: 'user-1' }]);
  });

  it('a built-in category can be edited but not deleted', async () => {
    const user = userEvent.setup();
    installFakeBackend({ categories: [{ ...(fixture('category.json') as Category), builtin: true }] });
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Categories' }));
    const card = (await main().findByText((fixture('category.json') as { name: string }).name)).closest('li') as HTMLElement;
    expect(within(card).getByRole('button', { name: 'Edit' })).toBeTruthy();
    expect(within(card).queryByRole('button', { name: 'Delete' })).toBeNull();
  });

  it('shows why a category was refused, in the dialog', async () => {
    const user = userEvent.setup();
    installFakeBackend({ failures: { create_category: 'NAME_TAKEN' } });
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Categories' }));
    await user.click(await screen.findByRole('button', { name: 'New category' }));
    const dialog = await screen.findByRole('dialog', { name: 'New category' });
    await user.type(within(dialog).getByRole('textbox', { name: 'Name' }), 'Videos');
    await user.click(within(dialog).getByRole('button', { name: 'Save' }));
    expect((await within(dialog).findByRole('alert')).textContent).toBe(ERROR_MESSAGES.NAME_TAKEN);
  });
});
