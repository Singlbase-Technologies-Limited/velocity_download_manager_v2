import { describe, expect, it } from 'vitest';
import { windowHeading } from './appInfo';

// Validating the app_info response (shape and contract version) is tested in contract.test.ts.

describe('windowHeading', () => {
  it('shows the product name and version', () => {
    expect(windowHeading({ name: 'Velocity Download Manager (dev)', version: '0.1.0', commandApiVersion: 1 })).toBe(
      'Velocity Download Manager (dev) 0.1.0',
    );
  });
});
