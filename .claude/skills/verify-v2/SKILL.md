---
name: verify-v2
description: Deliberate verification of the current branch at the level the change needs (L1 check:fast, L2 test:focused, L3 check:pr, L4 check:full), reported as PASS, FAIL, BLOCKED or NOT RUN per check and platform. Manual; run before a PR or when asked to verify.
disable-model-invocation: true
argument-hint: "[full]"
---

# Verify this branch

The levels and their scope are in [docs/TESTING.md](../../../docs/TESTING.md). Save logs to the session's scratch directory, never to the repository.

## 1. See what changed

- `git status` and `git diff --stat origin/main...HEAD`. Uncommitted work counts too.
- The issue's *Testing* section names the level the change needs.

## 2. Run the levels

Locally, only the fast tests (PO decision, 2026-10-31); the heavy levels run in the pull request's CI.

1. **`pnpm test:focused -- <area>`** for every area with new behaviour, to show those tests ran by name. A single test file or test when that is enough.
2. **Formatting and lint** for what changed: `cargo fmt --check`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `tsc --noEmit`, ESLint on the changed files.
3. **In CI, not here:** `check:pr`'s selection and `check:fast` run on the pull request (PR checks), and the Windows checks after merge (Main checks). Report them from that run. `pnpm check:full`, the end-to-end runs and packaging run only when the PO asks, or in their workflows.

## 3. Report

| Check | Command | Platform | Result |
|---|---|---|---|
| … | the exact command | Windows / Linux (WSL) / macOS | PASS, FAIL, BLOCKED or NOT RUN, with the key output line |

- **Only the checks that actually ran.** Anything not run is NOT RUN, with the reason.
- **A FAIL shows its actual error output,** and the work stops there (CLAUDE.md, step 4).
- **Platforms:** a Windows pass says nothing about Linux or macOS. Use `/platform-verify` when the change can behave differently per platform.
- **CI:** report CI checks only after seeing their results. An unselected job is NOT RUN, not passed. For a failed run, use `/ci-triage`.
