#!/usr/bin/env node
// CI job selection (#13, docs/CI.md): `node scripts/ci-select.mjs --base <ref>`.
// Decides which pull-request jobs run, with the same rules as `pnpm check:pr`
// (scripts/checks-lib.mjs). In GitHub Actions it writes the decision to $GITHUB_OUTPUT
// (rust, frontend, docs, native, windows: "true"/"false"; rust_packages: space-separated) and a table of every
// job, run or skipped, with its reason to $GITHUB_STEP_SUMMARY. Elsewhere it prints both.
// It needs git history back to the merge base (actions/checkout with fetch-depth: 0) and cargo.
import { appendFileSync } from 'node:fs';
import { jobSummaryMarkdown, selectJobs } from './checks-lib.mjs';
import { changedPathsSince, loadWorkspace } from './checks-runner.mjs';

const argv = process.argv.slice(2).filter((a) => a !== '--');
const base = argv[0] === '--base' && argv.length === 2 ? argv[1] : undefined;
// A manual run (workflow_dispatch) has no base to compare with: it checks everything.
const all = argv[0] === '--all' && argv.length === 1;
if (!base && !all) {
  console.error('Usage: node scripts/ci-select.mjs --base <ref> | --all');
  process.exit(2);
}

let changed;
let workspace;
try {
  // An unknown path selects everything (checks-lib.mjs), so --all goes through the same rule.
  changed = all ? { paths: ['(manual run, everything)'], mergeBase: '' } : changedPathsSince(base);
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
  native: String(selection.jobs.native.run),
  windows: String(selection.jobs.windows.run),
  rust_packages: selection.rustPackages.join(' '),
};
const summary = `${jobSummaryMarkdown(selection)}\n${all ? 'Manual run: every job selected.' : `Base: \`${base}\`, merge base \`${changed.mergeBase.slice(0, 12)}\`.`}\n`;

if (process.env.GITHUB_OUTPUT) {
  appendFileSync(process.env.GITHUB_OUTPUT, Object.entries(outputs).map(([k, v]) => `${k}=${v}\n`).join(''));
}
if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
console.log(summary);
console.log(JSON.stringify(outputs));
