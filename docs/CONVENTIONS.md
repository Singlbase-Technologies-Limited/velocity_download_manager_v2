# Naming conventions

Names follow each language's standard style, plus the project choices below. When this page doesn't cover a case, copy what the nearest existing code does, and add the case here in the same PR. Formatters and linters (`cargo fmt`, clippy, ESLint) enforce layout; this page covers what they can't.

Branch, commit and pull-request names are in [DEVELOPMENT_WORKFLOW.md](DEVELOPMENT_WORKFLOW.md).

## General

- **Use the domain words from the [glossary](GLOSSARY.md),** such as lease, coverage, validator and checkpoint. One concept has one name across Rust, TypeScript, docs and the UI contract. Don't use synonyms such as `chunk` for a lease or `resumeData` for a checkpoint.
- **Spell words out** (`download`, not `dl`). Common abbreviations are fine: `url`, `id`, `http`, `io`, `ui`.
- **Booleans read as a question:** `is_paused`, `has_validator`, `canResume`.
- **Units go in the name** when the type doesn't carry them: `timeout_ms`, `size_bytes`, `rateBytesPerSec`.

## Rust

These follow the [Rust API guidelines on naming](https://rust-lang.github.io/api-guidelines/naming.html).

| Item | Style | Example |
|---|---|---|
| Crates | `kebab-case` with a `velocity-` prefix | `velocity-core`, `velocity-desktop-v2` |
| Modules and files | `snake_case` | `validation.rs`, `database/migrations.rs` |
| Types, traits, enum variants | `UpperCamelCase` | `AppInfo`, `DirectTransfer`, `DownloadStatus::Paused` |
| Functions, methods, variables | `snake_case` | `app_info()`, `select_checks` |
| Constants and statics | `SCREAMING_SNAKE_CASE` | `PRODUCT_NAME`, `MAX_URL_LEN` |
| Conversions | `as_`/`to_`/`into_` by cost and ownership; `from_` constructors | `to_record()`, `from_metadata()` |
| Fallible constructors | `parse_…` or `try_…`, returning `Result` | `parse_content_range()` |
| Unit tests | In `#[cfg(test)] mod tests` beside the code; the name states the behaviour | `identifier_differs_from_v1` |
| Integration tests | `crates/<crate>/tests/<behaviour>.rs` | `tests/range_probe.rs` |

## TypeScript and React

| Item | Style | Example |
|---|---|---|
| Components and their files | `PascalCase`, one component per file | `App.tsx`, `DownloadList.tsx` |
| Other modules | `camelCase` | `appInfo.ts`, `formatSize.ts` |
| Hooks | `useCamelCase` | `useDownloads()` |
| Functions and variables | `camelCase` | `parseAppInfo()`, `windowHeading()` |
| Types and interfaces | `PascalCase`, no `I` prefix | `AppInfo` |
| Constants | `SCREAMING_SNAKE_CASE` for true constants, `camelCase` otherwise | `MAX_VISIBLE_ROWS` |
| Tests | Next to the code as `<module>.test.ts(x)`. `describe` names the unit, `it` states the behaviour | `appInfo.test.ts`, `it('rejects a response without a version')` |

## The UI contract (between Rust and TypeScript)

| Item | Style | Example |
|---|---|---|
| Tauri commands | `snake_case`, verb first | `app_info`, `add_downloads`, `get_snapshot` |
| Events | `velocity://` plus a `kebab-case` noun | `velocity://snapshot` |
| Error codes | `SCREAMING_SNAKE_CASE`, stable once released; they describe the cause, not the screen | `INTEGRITY_CHECK_FAILED` |
| Fields crossing the boundary | `snake_case` in Rust. The TypeScript side mirrors them as the contract (#25) decides | — |

## Scripts, files and docs

| Item | Style | Example |
|---|---|---|
| `package.json` scripts | `group:action` | `check:fast`, `test:focused` |
| Node scripts | `kebab-case.mjs`, tests as `<name>.test.mjs` | `check-pr.mjs`, `checks-lib.test.mjs` |
| Top-level docs | `UPPER_SNAKE_CASE.md` | `DEVELOPMENT_WORKFLOW.md` |
| Decision records | `NNNN-kebab-title.md` | `0001-v2-direction.md` |
| Research | `YYYY-MM-DD-kebab-title.md` | `2026-10-21-libcurl-engine-research-r2.md` |
| Environment variables | `VELOCITY_` plus `SCREAMING_SNAKE_CASE` | `VELOCITY_DATA_DIR` |
