---
name: verify-v2
description: Deliberate verification of the current branch at the level the change needs (L1 check:fast, L2 test:focused, L3 check:pr, L4 check:full), reported as PASS, FAIL, BLOCKED or NOT RUN per check and platform. Manual; run before a PR or when asked to verify.
disable-model-invocation: true
argument-hint: "[full]"
---

# Verify this branch

> **Temporarily** (#204), verification is the batch CI run, not a local one: see [CI.md](../../../docs/CI.md#while-ci-runs-by-hand-only). This applies to `/tdd-change` and `/platform-verify` too.

The levels and their scope are in [docs/TESTING.md](../../../docs/TESTING.md). Save logs to the session's scratch directory, never to the repository.

## 1. See what changed

- `git status` and `git diff --stat origin/main...HEAD`. Uncommitted work counts too.
- The issue's *Testing* section names the level the change needs.

## 2. Run the levels

1. **`pnpm check:pr`.** It prints which paths selected which checks, then runs them. Lockfiles, shared configuration, `scripts/`, `.claude/` and unknown paths select everything.
2. **`pnpm test:focused -- <area>`** for every area with new behaviour, to show those tests ran by name.
3. **`pnpm check:full`,** when `$ARGUMENTS` is `full` or the change touches build settings, the release profile or packaging. It includes the release build, which takes about 4 minutes on Windows.
4. **`pnpm check:fast`** before every PR. When `check:pr` selected everything, its run counts.

## 3. Report

| Check | Command | Platform | Result |
|---|---|---|---|
| … | the exact command | Windows / Linux (WSL) / macOS | PASS, FAIL, BLOCKED or NOT RUN, with the key output line |

- **Only the checks that actually ran.** Anything not run is NOT RUN, with the reason.
- **A FAIL shows its actual error output,** and the work stops there (CLAUDE.md, step 4).
- **Platforms:** a Windows pass says nothing about Linux or macOS. Use `/platform-verify` when the change can behave differently per platform.
- **CI:** report CI checks only after seeing their results. An unselected job is NOT RUN, not passed. For a failed run, use `/ci-triage`.
