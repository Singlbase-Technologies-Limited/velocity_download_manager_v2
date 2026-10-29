# Logs, redaction and diagnostics

What V2 records about itself, how secrets are kept out of it, and what the user can share (#28). Privacy as a whole is in [PRIVACY.md](PRIVACY.md).

## The app's log

- **Where:** in memory only. `velocity_core::logs::AppLog` keeps the last 1,000 lines, each cut at 2,000 characters. Nothing is written to a log file, a terminal or the network.
- **What:** the app's `tracing` events of level info, warn and error. Debug and trace events are dropped. Today they say:
  - that the app started, and its version;
  - when an engine couldn't start;
  - each change of the transfer engine's or the torrent helper's health;
  - each change of the storage error;
  - when diagnostics are exported, or couldn't be.
- **How:** the app installs `AppLog`'s layer as the global `tracing` subscriber when it starts (`src-tauri/src/diagnostics.rs`). Each line is redacted before it is kept.

## The engines' logs

The transfer engine and the torrent helper are child processes ([ENGINE_PROTOCOL.md](ENGINE_PROTOCOL.md)). Their stderr is kept in memory by `velocity_child::log::ChildLog`: the last 500 lines, each cut at 2,000 characters, and each redacted before it is kept. The manager reads them through `DirectTransfer::recent_log` and `TorrentEngine::recent_log`.

## Redaction

`velocity_core::redact::redact`, ported from V1's `redact.rs` and extended, hides:

| What | Example | Becomes |
|---|---|---|
| A URL's login, a proxy's included, up to the last `@` of the host part | `socks5h://alice:pw@proxy:1080` | `socks5h://[redacted]@proxy:1080` |
| Authorization, cookie and API-key headers, libcurl's debug lines included | `> Proxy-Authorization: Basic …` | `> Proxy-Authorization: [redacted]` |
| Passwords, passphrases and secrets, JSON's included | `"password":"p@ss"` | `"password":[redacted]` |
| Token-like query values and assignments | `?signature=…`, `access_token=…` | `?signature=[redacted]` |
| Private keys (PEM and OpenSSH) | `-----BEGIN … PRIVATE KEY----- …` | `[redacted private key]` |
| Magnet links | `magnet:?xt=…` | `magnet:[redacted]` |
| A tracker URL's path and query, where passkeys live | `https://t.test/abc/announce` | `https://t.test/[redacted]` |
| Secrets registered this session (`register_secret`, at least 8 characters, at most 64 kept) | | `[redacted]` |

Hosts, ordinary paths and queries, and SSH key fingerprints (`SHA256:…`, which identify public keys) are kept, so a report still says which server failed. Its tests are in `crates/velocity-core/src/redact.rs`.

## Diagnostics

`get_diagnostics` returns a `DiagnosticReport` (`velocity_core::diagnostics`, [CONTRACTS.md](CONTRACTS.md)):

- the app's name, version, contract version, operating system and processor;
- the transfer engine's health and capabilities, with the libcurl it runs on and its TLS backend;
- the torrent helper's health and libtorrent version;
- how many downloads are in the list, by status (no names, links or folders);
- the storage error, if any;
- the app's log and both engines' logs.

**Exporting:** `export_diagnostics` asks where to save, with the system's chooser, and writes the report as indented JSON. Each string in it is redacted once more, so nothing a part missed is written, and the JSON stays whole. The test `an_export_holds_no_secret_from_any_log` plants a secret of each kind in every log and checks that none reaches the export.

**Where it is shown:** `src/features/diagnostics/DiagnosticsPanel.tsx`, under Settings, About and diagnostics (#30; PO decision on #28). It shows the report, both engines' "Restart" buttons, and "Export diagnostics…". It has no "Copy", unlike V1's, so the export is the only way out and it always runs the final redaction.

## Crash logs

The crash policy (#28): **local crash logs only.** Any telemetry or crash upload needs a separate PO decision.

- When the app panics, `src-tauri/src/diagnostics.rs` logs it as an error, writes a crash log and then lets the default panic handling run.
- **The crash log** (`velocity_core::crash::write_crash_log`):
  - a file `crash-<milliseconds>.txt` in the app's data folder, under `crash-logs` ([COEXISTENCE.md](COEXISTENCE.md));
  - it holds the version, the panic message and location, and a backtrace, all redacted;
  - it is at most 64 KiB, written beside the folder's other files and then renamed into place;
  - only the newest 5 are kept, and nothing else in the folder is touched.
- **Nothing is sent.** The user can attach a crash log to a report by hand.

## Not here yet

- A log file on disk, and log levels the user can choose.
- The automated secret-leak scan across the database, checkpoints, temporary files and child arguments (#71).
