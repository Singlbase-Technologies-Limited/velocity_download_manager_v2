# Engine mode costs (#38)

**Date:** 2026-10-22. **Code:** `main` at `528c9e1` plus the fixes on `test/38-engine-mode-costs`. **Question it answers:** what does running the engine as a child process cost compared with running it in the app's process? These are the measurements the [research report](2026-10-21-libcurl-engine-research-r2.md) (section 4.3) asks for before #38 is decided, following the [prototype results](engine-prototype-results.md).

**Short answer:** on this machine, the child process costs no measurable throughput, about 2 to 6% more CPU, about 6 MiB more memory, 3 to 20 ms more to start and about 40 ms more to shut down. Its command round trips are 0.1 ms typically and at most 4.2 ms, against the report's 50 ms threshold. It notices a crash within 1 ms in most runs, but on Windows one run in each series (of 7 and of 11) took about 7 s.

## How it was measured

`crates/velocity-transfer/examples/mode_cost.rs`:

```sh
cargo build --release -p velocity-transfer --bins --examples
target/release/examples/mode_cost --mib 256 --runs 11
```

- **The server:** a local test server on loopback, in its own process, so its memory and CPU count against neither mode. Unthrottled, 4 connections, seeded random bytes.
- **The runs:** each run is a fresh process, alternating which mode goes first. Every downloaded file is compared with its source.
- **CPU:** the engine's process plus the app side, during the download only. On Linux it comes from `/proc`; on Windows from `Get-Process`, because there is no dependency for the Windows API.
- **Peak memory:** `VmHWM` on Linux, `PeakWorkingSet64` on Windows. "Parent" is the measuring process, which stands in for the app.
- **Command round trip:** the time for one `snapshot()` command. "Busy" means timed every 10 ms during a download; "idle" means 1,000 in a row afterwards. Events take the same path back as replies, so this stands in for event latency, which wasn't measured on its own.
- **Shutdown:** with a download under way. In-process, `cancel_all` until every worker has stopped; for the child, closing its stdin until it exits. In both, the part file is gone afterwards (checked).
- **Crash detection:** a child killed with `kill -9` or `taskkill /F` mid-download. The time runs from the kill command returning until the client's event stream ends.

**Machines:**
- **Windows 11** (build 26200, i7-13620H): files written to the system temp folder on disk.
- **Linux, WSL2 Ubuntu 24.04.4** (kernel 6.6.87.2, 2 vCPU, 1.9 GB RAM): files written to `/dev/shm`, because disk writes in this small VM stalled for up to 57 s in both modes (see [Findings](#findings)).

These are single-machine loopback results. They compare the two modes with each other; they say nothing about real-network speed.

## Results

256 MiB, 11 runs per mode. Each cell shows the median, with the range in brackets.

| Metric | Windows, in-process | Windows, child | Linux, in-process | Linux, child |
|---|---|---|---|---|
| Throughput (MiB/s) | 308 (265 to 386) | 311 (232 to 336) | 757 (515 to 881) | 791 (682 to 903) |
| CPU during the download (s) | 0.83 (0.64 to 1.03) | 0.88 (0.64 to 1.14) | 0.44 (0.41 to 0.65) | 0.45 (0.41 to 0.52) |
| Peak memory, parent (MiB) | 12.2 | 6.2 | 11.0 | 6.4 |
| Peak memory, child (MiB) | — | 12.2 | — | 11.0 |
| Startup (ms) | 1.3 | 20.7 (16.6 to 37.7) | 0.3 | 3.5 (2.9 to 25.9) |
| Round trip, busy, median (µs) | 1.9 | 115 | 1.3 | 74 |
| Round trip, busy, p99 (µs) | 9 | 269 (187 to 575) | 17 | 1,811 (441 to 4,208) |
| Round trip, idle, median (µs) | 0.1 | 43 | 0.07 | 98 |
| Shutdown (ms) | 1.2 | 40.8 (40.5 to 102) | 1.1 | 40.3 (40.2 to 60.4) |
| Crash detection (ms) | — | 0.03 (0.01 to 7,000) | — | 0.41 (0.32 to 0.72) |

A 512 MiB run of 7 per mode on Windows gave the same picture: medians of 368 and 309 MiB/s, with one child run at 19.5 MiB/s.

## Findings

- **Throughput:** no difference is measurable. The gap between the medians (1% on Windows, 4% on Linux, both in the child's favour) is smaller than the spread within each mode. The report's threshold was at most 2% lower throughput for the child.
- **CPU:** the child uses slightly more, 6% on Windows and 2% on Linux at the medians, with overlapping ranges. File data never crosses the pipe; only commands and rate-limited events do.
- **Memory:** a second process adds about 6 MiB, the measuring process's own baseline. The engine itself needs 11 to 12 MiB in either mode.
- **Latency:** the worst child round trip, 4.2 ms on the 2-vCPU Linux VM during a download, is a twelfth of the report's 50 ms threshold.
- **Startup and shutdown:** the child adds 3 to 38 ms at startup. Shutdown takes about 40 ms because both sides poll every 20 ms (`stdio.rs`); #39 can shorten that.
- **Crashes:**
  - **Detection:** a killed child is noticed in under 1 ms in most runs, and every later command fails as `EngineUnavailable`.
  - **The Windows outlier:** on Windows, 2 runs (one in each series) took about 7 s. That's unexplained so far; it may be Windows delaying a process's end while file writes are pending.
  - **What's left behind:** a killed child always leaves its `.vdmpart` file (36 of 36 runs over both platforms). With no checkpoints yet (#43), a restart can't resume it, so the time to resume after a crash can't be measured. A new child is ready again in the startup time above.
- **Found and fixed while measuring:**
  - **An unbounded queue:** the segment forwarders in `download.rs` used an unbounded channel. With a slow writer, a whole file queued in memory: 524 MiB peak for a 512 MiB file. It's now bounded at two events per segment, with a regression test.
  - **A race in the test server:** its `body_bytes_sent` counter was updated after each write, which made a test fail on Windows CI after #139 and #140. It's now updated before the write.
- **Disk stalls:** with the files on the WSL VM's disk, 512 MiB downloads in both modes sometimes took 5 to 57 s instead of 1 s, while CPU time stayed at 1.1 to 1.5 s. The engine was waiting for the disk. This affects both modes, so it doesn't bear on #38, but stall detection (#48) and the writer (#49) should expect it.

## What was not measured

- **Event latency on its own:** the command round trip stands in for it.
- **Time to resume after a crash:** this needs checkpoints (#43).
- **Other machines:** native Linux on hardware, and macOS (#83).
- **Real networks:** these are loopback results only.
