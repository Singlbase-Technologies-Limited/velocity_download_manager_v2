#!/usr/bin/env node
// The broken-main tracking issue (#16, docs/CI.md "When main breaks"):
//
//   node scripts/ci-main-status.mjs [--report ci-diagnostics.json] [--title "main is failing"]
//
// Run as the last step of a `main` run, with GH_TOKEN allowed to write issues. A red run opens the
// issue (or comments on it when it's already open); a green run closes it. The decision is in
// main-status-lib.mjs (unit tested); this file reads the report and calls `gh` with argument lists.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { TRACKING_TITLE, trackingAction } from './main-status-lib.mjs';

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] !== undefined ? args[i + 1] : fallback;
};
const reportPath = opt('report', 'ci-diagnostics.json');
const title = opt('title', TRACKING_TITLE);
const repo = process.env.GITHUB_REPOSITORY;

function gh(ghArgs) {
  const r = spawnSync('gh', ghArgs, { encoding: 'utf8' });
  if (r.status !== 0) throw new Error(`gh ${ghArgs.slice(0, 2).join(' ')} failed: ${r.stderr || r.error?.message}`);
  return r.stdout;
}

try {
  if (!repo) throw new Error('GITHUB_REPOSITORY is not set; run this in GitHub Actions');
  let diagnostics = null;
  try {
    diagnostics = existsSync(reportPath) ? JSON.parse(readFileSync(reportPath, 'utf8')) : null;
  } catch {
    diagnostics = null;
  }
  const open = JSON.parse(gh(['issue', 'list', '--repo', repo, '--state', 'open', '--search', `in:title "${title}"`, '--json', 'number,title']));
  const openIssue = open.find((i) => i.title === title)?.number ?? null;
  const run = {
    url: `${process.env.GITHUB_SERVER_URL}/${repo}/actions/runs/${process.env.GITHUB_RUN_ID}`,
    sha: process.env.GITHUB_SHA ?? 'unknown',
    ref: process.env.GITHUB_REF ?? '',
  };
  const action = trackingAction({ diagnostics, openIssue, run, title });
  if (action.action === 'open') console.log(gh(['issue', 'create', '--repo', repo, '--title', action.title, '--body', action.body]).trim());
  else if (action.action === 'comment') console.log(gh(['issue', 'comment', String(action.issue), '--repo', repo, '--body', action.body]).trim());
  else if (action.action === 'close') console.log(gh(['issue', 'close', String(action.issue), '--repo', repo, '--comment', action.body]).trim());
  console.log(`tracking issue "${title}": ${action.action}${action.issue ? ` #${action.issue}` : ''}`);
} catch (e) {
  // Fail the step: a tracking issue that silently didn't open would hide a red main.
  console.error(`ci-main-status failed: ${e instanceof Error ? e.message : e}`);
  process.exit(1);
}
