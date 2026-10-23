# The engine protocol and supervisor

The direct-download engine runs as a child process of the app ([ADR-0012](decisions/0012-engine-child-process.md)). This document describes how the two talk, and how the app keeps the engine running (#39). The commands, replies and events themselves are the `DirectTransfer` wire forms in [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md).

The code:

| Part | File |
|---|---|
| Framing, messages, versions | [`crates/velocity-transfer/src/protocol.rs`](../crates/velocity-transfer/src/protocol.rs) |
| The engine's side (`velocity-transfer serve`) and the app's client (`ChildProcessEngine`) | [`crates/velocity-transfer/src/stdio.rs`](../crates/velocity-transfer/src/stdio.rs) |
| The supervisor (`SupervisedEngine`) | [`crates/velocity-transfer/src/supervisor.rs`](../crates/velocity-transfer/src/supervisor.rs) |
| The engine's stderr log | [`crates/velocity-transfer/src/engine_log.rs`](../crates/velocity-transfer/src/engine_log.rs) |

The app doesn't use the supervisor yet; the manager is wired to it in #58.

## Transport

- **Pipes only:** the engine's inherited stdin and stdout carry the protocol, and its stderr carries free text for diagnostics. There is no socket, port or shared secret.
- **One argument:** the engine is started as `velocity-transfer serve`, with no other argument.
- **A cleared environment:** the engine inherits no environment variables except a fixed list of system settings that hold no secrets:
  - on Windows, `SystemRoot` and `windir`, which Windows sockets need;
  - on Linux, `SSL_CERT_FILE` and `SSL_CERT_DIR`, which tell OpenSSL where the trust store is when they are set.

  Proxy variables are dropped with the rest ([ADR-0008](decisions/0008-proxy-support.md)).
- **Secrets:** credentials will cross only the pipe, never arguments or the environment (#70). Today the engine refuses a transfer with credentials or custom headers as `Unsupported`.
- **No console window:** on Windows the engine starts with `CREATE_NO_WINDOW`.

## Framing

- **Frames:** each message is one frame, a 4-byte little-endian length followed by that many bytes of UTF-8 JSON.
- **Size limit:** `MAX_MESSAGE_BYTES` is 4 MiB on both sides.
  - **Reading:** a longer frame is refused from its length alone, before any of its body is read.
  - **Writing:** a message over the limit is never written. If a reply would be too large, the engine answers `INTERNAL` instead and says so on stderr.
  - **The value** is a starting value, not a measured one; a snapshot of 10,000 transfers is about 3 MiB.
- **Unknown fields:** a message with a field the reader doesn't know is refused, at every level: the envelope, the command, the spec, the snapshot and its progress and error.
- **Untrusted input:** both sides treat what they read as untrusted. A frame that is too large, cut off, not JSON or not a known message ends the session; nothing is guessed at.

## Messages

| Direction | `type` | Fields | When |
|---|---|---|---|
| engine → app | `hello` | `protocol`, `contract` | First, as soon as the engine starts |
| app → engine | `start` | `protocol`, `contract`, `firstTransferId` | The answer to `hello`, if the versions match |
| app → engine | `request` | `seq`, `command` | Any number, after `start` |
| engine → app | `reply` | `seq`, `result` (`{"Ok":{…}}` or `{"Err":"NOT_FOUND"}`) | One per request |
| engine → app | `event` | `event` | At any time after `start` |

For example:

```json
{"type":"hello","protocol":2,"contract":3}
{"type":"start","protocol":2,"contract":3,"firstTransferId":12}
{"type":"request","seq":7,"command":{"command":"pause","id":3}}
{"type":"reply","seq":7,"result":{"Err":"NOT_FOUND"}}
```

## Versions

- **What's checked:** the protocol version (`PROTOCOL_VERSION`, now 2) and the contract version (`TRANSFER_CONTRACT_VERSION`, now 6: version 2 added checkpoints, #43, version 3 the global speed limit, #49, version 4 the connection detail in snapshots, #47, version 5 the checkpoint's encoding, #45, and version 6 integrity, #46) must match exactly, and both sides check them.
- **The first frame:** the app refuses an engine whose `hello` has other versions, or whose first frame isn't a `hello`, as `ENGINE_INCOMPATIBLE`. Version 1 was the #37 prototype's JSON lines; an engine that still speaks it is refused this way.
- **The answer:** the engine refuses a `start` with other versions by exiting, with the reason on stderr.
- **An engine that exits before `hello`** failed to start: `ENGINE_START_FAILED`. A program that isn't there is `ENGINE_MISSING`.

## Shutdown

- **Graceful stop:** closing the engine's stdin is the stop request. The engine pauses every transfer, keeping its part file for a resume from its last checkpoint (#43), waits up to 5 seconds for its workers to stop, and exits.
- **Kill:** the app waits up to 5 seconds for the engine's stdout to end, and kills it if it doesn't. The engine does the same when the app goes away.
- **No polling:** the app waits on the end of the engine's stdout, not on a polling loop.

## Supervisor

`SupervisedEngine` implements `DirectTransfer` around `ChildProcessEngine`.

- **Crash detection:** a crash is noticed when the engine's stdout ends or breaks the protocol. On Windows that took a median of 0.02 ms and at most 0.7 s over 22 measured kills ([measurements](research/engine-mode-costs.md#rerun-after-39)).
- **Lost transfers:** each transfer that was unfinished is reported as Failed with a retryable error, "the transfer engine stopped unexpectedly" (PO decision on #39).
  - Their part files stay on disk; checkpoints (#43) will resume from them.
  - The supervisor keeps answering for these transfers: `snapshot` lists them and `remove` forgets them. Other commands for them fail with `INVALID_STATE`.
- **Restart:** it starts a new engine after a backoff of 1, 2, 4, 8 and 16 seconds, up to 32, doubling with each crash in the window. A start that fails counts as a crash.
- **The cap:** more than 5 crashes within 5 minutes (V1's limit) leave the engine *unavailable*. Commands then fail with `ENGINE_UNAVAILABLE` until `restart()`, the "Restart transfer engine" action, clears the count and starts a new engine.
- **Unfixable starts:** an engine that is missing or incompatible is unavailable at once, because starting it again can't help.
- **Health:** `health()` and `subscribe_health()` report `Running`, `Restarting { attempt }` and `Unavailable { code }`.
- **Ids:** a new engine is told, in `start`, to assign ids after the highest one it has seen, so an id never names two transfers.
- **No lock held across I/O:** a command takes its own reference to the running engine before talking to it, so no lock is held while waiting for the engine.
- **Hangs** (#48): an engine that hangs without exiting is noticed and restarted.
  - Every 5 seconds the supervisor asks it for a snapshot. When 2 checks in a row get no answer within 5 seconds, it kills the engine and handles it like a crash. The values are starting values.
  - Its transfers fail as retryable, with "the transfer engine stopped responding and was restarted", and the hang counts towards the cap.
  - A stalled disk doesn't look like a hang. The engine answers commands on its own thread, and its workers write and sync without holding the engine's state. One exception: cancelling a *paused* transfer deletes its part file while the state is held.
  - A command that is still waiting times out after 10 seconds with `ENGINE_UNAVAILABLE`, as before.

## Engine log

The engine's stderr is read on a thread of its own, so a chatty engine can't block on a full pipe.

- **Size:** the last 500 lines are kept in memory, each cut at 2,000 characters (V1's limits). A line without a newline is read in pieces of at most 8,000 bytes.
- **Redaction:** a login inside a URL (`https://user:password@host/`) is replaced with `<redacted>` before the line is stored. The shared redaction layer of #28 replaces this.
- **Storage:** nothing is written to disk. `SupervisedEngine::log()` returns the lines for diagnostics (#28).

## Tests

| What | Where |
|---|---|
| Framing, size limits, unknown fields, versions | `protocol.rs` unit tests |
| Unknown fields refused by every contract wire form | `velocity-core` `transfer.rs` unit tests |
| Handshake, version mismatch, a prototype-era engine, a protocol violation and an oversized frame from the engine, the engine's arguments and environment | `stdio.rs` unit tests, against a fake engine over in-memory pipes |
| Redaction and bounds of the engine log | `engine_log.rs` unit tests |
| Crash and restart, lost transfers, the cap and a restart by hand, graceful stop, a missing program, and on Linux the real process's arguments and environment from `/proc` | `tests/supervisor.rs`, with a real `velocity-transfer serve` child killed by `taskkill /F` or `kill -9` |
| Every contract behaviour, in process and as a child process | `tests/engine_contract.rs` |
