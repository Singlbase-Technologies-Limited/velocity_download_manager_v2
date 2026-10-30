#!/usr/bin/env node
// Checks a pull request description, and its title, against .github/PULL_REQUEST_TEMPLATE.md (PO decision on #59).
// Usage: pnpm check:pr-body <description file> --title "<title>"
// The agent guard runs the same check before `gh pr create` and `gh pr edit`.
import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { checkPrBody, checkPrTitle, prBodyArgs } from './pr-body-lib.mjs';

const parsed = prBodyArgs(process.argv.slice(2));
if (parsed === null) {
  console.error('Usage: pnpm check:pr-body <description file> --title "<title>"');
  process.exit(2);
}
const { file, title } = parsed;

const root = resolve(import.meta.dirname, '..');
const template = readFileSync(join(root, '.github', 'PULL_REQUEST_TEMPLATE.md'), 'utf8');
const body = readFileSync(resolve(file), 'utf8');
const problems = [...checkPrBody(body, template), ...(title === undefined ? [] : checkPrTitle(title, body))];
if (problems.length > 0) {
  console.error(`check:pr-body: ${String(problems.length)} problem(s) in ${file}:`);
  for (const p of problems) console.error(`- ${p}`);
  process.exit(1);
}
console.log(`check:pr-body: ${file} follows .github/PULL_REQUEST_TEMPLATE.md${title === undefined ? ' (title not checked: pass --title)' : ', and the title is valid'}.`);
