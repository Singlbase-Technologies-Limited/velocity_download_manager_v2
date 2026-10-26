# The `DirectTransfer` engine contract

The manager talks to a direct-download engine (HTTP, FTP, SFTP, Metalink) only through the `DirectTransfer` contract in [`crates/velocity-core/src/transfer.rs`](../crates/velocity-core/src/transfer.rs) (#36, [ADR-0002](decisions/0002-engine-crate-layout.md)). The manager never sees libcurl types, engine-internal ids or aria2 vocabulary. Torrent operations (trackers, peers, file priorities, seeding, magnet metadata) are not part of this contract; they belong to the torrent bridge (research report section 4.2).

Contract version: `TRANSFER_CONTRACT_VERSION = 11`. Raise it with any incompatible change to a command, reply, event or type. Version 2 added checkpoints (#43), version 3 the global speed limit (#49), version 4 the connection detail in snapshots (#47), version 5 the checkpoint's `contentEncoding` (#45), version 6 integrity (#46): checked checksums with SHA-224 and SHA-384, `pieces` in the spec, and `verification` in snapshots, version 7 the performance profile (#57): `set_engine_limits`, and capabilities per protocol, and version 8 HTTP details (#40): `directory` and an optional `fileName` in place of `destination`, `credentials` in place of the credential handle, `referrer` and `userAgent` in the spec, and `fileName` in snapshots, version 9 FTP and FTPS (#51): `requireEncryption` in the spec and `encryption` in each protocol's capabilities, version 10 SFTP (#53): `sshKey` in the spec, `hostKey` in errors, and `trust_host_key`, and version 11 Metalink (#54): `contentType` in snapshots, and engines may report `metalink`.

## Commands

| Method | Allowed when the transfer is | Result |
|---|---|---|
| `capabilities()` | always | What the engine really supports: each protocol with whether it is segmented, resumes, checks validators, uses HTTP/2 and can be encrypted; live connection changes; Metalink; and the connection cap per host in force |
| `add(TransferSpec)` | — | A new `TransferId`. The transfer starts as Queued |
| `pause(id)` | Queued, Connecting, Downloading, Retrying, Stalled or Verifying | Paused, progress kept |
| `resume(id)` | Paused | Queued again, continuing from durable progress |
| `cancel(id)` | unfinished (any of the above, or Paused) | Removed. The part file is discarded; the destination file is never touched |
| `remove(id)` | finished: Completed, Failed or Removed | The engine forgets the transfer and its checkpoint, and sends `Removed`. Files are kept |
| `set_limits(id, Limits)` | known transfer | The transfer's own speed limit (`None` = unlimited), applied while it runs. The lower of it and the global limit applies |
| `set_global_limits(Limits)` | always | The limit all transfers share (`None` = unlimited), applied while they run. Its allowance goes to whichever transfers are active |
| `set_engine_limits(EngineLimits)` | always | The performance profile's limits (#57, [SETTINGS.md](SETTINGS.md#performance)): `maxConnectionsPerHost` (1 to 16) and `minSegmentBytes` (1 MiB to 1 GiB), `InvalidInput` outside them. A lower cap holds running transfers back at once (`perHostCap`); the smallest piece applies to transfers that start after it. The engine starts at Balanced's 8 and 1 MiB, and the supervisor gives a restarted engine the limits its predecessor had |
| `set_connections(id, n)` | known transfer | The count actually applied: at least 1, at most `max_connections_per_host`. It is the most the transfer may use, applied while it runs without a restart, or when a paused one resumes. Within it, the engine shares connections between transfers and ramps up while each helps; `connectionDetail` says what holds a transfer below it ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#connections-47)). FTP and SFTP transfers apply 1 |
| `trust_host_key(host, port, fingerprint)` | always | Pins the SFTP host key the engine last saw from `host` and `port` with that SHA-256 fingerprint, as a transfer's `hostKey` showed it (#53). `NotFound` when it saw no such key there; `InvalidState` when another key is pinned, since a changed key is never trusted. The caller then retries the transfer ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#host-keys)) |
| `snapshot()` | always | The state of every transfer the engine knows |
| `subscribe()` | always | A receiver that gets every event from then on |

A limit below 1 KiB/s (`Limits::MIN_BPS`), in a spec or a command, is refused with `InvalidInput`. Commands fail with an `ErrorCode` ([CONTRACTS.md](CONTRACTS.md)): `NotFound` for an unknown id, `InvalidState` when the status doesn't allow the command, `EngineUnavailable` when the engine can't be reached. **A failed download is not a failed command:** it arrives as an event whose snapshot has status Failed and a `TransferError { class, httpStatus, message, retryable }`.

## Types

- **`TransferSpec`:** URLs (primary first, then mirrors; never empty, with no login in them), the folder and an optional file name, a login, custom headers, a referrer, a user agent, an optional trusted checksum, optional trusted piece hashes, limits, and the requested connection count. `TransferSpec::check_request` checks what a user or the browser may set, and the engine runs it on `add` (#40):
  - **`directory` and `fileName`:** the file goes in `directory`. Without a `fileName`, the server names it ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#file-names-40)). A given name must be one safe component (`naming::check_file_name`, else `InvalidFilename`), and is refused with `FileExists` when taken; #31 adds the conflict settings. A spec with a `resume` must give the name (`InvalidInput`).
  - **`credentials`:** `{ userName, password }`, sent as HTTP Basic only to the first URL's origin, or as FTP's `USER` and `PASS`. The user name has no colon, and neither part has a control character or exceeds 1 KiB. A URL with a login in it is refused with `InvalidUrl`; `request::split_login` moves it out.
  - **`sshKey`:** for an `sftp://` link, `{ path, passphrase }`: log in with this private key, as the user of `credentials`. The path is absolute; the passphrase follows the credentials' rules and travels like a password. Refused with `InvalidInput` for any other link or without `credentials` ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#sftp-53)).
  - **`requireEncryption`:** for an `ftp://` link, insist on TLS (`AUTH TLS`) for the login and the file, and fail if the server can't, with no fallback to plain FTP (PO decision on #51). `https://` and `ftps://` are always encrypted. With an `http://` link it is refused with `InvalidInput` ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#logins-and-encryption)).
  - **`headers`:** at most 32, each an RFC 9110 token name with a value of at most 8 KiB without control characters but tab. Headers the engine manages itself are refused: `Host`, `Content-Length`, `Range`, `If-Range`, `Accept-Encoding`, `Connection`, `Transfer-Encoding`, `TE`, `Upgrade`, `Expect`, `Keep-Alive`, `Proxy-Connection`, `Proxy-Authorization`, `Referer` and `User-Agent`. They go only to the first URL's origin.
  - **`referrer`:** an `http` or `https` URL without a login, at most 8 KiB, sent as `Referer` with every request.
  - **`userAgent`:** at most 256 bytes without control characters, sent with every request. Without one, `VelocityDownloadManager/<version>` (`request::default_user_agent`, PO decision on #40).
  - **`checksum`:** `{ algorithm, hex }`, with `md5`, `sha1`, `sha224`, `sha256`, `sha384` or `sha512`, the six V1 accepts. A digest that isn't hexadecimal, or doesn't have its algorithm's length (32, 40, 56, 64, 96 or 128 characters), is refused with `InvalidInput`; uppercase is accepted. The whole file is checked before it is renamed into place.
  - **`pieces`:** `{ algorithm, length, hex }`, one digest per `length` bytes, the last piece ending with the file, such as a Metalink file's `<pieces>` (#54 fills it). `InvalidInput` for a bad digest, pieces under 1 KiB (`PieceHashes::MIN_LENGTH`), or no pieces, or more digests than 2 MiB of hex (`MAX_HEX_BYTES`), so that a spec always fits in one message to the engine's child process: 32,768 SHA-256 digests, 32 GiB of file in 1 MiB pieces. Each piece is checked once covered, and a bad one is fetched again.
- **`TransferSnapshot`:** the id, a `DownloadStatus`, `Progress` (accepted bytes only; the connections in use now, and `connectionDetail` while a direct transfer runs), `wasted_bytes`, the last `TransferError`, `verification`: what was proven about the bytes once the transfer completed, `null` until then ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#what-was-proven)), `fileName`: the name the file is saved under, `null` until the server named it, and `contentType`: the file's media type as the server gave it (`Content-Type` without its parameters, in lowercase), `null` for FTP and SFTP and when the server gave none or an invalid one. The manager uses it to know a link to a Metalink file ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#metalink-54)).
- **`TransferEvent`:** `changed { snapshot }` or `removed { id }`.

A direct engine reports only these statuses: Queued, Connecting, Downloading, Retrying, Stalled, Paused, Verifying, Completed, Failed and Removed. "Cancelled" in the research report is Removed here.

- **Retrying:** the transfer waits before trying again (#48). When the engine starts a failed download again, `error` holds the failure it waits after, with "Retrying automatically (n of 2)." The error is cleared once bytes come again.
- **Stalled:** no byte has come from the network for a while. The transfer isn't failed, and goes back to Downloading when bytes come.
- **Verifying:** every byte is in; the engine is finishing the whole-file hash before the file is renamed (#46).
- **A failed check** (a wrong checksum, a piece still bad after it was fetched again) ends as Failed with a message. The part file is kept for inspection and isn't renamed, the engine doesn't start it again on its own, and its last checkpoint trusts no byte, so a resume from it downloads the whole file again ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#a-failed-check)).

## Checkpoints and resuming (#43)

- **`TransferEvent::Checkpoint { id, checkpoint }`:** the engine synced the transfer's part file, and the checkpoint's durable byte ranges are on disk. The app saves it.
- **`TransferSpec::resume`:** how to treat what an earlier attempt left on disk.
  - `None`: a new download.
  - `Resume::Checkpoint { checkpoint }`: continue from a saved checkpoint.
  - `Resume::DiscardPartFile {}`: the part file is the download's own but has no usable checkpoint; delete it and start over.
- **`Checkpoint`:**
  - `format`;
  - `source`, the URL without login, query or fragment (`Checkpoint::source_of`);
  - `total`;
  - `etag`, as the first answer sent it, strong or weak;
  - `lastModified`, only when it was the strong validator;
  - `contentEncoding`, the encoding the server applied, or none. Checkpoints saved before version 5 lack it and read as none;
  - `durable`, half-open byte ranges.

  No credential and no URL query is ever in it. The engine checks a returned checkpoint like any other input: one that doesn't fit its file isn't trusted.
- **`pause` and `resume`** work since #43: a paused transfer keeps its part file and its last checkpoint. The rules are in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#persistence-and-recovery-43).

## Secrets

- **Logins** cross the contract in memory, because the engine must send them (PO decision on #40). To an engine in a child process they go only through its pipe, never its arguments or environment ([ADR-0012](decisions/0012-engine-child-process.md)). They never reach a checkpoint, the database or a log. `Debug` output of `Credentials`, an `SshKey`, a `TransferSpec` or the libcurl layer's `RequestOptions` shows `<redacted>` for every secret (tested). The keychain and the secret-leak scan are #70's and #71's.
- **Header values** may be secrets (cookies, tokens). They cross the contract the same way, and `Debug` output of a `Header`, a `TransferSpec` or `RequestOptions` never shows them (tested).
- **Only to the first origin:** the login and the custom headers go only to the first URL's origin (scheme, host and port, as libcurl reads the URL). Once a redirect leaves it, no request carries them ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#redirects-40)).

## Across a process boundary

In production the engine runs in a child process ([ADR-0012](decisions/0012-engine-child-process.md)); tests and the CLI also run it in the app's process. Every value in the contract is owned and serialisable, and the trait has no callbacks, references or lifetimes, so a client for a child-process engine can implement it too. `TransferCommand` and `TransferReply` are the wire forms of the commands. `dispatch(engine, command)` runs one command against any engine; a child process runs it on each message it receives.

How these wire forms cross the child's stdin and stdout is described in [ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md): the framing, the version handshake, the size limit and the supervisor (#39).

Every wire form refuses a field it doesn't know, at every level. Commands and replies without data are struct variants with no fields, such as `TransferCommand::Snapshot {}`, because serde ignores extra fields on a unit variant of a tagged enum. Their JSON is unchanged: `{"command":"snapshot"}`.

Wire names are camelCase and tagged, for example:

```json
{"command":"setConnections","id":3,"connections":2}
{"type":"changed","snapshot":{"id":1,"status":"downloading","progress":{...},"wastedBytes":0,"error":null}}
```

Tests check that every command, reply and event round-trips through JSON, and that `Box<dyn DirectTransfer>` compiles and is `Send`.

## The manager's side

[`crates/velocity-core/src/manager/`](../crates/velocity-core/src/manager/mod.rs) is the manager. It:

- gives each download its own app id (`d-0001`), never the engine's id;
- refuses torrent kinds and protocols missing from `capabilities()` with `Unsupported`, before calling the engine;
- checks the record's status before sending a command, so a forbidden command never reaches the engine;
- changes the status of a download the engine runs only when the engine reports it, never optimistically. A download the engine isn't running is Queued while it waits for a slot or the schedule, and Paused when the user paused it;
- owns the queue (#50): it gives the engine a download only when a slot is free, at most `QueueSettings::max_concurrent_downloads` at once, in queue order (`move_download`), and runs scheduled downloads only while the window is open (`tick` reads the local time). `pause_all`, `resume_all` and new settings apply at once;
- treats engine events as untrusted: it ignores events for unknown or forgotten transfers, and shows a torrent-only status from a direct engine as `Unknown`.

### Saving and starting again (#58)

The manager is opened over the database (`Manager::open`), and every change it makes is saved there ([DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md)). The code is in `manager/persist.rs` and `manager/recovery.rs`.

- **What is saved:**
  - a new download, with what it needs to start again: every source without its login, the user agent, the referrer without its query and fragment, the checksum and piece hashes, the speed limit and the connections. No secret is saved;
  - its status, name, byte counts, error, verification and completion time after each event. Progress alone is saved at most every 5 s (`PROGRESS_SAVE_MS`); everything else at once;
  - the queue, whenever it changed;
  - each checkpoint event. The checkpoint is dropped once the download completes or is removed. A failed download keeps its checkpoint, for a retry.
- **A failed save** doesn't undo what happened: the download did pause or complete. The failure is kept for the app to show (`storage_error`, in the snapshot).
- **When the app starts:**
  - finished downloads come back as they ended;
  - unfinished ones wait in their saved order, a paused one still paused. Each resumes from its checkpoint when it fits, or starts over by discarding its part file (`resume_from`, PO decision on #43). Nothing starts in `open`: the app applies its queue settings first;
  - **a download that had a login, custom headers or an SSH key comes back Paused** with a note. It resumes or is retried only after `set_login` gives them again (PO decision on #58);
  - a record this build can't read, or a torrent record until the manager runs torrents, is listed by `unavailable()` and never acted on;
  - downloads removed from the list stay in the history only (#32), and aren't loaded;
  - ids continue after the highest one stored, so none is given twice.

### Retrying, removing and clearing (#58)

In `manager/finished.rs`:

- **`retry`** takes a failed or removed download back to the end of the queue. A failed one resumes from its checkpoint; a removed one starts over, because cancelling discarded its part file. The engine already retries a retryable failure by itself (#48), so these are the user's retries only. `retry_failed` retries every failed download.
- **`remove`** takes a download out of the list; it stays in the history. An unfinished one is cancelled first, which discards its part file.
- **With its files** (PO decision on #58), only the download's own go: its part file, and a completed download's file if it is a plain file inside its folder, not reached through a link, and no larger than the download. Anything else is kept and returned with the reason. The rules are the ones torrents use (`torrent_files::delete_owned`).
- **`clear_completed`** removes every completed download from the list and keeps its file.
- **A download's own speed limit and connections** (`set_download_limits`, `set_download_connections`) apply at once to a download the engine has, and to the next start of one it doesn't. They are saved.

### The engine stopping (#58)

- **A lost download comes back on its own:**
  - When the engine crashes, hangs or is restarted by hand, the supervisor reports what it was running as failed, with `ENGINE_STOPPED`, `ENGINE_HUNG` or `ENGINE_RESTARTED` (#39).
  - The manager recognises these (`lost_by_engine`) and puts the download back to waiting, with the reason shown.
  - It then resumes from its checkpoint once an engine runs (PO decision on #58).
- **While the engine is unavailable,** a waiting download it refuses with `ENGINE_UNAVAILABLE` stays waiting instead of failing.
- **Health and restarting:** `DirectTransfer::health` and `restart` report the engine's health and restart it ("Restart transfer engine"). An engine in the app's process is always running, and `restart` is `Unsupported` for it.
- **A missing engine program:** when the program can't start, the app runs with `MissingEngine`. It refuses every command as `ENGINE_UNAVAILABLE` and reports why as its health, so the app opens and says what is wrong.

### In the app (#58)

`ManagerService` runs the manager on a thread of its own:

- **Commands** reach it through a bounded channel. No lock is held while it talks to the engine or the database.
- **Every 100 ms** it applies the engine's events and the schedule.
- **Snapshots:** when the `DownloadsSnapshot` changed, it sends it, at most every 250 ms (V1 refreshed every 250 ms to 5 s).
- **The desktop shell** (`src-tauri/src/downloads.rs`) starts it, sends each snapshot as the `downloads-snapshot` event, and exposes V1's direct-download commands. They are listed in [CONTRACTS.md](CONTRACTS.md#the-downloads-commands).

Saving the queue settings is #30.

## The fake engine for tests

`transfer::fake::FakeTransfer` is compiled only for tests. It follows the command rules above, and does nothing else on its own. A test keeps a `FakeControl` to script progress, failures, completion, stray events and an unavailable engine, and to read the log of commands the engine received.
