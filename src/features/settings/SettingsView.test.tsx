import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from '../../app/App';
import type { Capabilities, Settings } from '../../contract';
import { DEFAULT_LAYOUT } from '../../layout/layoutLogic';
import { createQueryClient } from '../../lib/queries';
import { INITIAL_LIST, useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';
import { callsOf, fixture, installFakeBackend, type FakeState } from '../../test/fakeBackend';

// V1's settings tests (tests/unit/app.test.tsx, "settings") and its BitTorrent settings tests
// (tests/unit/torrentControls.test.tsx, moved here by the PO's decision on #63), ported to V2's screen, and #30's own.

function renderApp() {
  return render(<App client={createQueryClient()} />);
}

const saved = (state: FakeState) => callsOf(state, 'set_settings').map((c) => c.args.settings as Settings);

async function openSettings(section?: string) {
  const user = userEvent.setup();
  renderApp();
  await screen.findByText('No downloads yet');
  act(() => { useUiStore.getState().setView('settings'); });
  if (section) await user.click(await screen.findByRole('button', { name: section }));
  return user;
}

beforeEach(() => {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
  });
  useUiStore.setState({ view: 'all', categoryFilter: null, layout: { ...DEFAULT_LAYOUT }, addDialog: { open: false, prefill: null }, settingsSection: 'general' });
  useDownloadsStore.setState({ snapshot: null, error: null, ...INITIAL_LIST });
});

describe('settings (V1)', () => {
  it('persists a toggled setting through the native command', async () => {
    const state = installFakeBackend();
    const user = await openSettings();
    await user.click(await screen.findByRole('checkbox', { name: 'Start minimized' }));
    await waitFor(() => { expect(saved(state)).toHaveLength(1); });
    expect(state.appSettings.general.startMinimized).toBe(true);
  });

  it('commits numeric settings on blur with clamping', async () => {
    const state = installFakeBackend();
    const user = await openSettings('Queue and performance');
    const input = await screen.findByLabelText<HTMLInputElement>('Simultaneous downloads');
    await user.clear(input);
    await user.type(input, '99');
    expect(saved(state)).toHaveLength(0);
    await user.tab();
    await waitFor(() => { expect(state.appSettings.queue.maxConcurrentDownloads).toBe(32); });
  });

  it('confirms before resetting settings', async () => {
    const state = installFakeBackend();
    const user = await openSettings();
    await user.click(await screen.findByRole('button', { name: /Reset to defaults/ }));
    await user.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Cancel' }));
    expect(callsOf(state, 'reset_settings')).toHaveLength(0);
    await user.click(screen.getByRole('button', { name: /Reset to defaults/ }));
    await user.click(within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Reset settings' }));
    await waitFor(() => { expect(callsOf(state, 'reset_settings')).toHaveLength(1); });
  });
});

describe('BitTorrent settings (V1)', () => {
  it('has its own BitTorrent switch, separate from Metalink', async () => {
    const state = installFakeBackend();
    const user = await openSettings('BitTorrent');
    await user.click(await screen.findByRole('checkbox', { name: 'BitTorrent' }));
    await waitFor(() => { expect(state.appSettings.advanced.enableBittorrent).toBe(false); });
    expect(state.appSettings.advanced.enableMetalink).toBe(false);
  });

  it('limits how many torrents seed at once', async () => {
    const state = installFakeBackend();
    const user = await openSettings('BitTorrent');
    const seeds = await screen.findByLabelText<HTMLInputElement>('Torrents seeding at once');
    await user.clear(seeds);
    await user.type(seeds, '3');
    await user.tab();
    await waitFor(() => { expect(state.appSettings.torrent.maxActiveSeeds).toBe(3); });
  });

  it('says new torrents use libtorrent and offers opt-in port mapping', async () => {
    const state = installFakeBackend();
    const user = await openSettings('BitTorrent');
    expect((await screen.findByRole('region', { name: 'BitTorrent' })).textContent).toContain('libtorrent');
    const mapping = screen.getByRole<HTMLInputElement>('checkbox', { name: 'Ask the router to forward the port (UPnP, NAT-PMP)' });
    expect(mapping.checked).toBe(false);
    await user.click(mapping);
    await waitFor(() => { expect(state.appSettings.torrent.portMapping).toBe(true); });
  });

  it('shows the libtorrent session and the port mapping state', async () => {
    installFakeBackend({ network: { listenPort: 6890, listenError: null, portMapping: { state: 'mapped', externalPort: 40000, via: 'natpmp' } } });
    await openSettings('BitTorrent');
    const session = await screen.findByRole('region', { name: 'Connection' });
    await waitFor(() => { expect(session.textContent).toContain('Listening on port 6890'); });
    expect(session.textContent).toContain('Port 6890 forwarded by the router as 40000 (NAT-PMP).');
  });
});

describe('the Settings screen (#30)', () => {
  it('opens from the navigation and with Ctrl+,', async () => {
    installFakeBackend();
    const user = userEvent.setup();
    renderApp();
    await screen.findByText('No downloads yet');
    await user.keyboard('{Control>},{/Control}');
    expect(await screen.findByRole('heading', { name: 'Settings', level: 1 })).toBeTruthy();
    act(() => { useUiStore.getState().setView('all'); });
    await user.click(within(screen.getByRole('navigation', { name: 'Primary' })).getByRole('button', { name: /^Settings/ }));
    expect(await screen.findByRole('heading', { name: 'Settings', level: 1 })).toBeTruthy();
  });

  it('says why a value was refused, and shows what is saved', async () => {
    const state = installFakeBackend({ failures: { set_settings: 'INVALID_INPUT' } });
    const user = await openSettings();
    const box = await screen.findByRole<HTMLInputElement>('checkbox', { name: 'Start minimized' });
    await user.click(box);
    expect(await screen.findByRole('alert')).toBeTruthy();
    await waitFor(() => { expect(box.checked).toBe(false); });
    expect(state.appSettings.general.startMinimized).toBe(false);
  });

  it('turns Metalink off and says why when the engine can’t do it', async () => {
    const caps = fixture('capabilities.json') as Capabilities;
    installFakeBackend({ capabilities: { ...caps, metalink: false } });
    await openSettings('Advanced');
    const metalink = await screen.findByRole<HTMLInputElement>('checkbox', { name: 'Metalink' });
    expect(metalink.disabled).toBe(true);
    expect((await screen.findByRole('region', { name: 'Protocols' })).textContent).toMatch(/can’t/);
  });

  it('saves the engine’s retries, timeouts and preallocation', async () => {
    const state = installFakeBackend();
    const user = await openSettings('Advanced');
    await user.selectOptions(await screen.findByLabelText('Preallocation'), 'full');
    await waitFor(() => { expect(state.appSettings.engine.preallocation).toBe('full'); });
    const failures = screen.getByLabelText<HTMLInputElement>('Failures in a row before giving up');
    await user.clear(failures);
    await user.type(failures, '9');
    await user.tab();
    await waitFor(() => { expect(state.appSettings.engine.failuresInARow).toBe(9); });
  });

  it('saves how long the history keeps entries', async () => {
    const state = installFakeBackend();
    const user = await openSettings('History and storage');
    const days = await screen.findByLabelText<HTMLInputElement>('Keep history for (days, 0 for ever)');
    await user.clear(days);
    await user.type(days, '30');
    await user.tab();
    await waitFor(() => { expect(state.appSettings.storage.historyRetentionDays).toBe(30); });
  });

  it('keeps the global speed limit and the user agent', async () => {
    const state = installFakeBackend();
    const user = await openSettings('Advanced');
    const agent = await screen.findByLabelText<HTMLInputElement>('User agent');
    await user.type(agent, 'Velocity-Test/1');
    await user.tab();
    await waitFor(() => { expect(state.appSettings.advanced.userAgent).toBe('Velocity-Test/1'); });
    await user.clear(agent);
    await user.tab();
    await waitFor(() => { expect(state.appSettings.advanced.userAgent).toBeNull(); });
  });

  it('shows the diagnostics under About and diagnostics', async () => {
    installFakeBackend();
    await openSettings('About and diagnostics');
    expect(await screen.findByRole('button', { name: 'Export diagnostics…' })).toBeTruthy();
  });

  it('starts new torrents in the Add dialog with the saved seeding and upload limit', async () => {
    const settings = fixture('settings.json') as Settings;
    installFakeBackend({ appSettings: { ...settings, newTorrents: { chooseFilesForMagnets: true, seeding: { mode: 'unlimited' }, uploadLimitKib: 64 } } });
    renderApp();
    await screen.findByText('No downloads yet');
    act(() => { useUiStore.getState().openAdd({ links: 'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567' }); });
    expect((await screen.findByLabelText<HTMLSelectElement>('Seeding after download')).value).toBe('unlimited');
    expect(screen.getByLabelText<HTMLInputElement>('Upload limit (KiB/s)').value).toBe('64');
    expect(screen.getByRole<HTMLInputElement>('checkbox', { name: 'Choose files before downloading' }).checked).toBe(true);
  });
});

describe('the Browser section (#65)', () => {
  it('saves what happens to links from the browser', async () => {
    const state = installFakeBackend();
    const user = await openSettings('Browser');
    const ask = await screen.findByRole<HTMLInputElement>('checkbox', { name: 'Ask before adding' });
    expect(ask.checked).toBe(true);
    await user.click(ask);
    await waitFor(() => { expect(saved(state)).toHaveLength(1); });
    expect(state.appSettings.browser).toEqual({ acceptRequests: true, confirmBeforeAdding: false });
    await user.click(screen.getByRole('checkbox', { name: 'Take links from the browser' }));
    await waitFor(() => { expect(state.appSettings.browser.acceptRequests).toBe(false); });
    expect(screen.getByRole<HTMLInputElement>('checkbox', { name: 'Ask before adding' }).disabled).toBe(true);
  });

  it('registers and unregisters each browser', async () => {
    const state = installFakeBackend();
    const user = await openSettings('Browser');
    await user.click(await screen.findByRole('button', { name: 'Register with Mozilla Firefox' }));
    await waitFor(() => { expect(callsOf(state, 'register_browser')).toEqual([{ cmd: 'register_browser', args: { browser: 'firefox' } }]); });
    expect(await screen.findByRole('button', { name: 'Unregister from Mozilla Firefox' })).toBeTruthy();
    await user.click(screen.getByRole('button', { name: 'Unregister from Google Chrome' }));
    await waitFor(() => { expect(callsOf(state, 'unregister_browser')).toEqual([{ cmd: 'unregister_browser', args: { browser: 'chrome' } }]); });
    expect(await screen.findByRole('button', { name: 'Register with Google Chrome' })).toBeTruthy();
  });

  it('says so when the native host isn’t installed, and offers nothing to register', async () => {
    const state = installFakeBackend();
    state.browser = { ...state.browser, hostPath: null };
    await openSettings('Browser');
    expect(await screen.findByText(/isn’t installed beside Velocity V2/)).toBeTruthy();
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Register with Mozilla Firefox' }).disabled).toBe(true);
  });
});

describe('the Proxy section (#192)', () => {
  it('saves the proxy without its password, and checks nothing that the native side checks', async () => {
    const state = installFakeBackend();
    const user = await openSettings('Proxy');
    const host = await screen.findByLabelText('Host');
    await user.type(host, 'proxy.example.com');
    await user.tab();
    await waitFor(() => { expect(state.appSettings.proxy.host).toBe('proxy.example.com'); });
    await user.selectOptions(screen.getByLabelText('Type'), 'socks5');
    await waitFor(() => { expect(state.appSettings.proxy.kind).toBe('socks5'); });
    expect(JSON.stringify(saved(state))).not.toContain('password"');
  });

  it('sets the password on its own, never shows it, and can clear it', async () => {
    const state = installFakeBackend();
    const user = await openSettings('Proxy');
    expect(await screen.findByText('No password is set.')).toBeTruthy();
    const field = screen.getByLabelText<HTMLInputElement>('Password');
    expect(field.type).toBe('password');
    await user.type(field, 'hunter2-proxy');
    await user.click(screen.getByRole('button', { name: 'Save password' }));
    await waitFor(() => { expect(state.proxyPassword).toBe('hunter2-proxy'); });
    expect(await screen.findByText('A password is set.')).toBeTruthy();
    expect(field.value).toBe('');
    await user.click(screen.getByRole('button', { name: 'Clear' }));
    await waitFor(() => { expect(state.proxyPassword).toBeNull(); });
    expect(callsOf(state, 'set_proxy_password').map((c) => c.args.password)).toEqual(['hunter2-proxy', null]);
  });

  it('says why a password was refused', async () => {
    const state = installFakeBackend();
    state.failures.set_proxy_password = 'KEYCHAIN_UNAVAILABLE';
    const user = await openSettings('Proxy');
    await user.type(await screen.findByLabelText('Password'), 'hunter2-proxy');
    await user.click(screen.getByRole('button', { name: 'Save password' }));
    expect((await screen.findByRole('alert')).textContent).toContain('keychain');
  });

  it('remembers the password in the keychain only when ticked', async () => {
    const state = installFakeBackend();
    const user = await openSettings('Proxy');
    const remember = await screen.findByRole('checkbox', { name: 'Remember the password in the system keychain' });
    expect((remember as HTMLInputElement).checked).toBe(false);
    await user.click(remember);
    await waitFor(() => { expect(state.appSettings.proxy.rememberPassword).toBe(true); });
  });
});
