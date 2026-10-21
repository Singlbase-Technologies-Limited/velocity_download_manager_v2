# CI diagnostics

Every CI run reports **all** of its failures at once, in one table and one machine-readable file, so a fix can address everything in one round instead of failing one check at a time (#91). This page explains how to read that report. The CI design itself is in [CI.md](CI.md).

## What every run produces

| Where | What |
|---|---|
| Run summary (the run's page on GitHub) | A **Job selection** table: each job runs or is skipped, with the reason. A **CI diagnostics** table: one row per check, with its status and its first errors, each with `file:line` |
| `ci-diagnostics` artifact | `ci-diagnostics.json`: the same rows, with up to 10 errors per check |
| `report-<job>` artifacts | Each job's own report, which the summary combines (pull requests only) |
| The `ci-required` check (PRs) or the `linux` job (`main`) | Failed unless every mandatory check passed or was deselected by the selector |

Artifacts are kept 7 days for pull requests and 14 days for `main`.

## Statuses

| Status | Meaning | What to do |
|---|---|---|
| **PASSED** | The check ran and passed | — |
| **FAILED** | The check ran and found a defect | Fix the code |
| **INFRA** | A setup step failed (checkout, Node, toolchain, cache, `pnpm install`), or a job ended before writing its report | Read the job log. A runner or network fault is a re-run candidate. A broken pin or lockfile is a defect |
| **BLOCKED** | Didn't run because its prerequisite failed (named in the row) | Fix the prerequisite |
| **SKIPPED-BY-SELECTOR** | Not needed for this change, with the reason (CI.md, "Selecting jobs") | — |
| **CANCELLED** | A newer push superseded the run, or someone cancelled it | Look at the newer run |

The run passes only when every mandatory check is PASSED or SKIPPED-BY-SELECTOR. A FAILED, INFRA, BLOCKED or CANCELLED mandatory check always fails it.

## How a run collects every failure

- **Within a job,** the check runner runs every check even after one fails (`check:fast`, `check:pr`). Clippy uses `--keep-going`, so a crate that doesn't compile doesn't hide the others. `cargo test` uses `--no-fail-fast`, so every test binary runs. Vitest runs every test file.
- **Between jobs,** report and summary steps run with `if: ${{ !cancelled() }}`, never `always()`, which would also run in cancelled runs. Mandatory checks never use `continue-on-error`, which would turn a failure into a success. The one exception is downloading the reports: a missing report is itself classified as INFRA.
- **Errors are extracted** by `scripts/diagnostics-lib.mjs`, with unit tests built from real tool output. It reads rustfmt diffs, rustc and clippy errors with `-->` locations, failed Rust tests with their panic location, `tsc`, ESLint, Vitest and `check:links`. When it recognises nothing, it keeps the last 10 lines of output.
- **Packaging and release jobs** (#17, #18) keep strict `needs` semantics: they never run after a failed mandatory check.

## Reading a failed run

```sh
GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh run download <run id> -n ci-diagnostics -D <scratch dir>
GH_TOKEN="$(gh auth token -u velocity-claude-agent)" gh run view <run id> --log-failed   # full logs of the failed steps
```

In Claude Code, `/ci-triage <run id or PR>` does this and proposes one fix plan for every failure (`.claude/skills/ci-triage/`).

## Re-run policy

- **FAILED:** fix the code. Don't re-run hoping it passes; a flaky test is a defect too.
- **INFRA:** one re-run (`gh run rerun <id> --failed`) is reasonable for a runner or network fault. If the same step fails again, treat it as a defect.
- **Never** make a check optional, skip it, or relax it to get a green run (AGENTS.md).

## `ci-diagnostics.json`

```json
{
  "schema": 1,
  "run": { "id": "…", "url": "…", "sha": "…", "event": "pull_request", "workflow": "PR checks" },
  "ok": false,
  "rows": [
    {
      "check": "Rust unit tests (cargo test)",
      "job": "code",
      "status": "FAILED",
      "errors": [
        { "file": "src-tauri/src/lib.rs", "line": 63, "col": 9, "test": "tests::name", "message": "…" }
      ]
    },
    { "check": "Code checks", "job": "code", "status": "INFRA", "reason": "setup step \"setup\" failed before any check ran; a re-run candidate", "errors": [] }
  ]
}
```

`reason` is present on rows classified from a job's result rather than from its checks. Messages are the tools' own output, which contains no secrets: PR jobs have none (CI.md).
