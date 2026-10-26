# The child-process protocol and supervisor

Two programs run as child processes of the app:
- **the direct-download engine** (`velocity-transfer serve`, [ADR-0012](decisions/0012-engine-child-process.md));
- **the torrent helper** (`velocity-torrent serve`, [ADR-0014](decisions/0014-torrent-helper-child-process.md)).

Both speak the same protocol, and the same supervisor keeps both running (#39, #62). The commands, replies and events differ:
- the engine's are the `DirectTransfer` wire forms in [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md);
- the helper's are the torrent contract in [TORRENTS.md](TORRENTS.md#the-torrent-contract).

Below, *the child* means either program.

The code:

| Part | File |
|---|---|
| Framing and the size limit | [`crates/velocity-child/src/frame.rs`](../crates/velocity-child/src/frame.rs) |
| Messages, the `Contract` trait, versions | [`crates/velocity-child/src/message.rs`](../crates/velocity-child/src/message.rs) |
| The child's serve loop | [`crates/velocity-child/src/serve.rs`](../crates/velocity-child/src/serve.rs) |
| The app's client (`ChildProcess`), the child's arguments and environment | [`crates/velocity-child/src/client.rs`](../crates/velocity-child/src/client.rs) |
| The supervisor, and the `Book` each kind of child supplies | [`crates/velocity-child/src/supervisor.rs`](../crates/velocity-child/src/supervisor.rs) |
| The items of a running child and the orphans of earlier ones | [`crates/velocity-child/src/ledger.rs`](../crates/velocity-child/src/ledger.rs) |
| The child's stderr log | [`crates/velocity-child/src/log.rs`](../crates/velocity-child/src/log.rs) |
| The engine's contract, client (`ChildProcessEngine`) and supervisor (`SupervisedEngine`) | [`crates/velocity-transfer/src/protocol.rs`](../crates/velocity-transfer/src/protocol.rs), [`stdio.rs`](../crates/velocity-transfer/src/stdio.rs), [`supervisor.rs`](../crates/velocity-transfer/src/supervisor.rs) |
| The helper's contract, client (`ChildTorrentEngine`) and supervisor (`SupervisedTorrentEngine`) | [`crates/velocity-torrent/src/lib.rs`](../crates/velocity-torrent/src/lib.rs), [`stdio.rs`](../crates/velocity-torrent/src/stdio.rs), [`supervisor.rs`](../crates/velocity-torrent/src/supervisor.rs) |

The app doesn't use the supervisors yet; the manager is wired to them in #58.

## Transport

- **Pipes only:** the child's inherited stdin and stdout carry the protocol, and its stderr carries free text for diagnostics. There is no socket, port or shared secret.
- **One argument:** the child is started as `velocity-transfer serve` or `velocity-torrent serve`, with no other argument.
- **A cleared environment:** the child inherits no environment variables except a fixed list of system settings that hold no secrets:
  - on Windows, `SystemRoot` and `windir`, which Windows sockets need;
  - on Linux, `SSL_CERT_FILE` and `SSL_CERT_DIR`, which tell OpenSSL where the trust store is when they are set.

  Proxy variables are dropped with the rest ([ADR-0008](decisions/0008-proxy-support.md)).
- **Secrets:** logins and custom header values cross only the pipe, inside an `add` command's spec, never arguments or the environment (#40, PO decision). `Debug` output never shows them ([ENGINE_CONTRACT.md](ENGINE_CONTRACT.md#secrets)).
- **No console window:** on Windows the child starts with `CREATE_NO_WINDOW`.

## Framing

- **Frames:** each message is one frame, a 4-byte little-endian length followed by that many bytes of UTF-8 JSON.
- **Size limit:** `MAX_MESSAGE_BYTES` is 4 MiB on both sides.
  - **Reading:** a longer frame is refused from its length alone, before any of its body is read.
  - **Writing:** a message over the limit is never written. If a reply would be too large, the child answers `INTERNAL` instead and says so on stderr.
  - **The value** is a starting value, not a measured one; a snapshot of 10,000 transfers is about 3 MiB.
- **Unknown fields:** a message with a field the reader doesn't know is refused, at every level: the envelope, the command, the spec, the snapshot and its progress and error.
- **Untrusted input:** both sides treat what they read as untrusted. A frame that is too large, cut off, not JSON or not a known message ends the session; nothing is guessed at.

## Messages

| Direction | `type` | Fields | When |
|---|---|---|---|
| child → app | `hello` | `protocol`, `contract` | First, as soon as the child starts |
| app → child | `start` | `protocol`, `contract`, `firstId` | The answer to `hello`, if the versions match |
| app → child | `request` | `seq`, `command` | Any number, after `start` |
| child → app | `reply` | `seq`, `result` (`{"Ok":{…}}` or `{"Err":"NOT_FOUND"}`) | One per request |
| child → app | `event` | `event` | At any time after `start` |

For example:

```json
{"type":"hello","protocol":3,"contract":11}
{"type":"start","protocol":3,"contract":11,"firstId":12}
{"type":"request","seq":7,"command":{"command":"pause","id":3}}
{"type":"reply","seq":7,"result":{"Err":"NOT_FOUND"}}
```

## Versions

- **What's checked:** the protocol version and the child's contract version must match exactly, and both sides check them.
  - **The protocol** (`PROTOCOL_VERSION`) is now 3. Version 1 was the #37 prototype's JSON lines; version 2 named the first id `firstTransferId`; version 3 (#62) names it `firstId`, for every contract.
  - **The engine's contract** (`TRANSFER_CONTRACT_VERSION`) is now 11; what each version added is in [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md).
  - **The helper's contract** (`TORRENT_CONTRACT_VERSION`) is now 3 ([TORRENTS.md](TORRENTS.md#the-torrent-contract)): version 2 added file choice, and version 3 seeding, resume data and `restore`.
- **The first frame:** the app refuses a child whose `hello` has other versions, or whose first frame isn't a `hello`, as `ENGINE_INCOMPATIBLE`. An engine that still speaks protocol 1 or 2 is refused this way.
- **The answer:** the child refuses a `start` with other versions by exiting, with the reason on stderr.
- **A child that exits before `hello`** failed to start: `ENGINE_START_FAILED`. A program that isn't there is `ENGINE_MISSING`.

## Shutdown

- **Graceful stop:** closing the child's stdin is the stop request.
  - **The engine** pauses every transfer, keeping its part file for a resume from its last checkpoint (#43), waits up to 5 seconds for its workers to stop, and exits.
  - **The helper** exits; libtorrent's files stay as they are. Before the app closes it, the app collects every torrent's resume data (`collect_resume_data`), so the next session needn't check them ([TORRENTS.md](TORRENTS.md#resume-data-and-recovery)).
- **Kill:** the app waits up to 5 seconds for the child's stdout to end, and kills it if it doesn't. The engine does the same when the app goes away.
- **No polling:** the app waits on the end of the child's stdout, not on a polling loop.
- **Dropping a supervisor** stops its child before it returns: the child has exited, or been killed, by then. No other thread keeps a retired child alive, so its files are closed (#62).

## Supervisor

`velocity_child::supervisor::Supervisor` runs one kind of child, with a `Book` for what that kind keeps:
- **`SupervisedEngine`** implements `DirectTransfer` around `ChildProcessEngine`, with `TransferBook`;
- **`SupervisedTorrentEngine`** implements `TorrentEngine` around `ChildTorrentEngine`, with `TorrentBook`.

The rules below are the engine's. The helper follows them too, with its own messages ("the torrent helper stopped unexpectedly"), its torrent settings given to each new helper, and `version` as its hang check. A torrent that a crashed helper had fails as retryable, then the new helper gets it back under its id, from its latest resume data (PO decision on #62; [TORRENTS.md](TORRENTS.md#resume-data-and-recovery)).

- **A new child is subscribed to before it is prepared**, so nothing it sends while it gets its setup is lost.
- **An orphan the running child reports again** is the child's again: the ledger stops answering for it.
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

## Child log

The child's stderr is read on a thread of its own, so a chatty child can't block on a full pipe.

- **Size:** the last 500 lines are kept in memory, each cut at 2,000 characters (V1's limits). A line without a newline is read in pieces of at most 8,000 bytes.
- **Redaction:** a login inside a URL (`https://user:password@host/`) is replaced with `<redacted>` before the line is stored. The shared redaction layer of #28 replaces this.
- **Storage:** nothing is written to disk. `SupervisedEngine::log()` and `SupervisedTorrentEngine::log()` return the lines for diagnostics (#28).

## Tests

| What | Where |
|---|---|
| Framing and size limits | `velocity-child` `frame.rs` unit tests |
| The envelope's names, unknown fields, versions | `velocity-child` `message.rs` unit tests, and `velocity-transfer` `protocol.rs` for the engine's own types |
| Handshake, request and reply, events, version mismatch, a prototype-era child, a protocol violation and an oversized frame from the child, the child's arguments and environment | `velocity-child` `client.rs` unit tests, against a fake child over in-memory pipes |
| The backoff; the ledger's orphans and ids | `velocity-child` `supervisor.rs` and `ledger.rs` unit tests |
| Redaction and bounds of the child's log | `velocity-child` `log.rs` unit tests |
| Unknown fields refused by every contract wire form | `velocity-core` `transfer.rs` and `torrent_engine.rs` unit tests |
| Crash and restart, lost transfers, the cap and a restart by hand, graceful stop, a missing program, and on Linux the real process's arguments and environment from `/proc` | `velocity-transfer` `tests/supervisor.rs`, with a real `velocity-transfer serve` child killed by `taskkill /F` or `kill -9` |
| Every engine contract behaviour, in process and as a child process | `velocity-transfer` `tests/engine_contract.rs` |
| The helper: real downloads, refused input, lifecycle, and a crash with its restart and its torrents back | `velocity-torrent` `tests/helper.rs`, with a real `velocity-torrent serve` child |
| Seeding, resume data, and a crashed helper's seeding torrent back from its resume data | `velocity-torrent` `tests/lifecycle.rs`, with a real `velocity-torrent serve` child |
