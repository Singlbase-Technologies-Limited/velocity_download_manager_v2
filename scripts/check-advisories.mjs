#!/usr/bin/env node
// `pnpm check:advisories` (#75): known security advisories in the Rust and npm dependencies as locked,
// with cargo-deny (installed by hand: `cargo install cargo-deny --locked --version 0.20.2`) and pnpm
// audit. Run by .github/workflows/advisories.yml (by hand for now, weekly again later; docs/CI.md), and by hand. The process is in
// docs/SECURITY.md#dependency-advisories-75.
import { ADVISORY_COMMANDS } from './checks-lib.mjs';
import { runAndExit } from './checks-runner.mjs';

runAndExit('check:advisories', ADVISORY_COMMANDS);
