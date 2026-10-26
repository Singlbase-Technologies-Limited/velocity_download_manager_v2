# ADR-0014: libtorrent runs in a supervised helper process, on a child-process layer shared with the engine

- **Status:** Accepted
- **Date:** 2026-10-26
- **Decided by:** the Product Owner
- **Issue:** #62

## Context

V2 uses libtorrent for every torrent ([ADR-0001](0001-v2-direction.md), [ADR-0005](0005-v1-features-without-aria2.md)), and only the torrent bridge calls it ([ADR-0002](0002-engine-crate-layout.md)). [ADR-0012](0012-engine-child-process.md) put the direct-download engine in a supervised child process, so a native fault or a panic ends only the engine. It left libtorrent out of scope, for #62 to decide.

These are the facts on 2026-10-26:

- libtorrent is a large C++ library, with Boost and OpenSSL. In the app's process, a fault in it would end the whole app, the UI and the direct downloads with it.
- The engine's child-process code (#39) was written for the `DirectTransfer` contract. It is about 2,000 lines: the framing, the handshake, request and reply, events, hang checks, and restart with a backoff and a cap. Its types were private to `velocity-transfer`.
- V1 ran libtorrent in the app's process.

## Decision

1. **libtorrent runs in the torrent helper,** a child process the app starts and supervises: `velocity-torrent serve`. It works the way the engine does: one argument, a cleared environment, stderr kept in memory, the same restart backoff and cap, and the same hang checks.
2. **One shared child-process layer.** The engine's child-process code becomes generic over its messages, in a new workspace crate, `crates/velocity-child`, with no new outside dependencies:
   - **The framing:** `frame`.
   - **The envelope:** `message`, with a `Contract` trait that supplies the command, reply and event types and the contract version.
   - **Each side of a connection:** the child's `serve` loop and the app's `client`.
   - **The supervision:** `supervisor` restarts a crashed or hung child, and its `Book` holds what each kind of child needs: its setup, its events, and what happens to its work.
   - **The bookkeeping:** `ledger` keeps the current items and the orphans.

   The engine moves onto it in the same change, and its contract, crash and supervisor tests pass unchanged.
3. **The crates:**
   - `velocity-lt` is the bridge and the only crate that calls libtorrent, as `velocity-curl` is for libcurl.
   - `velocity-torrent` is the torrent engine: what reaches libtorrent and what its state means, the helper program, its client and its supervisor. It mirrors `velocity-transfer`.
   - The torrent contract, `TorrentEngine`, lives in `velocity-core`, beside `DirectTransfer`.
4. **The envelope's version becomes 3.** Its `start` message names the first id `firstId` for every contract; version 2 said `firstTransferId`.

## Options considered

| Question | Option | For | Against |
|---|---|---|---|
| Where libtorrent runs | **A. A supervised helper process (chosen)** | A fault in libtorrent ends only the helper; the app and direct downloads keep running, and a new helper starts | A second program to package and sign (#81); torrents that were running fail as retryable and are added again |
| | B. In the app's process (V1) | Simplest; no protocol | A libtorrent fault ends the whole app |
| | C. In the app's process first, then a later move | Fast start | The same fault exposure until the move, and the work done twice |
| Sharing the child-process code | **A. One generic layer, the engine migrated (chosen)** | One implementation of framing, handshake and supervision; a fix helps both | The PR touches the tested #39 code |
| | B. Share the framing only; a second supervisor | Faster | Two supervisors, a defect under the reuse rule ([AGENTS.md](../../AGENTS.md#reuse-before-you-add)) |
| | C. No supervisor for the helper yet | Smallest | No restart after a crash until later |
| The helper's crate | **A. A new crate, `velocity-torrent` (chosen)** | The same split as libcurl: bridge and engine | One more crate |
| | B. Inside `velocity-lt` | Fewer crates | The bridge crate then holds engine logic too |

## Consequences

- **What it protects:** a crash in libtorrent, Boost or OpenSSL no longer reaches the app, and its torrents fail as retryable. The helper gets its settings again when it restarts.
- **One implementation:** the engine and the helper share every line of their child-process code. `ENGINE_PROTOCOL.md` describes the envelope once, and each contract on its own.
- **What it costs:**
  - The helper is a second program to ship and sign (#81).
  - It makes protocol version 3, so an engine binary from before #62 is refused as incompatible. Engine and app always ship together.
  - Until resume data arrives (#62's lifecycle part), the torrents a crashed helper had start again from what is on disk, after libtorrent checks it.
- **Documents:** [ARCHITECTURE.md](../ARCHITECTURE.md), [ENGINE_PROTOCOL.md](../ENGINE_PROTOCOL.md) and [TORRENTS.md](../TORRENTS.md).

## Follow-up

- #62's later parts extend the torrent contract: magnets and file choice, lifecycle and seeding, trackers and port mapping.
- #58 wires the helper into the app, as it does the engine.
- #81 packages and signs `velocity-torrent`.
- **Revisit** if measurements (#77) show the helper's process boundary costs torrents throughput or latency the report's thresholds don't allow.
