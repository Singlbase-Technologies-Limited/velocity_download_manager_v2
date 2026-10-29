import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import type { DownloadRecord, DownloadsSnapshot } from '../contract';
import { DEFAULT_LAYOUT, LAYOUT } from '../layout/layoutLogic';
import { createQueryClient } from '../lib/queries';
import { INITIAL_LIST, useDownloadsStore } from '../stores/downloads';
import { useUiStore, type SettingsSection, type View } from '../stores/ui';
import { callsOf, emptySnapshot, installFakeBackend, type FakeState } from '../test/fakeBackend';
import { makeRecord } from '../test/records';
import { App } from './App';

// The keyboard and screen-reader behaviours of the accessibility audit (#66, docs/ACCESSIBILITY.md) that no other
// test covers: the list's keys, focus in and out of dialogs, and a name for every control in every view.

function renderApp() {
  return render(<App client={createQueryClient()} />);
}

const progress = makeRecord().progress;
const archive = makeRecord({ id: 'a', fileName: 'archive.zip', status: 'downloading', createdAtMs: 1, progress: { ...progress, completedBytes: 250, totalBytes: 1000 } });
const movie = makeRecord({ id: 'b', fileName: 'movie.mp4', status: 'paused', createdAtMs: 2 });
const done = makeRecord({ id: 'c', fileName: 'done.iso', status: 'completed', createdAtMs: 3 });

function withDownloads(downloads: DownloadRecord[]): DownloadsSnapshot {
  return { ...emptySnapshot(), downloads };
}

const grid = () => screen.findByRole('grid', { name: 'Downloads' });
const selected = async () =>
  within(await grid())
    .getAllByRole('row')
    .filter((r) => r.getAttribute('aria-selected') === 'true')
    .map((r) => r.querySelector('[data-column="name"]')?.textContent);

/** The roles a user reaches or reads by name, and those whose name is empty. */
const NAMED_ROLES = ['button', 'checkbox', 'textbox', 'searchbox', 'combobox', 'spinbutton', 'slider', 'separator', 'menuitem', 'tab', 'radio', 'link', 'grid', 'dialog', 'alertdialog', 'progressbar', 'navigation', 'region', 'complementary'];
function unnamed(): string[] {
  return NAMED_ROLES.flatMap((role) =>
    screen.queryAllByRole(role, { name: (n) => n.trim() === '' }).map((el) => `${role}: ${el.outerHTML.slice(0, 120)}`),
  );
}
/** Icons drawn inside a named control are decoration: hidden from screen readers. */
function spokenIcons(): string[] {
  return [...document.querySelectorAll('button svg, [role="menuitem"] svg')].filter((svg) => svg.getAttribute('aria-hidden') !== 'true').map((svg) => svg.outerHTML.slice(0, 80));
}

beforeEach(() => {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
  });
  Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: 1366 });
  useUiStore.setState({ view: 'all', categoryFilter: null, layout: { ...DEFAULT_LAYOUT }, addDialog: { open: false, prefill: null }, detailsId: null, settingsSection: 'general' });
  useDownloadsStore.setState({ snapshot: null, error: null, ...INITIAL_LIST });
});

describe('the download list from the keyboard (V1’s keys)', () => {
  let state: FakeState;
  beforeEach(() => {
    state = installFakeBackend({ snapshot: withDownloads([archive, movie, done]) });
  });

  it('End selects the last row, Ctrl+A every row, and Escape none', async () => {
    const user = userEvent.setup();
    renderApp();
    (await grid()).focus();
    await user.keyboard('{End}');
    // Newest first: done.iso, movie.mp4, archive.zip.
    expect(await selected()).toEqual(['archive.zip']);
    await user.keyboard('{Control>}a{/Control}');
    expect(await selected()).toEqual(['done.iso', 'movie.mp4', 'archive.zip']);
    await user.keyboard('{Escape}');
    expect(await selected()).toEqual([]);
  });

  it('Enter opens the details of the focused row', async () => {
    const user = userEvent.setup();
    renderApp();
    (await grid()).focus();
    await user.keyboard('{Home}{Enter}');
    const panel = await screen.findByRole('complementary', { name: 'Download details' });
    expect(within(panel).getByRole('heading', { name: 'done.iso' })).toBeTruthy();
  });

  it('Delete asks before removing the selection, and Escape keeps it', async () => {
    const user = userEvent.setup();
    renderApp();
    const list = await grid();
    list.focus();
    await user.keyboard('{End}{Delete}');
    const confirm = await screen.findByRole('alertdialog');
    expect(confirm.contains(document.activeElement)).toBe(true);
    await user.keyboard('{Escape}');
    await waitFor(() => { expect(screen.queryByRole('alertdialog')).toBeNull(); });
    expect(callsOf(state, 'remove_downloads')).toEqual([]);
    expect(document.activeElement).toBe(list);
  });
});

describe('focus in dialogs', () => {
  it('moves into the Add dialog, and back to the button that opened it when it closes', async () => {
    const user = userEvent.setup();
    installFakeBackend();
    renderApp();
    const open = await screen.findByRole('button', { name: 'Add download' });
    await user.click(open);
    const dialog = await screen.findByRole('dialog', { name: 'Add download' });
    expect(dialog.contains(document.activeElement)).toBe(true);
    await user.keyboard('{Escape}');
    await waitFor(() => { expect(screen.queryByRole('dialog')).toBeNull(); });
    expect(document.activeElement).toBe(open);
  });

  it('keeps Tab inside an open dialog', async () => {
    const user = userEvent.setup();
    installFakeBackend();
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Add download' }));
    const dialog = await screen.findByRole('dialog', { name: 'Add download' });
    // One more Tab than the dialog has stops crosses its end at least once.
    const stops = dialog.querySelectorAll('a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])').length;
    for (let i = 0; i <= stops; i++) {
      await user.tab();
      expect(dialog.contains(document.activeElement), `tab ${String(i)}`).toBe(true);
    }
  });
});

describe('menus from the keyboard', () => {
  it('the column chooser opens with Enter, moves with the arrows, and gives focus back on Escape', async () => {
    const user = userEvent.setup();
    installFakeBackend({ snapshot: withDownloads([archive]) });
    renderApp();
    await grid();
    const trigger = screen.getByRole('button', { name: 'Choose columns' });
    trigger.focus();
    await user.keyboard('{Enter}');
    const menu = await screen.findByRole('menu');
    expect(menu.contains(document.activeElement)).toBe(true);
    const first = document.activeElement;
    await user.keyboard('{ArrowDown}');
    expect(document.activeElement).not.toBe(first);
    expect(menu.contains(document.activeElement)).toBe(true);
    await user.keyboard('{Escape}');
    await waitFor(() => { expect(screen.queryByRole('menu')).toBeNull(); });
    expect(document.activeElement).toBe(trigger);
  });
});

describe('every control has a name, and icons stay silent', () => {
  it('in the list with the details panel open', async () => {
    const user = userEvent.setup();
    installFakeBackend({ snapshot: withDownloads([archive, movie, done]) });
    renderApp();
    (await grid()).focus();
    await user.keyboard('{Home}{Enter}');
    await screen.findByRole('complementary', { name: 'Download details' });
    expect(unnamed()).toEqual([]);
    expect(spokenIcons()).toEqual([]);
  });

  it('with the navigation shown as icons', async () => {
    Object.defineProperty(window, 'innerWidth', { configurable: true, writable: true, value: LAYOUT.TABLE_MIN + LAYOUT.SIDEBAR_MIN - 1 });
    installFakeBackend({ snapshot: withDownloads([archive]) });
    renderApp();
    expect((await screen.findByRole('navigation', { name: 'Primary' })).getAttribute('data-mode')).toBe('rail');
    expect(unnamed()).toEqual([]);
    expect(spokenIcons()).toEqual([]);
  });

  it('in the Add dialog', async () => {
    const user = userEvent.setup();
    installFakeBackend();
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Add download' }));
    await screen.findByRole('dialog', { name: 'Add download' });
    expect(unnamed()).toEqual([]);
    expect(spokenIcons()).toEqual([]);
  });

  it.each<View>(['categories', 'history'])('on the %s screen', async (view) => {
    installFakeBackend({ snapshot: withDownloads([archive]) });
    renderApp();
    await grid();
    act(() => { useUiStore.getState().setView(view); });
    await waitFor(() => { expect(screen.queryByRole('grid', { name: 'Downloads' })).toBeNull(); });
    expect(unnamed()).toEqual([]);
    expect(spokenIcons()).toEqual([]);
  });

  it.each<SettingsSection>(['general', 'downloads', 'performance', 'bittorrent', 'notifications', 'appearance', 'storage', 'advanced', 'about'])(
    'in Settings, %s',
    async (section) => {
      installFakeBackend();
      renderApp();
      await screen.findByText('No downloads yet');
      act(() => {
        useUiStore.getState().setView('settings');
        useUiStore.getState().setSettingsSection(section);
      });
      await screen.findByRole('navigation', { name: 'Settings sections' });
      await waitFor(() => { expect(screen.queryByText('Loading settings…')).toBeNull(); });
      expect(unnamed()).toEqual([]);
      expect(spokenIcons()).toEqual([]);
    },
  );
});
