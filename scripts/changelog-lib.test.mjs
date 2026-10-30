import { describe, expect, it } from 'vitest';
import { changelogSection, parseSubject } from './changelog-lib.mjs';

describe('parseSubject', () => {
  it('reads a squash merge of an issue PR: type, description, issue and PR', () => {
    expect(parseSubject('build: Windows installer, per-user NSIS, as the release identity (#81) (#220)')).toEqual({
      type: 'build',
      scope: null,
      breaking: false,
      description: 'Windows installer, per-user NSIS, as the release identity',
      pr: 220,
      issue: 81,
    });
  });

  it('reads a scope, a breaking change, and a title with only the PR number', () => {
    expect(parseSubject('fix(engine)!: keep the part file (#212)')).toEqual({ type: 'fix', scope: 'engine', breaking: true, description: 'keep the part file', pr: 212, issue: null });
  });

  it('keeps numbers inside the description', () => {
    expect(parseSubject('docs: what #80 found (#219)')?.description).toBe('what #80 found');
  });

  it('is null for a subject that is not a Conventional Commit', () => {
    for (const s of ['Merge branch main', 'Update README.md', 'Feat: capital type', 'fix:missing space']) expect(parseSubject(s), s).toBeNull();
  });
});

describe('changelogSection', () => {
  // As `git log --first-parent` lists them: newest first.
  const subjects = [
    'perf: direct downloads start at the full connection count (#221) (#226)',
    'Update notes',
    'feat!: a new settings format (#230) (#231)',
    'test: the snapshot timing test waits for the last change (#188) (#216)',
    'docs: user guides (#79) (#224)',
    'fix(ui): the column chooser keeps Name (#59) (#200)',
    'feat: release-candidate builds (#17) (#223)',
    'refactor: one packaging step for both platforms (#82) (#222)',
    'wip: something unusual (#232)',
  ];

  it('groups the changes, oldest first, with their issue and PR', () => {
    const { text, unparsed } = changelogSection('0.1.0 - 2026-11-02', subjects);
    expect(text).toBe(
      [
        '## 0.1.0 - 2026-11-02',
        '',
        '### Breaking changes',
        '',
        '- a new settings format (#230, PR #231)',
        '',
        '### Features',
        '',
        '- release-candidate builds (#17, PR #223)',
        '',
        '### Fixes',
        '',
        '- **ui:** the column chooser keeps Name (#59, PR #200)',
        '',
        '### Performance',
        '',
        '- direct downloads start at the full connection count (#221, PR #226)',
        '',
        '### Documentation',
        '',
        '- user guides (#79, PR #224)',
        '',
        '### Tests',
        '',
        '- the snapshot timing test waits for the last change (#188, PR #216)',
        '',
        '### Other changes',
        '',
        '- one packaging step for both platforms (#82, PR #222)',
        '- something unusual (PR #232)',
        '- Update notes',
        '',
      ].join('\n'),
    );
    expect(unparsed).toEqual(['Update notes']);
  });

  it('says when there is nothing', () => {
    expect(changelogSection('Unreleased', []).text).toBe('## Unreleased\n\nNo changes.\n');
  });
});
