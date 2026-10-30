import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { checkPrBody, checkPrTitle, prBodyArgs } from './pr-body-lib.mjs';

// PR descriptions follow .github/PULL_REQUEST_TEMPLATE.md (PO decision on #59). The rules come from the template itself.
const template = readFileSync(new URL('../.github/PULL_REQUEST_TEMPLATE.md', import.meta.url), 'utf8');
const valid = readFileSync(new URL('./fixtures/pr-body-valid.md', import.meta.url), 'utf8');
const check = (body) => checkPrBody(body, template);
const problems = (body) => check(body).join('\n');

describe('checkPrBody', () => {
  it('passes a description that follows the template', () => {
    expect(check(valid)).toEqual([]);
  });

  it('wants every section of the template, in its order, and no other', () => {
    expect(problems(valid.replace('## Architecture and data\n', '## Architecture\n'))).toMatch(/"Architecture and data" is missing/);
    const swapped = valid.replace('## Change type', '## TEMP').replace('## Linked issue', '## Change type').replace('## TEMP', '## Linked issue');
    expect(problems(swapped)).toMatch(/out of the template's order/);
    expect(problems(valid.replace('## Documentation', '## Security\n\nMore.\n\n## Documentation'))).toMatch(/"Security" isn't a section of the template/);
  });

  it('starts with "Closes #N." or "Part of #N.", and no other issue is closed', () => {
    expect(problems(valid.replace('Closes #41.', 'This closes #41.'))).toMatch(/first line of "Linked issue"/);
    expect(check(valid.replace('Closes #41.', 'Part of #41.'))).toEqual([]);
    expect(problems(valid.replace('Adaptive connections (#47).', 'Adaptive connections, which fixes #47.'))).toMatch(/closes #47/);
    expect(problems(valid.replace('Closes #41.', 'Part of #41.').replace('The probe\'s range size.', 'Whether this resolves #41.'))).toMatch(/closes #41, but the description says "Part of #41"/);
  });

  it('keeps every checklist item, in the template’s words', () => {
    expect(problems(valid.replace('- [ ] Packaging or release\n', ''))).toMatch(/"Change type" lacks the item "Packaging or release"/);
    expect(problems(valid.replace('- [x] Existing code and patterns reused;', '- [x] **Reuse:** existing code reused;'))).toMatch(/lacks the item "Existing code and patterns reused/);
    // More words after the item are fine.
    expect(check(valid.replace('(AGENTS.md, "Reuse before you add").', '(AGENTS.md, "Reuse before you add"). V1’s list.'))).toEqual([]);
  });

  it('ticks at least one change type', () => {
    expect(problems(valid.replace(/- \[x\] (Feature|Documentation|Test)/g, '- [ ] $1'))).toMatch(/no change type is ticked/);
  });

  it('says what isn’t included', () => {
    expect(problems(valid.replace('**Not included:**', '**Left out:**'))).toMatch(/"What changed" lacks "\*\*Not included:\*\*"/);
  });

  it('records the commands run, the level reached and each check’s result', () => {
    expect(problems(valid.replace('```text', '```'))).toMatch(/"Tests" lacks the commands actually run/);
    expect(problems(valid.replace('Verification level reached (docs/TESTING.md): L3', 'Verification level reached (docs/TESTING.md): high'))).toMatch(/verification level/);
    expect(problems(valid.replace('| Linux | PASS |', '| Linux | OK |'))).toMatch(/"Linux" has the result "OK"/);
    expect(problems(valid.replace('| Check or platform | Result | Notes |', '| Check | Result |'))).toMatch(/"Tests" lacks the table "\| Check or platform \| Result \| Notes \|"/);
  });

  it('gives each acceptance criterion a result and evidence', () => {
    expect(problems(valid.replace('| PASS | `a_200_to_a_later_range_is_aborted` |', '| PASS | |'))).toMatch(/"Acceptance criteria" has a row with an empty cell/);
    expect(problems(valid.replace(/\| A `200` to a later range[^\n]*\n/, ''))).toMatch(/"Acceptance criteria" has no row/);
  });

  it('says what happened to the README', () => {
    expect(problems(valid.replace('| `README.md` | Reviewed; no change |\n', ''))).toMatch(/README\.md/);
  });

  it('fills in everything the template leaves blank', () => {
    expect(problems(valid.replace('- The probe\'s range size.', '-'))).toMatch(/"Things to check during review" is empty/);
    expect(problems(valid.replace('## Things to check during review\n', '## Things to check during review\n\n<!-- Anything. -->\n'))).toMatch(/comment left from the template/);
    expect(problems(valid.replace('| macOS | NOT RUN |', '| macOS | PASS / FAIL / BLOCKED / NOT RUN |'))).toMatch(/"macOS" has the result/);
  });

  it('ends with the reviewer’s footer and the attribution', () => {
    expect(problems(valid.replace(/The reviewer checks this PR[^\n]*\n/, ''))).toMatch(/footer/);
    expect(problems(valid.replace('🤖 Generated with [Claude Code](https://claude.com/claude-code)', ''))).toMatch(/attribution/);
  });
});

describe('checkPrTitle', () => {
  it('is a Conventional Commit that ends with the issue', () => {
    expect(checkPrTitle('feat(engine): add bounded range probe (#41)', valid)).toEqual([]);
    expect(checkPrTitle('fix: keep line endings stable on Windows (#12)', valid.replace('Closes #41.', 'Part of #12.'))).toEqual([]);
  });

  it('refuses another type, a missing issue, a closing word, or another issue', () => {
    expect(checkPrTitle('Feature: add bounded range probe (#41)', valid).join()).toMatch(/Conventional Commit/);
    expect(checkPrTitle('feat(engine): add bounded range probe', valid).join()).toMatch(/end with "\(#41\)"/);
    expect(checkPrTitle('feat(engine): add bounded range probe (closes #41)', valid).join()).toMatch(/end with "\(#41\)"/);
    expect(checkPrTitle('feat(engine): add bounded range probe (#42)', valid).join()).toMatch(/end with "\(#41\)"/);
  });
});

describe('prBodyArgs', () => {
  it('takes the file with or without --title, in either order', () => {
    expect(prBodyArgs(['pr.md'])).toEqual({ file: 'pr.md', title: undefined });
    expect(prBodyArgs(['pr.md', '--title', 'fix: x (#1)'])).toEqual({ file: 'pr.md', title: 'fix: x (#1)' });
    expect(prBodyArgs(['--title', 'fix: x (#1)', 'pr.md'])).toEqual({ file: 'pr.md', title: 'fix: x (#1)' });
  });

  it('refuses a missing file, or --title without a value', () => {
    expect(prBodyArgs([])).toBeNull();
    expect(prBodyArgs(['--title', 'fix: x (#1)'])).toBeNull();
    expect(prBodyArgs(['pr.md', '--title'])).toBeNull();
  });
});
