#!/usr/bin/env node
// `pnpm check:fast`: verification level L1 (formatting, lint, type checks and unit tests) for the
// whole workspace. See docs/TESTING.md.
import { checkCommands } from './checks-lib.mjs';
import { requirePrerequisites, runAndExit } from './checks-runner.mjs';

requirePrerequisites();
runAndExit('check:fast', checkCommands({ all: true, frontend: true, rustPackages: [] }));
