# The UI contract

The UI (`src/`) and the native side (`src-tauri/`, `crates/`) talk only through typed Tauri commands and events. This page describes the shared types, how they're kept in step, and how to change them (#25).

## The shared types

They're defined once in Rust, in `crates/velocity-core/src/domain.rs` (plus `AppInfo` in `lib.rs`), and mirrored by strict Zod schemas in `src/contract.ts`.

| Type | What it is | Notes |
|---|---|---|
| `DownloadStatus` | Where a download is: `queued`, `connecting`, `downloading`, `paused`, `verifying`, `retrying`, `stalled`, `completed`, `failed`, `removed`, `fetchingMetadata`, `choosingFiles`, `seeding`, `unknown` | V1's fourteen values. **A new status needs an ADR**, so the UI stays consistent. `completed`, `failed` and `removed` are terminal |
| `DownloadKind` | `http`, `ftp`, `sftp`, `metalink`, `torrent`, `magnet` | — |
| `DownloadRecord` | One download as the UI sees it: a stable app `id`, kind, source URL (credentials removed), file name, folder, category, status, progress, error, `verification`, timestamps (milliseconds since the Unix epoch) | Never carries an engine's own id |
| `Verification` | What was proven about a completed download's bytes: `verified` (a checksum or trusted piece hashes matched), `sizeVerified` (complete, with no hash checked) or `notVerified` (resumed without a strong validator, [ADR-0010](decisions/0010-resume-without-validator.md)). The fixture `verifications.json` lists them | `null` until the download completes. Show "Verified" only for `verified`. Added in contract version 3 (#46) |
| `Progress` | Completed and total bytes (`totalBytes` is `null` while unknown), download and upload speeds in bytes per second, the connections in use now, and `connectionDetail` | — |
| `ConnectionDetail` | A direct download's connections: `requested`, `allowed` (at least 1, never above `requested`) and `limitedBy`, what holds `allowed` below `requested`: `perHostCap`, `sharedBudget`, `serverThrottling`, `rampingUp` (adaptive connections are still being added), `noGain` (more connections stopped making it faster) or `fileSize`, or `null` (#47). The fixture `connection-limits.json` lists them | `null` for torrents and while no engine reports it. Added in contract version 2 |
| `TransferError` | `{ class, httpStatus, message, retryable }`. `class` is `transient`, `throttled` or `permanent` | Engine-neutral. A permanent failure is never retryable |
| `Capabilities` | What an engine really supports: protocols, live connection changes, validator-checked resume, Metalink, maximum connections per host | Reported, never assumed (research report section 4.2) |
| `ErrorCode` | Stable codes for command failures, in `SCREAMING_SNAKE_CASE` (22 codes: V1's, without the four aria2 `RPC_*` codes). Each has a user-facing sentence (`user_message`) | The code is the contract; the message is for people |
| `AppInfo` | `{ name, version, commandApiVersion }` | Returned by `app_info` |

**Conventions:**
- **Field names** are camelCase on the wire (`#[serde(rename_all = "camelCase")]`).
- **Optional values** are `null`, never omitted.
- **No aria2 terms:** no aria2 shapes or option names appear in any of these types.

## The contract version

`COMMAND_API_VERSION` exists in both `velocity_core` and `src/contract.ts`, and `app_info` reports the native side's version. At startup the UI validates the response. **If the versions differ, the window refuses to start** and explains that the two parts don't match, rather than misreading messages. Raise the version, on both sides in the same PR, whenever a command or shared type changes incompatibly.

## How the two sides stay in step

1. **Rust generates contract fixtures** in `tests/fixtures/contract/` from the real types: `statuses.json`, `error-codes.json`, `download-record.json`, `capabilities.json` and `app-info.json`. A Rust test fails when a fixture no longer matches the types: "fixture … is out of date; regenerate it with `VELOCITY_UPDATE_FIXTURES=1`".
2. **Vitest validates the same fixtures** with the UI's strict schemas (`src/contract.test.ts`). An unknown field, a missing field, an unknown enum value or a wrong type fails the test.
3. **So a change on one side alone always fails a test:**
   - Change Rust without regenerating, and the Rust contract test fails.
   - Regenerate without updating the schemas, and the Vitest contract test fails.
   - Change only the schemas, and they no longer match the fixtures.

   This was shown in #25 by renaming a field.

## Changing the contract

1. **Change the Rust type,** test first in `domain.rs`.
2. **Regenerate the fixtures:** `VELOCITY_UPDATE_FIXTURES=1 cargo test -p velocity-core contract`.
3. **Update `src/contract.ts`** until `pnpm test:focused -- frontend` passes.
4. **If the change is incompatible,** raise `COMMAND_API_VERSION` on both sides.
5. **Update this page** if a type's meaning changed.

Status values and error codes are part of the contract: adding a status needs an ADR, and every new error code needs a user-facing message, which a test checks.
