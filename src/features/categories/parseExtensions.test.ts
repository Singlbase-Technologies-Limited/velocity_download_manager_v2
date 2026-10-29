import { describe, expect, it } from 'vitest';
import { parseExtensions } from './parseExtensions';

describe('parseExtensions', () => {
  it('splits on spaces, commas and semicolons, without dots, lowercase, each once', () => {
    expect(parseExtensions(' .ZIP, 7z;rar  zip ..tar ')).toEqual(['zip', '7z', 'rar', 'tar']);
  });

  it('is empty for nothing typed', () => {
    expect(parseExtensions('  , ; ')).toEqual([]);
  });
});
