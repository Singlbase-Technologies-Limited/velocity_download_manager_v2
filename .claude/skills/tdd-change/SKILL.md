---
name: tdd-change
description: Implement one behaviour in V2 test-first with Red–Green–Refactor, using the right focused test command for the area (Rust crate, frontend or scripts). Use for every meaningful code change or bug fix, including regression tests for bugs.
argument-hint: "<behaviour to implement>"
---

# Red–Green–Refactor: $ARGUMENTS

One behaviour per cycle. The rules are in AGENTS.md ("Engineering") and `.claude/rules/testing.md`. The commands are in [docs/TESTING.md](../../../docs/TESTING.md).

## Choose the area and its focused command

| Code under change | Test location | Focused command |
|---|---|---|
| A Rust crate (`crates/<crate>/`, `src-tauri/`) | Unit: `#[cfg(test)] mod tests` beside the code. Integration: `crates/<crate>/tests/<behaviour>.rs` | `pnpm test:focused -- <crate> [test name]` |
| Frontend (`src/`) | `src/**/<module>.test.ts(x)` | `pnpm test:focused -- frontend` |
| Scripts (`scripts/`) | `scripts/<name>.test.mjs` | `pnpm test:focused -- scripts -t "<describe name>"` |

The crate names are in `cargo metadata --no-deps`; `test:focused` lists them when given an unknown area.

## 1. Red

- **Write the smallest test** that states the behaviour as an observable outcome, named after the behaviour ([docs/CONVENTIONS.md](../../../docs/CONVENTIONS.md)).
- **Run it and watch it fail for the right reason.** An assertion about the missing behaviour counts. A compile error in unrelated code, or a typo, doesn't.
- **Record the failing output** (the test name and key message) for the PR's "Red first" line.
- **For a bug,** the regression test reproduces the reported failure first.

## 2. Green

- **Make the smallest change that passes.** Reuse existing code and patterns before adding new ones.
- **Rerun the focused command until it passes,** with no other test newly failing.

## 3. Refactor

- **Improve names, duplication and structure,** with the test still green. Never weaken, skip or loosen a test to get there.
- **Rerun the focused command** before moving to the next behaviour. Run `pnpm check:fast` (fmt, clippy `-D warnings`, cargo test, tsc, ESLint, Vitest) once, before opening the PR (CLAUDE.md, "Long runs").

## When it won't go green

Stop and report the actual error output and what you tried. Don't change the test to match broken behaviour, add `#[allow]` or `eslint-disable` to get past a check, or mark a test ignored.
