---
name: ci-triage
description: Diagnose a failed V2 CI run from its ci-diagnostics.json report. Downloads the report, groups related failures by cause, separates INFRA (re-run candidates) from code defects, and proposes one fix plan covering every failure at once. Read-only; use when a PR or main run fails, or when asked why CI is red.
argument-hint: "[run id, PR number or branch; default: the current branch's latest run]"
---

# CI triage: $ARGUMENTS

Read-only: download and read, never push, re-run or edit in this step. How the report is built is in [docs/CI_DIAGNOSTICS.md](../../../docs/CI_DIAGNOSTICS.md). Run `gh` as `GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh …`.

## 1. Find the run

- **A run id:** use it.
- **A PR number:** `gh pr checks <N>`, then `gh run list --branch <head> --limit 1`.
- **Nothing given:** the latest run of the current branch, `gh run list --branch "$(git branch --show-current)" --limit 1`.
- **Still in progress:** `gh run view <id>` shows it. Wait, rather than triaging a partial run.

## 2. Read the report

```sh
gh run download <id> -n ci-diagnostics -D "$SCRATCH/ci-<id>"   # writes ci-diagnostics.json
```

Use the session's scratch directory, never the repository. `rows` lists every check, each with a `status` and up to 10 `errors` (`file`, `line`, `col`, `test`, `message`).

**If there's no `ci-diagnostics` artifact** (the summary job itself didn't run), fall back to `gh run view <id> --log-failed`.

## 3. Classify

| Status | Meaning | Action |
|---|---|---|
| FAILED | A check found a defect in the code | Fix it |
| INFRA | A setup step failed (checkout, toolchain, cache, install), or a job died before reporting | Read `gh run view <id> --log-failed` for that job. A network or runner fault is a re-run candidate: propose a re-run, don't do it. A broken toolchain pin or lockfile is a defect |
| BLOCKED | Didn't run because a prerequisite failed | Fix the prerequisite; no separate action |
| CANCELLED | A newer push superseded the run, or someone stopped it | Usually nothing; check the newer run |
| SKIPPED-BY-SELECTOR | Not needed for the change | Nothing |
| PASSED | — | Nothing |

## 4. Group by cause

Group the errors that share a root cause before planning any fix:

- **Same file and nearby lines:** usually one edit.
- **Compile errors before test failures:** a crate that doesn't compile can make its tests and clippy fail too. Fix the compile error first.
- **Same message across checks** (for example a type change that breaks `tsc` and Vitest): one cause.
- **Formatting:** `cargo fmt --all` fixes every rustfmt row at once.

## 5. Propose one plan

Report this to the user:

1. **The run:** its link, the failing checks, and the number of errors per check.
2. **The causes:** each group with its files and lines, and which rows it explains.
3. **One fix plan for all of them,** in order: compile errors, then lint and format, then tests. Name the focused command that proves each fix (`pnpm test:focused -- <area>`, `pnpm check:pr`).
4. **INFRA rows:** what failed, and whether a re-run is reasonable.

Never weaken a test or a check to make CI green (AGENTS.md).
