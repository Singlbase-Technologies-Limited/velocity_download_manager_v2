import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';
import { App } from '../../app/App';
import type { HistoryEntry, HistoryPage, HistoryQuery } from '../../contract';
import { DEFAULT_LAYOUT } from '../../layout/layoutLogic';
import { createQueryClient } from '../../lib/queries';
import { INITIAL_LIST, useDownloadsStore } from '../../stores/downloads';
import { useUiStore } from '../../stores/ui';
import { callsOf, fixture, installFakeBackend, type FakeState } from '../../test/fakeBackend';

// The history view (#32), as V1's HistoryView: search, a status filter, pages, removing entries and clearing it, never
// touching a file. V1 had no tests of its own for it.

function renderApp() {
  return render(<App client={createQueryClient()} />);
}

const sample = (fixture('history.json') as { page: HistoryPage }).page;
const [archive, missing] = sample.entries as [HistoryEntry, HistoryEntry];
const entry = (i: number): HistoryEntry => ({ ...archive, id: `h-${String(i)}`, fileName: `file-${String(i).padStart(3, '0')}.zip`, completedAtMs: 1_792_600_000_000 - i * 1000 });
const queries = (state: FakeState) => callsOf(state, 'get_history').map((c) => c.args.query as HistoryQuery);

async function openHistory() {
  const user = userEvent.setup();
  renderApp();
  await user.click(await screen.findByRole('button', { name: /^History/ }));
  return user;
}

beforeEach(() => {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
  });
  useUiStore.setState({ view: 'all', categoryFilter: null, layout: { ...DEFAULT_LAYOUT }, addDialog: { open: false, prefill: null } });
  useDownloadsStore.setState({ snapshot: null, error: null, ...INITIAL_LIST });
});

describe('the history (#32)', () => {
  it('lists finished downloads with their status, and why one failed', async () => {
    installFakeBackend({ history: () => sample });
    await openHistory();
    const table = await screen.findByRole('table', { name: 'History' });
    const rows = within(table).getAllByRole('row').slice(1);
    expect(rows).toHaveLength(2);
    expect(rows[0]?.textContent).toContain('archive.zip');
    expect(rows[0]?.textContent).toContain('Completed');
    expect(rows[1]?.textContent).toContain('The file was not found on the server (HTTP 404).');
    expect(rows[1]?.textContent).toContain('In the list');
    expect(screen.getByText('2 entries')).toBeTruthy();
  });

  it('says so when there is no history yet', async () => {
    installFakeBackend({ history: () => ({ entries: [], total: 0 }) });
    await openHistory();
    expect(await screen.findByText('No history yet')).toBeTruthy();
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Clear history' }).disabled).toBe(true);
  });

  it('searches and filters by status', async () => {
    const state = installFakeBackend({ history: () => sample });
    const user = await openHistory();
    await screen.findByRole('table', { name: 'History' });
    await user.type(screen.getByRole('searchbox', { name: 'Search history' }), 'album');
    await waitFor(() => { expect(queries(state).at(-1)).toMatchObject({ search: 'album', offset: 0 }); });
    await user.selectOptions(screen.getByLabelText('Filter by status'), 'Cancelled');
    await waitFor(() => { expect(queries(state).at(-1)).toMatchObject({ search: 'album', status: 'cancelled', offset: 0 }); });
  });

  it('pages through a long history', async () => {
    const state = installFakeBackend({
      history: (q) => {
        const all = Array.from({ length: 250 }, (_, i) => entry(i));
        const offset = q.offset ?? 0;
        return { entries: all.slice(offset, offset + (q.limit ?? 100)), total: all.length };
      },
    });
    const user = await openHistory();
    expect(await screen.findByText('1–100 of 250')).toBeTruthy();
    await user.click(screen.getByRole('button', { name: 'Next page' }));
    expect(await screen.findByText('101–200 of 250')).toBeTruthy();
    expect(queries(state).at(-1)).toMatchObject({ limit: 100, offset: 100 });
  });

  it('removes the selected entries, saying their files stay', async () => {
    const state = installFakeBackend({ history: () => sample });
    const user = await openHistory();
    const table = await screen.findByRole('table', { name: 'History' });
    await user.click(within(table).getByRole('checkbox', { name: 'Select archive.zip' }));
    await user.click(screen.getByRole('button', { name: 'Remove selected' }));
    await waitFor(() => { expect(callsOf(state, 'delete_history_entries')[0]?.args).toEqual({ ids: [archive.id] }); });
    expect(await screen.findByText(/Files on disk were not touched/)).toBeTruthy();
  });

  it('clears the history only after asking, and says files stay', async () => {
    const state = installFakeBackend({ history: () => sample });
    const user = await openHistory();
    await screen.findByRole('table', { name: 'History' });
    await user.click(screen.getByRole('button', { name: 'Clear history' }));
    let dialog = await screen.findByRole('alertdialog');
    expect(dialog.textContent).toContain('Downloaded files are not deleted');
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(callsOf(state, 'clear_history')).toHaveLength(0);
    await user.click(screen.getByRole('button', { name: 'Clear history' }));
    dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: 'Clear history' }));
    await waitFor(() => { expect(callsOf(state, 'clear_history')).toHaveLength(1); });
  });

  it('downloads an entry again through the Add dialog', async () => {
    installFakeBackend({ history: () => sample });
    const user = await openHistory();
    const table = await screen.findByRole('table', { name: 'History' });
    await user.click(within(table).getByRole('button', { name: 'Download archive.zip again' }));
    expect((await screen.findByLabelText<HTMLTextAreaElement>('Links')).value).toBe(archive.sourceUrl);
  });

  it('says why the history couldn’t be read, and tries again', async () => {
    const state = installFakeBackend({ history: () => sample, failures: { get_history: 'DATABASE' } });
    const user = await openHistory();
    expect(await screen.findByText('The history couldn’t be read')).toBeTruthy();
    delete state.failures.get_history;
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByRole('table', { name: 'History' })).toBeTruthy();
  });

  it('shows an entry of a kind this build doesn’t know without guessing it', async () => {
    installFakeBackend({ history: () => ({ entries: [missing], total: 1 }) });
    await openHistory();
    const table = await screen.findByRole('table', { name: 'History' });
    expect(within(table).getAllByRole('row')[1]?.textContent).toContain('missing.iso');
  });
});
