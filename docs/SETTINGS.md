# Settings

The settings V2 has so far, what each one does, and its bounds. They are sections of one settings document in the database, each read and saved through `Database::section` and `Database::save_section`, and checked first (the `Section` trait). The settings model as a whole, how it is saved and the Settings screen are #30; this page grows with it. Each setting is checked in Rust before it applies, whatever the screen allows.

## Performance

**Performance profiles are limits for the adaptive engine** ([ADR-0005](decisions/0005-v1-features-without-aria2.md), #57), not V1's aria2 option sets. Within a profile's limits, the engine still adds connections one at a time, only while each one makes the download faster ([ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#adaptive-connections), #47). A server that gains nothing from more connections gets fewer.

| Profile | Connections, per download and per server | Smallest piece | Notes |
|---|---|---|---|
| Conservative | 2 | 20 MiB | V1's values, for servers or networks that dislike many connections |
| **Balanced**, the default | 8 | 1 MiB | The engine's values before profiles existed, so nothing changed without a measurement |
| Maximum | 16 | 1 MiB | V1's connections. More than 8 only helps a server that gives each connection its own speed |
| Custom | The user's: 1 to 16 | The user's: 1 to 1024 MiB | V1's `max_connection_per_server` and `min_split_size_mib`. aria2's `split` count is gone, as V2 splits by itself |

- **All profiles add a connection at the same 10% gain** for now (PO decision on #57). Whether Conservative and Maximum should use other thresholds is for #77's measurements.
- **The smallest piece** applies to downloads that start after the profile changes. The connection cap applies at once: a lower one holds running downloads back, which their details then say ("the performance profile allows no more to one server").
- **The shared budget** for all downloads together stays 48 connections, V1's `connection_budget` (#47).
- **In code:** `PerformanceSettings` in [`crates/velocity-core/src/performance.rs`](../crates/velocity-core/src/performance.rs) maps a profile to `EngineLimits`. `Manager::set_performance` applies it, and new downloads ask for at most its connections (`Manager::connections_for_new_downloads`). The engine takes the limits through `set_engine_limits` ([ENGINE_CONTRACT.md](ENGINE_CONTRACT.md)).
- **Removed,** as ADR-0005 decided: "allow mid-download tuning" (V2 changes connections without restarting a download), and the aria2 optimizer.

## Queue

`QueueSettings` in [`crates/velocity-core/src/queue.rs`](../crates/velocity-core/src/queue.rs) (#50):

| Setting | Default | Bounds |
|---|---|---|
| `maxConcurrentDownloads` | 3 | 1 to 32 |
| `startImmediately` | on | — |
| `schedule.enabled` | off | — |
| `schedule.startTime`, `schedule.stopTime` | 02:00 to 07:00 | `HH:MM`, 24-hour; no stop time runs to midnight, and a stop before the start crosses midnight |
| `schedule.days` | every day | 0 (Monday) to 6 (Sunday); at least one when the schedule is on |

How the queue applies them is in [ENGINE_CONTRACT.md](ENGINE_CONTRACT.md#the-managers-side).

## Folders

`FolderSettings` in [`crates/velocity-core/src/destination.rs`](../crates/velocity-core/src/destination.rs) (#31), V1's `downloads` settings, saved as the settings document's `downloads` section. #30 puts them on the Settings screen (PO decision on #31). What they do is in [FILE_NAMING.md](FILE_NAMING.md#folders).

| Setting | Default | Bounds |
|---|---|---|
| `defaultDirectory` | empty: the system's Downloads folder, or `Downloads` in the home folder when the system names none | An absolute folder, at most 4096 bytes, with no control character and no `..` part |
| `autoCategorize` | on | — |
| `categorySubfolders` | on, V1's default since its #38 | — |

- **Before any are saved,** the defaults apply.
- **Saved ones that can't be read,** or that fail their checks, are a `DATABASE` error and are left as they were, never replaced by the defaults.

## History

`StorageSettings` in [`crates/velocity-core/src/history.rs`](../crates/velocity-core/src/history.rs) (#32), V1's `storage` settings, saved as the settings document's `storage` section. `set_storage_settings` saves them and applies them at once; the app also applies them when it starts. #30 puts them on the Settings screen.

| Setting | Default | Bounds |
|---|---|---|
| `historyRetentionDays` | 90 | 0 to 3650; 0 keeps entries forever |
| `maxHistoryEntries` | 10,000 | 100 to 1,000,000 |

- **What they forget:** entries no longer in the download list that finished more than the retention ago, then the oldest beyond the most kept. An entry still in the list is never forgotten.
- **Never files:** forgetting an entry deletes its record, with its checkpoint and torrent row, and never a file on disk.
- **A failure to apply them** at start is the snapshot's `storageError`, which the window shows; the app runs on.

## The window

V1's `general`, `notifications` and `appearance` settings and its first-run flag, in [`crates/velocity-core/src/app_settings.rs`](../crates/velocity-core/src/app_settings.rs) (#29). Each is a section of the settings document. #30 puts them on the Settings screen. What the window does with them is in [UI_ARCHITECTURE.md](UI_ARCHITECTURE.md).

| Section | Setting | Default | Bounds |
|---|---|---|---|
| `general` | `closeToTray` | on | Closing hides the window in the tray, when there is one |
| | `startMinimized` | off | Starts hidden in the tray, when there is one |
| | `confirmExitWithActive` | on | Asks before quitting while downloads are active |
| `notifications` | `onComplete` | on | Also when a torrent starts seeding, or waits for its files to be chosen |
| | `onFailure` | on | — |
| | `onStart` | off | When a download leaves the queue or a pause, not when it recovers from a retry or a stall |
| | `onEngineFailure` | on | When the transfer engine or the torrent helper becomes unavailable |
| `appearance` | `theme` | `system` | `system`, `light` or `dark` |
| | `density` | `compact` | `compact` or `comfortable`, for the download list (#59) |
| | `layout.sidebarWidth` | `null`: about 17% of the window, 220 to 240 px | 184 to 360 px |
| | `layout.sidebarCollapsed`, `layout.sidebarVisible` | off, on | — |
| | `layout.detailsWidth` | `null`: a fifth of the window, 300 to 340 px | 280 to 600 px, for the details panel (#61) |
| | `layout.detailsVisible` | off | — |
| | `visibleColumns` | `name`, `size`, `progress`, `speed`, `eta`, `status`, `category`, `added` (V1's) | The download list's columns, in display order (#59). Always with `name`, each column once. The ids are V1's 13: those above and `downloaded`, `avgSpeed`, `destination`, `started` and `completed` (`ColumnId`, fixture `columns.json`) |
| | `columnWidths` | none: each column's default width | The widths the user set by dragging, in CSS pixels by column id, 48 to 1200 px (#59) |
| `onboarding` | `completed` | off | Set by finishing the first-run setup, which also saves its folder as `downloads.defaultDirectory` |

- **Saved and read like the folder settings:** a panel or column width outside its bounds, and a column list without `name` or with a column twice, are refused with `INVALID_INPUT`. An unknown column id doesn't parse, so `set_appearance` refuses it before it reaches the core, and a saved section that can't be read is a `DATABASE` error for the window, left as it was.
- **Read when the app starts:** `general` and `notifications` are read when the app starts. A section that can't be read is left as it was, and its defaults apply until it is fixed, so closing and notifications keep working. The Settings screen (#30) will change them while the app runs.

## What a build can do

Settings and details never offer what the build can't do. The engine reports its capabilities per protocol (`ProtocolCapabilities`, [CONTRACTS.md](CONTRACTS.md)): whether it downloads the protocol at all, and whether it segments, resumes, checks validators, uses HTTP/2 and can encrypt it. In the UI, `availability` ([`src/capabilities.ts`](../src/capabilities.ts)) turns that into "available" or a reason, and `CapabilityOption` shows a control that is disabled, with the reason beside it, when the build lacks something. #30 and the download details (R8) place it.
