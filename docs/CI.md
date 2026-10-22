# CI design

V2's CI runs on GitHub Actions in a private repository on GitHub Free. The plan includes 2,000 runner minutes a month, and usage is **blocked**, not billed, once they run out ([billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)). Every design choice below protects two things: the checks that matter run on every change, and the monthly quota lasts the month.

This page is the design. The workflows are written in #13, #14, #16 to #19 and #91. Every figure below is labelled as **measured** or **assumed**. Assumed CI times get replaced by measured ones as the jobs start running (#14).

## Tiers

| # | Event | What runs | Platforms | Purpose | Built in |
|---|---|---|---|---|---|
| 1 | `pull_request` into `main` (opened, synchronised, reopened) | L1 for the selected areas, plus L2 for those areas. Docs-only changes run only the docs job | Linux | Fast feedback before review. One aggregate check, `ci-required`, summarises it | #13, #14, #91 |
| 2 | `push` to `main` (that is, after a squash merge) | **Linux:** every L1 check and the docs check, then save the Rust cache ([`main.yml`](../.github/workflows/main.yml), #14). Later: broader suites, and **Windows** jobs when Windows-relevant paths changed (below) | Linux, Windows | Catch what the PR tier deliberately left out, before a release candidate is built from `main`, and keep the PR cache warm | #14, #16 |
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
- **Native builds (#35):** the two native build jobs run only for their own inputs (`scripts/native/`, and native lock files other than `Cargo.lock` and `pnpm-lock.yaml`), for `.github/`, and for paths no rule knows. Other shared files, such as `Cargo.lock` or another check script, don't select them: the builds take minutes, and Windows minutes bill double ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)).
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
| `select` | Always | `node scripts/ci-select.mjs --base <PR base>` writes `rust`, `frontend`, `docs` and `native` flags to its outputs. It adds a table to the **run summary** with every job, *runs* or *skipped*, the reason for each, and the reason for each changed path. When docs are selected, the same job runs `node scripts/check-links.mjs` as a step |
| `code` | Rust or frontend selected | `pnpm check:pr`, the same command as locally. Tauri system packages and the pinned toolchain are installed only when Rust is selected |
| `native-linux`, `native-windows` | Native build inputs, CI files or unknown paths changed | `node scripts/native/build.mjs` on `ubuntu-latest` and `windows-latest`: verifies every source, builds, checks `curl -V` against the lock file, and uploads `build-info.json` as the `build-info-linux` or `build-info-windows` artifact (7 days). No cache yet |
| `ci-required` | Unless the run was cancelled (`if: ${{ !cancelled() }}`) | Passes only if `select` succeeded and `code` and both native builds each succeeded or were skipped by the selection. It writes its own table to the run summary. A run superseded by a newer push is cancelled whole and reports nothing; `always()` would report a false failure there |

**Why so few jobs.** GitHub bills every job's time **rounded up to a whole minute** ([runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing)), and every job repeats checkout and setup. So Rust and frontend share the `code` job, and the one-second docs check runs inside `select`. A docs-only PR then costs 2 billed minutes (`select` and `ci-required`), and a code PR costs the `code` job plus 2.

**Skipped jobs and required checks.** A job skipped by an `if:` condition reports *Success*, while a workflow skipped by path filters leaves a required check *Pending* forever ([troubleshooting required checks](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/collaborating-on-repositories-with-code-quality-features/troubleshooting-required-status-checks)). So the workflow never uses `on.paths`, and `ci-required` reports on every run that isn't cancelled. That keeps it usable as a required check if branch protection becomes available (#7).

**Selection errors fail the run.** If `ci-select.mjs` can't compute the change (for example, missing history), it exits non-zero rather than guessing, and `ci-required` fails.

## Caching

The repository gets **10 GB of Actions cache**, separate from artifact storage. GitHub evicts the least recently used entries above that ([billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)).

| Cache | Key | Contents | Size | Reasoning |
|---|---|---|---|---|
| Rust (Linux) | OS + toolchain + `Cargo.lock` hash | Compiled **dependencies** only (`~/.cargo/registry`, `target/` without workspace crates) | **Measured in CI:** 319 MiB as saved (1.1 GiB of `target/` plus a 201 MiB registry before compression); locally 365 MiB with gzip | A cold build is most of a cold job (**measured:** 571 s for the clippy and test builds on 2 vCPUs). Workspace crates rebuild in seconds, so caching them would only add upload time and size |
| Rust (Windows) | Same | Same | **Assumed:** similar to Linux (not measured) | Windows minutes cost more ($0.010 against $0.006 a minute), so a cache hit saves more |
| pnpm store | — | **Not cached** | 147 MiB (Linux) | `pnpm install --frozen-lockfile` took **4 s** in CI (PR #104), less than a cache restore and save would cost (PO decision on #14) |

**Caching rules:**

- **Budget:** about 0.6 GB a platform when compressed, so Linux and Windows, with one previous key each during a lockfile change, use about 2.5 GB of the 10 GB.
- **Save on `main` only;** pull requests restore. A PR branch's cache is visible only to that branch, so saving it there would fill the 10 GB with copies.
- **Keys change only when the toolchain or lockfile changes,** so a cache uploads rarely.
- **No cache for Tauri system packages** (`apt`): they installed in **35 s** in CI (PR #104), about what restoring a cache of them would take.

**How it's wired (#14):**

- **Action:** GitHub's own `actions/cache/restore` and `actions/cache/save`, pinned by SHA. No third-party cache action is used, because any third-party action is a dependency that needs PO approval (AGENTS.md).
- **Setup:** the shared composite action [`.github/actions/setup`](../.github/actions/setup/action.yml) restores the cache for every job that builds Rust. When the exact key is missing, it falls back to the newest older key (`restore-keys`), so a dependency bump starts mostly warm.
- **Saving:** only [`main.yml`](../.github/workflows/main.yml), the Linux checks after each merge to `main`, saves the cache. It does so after a fully green run, when the exact key isn't cached yet, and after removing the workspace crates' artefacts.
- **No incremental compilation in CI:** `CARGO_INCREMENTAL=0` keeps the cache smaller and CI builds faster.

## Cancelling and de-duplicating

- **Superseded PR runs are cancelled.** Each workflow sets `concurrency: { group: <workflow>-<PR or ref>, cancel-in-progress: true }` for pull requests ([concurrency](https://docs.github.com/en/actions/writing-workflows/choosing-what-your-workflow-does/control-the-concurrency-of-workflows-and-jobs)). Runs on `main` and on tags are never cancelled: each commit's result matters.
- **No duplicate runs for one commit.** Pull-request checks trigger on `pull_request` only, and `push` triggers only for `main` and tags. A branch push therefore doesn't run CI a second time beside its PR.
- **Path filters live in the selector,** not in `on.paths`. Then the aggregate `ci-required` check always reports, instead of being missing when a workflow is skipped.

## Permissions and supply chain

GitHub's guidance is in [secure use](https://docs.github.com/en/actions/reference/security/secure-use):

- **Read-only by default.** Every workflow declares `permissions: contents: read`, and a job raises only what it needs (for example `contents: write` for the draft-release job in #18).
- **Pinned actions.** Third-party actions are pinned to a **full commit SHA**, with the version in a comment. GitHub's own `actions/*` are pinned the same way, for consistency.
- **Dependabot** updates the pins: [`.github/dependabot.yml`](../.github/dependabot.yml) checks the workflows and the composite action **monthly**, grouped into **one PR**, so an update costs one CI run. Each update is an ordinary PR the PO reviews. Cargo and npm updates aren't automated: dependency changes need PO approval (#9, #75).
- **No `pull_request_target`** that checks out PR code. No secrets in PR workflows.
- **Untrusted text stays out of commands.** Titles, branch names and other user-controlled strings never go straight into `run:` scripts; they pass through environment variables.
- **Self-hosted runners:** none. If one is ever added (PO decision), it never runs code from a fork.

## Collecting failures

Every run completes every check that can still run, and reports all failures in one place (#91). The details are in [CI_DIAGNOSTICS.md](CI_DIAGNOSTICS.md).

- **Checks run to completion:**
  - The check runner runs every check even after one fails.
  - Clippy uses `--keep-going` and `cargo test` uses `--no-fail-fast`.
  - Report and summary steps use `if: ${{ !cancelled() }}`.
  - Mandatory checks never use `continue-on-error`, which would turn a failure into a success.
  - Cross-platform matrices (#16, #17, #19) use `fail-fast: false`.
- **Infrastructure problems are reported as INFRA,** separately from code defects. That covers a failed checkout, toolchain install, cache restore or `pnpm install`, and a job that died before reporting. A check that couldn't run because its prerequisite failed is **BLOCKED**, with the prerequisite named.
- **One consolidated report.** `ci-required` (PRs) and the `main` job publish a table in the run summary and the `ci-diagnostics` artifact (`ci-diagnostics.json`). The run fails unless every mandatory check passed or was deselected. The `/ci-triage` skill reads it.
- **Gated jobs stay strict.** Packaging and release jobs (#17, #18) keep the default `needs` semantics, so nothing is built for release after a failed check.

## L4 schedule

| Run | When | Content | Platforms |
|---|---|---|---|
| Weekly full | Mondays 03:00 UTC, only if `main` changed since the last run | `check:full`, all L2 suites | Linux, Windows |
| Fuzzing, stress, extended benchmarks | Weekly, staggered, once the suites exist (#73, #77, #20) | Time-boxed per target, with a fixed seed recorded | Linux |
| macOS | `workflow_dispatch` only | `check:full` | macOS (#83) |

macOS minutes cost about 10 times Linux ($0.062 against $0.006 a minute), so macOS never runs on a schedule or per change until it's an established platform and the quota allows it.

## Costs

**Measured locally** (docs/DEVELOPMENT.md, 2026-10-21). A cold dependency build takes 278 s on Windows (16 logical CPUs) and 413 s on Linux (WSL2, 2 vCPUs, similar to a 2-vCPU hosted runner). A warm `check:fast` takes 5.5–9 s. A full `check:full` takes 224–249 s, of which 215–232 s is the release build.

**Measured in CI** (`ubuntu-latest`, 2 vCPUs, 2026-10-21). Every run of the PR tier and the first `main` run so far. Billed minutes are each job rounded up to a whole minute ([runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing)); a skipped job costs nothing.

| Run | Change | Cache | Wall time | Jobs (s) | Billed |
|---|---|---|---|---|---|
| [39657529808](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39657529808) | PR #104, everything | none | 542 s | select 14, code 519, docs 31, ci-required 4 | 12 min |
| [39663435122](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39663435122) | PR #104, everything | none | 566 s | select 21, code 536, ci-required 4 | 11 min |
| [39666333757](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39666333757) | PR #105, everything | none | 505 s | select 21, code 476, ci-required 3 | 10 min |
| [39669208703](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39669208703) | `main` after #105, all L1, **saved the cache** | none | 453 s | linux 453 | 8 min |
| [39670447026](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39670447026) | demo #106, docs only | — | 32 s | select 25, ci-required 4 (`code` skipped) | 2 min |
| [39670571690](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39670571690) | demo #107, one crate | warm | 113 s | select 12, code 92, ci-required 4 | 4 min |
| [39670483912](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39670483912) | demo #108, `Cargo.lock` (everything) | warm, via the fallback key | 181 s | select 22, code 150, ci-required 4 | 5 min |

- **Medians:**
  - Successful PR runs: **343 s** (6 runs).
  - Cold PR runs: **542 s**, 11 billed minutes.
  - Warm code PR runs: 113–181 s, **4–5 billed minutes**.
- **A cold `code` job:** about 1 min of setup (Node 7 s, Tauri packages 35 s, toolchain 9 s, `pnpm install` 4 s), then about 7.5 min of compiling (clippy about 280 s, tests about 140 s).
- **With the cache restored:** clippy takes 2–3 s and the tests 3–6 s. The cache is **319 MiB** (key `rust-v1-Linux-<hash>`, saved by `main`).
- **The rest is the 3 billed minutes** of fixed cost per code run: `select`, the setup inside `code`, and `ci-required`.
- **Failure collection (#91) adds no billed minutes.** A warm run with everything selected cost 4 billed minutes before and after it: [39671582041](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39671582041) (PR #109) against [39676944320](https://github.com/vincentarisawork/velocity_download_manager_v2/actions/runs/39676944320) (PR #110). `ci-required` grew from 5 s to 17–19 s (checkout, Node, downloading the reports), still within its one billed minute.

**Still assumed** (no runs yet):

| Job | Cache | Duration | Basis |
|---|---|---|---|
| Post-merge Linux, warm | warm | 4 min billed | The warm `code` job plus about 1 min of setup |
| Post-merge Windows | warm | 8 min plus 2 | Windows runners set up more slowly |
| Release candidate (Linux + Windows, with installers) | warm | 40 min | Two `check:full` runs plus bundling |
| Weekly full (Linux + Windows) | warm | 30 min | Two `check:full` runs |

**Monthly estimate at the current pace.** One agent, about 4 merged PRs a working day, so about 80 a month. About 3 CI runs per PR after cancellation; about a quarter of PRs are docs-only.

| Item | Runs a month | Billed minutes each | Minutes a month |
|---|---|---|---|
| PR checks, code (measured 4–5, warm) | 60 PRs × 3 = 180 | 5 | 900 |
| PR checks, docs-only (measured) | 20 PRs × 3 = 60 | 2 | 120 |
| Post-merge Linux | 80 | 4 | 320 |
| Post-merge Windows (Windows-relevant merges, assumed half; #16) | 30 | 10 | 300 |
| Weekly full | 4 | 30 | 120 |
| Cold-cache penalty (new `Cargo.lock` or toolchain, assumed 8; cold `main` and PR runs) | 8 | +6 | 48 |
| Release candidates (assumed 2) | 2 | 40 | 80 |
| **Total** | | | **≈ 1,890** |

**That is under the 2,000-minute quota, with little margin.** The PR tier dominates. If it runs over:

1. **Push to the PR branch less often:** batch fixes before pushing. Cancellation discards superseded runs (demonstrated on demo #107), but a started job still costs its minutes.
2. **Run Windows post-merge on a daily batch** of the merged commits, instead of per merge. This saves up to 300 minutes, at the cost of slower Windows feedback.
3. **PO decisions:** a payment method or plan change (then overage is billed instead of blocked), or making the repository public (standard runners are free; #90).

**Windows minutes:** the runner pricing page lists only dollar rates and states no multiplier against the included minutes. Until the first Windows run shows its effect on the usage report, budget Windows as if it counted double. Then the Windows line above becomes 600 minutes, and the total about 2,190, over the quota.

**Artifact storage is 500 MB** (shared with Packages). Retention is set per upload:

- 7 days for diagnostics and logs;
- 14 days for release-candidate installers;
- releases go to GitHub Releases, which don't count as Actions artifacts.

The repository default is 90 days ([retention](https://docs.github.com/en/organizations/managing-organization-settings/configuring-the-retention-period-for-github-actions-artifacts-and-logs-in-your-organization)) and is too long for 500 MB.

**Watching the quota:** check usage in the account's billing settings at least weekly while the CI tiers are new. #14 records the first measured month and updates the tables above.
