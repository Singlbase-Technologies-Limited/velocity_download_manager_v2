# Reviewing pull requests

This is what a reviewer (the Product Owner today, future maintainers later) checks before approving and squash-merging a pull request into `main`. The workflow itself is in [DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md). The [pull request template](../.github/PULL_REQUEST_TEMPLATE.md) asks authors for the evidence listed here, and links back to this page.

## Before reading the code

- [ ] The PR is linked to an issue and stays within that issue's scope. Anything else is split out or has its own issue.
- [ ] The base is `main` and the branch name is `<type>/<issue>-<slug>`.
- [ ] The title is a Conventional Commit (`type(scope): description`). When merging, check that the proposed squash commit message follows the convention too, because a PR with a single commit keeps that commit's message.
- [ ] The description is the PR template filled in, and passes `pnpm check:pr-body <file> --title "…"` (PO decision on #59).
- [ ] Closing keywords (`closes #N`, `fixes #N`) appear only where merging should really close that issue.

## Evidence

- [ ] The tests were written first. The PR shows the failing test (red) before the change, for meaningful behaviour.
- [ ] The commands actually run are listed with their results. Every platform and check is marked PASS, FAIL, BLOCKED or NOT RUN, and nothing unrun is described as passing.
- [ ] `pnpm check:fast` passed, and the selected CI checks passed. Temporarily, the batch CI run that included the PR's head commit passed instead ([CI.md](CI.md#while-ci-runs-by-hand-only), #204).
- [ ] No test was deleted, skipped or weakened, and no lint rule was relaxed, just to get a pass. Any changed assertion is explained.

## Correctness and safety

- [ ] Untrusted input is validated at the native boundary: URLs, file names, paths, headers, server responses, torrent metadata, extension messages.
- [ ] No shell interpolation of user input.
- [ ] TLS verification and other security controls are not weakened.
- [ ] Secrets don't reach logs, the database, checkpoints, temporary files or process arguments.
- [ ] User data survives: schema changes use versioned migrations, and nothing deletes or recreates user data to fix a problem.
- [ ] File operations stay inside the folders the user chose, don't follow links (symbolic links, junctions, mount points; `velocity_core::fs_safety`), and delete only files the download owns.
- [ ] V2 stays separate from V1: no shared identifiers, data directories, ports or browser registrations.

## Maintainability

- [ ] The change sits in the right layer: UI in `src/`, thin Tauri commands in `src-tauri/`, domain logic in `crates/`.
- [ ] No new dependency without Product Owner approval and a recorded licence ([DEPENDENCIES.md](DEPENDENCIES.md)); the register of approved dependencies is updated.
- [ ] The documentation affected by the change is updated in the same PR, and the README is still accurate.

## After merging

- [ ] The head branch was deleted (automatic).
- [ ] The issue is closed only if all its acceptance criteria are met; otherwise it gets a comment saying what remains.
- [ ] The project board shows the real state.
