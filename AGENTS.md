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
- Only the Product Owner decides merges. The PO merges, or approves one specific PR (for example, "I approve the PR") for the agent to squash-merge. Delete the branch after merge.
- Keep each pull request focused on one issue.
- The full procedure, from branch and commit names to merges and release tags, is in [docs/DEVELOPMENT_WORKFLOW.md](docs/DEVELOPMENT_WORKFLOW.md). What reviewers check is in [docs/REVIEWING.md](docs/REVIEWING.md).

## GitHub account for AI agents

- **The agent account:** AI agents commit, push and open pull requests here as the machine account **`velocity-claude-agent`**, never as the Product Owner. The PO can't approve PRs they authored.
- **How it's wired:** the PO's account stays the default `gh` account on the PC. Agents pass the agent's token explicitly, and this repository's git remote uses the `github-velocity-agent` SSH alias.
- **Before pushing:** check the identity as described in [docs/AI_AGENT_ACCOUNT.md](docs/AI_AGENT_ACCOUNT.md). If it isn't the agent, stop and ask the PO.
- **Organisation repositories,** such as the original Velocity Download Manager, use the PO's account. The agent is not an organisation member.

## Commands

```sh
pnpm check:prereqs             # check prerequisites
pnpm install --frozen-lockfile # install JavaScript dependencies
pnpm dev                       # run the app
pnpm check:fast                # L1 checks: fmt, clippy, cargo test, tsc, ESLint, Vitest
pnpm test:focused -- <area>    # L2: one crate, `frontend` or `scripts`, while iterating
pnpm check:pr                  # L3: the checks this branch's changes need
pnpm check:full                # L4 locally: L1 + production web build + Rust release build
```

Levels and selection rules: [docs/TESTING.md](docs/TESTING.md).

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
