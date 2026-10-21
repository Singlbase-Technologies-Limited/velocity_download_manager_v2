# Testing and verification levels

V2 sorts its checks into levels, so the feedback while you work stays fast and the heavy suites run only when a change needs them. Issue #15 owns the full definitions and the change-to-test matrix. This page lists the commands that exist today and what each one covers.

All commands run every check even after one fails, print a PASS/FAIL summary with timings, and exit non-zero if anything failed. They run tools directly, never through a shell.

## Levels and commands

| Level | Command | Scope | When |
|---|---|---|---|
| L1 | `pnpm check:fast` | Whole workspace: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace`, `tsc --noEmit`, ESLint (zero warnings), Vitest | Before every pull request |
| L2 | `pnpm test:focused -- <area> [args]` | The tests of one area. A crate runs `cargo test -p <crate> [args]` (unit and integration tests); `frontend` runs `vitest run src`; `scripts` runs `vitest run scripts` | While iterating (Red–Green–Refactor) |
| L3 | `pnpm check:pr [--base <ref>]` | The L1 checks that the branch's changes need, judged from every path changed since the merge base with `origin/main` (committed, staged, unstaged and untracked) | Before pushing; CI uses the same rules ([CI.md](CI.md)) |
| L4 | `pnpm check:full` | Every L1 check, then `vite build` and `cargo build --workspace --release` (the release profile: LTO, one codegen unit) | Before a release candidate, or after changing build settings |
| L5 | (none yet) | Production acceptance on installed builds | Defined in #15 |

**In CI** ([CI.md](CI.md)): pull requests run `pnpm check:pr` for the selected areas, and every merge to `main` runs `pnpm check:fast` and `pnpm check:links` on Linux.

For documentation, `pnpm check:links` checks that every relative link and heading anchor in the repository's Markdown files resolves. External links aren't fetched. CI's docs job runs it ([CI.md](CI.md)).

L2 integration and protocol suites (for example, an HTTP test server for the engine) don't exist yet. When they arrive they run under `test:focused` for their crate. L4 cross-platform, end-to-end, fuzz, stress and benchmark suites will join `check:full` or get their own deliberate jobs (#15).

### Examples

```sh
pnpm test:focused -- velocity-core                 # cargo test -p velocity-core
pnpm test:focused -- velocity-core app_info        # only tests whose name contains app_info
pnpm test:focused -- scripts -t selectChecks       # Vitest, one describe block
pnpm test:watch                                    # Vitest in watch mode
cargo test -p velocity-core -- --nocapture         # cargo directly, with output
pnpm check:pr                                      # what this branch needs
pnpm check:pr --base HEAD                          # only the uncommitted changes
```

Areas for `test:focused` are the workspace crates (`velocity-core`, `velocity-desktop-v2`), `frontend` and `scripts`. An unknown area prints the valid ones.

## How `check:pr` selects checks

The rules live in [`scripts/checks-lib.mjs`](../scripts/checks-lib.mjs) and are unit tested in `scripts/checks-lib.test.mjs`. The CI change detection (#13) reuses them, so local and CI selection can't drift apart. The first matching rule wins:

| Changed path | Selects |
|---|---|
| Lockfiles and shared configuration: `Cargo.lock`, `Cargo.toml`, `pnpm-lock.yaml`, `package.json`, `rust-toolchain.toml`, `.gitattributes`, `.editorconfig`, `.gitignore`, `.cargo/`, and `scripts/` (the check machinery) | **Everything** |
| Inside a workspace crate (`crates/<name>/`, `src-tauri/`) | Rust checks for that crate **and every crate that depends on it** (read from `cargo metadata`), plus `cargo fmt` |
| Frontend: `src/`, `public/`, `index.html`, `tsconfig.json`, `vite.config.ts`, `eslint.config.js` | `tsc`, ESLint, Vitest |
| Documentation: `docs/`, root `*.md` | No code checks |
| **Any other path** | **Everything** |

Selecting too little would let a broken change through, so anything shared or unknown selects everything. A new top-level directory needs a rule in `checks-lib.mjs` (with a test) before it can select less.

## Build settings for fast feedback

These settings were measured in #3. The timings are in [DEVELOPMENT.md](DEVELOPMENT.md#check-timings).

- **Dependencies are built with `opt-level = 1` and no debug info.** The app runs at a usable speed, and dependencies are compiled once.
- **Workspace crates are built with `debug = "line-tables-only"`.** Backtraces keep file and line numbers. The Tauri shell's library is also built as a `staticlib` and a `cdylib` for mobile targets. With full debug info, every test run re-linked several hundred megabytes. To debug with variables in a debugger, override it for that build: `CARGO_PROFILE_DEV_DEBUG=true cargo build`.
- **The release profile** (LTO, one codegen unit, `opt-level = "s"`) applies only to `--release` builds and never to development.

Two other settings were measured and rejected:

- **`tsc --incremental`** was slower: 1.3–1.6 s against 0.9 s, because writing the build-info file costs more than it saves on a project this size. Revisit when the frontend grows.
- **ESLint `--cache`** was rejected for correctness. The configuration uses type-aware rules, and ESLint caches results per file. A type change in one file can change the lint result of another file whose cache entry is still valid, so a cached run could pass when a fresh one fails.

## Writing tests

- **Test first:** write the failing test, make it pass with the smallest change, then refactor. Bug fixes get a regression test.
- **Never weaken a test to get a pass:** don't delete, skip or weaken it.
- **Location:** Rust unit tests live next to the code in `#[cfg(test)] mod tests`. Integration tests go in a crate's `tests/` directory. Frontend tests are `src/**/*.test.ts(x)`, and script tests are `scripts/**/*.test.mjs`.
- **Determinism:** tests don't use the network, wall-clock timing or shared global state. Fake servers and fixtures arrive with the engine work.
