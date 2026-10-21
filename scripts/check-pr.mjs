#!/usr/bin/env node
// `pnpm check:pr [--base <ref>]`: verification level L3 run locally. Runs the L1 checks that the
// branch's changes need, judged from every path changed since the merge base with the base ref
// (default origin/main): committed, staged, unstaged and untracked. Shared or unknown paths select
// everything. See docs/TESTING.md.
import { spawnSync } from 'node:child_process';
import { checkCommands, selectChecks } from './checks-lib.mjs';
import { loadWorkspace, requirePrerequisites, root, runAndExit } from './checks-runner.mjs';

const git = (...args) => {
  const r = spawnSync('git', args, { cwd: root, encoding: 'utf8', windowsHide: true });
  if (r.status !== 0) {
    console.error(`git ${args.join(' ')} failed:\n${r.stderr}`);
    process.exit(2);
  }
  return r.stdout;
};
const nulList = (out) => out.split('\0').filter(Boolean);

const argv = process.argv.slice(2).filter((a) => a !== '--');
const base = argv.length === 0 ? 'origin/main' : argv[0] === '--base' && argv.length === 2 ? argv[1] : undefined;
if (!base) {
  console.error('Usage: pnpm check:pr [--base <ref>]');
  process.exit(2);
}

requirePrerequisites();
const mergeBase = git('merge-base', 'HEAD', base).trim();
const changed = [
  ...new Set([
    ...nulList(git('diff', '--name-only', '--no-renames', '-z', mergeBase)),
    ...nulList(git('ls-files', '--others', '--exclude-standard', '-z')),
  ]),
];

const selection = selectChecks(changed, loadWorkspace());
console.log(`check:pr: ${changed.length} changed path(s) since ${base} (${mergeBase.slice(0, 7)})`);
for (const r of selection.reasons) console.log(`  ${r}`);
const commands = checkCommands(selection);
if (commands.length === 0) {
  console.log('\nNo code checks are needed for these changes.');
  process.exit(0);
}
runAndExit('check:pr', commands);
