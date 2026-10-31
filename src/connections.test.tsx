import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ConnectionLine } from './ConnectionLine';
import { connectionSummary, limitReason } from './connections';
import { ConnectionLimitSchema } from './contract';

describe('limitReason', () => {
  it('every limit the engine reports has its own sentence', () => {
    // The fixture generated from Rust: a new limit without a sentence fails here.
    const limits = ConnectionLimitSchema.array().parse(
      JSON.parse(readFileSync(join(__dirname, '..', 'tests', 'fixtures', 'contract', 'connection-limits.json'), 'utf8')),
    );
    const sentences = limits.map(limitReason);
    // Each completes "…asked for: <reason>.", so it has no full stop of its own.
    for (const [i, s] of sentences.entries()) expect(s, limits[i]).toMatch(/^\S.{10,}[^.]$/);
    expect(new Set(sentences).size).toBe(limits.length);
  });
});

describe('connectionSummary', () => {
  it('names the limit that holds a download below what it asked for', () => {
    expect(connectionSummary(3, { requested: 8, allowed: 4, limitedBy: 'noGain' })).toBe(
      'Using 3 connections, up to 4 of the 8 asked for: more connections stopped making it faster.',
    );
    expect(connectionSummary(2, { requested: 16, allowed: 2, limitedBy: 'perHostCap' })).toBe(
      'Using 2 connections, up to 2 of the 16 asked for: the performance profile allows no more to one server.',
    );
    // #221: the ramp starts with all of them and tries fewer, so it no longer says "adding".
    expect(connectionSummary(7, { requested: 8, allowed: 7, limitedBy: 'rampingUp' })).toBe(
      'Using 7 connections, up to 7 of the 8 asked for: Velocity is still finding how many connections help.',
    );
  });

  it('says only the count when nothing holds it back, or nothing is known', () => {
    expect(connectionSummary(8, { requested: 8, allowed: 8, limitedBy: null })).toBe('Using 8 connections.');
    expect(connectionSummary(1, { requested: 8, allowed: 8, limitedBy: null })).toBe('Using 1 connection.');
    expect(connectionSummary(0, null)).toBe('Not connected.');
  });
});

describe('ConnectionLine', () => {
  it('shows the summary with the reason', () => {
    const html = renderToStaticMarkup(
      <ConnectionLine active={2} detail={{ requested: 8, allowed: 2, limitedBy: 'serverThrottling' }} />,
    );
    expect(html).toContain('Using 2 connections, up to 2 of the 8 asked for: the server asked Velocity to slow down.');
  });
});
