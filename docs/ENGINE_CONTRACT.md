# The `DirectTransfer` engine contract

The manager talks to a direct-download engine (HTTP, FTP, SFTP, Metalink) only through the `DirectTransfer` contract in [`crates/velocity-core/src/transfer.rs`](../crates/velocity-core/src/transfer.rs) (#36, [ADR-0002](decisions/0002-engine-crate-layout.md)). The manager never sees libcurl types, engine-internal ids or aria2 vocabulary. Torrent operations (trackers, peers, file priorities, seeding, magnet metadata) are not part of this contract; they belong to the torrent bridge (research report section 4.2).

Contract version: `TRANSFER_CONTRACT_VERSION = 4`. Raise it with any incompatible change to a command, reply, event or type. Version 2 added checkpoints (#43), version 3 the global speed limit (#49), and version 4 the connection detail in snapshots (#47).

## Commands

| Method | Allowed when the transfer is | Result |
|---|---|---|
| `capabilities()` | always | What the engine really supports: protocols, live connection changes, validator-checked resume, Metalink, the connection cap per host |
| `add(TransferSpec)` | — | A new `TransferId`. The transfer starts as Queued |
| `pause(id)` | Queued, Connecting, Downloading, Retrying, Stalled or Verifying | Paused, progress kept |
| `resume(id)` | Paused | Queued again, continuing from durable progress |
| `cancel(id)` | unfinished (any of the above, or Paused) | Removed. The part file is discarded; the destination file is never touched |
| `remove(id)` | finished: Completed, Failed or Removed | The engine forgets the transfer and its checkpoint, and sends `Removed`. Files are kept |
| `set_limits(id, Limits)` | known transfer | The transfer's own speed limit (`None` = unlimited), applied while it runs. The lower of it and the global limit applies |
| `set_global_limits(Limits)` | always | The limit all transfers share (`None` = unlimited), applied while they run. Its allowance goes to whichever transfers are active |
| `set_connections(id, n)` | known transfer | The count actually applied: at least 1, at most `max_connections_per_host`. It is the most the transfer may use, applied while it runs without a restart, or when a paused one resumes. Within it, the engine shares connections between transfers and ramps up while each helps; `connectionDetail` says what holds a transfer below it ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#connections-47)) |
| `snapshot()` | always | The state of every transfer the engine knows |
| `subscribe()` | always | A receiver that gets every event from then on |

A limit below 1 KiB/s (`Limits::MIN_BPS`), in a spec or a command, is refused with `InvalidInput`. Commands fail with an `ErrorCode` ([CONTRACTS.md](CONTRACTS.md)): `NotFound` for an unknown id, `InvalidState` when the status doesn't allow the command, `EngineUnavailable` when the engine can't be reached. **A failed download is not a failed command:** it arrives as an event whose snapshot has status Failed and a `TransferError { class, httpStatus, message, retryable }`.

## Types

- **`TransferSpec`:** URLs (primary first, then mirrors; never empty, credentials removed), the final destination path, a credential handle, custom headers, an optional trusted checksum (MD5, SHA-1, SHA-256 or SHA-512), limits, and the requested connection count.
- **`TransferSnapshot`:** the id, a `DownloadStatus`, `Progress` (accepted bytes only; the connections in use now, and `connectionDetail` while a direct transfer runs), `wasted_bytes` and the last `TransferError`.
- **`TransferEvent`:** `changed { snapshot }` or `removed { id }`.

A direct engine reports only these statuses: Queued, Connecting, Downloading, Retrying, Stalled, Paused, Verifying, Completed, Failed and Removed. "Cancelled" in the research report is Removed here.

- **Retrying:** the transfer waits before trying again (#48). When the engine starts a failed download again, `error` holds the failure it waits after, with "Retrying automatically (n of 2)." The error is cleared once bytes come again.
- **Stalled:** no byte has come from the network for a while. The transfer isn't failed, and goes back to Downloading when bytes come.

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
  - `etag` and `lastModified`;
  - `durable`, half-open byte ranges.

  No credential and no URL query is ever in it. The engine checks a returned checkpoint like any other input: one that doesn't fit its file isn't trusted.
- **`pause` and `resume`** work since #43: a paused transfer keeps its part file and its last checkpoint. The rules are in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#persistence-and-recovery-43).

## Secrets

- **Credentials** cross the contract only as a `CredentialHandle`, an opaque number. The app keeps the user name and password in memory (#70), so they can't reach a checkpoint, a log or a message.
- **Header values** may be secrets (cookies, tokens). They cross the contract in memory, because the engine must send them, but `Debug` output of a `Header` or `TransferSpec` always shows `<redacted>` (tested).

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

[`crates/velocity-core/src/manager.rs`](../crates/velocity-core/src/manager.rs) is the first slice of the manager. It:

- gives each download its own app id (`d-0001`), never the engine's id;
- refuses torrent kinds and protocols missing from `capabilities()` with `Unsupported`, before calling the engine;
- checks the record's status before sending a command, so a forbidden command never reaches the engine;
- changes a record's status only when the engine reports it, never optimistically;
- treats engine events as untrusted: it ignores events for unknown or forgotten transfers, and shows a torrent-only status from a direct engine as `Unknown`.

Persistence (#26), the queue and settings (#30) build on it later.

## The fake engine for tests

`transfer::fake::FakeTransfer` is compiled only for tests. It follows the command rules above, and does nothing else on its own. A test keeps a `FakeControl` to script progress, failures, completion, stray events and an unavailable engine, and to read the log of commands the engine received.
