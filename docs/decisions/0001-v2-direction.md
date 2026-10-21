# ADR-0001: V2 builds its own direct-download engine, uses libtorrent for BitTorrent and contains no aria2

- **Status:** Accepted
- **Date:** 2026-10-21
- **Decided by:** the Product Owner (direction stated in the epic #1)
- **Issue:** #1, recorded under #8

## Context

The original Velocity Download Manager (V1) hands every HTTP, HTTPS and FTP download to aria2, a GPL-licensed engine that runs as a separate process. BitTorrent runs on libtorrent. The [research report, revision 2](../research/2026-10-21-libcurl-engine-research-r2.md), found these gaps in aria2 as V1 ships it (section 1, verified in V1's code):

- **Protocols:** HTTP/1.1 only, and no SFTP in V1's builds.
- **Resume:** no ETag or `If-Range` validation, so a file changed on the server can be stitched from two versions.
- **Windows TLS:** no TLS 1.3 on Windows.
- **Connection changes:** changing the connection count restarts a running transfer.
- **Coupling:** the manager is coupled to aria2's API in about 55 places.

None of these was measured as a speed gap.

The report compared libcurl, reqwest, Wget2, Axel and Gopeed (section 2). It recommends a Velocity-owned engine with libcurl 8.23.0 or later as the transport, behind a small engine-neutral contract (sections 4 and 12). The report was written for V1, so it keeps aria2 as V1's default and legacy engine.

## Decision

The Product Owner's direction for V2, as stated in #1:

1. **Own engine.** V2 builds its own direct-download engine, orchestrated in Rust.
2. **libcurl.** libcurl is the leading transport candidate, subject to the report's version and security requirements: a floor of 8.23.0, provable build identity and the advisory policy in section 8.1.
3. **Protocols.** HTTP, HTTPS, FTP and SFTP are required.
4. **BitTorrent.** libtorrent handles BitTorrent v1, v2-only and hybrid torrents, and magnet workflows.
5. **No aria2 in V2.** It is not packaged, not a runtime dependency and not a hidden fallback, and no aria2 source is copied. V1, or an independently supplied aria2, may be used only as a benchmark and feature reference.
6. **Coexistence.** V1 and V2 install and run side by side, with isolated identity, data, IPC and native-messaging registrations.

Where the report assumes aria2 stays bundled, or that direct downloads fall back to aria2, this direction overrides it.

## What this means for the research report

| Report recommendation (written for V1) | In V2 |
|---|---|
| aria2 stays the default until the new engine passes its gates (sections 9, 12) | **Does not apply.** V2 has one direct-download engine, its own |
| An `Aria2Direct` adapter behind the `DirectTransfer` contract (section 4.2) | **Does not apply.** The contract exists for the deployment models (#36, #38) and for honest capability reporting, not to host aria2 |
| Unfinished aria2 downloads stay on aria2; mixed-engine routing and rollback (section 6) | **Does not apply.** Whether V2 imports anything from a V1 installation is a separate decision (#24) |
| GPL obligations continue while installers ship aria2 (section 8.3) | **Does not apply to V2,** which ships no aria2. V2's own licence is decided in #86 |
| Benchmarks against the best valid aria2 configuration (section 7) | **Applies, as a reference only.** V1's aria2 is a benchmark contender, never a V2 component (#20) |
| Transfer-correctness design: probe, leases, validators, integrity (section 5) | **Applies.** Built test-first in #41 to #46 |
| Security controls and libcurl version policy (section 8) | **Applies** (#35, #67 to #72, #75) |

## Options considered

These are summarised from the report's comparison in sections 2 and 4. The full candidate list, which also covers Wget2, Axel and Gopeed, is in section 2.

| Option | For | Against |
|---|---|---|
| A. Own engine on libcurl, no aria2 (chosen) | One stack covers HTTP/1.1, HTTP/2, FTP, FTPS, SFTP and every proxy type, with the connection controls segmentation needs. Permissive licence. V2 owns resume validation and integrity | Velocity must build segmentation, queueing, resume state and integrity itself. libcurl is C code, so memory-safety bugs are curl's (tracked advisories) |
| B. Keep aria2, as V1 does | Proven and already integrated | It keeps the gaps listed above and the GPL sidecar, and its upstream last released in 2023 |
| C. Own engine on reqwest/hyper | Pure Rust and memory safe | HTTP only. FTP and SFTP would need two more protocol stacks, and caps, rate limits and stall detection would all be custom |

## Consequences

- **The engine replaces everything aria2 did for V1** (report section 1, "What aria2 owns"): the queue and concurrency (#50), segmentation and leases (#41, #42, #47), resume state and checkpoints (#43), integrity (#46), retries and stall detection (#48), speed limits (#49), Metalink and mirrors (#54, #55), and FTP and SFTP (#51, #53).
- **Parity with V1 is measured,** not assumed (#22, #85). V1 features whose meaning changes without aria2 are decided in #23.
- **Licensing and dependencies:** no copyleft engine ships with V2. Each native library still needs the licence and provenance checks in #9 and #76.
- **Open decisions this direction leaves:**
  - #33: where the engine logic and the libcurl layer live.
  - #34: how libcurl is built and pinned.
  - #38: whether the engine runs in-process or as a child process.
  - #44: resume without a validator.
  - #52: the SFTP backend.
  - #56: proxies.
  - #68: browser links to private networks.
  - #11: the final identifiers. #11 has since recorded that V2 will replace V1 under the name "Velocity Download Manager"; side-by-side coexistence applies at least during development.

## Follow-up

- [ARCHITECTURE.md](../ARCHITECTURE.md) describes the components that follow from this decision.
- **Revisiting:** this decision would be revisited only if the libcurl prototype (#37) failed the report's gates, and then only through a new ADR.
