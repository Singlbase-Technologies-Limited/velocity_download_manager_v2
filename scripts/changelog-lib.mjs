// The changelog draft (#84, docs/RELEASE_PROCESS.md): one section of CHANGELOG.md, from the subjects
// of the squash-merged commits on `main`, which are the pull requests' Conventional Commit titles
// (https://www.conventionalcommits.org/en/v1.0.0/, docs/CONVENTIONS.md). The PO edits the draft
// into release notes. No I/O here, so every rule is unit tested (changelog-lib.test.mjs).

/** The changelog's groups, in order, with the commit types each takes. */
export const GROUPS = [
  { title: 'Breaking changes', types: [] },
  { title: 'Features', types: ['feat'] },
  { title: 'Fixes', types: ['fix'] },
  { title: 'Performance', types: ['perf'] },
  { title: 'Security', types: ['security'] },
  { title: 'Documentation', types: ['docs'] },
  { title: 'Packaging and builds', types: ['build'] },
  { title: 'Continuous integration', types: ['ci'] },
  { title: 'Tests', types: ['test'] },
  { title: 'Other changes', types: ['chore', 'refactor', 'style', 'revert'] },
];

const TITLE = /^([a-z]+)(?:\(([^)]+)\))?(!)?: (.+)$/;

/**
 * A squash-merge subject such as `fix(engine)!: keep the part file (#74) (#212)`: its type, scope,
 * whether it breaks something, the description, and the numbers in the trailing `(#N)` groups.
 * GitHub appends the pull request's number last, so the one before it, when there is one, is the
 * issue the title named. Null when the subject isn't a Conventional Commit.
 */
export function parseSubject(subject) {
  let rest = subject.trim();
  const numbers = [];
  for (let m; (m = /\s*\(#(\d+)\)$/.exec(rest)); ) {
    numbers.unshift(Number(m[1]));
    rest = rest.slice(0, m.index);
  }
  const m = TITLE.exec(rest);
  if (!m) return null;
  return {
    type: m[1],
    scope: m[2] ?? null,
    breaking: m[3] === '!',
    description: m[4],
    pr: numbers.length ? numbers[numbers.length - 1] : null,
    issue: numbers.length > 1 ? numbers[numbers.length - 2] : null,
  };
}

/** One changelog line for a parsed subject. */
function entry(c) {
  const scope = c.scope ? `**${c.scope}:** ` : '';
  const refs = [c.issue ? `#${c.issue}` : null, c.pr ? `PR #${c.pr}` : null].filter(Boolean).join(', ');
  return `- ${scope}${c.description}${refs ? ` (${refs})` : ''}`;
}

/**
 * The section for `heading` (such as `0.1.0 - 2026-11-02` or `Unreleased`) from `subjects`, newest
 * first as `git log` lists them; the section lists them oldest first. Subjects that aren't
 * Conventional Commits go to Other changes as they are, and are returned in `unparsed` so the
 * draft can say so.
 */
export function changelogSection(heading, subjects) {
  const parsed = [];
  const unparsed = [];
  for (const subject of [...subjects].reverse()) {
    const c = parseSubject(subject);
    if (c) parsed.push(c);
    else unparsed.push(subject);
  }
  const lines = [`## ${heading}`, ''];
  for (const group of GROUPS) {
    const items = group.title === 'Breaking changes' ? parsed.filter((c) => c.breaking) : parsed.filter((c) => !c.breaking && group.types.includes(c.type));
    const extra = group.title === 'Other changes' ? [...parsed.filter((c) => !c.breaking && !GROUPS.some((g) => g.types.includes(c.type))), ...unparsed.map((s) => ({ description: s }))] : [];
    const all = [...items, ...extra];
    if (!all.length) continue;
    lines.push(`### ${group.title}`, '', ...all.map(entry), '');
  }
  if (lines.length === 2) lines.push('No changes.', '');
  return { text: lines.join('\n'), unparsed };
}
