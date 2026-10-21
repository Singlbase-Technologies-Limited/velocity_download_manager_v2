# Reviewing pull requests

This is what a reviewer (the Product Owner today, future maintainers later) checks before approving and squash-merging a pull request into `main`. The workflow itself is in [DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md).

## Before reading the code

- [ ] The PR is linked to an issue and stays within that issue's scope. Anything else is split out or has its own issue.
- [ ] The base is `main` and the branch name is `<type>/<issue>-<slug>`.
- [ ] The title is a Conventional Commit (`type(scope): description`). When merging, check that the proposed squash commit message follows the convention too, because a PR with a single commit keeps that commit's message.
- [ ] Closing keywords (`closes #N`, `fixes #N`) appear only where merging should really close that issue.

## Evidence

- [ ] The tests were written first. The PR shows the failing test (red) before the change, for meaningful behaviour.
- [ ] The commands actually run are listed with their results. Every platform and check is marked PASS, FAIL, BLOCKED or NOT RUN, and nothing unrun is described as passing.
- [ ] `pnpm check:fast` passed, and the selected CI checks passed once CI exists.
- [ ] No test was deleted, skipped or weakened, and no lint rule was relaxed, just to get a pass. Any changed assertion is explained.

## Correctness and safety

- [ ] Untrusted input is validated at the native boundary: URLs, file names, paths, headers, server responses, torrent metadata, extension messages.
- [ ] No shell interpolation of user input.
- [ ] TLS verification and other security controls are not weakened.
- [ ] Secrets don't reach logs, the database, checkpoints, temporary files or process arguments.
- [ ] User data survives: schema changes use versioned migrations, and nothing deletes or recreates user data to fix a problem.
- [ ] File operations stay inside the folders the user chose, don't follow links or reparse points, and delete only files the download owns.
- [ ] V2 stays separate from V1: no shared identifiers, data directories, ports or browser registrations.

## Maintainability

- [ ] The change sits in the right layer: UI in `src/`, thin Tauri commands in `src-tauri/`, domain logic in `crates/`.
- [ ] No new dependency without Product Owner approval and a recorded licence (#9).
- [ ] The documentation affected by the change is updated in the same PR, and the README is still accurate.

## After merging

- [ ] The head branch was deleted (automatic).
- [ ] The issue is closed only if all its acceptance criteria are met; otherwise it gets a comment saying what remains.
- [ ] The project board shows the real state.
