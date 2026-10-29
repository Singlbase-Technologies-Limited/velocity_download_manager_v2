# Architecture

V2 is a Tauri 2 desktop app with a React UI and a Rust core. It downloads files over HTTP, HTTPS, FTP and SFTP with its own transfer engine, on libcurl and, for SFTP, russh, and uses libtorrent for BitTorrent. It contains no aria2: the direction is recorded in [ADR-0001](decisions/0001-v2-direction.md).

**Most of this page is the target design.** Today the app opens its database, starts the transfer engine and the download manager, and has the downloads commands and snapshot events (#58); the UI still shows only the app's name and version, and calls `app_info`. Each component below names the issue that builds it. Parts that wait on an open decision are marked **pending**.

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
                            DirectTransfer contract              TorrentEngine contract
                                          │                               │
                                          ▼                               ▼
                  Transfer engine: probe, leases, checkpoints,     Torrent helper (child,
                  integrity, retries, limits (child, ADR-0012)     ADR-0014) → torrent bridge
                                          │                         → libtorrent (v1, v2,
                                          ▼                         hybrid, magnets)
                  libcurl layer: the only code that calls libcurl         │
                                          │                               │
                                          ▼                               ▼
                         HTTP, HTTPS, FTP and SFTP servers         peers and trackers
```

| Component | Path | Responsibility | Today | Built in |
|---|---|---|---|---|
| UI | `src/` | React and TypeScript. It talks to Rust only through typed commands and events, and validates every native response before use | The window (#29): first-run setup, sidebar with each list's count, status bar, Categories screen, theme, saved layout, and the exit confirmation; checks the contract version (`src/contract.ts`). Its structure is in [UI_ARCHITECTURE.md](UI_ARCHITECTURE.md) | #59 to #61, #63 |
| Desktop shell | `src-tauri/` | Tauri 2 app: thin typed commands that delegate to the core, the window, the tray, notifications, the single instance and the browser bridge | `app_info`, and since #58 the downloads: it opens the database, starts the supervised engine from beside its own executable (or runs without one and says why), runs the manager on its own thread, sends `downloads-snapshot` events and has V1's direct-download commands ([CONTRACTS.md](CONTRACTS.md#the-downloads-commands)). Since #29: the tray, close to tray, start minimised, the quit request, desktop notifications and the folder chooser ([CONTRACTS.md](CONTRACTS.md#the-windows-commands)) | #64, #65 |
| Core | `crates/velocity-core/` | Everything that doesn't need a window: domain models, the download manager and queue, input validation, naming, categories, settings, history and SQLite persistence | `app_info()`, the domain types (#25), identity and data paths (#27), the manager's first slice (#36), and the SQLite database with versioned migrations, backups and unknown-value handling ([DATA_AND_MIGRATIONS.md](DATA_AND_MIGRATIONS.md), #26). The manager saves its downloads, brings them back when the app starts, retries, removes with the download's own files only, and resumes what a lost engine was running (#58, [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md#the-managers-side)). Categories, the folder a download goes in, and what happens when its name is taken (#31, [FILE_NAMING.md](FILE_NAMING.md)) | #25, #26, #30 to #32, #50, #58 |
| `DirectTransfer` contract | `crates/velocity-core/` (ADR-0002) | The engine-neutral boundary for direct downloads: commands, events, statuses, errors and capabilities. The manager never sees libcurl types. Details: [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md) | The trait, its wire commands, a test-only fake engine, and the manager (`manager/`) | #36 |
| Transfer engine | `crates/velocity-transfer/` (ADR-0002) | The range probe, leased segmentation, durable checkpoints, changed-object detection, integrity, retries, connection budgets and speed limits | The probe as the first segment, with every response of report section 5.1 handled and classified before any byte is written: ranges, sequential ranges without a total, one stream, and a retry without Range after a multipart answer (#41, [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md)). The range scheduler (#42): leases that own their bytes alone, an equal share per free connection, splits that stop the old request at the new boundary, invariants I1 to I7 checked by property tests, and a fallback to one stream after a second anomaly. Checkpoints (#43): the part file is synced every 2 s or 16 MiB and on a pause, and the durable coverage is sent to the app. A resume trusts only a checkpoint that fits, pause and resume work, and a crash resumes from the last checkpoint (100 randomised `kill -9` trials). A sparse `.vdmpart` part file on Windows, and a finish that renames without ever replacing an existing file ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#persistence-and-recovery-43)). `InProcessEngine` implements the contract over it, refusing what it can't do yet as `Unsupported`; `velocity-transfer get` runs one download by hand. Connections (#47): `set_connections` works while a transfer runs, the per-host cap and a shared budget are shared max-min fairly, each download ramps up while connections help, and over HTTP/2 it multiplexes first and switches to connections of their own only when measurably faster ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#connections-47)). No validators yet (#45). Results: [engine-prototype-results.md](research/engine-prototype-results.md) | #37, #41 to #49 |
| libcurl layer | `crates/velocity-curl/` (ADR-0002) | The only crate that touches libcurl: FFI and the `Multi` loop on its own thread. libcurl 8.23.0 or later, with a build identity record; obtained as [ADR-0004](decisions/0004-libcurl-build-path.md) decides | Links the Velocity-built libcurl ([BUILDING_NATIVE.md](BUILDING_NATIVE.md)); `Libcurl::start()` refuses a libcurl that fails the capability check. `CurlLoop` runs every transfer on one `Multi` thread: a gate sees the final response head before any body byte, a slow reader pauses its transfer instead of growing a buffer, and the protocol and redirect allowlists, an explicitly empty proxy and no `Accept-Encoding` are set on every fetch | #35, #37 |
| SFTP layer | `crates/velocity-sftp/` ([ADR-0013](decisions/0013-sftp-russh.md)) | The only crate that touches russh. SFTP fetches on their own tokio runtime, answering with the same events as libcurl's through the shared `Transport` interface and a `FetchPipe`. V2's own known_hosts file: unknown host keys wait for the user, changed keys are always refused | No native library: russh with ring. The host key is checked before the login; password and key logins; reads from an offset, so the engine's probe, checkpoints and retries apply as for FTP | #53 |
| Engine process | `velocity-transfer serve`, a child process ([ADR-0012](decisions/0012-engine-child-process.md)) | The engine runs as a child process the app starts and supervises, speaking a versioned protocol over stdio | `velocity-transfer serve` speaks the framed, versioned protocol of [ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md). `ChildProcessEngine` is its client: one argument, a cleared environment, and stderr kept in memory. `SupervisedEngine` restarts a crashed engine with backoff and a cap, and reports the transfers a crash lost as retryable failures. The same contract tests pass in process and as a child process. The app runs it from beside its own executable (#58); packaging it as a sidecar is #81 | #38, #39 |
| Child-process layer | `crates/velocity-child/` ([ADR-0014](decisions/0014-torrent-helper-child-process.md)) | What the engine and the torrent helper share: the framing, the handshake and envelope, the serve loop, the app's client, and the supervisor with its restarts and hang checks | Both children run on it; each supplies a contract and a book for its supervisor ([ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md)) | #39, #62 |
| `TorrentEngine` contract | `crates/velocity-core/` (`torrent_engine.rs`) | The boundary for torrents: commands, events and snapshots in V2's types. Torrents never pass through `DirectTransfer` | Version 2: settings, add a `.torrent` file or a magnet, file choice, a torrent's metadata in chunks, pause, resume, remove, snapshot ([TORRENTS.md](TORRENTS.md#the-torrent-contract)). Torrent and magnet parsing in `torrent.rs` | #62 |
| Torrent engine and helper | `crates/velocity-torrent/` ([ADR-0014](decisions/0014-torrent-helper-child-process.md)) | What reaches libtorrent and what its state means; the helper `velocity-torrent serve`, its client and its supervisor | Downloads v1, v2-only and hybrid torrents and magnets, checks every input first (a magnet's metadata included, before any byte is written), downloads only the chosen files, and completes a torrent only after its files are flushed. A crashed helper is restarted. Seeding, trackers and port mapping too (#62). The app starts it from beside its own executable, without linking libtorrent, and the manager runs torrents beside direct downloads (#58, [TORRENTS.md](TORRENTS.md#in-the-app-58)) | #62 |
| Torrent bridge | `crates/velocity-lt/` ([ADR-0002](decisions/0002-engine-crate-layout.md)) | The only crate that calls libtorrent: a small C++ shim over one session. It never deletes files | libtorrent 2.0.15, built with vcpkg ([BUILDING_NATIVE.md](BUILDING_NATIVE.md#libtorrent)) | #62 |
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
| **Transfer engine** | Active transfers: connections, leases, received bytes and rate. It also owns the durable progress of each transfer, written to its checkpoint before the record advances. Checkpoints live in the SQLite database (PO decision, 2026-10-22, on #26); their format, and how the engine's child process gets them there, are #43 |
| **libtorrent** (in the torrent helper) | The torrent session: peers, pieces and resume data. The helper applies each torrent's seeding policy and the seeding queue, and its supervisor keeps each torrent's latest resume data in memory (PO decisions on #62). The manager owns torrent records, and stores their policies, metadata and resume data, without trackers (#58) |
| **The user's file system** | Finished files, written atomically: download to a part file, verify, then rename without replacing anything (#43, #72) |
| **Memory only** | Credentials, cookies and custom headers, as `Secret`s wiped when dropped. Never written to the database, logs, checkpoints or process arguments; saved in the system keychain only when the user asks, for that download ([ADR-0015](decisions/0015-keychain-with-consent.md), #70, #71) |

## Contracts

The UI and the native side share engine-neutral types: download records, statuses, progress, `TransferError`, capabilities and error codes. They're defined once in Rust (`crates/velocity-core/src/domain.rs`) and validated in the UI by strict schemas (`src/contract.ts`). Contract fixtures generated from Rust keep the two in step, and `COMMAND_API_VERSION` stops a mismatched UI from starting. Details: [CONTRACTS.md](CONTRACTS.md).

## A direct download, start to finish

This is the target flow. The details are designed and tested in the issues named in each step.

1. **Add.** The UI sends the request. The core validates the URL (scheme allowlist, IDN handling, credentials moved out of the URL), the destination and the name, then stores the record and queues it (#31, #50, #58, #67).
2. **Start.** When the queue allows, the manager passes a transfer spec through the `DirectTransfer` contract (#36). The queue is the manager's own (#50): at most `maxConcurrentDownloads` run at once (3 by default), waiting downloads start in queue order, and scheduled ones run only while the schedule's window is open in local time. A paused or finished download frees its slot. The engine is never given a download before its turn, so it has no queue of its own.
3. **Probe.** The first request is a bounded range request whose bytes are kept. The response shows whether the server supports ranges, without a wasted full transfer (#41).
4. **Transfer.** Segments are assigned as exclusive leases. Free workers split the largest remaining lease. Every byte is accepted only from the lease that owns it (#42, #47). Retries, back-off, stall detection and speed limits apply per transfer and globally (#48, #49).
5. **Checkpoint.** Progress becomes durable only after the data is flushed to disk, so a crash resumes from trusted coverage (#43). Every range request carries the strong validator in `If-Range`, so a file that changed on the server is never mixed with the old one (#45).
6. **Complete.** The engine verifies coverage, length and, when a trusted hash exists, the whole-file hash. It flushes the file, then renames it without replacing an existing file. The status says "Verified" only when a hash was checked (#46).

Status reaches the UI as events: the manager sends a `DownloadsSnapshot` whenever it changed, at most every 250 ms (#58). The status vocabulary for direct downloads is Queued, Connecting, Downloading, Retrying, Stalled, Paused, Verifying, Completed, Failed and Removed; a cancelled download ends as Removed (#25, #36).

## Processes and boundaries

| Process | Started by | Talks to |
|---|---|---|
| Desktop app | The user | The UI through typed commands and events, and the native host through local IPC (#65) |
| Transfer engine | The app, which starts it as a child process ([ADR-0012](decisions/0012-engine-child-process.md)) | The app over inherited stdin and stdout pipes: no listening socket, no port, no shared secret (#39) |
| Torrent helper | The app, which starts it as a child process ([ADR-0014](decisions/0014-torrent-helper-child-process.md)) | The app over the same kind of pipes and protocol. It also listens for BitTorrent peers and reaches trackers and peers on the network, as torrents need (#62) |
| Native host | The browser, through native messaging | The app over local IPC that accepts only local clients |

Everything that crosses a boundary is untrusted and validated on arrival: URLs, file names, server responses, browser messages and engine messages.

**Identity and data folders:** `branding.json` is the single source of V2's identity. The data folder (`%APPDATA%\dev.velocity.downloadmanager.v2` on Windows today), the single-instance key and the IPC pipe name all derive from the identifier, so nothing is shared with V1. Details and the side-by-side check: [COEXISTENCE.md](COEXISTENCE.md).

## Open decisions

These decision issues shape the architecture. Until each is decided and recorded as an ADR, the parts that depend on it are marked pending above.

None is open now.

Decided since: libtorrent runs in a supervised helper process, on a child-process layer shared with the engine ([ADR-0014](decisions/0014-torrent-helper-child-process.md)); the engine runs as a supervised child process ([ADR-0012](decisions/0012-engine-child-process.md)); SFTP uses russh in its own crate behind a shared transport interface ([ADR-0013](decisions/0013-sftp-russh.md), superseding [ADR-0011](decisions/0011-sftp-libssh2.md)); without a validator, a download resumes after a length check and shows "not verified" ([ADR-0010](decisions/0010-resume-without-validator.md)).

The research these questions draw on is in [docs/research/](research/README.md). The terms used here are defined in the [glossary](GLOSSARY.md).
