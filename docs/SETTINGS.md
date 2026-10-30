# Settings

Every V2 setting, what it does, and its bounds. They are sections of one settings document in the database, each read and saved through `Database::section` and `Database::save_section`, and checked first (the `Section` trait). Each setting is checked in Rust before it applies, whatever the screen allows.

## The Settings screen

The screen (#30, `src/features/settings/SettingsView.tsx`) edits every section below except the window's look, which keeps saving through `set_appearance` so it never overwrites a panel width ([the window](#the-window)).

- **Saved whole:** `set_settings` checks every section first and saves none when one is refused (`INVALID_INPUT`); then it writes them all in one go, and applies them at once (`velocity_core::settings::Settings`, `Manager::set_settings`). The screen says "Saving…", then "Saved", or why it wasn't, and shows what is saved.
- **When each applies:** at once, all of them. The engine's retries, timeouts, preallocation and write buffer apply to downloads that start or retry after the change; the connection budget, the queue, the limits and the torrent helper's settings apply to running ones too. None needs a restart.
- **Read with defaults:** a section is read merged over its defaults, so settings an older V2 saved get the newer fields' defaults (V1's `migrate_settings_json`). A field this build doesn't know is refused, never dropped.
- **When the app starts:** the saved settings are applied (`Manager::apply_saved_settings`), and the torrent helper starts with its own. A failure is logged, and the defaults hold for what couldn't be read.
- **Reset to defaults** asks first, and keeps the download folder (V1's).
- **What the build can't do** is disabled with the reason ([below](#what-a-build-can-do)). Today's engine does everything the screen offers, Metalink included (#54).

## Engine

`EngineSettings` in [`crates/velocity-core/src/transfer.rs`](../crates/velocity-core/src/transfer.rs) (#30), the `engine` section: V1's retry, timeout, connection-budget and disk settings, as ADR-0005 mapped them. The engine gets them through `set_engine_settings` (transfer contract version 14); its supervisor gives them to a new engine after a restart.

| Setting | Default | Bounds | V1's |
|---|---|---|---|
| `failuresInARow` | 5 | 1 to 100 | `max_tries` |
| `maxRetryWaitSecs` | 60 | 1 to 600 | `retry_wait_secs` |
| `stalledAfterSecs` | 15 | 5 to 600 | `lowest_speed_limit_kib`, on time without data rather than speed |
| `resetAfterSecs` | 30 | 5 to 3600, never below `stalledAfterSecs` | `timeout_secs` |
| `connectTimeoutSecs` | 30 | 1 to 300 | `connect_timeout_secs` (HTTP, HTTPS and FTP; SFTP keeps its own) |
| `downloadRetries` | 2 | 0 to 10 | `app_retries` |
| `connectionBudget` | 48 | 1 to 256 | `connection_budget` |
| `preallocation` | `sparse` | `none`, `sparse` or `full` | `file_allocation` |
| `writeBufferMib` | 2 | 1 to 64, per connection | `disk_cache_mib` |

- **Preallocation:** `sparse` marks the part file sparse on Windows (#42's measurement), `none` doesn't, and `full` sets the file's length as soon as its size is known, so a disk too small fails at once.
- **Write buffer:** how many fetch events, of at most 1 MiB each, a connection may have waiting for the writer.

## Advanced and new torrents

`AdvancedSettings` (the `advanced` section) and `NewTorrentSettings` (`newTorrents`) in [`crates/velocity-core/src/settings.rs`](../crates/velocity-core/src/settings.rs) (#30).

| Section | Setting | Default | Bounds |
|---|---|---|---|
| `advanced` | `globalDownloadLimitKib` | 0: none | 0, or at least 1 KiB/s (#49) |
| | `enableBittorrent` | on (V1's) | Off refuses torrents and magnet links with `PROTOCOL_DISABLED` |
| | `enableMetalink` | off (V1's) | Offered when the engine reads Metalink, as it does since #54 |
| | `userAgent` | `null`: `VelocityDownloadManager/<version>` | At most 256 bytes, no control characters; for downloads added after it |
| `newTorrents` | `chooseFilesForMagnets` | off | The Add dialog's "Choose files before downloading" |
| | `seeding` | never | As a torrent's seeding policy ([TORRENTS.md](TORRENTS.md)) |
| | `uploadLimitKib` | 0: none | The Add dialog's upload limit |

## Browser

`BrowserSettings` (the `browser` section) in [`crates/velocity-core/src/browser/mod.rs`](../crates/velocity-core/src/browser/mod.rs) (#65), V1's. The screen's Browser section also registers the native host with each browser ([BROWSER_EXTENSION.md](BROWSER_EXTENSION.md#registering-with-each-browser)).

| Setting | Default | What it does |
|---|---|---|
| `acceptRequests` | on | Off refuses links from the extension (`DISABLED`); the browser then downloads them |
| `confirmBeforeAdding` | on | A link opens the Add dialog. Off adds it at once, unless it is a torrent, a magnet link, a taken name or a link added before |

The torrent helper's own settings are the `torrent` section, `TorrentSettings` ([TORRENTS.md](TORRENTS.md#settings-and-identity)); the screen shows them with the helper's port and port mapping (`get_torrent_network`).

## Proxy

`ProxySettings` (the `proxy` section) in [`crates/velocity-core/src/proxy.rs`](../crates/velocity-core/src/proxy.rs) (#192, [ADR-0008](decisions/0008-proxy-support.md)). The screen's Proxy section edits it. It holds no password.

| Setting | Default | Bounds |
|---|---|---|
| `kind` | `none` | `none`, `http`, `https` or `socks5` |
| `host` | empty | With a proxy: a DNS name (an international one kept in its ASCII form, as links are), an IPv4 address, or an IPv6 address in brackets; no scheme, port or login. Without one, kept as typed |
| `port` | 8080 | 1 to 65535 with a proxy |
| `userName` | empty: no login | What a login may have: no `:` or control character |
| `rememberPassword` | off | Keep the password in the system keychain ([ADR-0015](decisions/0015-keychain-with-consent.md)) |

- **The password** is set on its own (`set_proxy_password`) and never comes back to the window, which only learns whether one is set (`get_proxy_password_set`). It is held in memory, and saved in the keychain only while `rememberPassword` is on; turning it off deletes it there (PO decision on #192). A keychain that refuses is said (`KEYCHAIN_UNAVAILABLE`), and the password is still used until the app closes.
- **What it covers:** every direct download (HTTP, HTTPS, FTP, SFTP) that starts, resumes or retries after a change, and the torrent helper at once. SFTP and torrents can't use an HTTPS proxy: SFTP downloads fail with a message saying so, and no torrent starts (`PROXY_UNSUPPORTED`). How each engine uses it is in [ENGINE_INTERNALS.md](ENGINE_INTERNALS.md#proxy-192) and [TORRENTS.md](TORRENTS.md#settings-and-identity).
- **When the app starts** the torrent helper gets the proxy, with a remembered password, before anything else, and the transfer engine before the first download starts.

## The window


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

V1's `general`, `notifications` and `appearance` settings and its first-run flag, in [`crates/velocity-core/src/app_settings.rs`](../crates/velocity-core/src/app_settings.rs) (#29). Each is a section of the settings document, on the Settings screen (#30). What the window does with them is in [UI_ARCHITECTURE.md](UI_ARCHITECTURE.md).

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
- **Read when the app starts:** `general` and `notifications` are read when the app starts. A section that can't be read is left as it was, and its defaults apply until it is fixed, so closing and notifications keep working. The Settings screen (#30) changes them while the app runs, and closing and notifications follow at once.

## What a build can do

Settings and details never offer what the build can't do. The engine reports its capabilities per protocol (`ProtocolCapabilities`, [CONTRACTS.md](CONTRACTS.md)): whether it downloads the protocol at all, and whether it segments, resumes, checks validators, uses HTTP/2 and can encrypt it. In the UI, `availability` ([`src/capabilities.ts`](../src/capabilities.ts)) turns that into "available" or a reason, and `CapabilityOption` shows a control that is disabled, with the reason beside it, when the build lacks something; the Settings screen's Metalink switch is one, disabled only with an engine that can't read Metalink (#30).
