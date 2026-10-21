# Velocity Download Manager V2 — contributor and agent rules

These rules apply to every contributor, human or AI agent. This is the bootstrap version from issue #2. Issue #4 replaces it with the full rule set, including path-scoped rules.

## Project

- V2 is a desktop download manager: a Tauri 2 shell (`src-tauri/`), a React and TypeScript UI (`src/`) and Rust core crates (`crates/`). It will have its own direct-download engine on libcurl, and libtorrent for BitTorrent.
- V2 is a separate application from the original Velocity Download Manager (V1) and must run beside it. Never reuse V1's identifiers, data directories, ports or registrations.
- V2 never bundles, calls or copies aria2.
- The Product Owner decides scope, architecture, dependencies, licensing, security trade-offs, repository settings, merges and releases.

## Workflow

- Every change starts from a GitHub issue.
- Branch from the latest `main` as `<type>/<issue>-<slug>`, for example `feat/41-range-probe` or `fix/12-ci-cache`.
- Open a pull request into `main`. Never push to `main` directly; the only exception was the bootstrap commit authorised in #2.
- Only the Product Owner merges. Delete the branch after merge.
- Keep each pull request focused on one issue.

## Commands

```sh
pnpm check:prereqs             # check prerequisites
pnpm install --frozen-lockfile # install JavaScript dependencies
pnpm dev                       # run the app
pnpm check:fast                # L1 checks: fmt, clippy, cargo test, tsc, ESLint, Vitest
```

Run `pnpm check:fast` before opening a pull request. Use pnpm only; `package.json` pins its version.

## Engineering rules

- **TDD:** for meaningful behaviour, write a failing test, make it pass with the smallest change, then refactor. Bug fixes get a regression test.
- **Tests:** never delete, skip or weaken a test to get a pass. Never weaken a security control.
- **Untrusted input:** URLs, file names, paths, server responses, extension messages and engine output are untrusted. Validate them at the native boundary, and never pass them through a shell.
- **User data:** use versioned migrations. Never delete or recreate user data to fix a problem.
- **Dependencies:** new crates, npm packages or native libraries need Product Owner approval, with their licence and maintenance status recorded.
- **Documentation:** update the relevant docs in the same pull request as the behaviour they describe.

## Reporting

- Report what you actually ran and what it printed.
- Mark each check as PASS, FAIL, BLOCKED or NOT RUN. Never claim an unrun check passed.
- Say what you could not verify, and why.
