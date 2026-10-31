import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { acceptedCandidate, candidateArtifact, parseReleaseTag, releaseFileName, releaseNotes, releaseRefusal } from './release-lib.mjs';
import { strictNeedsProblems } from './release-candidate-lib.mjs';

const commit = '3a93690d1f0c2b9e8a7d6c5b4a39281706f5e4d3';
const green = { id: 1, head_sha: commit, head_branch: 'main', status: 'completed', conclusion: 'success' };

describe('release tags', () => {
  it('are vX.Y.Z, never a pre-release', () => {
    expect(parseReleaseTag('v0.1.0')).toBe('0.1.0');
    for (const tag of ['v0.1.0-rc.1', '0.1.0', 'v0.1', 'release']) expect(parseReleaseTag(tag), tag).toBeNull();
  });
});

describe('the accepted candidate', () => {
  it("is the version's highest rc tag on the commit", () => {
    expect(acceptedCandidate(['v0.1.0-rc.1', 'v0.1.0-rc.3', 'v0.1.0-rc.2', 'v0.1.0'], '0.1.0')).toBe('v0.1.0-rc.3');
    expect(acceptedCandidate(['v0.1.0-rc.9', 'v0.1.0-rc.10'], '0.1.0')).toBe('v0.1.0-rc.10');
  });

  it("is none without one of this version's", () => {
    expect(acceptedCandidate(['v0.1.0', 'v0.0.9-rc.1'], '0.1.0')).toBeNull();
  });
});

describe('which tags may become a release', () => {
  const ok = { tag: 'v0.1.0', version: '0.1.0', commit, onMain: true, mainChecks: green, candidate: 'v0.1.0-rc.2' };

  it('a vX.Y.Z tag on a candidate commit of main with green Main checks', () => {
    expect(releaseRefusal(ok)).toBeNull();
  });

  it('not a tag on a commit that is not on main (#18)', () => {
    expect(releaseRefusal({ ...ok, onMain: false })).toBe(`${commit} is not on main: release candidates come only from main (#17)`);
  });

  it('not without green Main checks, or without an accepted candidate', () => {
    expect(releaseRefusal({ ...ok, mainChecks: null })).toMatch(/no green Main checks run/);
    expect(releaseRefusal({ ...ok, candidate: null })).toMatch(/no release-candidate tag v0.1.0-rc.N/);
  });

  it("not a pre-release tag, or one for another version than package.json's", () => {
    expect(releaseRefusal({ ...ok, tag: 'v0.1.0-rc.2' })).toBe('v0.1.0-rc.2 is not a release tag (vX.Y.Z)');
    expect(releaseRefusal({ ...ok, tag: 'v0.2.0' })).toBe('v0.2.0 is for 0.2.0, but package.json says 0.1.0');
  });
});

describe("the candidate's files", () => {
  it("are found as the candidate's artifact, unless it expired", () => {
    const artifacts = [
      { name: 'velocity-v2-windows-installer', expired: false },
      { name: 'velocity-v2-0.1.0-rc.2-3a93690-20261102', expired: false },
    ];
    expect(candidateArtifact(artifacts, 'v0.1.0-rc.2', commit)?.name).toBe('velocity-v2-0.1.0-rc.2-3a93690-20261102');
    expect(candidateArtifact([{ ...artifacts[1], expired: true }], 'v0.1.0-rc.2', commit)).toBeNull();
    expect(candidateArtifact(artifacts, 'v0.1.0-rc.1', commit)).toBeNull();
  });

  it('are renamed for the release, keeping what each is', () => {
    expect(releaseFileName('velocity-v2-0.1.0-rc.2-3a93690-20261102-windows-x64-setup.exe', '0.1.0')).toBe('velocity-v2-0.1.0-windows-x64-setup.exe');
    expect(releaseFileName('velocity-v2-0.1.0-rc.2-3a93690-20261102-licence-report-linux.json', '0.1.0')).toBe('velocity-v2-0.1.0-licence-report-linux.json');
    expect(() => releaseFileName('SHA256SUMS', '0.1.0')).toThrow(/isn't a release candidate's file/);
  });
});

describe('the release notes', () => {
  const changelog = '# Changelog\n\nIntro.\n\n## Unreleased\n\n- next\n\n## 0.1.0 - 2026-11-02\n\n### Features\n\n- downloads\n\n## 0.0.9\n\n- old\n';

  it("are the version's CHANGELOG.md section, without its heading", () => {
    expect(releaseNotes(changelog, '0.1.0')).toBe('### Features\n\n- downloads\n');
    expect(releaseNotes(changelog, '0.0.9')).toBe('- old\n');
  });

  it('must be written before the release', () => {
    expect(() => releaseNotes(changelog, '0.2.0')).toThrow('CHANGELOG.md has no "## 0.2.0" section');
    expect(() => releaseNotes('## 0.1.0\n\n## 0.0.9\n- x\n', '0.1.0')).toThrow('section is empty');
  });
});

describe('the release workflow', () => {
  it('runs a job only when every job it needs succeeded (#91)', () => {
    const text = readFileSync(join(import.meta.dirname, '..', '.github', 'workflows', 'release.yml'), 'utf8');
    expect(strictNeedsProblems(text)).toEqual([]);
  });

  it('creates the draft only in the protected release environment, with the only write permission', () => {
    const text = readFileSync(join(import.meta.dirname, '..', '.github', 'workflows', 'release.yml'), 'utf8');
    expect(text).toMatch(/^permissions:\n {2}contents: read\n/m);
    expect(text.match(/contents: write/g)).toHaveLength(1);
    expect(text).toMatch(/environment: release/);
    expect(text).toMatch(/gh release create .*--draft/);
    expect(text).not.toMatch(/gh release (edit|upload)|--draft=false|gh release create (?!.*--draft)/);
  });
});
