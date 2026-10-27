import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { nameForDownload } from './naming';

// Generated from the Rust rules (`name_for_download` in crates/velocity-core/src/naming.rs):
// VELOCITY_UPDATE_FIXTURES=1 cargo test -p velocity-core contract
interface NamingCase {
  url: string;
  typed: string | null;
  name: string | null;
  error: string | null;
}
const fixture = JSON.parse(
  readFileSync(join(__dirname, '..', 'tests', 'fixtures', 'contract', 'naming-cases.json'), 'utf8'),
) as { known: string[]; cases: NamingCase[] };

describe('nameForDownload', () => {
  const known = new Set(fixture.known);

  it.each(fixture.cases)('names $url typed as $typed as the native rule does', (c) => {
    const preview = nameForDownload(c.url, c.typed, known);
    expect(preview).toEqual(c.error ? { ok: false } : { ok: true, name: c.name });
  });

  it('covers the cases that matter to the preview', () => {
    const names = fixture.cases.map((c) => c.name);
    expect(names).toContain('Annual report.pdf');
    expect(names).toContain(null);
    expect(fixture.cases.some((c) => c.error === 'INVALID_FILENAME')).toBe(true);
  });
});
