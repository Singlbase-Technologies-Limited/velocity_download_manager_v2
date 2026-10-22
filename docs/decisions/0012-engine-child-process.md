# ADR-0012: The direct-download engine runs as a supervised child process

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #38

## Context

The direct-download engine hosts C code: libcurl, the TLS library, nghttp2 and later libssh2 ([ADR-0011](0011-sftp-libssh2.md)). In the app's process, a native fault or a Rust panic (release builds abort on panic) ends the whole app, the UI and the torrent session with it. In a child process, only the engine dies.

The research report (section 4.3) recommended a child process unless the prototype showed it cost more than 2% throughput or 50 ms of event latency. The #37 prototype built both modes behind the same `DirectTransfer` contract ([engine-prototype-results.md](../research/engine-prototype-results.md)), and #38 measured them ([engine-mode-costs.md](../research/engine-mode-costs.md)). On Windows 11 and Linux (WSL2), over loopback, compared with in-process, the child process cost:

- **Throughput:** no measurable difference. The medians differed by 1 to 4%, in the child's favour, inside the spread within each mode.
- **CPU:** 2 to 6% more at the medians, with overlapping ranges.
- **Memory:** about 6 MiB more, for the second process.
- **Command round trips:** at most 4.2 ms, against the 50 ms threshold.
- **Startup and shutdown:** 3 to 38 ms more to start, and about 40 ms more to shut down.
- **Crashes:** a killed child was noticed in under 1 ms in most runs, but in about 7 s in one Windows run per series.

## Decision

1. **The direct-download engine runs as a child process** (`velocity-transfer serve`), started and supervised by the app over its inherited stdin and stdout. There is no listening socket, port or shared secret.
2. **The app reaches it only through the `DirectTransfer` contract,** with the client in `crates/velocity-transfer/src/stdio.rs`. The in-process engine stays for tests and the CLI, and is not shipped as a production mode.
3. **The production protocol and the supervisor are #39:**
   - framing and versioning that reject a mismatched engine;
   - restart with backoff;
   - cleanup or resume of the part files a crashed engine leaves behind;
   - engine messages treated as untrusted.
4. **Secrets cross only the pipe,** never process arguments or environment variables (`.claude/rules/security.md`).
5. **libtorrent is not part of this decision.** Where the torrent bridge runs is decided with #62.

## Options considered

| Option | For | Against |
|---|---|---|
| **A. Child process (chosen)** | A native fault or panic in the engine ends only the engine; the UI and torrents keep running, and a new engine is ready in milliseconds. No measurable throughput cost | A supervisor and protocol to build (#39), a second binary to package and sign (#81), about 6 MiB more memory and about 40 ms more at shutdown. **Not a sandbox:** the child runs as the same user with the same file access |
| B. In the app's process | Nothing extra to build or ship; the lowest latency | An engine crash closes the whole app. Memory-corruption bugs in C code can reach the app's memory |

## Consequences

- **#39** (protocol and supervisor) goes ahead; its "if a child process is chosen" condition is met.
- **#58** wires the manager to the engine through the child-process client.
- **#81** packages `velocity-transfer` as a sidecar binary (Tauri `externalBin`), with signing.
- **#28** covers the engine's stderr in diagnostics, and the "Restart transfer engine" action stays ([ADR-0005](0005-v1-features-without-aria2.md), via docs/PARITY.md).
- **Crash recovery depends on checkpoints (#43).** Until they exist, a restarted engine can't resume what a crashed one was downloading.

## Follow-up

- **The Windows crash-detection outlier** (about 7 s in one run per series) is investigated in #39, which owns crash detection.
- **Revisit** if V2's own benchmarks (#77) show the child process missing the report's thresholds on real networks or other machines.
