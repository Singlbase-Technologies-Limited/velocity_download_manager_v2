# Engine internals

How the direct-download engine in `crates/velocity-transfer` fetches a file. The contract the manager sees is in [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md), and how the engine's child process talks to the app is in [ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md). The design comes from section 5 of the [research report](research/2026-10-21-libcurl-engine-research-r2.md); the terms are in the [glossary](GLOSSARY.md).

## The probe (#41)

The probe is the download's first request and also its first segment, so its bytes are kept. It is `GET` with `Range: bytes=0-(S−1)`, where S is `DownloadOptions::probe_bytes`. That's 4 MiB in the engine, the report's starting value for #47 to tune.

- **No HEAD:** a HEAD request is never sent, and `Accept-Ranges` is never taken as proof of range support.
- **No encoding:** no `Accept-Encoding` is sent. A body with any other encoding than identity isn't the file's own bytes.
- **Decided before any byte:** the final response is classified in libcurl's header callback ([`probe.rs`](../crates/velocity-transfer/src/probe.rs)). An answer the engine refuses is aborted there, so none of its body is written.

| Final response | What the engine does | `Mode` |
|---|---|---|
| `206`, `Content-Range: bytes 0-b/T` | Keeps bytes 0 to b, then fetches the rest as parallel segments | `Ranges` |
| `206` with fewer bytes than asked (b < S−1) | Keeps them; the rest is part of the segments | `Ranges` |
| `206`, `bytes 0-b/*` (no total) | Keeps bytes 0 to b. It asks for the rest with one open range, `bytes=(b+1)-`, and repeats from the first missing byte until a `416` says the file ends there. No parallel segments | `SequentialRanges` |
| `206` `multipart/byteranges` | Aborts, and retries once without `Range` | `SingleStream` after the retry |
| `200` | Keeps this response as the whole file: no second request, nothing wasted | `SingleStream` |
| `416` with `bytes */0` | An empty file | `Empty` |
| `401`, `407` | Fails: the server needs credentials. Not retryable | — |
| `429`, `503` | Fails as throttled, retryable. Waiting and honouring `Retry-After` are #48 | — |
| Other `4xx` | Fails, not retryable | — |
| Other `5xx` | Fails, retryable | — |
| A malformed `Content-Range`, a `Content-Length` that disagrees with it, a start other than 0, an end past the request or the total, an unrequested encoding | Fails before writing, not retryable | — |

## Later segments: the range scheduler (#42)

After a probe that gave a total, the rest of the file is fetched with leases from the range scheduler ([`scheduler.rs`](../crates/velocity-transfer/src/scheduler.rs), report section 5.2). The downloader's thread owns it and makes every change; no lock is held across I/O.

### Leases

A lease owns `[cursor, end)` of the file alone; `[start, cursor)` is already written. Its id is never reused within a download.

| State | Meaning |
|---|---|
| Assigned | The request `Range: bytes=start-(end−1)` was made; no answer yet |
| InFlight | The answer was checked and accepted in the header callback (`check_segment`) |
| Completed | `cursor == end` |
| Failed | It ended early; the rest of its range is unassigned again |
| Revoked | It was cancelled; the rest of its range is unassigned again |

- **Assignment:** a free connection gets the front of the unassigned bytes, an equal share per free connection. The share is rounded up to `align` and is at most `max_lease`. Sharing spreads a file over the connections without splits.
- **Acceptance:** bytes are accepted only from the lease that is in flight, at its cursor, and only up to its *current* end. Bytes for a lease that isn't live (ended, revoked, unknown), at another offset, or past its end are dropped and counted as wasted. They enter coverage only once the write to the part file has returned.
- **Split (work-stealing):** a free connection with nothing unassigned takes half of the live lease with the largest remainder. The remainder must be at least `2 × min_split`, and the midpoint is rounded down to `align`.
  - The old lease shrinks to end at the midpoint.
  - Its request is told to stop there: `FetchLimit::stop_at` in the libcurl layer delivers bytes up to the limit, then stops the transfer, and reports that as a completed fetch, not an error.
  - Over HTTP/1.1 its connection closes. Over HTTP/2 only the stream should be reset; that is tested with #47.
- **Retries:** a lease that ends before its end fails, and the rest of its range is handed out again under a new id. A late callback from the old request carries the old id and is refused.

| Size | Value | Where it comes from |
|---|---|---|
| `max_lease` | 32 MiB | Report section 5.2, a hypothesis for #47 to tune |
| `min_split` | 1 MiB (a remainder of at least 2 MiB splits) | A starting value; the report names no number |
| `align` | 64 KiB | Report section 5.2 |

### Answers that aren't what was asked

Each answer is checked in the header callback (`check_segment`):

- **Ranges lost:** a `200` is aborted before its body is written. When the live leases end, the rest of the file comes as one stream that writes only the missing bytes, so the file is never mixed.
- **Anomaly:** an answer that contradicts its request, such as a wrong or malformed `Content-Range`, a length that disagrees with it, or a multipart body. It also covers:
  - a body cut off before its range ended;
  - a chunked body that stopped short of its `Content-Range`;
  - a body that ran past its range.

  The first anomaly is tolerated, and the lease's rest is leased again. At the **second** on the same download (`MAX_ANOMALIES`), no more leases are handed out, and the rest comes as one stream that writes only the missing bytes (`RangesLost`).
- **Short answers:** a `206` with fewer bytes than asked is honest (RFC 9110 §14.2). Its lease fails, and only the remainder is leased again.
- **Failure:** an error status, or a different total, which means the file changed (#45).

### Invariants

Checked after every change in debug builds (`Scheduler::check`), and by the property tests:

- **I1 Partition:** the coverage C, the live leases' `[cursor, end)` and the unassigned bytes U are disjoint and make up `[0, T)`.
- **I2 Exclusivity:** no two live leases overlap, and none overlaps C.
- **I3 Acceptance:** a byte at offset p is written only by the live lease whose cursor is p.
- **I4 Monotonic coverage:** C only grows within an epoch; `new_epoch` (a changed file, #45) starts again from an empty C.
- **I5 Durability:** the durable coverage D is part of C and was synced before it was recorded (`synced`). Saving D in a checkpoint is #43.
- **I6 Completion:** C covers the file and no lease is live (`is_complete`).
- **I7 Conservation:** bytes received = growth of C + wasted bytes.

## Accounting

Every received byte is either written into the part file once, or counted in `wasted_bytes`. That includes bytes past a lease's end, bytes for a lease that isn't live, bytes the single stream sends again, and bytes already covered. Progress counts written bytes only. A server that ignores ranges costs exactly one GET with no wasted bytes, which a fixture test checks.

## Persistence and recovery (#43)

A download survives a pause, an app restart, an engine crash and a power loss without a corrupt or falsely finished file. The rules are in the [persistence rule](../.claude/rules/persistence.md); the PO decisions are on #43.

### Checkpoints

- **When:** every 2 s or 16 MiB, whichever comes first (`Cadence::DEFAULT`, the report's starting values). Also when a download is paused. The probe takes them too once its answer has given the file's total, because the probe can be the whole download.
- **How, in this order:**
  1. the part file is synced (`sync_data`);
  2. the scheduler marks what is covered as durable (D);
  3. the checkpoint is sent.

  A byte is in D only once it is on disk (invariant I5).
- **What it holds** (`Checkpoint`, [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md)):
  - a format version (`CHECKPOINT_FORMAT`);
  - the source without its login, query and fragment, so no token;
  - the total;
  - the ETag and Last-Modified of the first answer (recorded; #45 uses them);
  - D as half-open byte ranges.

  Leases are never saved.
- **Where it goes:** the engine sends it as a `checkpoint` event over its pipe. The app saves it in its database (the `checkpoints` table, [DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md)); only the app touches the database (PO decision). Until the manager is wired to the database (#58), nothing saves it yet.
- **Only ranges resume.** A single stream (a server without ranges) and a download without a total (sequential ranges) take no checkpoint, because nothing could resume them. Pausing them keeps the part file, and resuming starts over.

### Resuming

The app hands back what it saved, in `TransferSpec::resume`:

| `resume` | The engine |
|---|---|
| `checkpoint` | Trusts the checkpoint only if it fits: this format, this download's source, and ranges that are sorted, disjoint, non-empty and inside the total. If it fits, the part file is reopened (never through a link), only D is trusted, and only the rest is fetched. A changed total, or a `416` inside the old one, means the file on the server changed: the old bytes are discarded and the download starts over. If it doesn't fit, or the part file is gone, the download starts over |
| `discardPartFile` | The part file is the download's own but has no usable checkpoint: it is deleted and the download starts over (PO decision). A link, or anything that isn't a plain file, is refused and never touched |
| none | A new download. An existing part file is never reused: the download fails |

`StoredDownload::resume` in the database layer makes the choice from a record and its saved checkpoint. It picks the checkpoint if it's known and of this record's source, `discardPartFile` otherwise, and nothing for a finished record. A record this build can't route is never resumed.

### Pausing, stopping and crashing

- **Pause:** the requests stop, a last checkpoint is taken, and the part file stays. **Resume** starts a new worker from that checkpoint. Cancelling a paused transfer deletes its part file.
- **When the app goes away,** or closes the engine's pipe, the engine pauses every transfer and keeps their part files, so the app can resume them when it starts again. It no longer cancels them, as #39 did.
- **A failure:** one worth retrying, after a checkpoint was taken, keeps the part file for a retry (#48). Any other failure deletes it.
- **A crash** (`kill -9`, a power loss) leaves the part file and the last checkpoint the app saved. Nothing that looks finished exists until the rename.

### Finishing

The file is renamed into place only when coverage is complete:

1. the part file is sized and synced;
2. it is renamed **without ever replacing** an existing file. On Windows that's `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`, and with `MOVEFILE_WRITE_THROUGH`. On Linux it's `renameat2(RENAME_NOREPLACE)`, then a sync of the folder, so the new name survives a power loss.

A file that appeared at the destination during the download is kept, and the download fails with `FileExists`. On a Linux file system without `RENAME_NOREPLACE`, a hard link does the same job. The rename, unlike #37's hard link, also works on FAT32 and exFAT drives.

### Part files

[`files.rs`](../crates/velocity-transfer/src/files.rs) holds the only operations on part files:

- **Creating one:** exclusively, never over an existing name and never through a link. It's marked sparse on Windows (`FSCTL_SET_SPARSE`), so NTFS doesn't zero-fill the gap in front of a segment (see below).
- **Reopening one:** without following a link, checked again on the open handle.
- **Deleting one:** only if it's a plain file, never a link.
- **The no-replace rename:** described above.

The calls go through `windows-sys` and `libc` (PO decision), each `unsafe` call with its `SAFETY` note. `DownloadReport::sparse` says whether this run's part file was made sparse. A file system without sparse files, such as FAT or exFAT, still works, only slower.

## Disk writes far into a new file (Windows)

Segments are written in parallel, so a later segment writes far past what is already in the part file. Measured with `crates/velocity-transfer/examples/high_offset_write.rs` (report open question 7; #42, 2026-10-23). It writes 1 MiB into a new file and syncs it, 3 runs each:

| Machine | Offset of the write | No preallocation | After `set_len` | At offset 0 |
|---|---|---|---|---|
| Windows 11 (build 26200), NTFS, the system temp folder | 1 GiB | 0.6, 67 and 35 s | 4.5, 61 and 32 s | 0.4–0.5 ms |
| | 4 GiB | 120, 158 and 92 s | 128, 122 and 82 s | 0.4–0.5 ms |
| WSL2 Ubuntu 24.04, ext4 | 1 GiB and 4 GiB | 0.5–1.0 ms | 0.5 ms | 0.5 ms |

- **What happens:** NTFS zero-fills the whole gap before the write returns, here at about 30 to 50 MB/s. `set_len` doesn't help, because it doesn't move NTFS's valid data length. ext4 leaves the gap empty.
- **The effect, before #43:** a multi-GB parallel download on Windows could stall for minutes on a later segment's first write. The occasional 10 to 18 s runs of the fixture tests on Windows were likely the same effect, on a 128 MiB file.
- **The fix (#43, PO decision on #42):** the part file is sparse on Windows, so the gap is never filled.

**After #43**, on the same Windows machine (2026-10-23, `--only sparse,at_zero`, 3 runs each), the 1 MiB write into a part file as the engine creates it:

| Offset of the write | Sparse part file: write | Write and sync | At offset 0: write and sync |
|---|---|---|---|
| 1 GiB | 0.4–0.6 ms | 1.8–2.2 ms | 1.8–4.7 ms |
| 4 GiB | 0.5–0.7 ms | 2.0–7.2 ms | 1.7–2.1 ms |
| 16 GiB | 0.5–0.7 ms | 1.7–10.6 ms | 1.7–1.9 ms |

The gap is no longer filled: a write far into a sparse part file costs what a write at its start does. On a file system without sparse files (FAT, exFAT), the zero-fill remains.

Run it with `cargo run --release -p velocity-transfer --example high_offset_write -- [dir] [--gib N] [--runs R] [--only no_prealloc,set_len,sparse,at_zero]`.

## Tests

- **Fixtures:** `crates/velocity-transfer/tests/download.rs` runs the real engine against the local test server ([`velocity-test-http`](../crates/velocity-test-http/src/lib.rs)), whose options make it misbehave on demand. There is one test per row of report section 5.1's table, each checking the file's bytes, the number of GETs and the wasted bytes. A further test slows one connection so that a split happens, and checks that the old request stops at the boundary.
- **Properties:** `crates/velocity-transfer/tests/scheduler.rs` runs 3,000 generated scenarios of a simulated transport through the scheduler, with `proptest`. The scenarios mix random requests, chunk sizes, splits, refused answers, duplicate and late callbacks, cancellations, syncs, crashes that keep only D, and new epochs. After every event it checks I1 to I7 and that every covered byte equals the source. At the end it checks that the finished file is the source. Breaking the scheduler on purpose is caught: a split that doesn't shrink the old lease, accepting bytes at any offset, and a failed lease that doesn't give its bytes back each fail it.
- **Persistence fixtures:** `tests/download.rs` also covers:
  - checkpoints of synced bytes, the probe's included;
  - a pause and a resume from the last checkpoint, fetching only what was missing;
  - a file that changed on the server, which starts over;
  - checkpoints that don't fit, which aren't trusted;
  - a missing or discarded part file;
  - a file created at the destination mid-download, never replaced;
  - the sparse part file.

  `tests/engine_contract.rs` checks pause, resume and cancel in both engine modes.
- **Crash trials (L4):** `tests/crash_recovery.rs` kills a real engine (`kill -9`, `taskkill /F`) at a random moment, then resumes in a new one from the last checkpoint the first sent. Every trial checks four things:
  - nothing looks finished before the rename;
  - the file equals the source;
  - what wasn't durable at the kill is at most one checkpoint interval's worth;
  - the bytes fetched twice are at most that plus what was in flight.

  CI runs 3 trials. The full 100 run by hand with `VELOCITY_CRASH_TRIALS=100 cargo test -p velocity-transfer --test crash_recovery -- --nocapture` (PO decision on #43), and `VELOCITY_CRASH_SEED` repeats a run.
- **Unit tests:** the tests in `probe.rs`, `scheduler.rs`, `files.rs` and the libcurl layer's `tests/curl_loop.rs` (`FetchLimit`) cover each rule on its own.

## Not yet

- **HTTP/2:** the fixtures run over HTTP/1.1. The HTTP/2 fixtures, including the split's stream reset, need the HTTPS test server, and are #47 (PO decisions on #41 and #42). libcurl gives the engine the same final status and headers over either version.
- **Retries and waits** for errors, throttling and stalls: #48.
- **Validators and changed files:** a changed `Content-Encoding` between responses is an anomaly for now. Treating it, a changed total and changed validators as a changed object, with `new_epoch`, is #45.
- **Saving checkpoints in the app:** the manager saves the engine's checkpoint events in the database and resumes unfinished downloads at startup with `StoredDownload::resume` (#58).
