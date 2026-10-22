# Architecture

V2 is a Tauri 2 desktop app with a React UI and a Rust core. It downloads files over HTTP, HTTPS, FTP and SFTP with its own transfer engine on libcurl, and uses libtorrent for BitTorrent. It contains no aria2: the direction is recorded in [ADR-0001](decisions/0001-v2-direction.md).

**Most of this page is the target design.** Today the repository holds only the bootstrap shell: the UI calls one typed command, `app_info`. Each component below names the issue that builds it. Parts that wait on an open decision are marked **pending**.

## Components

```
 Browser extension ──(native messaging, stdio)──▶ Native host ──(local IPC)──▶ ┐
                                                                               │
 React UI (src/) ◀──(typed commands and events)──▶ Desktop shell (src-tauri/) ◀┘
                                                          │
                                                          ▼
                                     velocity-core: manager, queue, policy, validation,
                                     persistence (SQLite), settings, categories, history
                                          │                               │
                            DirectTransfer contract                 torrent bridge
                                          │                               │
                                          ▼                               ▼
                  Transfer engine: probe, leases, checkpoints,       libtorrent
                  integrity, retries, limits (process model #38)   (BitTorrent v1, v2,
                                          │                         hybrid, magnets)
                                          ▼                               │
                  libcurl layer: the only code that calls libcurl         │
                                          │                               │
                                          ▼                               ▼
                         HTTP, HTTPS, FTP and SFTP servers         peers and trackers
```

| Component | Path | Responsibility | Today | Built in |
|---|---|---|---|---|
| UI | `src/` | React and TypeScript. It talks to Rust only through typed commands and events, and validates every native response before use | Shows the app name and version; checks the contract version (`src/contract.ts`) | #29, #59 to #61, #63 |
| Desktop shell | `src-tauri/` | Tauri 2 app: thin typed commands that delegate to the core, the window, the tray, notifications, the single instance and the browser bridge | One command, `app_info` | #27, #29 |
| Core | `crates/velocity-core/` | Everything that doesn't need a window: domain models, the download manager and queue, input validation, naming, categories, settings, history and SQLite persistence | `app_info()`, the domain types (#25), identity and data paths (#27), and the manager's first slice (#36) | #25, #26, #30 to #32, #50, #58 |
| `DirectTransfer` contract | `crates/velocity-core/` (ADR-0002) | The engine-neutral boundary for direct downloads: commands, events, statuses, errors and capabilities. The manager never sees libcurl types. Details: [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md) | The trait, its wire commands, a test-only fake engine, and the manager's first slice (`manager.rs`) | #36 |
| Transfer engine | `crates/velocity-transfer/` (ADR-0002) | The range probe, leased segmentation, durable checkpoints, changed-object detection, integrity, retries, connection budgets and speed limits | — | #37, #41 to #49 |
| libcurl layer | `crates/velocity-curl/` (ADR-0002) | The only crate that touches libcurl: FFI and the `Multi` loop on its own thread. libcurl 8.23.0 or later, with a build identity record; obtained as [ADR-0004](decisions/0004-libcurl-build-path.md) decides | Links the Velocity-built libcurl ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)); `Libcurl::start()` refuses a libcurl that fails the capability check | #35, #37 |
| Engine process | pending #38 | The engine runs either in the app process or as a supervised child process speaking a versioned protocol over stdio | — | #38, #39 |
| Torrent bridge | to be created | A narrow Rust bridge to libtorrent | — | #62 |
| Native host | to be created | A short-lived process started by the browser. It validates messages and forwards them to the app over local IPC | — | #65 |
| Browser extension | to be created | Offers browser downloads to V2 and falls back to the browser on any failure | — | #64 |

**Placement rules** (also in [AGENTS.md](../AGENTS.md#architecture)):

- UI code lives in `src/`.
- Tauri commands in `src-tauri/` validate input and delegate to the core, without duplicating its logic. This rule is carried over from V1's `AGENTS.md`.
- Reusable domain logic lives in `crates/`. This rule is also carried over from V1.
- Only the libcurl layer calls libcurl, and only the torrent bridge calls libtorrent. This follows the research report, section 4.2, decided in [ADR-0002](decisions/0002-engine-crate-layout.md).

## Ownership of state

Each piece of state has one owner. Other components ask the owner instead of keeping their own copy.

| Owner | State |
|---|---|
| **Core** (the manager) | **Download records:** a stable app id per download, its source, destination, category and status, and its history. **Policy:** the queue order and concurrency limit, the schedule window, naming and conflict decisions, and settings. **Validation:** validation of all input. Records are stored in SQLite with versioned migrations (#26) |
| **Transfer engine** | Active transfers: connections, leases, received bytes and rate. It also owns the durable progress of each transfer, written to its checkpoint before the record advances. Whether checkpoints live in SQLite or in a sidecar file is decided in #43 |
| **libtorrent** | The torrent session: peers, pieces and resume data. The manager owns torrent records and seeding policies (#62) |
| **The user's file system** | Finished files, written atomically: download to a part file, verify, then rename without replacing anything (#43, #72) |
| **Memory only** | Credentials, cookies and custom headers. Never written to the database, logs, checkpoints or process arguments (#70, #71) |

## Contracts

The UI and the native side share engine-neutral types: download records, statuses, progress, `TransferError`, capabilities and error codes. They're defined once in Rust (`crates/velocity-core/src/domain.rs`) and validated in the UI by strict schemas (`src/contract.ts`). Contract fixtures generated from Rust keep the two in step, and `COMMAND_API_VERSION` stops a mismatched UI from starting. Details: [CONTRACTS.md](CONTRACTS.md).

## A direct download, start to finish

This is the target flow. The details are designed and tested in the issues named in each step.

1. **Add.** The UI sends the request. The core validates the URL (scheme allowlist, IDN handling, credentials moved out of the URL), the destination and the name, then stores the record and queues it (#31, #50, #58, #67).
2. **Start.** When the queue allows, the manager passes a transfer spec through the `DirectTransfer` contract (#36).
3. **Probe.** The first request is a bounded range request whose bytes are kept. The response shows whether the server supports ranges, without a wasted full transfer (#41).
4. **Transfer.** Segments are assigned as exclusive leases. Free workers split the largest remaining lease. Every byte is accepted only from the lease that owns it (#42, #47). Retries, back-off, stall detection and speed limits apply per transfer and globally (#48, #49).
5. **Checkpoint.** Progress becomes durable only after the data is flushed to disk, so a crash resumes from trusted coverage (#43). Every range request carries the strong validator in `If-Range`, so a file that changed on the server is never mixed with the old one (#45).
6. **Complete.** The engine verifies coverage, length and, when a trusted hash exists, the whole-file hash. It flushes the file, then renames it without replacing an existing file. The status says "Verified" only when a hash was checked (#46).

Status reaches the UI as events. The status vocabulary for direct downloads is Queued, Connecting, Downloading, Retrying, Stalled, Paused, Verifying, Completed, Failed and Removed; a cancelled download ends as Removed (#25, #36).

## Processes and boundaries

| Process | Started by | Talks to |
|---|---|---|
| Desktop app | The user | The UI through typed commands and events, and the native host through local IPC (#65) |
| Transfer engine | The app, if #38 chooses a child process | The app over inherited stdin and stdout pipes: no listening socket, no port, no shared secret (#39) |
| Native host | The browser, through native messaging | The app over local IPC that accepts only local clients |

Everything that crosses a boundary is untrusted and validated on arrival: URLs, file names, server responses, browser messages and engine messages.

**Identity and data folders:** `branding.json` is the single source of V2's identity. The data folder (`%APPDATA%\dev.velocity.downloadmanager.v2` on Windows today), the single-instance key and the IPC pipe name all derive from the identifier, so nothing is shared with V1. Details and the side-by-side check: [COEXISTENCE.md](COEXISTENCE.md).

## Open decisions

These decision issues shape the architecture. Until each is decided and recorded as an ADR, the parts that depend on it are marked pending above.

| Issue | Question | What depends on it |
|---|---|---|
| #38 | Engine in the app process, or a supervised child process? | Process model, IPC protocol (#39), packaging |
| #52 | SFTP through libssh2 in libcurl, or russh | SFTP (#53), dependencies |
| #44 | Resume when the server gives no reliable validator | Checkpoint and resume rules |

The research these questions draw on is in [docs/research/](research/README.md). The terms used here are defined in the [glossary](GLOSSARY.md).
