#!/usr/bin/env node
// `pnpm test:focused -- <area> [extra args]`: verification level L2, the tests of one area while
// you iterate. Areas are the workspace crates, `frontend` and `scripts`. See docs/TESTING.md.
//   pnpm test:focused -- velocity-core            cargo test -p velocity-core
//   pnpm test:focused -- velocity-core range      cargo test -p velocity-core range
//   pnpm test:focused -- scripts -t selectChecks  vitest run scripts -t selectChecks
import { focusedCommands } from './checks-lib.mjs';
import { loadWorkspace, requirePrerequisites, runAndExit } from './checks-runner.mjs';

requirePrerequisites();
let commands;
try {
  commands = focusedCommands(process.argv.slice(2), loadWorkspace());
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(2);
}
runAndExit('test:focused', commands);
