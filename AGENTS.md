# Velocity Download Manager V2 — contributor and agent rules

These rules apply to every contributor, human or AI agent. Each rule has one home: this file holds the rules for the whole repository, and it links to the documents that hold the details. Claude Code also reads [CLAUDE.md](CLAUDE.md) and the path-scoped rules in `.claude/rules/`.

## Project and authority

- **What V2 is:** a desktop download manager with a Tauri 2 shell (`src-tauri/`), a React and TypeScript UI (`src/`) and Rust crates (`crates/`). It has its own direct-download engine on libcurl for HTTP, HTTPS, FTP and SFTP, and uses libtorrent for BitTorrent ([ADR-0001](docs/decisions/0001-v2-direction.md)).
- **No aria2:** V2 never bundles, calls, falls back to or copies aria2. V1, the original Velocity Download Manager, may serve only as a feature and benchmark reference, and is never modified from V2 work.
- **Separate from V1:** V2 runs beside V1, so it never reuses V1's identifiers, data directories, ports or registrations (#11, #27).
- **Platforms:** Windows and Linux are verified for development. macOS is not yet established (#83).
- **The Product Owner (PO) decides** scope, architecture, dependencies, licensing, security trade-offs, repository settings, merges and releases. When a change needs one of these decisions, stop and ask; don't decide by default. Accepted decisions are recorded in [docs/decisions/](docs/decisions/README.md).

## Evidence before claims

- **What counts as evidence:** the checked-out code, tests, GitHub issues and the documents in `docs/`. Check the current state before you describe it.
- **Requirements:** read the issue's scope, acceptance criteria, non-goals and linked documents before starting. Don't invent requirements or widen the scope silently.
- **External APIs:** use the official documentation for the exact version of a library, tool or platform. Record documented limitations, and separate what you verified from what you assumed.

## Architecture

The components and who owns what state are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Keep each layer to its job:

- **`src/`:** UI and presentation state. It reaches native code only through the typed commands and events, never directly.
- **`src-tauri/`:** the desktop shell. Its commands validate input and delegate to the core, without duplicating core logic.
- **`crates/velocity-core/`:** reusable domain logic: the manager, policy, validation and persistence.
- **Engine code:** the transfer engine lives in `crates/velocity-transfer` and the libcurl layer in `crates/velocity-curl` ([ADR-0002](docs/decisions/0002-engine-crate-layout.md)). Only the libcurl layer calls libcurl, and only the torrent bridge calls libtorrent.
- **Abstractions:** create them for real boundaries, not for hypothetical reuse. Prefer small, cohesive modules and composition over large conditional ones.

## Reuse before you add

- **Search before writing.** Search the codebase for code that already does the job: a function, type, component, hook, command, error code, script or test helper. Reuse or extend it.
- **Follow the established pattern** for the kind of code you're writing: error handling, validation, state, commands, tests and naming. Two different solutions to the same problem are a defect.
- **A new pattern needs a reason.** If you introduce one, or a second implementation of an existing concept, say why in the PR. When it replaces the old one, migrate the old uses in the same PR, or open an issue for them.
- **Names:** follow [docs/CONVENTIONS.md](docs/CONVENTIONS.md).

## Engineering

- **Test-driven:** for meaningful behaviour, write a failing test, make it pass with the smallest change, then refactor. Every bug fix gets a regression test, unless you document a concrete reason in the PR.
- **Tests:**
  - Test behaviour and contracts, not private details.
  - Prefer deterministic local fixtures over the public internet.
  - Never delete, skip, weaken or rewrite a test just to get a pass.
- **Errors are explicit and actionable:**
  - No swallowed errors.
  - No fake progress, statuses or results.
  - No test-only workarounds in production code.
  - No unmeasured performance claims.
- **Unfinished features:** an unfinished feature is absent or clearly labelled, never faked with mock data.
- **Quality:** TypeScript strictness and Rust warnings matter. Code must pass `pnpm check:fast` with zero lint warnings.
- **Dependencies:** a new crate, npm package, native library, service or background process needs PO approval, with its licence and maintenance status recorded ([docs/DEPENDENCIES.md](docs/DEPENDENCIES.md)). Don't add one for behaviour the current stack already has.

## Security and data

- **Untrusted input:** URLs, file names, paths, headers, server responses, torrent metadata, browser and extension messages, and engine output are untrusted. Validate them with typed parsing at the native boundary.
- **No shells:** never pass user input through a shell. Run programs with argument lists only.
- **TLS and security controls:** never disable TLS verification or weaken a security control, even for a test.
- **Secrets stay out of storage:** credentials, cookies, custom headers, SSH keys and tokens never reach the database, logs, checkpoints, temporary files, process arguments or the UI. Redact them in diagnostics.
- **File safety:** check path containment and ownership before you move, overwrite or delete anything. Never follow links or reparse points, and delete only files the operation owns.
- **No telemetry:** no telemetry, analytics or network calls beyond the downloads themselves without PO approval.
- **User data:** use versioned, backward-compatible migrations. Never delete or recreate user data to fix a problem.

## Workflow

The full procedure is in [docs/DEVELOPMENT_WORKFLOW.md](docs/DEVELOPMENT_WORKFLOW.md), and what reviewers check is in [docs/REVIEWING.md](docs/REVIEWING.md). The rules:

- **Follow the roadmap:** epic #1 lists every issue by phase, and the milestones R1 to R10 set the order. Take the next issue from the earliest milestone that has open, unblocked work, and confirm the choice with the PO.
- **Issues and branches:**
  - Every change starts from a GitHub issue.
  - Branch from the latest `main` as `<type>/<issue>-<slug>`, for example `feat/41-range-probe`.
  - Keep one issue per pull request.
- **Never push to `main` directly.** Open a pull request into `main`. Never force-push or rewrite shared history, and check for uncommitted work before switching branches.
- **The PO decides merges.** The PO merges, or approves one specific PR, for example with "I approve the PR", for the agent to squash-merge. Branches are deleted after merge.
- **Closing keywords:** write `closes #N` or `fixes #N` only when merging should close that issue. Otherwise write "Part of #N".
- **Keep the issue, its project board item and epic #1 current.** After a merge, tick the issue's box in #1; GitHub doesn't tick it.

## GitHub account for AI agents

- **The agent account:** AI agents commit, push and open pull requests here as **`velocity-claude-agent`**, never as the PO, because the PO can't approve PRs they authored.
- **How it's wired:**
  - The PO's account stays the default `gh` account.
  - Agents pass the agent's token explicitly.
  - The git remote uses the `github-velocity-agent` SSH alias.
- **Before pushing:** check the identity as described in [docs/AI_AGENT_ACCOUNT.md](docs/AI_AGENT_ACCOUNT.md). If it isn't the agent, stop and ask the PO.
- **Organisation repositories,** including V1, use the PO's account. The agent is not an organisation member.

## Commands

```sh
pnpm check:prereqs             # check prerequisites
pnpm install --frozen-lockfile # install JavaScript dependencies
pnpm dev                       # run the app
pnpm test:focused -- <area>    # L2: one crate, `frontend` or `scripts`, while iterating
pnpm check:fast                # L1 checks: fmt, clippy, cargo test, tsc, ESLint, Vitest
pnpm check:pr                  # L3: the checks this branch's changes need
pnpm check:full                # L4 locally: L1 + production web build + Rust release build
pnpm check:links               # relative links and anchors in all Markdown files
pnpm check:guidance            # AGENTS.md, CLAUDE.md, rules and skills (docs/AGENT_GUIDANCE.md)
pnpm native:build              # build libcurl and its libraries from verified sources (docs/BUILDING_NATIVE.md)
```

Run `pnpm check:fast` before opening a pull request. Use pnpm only; `package.json` pins its version. The levels and selection rules are in [docs/TESTING.md](docs/TESTING.md).

## Documentation

- **Same PR:** update the canonical document in the same PR as the behaviour, setup, configuration, security, packaging or operations it describes.
- **One home:** each topic has one canonical document. Link to it instead of copying it, and add new documents to [docs/README.md](docs/README.md).
- **README:** review the root README in every PR. Don't rewrite unrelated documentation.

## Reporting

- **Report what actually ran:** the branch, the files changed, the commands run with their real results, the documentation updated, and the remaining limitations.
- **Mark every check** as PASS, FAIL, BLOCKED or NOT RUN. Never claim an unrun check passed.
- **If a tool, account, board or platform is unavailable,** say so. Never pretend it was used.
