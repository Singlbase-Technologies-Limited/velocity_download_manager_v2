import { describe, expect, it } from 'vitest';
import { parseAppInfo, windowHeading } from './appInfo';

describe('parseAppInfo', () => {
  it('accepts the shape returned by the app_info command', () => {
    expect(parseAppInfo({ name: 'Velocity Download Manager V2 (dev)', version: '0.1.0' })).toEqual({
      name: 'Velocity Download Manager V2 (dev)',
      version: '0.1.0',
    });
  });

  it('rejects anything else instead of rendering it', () => {
    expect(() => parseAppInfo(null)).toThrow('Unexpected app_info response');
    expect(() => parseAppInfo({ name: 'x' })).toThrow('Unexpected app_info response');
    expect(() => parseAppInfo({ name: 1, version: '0.1.0' })).toThrow('Unexpected app_info response');
  });
});

describe('windowHeading', () => {
  it('shows the product name and version', () => {
    expect(windowHeading({ name: 'Velocity Download Manager V2 (dev)', version: '0.1.0' })).toBe(
      'Velocity Download Manager V2 (dev) 0.1.0',
    );
  });
});
