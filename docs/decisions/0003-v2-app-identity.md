# ADR-0003: V2 keeps its own technical identity, separate from V1's

- **Status:** Accepted
- **Date:** 2026-10-22, completing the partial decision of 2026-10-21
- **Decided by:** the Product Owner
- **Issue:** #11

## Context

V2 will replace the original Velocity Download Manager (V1) under the same product name (PO decision, 2026-10-21, #11). V1's technical identity decides several things:

- **Its data directory:** derived from the bundle identifier `com.velocity.downloadmanager`.
- **Its single-instance lock and IPC pipe names.**
- **Its native-messaging host:** `com.velocity.download_manager`.
- **Its browser-extension IDs.**

If V2 reused any of these, it would open V1's database or intercept V1's browser messages. While both are installed, that's exactly what the coexistence rule forbids ([ADR-0001](0001-v2-direction.md)).

## Decision

**V2 keeps its own technical identity, also after release; it doesn't take over V1's.** The visible product name is the same; the identifiers aren't:

| Identifier | V1 (reference) | V2 release builds | V2 development builds |
|---|---|---|---|
| Product name | Velocity Download Manager | Velocity Download Manager | Velocity Download Manager (dev) |
| App identifier: decides the data directory, IPC names and single instance | `com.velocity.downloadmanager` | `com.velocity.downloadmanager.v2` | `dev.velocity.downloadmanager.v2` |
| Native-messaging host | `com.velocity.download_manager` | `com.velocity.download_manager_v2` | a dev variant, set in #65 |
| Firefox extension ID | `velocity-download-manager@velocity.local` | `velocity-download-manager-v2@velocity.local` | set in #64 |
| Chrome extension key and ID | V1's key | a **new** key, generated when the extension is ported (#64) | set in #64 |

Development and release identifiers also differ, so a development build never shares data with an installed release.

## Options considered

| Option | For | Against |
|---|---|---|
| **Own identity (chosen)** | V2 never touches V1's data or browser registrations. No conversion of V1 data is needed to install V2. Dev, release and V1 can all coexist | Users get a separate browser extension. Taking over from V1 needs an explicit installer step (#81) |
| Take over V1's identity at release | A seamless in-place upgrade; the same extension IDs | V2 would have to read and convert V1's data safely; more work and risk; no coexistence during the transition |
| Decide at packaging (#81) | More information later | #27, #64 and #65 would build on placeholders |

## Consequences

- **Release identity:** the app identifier switches to `com.velocity.downloadmanager.v2` in the packaging work (#81 to #83). Until then, the repository keeps the development identifier. The tests in `src-tauri/src/lib.rs` keep guarding that V2's identifier and dev port never match V1's.
- **Taking over from V1** is an installer feature: it offers to remove V1 (#81). V1's data is never modified.
- **Importing V1 data** (settings, categories, history) is a separate, later decision (#24). Any import is read-only toward V1.
- **Identity work:** #27 (data paths, single instance, IPC names), #64 (extension) and #65 (native host) use these values.
