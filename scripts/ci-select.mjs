#!/usr/bin/env node
// CI job selection (#13, docs/CI.md): `node scripts/ci-select.mjs --base <ref>`.
// Decides which pull-request jobs run, with the same rules as `pnpm check:pr`
// (scripts/checks-lib.mjs). In GitHub Actions it writes the decision to $GITHUB_OUTPUT
// (rust, frontend, docs: "true"/"false"; rust_packages: space-separated) and a table of every
// job, run or skipped, with its reason to $GITHUB_STEP_SUMMARY. Elsewhere it prints both.
// It needs git history back to the merge base (actions/checkout with fetch-depth: 0) and cargo.
import { appendFileSync } from 'node:fs';
import { jobSummaryMarkdown, selectJobs } from './checks-lib.mjs';
import { changedPathsSince, loadWorkspace } from './checks-runner.mjs';

const argv = process.argv.slice(2).filter((a) => a !== '--');
const base = argv[0] === '--base' && argv.length === 2 ? argv[1] : undefined;
if (!base) {
  console.error('Usage: node scripts/ci-select.mjs --base <ref>');
  process.exit(2);
}

let changed;
let workspace;
try {
  changed = changedPathsSince(base);
  workspace = loadWorkspace();
} catch (e) {
  // Never guess on an error: a failed selection fails the run, which `ci-required` reports.
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}

const selection = selectJobs(changed.paths, workspace);
const outputs = {
  rust: String(selection.jobs.rust.run),
  frontend: String(selection.jobs.frontend.run),
  docs: String(selection.jobs.docs.run),
  rust_packages: selection.rustPackages.join(' '),
};
const summary = `${jobSummaryMarkdown(selection)}\nBase: \`${base}\`, merge base \`${changed.mergeBase.slice(0, 12)}\`.\n`;

if (process.env.GITHUB_OUTPUT) {
  appendFileSync(process.env.GITHUB_OUTPUT, Object.entries(outputs).map(([k, v]) => `${k}=${v}\n`).join(''));
}
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
console.log(summary);
console.log(JSON.stringify(outputs));
