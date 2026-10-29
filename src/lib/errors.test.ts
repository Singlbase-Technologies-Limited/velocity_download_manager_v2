import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { ErrorCodeSchema } from '../contract';
import { ApiError, ERROR_MESSAGES, toApiError } from './errors';

const fixture = JSON.parse(
  readFileSync(join(__dirname, '..', '..', 'tests', 'fixtures', 'contract', 'error-codes.json'), 'utf8'),
) as { code: string; message: string }[];

describe('error messages', () => {
  it('are the sentences Rust gives each code, so both sides say the same', () => {
    expect(Object.entries(ERROR_MESSAGES).map(([code, message]) => ({ code, message }))).toEqual(fixture);
  });
});

describe('toApiError', () => {
  it('turns a code from a command into its message', () => {
    const e = toApiError('NAME_TAKEN');
    expect(e).toBeInstanceOf(ApiError);
    expect(e.code).toBe('NAME_TAKEN');
    expect(e.message).toBe(ERROR_MESSAGES.NAME_TAKEN);
  });

  it('keeps an ApiError as it is', () => {
    const e = new ApiError('IO', 'x');
    expect(toApiError(e)).toBe(e);
  });

  it('says plainly when the failure isn’t one of the codes, with what it was', () => {
    for (const odd of ['the shell is gone', new Error('the shell is gone')]) {
      const e = toApiError(odd);
      expect(e.code).toBe('INTERNAL');
      expect(e.message).toBe(ERROR_MESSAGES.INTERNAL);
      expect(e.detail).toBe('the shell is gone');
    }
  });

  it('every code has a message', () => {
    for (const code of ErrorCodeSchema.options) expect(ERROR_MESSAGES[code]).toMatch(/^[A-Z].+\.$/);
  });
});
