#!/usr/bin/env node
// `pnpm check:full`: everything runnable locally. Every L1 check, then the frontend production
// build and the Rust release build (release profile: LTO, one codegen unit). See docs/TESTING.md.
import { FULL_EXTRA_COMMANDS, checkCommands } from './checks-lib.mjs';
import { requirePrerequisites, runAndExit } from './checks-runner.mjs';

requirePrerequisites();
runAndExit('check:full', [...checkCommands({ all: true, frontend: true, rustPackages: [] }), ...FULL_EXTRA_COMMANDS]);
