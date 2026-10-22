# Engine prototype results (#37)

**Date:** 2026-10-22. **Code:** `crates/velocity-curl` and `crates/velocity-transfer` on `feat/37-engine-core`. **Question it answers:** does the libcurl design in the [research report](2026-10-21-libcurl-engine-research-r2.md) (sections 4 and 5, stage 3) work correctly, in the app's process and as a child process? It is the evidence for #38 (which mode) and for going on with the full engine (#41 to #49).

**Short answer:** yes, for what the prototype covers. Both modes pass the same contract tests and download real HTTPS files byte for byte on Windows and Linux. Performance and the cost of the child process were **not measured** (see [What was not run](#what-was-not-run)).

## What the prototype is

| Part | Where | What it does |
|---|---|---|
| libcurl loop | `velocity-curl/src/fetch.rs` | One thread owns the `Multi`. Fetches arrive over a channel with a wakeup. A gate in the header callback accepts or aborts the final response before any body byte is delivered. Body data goes into a bounded buffer per fetch (1 MiB); when it's full the transfer pauses (`CURL_WRITEFUNC_PAUSE`). Only `http`, `https` and `ftp` are allowed, redirects only to `http` and `https`, and ambient proxy variables are ignored |
| Probe | `velocity-transfer/src/probe.rs` | Report section 5.1, minimal: the first segment (`bytes=0-S-1`, S = 4 MiB) doubles as the probe. Only a `206` with a valid `Content-Range` proves ranges. Every later segment is checked again |
| Download | `velocity-transfer/src/download.rs` | Static N segments after the probe, positioned writes into `<destination>.vdmpart` with byte coverage, a single-stream fallback that writes only missing bytes, and a finish that never replaces an existing file (`hard_link`, then remove the part file). Cancellation removes the part file |
| Engine | `velocity-transfer/src/engine.rs` | `InProcessEngine` implements the `DirectTransfer` contract: a worker thread per transfer over one shared loop, rate-limited progress events with a measured speed, and `Unsupported` for everything it can't do yet |
| Child process | `velocity-transfer/src/stdio.rs` | `velocity-transfer serve` runs the same engine behind JSON lines on stdin and stdout (a `ready` line, then `request`, `reply` and `event` messages in the contract's wire forms). `ChildProcessEngine` is the client. When stdin closes, the child cancels every transfer, removes their part files and exits |
| CLI | `velocity-transfer/src/main.rs` | `velocity-transfer get <url> <destination> [--connections N] [--probe-bytes B]` for manual runs. The issue's `velocity-transfer-bench` name was meant for benchmarks, which were dropped ([ADR-0009](../decisions/0009-no-aria2-benchmark-gate.md)) |

## Evidence

### Tests (L1 and L2, local fixtures)

All run against real libcurl, real sockets and real files, using the local test server in `crates/velocity-test-http`.

| Suite | Tests | What they show |
|---|---|---|
| `velocity-transfer` unit tests | 17 | Probe and segment classification, URL checks, splitting, coverage |
| `velocity-transfer/tests/download.rs` | 9 | Section 5.4's rows the prototype covers: ranges ignored → one stream with **zero extra GETs**; malformed `Content-Range` **refused, nothing left behind**; a `200` for a range after byte 0 **never written**, the rest fetched as one stream with each byte written once. Also: an empty file, a file smaller than the probe (one GET), no HEAD request, at most N connections, a 404, an existing destination never replaced |
| `velocity-transfer/tests/engine_contract.rs` | 8 | Each test runs against **both** modes: capabilities, completion with the exact bytes, a failure with its HTTP status, cancel → Removed with the part file deleted, remove, refused commands with their error codes, pause refused as `Unsupported`. One more checks that a child whose parent goes away removes its part file (it fails when the child's `cancel_all` is removed) |
| `velocity-curl/tests/` | 9 + 1 + 2 | The loop: gate, back-pressure, cancellation, redirects only to `http(s)`, no ambient proxy, the linked libcurl's version and features |

The tests compare every downloaded byte with the source, which is stronger than comparing hashes. A whole-file hash check inside the engine is #46.

### Manual HTTPS runs (real servers)

Files whose SHA-256 is pinned in [`scripts/native/deps.lock`](../../scripts/native/deps.lock), checked with `sha256sum` after each run. The zlib URL redirects from GitHub to its download host, so it also exercises a cross-host `https` redirect.

| Platform | Mode | File | Settings | Result |
|---|---|---|---|---|
| Windows 11 (build 26200, i7-13620H), Schannel | In-process (`get`) | curl 8.23.0 (2,990,916 bytes) | 1 and 4 connections, 4 MiB probe (one fetch) | PASS: SHA-256 `8dadd7d2…e6c5` |
| | | | 4 connections, 256 KiB probe (5 fetches) | PASS, 0 wasted bytes |
| | | zlib 1.3.2 (1,320,064 bytes) | the same three runs | PASS: SHA-256 `d7a06547…92f3` |
| | Child process (`serve`) | curl, zlib | 4 connections | PASS: `queued → connecting → downloading → completed`, child exit 0 |
| | In-process | existing destination | | PASS: refused (`FileExists`), file untouched |
| | In-process | a missing file on curl.se | | PASS: failed with HTTP 404 |
| Linux (WSL2 Ubuntu 24.04, kernel 6.6.87.2, 2 vCPU), OpenSSL 3.5.9 | In-process | curl, zlib | 4 connections with 256 KiB and 4 MiB probes | PASS, hashes match |
| | Child process | curl, zlib | 4 connections | PASS, child exit 0 |
| macOS | — | — | — | NOT RUN: no Mac (#83) |

Timings were printed but are single runs over a home connection, so they are not reported as results.

## Findings

- **The design holds.** Using the first segment as the probe costs no extra request, and the gate in the header callback rejects a bad response before any byte is written. Back-pressure keeps memory bounded per fetch, as the tests that track the buffer's peak show.
- **The curl crate pin works as ADR-0004 expected.** Because `Multi` isn't `Send` at the pinned commit, it's created on its loop thread; nothing else needed changing.
- **A portability trap:** `CURLcode` is a `u32` on Linux and an `i32` on Windows. Error codes are carried as `i64`. Linux CI caught it.
- **The contract fits a process boundary unchanged.** The child-process client reuses `TransferCommand`, `TransferReply`, `TransferEvent` and `dispatch` as they are; the framing adds only a sequence number and a `ready` line. The same tests pass in both modes.
- **Files smaller than the probe never use parallel connections.** With S = 4 MiB, the two pinned tarballs (1.3 and 3.0 MB) took one GET each. Whether 4 MiB is right is a tuning question for #47, not a correctness problem.

## What was not run

- **No performance measurements.** The report's section 4.3 asks for throughput, CPU, event latency, memory, restart time after `kill -9` and shutdown time, in both modes. None were measured: there is no benchmark harness yet (#77), and the engine rule forbids speed claims without one. So #38 can't yet use the report's cost thresholds (at most 2% throughput and 50 ms of event latency for the child process).
- **No crash recovery.** There are no checkpoints (#43), so a killed child loses its transfers. The client reports `EngineUnavailable` when the child goes away, but that path has no test yet; supervision and restarts are #39.
- **Not in the prototype:** pause and resume, retries, stall detection (a 60 s silence limit stands in), limits, live connection changes, credentials and headers, FTP and SFTP. Each is refused as `Unsupported`, not faked.

## For #38

Both modes work, and the contract keeps the choice open. What each costs, from this prototype:

- **In-process:** nothing extra to build or ship. A native fault in libcurl or the TLS library ends the whole app.
- **Child process:** about 350 lines for the framing, the client and their tests, and a second binary to package and sign. A crash ends only the engine, but restarting and resuming needs #39 and #43 first.

The research report recommends the child process unless it costs too much. That cost is the part not yet measured.
