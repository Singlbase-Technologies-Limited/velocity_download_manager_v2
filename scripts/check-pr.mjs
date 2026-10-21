#!/usr/bin/env node
// `pnpm check:pr [--base <ref>]`: verification level L3 run locally. Runs the L1 checks that the
// branch's changes need, judged from every path changed since the merge base with the base ref
// (default origin/main): committed, staged, unstaged and untracked. Shared or unknown paths select
// everything. CI makes the same decision with scripts/ci-select.mjs. See docs/TESTING.md.
import { checkCommands, selectChecks } from './checks-lib.mjs';
import { changedPathsSince, loadWorkspace, requirePrerequisites, runAndExit } from './checks-runner.mjs';

const argv = process.argv.slice(2).filter((a) => a !== '--');
const base = argv.length === 0 ? 'origin/main' : argv[0] === '--base' && argv.length === 2 ? argv[1] : undefined;
if (!base) {
  console.error('Usage: pnpm check:pr [--base <ref>]');
  process.exit(2);
}

requirePrerequisites();
let changed;
try {
  changed = changedPathsSince(base);
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(2);
}

const selection = selectChecks(changed.paths, loadWorkspace());
console.log(`check:pr: ${changed.paths.length} changed path(s) since ${base} (${changed.mergeBase.slice(0, 7)})`);
for (const r of selection.reasons) console.log(`  ${r}`);
const commands = checkCommands(selection);
if (commands.length === 0) {
  console.log('\nNo code checks are needed for these changes.');
  process.exit(0);
}
runAndExit('check:pr', commands);
