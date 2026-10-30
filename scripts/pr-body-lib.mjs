// Checks a pull request's title and description against .github/PULL_REQUEST_TEMPLATE.md (PO decision on #59), so
// every PR reads the same way. The rules come from the template itself: its sections and their order, its checklist
// items in its words, and its tables. On top of that, the conventions of docs/DEVELOPMENT_WORKFLOW.md: one linked
// issue, closed only by the first line, a Conventional Commit title, and the attribution line at the end.
// Used by `pnpm check:pr-body` and by the agent guard before `gh pr create` and `gh pr edit`.

export const ATTRIBUTION = '🤖 Generated with [Claude Code](https://claude.com/claude-code)';
const TYPES = 'feat|fix|chore|docs|test|build|ci|security|perf|refactor';
const RESULTS = /^(PASS|FAIL|BLOCKED|NOT RUN)\b/;
/** GitHub's closing keywords, which it acts on anywhere in the text. */
const CLOSING = /\b(close[sd]?|fix(?:e[sd])?|resolve[sd]?)\b:?\s*#(\d+)/gi;

const withoutComments = (text) => text.replace(/<!--[\s\S]*?-->/g, '');
const normalise = (row) => row.replace(/\s*\|\s*/g, ' | ').replace(/\s+/g, ' ').trim();
const cellsOf = (row) => row.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim());

/** The `## ` sections of a Markdown text, in order: `{ title, body }`. The text before the first is dropped. */
function sections(text) {
  const parts = text.split(/^## +(.+?)\s*$/m);
  const out = [];
  for (let i = 1; i < parts.length; i += 2) out.push({ title: parts[i], body: parts[i + 1] ?? '' });
  return out;
}

/** What the template asks of each section: its checklist items and its tables' header rows. */
function rulesOf(template) {
  return sections(withoutComments(template)).map(({ title, body }) => ({
    title,
    items: [...body.matchAll(/^- \[ \] (.+?)\s*$/gm)].map((m) => m[1]),
    tables: body.split('\n').filter((line, i, all) => line.trim().startsWith('|') && !all[i - 1]?.trim().startsWith('|')).map(normalise),
  }));
}

/** The table that starts with `header` in `text`: its data rows, as cells. */
function tableRows(text, header) {
  const lines = text.split('\n');
  const start = lines.findIndex((l) => normalise(l) === header);
  if (start < 0) return null;
  const rows = [];
  for (const line of lines.slice(start + 2)) {
    if (!line.trim().startsWith('|')) break;
    rows.push(cellsOf(line));
  }
  return rows;
}

/** The linked issue, from the description's first line: `{ kind: 'Closes' | 'Part of', issue }`, or null. */
export function linkedIssue(body) {
  const linked = sections(body).find((s) => s.title === 'Linked issue');
  const first = linked?.body.split('\n').map((l) => l.trim()).find((l) => l);
  const m = first && /^(Closes|Part of) #(\d+)\.$/.exec(first);
  return m ? { kind: m[1], issue: Number(m[2]) } : null;
}

/** What is wrong with `body` against `template`; empty when it follows it. */
export function checkPrBody(body, template) {
  const problems = [];
  const text = body.replace(/\r\n/g, '\n');
  const rules = rulesOf(template);
  const found = sections(text);
  const titles = found.map((s) => s.title);
  const expected = rules.map((r) => r.title);

  for (const t of expected) if (!titles.includes(t)) problems.push(`The section "${t}" is missing.`);
  for (const t of titles) if (!expected.includes(t)) problems.push(`"${t}" isn't a section of the template.`);
  for (const t of new Set(titles)) if (titles.filter((x) => x === t).length > 1) problems.push(`The section "${t}" appears twice.`);
  const known = titles.filter((t) => expected.includes(t));
  if (known.some((t, i) => i > 0 && expected.indexOf(t) < expected.indexOf(known[i - 1] ?? ''))) {
    problems.push(`The sections are out of the template's order: ${expected.join(', ')}.`);
  }

  // The first line links the issue, and nothing else closes one.
  const linked = linkedIssue(text);
  if (!linked) problems.push('The first line of "Linked issue" must be "Closes #N." or "Part of #N.".');
  const firstLine = linked ? `${linked.kind} #${String(linked.issue)}.` : null;
  const rest = firstLine ? text.replace(firstLine, '') : text;
  for (const m of rest.matchAll(CLOSING)) {
    const issue = Number(m[2]);
    problems.push(
      linked?.kind === 'Part of' && issue === linked.issue
        ? `"${m[0]}" closes #${String(issue)}, but the description says "Part of #${String(issue)}". GitHub acts on closing words anywhere.`
        : `"${m[0]}" closes #${String(issue)}: only the first line may close an issue, and only the linked one. Write "Part of #${String(issue)}" or "Refs #${String(issue)}".`,
    );
  }

  for (const rule of rules) {
    const section = found.find((s) => s.title === rule.title);
    if (!section) continue;
    const content = section.body.split(/^---\s*$/m)[0] ?? '';
    for (const item of rule.items) {
      const kept = content.split('\n').some((l) => /^- \[[ xX]\] /.test(l) && l.replace(/^- \[[ xX]\] /, '').startsWith(item));
      if (!kept) problems.push(`"${rule.title}" lacks the item "${item}", in the template's words (more words may follow it).`);
    }
    for (const header of rule.tables) {
      const rows = tableRows(content, header);
      if (!rows) {
        problems.push(`"${rule.title}" lacks the table "${header}".`);
        continue;
      }
      if (rows.length === 0) problems.push(`"${rule.title}" has no row in its table.`);
      const names = cellsOf(header);
      const result = names.indexOf('Result');
      for (const row of rows) {
        if (row.some((c, i) => !c && names[i] !== 'Notes')) problems.push(`"${rule.title}" has a row with an empty cell: | ${row.join(' | ')} |.`);
        const value = row[result] ?? '';
        if (result >= 0 && value && (!RESULTS.test(value) || value.includes(' / '))) {
          problems.push(`"${row[0] ?? ''}" has the result "${value}"; it must be PASS, FAIL, BLOCKED or NOT RUN.`);
        }
      }
    }
    if (/^-\s*$/m.test(content)) problems.push(`"${rule.title}" still has an empty "-" item from the template.`);
  }

  const section = (title) => found.find((s) => s.title === title)?.body ?? '';
  if (titles.includes('Change type') && !/^- \[[xX]\] /m.test(section('Change type'))) problems.push('"Change type": no change type is ticked.');
  if (titles.includes('What changed') && !section('What changed').includes('**Not included:**')) problems.push('"What changed" lacks "**Not included:**", with what the PR leaves out.');
  if (titles.includes('Tests')) {
    const tests = section('Tests');
    if (!/Commands actually run, with their results:\s*\n+```text\n[\s\S]*?\S[\s\S]*?\n```/.test(tests)) problems.push('"Tests" lacks the commands actually run, with their results, in a ```text block.');
    if (!/^Verification level reached \(docs\/TESTING\.md\): L[1-5]\b/m.test(tests)) problems.push('"Tests" lacks the verification level reached (L1 to L5).');
  }
  if (titles.includes('Documentation') && !(tableRows(section('Documentation'), normalise('| Document | Updated, or why no update was needed |')) ?? []).some((r) => r[0]?.includes('README.md'))) {
    problems.push('"Documentation" has no row for README.md: it is reviewed in every PR.');
  }
  const review = section('Things to check during review').split(/^---\s*$/m)[0] ?? '';
  if (titles.includes('Things to check during review') && !/^- +\S/m.test(withoutComments(review))) problems.push('"Things to check during review" is empty.');
  if (text.includes('<!--')) problems.push('A comment left from the template: remove every <!-- … -->.');

  const footer = template.split('\n').find((l) => l.startsWith('The reviewer checks this PR'));
  if (footer && !text.includes(footer)) problems.push(`The reviewer's footer is missing: "${footer}"`);
  if (!text.trimEnd().endsWith(ATTRIBUTION)) problems.push(`The description must end with the attribution line: ${ATTRIBUTION}`);
  return problems;
}

/** What is wrong with the title: a Conventional Commit (docs/DEVELOPMENT_WORKFLOW.md) ending with the linked issue. */
export function checkPrTitle(title, body) {
  const problems = [];
  if (!new RegExp(`^(${TYPES})(\\([a-z0-9][a-z0-9-]*\\))?!?: \\S`).test(title)) {
    problems.push(`The title isn't a Conventional Commit: "<type>(<scope>): <description> (#N)", with a type of ${TYPES.split('|').join(', ')}.`);
  }
  const linked = linkedIssue(body.replace(/\r\n/g, '\n'));
  if (linked && !title.endsWith(` (#${String(linked.issue)})`)) problems.push(`The title must end with "(#${String(linked.issue)})", the linked issue.`);
  return problems;
}

/** `pnpm check:pr-body`'s arguments: the description file, and the title after `--title`, which is optional. Null when
 * the file is missing or `--title` has no value. */
export function prBodyArgs(args) {
  const at = args.indexOf('--title');
  const title = at >= 0 ? args[at + 1] : undefined;
  if (at >= 0 && title === undefined) return null;
  const file = args.find((a, i) => !a.startsWith('--') && (at < 0 || i !== at + 1));
  return file === undefined ? null : { file, title };
}
