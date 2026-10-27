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
| `TransferError` | `{ class, httpStatus, message, retryable, hostKey }`. `class` is `transient`, `throttled` or `permanent`. `hostKey` is `{ host, port, fingerprint, pinned }` when an SFTP server's host key isn't trusted, else `null` | Engine-neutral. A permanent failure is never retryable. With `hostKey`, the UI shows the SHA-256 `fingerprint` for the user to confirm; with `pinned` set, the key changed and is always refused ([SECURITY.md](SECURITY.md#sftp-host-keys-53)). `hostKey` since contract version 6 (#53) |
| `Capabilities` | What an engine really supports: the protocols, each as a `ProtocolCapabilities` (`kind`, `segmented`, `resume`, `validatorResume`, `http2`, and `encryption`: whether the connection can be encrypted with its certificate checked, `https://` or FTPS; a protocol not listed isn't supported), live connection changes, Metalink, and the most connections per host now (the performance profile's cap) | Reported, never assumed (research report section 4.2). Per protocol since contract version 4 (#57), with `encryption` since version 5 (#51). `cleartextWarning` in `src/capabilities.ts` holds the warning for a plain `ftp://` download (PO decision on #51); [SETTINGS.md](SETTINGS.md#what-a-build-can-do) says how the UI uses it |
| `ErrorCode` | Stable codes for command failures, in `SCREAMING_SNAKE_CASE` (23 codes: V1's, without the four aria2 `RPC_*` codes, plus `NAME_TAKEN`, since contract version 9, for a category name another category has). Each has a user-facing sentence (`user_message`) | The code is the contract; the message is for people |
| `AppInfo` | `{ name, version, commandApiVersion }` | Returned by `app_info` |
| `DownloadsSnapshot` | What the UI is sent about the downloads (#58): `downloads` (records, the unfinished ones in queue order, then the finished ones, newest first), `unavailable` (stored downloads this build can't act on: `id`, `sourceUrl`, `fileName`, `directory`, `createdAtMs`), `needsLogin` (ids of downloads that came back after a restart without their login), `engine`, `torrentEngine` (the torrent helper's health, since version 8) and `storageError` (the last failure to save, or `null`) | Sent as the `downloads-snapshot` event and returned by `get_snapshot` |
| `EngineHealth` | `{ state: "running" }`, `{ state: "restarting", attempt }` or `{ state: "unavailable", code }` | The fixture `engine-health.json` lists one of each |
| `Category` | A download category (#31): `id`, `name`, `color` (`#rrggbb`), `icon`, `extensions` (lowercase, without the dot), `directory` (its own folder, or `null`) and `builtin`. The fixture `category-icons.json` lists the icons | Since contract version 9. Created and edited as a `CategoryInput` (the same fields without `id` and `builtin`). The rules are in [FILE_NAMING.md](FILE_NAMING.md#categories) |
| `DestinationCheck` | What adding a download would do (#31): `directory`, `directoryExists`, `automaticDirectory`, `categoryId`, `fileName` (`null` when the server will name it), `fileExists`, `partialDownloadExists` (V2's or V1's) and `suggestedName` (the first free name when it is taken) | Since contract version 9. Free disk space isn't reported yet |
| `OnConflict` | What adding does when the name is taken (#31): `ask` or `rename`. The fixture `on-conflict.json` lists them | Never replace (PO decision on #31). For #60's Add command |
| `BatchResult` | What a command over several downloads did: `done` (ids), `failed` (`{ id, code }`), and `kept` (`{ path, reason, code }`, files a removal with files left in place). `reason` is `notPlain`, `tooLarge`, `behindLink`, `notEmpty` or `failed`; `code` is set only for `failed` | The fixture `kept-reasons.json` lists the reasons |

**Conventions:**
- **Field names** are camelCase on the wire (`#[serde(rename_all = "camelCase")]`).
- **Optional values** are `null`, never omitted.
- **No aria2 terms:** no aria2 shapes or option names appear in any of these types.

## The contract version

`COMMAND_API_VERSION` exists in both `velocity_core` and `src/contract.ts`, and `app_info` reports the native side's version. At startup the UI validates the response. **If the versions differ, the window refuses to start** and explains that the two parts don't match, rather than misreading messages. Raise the version, on both sides in the same PR, whenever a command or shared type changes incompatibly.

## The downloads commands

The commands of [`src-tauri/src/downloads.rs`](../src-tauri/src/downloads.rs) (#58), V1's direct-download commands. A command that fails returns its `ErrorCode`. Commands that take `ids` accept at most 10,000 ids, each at most 64 characters, and return a `BatchResult`.

| Command | Arguments | Returns |
|---|---|---|
| `get_snapshot` | — | `DownloadsSnapshot` |
| `pause_downloads`, `resume_downloads`, `cancel_downloads`, `retry_downloads` | `ids` | `BatchResult` |
| `remove_downloads` | `ids`, `deleteFiles` | `BatchResult`, with the files kept |
| `retry_failed`, `pause_all`, `resume_all` | — | nothing |
| `clear_completed` | — | how many were cleared |
| `move_download` | `id`, `direction` (`top`, `up`, `down`, `bottom`) | nothing |
| `set_download_connections` | `id`, `connections` | the connections that apply |
| `set_download_speed_limit` | `id`, `kib` (KiB/s; 0 for no limit, otherwise at least 1) | nothing |
| `restart_engine` | — | `EngineHealth` |
| `trust_host_key` | `host`, `port`, `fingerprint` | nothing; the UI then retries the download |
| `choose_torrent_files` | `id`, `files` (indexes) | nothing |
| `set_torrent_seeding` | `id`, `seeding` (a seeding policy; `{ "mode": "never" }` stops seeding) | nothing |
| `restart_torrent_helper` | — | `EngineHealth` |

Adding downloads is #60's, and giving a login back to a download that needs it comes with its dialog (#59, #60).

## The category commands

The commands of [`src-tauri/src/categories.rs`](../src-tauri/src/categories.rs) (#31), V1's category commands and `check_destination`. A command that fails returns its `ErrorCode`. The rules are in [FILE_NAMING.md](FILE_NAMING.md).

| Command | Arguments | Returns |
|---|---|---|
| `list_categories` | — | `Category[]`, in the order the automatic category is chosen in |
| `create_category` | `input` (a `CategoryInput`) | the `Category`; `INVALID_INPUT`, `INVALID_PATH` or `NAME_TAKEN` |
| `update_category` | `id`, `input` | the `Category`; also `NOT_FOUND` |
| `delete_category` | `id` | nothing; `NOT_FOUND`, or `INVALID_STATE` for a built-in one. Its downloads are left without one |
| `set_download_category` | `id`, `categoryId` (`null` for none) | nothing; nothing is moved |
| `check_destination` | `url`, `directory`, `fileName`, `categoryId` (each `null` for automatic) | `DestinationCheck` |

## How the two sides stay in step

1. **Rust generates contract fixtures** in `tests/fixtures/contract/` from the real types: `statuses.json`, `error-codes.json`, `download-record.json`, `capabilities.json`, `app-info.json`, `downloads-snapshot.json`, `engine-health.json`, `batch-result.json`, `kept-reasons.json`, `category.json`, `category-icons.json`, `destination-check.json` and `on-conflict.json`. `naming-cases.json` holds the names `src/naming.ts` must give ([FILE_NAMING.md](FILE_NAMING.md#file-names)). A Rust test fails when a fixture no longer matches the types: "fixture … is out of date; regenerate it with `VELOCITY_UPDATE_FIXTURES=1`".
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
