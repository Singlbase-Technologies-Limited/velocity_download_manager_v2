# The UI contract

The UI (`src/`) and the native side (`src-tauri/`, `crates/`) talk only through typed Tauri commands and events. This page describes the shared types, how they're kept in step, and how to change them (#25).

## The shared types

They're defined once in Rust, in `crates/velocity-core/src/domain.rs` (plus `AppInfo` in `lib.rs`), and mirrored by strict Zod schemas in `src/contract.ts`.

| Type | What it is | Notes |
|---|---|---|
| `DownloadStatus` | Where a download is: `queued`, `connecting`, `downloading`, `paused`, `verifying`, `retrying`, `stalled`, `completed`, `failed`, `removed`, `fetchingMetadata`, `choosingFiles`, `seeding`, `unknown` | V1's fourteen values. **A new status needs an ADR**, so the UI stays consistent. `completed`, `failed` and `removed` are terminal |
| `DownloadKind` | `http`, `ftp`, `sftp`, `metalink`, `torrent`, `magnet` | — |
| `DownloadRecord` | One download as the UI sees it: a stable app `id`, kind, source URL (credentials removed), file name, folder, category, status, progress, error, `verification`, timestamps (milliseconds since the Unix epoch), `retryAtMs`: while `retrying`, when the engine tries again, `startedAtMs`: the first time the engine reported it running, and `averageSpeedBps`: the bytes per second it received while it ran, `null` until it ran for a second | Never carries an engine's own id. Since contract version 12 (#59): `retryAtMs` comes only from the engine, is `null` in every other status, and isn't saved; `startedAtMs` and `averageSpeedBps` are the manager's, and survive a restart ([DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md#schema)). Time waiting, paused, or before the engine's first report doesn't count, nor do bytes a download resumed with |
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
| `OnConflict` | What adding does when the name is taken (#31): `ask` or `rename`. The fixture `on-conflict.json` lists them | Never replace (PO decision on #31). Used by `add_downloads` |
| `AddRequest` | What the Add dialog asks for (#60): `links` (direct and magnet links, and links to `.torrent` files), `torrentFiles` (absolute paths), `directory`, `fileName` (one direct link only), `categoryId` (each `null` for automatic), `createDirectory`, `onConflict`, `allowDuplicate` ("Add anyway"), `startPaused`, `connections` (`null` for the performance profile's), `speedLimitKib` (0 for none), `headers`, `referrer`, `credentials` (`{ userName, password }`), `sshKey` (`{ path, passphrase }`, for `sftp://` links), `checksum` (`{ algorithm, hex }`), `requireEncryption` (for `ftp://` links) and `chooseFiles` (torrents wait in Choosing files) | Since contract version 11. The fixture `add-request.json` sets every field. The login, headers and passphrase stay in memory |
| `DownloadLogin` | What `set_download_login` gives a download again (#59): `credentials` (`{ userName, password }` or `null`), `sshKey` (`{ path, passphrase }` or `null`) and `headers`. The fixture `download-login.json` sets every part | Since contract version 12. Kept in memory only, never saved, and redacted in diagnostics |
| `AddResult` | `{ source, outcome }` for each link and `.torrent` file, in the request's order. `outcome.kind` is `added` (`id`), `conflict` (`check`, a `DestinationCheck`), `duplicate` (`duplicate`: `existingId`, `name`, `status`, `inList`, `canAddAnyway`), `failed` (`code`) or `notFetched` (`error`, a `TransferError`: a `.torrent` link couldn't be fetched) | Since contract version 11. The fixture `add-results.json` has one of each |
| `ShellSettings` | What the window starts with (#29): `appearance` (`theme`, `density`, `layout`, and the download list's `visibleColumns` and `columnWidths`), `onboardingCompleted`, and `defaultDirectory`, the folder downloads go to now (`null` when the system names none). The fixtures `themes.json` and `densities.json` list the themes and densities, and `columns.json` the column ids, the default columns and the width bounds | Since contract version 10; the columns since version 12 (#59). The settings are in [SETTINGS.md](SETTINGS.md#the-window). `set_appearance` takes and returns an `AppearanceSettings` |
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
| `set_download_login` | `id`, `login` (a `DownloadLogin`) | nothing; `INVALID_STATE` for a download that doesn't need its login, and the Add path's codes for an invalid one. The window then resumes or retries it (#59) |
| `set_download_connections` | `id`, `connections` | the connections that apply |
| `set_download_speed_limit` | `id`, `kib` (KiB/s; 0 for no limit, otherwise at least 1) | nothing |
| `restart_engine` | — | `EngineHealth` |
| `trust_host_key` | `host`, `port`, `fingerprint` | nothing; the UI then retries the download |
| `choose_torrent_files` | `id`, `files` (indexes) | nothing |
| `set_torrent_seeding` | `id`, `seeding` (a seeding policy; `{ "mode": "never" }` stops seeding) | nothing |
| `restart_torrent_helper` | — | `EngineHealth` |

Adding downloads is #60's. A download in the snapshot's `needsLogin` came back after a restart without its login, custom headers or SSH key, which are never saved; `set_download_login` gives them again, checked as when it was added, and refused whole if one isn't valid (PO decision on #59).

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

## The window's commands

The commands of [`src-tauri/src/shell.rs`](../src-tauri/src/shell.rs) (#29). A command that fails returns its `ErrorCode`. How the window uses them is in [UI_ARCHITECTURE.md](UI_ARCHITECTURE.md).

| Command | Arguments | Returns |
|---|---|---|
| `get_shell_settings` | — | `ShellSettings`; `DATABASE` when a section can't be read |
| `set_appearance` | `appearance` | the `AppearanceSettings` saved; `INVALID_INPUT` for a panel or column width outside its bounds, or a column list without `name` or with a column twice |
| `complete_onboarding` | `directory`: an existing folder, not a link | `ShellSettings`; `INVALID_PATH`, or `DESTINATION_MISSING` when it doesn't exist |
| `choose_directory` | `initial` (`null`, or a folder to start in) | the folder chosen with the system's chooser, or `null` when the user cancels |
| `quit_app` | — | nothing: the app quits. The window calls it only after the user confirms |

**The `quit-requested` event** carries `{ active }`: quitting would stop that many downloads, and `general.confirmExitWithActive` is on. The window asks the user, and calls `quit_app` only if they confirm.

## Adding downloads

The commands of [`src-tauri/src/adding.rs`](../src-tauri/src/adding.rs) (#60). A command that fails returns its `ErrorCode`. The rules are in the module docs of `velocity_core::manager::adding` and `torrent_links`.

| Command | Arguments | Returns |
|---|---|---|
| `add_downloads` | `request`: an `AddRequest` | an `AddResult` for each link and file, in order. The whole request is refused with `INVALID_INPUT`, adding nothing, when it is empty, has more than 500 links and files, names a file for more than one, or has a header, referrer, login, checksum or speed limit outside its rules. `.torrent` links are fetched first, for up to 60 seconds |
| `get_capabilities` | — | the transfer engine's `Capabilities`, so the dialog offers only what it can do |
| `choose_torrents` | — | the `.torrent` files chosen with the system's chooser; empty when the user cancels |
| `choose_ssh_key` | — | the SSH private key chosen with the system's chooser, starting in `~/.ssh`; `null` when the user cancels |

**What each link gets:**
- **Duplicates (PO decision on #60, V1's rules):** a direct link is the same download when its link is, as typed; a torrent when it shares an info hash, from a magnet link or a `.torrent` file. A torrent still in the list is `duplicate` with `canAddAnyway: false`, even with `allowDuplicate`, because two copies can't run. Anything else in the list or the history is `duplicate` until `allowDuplicate` is set.
- **Taken names:** `conflict` with `onConflict: ask`, or the first free name with `rename`. A torrent's names come from its metadata, so a taken one is always `conflict`, with no `suggestedName`.
- **The chosen folder** is created only with `createDirectory`, one level at a time and never through a link (`INVALID_PATH`).
- **A torrent's category** comes from its largest file, or from a magnet link's display name.
- **`.torrent` links** (an `http` or `https` link whose path ends in `.torrent`, as V1 told them apart) are fetched through the transfer engine into the app's `torrent-links` folder, at most 16 MiB, then added as the file. The file is kept without its trackers, which stay in memory as every torrent's do. A fetch that fails is `notFetched` with the server's or network's reason; one larger than 16 MiB, or that isn't a torrent, is `failed` with `INVALID_INPUT`.

**The `torrents-dropped` event** carries `{ paths, ignored }`: the `.torrent` files dropped on the window (plain files, never through a link, at most 500), and how many other things were dropped. The window opens the Add dialog with the files, or says only `.torrent` files can be dropped.

**The SFTP host key** has no command of its own here: an SFTP download stops before logging in with `error.hostKey` set (#53). The window then asks, and "Trust and continue" calls `trust_host_key` and then `retry_downloads`.

## How the two sides stay in step

1. **Rust generates contract fixtures** in `tests/fixtures/contract/` from the real types: `statuses.json`, `error-codes.json`, `download-record.json`, `capabilities.json`, `app-info.json`, `downloads-snapshot.json`, `engine-health.json`, `batch-result.json`, `kept-reasons.json`, `category.json`, `category-icons.json`, `destination-check.json`, `on-conflict.json`, `shell-settings.json`, `themes.json`, `densities.json`, `add-request.json` and `add-results.json`. `naming-cases.json` holds the names `src/naming.ts` must give ([FILE_NAMING.md](FILE_NAMING.md#file-names)). A Rust test fails when a fixture no longer matches the types: "fixture … is out of date; regenerate it with `VELOCITY_UPDATE_FIXTURES=1`".
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
