# ADR-0002: The transfer engine lives in its own crates, separate from the core

- **Status:** Accepted
- **Date:** 2026-10-22
- **Decided by:** the Product Owner
- **Issue:** #33

## Context

V2's own direct-download engine ([ADR-0001](0001-v2-direction.md)) may run inside the app or as a supervised child process; that choice is still open in #38. A child process must build into a small binary of its own. V1's rule was that `velocity-core` owns domain logic *and* engine integration. If the engine lived there, the child binary would link the whole core: SQLite, settings, the manager and its types.

The research report (section 4.2, decision 4 in section 10) proposes two crates. One is a pure-Rust engine that can be tested against a simulated transport. The other is a thin layer that is the only code touching libcurl. V1 already set a precedent for a separate native-bridge crate (`velocity-lt` for libtorrent).

## Decision

**Option A from #33:**

- **`crates/velocity-transfer`** holds the engine logic: range probe, leases and scheduler, checkpoints, integrity, retries, budgets and limits, and the stdio protocol. It is a library, plus an optional `velocity-transfer` sidecar binary.
- **`crates/velocity-curl`** is the only crate that touches libcurl: FFI and the `Multi` loop on its own thread.
- **`crates/velocity-core`** keeps policy, persistence and the adapters, including the `DirectTransfer` contract (#36) that the manager calls.

## Options considered

| Option | For | Against |
|---|---|---|
| **A. New `velocity-transfer` and `velocity-curl` crates (chosen)** | Supports both deployment models (#38). A sidecar links only the engine. The FFI boundary is clear. The engine is testable without libcurl | Two more crates in the workspace |
| B. Everything in `velocity-core` | Simplest workspace | A sidecar binary would link the whole core; libcurl types leak into policy code |
| C. Only `velocity-curl` separate, logic in the core | Isolates FFI | The same sidecar problem as B, only smaller |

## Consequences

- **Rules:** `.claude/rules/transfer-engine.md` and `rust-native.md` apply to these paths. The rule "only `velocity-curl` calls libcurl" is no longer pending.
- **Unblocked:** #36 (contract), #37 (prototype) and #35 (libcurl provenance) can name these crates. The crates themselves are created by the first issue that needs them (#36 or #37), each as a new workspace member.
- **Dependencies:** adding libcurl or its Rust bindings still needs PO approval under the dependency policy (#9) and the libcurl build decision (#34).
- **Process model:** whether the engine runs in-process or as a child is still #38. This layout supports both.
