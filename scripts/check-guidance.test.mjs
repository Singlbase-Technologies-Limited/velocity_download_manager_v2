import { describe, expect, it } from 'vitest';
import { loadGuidance } from './check-guidance.mjs';
import { checkGuidance } from './guidance-lib.mjs';

// The real guidance files (#88): this fails as soon as one drifts, on any branch Vitest runs on.
describe('the repository guidance', () => {
  it('passes every guidance check', () => {
    const input = loadGuidance();
    expect(Object.keys(input.files)).toEqual(expect.arrayContaining(['AGENTS.md', 'CLAUDE.md']));
    expect(checkGuidance(input)).toEqual([]);
  });
});
