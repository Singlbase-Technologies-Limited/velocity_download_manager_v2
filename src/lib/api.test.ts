import { mockIPC } from '@tauri-apps/api/mocks';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { api } from './api';
import { ApiError, ERROR_MESSAGES } from './errors';

const fixture = (name: string): unknown =>
  JSON.parse(readFileSync(join(__dirname, '..', '..', 'tests', 'fixtures', 'contract', name), 'utf8'));

describe('the command bridge', () => {
  it('calls the command with its arguments and returns the checked answer', async () => {
    const calls: [string, unknown][] = [];
    mockIPC((cmd, args) => {
      calls.push([cmd, args]);
      return fixture('shell-settings.json');
    });
    const settings = await api.completeOnboarding('/home/user/Downloads');
    expect(calls).toEqual([['complete_onboarding', { directory: '/home/user/Downloads' }]]);
    expect(settings).toEqual(fixture('shell-settings.json'));
  });

  it('refuses an answer that doesn’t match the contract', async () => {
    mockIPC(() => ({ ...(fixture('shell-settings.json') as object), extra: 1 }));
    await expect(api.shellSettings()).rejects.toMatchObject({ code: 'INTERNAL' });
  });

  it('turns a command’s error code into an ApiError with its message', async () => {
    mockIPC(() => {
      // Tauri rejects a failed command with its serialised error: the code, a string.
      // eslint-disable-next-line @typescript-eslint/only-throw-error
      throw 'DESTINATION_MISSING';
    });
    const failure = await api.completeOnboarding('/nope').catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(ApiError);
    expect(failure).toMatchObject({ code: 'DESTINATION_MISSING', message: ERROR_MESSAGES.DESTINATION_MISSING });
  });

  it('a cancelled folder chooser is null, not an error', async () => {
    mockIPC(() => null);
    await expect(api.chooseDirectory(null)).resolves.toBeNull();
  });
});
