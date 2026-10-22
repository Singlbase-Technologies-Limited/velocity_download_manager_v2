# The `DirectTransfer` engine contract

The manager talks to a direct-download engine (HTTP, FTP, SFTP, Metalink) only through the `DirectTransfer` contract in [`crates/velocity-core/src/transfer.rs`](../crates/velocity-core/src/transfer.rs) (#36, [ADR-0002](decisions/0002-engine-crate-layout.md)). The manager never sees libcurl types, engine-internal ids or aria2 vocabulary. Torrent operations (trackers, peers, file priorities, seeding, magnet metadata) are not part of this contract; they belong to the torrent bridge (research report section 4.2).

Contract version: `TRANSFER_CONTRACT_VERSION = 1`. Raise it with any incompatible change to a command, reply, event or type.

## Commands

| Method | Allowed when the transfer is | Result |
|---|---|---|
| `capabilities()` | always | What the engine really supports: protocols, live connection changes, validator-checked resume, Metalink, the connection cap per host |
| `add(TransferSpec)` | — | A new `TransferId`. The transfer starts as Queued |
| `pause(id)` | Queued, Connecting, Downloading, Retrying, Stalled or Verifying | Paused, progress kept |
| `resume(id)` | Paused | Queued again, continuing from durable progress |
| `cancel(id)` | unfinished (any of the above, or Paused) | Removed. The part file is discarded; the destination file is never touched |
| `remove(id)` | finished: Completed, Failed or Removed | The engine forgets the transfer and its checkpoint, and sends `Removed`. Files are kept |
| `set_limits(id, Limits)` | known transfer | The speed limit (`None` = unlimited) |
| `set_connections(id, n)` | known transfer | The count actually applied: at least 1, at most `max_connections_per_host` |
| `snapshot()` | always | The state of every transfer the engine knows |
| `subscribe()` | always | A receiver that gets every event from then on |

Commands fail with an `ErrorCode` ([CONTRACTS.md](CONTRACTS.md)): `NotFound` for an unknown id, `InvalidState` when the status doesn't allow the command, `EngineUnavailable` when the engine can't be reached. **A failed download is not a failed command:** it arrives as an event whose snapshot has status Failed and a `TransferError { class, httpStatus, message, retryable }`.

## Types

- **`TransferSpec`:** URLs (primary first, then mirrors; never empty, credentials removed), the final destination path, a credential handle, custom headers, an optional trusted checksum (MD5, SHA-1, SHA-256 or SHA-512), limits, and the requested connection count.
- **`TransferSnapshot`:** the id, a `DownloadStatus`, `Progress` (accepted bytes only), `wasted_bytes` and the last `TransferError`.
- **`TransferEvent`:** `changed { snapshot }` or `removed { id }`.

A direct engine reports only these statuses: Queued, Connecting, Downloading, Retrying, Stalled, Paused, Verifying, Completed, Failed and Removed. "Cancelled" in the research report is Removed here.

## Secrets

- **Credentials** cross the contract only as a `CredentialHandle`, an opaque number. The app keeps the user name and password in memory (#70), so they can't reach a checkpoint, a log or a message.
- **Header values** may be secrets (cookies, tokens). They cross the contract in memory, because the engine must send them, but `Debug` output of a `Header` or `TransferSpec` always shows `<redacted>` (tested).

## Across a process boundary

In production the engine runs in a child process ([ADR-0012](decisions/0012-engine-child-process.md)); tests and the CLI also run it in the app's process. Every value in the contract is owned and serialisable, and the trait has no callbacks, references or lifetimes, so a client for a child-process engine can implement it too. `TransferCommand` and `TransferReply` are the wire forms of the commands. `dispatch(engine, command)` runs one command against any engine; a child process runs it on each message it receives.

The #37 prototype's framing is JSON lines over the child's stdin and stdout, in [`crates/velocity-transfer/src/stdio.rs`](../crates/velocity-transfer/src/stdio.rs): the child writes `{"type":"ready","protocol":1}`, the parent sends `{"type":"request","seq":N,"command":{…}}`, and the child answers `{"type":"reply","seq":N,"result":{"Ok":{…}}}` or `{"result":{"Err":"NOT_FOUND"}}`, and pushes `{"type":"event","event":{…}}`. The production framing, versioning and supervision are #39.

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
