# ADR-0016: The released app is named "Velocity Download Manager V2"

- **Status:** Accepted
- **Date:** 2026-10-30
- **Decided by:** the Product Owner
- **Issue:** #81

## Context

[ADR-0003](0003-v2-app-identity.md) gave V2 its own technical identity: identifiers, data folders, pipes and browser names that never collide with V1's. It kept V1's product name, "Velocity Download Manager", for the released app.

The Windows installer Tauri 2.12.3 generates names three things after the product name, as its NSIS template shows (`${PRODUCTNAME}`):

- the install folder, `%LOCALAPPDATA%\<product name>` for a per-user install;
- the Apps & features entry, the registry key `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<product name>`;
- the Start menu shortcut.

V1 is installed on the PO's PC under exactly these names: the key `Velocity Download Manager`, and the folder `C:\Users\…\AppData\Local\Velocity Download Manager` (read 2026-10-30). A V2 installer with the same product name would install over V1 and replace its uninstall entry.

## Decision

**The released app's product name is "Velocity Download Manager V2".** It names the install folder, the Apps & features entry, the Start menu shortcut and the window title. It matches the browser extension's release name, "Velocity Download Manager V2".

Everything else in ADR-0003 stands: the identifiers, the native-messaging host and the extension IDs don't change. This replaces only ADR-0003's release product name.

## Options considered

| Option | For | Against |
|---|---|---|
| **"Velocity Download Manager V2" (chosen)** | Its own folder, uninstall entry and shortcut, with Tauri's own installer; the two are told apart in Apps & features | V2 shows a version in its name |
| Keep the name, with a custom installer script | One product name for both | Replacing Tauri's NSIS template means maintaining it; the two look identical in Apps & features |

## Consequences

- **V1 and V2 install side by side,** each with its own folder and uninstall entry. A test fails if the release product name ever equals V1's (`crates/velocity-core/src/branding.rs`).
- **`branding.json`'s `releaseProductName`,** [COEXISTENCE.md](../COEXISTENCE.md) and the packaging configuration (`src-tauri/tauri.release.conf.json`) carry the new name.
- **Offering to remove V1** from V2's installer, which ADR-0003 mentions, moves to a later issue (PO decision on #81).

## Follow-up

Revisit if V2 replaces V1 for good: the name could then drop "V2" in a release that also removes V1.
