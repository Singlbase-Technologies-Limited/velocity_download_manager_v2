import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Toaster } from 'sonner';
import { describe, expect, it } from 'vitest';
import { ConfirmHost } from '../../components/ConfirmHost';
import type { DiagnosticReport } from '../../contract';
import { ERROR_MESSAGES } from '../../lib/errors';
import { createQueryClient } from '../../lib/queries';
import { callsOf, fixture, installFakeBackend } from '../../test/fakeBackend';
import { DiagnosticsPanel } from './DiagnosticsPanel';

// The diagnostics panel (#28, V1's DiagnosticsPanel). #30 puts it on the Settings screen (PO decision on #28), so it
// is tested on its own here.

const sample = fixture('diagnostics.json') as DiagnosticReport;

function renderPanel() {
  return render(
    <QueryClientProvider client={createQueryClient()}>
      <DiagnosticsPanel />
      <ConfirmHost />
      <Toaster />
    </QueryClientProvider>,
  );
}

describe('the diagnostics panel (#28)', () => {
  it('says what the app and its engines are, and that nothing is sent anywhere', async () => {
    installFakeBackend({ diagnostics: () => sample });
    renderPanel();
    const app = await screen.findByRole('region', { name: 'Velocity Download Manager (dev)' });
    expect(app.textContent).toContain('Version 0.1.0');
    expect(app.textContent).toContain('linux x86_64');
    expect(app.textContent).toContain('no analytics or telemetry');
    const engines = screen.getByRole('region', { name: 'Engines' });
    expect(engines.textContent).toContain('libcurl 8.23.0');
    expect(engines.textContent).toContain('OpenSSL/3.5.9');
    expect(engines.textContent).toContain('libtorrent 2.0.11.0');
    expect(engines.textContent).toContain(ERROR_MESSAGES.ENGINE_MISSING);
  });

  it('shows the app’s recent log, newest last', async () => {
    installFakeBackend({ diagnostics: () => sample });
    renderPanel();
    const log = await screen.findByRole('list', { name: 'Recent log' });
    expect(within(log).getAllByRole('listitem')[0]?.textContent).toContain("the torrent helper couldn't start");
  });

  // Before any export succeeds: sonner keeps its toasts from one test to the next.
  it('says nothing was saved when the user cancels the export', async () => {
    const state = installFakeBackend({ diagnostics: () => sample, exportedTo: null });
    const user = userEvent.setup();
    renderPanel();
    await user.click(await screen.findByRole('button', { name: 'Export diagnostics…' }));
    await waitFor(() => { expect(callsOf(state, 'export_diagnostics')).toHaveLength(1); });
    expect(screen.queryByText(/Saved to/)).toBeNull();
  });

  it('exports the diagnostics to a file the user chooses, and says where', async () => {
    const state = installFakeBackend({ diagnostics: () => sample, exportedTo: '/home/user/velocity-diagnostics.json' });
    const user = userEvent.setup();
    renderPanel();
    await user.click(await screen.findByRole('button', { name: 'Export diagnostics…' }));
    await waitFor(() => { expect(callsOf(state, 'export_diagnostics')).toHaveLength(1); });
    expect(await screen.findByText('Saved to /home/user/velocity-diagnostics.json')).toBeTruthy();
  });

  it('restarts the transfer engine after asking, and asks for the diagnostics again', async () => {
    const state = installFakeBackend({ diagnostics: () => sample });
    const user = userEvent.setup();
    renderPanel();
    await user.click(await screen.findByRole('button', { name: 'Restart transfer engine' }));
    const dialog = await screen.findByRole('alertdialog');
    await user.click(within(dialog).getByRole('button', { name: 'Restart engine' }));
    await waitFor(() => { expect(callsOf(state, 'restart_engine')).toHaveLength(1); });
    await waitFor(() => { expect(callsOf(state, 'get_diagnostics').length).toBeGreaterThan(1); });
  });

  it('says why the diagnostics couldn’t be read', async () => {
    installFakeBackend({ diagnostics: () => sample, failures: { get_diagnostics: 'INTERNAL' } });
    renderPanel();
    expect(await screen.findByRole('alert')).toBeTruthy();
  });
});
