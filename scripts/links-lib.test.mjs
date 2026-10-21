import { describe, expect, it } from 'vitest';
import { brokenLinks, githubSlug } from './links-lib.mjs';

describe('githubSlug', () => {
  it('matches GitHub heading anchors', () => {
    expect(githubSlug('Running beside the original Velocity Download Manager')).toBe(
      'running-beside-the-original-velocity-download-manager',
    );
    expect(githubSlug('TypeScript and React')).toBe('typescript-and-react');
    expect(githubSlug('V1 `CLAUDE.md`')).toBe('v1-claudemd');
    expect(githubSlug('1. Read the issue completely')).toBe('1-read-the-issue-completely');
  });
});

describe('brokenLinks', () => {
  const files = {
    'README.md': '# Title\n\nSee [docs](docs/A.md), [anchor](docs/A.md#second-part) and [web](https://example.com).',
    'docs/A.md': '# A\n\n## Second part\n\n[back](../README.md) [self](#a) [missing](B.md) [bad anchor](#nope)',
  };
  const read = (p) => files[p];

  it('reports only missing files and missing anchors', () => {
    expect(brokenLinks(['README.md', 'docs/A.md'], read)).toEqual([
      { file: 'docs/A.md', link: 'B.md', problem: 'missing file' },
      { file: 'docs/A.md', link: '#nope', problem: 'missing anchor' },
    ]);
  });

  it('ignores external links and links inside code', () => {
    const only = { 'x.md': 'Use `[x](nowhere.md)` and\n\n```md\n[y](nowhere.md)\n```\n[z](https://a.b/c)' };
    expect(brokenLinks(['x.md'], (p) => only[p])).toEqual([]);
  });

  it('treats a directory link as valid when the directory has files', () => {
    const tree = { 'a.md': '[dir](sub/)', 'sub/README.md': '# Sub' };
    expect(brokenLinks(['a.md'], (p) => tree[p], (p) => Object.keys(tree).some((k) => k.startsWith(p)))).toEqual([]);
  });
});
