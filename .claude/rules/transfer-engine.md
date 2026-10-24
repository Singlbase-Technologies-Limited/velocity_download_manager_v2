---
# Decided in ADR-0002 (#33).
paths:
  - "crates/velocity-transfer/**/*"
  - "crates/velocity-curl/**/*"
  - "crates/velocity-sftp/**/*"
---

# Transfer engine, libcurl layer and SFTP layer

The design is in [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md) and section 5 of the [research report](../../docs/research/2026-10-21-libcurl-engine-research-r2.md). The terms are defined in the [glossary](../../docs/GLOSSARY.md).

- **Contract:** the manager sees only the `DirectTransfer` contract (#36). No libcurl type, handle or error code leaves the libcurl layer, and no russh type leaves the SFTP layer.
- **One transport interface:** the engine fetches through `velocity_curl::Transport` (ADR-0013). A new transport answers with the same events through a `FetchPipe`; it never gets a download path of its own.
- **libcurl:** only the libcurl layer calls it. Add and remove easy handles only on the `Multi` thread. Callbacks never block, never `.await`, and never call back into the manager.
- **SFTP:** only the SFTP layer calls russh, on its own runtime. The host key is checked against V2's known_hosts file before the login: unknown keys wait for the user, and changed keys are always refused (#53).
- **Probe:** the probe never costs a second full transfer. HEAD and `Accept-Ranges` are never treated as proof of range support. A `200` for a range that starts after byte 0 is aborted in the header callback, before any byte is written (#41).
- **Leases:** a byte is accepted only from the live lease that owns it. Lease ids are never reused. Late or unknown writes are dropped and counted as wasted. Invariants I1 to I7 are property-tested over a simulated transport (#42).
- **Durability:** coverage becomes durable only after an fsync and an atomic checkpoint write. Leases are never persisted (#43).
- **Validators:** only strong validators go in `If-Range`. A changed object discards the old epoch's bytes and is never mixed with new ones. A `200` is never appended (#45).
- **Completion:** a download is Completed only after coverage, length and, when a trusted hash exists, the whole-file hash are proven. The file is then renamed without replacing an existing one. "Verified" is shown only when a hash was checked (#46).
- **Memory:** buffers and queues are bounded. When the writer queue is full, the transfer pauses; nothing grows without limit (#49).
- **Starting values:** values the report calls hypotheses (segment sizes, ramp-up thresholds, timeouts) are named constants with a comment pointing to the measurement that set them. They are never presented as requirements.
- **Performance claims:** a speed or resource claim needs a measured run with its results attached. There's no benchmark harness yet, and no aria2 comparison ([ADR-0009](../../docs/decisions/0009-no-aria2-benchmark-gate.md)), so until V2's own benchmarks exist (#77), make no such claim.
