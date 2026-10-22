# Glossary

Terms used across V2's documents, issues and code, in alphabetical order. Where a term comes from the [research report](research/2026-10-21-libcurl-engine-research-r2.md), the section is given. Code and docs use these words with these meanings. A new term is added here in the same PR that introduces it.

| Term | Meaning |
|---|---|
| **ADR** | Architecture decision record: one numbered, accepted decision with its context and consequences. See [decisions/](decisions/README.md) |
| **Back-pressure** | Pausing a transfer while the bounded disk-write queue is full, so a slow disk slows the network instead of growing memory (report §5.2; #49) |
| **Budget** | The connection allowance per download, per host and globally, which the scheduler shares fairly between downloads (#47) |
| **Build identity record** | `build-info.json`: what one native build contains and how it was made: each library's source, SHA-256, signature result and build options, what `curl -V` reports as linked, the artefact checksums, the toolchains and the V2 commit (#35; [BUILDING_NATIVE.md](BUILDING_NATIVE.md#build-infojson)) |
| **Capability** | Something an engine or build really supports, such as a protocol, live connection changes or validator-checked resume. It is reported, never assumed, so the UI can explain differences truthfully (report §4.2; #57) |
| **Changed object** | A file that changed on the server during a download, detected through its validator, its total length or its encoding. The old bytes are discarded and the download restarts once (report §5.3; #45) |
| **Checkpoint** | The durable record of a transfer's progress: the format version, a URL fingerprint, the total length, the validator, the encoding and the durable coverage. It is written atomically after an fsync (report §5.2; #43) |
| **Coverage (C)** | The disjoint byte intervals of a file that were written to disk by the lease that owned them (report §5.2) |
| **Credential handle** | An opaque number that refers to credentials the app keeps in memory. Only the handle crosses the `DirectTransfer` contract, never the secret (#36, #70) |
| **`DirectTransfer`** | The engine-neutral contract between the manager and the direct-download engine: add, pause, resume, cancel, remove, limits, connections, snapshots and events (report §4.2; #36) |
| **Durable coverage (D)** | The part of coverage that was fsynced and then recorded in a checkpoint. After a crash, only D is trusted (report §5.2) |
| **Engine** | The component that performs transfers. V2 has its own transfer engine for direct downloads, and libtorrent for BitTorrent. V2 contains no aria2 ([ADR-0001](decisions/0001-v2-direction.md)) |
| **Epoch** | The period during which one validator holds. A validator change starts a new epoch with empty coverage (report §5.2, invariant I4) |
| **Hash frontier** | Hashing the file's contiguous prefix from byte 0 as it becomes covered. Segment hashes can't be combined into a whole-file hash, so the rest is read back at the end (report §5.3; #46) |
| **Invariants I1–I7** | The scheduler properties checked after every simulated event: partition, exclusivity, acceptance, monotonic coverage, durability, completion and conservation (report §5.2; #42) |
| **Lease** | Exclusive ownership of a byte range `[cursor, end)` by one request, with an id that is never reused. States: Assigned, InFlight, Completed, Failed and Revoked. Bytes from a revoked or unknown lease are dropped and counted as wasted (report §5.2; #42) |
| **libcurl layer** | The only code that calls libcurl: FFI and the `Multi` event loop on its own thread (report §4.2; `crates/velocity-curl`, ADR-0002) |
| **Native lock file** | `scripts/native/deps.lock`: the pinned version, source URL, SHA-256, signing key and licence of every native library V2 builds (#35; [DEPENDENCIES.md](DEPENDENCIES.md#lock-file)) |
| **No-clobber finalisation** | Completing a download by renaming its part file to the final name without replacing an existing file (report §5.3; #43) |
| **Part file** | The file being downloaded, before it is verified and renamed (`.vdmpart` in the report; the final name is set in #43) |
| **PO** | The Product Owner, who decides scope, architecture, dependencies, licensing, security, repository settings, merges and releases |
| **Probe** | The first request of a download: a bounded range request whose bytes are kept. Its response decides between ranges, sequential ranges and a single stream, without a wasted full transfer. V2 doesn't use HEAD as proof of range support (report §5.1; #41) |
| **Ranges lost** | The state when a server that accepted ranges stops honouring them. The download continues as one stream from the first uncovered byte (report §5.1) |
| **Sidecar** | A helper program shipped with the app and started by it, such as a child-process engine (pending #38) |
| **Single stream** | Downloading over one connection from byte 0, used when the server ignores ranges (report §5.1) |
| **Split (work-stealing)** | Halving the largest remaining lease so a free worker can take the second half. The old request stops at the new boundary (report §5.2; #42) |
| **Strong validator** | An ETag without the `W/` prefix, or a Last-Modified date that RFC 9110 §8.8.2.2 treats as strong. Only a strong validator may be sent in `If-Range` (report §5.3; #45) |
| **Transfer engine** | V2's own direct-download engine: probe, leases, checkpoints, integrity, retries and limits, over the libcurl layer, in `crates/velocity-transfer` (ADR-0002; process model pending #38) |
| **V1** | The original Velocity Download Manager (`Singlbase-Technologies-Limited/velocity-download-manager`). A reference for features and benchmarks only. Never modified from V2 work |
| **V2** | This application. It replaces V1 under the name Velocity Download Manager, and runs beside it during development (#11) |
| **Validator** | A response header that identifies one version of a file, either an ETag or Last-Modified. It is used to detect a changed object between requests (report §5.3). See also *strong validator* |
| **Verification levels (L1–L5)** | The test levels: L1 fast local checks, L2 focused tests, L3 pre-merge selection, L4 expensive suites, L5 production acceptance. See [TESTING.md](TESTING.md) and #15 |
| **Version floor** | The oldest libcurl V2 accepts, today 8.23.0. A build below it fails, and the engine refuses to start (report §8.1; #35) |
| **Wasted bytes** | Bytes received but not accepted into coverage, such as from a revoked lease, an overrun or a rejected response. Counted and reported, never hidden (report §5.1) |
