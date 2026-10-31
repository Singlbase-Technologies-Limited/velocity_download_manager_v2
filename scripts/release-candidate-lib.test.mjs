import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  candidateBuildInfo,
  candidateName,
  compactDate,
  mainChecksRun,
  packageNames,
  parseRcTag,
  refusal,
  sha256sums,
  strictNeedsProblems,
} from './release-candidate-lib.mjs';

const commit = '3a93690d1f0c2b9e8a7d6c5b4a39281706f5e4d3';
const date = new Date('2026-10-31T09:15:00Z');
const green = { id: 41113305464, head_sha: commit, head_branch: 'main', status: 'completed', conclusion: 'success' };

describe('release-candidate tags', () => {
  it('are vX.Y.Z-rc.N', () => {
    expect(parseRcTag('v0.1.0-rc.1')).toEqual({ version: '0.1.0', rc: 1 });
    expect(parseRcTag('v10.20.30-rc.12')).toEqual({ version: '10.20.30', rc: 12 });
  });

  it('are nothing else', () => {
    for (const tag of ['v0.1.0', '0.1.0-rc.1', 'v0.1.0-rc.0', 'v0.1-rc.1', 'v0.1.0-beta.1', 'v0.1.0-rc.1x', 'main']) {
      expect(parseRcTag(tag), tag).toBeNull();
    }
  });
});

describe('which commits may become a release candidate', () => {
  const ok = { commit, onMain: true, mainChecks: green, version: '0.1.0' };

  it('a commit on main with green Main checks', () => {
    expect(refusal(ok)).toBeNull();
    expect(refusal({ ...ok, tag: 'v0.1.0-rc.1' })).toBeNull();
  });

  it('not a commit that is not on main', () => {
    expect(refusal({ ...ok, onMain: false })).toBe(`${commit} is not on main: release candidates come only from main (#17)`);
  });

  it('not a commit without green Main checks', () => {
    expect(refusal({ ...ok, mainChecks: null })).toMatch(/has no green Main checks run on main/);
  });

  it('not from a tag that is not a release-candidate tag, or is for another version', () => {
    expect(refusal({ ...ok, tag: 'v0.1.0' })).toBe('v0.1.0 is not a release-candidate tag (vX.Y.Z-rc.N)');
    expect(refusal({ ...ok, tag: 'v0.2.0-rc.1' })).toBe('v0.2.0-rc.1 is for 0.2.0, but package.json says 0.1.0');
  });

  it('finds the green Main checks run on main for exactly that commit', () => {
    const runs = [
      { ...green, id: 1, conclusion: 'failure' },
      { ...green, id: 2, head_branch: 'batch/1-2-3' },
      { ...green, id: 3, head_sha: 'f'.repeat(40) },
      { ...green, id: 4, status: 'in_progress', conclusion: null },
      { ...green, id: 5 },
    ];
    expect(mainChecksRun(runs, commit)?.id).toBe(5);
    expect(mainChecksRun(runs.slice(0, 4), commit)).toBeNull();
  });
});

describe('names', () => {
  it('carry the version, the candidate, the short commit and the UTC date', () => {
    expect(compactDate(new Date('2026-12-31T23:59:59Z'))).toBe('20261231');
    expect(candidateName({ version: '0.1.0', tag: 'v0.1.0-rc.2', commit, date })).toBe('velocity-v2-0.1.0-rc.2-3a93690-20261031');
    expect(candidateName({ version: '0.1.0', tag: null, commit, date })).toBe('velocity-v2-0.1.0-main-3a93690-20261031');
  });

  it('rename each package the packaging workflows made', () => {
    const files = [
      'Velocity Download Manager V2_0.1.0_x64-setup.exe',
      'Velocity Download Manager V2_0.1.0_amd64.deb',
      'Velocity Download Manager V2-0.1.0-1.x86_64.rpm',
      'expected-files.txt',
    ];
    expect(packageNames('velocity-v2-0.1.0-rc.1-3a93690-20261031', files)).toEqual([
      { from: files[0], to: 'velocity-v2-0.1.0-rc.1-3a93690-20261031-windows-x64-setup.exe' },
      { from: files[1], to: 'velocity-v2-0.1.0-rc.1-3a93690-20261031-linux-amd64.deb' },
      { from: files[2], to: 'velocity-v2-0.1.0-rc.1-3a93690-20261031-linux-x86_64.rpm' },
    ]);
  });

  it('refuse a missing or doubled package', () => {
    expect(() => packageNames('n', ['a.deb', 'b.rpm'])).toThrow('expected one windows-x64-setup.exe package, found: none');
    expect(() => packageNames('n', ['a-setup.exe', 'a.deb', 'b.deb', 'c.rpm'])).toThrow('expected one linux-amd64.deb package, found: a.deb, b.deb');
  });
});

describe('the records', () => {
  it('SHA256SUMS lists every file in the sha256sum format, sorted', () => {
    expect(sha256sums([
      { name: 'b.deb', sha256: 'bb' },
      { name: 'a-setup.exe', sha256: 'aa' },
    ])).toBe('aa  a-setup.exe\nbb  b.deb\n');
  });

  it('build-info.json says what was built, from which commit and run, with each platform\'s native records', () => {
    const platforms = { windows: { native: { schema: 1, target: 'x86_64-pc-windows-msvc' } } };
    const info = candidateBuildInfo({
      name: 'velocity-v2-0.1.0-rc.1-3a93690-20261031',
      version: '0.1.0',
      tag: 'v0.1.0-rc.1',
      commit,
      date,
      run: { id: 7, url: 'https://github.com/o/r/actions/runs/7' },
      files: [{ name: 'x-setup.exe', sha256: 'aa', bytes: 3 }],
      platforms,
    });
    expect(info).toEqual({
      schema: 1,
      name: 'velocity-v2-0.1.0-rc.1-3a93690-20261031',
      version: '0.1.0',
      tag: 'v0.1.0-rc.1',
      commit,
      built: '2026-10-31T09:15:00.000Z',
      run: { id: 7, url: 'https://github.com/o/r/actions/runs/7' },
      files: [{ name: 'x-setup.exe', sha256: 'aa', bytes: 3 }],
      platforms,
    });
  });
});

describe('strict needs (#17, from #91)', () => {
  const strict = `name: x
on:
  workflow_dispatch:
jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - run: true
  package:
    needs: verify
    uses: ./.github/workflows/package-linux.yml
  assemble:
    needs: [verify, package]
    runs-on: ubuntu-latest
    steps:
      - name: Save a cache even after a failed step
        if: \${{ !cancelled() }}
        run: true
`;

  it('passes a workflow whose jobs run only after every job they need succeeded', () => {
    expect(strictNeedsProblems(strict)).toEqual([]);
  });

  it('refuses a job that runs after a failed or cancelled job it needs', () => {
    for (const condition of ['always()', '!cancelled()', 'failure()', 'cancelled()']) {
      const loose = strict.replace('    needs: [verify, package]\n', `    needs: [verify, package]\n    if: \${{ ${condition} }}\n`);
      expect(strictNeedsProblems(loose), condition).toEqual([`line 14: a job condition that runs after a failed or cancelled job: if: \${{ ${condition} }}`]);
    }
  });

  it('refuses continue-on-error, on a job or a step', () => {
    const loose = strict.replace('      - run: true\n', '      - run: true\n        continue-on-error: true\n');
    expect(strictNeedsProblems(loose)).toEqual(['line 9: continue-on-error lets the workflow pass with a failed step']);
  });

  it('verifies from the checkout, which keeps no credentials, so it never fetches', () => {
    // Runs 41184389337 and 41184399701 failed before verifying: `git fetch origin main` needed
    // the credentials the checkout doesn't keep. fetch-depth 0 brings origin/main already.
    const text = readFileSync(join(import.meta.dirname, '..', '.github', 'workflows', 'release-candidate.yml'), 'utf8');
    expect(text).toMatch(/fetch-depth: 0/);
    expect(text).not.toMatch(/^\s+git fetch\b/m);
  });

  it("builds a candidate's packages from nothing: no cache an earlier run saved (#233)", () => {
    const text = readFileSync(join(import.meta.dirname, '..', '.github', 'workflows', 'release-candidate.yml'), 'utf8');
    for (const platform of ['windows', 'linux']) {
      const job = new RegExp(String.raw`uses: \./\.github/workflows/package-${platform}\.yml\n\s+with:\n(?:\s+(?:#.*|[\w-]+: .*)\n)*?\s+cold: true\n`);
      expect(text, platform).toMatch(job);
    }
  });

  it('holds for the release-candidate workflow and the packaging workflows it calls', () => {
    for (const file of ['release-candidate.yml', 'package-windows.yml', 'package-linux.yml']) {
      const text = readFileSync(join(import.meta.dirname, '..', '.github', 'workflows', file), 'utf8');
      expect(strictNeedsProblems(text), file).toEqual([]);
    }
  });
});
