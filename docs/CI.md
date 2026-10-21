# CI design

V2's CI runs on GitHub Actions in a private repository on GitHub Free. The plan includes 2,000 runner minutes a month, and usage is **blocked**, not billed, once they run out ([billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)). Every design choice below protects two things: the checks that matter run on every change, and the monthly quota lasts the month.

This page is the design. The workflows are written in #13, #14, #16 to #19 and #91. Every figure below is labelled as **measured** or **assumed**. Assumed CI times get replaced by measured ones as the jobs start running (#14).

## Tiers

| # | Event | What runs | Platforms | Purpose | Built in |
|---|---|---|---|---|---|
| 1 | `pull_request` into `main` (opened, synchronised, reopened) | L1 for the selected areas, plus L2 for those areas. Docs-only changes run only the docs job | Linux | Fast feedback before review. One aggregate check, `ci-required`, summarises it | #13, #14, #91 |
| 2 | `push` to `main` (that is, after a squash merge) | Broader suites for the merged change's areas. **Windows** jobs when Windows-relevant paths changed (below) | Linux, Windows | Catch what the PR tier deliberately left out, before a release candidate is built from `main` | #16 |
| 3 | `workflow_dispatch` on a commit, or a `v*-rc.*` tag | Everything runnable (`check:full` and the L2 suites) plus installers | Linux, Windows; macOS once #83 establishes it | A downloadable, labelled release-candidate build from a verified `main` commit | #17 |
| 4 | `v*` tag (not `-rc`) | The release gate: it rechecks that the tag's commit passed tier 3, then builds artefacts with provenance into a **draft** release | Linux, Windows | Prepare a release the PO publishes by hand | #18 |
| 5 | `schedule` (weekly) and `workflow_dispatch` | L4: everything on Linux and Windows. Later also fuzzing, stress and extended benchmarks. macOS on demand only | Linux, Windows; macOS manual | Catch slow drift and run what's too expensive per change | #19 |

**No `develop` or `staging` tiers.** All work merges into `main`, and releases are tags on verified `main` commits ([DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md)).

**There is no server-side gate on GitHub Free for a private repository.** Branch protection, required status checks and rulesets return HTTP 403 (verified 2026-10-21, #89). Environment *required reviewers* are only available for public repositories on Free ([environments](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)). So:

- **"Required" means by convention:** the PO merges only when `ci-required` is green (docs/REVIEWING.md).
- **The release gate is a person:** only the PO pushes `v*` tags or starts a release run (an ask rule in `.claude/settings.json` stops the agent from doing it unprompted). The workflow creates a *draft* release, never a published one.
- **Revisit when the repository goes public or the plan changes** (#7, #90): rulesets, required checks and environment reviewers become available.

## Selecting jobs

Selection extends the rules `pnpm check:pr` already uses, in [`scripts/checks-lib.mjs`](../scripts/checks-lib.mjs). The same code decides locally and in CI, so the two can't drift apart (#13).

- **Fail-safe:** these select everything:
  - lockfiles (`Cargo.lock`, `pnpm-lock.yaml`);
  - **native lock files anywhere**, even inside a crate (`*.lock`, `vcpkg.json`);
  - shared configuration (`Cargo.toml`, `package.json`, `rust-toolchain.toml`…);
  - the check scripts (`scripts/`), `.github/` and `.claude/`;
  - **any path no rule knows**, such as a `package-lock.json`, which V2 doesn't use.
- **Crates:** a change inside a crate selects that crate and every crate that depends on it.
- **Frontend:** frontend paths select `tsc`, ESLint and Vitest.
- **Docs only:** `docs/` and root `*.md` select only the docs job (`pnpm check:links`: relative links and heading anchors).
- **Windows-relevant paths (tier 2):**
  - every Rust crate, `src-tauri/`, `Cargo.lock`, `rust-toolchain.toml`;
  - `scripts/`, since the check runner has Windows-specific code paths;
  - packaging configuration;
  - and, as always, unknown paths.

  Frontend-only and docs-only merges skip Windows, because a WebView difference is caught by the release-candidate tier instead.
- **Every selection prints its reasons** in the job log, as `check:pr` does locally. A job that wasn't selected is reported as **NOT RUN**, never as passed.

Saving minutes never removes a relevant correctness or security test. When in doubt, the selector selects more.

### How the PR tier applies it

The PR tier is implemented in [`.github/workflows/pr.yml`](../.github/workflows/pr.yml) (#13):

| Job | Runs when | What it does |
|---|---|---|
| `select` | Always | `node scripts/ci-select.mjs --base <PR base>` writes `rust`, `frontend` and `docs` flags to its outputs. It adds a table to the **run summary** with every job, *runs* or *skipped*, the reason for each, and the reason for each changed path. When docs are selected, the same job runs `node scripts/check-links.mjs` as a step |
| `code` | Rust or frontend selected | `pnpm check:pr`, the same command as locally. Tauri system packages and the pinned toolchain are installed only when Rust is selected |
| `ci-required` | Always (`if: always()`) | Passes only if `select` succeeded and `code` succeeded or was skipped by the selection. It writes its own table to the run summary |

**Why so few jobs.** GitHub bills every job's time **rounded up to a whole minute** ([runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing)), and every job repeats checkout and setup. So Rust and frontend share the `code` job, and the one-second docs check runs inside `select`. A docs-only PR then costs 2 billed minutes (`select` and `ci-required`), and a code PR costs the `code` job plus 2.

**Skipped jobs and required checks.** A job skipped by an `if:` condition reports *Success*, while a workflow skipped by path filters leaves a required check *Pending* forever ([troubleshooting required checks](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/collaborating-on-repositories-with-code-quality-features/troubleshooting-required-status-checks)). So the workflow never uses `on.paths`, and `ci-required` always reports. That keeps it usable as a required check if branch protection becomes available (#7).

**Selection errors fail the run.** If `ci-select.mjs` can't compute the change (for example, missing history), it exits non-zero rather than guessing, and `ci-required` fails.

## Caching

The repository gets **10 GB of Actions cache**, separate from artifact storage. GitHub evicts the least recently used entries above that ([billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)).

| Cache | Key | Contents | Size | Reasoning |
|---|---|---|---|---|
| Rust (Linux) | OS + toolchain + `Cargo.lock` hash + job kind | Compiled **dependencies** only (`~/.cargo/registry`, `target/` without workspace crates) | **Measured:** 1.1 GiB raw, 365 MiB gzip-compressed, plus a 201 MiB registry | A cold build is most of a cold job (**measured:** 571 s for the clippy and test builds on 2 vCPUs). Workspace crates rebuild in seconds, so caching them would only add upload time and size |
| Rust (Windows) | Same | Same | **Assumed:** similar to Linux (not measured) | Windows minutes cost more ($0.010 against $0.006 a minute), so a cache hit saves more |
| pnpm store | OS + `pnpm-lock.yaml` hash | The pnpm content store | **Measured:** 147 MiB (Linux) | `pnpm install --frozen-lockfile` then only links |

**Caching rules:**

- **Budget:** about 0.6 GB a platform when compressed, so Linux and Windows, with one previous key each during a lockfile change, use about 2.5 GB of the 10 GB.
- **Save on `main` only;** pull requests restore. A PR branch's cache is visible only to that branch, so saving it there would fill the 10 GB with copies.
- **Keys change only when the toolchain or lockfile changes,** so a cache uploads rarely.
- **No cache for Tauri system packages** (`apt`): restoring them costs about as much as installing them. To be measured in #14.

The choice of cache action (GitHub's own `actions/cache`, or a Rust-specific one) is made in #14. Any third-party action is a dependency that needs PO approval (AGENTS.md).

## Cancelling and de-duplicating

- **Superseded PR runs are cancelled.** Each workflow sets `concurrency: { group: <workflow>-<PR or ref>, cancel-in-progress: true }` for pull requests ([concurrency](https://docs.github.com/en/actions/writing-workflows/choosing-what-your-workflow-does/control-the-concurrency-of-workflows-and-jobs)). Runs on `main` and on tags are never cancelled: each commit's result matters.
- **No duplicate runs for one commit.** Pull-request checks trigger on `pull_request` only, and `push` triggers only for `main` and tags. A branch push therefore doesn't run CI a second time beside its PR.
- **Path filters live in the selector,** not in `on.paths`. Then the aggregate `ci-required` check always reports, instead of being missing when a workflow is skipped.

## Permissions and supply chain

GitHub's guidance is in [secure use](https://docs.github.com/en/actions/reference/security/secure-use):

- **Read-only by default.** Every workflow declares `permissions: contents: read`, and a job raises only what it needs (for example `contents: write` for the draft-release job in #18).
- **Pinned actions.** Third-party actions are pinned to a **full commit SHA**, with the version in a comment. GitHub's own `actions/*` are pinned the same way, for consistency.
- **Dependabot** updates the pins: a `github-actions` entry in `.github/dependabot.yml`, added in #14. Each update is an ordinary PR.
- **No `pull_request_target`** that checks out PR code. No secrets in PR workflows.
- **Untrusted text stays out of commands.** Titles, branch names and other user-controlled strings never go straight into `run:` scripts; they pass through environment variables.
- **Self-hosted runners:** none. If one is ever added (PO decision), it never runs code from a fork.

## Collecting failures

- **Independent checks run to completion.** A failing check doesn't stop the others in the same run (`continue-on-error` per step, as `check:fast` does locally). Cross-platform matrices use `fail-fast: false`.
- **Infrastructure problems are reported as BLOCKED,** separately from test failures: a runner lost, a download failed, the quota was exhausted.
- **One consolidated report.** `ci-required` publishes `ci-diagnostics.json` and a summary: what ran, what failed and why, and what wasn't selected. Built in #91; the `/ci-triage` skill reads it.

## L4 schedule

| Run | When | Content | Platforms |
|---|---|---|---|
| Weekly full | Mondays 03:00 UTC, only if `main` changed since the last run | `check:full`, all L2 suites | Linux, Windows |
| Fuzzing, stress, extended benchmarks | Weekly, staggered, once the suites exist (#73, #77, #20) | Time-boxed per target, with a fixed seed recorded | Linux |
| macOS | `workflow_dispatch` only | `check:full` | macOS (#83) |

macOS minutes cost about 10 times Linux ($0.062 against $0.006 a minute), so macOS never runs on a schedule or per change until it's an established platform and the quota allows it.

## Costs

**Measured locally** (docs/DEVELOPMENT.md, 2026-10-21). A cold dependency build takes 278 s on Windows (16 logical CPUs) and 413 s on Linux (WSL2, 2 vCPUs, similar to a 2-vCPU hosted runner). A warm `check:fast` takes 5.5–9 s. A full `check:full` takes 224–249 s, of which 215–232 s is the release build.

**Measured in CI.** The first run of the PR tier was on PR #104 ([run 39657529808](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39657529808), 2026-10-21, `ubuntu-latest`, no cache yet, everything selected).

| Job | Duration | Billed |
|---|---|---|
| `select` | 14 s | 1 min |
| `code`, cold: setup 57 s (Node 7 s, Tauri packages 35 s, Rust toolchain 9 s, `pnpm install` 4 s), then `check:pr` 7 min 36 s (clippy 302 s, cargo test 146 s) | 8 min 35 s | 9 min |
| `docs` (then a separate job, now a step in `select`) | 31 s | 1 min |
| `ci-required` | 4 s | 1 min |

GitHub bills every job **rounded up to a whole minute** ([runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing)). So each run carries 2 billed minutes for `select` and `ci-required`, on top of the `code` job.

**Still assumed, until #14 adds caching and measures warm runs:**

| Job | Cache | Duration | Basis |
|---|---|---|---|
| `code` | warm | 3 min (assumed) | The measured 57 s setup, plus rebuilding only the workspace crates on 2 vCPUs |
| Post-merge Linux | warm | 4 min plus 2 (assumed) | `code` plus L2 suites (none exist yet) |
| Post-merge Windows | warm | 8 min plus 2 (assumed) | Windows runners set up more slowly |
| Release candidate (Linux + Windows, with installers) | warm | 40 min (assumed) | Two `check:full` runs plus bundling |
| Weekly full (Linux + Windows) | warm | 30 min (assumed) | Two `check:full` runs |

**Monthly estimate at the current pace.** One agent, about 4 merged PRs a working day, so about 80 a month. About 3 CI runs per PR after cancellation; about a quarter of PRs are docs-only.

| Item | Runs a month | Billed minutes each | Minutes a month |
|---|---|---|---|
| PR checks, code (`code` warm 3 + 2) | 60 PRs × 3 = 180 | 5 | 900 |
| PR checks, docs-only (`select` + `ci-required`) | 20 PRs × 3 = 60 | 2 | 120 |
| Post-merge Linux | 60 | 6 | 360 |
| Post-merge Windows (Windows-relevant merges, assumed half) | 30 | 10 | 300 |
| Weekly full | 4 | 30 | 120 |
| Cold-cache penalty (lockfile or toolchain changes, assumed 8; cold `code` is 9 against 3 warm) | 8 | +6 | 48 |
| Release candidates (assumed 2) | 2 | 40 | 80 |
| **Total** | | | **≈ 1,930** |

**That is just under the 2,000-minute quota, with little margin.** The PR tier dominates. If it runs over:

1. **Measure warm runs first (#14).** Every minute saved on the `code` job saves about 180 a month.
2. **Push to the PR branch less often:** batch fixes before pushing. Cancellation discards superseded runs, but a started job still costs its minutes.
3. **Run Windows post-merge on a daily batch** of the merged commits, instead of per merge. This saves up to 300 minutes, at the cost of slower Windows feedback.
4. **PO decisions:** a payment method or plan change (then overage is billed instead of blocked), or making the repository public (standard runners are free; #90).

**Windows minutes:** the runner pricing page lists only dollar rates and states no multiplier against the included minutes. Until the first Windows run shows its effect on the usage report, budget Windows as if it counted double. Then the Windows line above becomes 600 minutes, and the total about 2,230, over the quota.

**Artifact storage is 500 MB** (shared with Packages). Retention is set per upload:

- 7 days for diagnostics and logs;
- 14 days for release-candidate installers;
- releases go to GitHub Releases, which don't count as Actions artifacts.

The repository default is 90 days ([retention](https://docs.github.com/en/organizations/managing-organization-settings/configuring-the-retention-period-for-github-actions-artifacts-and-logs-in-your-organization)) and is too long for 500 MB.

**Watching the quota:** check usage in the account's billing settings at least weekly while the CI tiers are new. #14 records the first measured month and updates the tables above.
