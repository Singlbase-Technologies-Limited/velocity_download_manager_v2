# Running V2 beside V1

V2 and the original Velocity Download Manager (V1) can be installed and run at the same time without sharing anything. Each has its own data, database, single-instance lock, IPC endpoint and browser registration (#27, [ADR-0003](decisions/0003-v2-app-identity.md)).

## One source of identity

`branding.json` at the repository root holds V2's identity. The core reads it (`velocity_core::branding`), and `src-tauri/tauri.conf.json` must carry the same identifier and product name, which a test checks.

| Field | Development builds (today) | Release builds (packaging, #81–#83) | V1, for comparison |
|---|---|---|---|
| Product name | Velocity Download Manager (dev) | Velocity Download Manager | Velocity Download Manager |
| Identifier: data folder, single-instance key, pipe name | `dev.velocity.downloadmanager.v2` | `com.velocity.downloadmanager.v2` | `com.velocity.downloadmanager` |
| Native-messaging host (#65) | `dev.velocity.download_manager_v2` | `com.velocity.download_manager_v2` | `com.velocity.download_manager` |
| Firefox extension ID (#64) | `velocity-download-manager-v2@velocity.local` | the same | `velocity-download-manager@velocity.local` |
| Chrome extension IDs (#64) | none yet (a new key is generated in #64) | the same | `eecopkg…` |

## What derives from the identifier

| Resource | V2 | Where |
|---|---|---|
| Data folder | `<platform data dir>/<identifier>`. On Windows: `%APPDATA%\dev.velocity.downloadmanager.v2` | `velocity_core::paths::Paths` (it uses `dirs`, so the native host, which has no Tauri, finds the same folder) |
| Override for tests and portable setups | `VELOCITY_V2_DATA_DIR`, an absolute path. V1's `VELOCITY_DATA_DIR` never affects V2 | `paths::DATA_DIR_ENV` |
| Database, checkpoints | `velocity.sqlite3` and `checkpoints/` inside the data folder (#26, #43) | `Paths::database`, `Paths::checkpoints_dir` |
| Single instance | Starting V2 again focuses the running window | `tauri-plugin-single-instance`, keyed on the identifier |
| IPC endpoint | Windows: `\\.\pipe\<identifier>.ipc.<user>.<hash of the data folder>`. Unix: `<data folder>/ipc.sock`. The browser bridge that listens on it is #65 | `Paths::ipc_endpoint` |

The release identifier starts with V1's identifier (`com.velocity.downloadmanager` + `.v2`). That's safe, because V1 uses its identifier exactly, never as a prefix: V2's data folder sits next to V1's, never inside it, and the pipe names differ (PO decision on #27). Tests check every derived resource for exact non-collision.

## Verified side by side (Windows, 2026-10-22)

The installed V1 0.1.0 and the V2 dev build ran at the same time on the PO's PC:

- **Separate data folders:** V1 used `%APPDATA%\com.velocity.downloadmanager` (its database and engine files). V2 created `%APPDATA%\dev.velocity.downloadmanager.v2` at startup. V2's folder isn't inside V1's.
- **IPC:** V1's pipe `\\.\pipe\com.velocity.downloadmanager.ipc.…` was present. V2 has no IPC server yet (#65).
- **Single instance:** a second V2 launch exited by itself with code 0, and one V2 process remained.
- **Closing:** closing V2 through its window left V1 and its engine running and responding, with its pipe still present.
- **Not checked:** closing V1 while V2 runs. V1 was in use on the PO's PC.

Linux and macOS side-by-side runs: NOT RUN.
